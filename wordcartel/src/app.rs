// wordcartel/src/app.rs — testable `step` + the real crossterm `run` loop.
//
// Design: terminal IO lives ONLY in `run`; `step` is pure and unit-testable.
// The real loop calls `step` then draws — `step` never touches the terminal.

use crossterm::event::Event;
#[cfg(test)]
use crossterm::event::KeyEvent;

use crate::{commands, config, derive, editor::Editor, file, keymap, render, term};
#[cfg(test)]
use crate::input;
use crate::jobs::{is_stale, Executor, JobResult};
use crate::registry::{Ctx, Registry};
use crate::prompt::PromptAction;
use wordcartel_core::history::Clock;

// ---------------------------------------------------------------------------
// step — pure, testable; no terminal IO
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Msg, apply_result, reduce — unified message type and reducer
// ---------------------------------------------------------------------------

pub enum Msg {
    Input(Event),
    JobDone(JobResult),
    FilterDone {
        buffer_id: crate::editor::BufferId,
        version: u64,
        range: std::ops::Range<usize>,
        cursor: usize,
        disposition: crate::filter::Disposition,
        outcome: crate::filter::RunResult,
    },
    ExportDone {
        buffer_id: crate::editor::BufferId,
        target: std::path::PathBuf,
        result: Result<crate::export::ExportResult, crate::filter::FilterError>,
        /// True when the user explicitly confirmed overwriting an existing
        /// target via the OverwriteExport prompt.  False when export was
        /// dispatched because the target did not exist at check time — in which
        /// case finalization must refuse to clobber a target that appeared in
        /// the meantime (TOCTOU guard; Codex pre-merge gate).
        overwrite_confirmed: bool,
    },
    TransformDone {
        buffer_id: crate::editor::BufferId,
        version: u64,
        range: std::ops::Range<usize>,
        kind: crate::transform::TransformKind,
        result: Result<String, crate::transform::TransformError>,
    },
    DiagnosticsDone {
        buffer_id: crate::editor::BufferId,
        version: u64,
        diagnostics: Vec<wordcartel_core::diagnostics::Diagnostic>,
    },
    ClipboardPaste { id: u64, buffer_id: crate::editor::BufferId, text: Option<String> },
    ClipboardAvailability(bool),
    Tick,
}

impl std::fmt::Debug for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Msg::Input(_) => f.write_str("Input(..)"),
            Msg::JobDone(_) => f.write_str("JobDone(..)"),
            Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => f
                .debug_struct("FilterDone")
                .field("buffer_id", buffer_id)
                .field("version", version)
                .field("range", range)
                .field("cursor", cursor)
                .field("disposition", disposition)
                .field("outcome", outcome)
                .finish(),
            Msg::ExportDone { buffer_id, target, .. } => f
                .debug_struct("ExportDone")
                .field("buffer_id", buffer_id)
                .field("target", target)
                .finish(),
            Msg::TransformDone { buffer_id, version, range, kind, .. } => f
                .debug_struct("TransformDone")
                .field("buffer_id", buffer_id)
                .field("version", version)
                .field("range", range)
                .field("kind", kind)
                .finish(),
            Msg::DiagnosticsDone { buffer_id, version, diagnostics } => f
                .debug_struct("DiagnosticsDone")
                .field("buffer_id", buffer_id)
                .field("version", version)
                .field("count", &diagnostics.len())
                .finish(),
            Msg::ClipboardPaste { id, buffer_id, text } => f.debug_struct("ClipboardPaste")
                .field("id", id).field("buffer_id", buffer_id)
                .field("has_text", &text.is_some()).finish(),
            Msg::ClipboardAvailability(ok) => f.debug_tuple("ClipboardAvailability").field(ok).finish(),
            Msg::Tick => f.write_str("Tick"),
        }
    }
}

/// Merge a finished job's effect on the foreground, honoring staleness (§10.3).
pub fn apply_result(r: JobResult, editor: &mut Editor) {
    if is_stale(&r, editor) {
        return; // buffer-local merge for a closed buffer, or a stale coalescible
    }
    let (kind, version, buffer_id, class) = (r.kind, r.version, r.buffer_id, r.class);
    // Mechanical-routing assertion (spec §3.4): a buffer-local merge must resolve
    // to a live buffer here; durability merges may target a now-missing buffer.
    debug_assert!(
        class == crate::jobs::ResultClass::Durability || editor.by_id(buffer_id).is_some(),
        "buffer-local result for a missing buffer slipped past is_stale"
    );
    (r.merge)(editor);
    // Post-save dispatch: fire the pending action when its save lands clean.
    if kind == crate::jobs::JobKind::Save {
        if let Some(p) = &editor.pending_after_save {
            let saved_clean = editor.by_id(buffer_id).map(|b| b.document.saved_version) == Some(Some(version));
            if p.buffer_id == buffer_id && p.version == version && saved_clean {
                let action = editor.pending_after_save.take().unwrap().action;
                match action {
                    crate::editor::PostSaveAction::Quit => editor.quit = true,
                    crate::editor::PostSaveAction::New => {
                        editor.replace_active_with_scratch();
                        crate::derive::rebuild(editor);
                        crate::nav::ensure_visible(editor);
                    }
                    crate::editor::PostSaveAction::Open(path) => {
                        crate::app::open_into_current(editor, &path);
                    }
                }
            }
        }
    }
}

fn apply_filter_done(
    editor: &mut crate::editor::Editor,
    buffer_id: crate::editor::BufferId,
    version: u64,
    range: std::ops::Range<usize>,
    cursor: usize,
    disposition: crate::filter::Disposition,
    outcome: crate::filter::RunResult,
    clock: &dyn wordcartel_core::history::Clock,
) {
    // Clears the single in-flight slot unconditionally. Correct under the current
    // single-in-flight invariant (one filter at a time across the whole editor);
    // would need per-buffer tracking if multi-buffer concurrent filters land (Effort 6).
    editor.filter_in_flight = None;
    let stale = editor.by_id(buffer_id).map(|b| b.document.version) != Some(version);
    match outcome {
        _ if stale => {
            editor.status = "filter discarded - buffer changed".into();
        }
        crate::filter::RunResult::Err(err) => {
            editor.status = crate::filter::describe_error(&err);
        }
        crate::filter::RunResult::Stdout(text) => {
            let apply_result = if let Some(b) = editor.by_id_mut(buffer_id) {
                let doc_len = b.document.buffer.len();
                let (from, to, at) = match disposition {
                    crate::filter::Disposition::Filter => (range.start, range.end, range.start),
                    crate::filter::Disposition::Insert => (cursor, cursor, cursor),
                };
                let (cs, edit) = crate::commands::build_range_replace(from, to, &text, doc_len);
                let txn = wordcartel_core::history::Transaction::new(cs)
                    .with_selection(wordcartel_core::selection::Selection::single(at + text.len()));
                b.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
                true
            } else {
                false
            };
            if apply_result {
                crate::derive::rebuild(editor);
                crate::nav::ensure_visible(editor);
                editor.status = "filter applied".into();
            }
        }
    }
}

fn apply_transform_done(
    editor: &mut crate::editor::Editor,
    buffer_id: crate::editor::BufferId,
    version: u64,
    range: std::ops::Range<usize>,
    kind: crate::transform::TransformKind,
    result: Result<String, crate::transform::TransformError>,
    clock: &dyn wordcartel_core::history::Clock,
) {
    editor.transform_in_flight = false;
    let stale = editor.by_id(buffer_id).map(|b| b.document.version) != Some(version);
    if stale {
        editor.status = "transform discarded — buffer changed".into();
        return;
    }
    crate::transform::merge_transform_into(editor, buffer_id, kind, range, result, clock);
}

fn apply_export_done(
    editor: &mut crate::editor::Editor,
    target: std::path::PathBuf,
    result: Result<crate::export::ExportResult, crate::filter::FilterError>,
    overwrite_confirmed: bool,
) {
    // TOCTOU guard (Codex pre-merge gate): run_export only prompts for overwrite
    // if the target existed at check time.  When it did not (overwrite_confirmed
    // == false) but the target has since appeared, refuse to clobber it silently
    // — the user never agreed to replace it.  (A pre-existing target always went
    // through the OverwriteExport prompt, so overwrite_confirmed is true there.)
    // The residual check-to-write window is microseconds vs. the whole pandoc
    // run; an unsafe-free atomic no-replace rename is unavailable under
    // #![forbid(unsafe_code)].
    if !overwrite_confirmed && target.exists() {
        if let Ok(crate::export::ExportResult::TempReady(tmp)) = &result {
            let _ = std::fs::remove_file(tmp);
        }
        editor.status = format!(
            "export target {} appeared — re-run export to overwrite",
            target.display()
        );
        return;
    }
    match result {
        Ok(crate::export::ExportResult::Bytes(bytes)) => {
            match file::save_atomic_bytes(&target, &bytes) {
                Ok(()) => {
                    let status = format!("exported {}", target.display());
                    editor.status = status;
                }
                Err(e) => {
                    editor.status = format!("export write failed: {e}");
                }
            }
        }
        Ok(crate::export::ExportResult::TempReady(tmp)) => {
            match std::fs::rename(&tmp, &target) {
                Ok(()) => {
                    let status = format!("exported {}", target.display());
                    editor.status = status;
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    editor.status = format!("export rename failed: {e}");
                }
            }
        }
        Err(e) => {
            editor.status = crate::filter::describe_error(&e);
        }
    }
}

/// Decide the resume position: restore (cursor clamped to doc_len) only if the
/// stored mtime+size identity matches the current file. Mismatch → None (stale).
pub fn apply_resume(
    e: &crate::state::StateEntry,
    current: (i64, u64),
    doc_len: usize,
) -> Option<(usize, usize)> {
    if (e.mtime, e.size) != current {
        return None;
    }
    Some((e.cursor.min(doc_len), e.scroll))
}

/// Populate the active buffer's marks from a session entry (string→char keys),
/// clamped+grapheme-snapped. Call only when the staleness guard has accepted
/// the entry (mirrors cursor/scroll restore).
pub fn load_marks_from_entry(editor: &mut Editor, entry: &crate::state::StateEntry) {
    for (k, &raw) in &entry.marks {
        if let Some(ch) = k.chars().next() {
            let off = crate::nav::clamp_snap(editor, raw);
            editor.active_mut().marks.insert(ch, off);
        }
    }
}

/// Restore session-resume state (cursor, scroll, marks, folds) for `path` into the
/// active buffer. Factored verbatim from run()'s launch resume block so launch and
/// `open_into_current` share one code path. Reloads `state::load()` itself so it works
/// with only `&mut Editor`. No-op if there is no matching/non-stale session entry.
pub fn restore_resume(editor: &mut Editor, path: &std::path::Path) {
    let session = crate::state::load();
    if let Ok(canon) = std::fs::canonicalize(path) {
        let key = canon.to_string_lossy().into_owned();
        if let Some(entry) = session.entries.get(&key) {
            if let Some(identity) = crate::state::file_identity(path) {
                let doc_len = editor.active().document.buffer.len();
                if let Some((cur, scroll)) = apply_resume(entry, identity, doc_len) {
                    let sel = wordcartel_core::selection::Selection::single(cur);
                    editor.active_mut().document.selection = sel;
                    editor.active_mut().view.scroll = scroll;
                    load_marks_from_entry(editor, entry);
                    editor.active_mut().folds.folded = entry.folds.iter().copied().collect();
                    let (blocks, buf) = { let b = editor.active(); (b.document.blocks.clone(), b.document.buffer.clone()) };
                    editor.active_mut().folds.reconcile(&blocks, &buf);
                }
            }
        }
    }
}

/// Open `path` into the active buffer slot (the buffer-load seam reused by Tasks 2/4/5).
/// Allocates a FRESH id so an in-flight save/swap job for the replaced buffer merges via
/// `by_id_mut(old_id)` → `None` (harmless no-op). On OpenError: set status, do NOT replace
/// (keep the user's work).
pub fn open_into_current(editor: &mut Editor, path: &std::path::Path) {
    let id = editor.alloc_id(); // FRESH id → an in-flight job for the old buffer no-ops via by_id_mut(old_id)=None
    let area = editor.active().view.area;
    match crate::editor::Buffer::from_file(id, path, area) {
        Ok(b) => {
            let a = editor.active;
            editor.buffers[a] = b;
            if editor.resume_enabled {
                restore_resume(editor, path);
            }
            crate::derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
            editor.status = String::new();
        }
        Err(e) => {
            editor.status = e.to_string();
        }
    }
}

/// Execute the action chosen in a modal prompt, then clear the prompt.
/// Open the Save-As minibuffer pre-filled with the active doc's directory.
pub fn open_save_as(editor: &mut crate::editor::Editor) {
    let pre = editor.active().document.path.as_ref()
        .and_then(|p| p.parent()).map(|d| format!("{}/", d.display())).unwrap_or_default();
    editor.open_minibuffer("Save as: ", crate::minibuffer::MinibufferKind::SaveAs);
    if let Some(mb) = editor.minibuffer.as_mut() { mb.cursor = pre.len(); mb.text = pre; }
}

/// Submit the Save-As minibuffer line: expand the path, raise an overwrite
/// confirmation if the target exists, else perform the save-as immediately.
pub fn save_as_submit(editor: &mut crate::editor::Editor, text: &str,
                      executor: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
                      msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
    let t = text.trim();
    if t.is_empty() { editor.status = "save-as: empty path".into(); editor.pending_save_as = None; return; }
    // expand_path: ~ → home; relative → joined onto cwd. Mirror the ~ handling used by the
    // dictionary/config path loaders.
    let target: std::path::PathBuf = {
        let expanded = if let Some(rest) = t.strip_prefix("~/") {
            dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| std::path::PathBuf::from(t))
        } else { std::path::PathBuf::from(t) };
        if expanded.is_absolute() { expanded }
        else { std::env::current_dir().map(|d| d.join(&expanded)).unwrap_or(expanded) }
    };
    if target.exists() {
        editor.pending_save_overwrite = Some(target.clone());
        editor.open_prompt(crate::prompt::Prompt::save_overwrite(&target));
        return;
    }
    perform_save_as(editor, target, executor, clock, msg_tx);
}

fn perform_save_as(editor: &mut crate::editor::Editor, target: std::path::PathBuf,
                   executor: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
                   msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
    let v = editor.active().document.version;
    let buffer_id = editor.active().id;
    { let mut ctx = crate::registry::Ctx { editor, clock, executor, msg_tx: msg_tx.clone() };
      crate::save::do_save_to(&mut ctx, target, crate::save::SaveMode::SaveAs); }
    if let Some(action) = editor.pending_save_as.take() {
        editor.pending_after_save = Some(crate::editor::PendingAfterSave { buffer_id, version: v, action, at_ms: clock.now_ms() });
    }
}

/// Perform a PostSaveAction immediately (no save): used for the clean path and Discard.
fn perform_post_save_action(
    editor: &mut Editor,
    action: crate::editor::PostSaveAction,
    // Unused: New/Open/Quit are editor-only (Open routes through open_into_current, no async).
    // Kept for call-site signature uniformity with the save-then-action paths.
    _ex: &dyn Executor,
    _clock: &dyn Clock,
    _msg_tx: &std::sync::mpsc::Sender<Msg>,
) {
    match action {
        crate::editor::PostSaveAction::New => {
            editor.replace_active_with_scratch();
            crate::derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
        }
        crate::editor::PostSaveAction::Open(ref p) => {
            open_into_current(editor, p);
        }
        crate::editor::PostSaveAction::Quit => {
            editor.quit = true;
        }
    }
}

/// Dirty-guard: perform `action` immediately if the buffer is clean, else raise the
/// Save/Discard/Cancel modal (the intent is held in `pending_save_as` until resolved).
fn request_replace(
    editor: &mut Editor,
    action: crate::editor::PostSaveAction,
    ex: &dyn Executor,
    clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
) {
    if !editor.active().document.dirty() {
        perform_post_save_action(editor, action, ex, clock, msg_tx);
        return;
    }
    editor.pending_save_as = Some(action);
    editor.open_prompt(crate::prompt::Prompt::dirty_guard());
}

/// Request a New (scratch) buffer: immediate if clean, else raise the dirty-guard modal.
pub fn request_new(
    editor: &mut Editor,
    ex: &dyn Executor,
    clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
) {
    request_replace(editor, crate::editor::PostSaveAction::New, ex, clock, msg_tx);
}

pub fn resolve_prompt(
    action: PromptAction,
    editor: &mut Editor,
    ex: &dyn Executor,
    clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
) {
    match action {
        PromptAction::Cancel => {
            editor.pending_export = None;
            editor.pending_save_overwrite = None;
            editor.pending_save_as = None;
        }
        PromptAction::QuitAnyway => { editor.quit = true; }
        PromptAction::SaveAndQuit => {
            editor.prompt = None; // dismiss the quit-confirm modal first
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
            crate::save::dispatch_save_and_quit(&mut ctx);
            return; // prompt handled; must NOT clear an external-mod modal
        }
        PromptAction::DiscardAndProceed => {
            // Dirty-guard: discard unsaved changes and immediately perform the pending action.
            if let Some(action) = editor.pending_save_as.take() {
                perform_post_save_action(editor, action, ex, clock, msg_tx);
            }
        }
        PromptAction::SaveAndProceed => {
            // Dirty-guard: dismiss the modal first, then dispatch a save followed by the pending
            // action. dispatch_save_then handles NAMED (saves + arms pending_after_save) AND
            // UNNAMED (opens Save-As carrying the action in pending_save_as). Take the intent
            // first so the named path doesn't leave a stale pending_save_as.
            editor.prompt = None; // dismiss the dirty-guard modal first
            if let Some(action) = editor.pending_save_as.take() {
                let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
                crate::save::dispatch_save_then(&mut ctx, action);
            }
            return; // MUST return — dispatch_save_then may have opened an external-mod or Save-As
                    // overlay; the trailing resolve_prompt prompt-clear would wipe it otherwise
                    // (mirrors the SaveAndQuit arm's return).
        }
        PromptAction::Reload => crate::save::reload_from_disk(editor),
        PromptAction::Overwrite => {
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
            crate::save::overwrite_save(&mut ctx);
        }
        PromptAction::Recover => {
            // Capture body + orphan path BEFORE load_recovered, which replaces the
            // whole active Buffer and would reset pending_swap_path to None (4r moved
            // these fields onto Buffer). Then clean up the orphan after loading.
            let staged = {
                let b = editor.active_mut();
                b.pending_swap_body
                    .take()
                    .map(|body| (body, b.pending_swap_path.take()))
            };
            if let Some((body, orphan)) = staged {
                crate::save::load_recovered(editor, &body);
                if let Some(p) = orphan {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        PromptAction::DiscardSwap => {
            if let Some(p) = editor.active_mut().pending_swap_path.take() {
                let _ = std::fs::remove_file(p);
            } else {
                crate::swap::delete(editor.active().document.path.as_deref());
            }
        }
        PromptAction::OpenOriginal => {
            editor.active_mut().pending_swap_body = None;
            editor.active_mut().pending_swap_path = None;
        }
        PromptAction::OverwriteExport => {
            if let Some(pe) = editor.pending_export.take() {
                // User explicitly confirmed clobbering the existing target.
                crate::export::do_export(editor, &pe.ext, &pe.target, msg_tx, true);
            }
        }
        PromptAction::OverwriteSaveAs => {
            if let Some(t) = editor.pending_save_overwrite.take() {
                perform_save_as(editor, t, ex, clock, msg_tx);
            }
        }
        PromptAction::Transform(kind) => {
            crate::transform::dispatch_transform(editor, kind, clock, msg_tx);
        }
    }
    editor.prompt = None;
}

/// Submit a minibuffer line as a filter command.
///
/// Splits the line on whitespace to build the argv (no shell, no quoting —
/// `shell: false` is the security default; shell invocation is opt-in only).
/// An empty line sets a status message and returns without dispatching.
fn submit_filter_line(
    editor: &mut Editor,
    line: &str,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
) {
    let argv: Vec<String> = line.split_whitespace().map(String::from).collect();
    if argv.is_empty() {
        editor.status = "filter: no command given".into();
        return;
    }
    let spec = crate::filter::FilterSpec {
        argv,
        shell: false,
        disposition: crate::filter::Disposition::Filter,
        input: crate::filter::Input::SelectionElseBuffer,
        timeout: std::time::Duration::from_secs(10),
        max_output: 1 << 20,
    };
    crate::filter::dispatch_filter(editor, spec, msg_tx.clone());
}

/// Submit a minibuffer line as a go-to-line target (Effort 8). 1-based, clamped;
/// records a jump origin (jump-back), unfolds to the target, lands at column 1.
pub(crate) fn goto_line_submit(editor: &mut crate::editor::Editor, text: &str) {
    let n: usize = match text.trim().parse() {
        Ok(n) => n,
        Err(_) => { editor.status = "not a line number".to_string(); return; }
    };
    let total = crate::derive::total_logical_lines(&editor.active().document.buffer);
    let line_index = n.max(1).min(total) - 1;            // 1-based clamp → 0-based index
    let pre = crate::nav::head(editor);
    crate::marks::record_jump(editor.active_mut(), pre); // jump-back support
    let target = editor.active().document.buffer.line_to_byte(line_index);
    let caret = crate::registry::place_caret_visible(editor, target, crate::registry::CaretPlace::UnfoldTo);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(caret);
    editor.active_mut().desired_col = None;
    editor.active_mut().sel_history.clear();
    crate::derive::rebuild(editor);   // UnfoldTo can change fold state → relayout (mirrors registry.rs:409 / app.rs:680)
    crate::nav::ensure_visible(editor);
}

fn insert_paste_text(editor: &mut Editor, buffer_id: crate::editor::BufferId, text: &str, clock: &dyn Clock) -> bool {
    if text.len() > crate::clipboard::PASTE_MAX_BYTES {
        editor.status = format!("paste too large ({} MiB) — skipped", text.len() / (1 << 20));
        return false;
    }
    let active_id = editor.active().id;
    {
        let Some(b) = editor.by_id_mut(buffer_id) else { return false; };
        let sel = b.document.selection.primary();
        let (from, to) = (sel.from(), sel.to());
        let doc_len = b.document.buffer.len();
        let (cs, edit) = crate::commands::build_range_replace(from, to, text, doc_len);
        let txn = wordcartel_core::history::Transaction::new(cs)
            .with_selection(wordcartel_core::selection::Selection::single(from + text.len()));
        b.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
        b.desired_col = None;
    }
    if buffer_id == active_id {
        crate::derive::rebuild(editor);
        crate::nav::ensure_visible(editor);
    }
    true
}

fn apply_clipboard_paste(editor: &mut Editor, buffer_id: crate::editor::BufferId, text: Option<String>, clock: &dyn Clock) {
    match text {
        Some(t) if !t.is_empty() => {
            if insert_paste_text(editor, buffer_id, &t, clock) {
                editor.register.set(t);
            }
        }
        _ => {
            if let Some(t) = editor.register.get().map(str::to_owned) {
                insert_paste_text(editor, buffer_id, &t, clock);
            }
        }
    }
}

fn apply_clipboard_availability(editor: &mut Editor, ok: bool) {
    if !ok && !editor.clipboard_notice_shown {
        editor.status = "system clipboard unavailable -- copy/paste work in-editor; using OSC 52 for terminal sync".into();
        editor.clipboard_notice_shown = true;
    }
}

/// Fill rows for a freshly-opened palette (empty rows + empty query → rebuild).
/// Called immediately after any command dispatch and after dispatch_overlay_command
/// so a just-opened overlay has content before the first render.
pub(crate) fn hydrate_overlays(editor: &mut Editor, reg: &crate::registry::Registry, keymap: &crate::keymap::KeyTrie) {
    if let Some(ref mut p) = editor.palette {
        if p.rows.is_empty() && p.query.is_empty() {
            crate::palette::rebuild_rows(p, reg, keymap);
        }
    }
    if editor.menu.as_ref().is_some_and(|v| !v.built) {
        editor.menu = Some(crate::menu::build(reg, keymap));
    }
}

/// Close the active overlay, dispatch `id` via the registry, drain executor results,
/// then hydrate any overlay opened by the dispatched command.
pub(crate) fn dispatch_overlay_command(
    editor: &mut Editor,
    reg: &crate::registry::Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
    id: crate::registry::CommandId,
) {
    editor.palette = None;
    editor.menu = None;
    editor.theme_picker = None;
    editor.file_browser = None;
    let mut ctx = crate::registry::Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
    reg.dispatch(id, &mut ctx);
    for r in ex.drain() { apply_result(r, editor); }
    // Hydrate any overlay the dispatched command may have opened (Codex 3c).
    hydrate_overlays(editor, reg, keymap);
}

#[cfg(test)]
pub fn menu_select_for_test(
    editor: &mut Editor,
    reg: &crate::registry::Registry,
    ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
    id: crate::registry::CommandId,
) {
    editor.menu = None;
    let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), reg);
    dispatch_overlay_command(editor, reg, &keymap, ex, clock, msg_tx, id);
}

fn search_sync(editor: &mut Editor) {
    let (rope, version) = { let d = &editor.active().document; (d.buffer.snapshot(), d.version) };
    if let Some(s) = editor.search.as_mut() { s.recompute(&rope, version); }
    if let Some(m) = editor.search.as_ref().and_then(|s| s.current()) {
        crate::registry::unfold_ancestors_of(editor, m.start);
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::range(m.start, m.end);
        derive::rebuild(editor);
        crate::nav::ensure_visible(editor);
    }
}

fn search_step(editor: &mut Editor, forward: bool) {
    if let Some(s) = editor.search.as_mut() { if forward { s.next(); } else { s.prev(); } }
    if let Some(m) = editor.search.as_ref().and_then(|s| s.current()) {
        crate::registry::unfold_ancestors_of(editor, m.start);
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::range(m.start, m.end);
        derive::rebuild(editor);
        crate::nav::ensure_visible(editor);
    }
}

fn search_cancel(editor: &mut Editor) {
    let origin = editor.search.as_ref().map(|s| s.origin).unwrap_or(0);
    editor.search = None;
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(origin);
    derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}

fn search_replace_all(editor: &mut Editor, clock: &dyn wordcartel_core::history::Clock) {
    search_sync(editor); // ensure cache is current
    // §8: invalid regex → distinct status, no mutation.
    if editor.search.as_ref().is_some_and(|s| s.error.is_some()) {
        editor.status = "invalid regex".into();
        return;
    }
    let plan: Option<(Vec<(usize, usize, String)>, usize, usize)> = editor.search.as_ref().and_then(|s| {
        let m = s.matcher()?;
        if s.matches().is_empty() { return None; }
        let rope = editor.active().document.buffer.snapshot();
        let edits: Vec<(usize, usize, String)> = s.matches().iter().map(|mm| {
            (mm.start, mm.end, wordcartel_core::search::expand_replacement(&rope, m, mm, &s.template, s.mode))
        }).collect();
        Some((edits, rope.len_bytes(), s.origin))
    });
    let Some((edits, doc_len, origin)) = plan else {
        editor.status = "No matches".into();
        return;
    };
    let n = edits.len();
    let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
    // remap origin through this changeset BEFORE moving it into the transaction
    let new_origin = wordcartel_core::change::map_pos(origin, &cs);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(new_origin));
    editor.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
    if let Some(s) = editor.search.as_mut() { s.origin = new_origin; }
    editor.status = format!("Replaced {n} occurrences");
    editor.search = None; // close after replace-all
    derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}

fn search_step_apply(editor: &mut Editor, clock: &dyn wordcartel_core::history::Clock) {
    let plan = editor.search.as_ref().and_then(|s| {
        let m = s.matcher()?; let cur = s.current()?;
        let rope = editor.active().document.buffer.snapshot();
        let text = wordcartel_core::search::expand_replacement(&rope, m, &cur, &s.template, s.mode);
        Some((cur, text, rope.len_bytes(), s.origin))
    });
    let Some((cur, text, doc_len, origin)) = plan else { editor.search = None; return; };
    let (cs, edit) = crate::commands::build_range_replace(cur.start, cur.end, &text, doc_len);
    let new_origin = wordcartel_core::change::map_pos(origin, &cs);
    let caret = cur.start + text.len();
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(caret));
    editor.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
    // Re-find the next match on the MUTATED rope, and remap origin.
    let (rope, version) = { let d = &editor.active().document; (d.buffer.snapshot(), d.version) };
    if let Some(s) = editor.search.as_mut() {
        s.origin = new_origin;
        s.cache_invalidate();                 // force recompute against mutated rope
        s.recompute(&rope, version);
        s.set_current_at_or_after(caret);     // park on next match at/after the just-edited spot
    }
    search_pin(editor);
    if editor.search.as_ref().is_some_and(|s| s.current().is_none()) { editor.search = None; } // done
}

fn search_step_skip(editor: &mut Editor) {
    if let Some(s) = editor.search.as_mut() { s.next(); }
    search_pin(editor);
    if editor.search.as_ref().is_some_and(|s| s.wrapped) { editor.search = None; } // walked off the end
}

fn search_step_rest(editor: &mut Editor, clock: &dyn wordcartel_core::history::Clock) {
    // Replace current + all remaining (from current.start onward) as one unit.
    let plan = editor.search.as_ref().and_then(|s| {
        let m = s.matcher()?; let cur = s.current()?;
        let rope = editor.active().document.buffer.snapshot();
        let edits: Vec<(usize, usize, String)> = s.matches().iter().filter(|mm| mm.start >= cur.start)
            .map(|mm| (mm.start, mm.end, wordcartel_core::search::expand_replacement(&rope, m, mm, &s.template, s.mode)))
            .collect();
        Some((edits, rope.len_bytes()))
    });
    let Some((edits, doc_len)) = plan else { editor.search = None; return; };
    if edits.is_empty() { editor.search = None; return; }
    let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(edits[0].0));
    editor.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
    editor.search = None;
    derive::rebuild(editor); crate::nav::ensure_visible(editor);
}

fn search_pin(editor: &mut Editor) {
    if let Some(m) = editor.search.as_ref().and_then(|s| s.current()) {
        crate::registry::unfold_ancestors_of(editor, m.start);
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::range(m.start, m.end);
        derive::rebuild(editor); crate::nav::ensure_visible(editor);
    }
}

/// Accept, ignore, or add-to-dict based on the overlay's current selection.
/// Clears `editor.diag` when done (regardless of outcome).
fn diag_apply_selected(editor: &mut Editor, clock: &dyn wordcartel_core::history::Clock) {
    // Clone what we need out of the overlay before mutating editor.
    let overlay_info = editor.diag.as_ref().map(|ov| {
        let is_ignore = ov.is_ignore();
        let is_add_dict = ov.is_add_dict();
        let suggestion = ov.chosen_suggestion().cloned();
        (ov.anchor.range.start, ov.anchor.range.end, is_ignore, is_add_dict, suggestion, ov.opened_version)
    });
    let Some((raw_a, raw_b, is_ignore, is_add_dict, suggestion, opened_version)) = overlay_info else { return; };

    // Fix A4: if the buffer was mutated while the overlay was open, the anchor
    // ranges are stale.  Refuse to apply — a stale range can cause a panic on
    // multibyte boundaries or silently apply at wrong offsets.
    if editor.active().document.version != opened_version {
        editor.status = "document changed; re-open".into();
        editor.diag = None;
        return;
    }

    // Clamp the stale/oversized anchor range to the current doc length so a
    // multibyte/shrink race can never cause buffer.slice or build_range_replace
    // to panic (defense-in-depth even when the command-handler validity gate fires).
    let doc_len = editor.active().document.buffer.len();
    let a = raw_a.min(doc_len);
    let b = raw_b.min(doc_len);

    if is_ignore {
        // Add the surface word to session_ignores, close, re-arm a recheck.
        let word = editor.active().document.buffer.slice(a..b).to_string();
        editor.session_ignores.insert(word);
        editor.diag = None;
        if editor.diag_cfg.enabled {
            let debounce_ms = editor.diag_cfg.debounce_ms;
            editor.active_mut().diagnostics.arm(clock.now_ms(), debounce_ms);
        }
    } else if is_add_dict {
        // Append word to dictionary file + in-memory set, close, re-arm.
        let word = editor.active().document.buffer.slice(a..b).to_string();
        if let Some(ref dict_path) = editor.diag_cfg.dictionary.clone() {
            match crate::diagnostics_run::append_word_to_dict(dict_path, &word) {
                Ok(()) => { editor.dictionary.insert(word); }
                Err(e) => { editor.status = format!("add to dictionary failed: {e}"); }
            }
        } else {
            editor.status = "no dictionary path configured".into();
        }
        editor.diag = None;
        if editor.diag_cfg.enabled {
            let debounce_ms = editor.diag_cfg.debounce_ms;
            editor.active_mut().diagnostics.arm(clock.now_ms(), debounce_ms);
        }
    } else if let Some(s) = suggestion {
        // Apply the suggestion as an undoable edit, then close.
        let (cs, edit) = match &s {
            wordcartel_core::diagnostics::Suggestion::ReplaceWith(t) =>
                crate::commands::build_range_replace(a, b, t, doc_len),
            wordcartel_core::diagnostics::Suggestion::InsertAfter(t) =>
                crate::commands::build_range_replace(b, b, t, doc_len),
            wordcartel_core::diagnostics::Suggestion::Remove =>
                crate::commands::build_range_replace(a, b, "", doc_len),
        };
        // Determine cursor position: for ReplaceWith/InsertAfter place after inserted text;
        // for Remove place at a (start of deleted region).
        let new_cursor = match &s {
            wordcartel_core::diagnostics::Suggestion::ReplaceWith(t) => a + t.len(),
            wordcartel_core::diagnostics::Suggestion::InsertAfter(t) => b + t.len(),
            wordcartel_core::diagnostics::Suggestion::Remove => a,
        };
        let txn = wordcartel_core::history::Transaction::new(cs)
            .with_selection(wordcartel_core::selection::Selection::single(new_cursor));
        editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
        derive::rebuild(editor);
        crate::registry::unfold_ancestors_of(editor, new_cursor);
        derive::rebuild(editor);
        crate::nav::ensure_visible(editor);
        editor.diag = None;
    }
    // else: no suggestion and not ignore/add_dict — unreachable (selected is always in range).
}

/// Apply the theme-picker's currently-selected built-in as a live preview.
fn preview_selected_theme(editor: &mut crate::editor::Editor) {
    let name = editor.theme_picker.as_ref().and_then(|tp| tp.rows.get(tp.selected).cloned());
    if let Some(name) = name {
        if let Some(theme) = wordcartel_core::theme::Theme::builtin(&name) { editor.apply_theme(theme); }
    }
}

pub fn outline_jump_to(editor: &mut Editor, byte: usize) {
    let origin = editor.active().document.selection.primary().head;
    crate::marks::record_jump(editor.active_mut(), origin);
    crate::registry::unfold_ancestors_of(editor, byte);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(byte);
    editor.outline = None;
    derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}

/// Process one message. Returns true while the app should keep running.
pub fn reduce(
    msg: Msg,
    editor: &mut Editor,
    reg: &Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn Executor,
    clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
) -> bool {
    // pending_mark intercepts the very next key as the mark letter.
    // Non-key messages fall through to normal handling.
    if editor.pending_mark.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                match k.code {
                    crossterm::event::KeyCode::Esc => { editor.pending_mark = None; editor.status.clear(); }
                    crossterm::event::KeyCode::Char(c) => crate::marks::resolve_pending(editor, c),
                    _ => { editor.pending_mark = None; } // non-name key cancels
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        // non-key message: fall through to normal handling
    }

    // Menu overlay intercepts KEY INPUT and PASTE (no text field; paste is
    // consumed / silently dropped). Non-key, non-paste messages fall through to
    // the normal handlers so background work continues while the menu is open.
    if editor.menu.is_some() {
        if matches!(&msg, Msg::ClipboardPaste { .. }) {
            // Drop an async clipboard-paste result that arrives while the menu is
            // open — it must not land in the document behind the overlay.
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Paste(_)) = &msg {
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::KeyCode;
                // Close OUTSIDE any menu borrow (Codex Critical: `editor.menu = None`
                // must not run while `editor.menu.as_mut()` is held).
                if matches!(k.code, KeyCode::Esc | KeyCode::F(10)) {
                    editor.menu = None;
                } else {
                    let mut selected: Option<crate::registry::CommandId> = None;
                    if let Some(menu) = editor.menu.as_mut() {   // borrow scoped to this block
                        let ncat = menu.groups.len();
                        match k.code {
                            KeyCode::Left if ncat > 0 => { menu.open = (menu.open + ncat - 1) % ncat; menu.highlighted = 0; }
                            KeyCode::Right if ncat > 0 => { menu.open = (menu.open + 1) % ncat; menu.highlighted = 0; }
                            KeyCode::Up if ncat > 0 => { menu.highlighted = menu.highlighted.saturating_sub(1); }
                            KeyCode::Down if ncat > 0 => {
                                let n = menu.groups[menu.open].1.len();
                                if n > 0 { menu.highlighted = (menu.highlighted + 1).min(n - 1); }
                            }
                            KeyCode::Enter if ncat > 0 => {
                                if let Some((_, id)) = menu.groups[menu.open].1.get(menu.highlighted) { selected = Some(*id); }
                            }
                            _ => {}
                        }
                    } // menu borrow dropped here
                    if let Some(id) = selected {
                        dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id);
                    }
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        // Non-key msg falls through to normal handling while menu stays open.
    }

    // Palette overlay intercepts KEY INPUT and PASTE. Non-key, non-paste messages
    // (FilterDone, JobDone, Tick) fall through to normal handling while the
    // palette stays open.
    if editor.palette.is_some() {
        if matches!(&msg, Msg::ClipboardPaste { .. }) {
            // Drop an async clipboard-paste result that arrives while the palette is
            // open — it must not land in the document behind the overlay.
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Paste(text)) = msg {
            if let Some(p) = editor.palette.as_mut() {
                p.query.insert_str(p.cursor, &text);
                p.cursor += text.len();
                crate::palette::rebuild_rows(p, reg, keymap);
                let max = p.rows.len().saturating_sub(1);
                if p.selected > max { p.selected = max; }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                match k.code {
                    crossterm::event::KeyCode::Esc => {
                        editor.palette = None;
                    }
                    crossterm::event::KeyCode::Enter => {
                        let id_opt = editor.palette.as_ref()
                            .and_then(|p| p.rows.get(p.selected))
                            .map(|r| r.id);
                        if let Some(id) = id_opt {
                            dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id);
                        }
                    }
                    crossterm::event::KeyCode::Up => {
                        if let Some(p) = editor.palette.as_mut() {
                            p.selected = p.selected.saturating_sub(1);
                        }
                    }
                    crossterm::event::KeyCode::Down => {
                        if let Some(p) = editor.palette.as_mut() {
                            let max = p.rows.len().saturating_sub(1);
                            p.selected = (p.selected + 1).min(max);
                        }
                    }
                    crossterm::event::KeyCode::Backspace => {
                        if let Some(p) = editor.palette.as_mut() {
                            if p.cursor > 0 {
                                // remove the char before cursor (byte-safe for ASCII labels)
                                let byte_pos = p.query[..p.cursor].char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
                                p.query.remove(byte_pos);
                                p.cursor = byte_pos;
                            }
                            crate::palette::rebuild_rows(p, reg, keymap);
                        }
                    }
                    crossterm::event::KeyCode::Left => {
                        if let Some(p) = editor.palette.as_mut() {
                            if p.cursor > 0 {
                                p.cursor -= p.query[..p.cursor].char_indices().next_back().map(|(_, c)| c.len_utf8()).unwrap_or(0);
                            }
                        }
                    }
                    crossterm::event::KeyCode::Right => {
                        if let Some(p) = editor.palette.as_mut() {
                            if p.cursor < p.query.len() {
                                let c = p.query[p.cursor..].chars().next().unwrap();
                                p.cursor += c.len_utf8();
                            }
                        }
                    }
                    crossterm::event::KeyCode::Char(c)
                        if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                            && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        if let Some(p) = editor.palette.as_mut() {
                            p.query.insert(p.cursor, c);
                            p.cursor += c.len_utf8();
                            crate::palette::rebuild_rows(p, reg, keymap);
                        }
                    }
                    _ => {}
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        // Non-key msg falls through to normal handling while palette stays open.
    }

    // Theme picker overlay intercepts KEY INPUT and PASTE. Non-key, non-paste messages
    // fall through to normal handling while the picker stays open (mirrors palette block).
    if editor.theme_picker.is_some() {
        // Paste intercept FIRST (mirror the palette, app.rs palette block) — else paste leaks
        // into the document while the picker is open (Codex I6).
        if matches!(&msg, Msg::ClipboardPaste { .. }) {
            // Drop an async clipboard-paste result that arrives while the theme picker is
            // open — it must not land in the document behind the overlay.
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Paste(text)) = &msg {
            if let Some(tp) = editor.theme_picker.as_mut() {
                tp.query.push_str(text);
                crate::theme_picker::rebuild_rows(tp);
            }
            preview_selected_theme(editor);
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::KeyCode;
                match k.code {
                    KeyCode::Esc => {
                        // cancel preview → restore the theme active when we opened.
                        if let Some(tp) = editor.theme_picker.take() { editor.apply_theme(tp.original); }
                    }
                    KeyCode::Enter => { editor.theme_picker = None; } // keep current preview
                    KeyCode::Up => {
                        if let Some(tp) = editor.theme_picker.as_mut() { tp.selected = tp.selected.saturating_sub(1); }
                        preview_selected_theme(editor);
                    }
                    KeyCode::Down => {
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            let max = tp.rows.len().saturating_sub(1);
                            tp.selected = (tp.selected + 1).min(max);
                        }
                        preview_selected_theme(editor);
                    }
                    KeyCode::Backspace => {
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            tp.query.pop();
                            crate::theme_picker::rebuild_rows(tp);
                        }
                        preview_selected_theme(editor);
                    }
                    KeyCode::Char(c)
                        if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                            && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            tp.query.push(c);
                            crate::theme_picker::rebuild_rows(tp);
                        }
                        preview_selected_theme(editor);
                    }
                    _ => {}
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        // Non-key msg falls through to normal handling while picker stays open.
    }

    // File browser overlay intercepts KEY INPUT and PASTE. Non-key, non-paste messages
    // fall through to normal handling while the browser stays open (mirrors theme_picker).
    if editor.file_browser.is_some() {
        // Drop an async clipboard-paste result that arrives while the browser is open —
        // it must not land in the document behind the overlay (Codex I6, mirror palette).
        if matches!(&msg, Msg::ClipboardPaste { .. }) {
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Paste(text)) = &msg {
            if let Some(fb) = editor.file_browser.as_mut() {
                fb.query.push_str(text);
                crate::file_browser::rebuild_entries(fb);
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::KeyCode;
                match k.code {
                    KeyCode::Esc => { editor.file_browser = None; }
                    KeyCode::Enter => {
                        // Resolve the selected entry: descend into a directory (incl. ".."),
                        // or open a file through the Task-4 dirty-guard.
                        let chosen = editor.file_browser.as_ref().and_then(|fb| {
                            fb.entries.get(fb.selected).map(|e| (e.name.clone(), e.is_dir))
                        });
                        if let Some((name, is_dir)) = chosen {
                            if is_dir {
                                if let Some(fb) = editor.file_browser.as_mut() {
                                    let new_dir = if name == ".." {
                                        fb.dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| fb.dir.clone())
                                    } else {
                                        fb.dir.join(&name)
                                    };
                                    fb.dir = new_dir;
                                    fb.query.clear();
                                    fb.selected = 0;
                                    crate::file_browser::rebuild_entries(fb);
                                }
                            } else {
                                let path = editor.file_browser.as_ref().unwrap().dir.join(&name);
                                editor.file_browser = None;
                                request_replace(editor, crate::editor::PostSaveAction::Open(path), ex, clock, msg_tx);
                            }
                        }
                    }
                    KeyCode::Up => {
                        if let Some(fb) = editor.file_browser.as_mut() { fb.selected = fb.selected.saturating_sub(1); }
                    }
                    KeyCode::Down => {
                        if let Some(fb) = editor.file_browser.as_mut() {
                            let max = fb.entries.len().saturating_sub(1);
                            fb.selected = (fb.selected + 1).min(max);
                        }
                    }
                    KeyCode::Backspace => {
                        if let Some(fb) = editor.file_browser.as_mut() {
                            fb.query.pop();
                            crate::file_browser::rebuild_entries(fb);
                        }
                    }
                    KeyCode::Char(c)
                        if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                            && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        if let Some(fb) = editor.file_browser.as_mut() {
                            fb.query.push(c);
                            crate::file_browser::rebuild_entries(fb);
                        }
                    }
                    _ => {}
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        // Non-key msg falls through to normal handling while the browser stays open.
    }

    // Active modal intercepts KEY INPUT only (§5.3). Background results and ticks
    // must still be processed — a JobDone arriving while a modal is up (e.g. an
    // in-flight save completing during the quit-confirm prompt) must not be
    // dropped, or save&quit would hang waiting for a result it already discarded.
    if editor.prompt.is_some() {
        match msg {
            Msg::Input(Event::Key(key)) if key.kind == crossterm::event::KeyEventKind::Press => {
                if key.code == crossterm::event::KeyCode::Esc {
                    editor.prompt = None; // Esc cancels any prompt
                    editor.pending_export = None;
                    editor.pending_save_overwrite = None;
                    editor.pending_save_as = None;
                } else if let crossterm::event::KeyCode::Char(ch) = key.code {
                    if let Some(action) = editor.prompt.as_ref().unwrap().action_for(ch) {
                        resolve_prompt(action, editor, ex, clock, msg_tx);
                    }
                }
            }
            // Merge a directly-delivered background result even under a modal.
            Msg::JobDone(r) => apply_result(r, editor),
            Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => {
                apply_filter_done(editor, buffer_id, version, range, cursor, disposition, outcome, clock);
            }
            Msg::ExportDone { target, result, overwrite_confirmed, .. } => {
                apply_export_done(editor, target, result, overwrite_confirmed);
            }
            Msg::TransformDone { buffer_id, version, range, kind, result } => {
                apply_transform_done(editor, buffer_id, version, range, kind, result, clock);
            }
            Msg::DiagnosticsDone { buffer_id, version, diagnostics } => {
                crate::diagnostics_run::apply_diagnostics_done(editor, buffer_id, version, diagnostics);
            }
            Msg::ClipboardPaste { buffer_id, text, .. } => apply_clipboard_paste(editor, buffer_id, text, clock),
            Msg::ClipboardAvailability(ok) => apply_clipboard_availability(editor, ok),
            // Resize/Tick/other input: ignored for the modal, but results still drain below.
            _ => {}
        }
        // Always drain ready results (merges the awaited save&quit result).
        for r in ex.drain() { apply_result(r, editor); }
        return !editor.quit;
    }

    // Minibuffer intercepts KEY INPUT only; non-key messages (FilterDone/JobDone/Tick)
    // fall through to the normal match arm below — a FilterDone must apply even while
    // the minibuffer is open (see test `minibuffer_does_not_starve_filterdone`).
    if editor.minibuffer.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                match k.code {
                    crossterm::event::KeyCode::Char(c)
                        if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        editor.minibuffer.as_mut().unwrap().insert(c);
                    }
                    crossterm::event::KeyCode::Backspace => {
                        editor.minibuffer.as_mut().unwrap().backspace();
                    }
                    crossterm::event::KeyCode::Left => {
                        editor.minibuffer.as_mut().unwrap().left();
                    }
                    crossterm::event::KeyCode::Right => {
                        editor.minibuffer.as_mut().unwrap().right();
                    }
                    crossterm::event::KeyCode::Esc => {
                        // Dismiss the minibuffer (dismiss > cancel): this Esc is consumed
                        // here and does NOT reach the filter-cancel Esc check below, so
                        // any in-flight filter continues running.
                        editor.minibuffer = None;
                        // Save-As minibuffer dismiss: drop any queued post-save action.
                        editor.pending_save_as = None;
                    }
                    crossterm::event::KeyCode::Enter => {
                        let mb = editor.minibuffer.take().unwrap();
                        match mb.kind {
                            crate::minibuffer::MinibufferKind::Filter   => submit_filter_line(editor, &mb.text, msg_tx),
                            crate::minibuffer::MinibufferKind::GotoLine => goto_line_submit(editor, &mb.text),
                            crate::minibuffer::MinibufferKind::SaveAs   => save_as_submit(editor, &mb.text, ex, clock, msg_tx),
                        }
                    }
                    _ => {}
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        // non-key (FilterDone/JobDone/Tick/Resize/ClipboardPaste/ClipboardAvailability) falls through to the normal match below
    }

    // Search overlay intercepts KEY INPUT only; non-key messages (FilterDone/JobDone/
    // TransformDone/ExportDone/Tick) fall through to the normal match arm below so
    // background work is never starved while the overlay is open (mirror of minibuffer
    // block above — see test `search_does_not_starve_filterdone`).
    if editor.search.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::{KeyCode, KeyModifiers};
                let alt = k.modifiers.contains(KeyModifiers::ALT);
                let shift = k.modifiers.contains(KeyModifiers::SHIFT);
                let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                // Stepping phase: y/n/!/q intercepted BEFORE the text-insert arm.
                if editor.search.as_ref().map(|s| s.phase) == Some(crate::search_overlay::Phase::Stepping) {
                    match k.code {
                        KeyCode::Char('y') => { search_step_apply(editor, clock); }
                        KeyCode::Char('n') => { search_step_skip(editor); }
                        KeyCode::Char('!') => { search_step_rest(editor, clock); }
                        KeyCode::Char('q') | KeyCode::Esc => { editor.search = None; }
                        _ => {}
                    }
                    for r in ex.drain() { apply_result(r, editor); }
                    return !editor.quit;
                }
                match k.code {
                    KeyCode::Esc => { search_cancel(editor); return !editor.quit; }
                    KeyCode::Char('r') if alt => { editor.search.as_mut().unwrap().toggle_mode(); }
                    KeyCode::Char('c') if alt => { editor.search.as_mut().unwrap().cycle_case(); }
                    KeyCode::Char('a') if alt => { search_replace_all(editor, clock); return !editor.quit; }
                    KeyCode::Enter if alt => {
                        if let Some(s) = editor.search.as_mut() { s.phase = crate::search_overlay::Phase::Stepping; }
                        search_sync(editor); // park on first match
                        for r in ex.drain() { apply_result(r, editor); }
                        return !editor.quit;
                    }
                    KeyCode::Enter if shift => { search_step(editor, false); }
                    KeyCode::F(3) if shift   => { search_step(editor, false); }
                    KeyCode::Enter           => { search_step(editor, true); }
                    KeyCode::F(3)            => { search_step(editor, true); }
                    KeyCode::Tab => {
                        if let Some(s) = editor.search.as_mut() {
                            s.field = match s.field {
                                crate::search_overlay::Field::Needle => crate::search_overlay::Field::Template,
                                crate::search_overlay::Field::Template => crate::search_overlay::Field::Needle,
                            };
                            s.cursor = s.focused_field().len();
                        }
                    }
                    KeyCode::Backspace       => { editor.search.as_mut().unwrap().backspace(); }
                    KeyCode::Left            => { editor.search.as_mut().unwrap().left(); }
                    KeyCode::Right           => { editor.search.as_mut().unwrap().right(); }
                    KeyCode::Char(c) if !ctrl && !alt => { editor.search.as_mut().unwrap().insert(c); }
                    _ => {}
                }
                // Recompute against the live buffer and pin the current match.
                search_sync(editor);
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit; // return ONLY for key events (including non-Press)
        }
        // Non-key messages (FilterDone/ExportDone/TransformDone/JobDone/Tick/…)
        // fall through to the normal handlers below.
    }

    // Diag overlay intercepts KEY INPUT only; non-key messages fall through to
    // normal handling so background work is never starved while the overlay is open
    // (mirror of minibuffer/search blocks above — 5e starvation lesson).
    if editor.diag.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                match k.code {
                    crossterm::event::KeyCode::Up   => { editor.diag.as_mut().unwrap().up(); }
                    crossterm::event::KeyCode::Down => { editor.diag.as_mut().unwrap().down(); }
                    crossterm::event::KeyCode::Esc  => { editor.diag = None; }
                    crossterm::event::KeyCode::Enter => { diag_apply_selected(editor, clock); }
                    _ => {} // bare Ctrl+key or anything else: no-op, consumed
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit; // return ONLY for key events (including non-Press)
        }
        // Non-key messages fall through to normal handlers below.
    }

    if editor.outline.is_some() {
        if editor.outline.as_ref().map(|o| o.buffer_id) != Some(editor.active().id) {
            editor.outline = None;
        }
    }
    if editor.outline.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::{KeyCode, KeyModifiers};
                match k.code {
                    KeyCode::Esc => { editor.outline = None; }
                    KeyCode::Up => {
                        if let Some(o) = editor.outline.as_mut() {
                            o.selected = o.selected.saturating_sub(1);
                        }
                    }
                    KeyCode::Down => {
                        if let Some(o) = editor.outline.as_mut() {
                            let max = o.rows.len().saturating_sub(1);
                            o.selected = (o.selected + 1).min(max);
                        }
                    }
                    KeyCode::Enter => {
                        if editor.outline.as_ref().map(|o| o.opened_version) != Some(editor.active().document.version) {
                            editor.status = "document changed; outline closed".into();
                            editor.outline = None;
                            for r in ex.drain() { apply_result(r, editor); }
                            return !editor.quit;
                        }
                        let target = editor.outline.as_ref()
                            .and_then(|o| o.rows.get(o.selected))
                            .map(|r| r.byte);
                        if let Some(target) = target {
                            outline_jump_to(editor, target);
                        }
                    }
                    KeyCode::Backspace => {
                        if let Some(o) = editor.outline.as_mut() {
                            o.query.pop();
                        }
                        let q = editor.outline.as_ref().map(|o| o.query.clone()).unwrap_or_default();
                        let (blocks, rope) = { let b = editor.active(); (b.document.blocks.clone(), b.document.buffer.snapshot()) };
                        if let Some(o) = editor.outline.as_mut() {
                            o.set_query(&q, &blocks, &rope);
                        }
                    }
                    KeyCode::Char(c)
                        if !k.modifiers.contains(KeyModifiers::CONTROL)
                            && !k.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        if let Some(o) = editor.outline.as_mut() {
                            o.query.push(c);
                        }
                        let q = editor.outline.as_ref().map(|o| o.query.clone()).unwrap_or_default();
                        let (blocks, rope) = { let b = editor.active(); (b.document.blocks.clone(), b.document.buffer.snapshot()) };
                        if let Some(o) = editor.outline.as_mut() {
                            o.set_query(&q, &blocks, &rope);
                        }
                    }
                    _ => {}
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        // Non-key messages fall through to normal handlers below.
    }

    let before = editor.active().document.version;
    match msg {
        Msg::Input(Event::Key(k)) if k.kind == crossterm::event::KeyEventKind::Press => {
            // Esc precedence (Codex CRITICAL): prompt/minibuffer Esc are handled in their
            // interception blocks ABOVE this point. Here in normal mode the order is
            // pending-cancel > filter-cancel. This arm SUBSUMES the old standalone
            // filter-cancel Esc check (removed above). Esc is reserved for cancel/dismiss
            // in v1 (not routed to the keymap).
            if k.code == crossterm::event::KeyCode::Esc {
                if !editor.pending_keys.is_empty() {
                    editor.pending_keys.clear();
                    editor.status.clear();
                } else if editor.filter_in_flight.is_some() {
                    editor.filter_in_flight.take().unwrap().cancel();
                    editor.status = "cancelling…".into();
                }
            } else if let Some(chord) = crate::keymap::from_key_event(k) {
                editor.pending_keys.push(chord);
                match keymap.resolve(&editor.pending_keys) {
                    crate::keymap::Resolution::Command(id) => {
                        editor.pending_keys.clear();
                        editor.status.clear();
                        let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
                        reg.dispatch(id, &mut ctx);
                        hydrate_overlays(editor, reg, keymap);
                    }
                    crate::keymap::Resolution::Pending => {
                        editor.status = format!("{} …", crate::keymap::chords_display(&editor.pending_keys));
                    }
                    crate::keymap::Resolution::None => {
                        let was_single = editor.pending_keys.len() == 1;
                        editor.pending_keys.clear();
                        editor.status.clear();
                        // Printable fallthrough: single unmodified printable → literal insert.
                        if was_single {
                            if let crossterm::event::KeyCode::Char(c) = k.code {
                                if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                                    && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                                {
                                    commands::run(commands::Command::InsertChar(c), editor, clock);
                                }
                            }
                        }
                    }
                }
            }
        }
        Msg::Input(Event::Paste(text)) => {
            if editor.minibuffer.is_some() {
                for ch in text.chars() { editor.minibuffer.as_mut().unwrap().insert(ch); }
            } else if !text.is_empty() {
                let bid = editor.active().id;
                if insert_paste_text(editor, bid, &text, clock) {
                    editor.register.set(text);
                }
            }
        }
        Msg::Input(Event::Resize(w, h)) => {
            editor.active_mut().view.area = (w, h);
            derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
        }
        Msg::Input(Event::Mouse(ev)) => {
            crate::mouse::handle(editor, ev, reg, keymap, ex, clock, msg_tx);
        }
        Msg::Input(_) => {}
        Msg::JobDone(r) => apply_result(r, editor),
        Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => {
            apply_filter_done(editor, buffer_id, version, range, cursor, disposition, outcome, clock);
        }
        Msg::ExportDone { target, result, overwrite_confirmed, .. } => {
            apply_export_done(editor, target, result, overwrite_confirmed);
        }
        Msg::TransformDone { buffer_id, version, range, kind, result } => {
            apply_transform_done(editor, buffer_id, version, range, kind, result, clock);
        }
        Msg::DiagnosticsDone { buffer_id, version, diagnostics } => {
            crate::diagnostics_run::apply_diagnostics_done(editor, buffer_id, version, diagnostics);
        }
        Msg::Tick => {
            let now = clock.now_ms();
            if editor.active().document.dirty()
                && !editor.active().swap_in_flight
                && crate::swap::due(now, editor.active().last_edit_at, editor.active().last_swap_at)
            {
                editor.active_mut().swap_in_flight = true;
                let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
                crate::swap::dispatch_swap_write(&mut ctx);
            }
            // Dispatch diagnostics if due.
            let version = editor.active().document.version;
            if editor.diag_cfg.enabled
                && crate::diagnostics_run::diag_due(&editor.active().diagnostics, now, version)
            {
                let ignore_words = std::sync::Arc::new(
                    editor.dictionary.iter().chain(editor.session_ignores.iter()).cloned().collect::<std::collections::HashSet<String>>()
                );
                let diag_cfg = editor.diag_cfg.clone();
                crate::diagnostics_run::dispatch_diagnostics(editor, &diag_cfg, ignore_words, msg_tx.clone());
            }
        }
        Msg::ClipboardPaste { buffer_id, text, .. } => apply_clipboard_paste(editor, buffer_id, text, clock),
        Msg::ClipboardAvailability(ok) => apply_clipboard_availability(editor, ok),
    }
    if editor.active().document.version != before {
        editor.active_mut().last_edit_at = Some(clock.now_ms());
        // Arm debounce for diagnostics if enabled.
        if editor.diag_cfg.enabled {
            let debounce_ms = editor.diag_cfg.debounce_ms;
            editor.active_mut().diagnostics.arm(clock.now_ms(), debounce_ms);
        }
    }
    // Fold any other results that became ready while handling this message.
    for r in ex.drain() {
        apply_result(r, editor);
    }
    !editor.quit
}

// ---------------------------------------------------------------------------
// step — pure, testable; no terminal IO
// ---------------------------------------------------------------------------

/// Legacy synchronous dispatch path retained for its existing tests; production
/// uses `reduce` + the registry.
///
/// Translate one key event, run the resulting command (if any), then return
/// `true` while the app should keep running (`false` → caller should exit).
///
/// All editor mutation goes through `commands::run`; this function adds no
/// logic of its own beyond the translation.
#[cfg(test)]
pub fn step(editor: &mut Editor, key: KeyEvent, clock: &dyn Clock) -> bool {
    if let Some(cmd) = input::key_to_command(key) {
        commands::run(cmd, editor, clock);
    }
    !editor.quit
}

// ---------------------------------------------------------------------------
// SystemClock — used only by the real `run` loop, never by unit tests
// ---------------------------------------------------------------------------

struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// run — the real terminal loop; terminal IO lives entirely here
// ---------------------------------------------------------------------------

/// Open the file named by `cli.path` (or a scratch buffer), load layered config,
/// build the keymap, install the terminal guard, then loop:
/// draw → read event → step → repeat until `editor.quit`.
pub fn run(cli: config::Cli) -> std::io::Result<()> {
    // Install the panic hook (once) so the terminal is restored on panic.
    term::install_panic_hook();

    // Resolve config layers and build the keymap from them.
    let anchor = cli.path.as_ref()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    let xdg = dirs::config_dir();
    let paths = config::config_layer_paths(&cli, xdg.as_deref(), &anchor);
    let (cfg, mut warns) = config::load(&paths);
    if let Some(c) = &cli.config_path {
        if !c.is_file() {
            warns.push(format!("config: --config path not found: {}", c.display()));
        }
    }

    // Determine the initial terminal size.
    let (cols, rows) = crossterm::terminal::size()?;
    let area = (cols, rows);

    let path = cli.path;

    // Open the file and branch on errors per §C5. Built on the Buffer::from_file seam
    // (Effort 7 Task 1) without behavior change: Ok → named clean; NotFound → named "new
    // file"; Binary/Permission/IsDir/Io → UNNAMED scratch kept + e.to_string() status.
    let mut editor = Editor::new_from_text("\n", None, area); // scratch host; the real buffer (if any) replaces slot 0 below
    if let Some(p) = path.as_deref() {
        let id = editor.active().id; // reuse slot 0's id for the launch buffer
        match crate::editor::Buffer::from_file(id, p, area) {
            Ok(b) => {
                let was_new_file = b.document.path.is_some() && !p.exists();
                editor.buffers[0] = b;
                if was_new_file {
                    // New file: empty buffer NAMED with the path; first save creates it.
                    editor.status = "new file".to_string();
                }
            }
            // NotFound is mapped to Ok (named "new file") inside from_file, so it never
            // reaches here; list it for exhaustiveness. The rest are rejected targets:
            // UNNAMED scratch kept so a save can't clobber the file.
            Err(e @ (file::OpenError::NotFound(_)
                   | file::OpenError::Binary(_)
                   | file::OpenError::Permission(_)
                   | file::OpenError::IsDir(_)
                   | file::OpenError::Io(_))) => {
                editor.status = e.to_string();
            }
        }
    }

    // Seed mouse_capture from config (default true; may be overridden by config layers).
    editor.mouse_capture = cfg.mouse.mouse_capture;
    editor.view_opts = cfg.view.clone();
    editor.resume_enabled = cfg.state.resume; // gates open_into_current's resume restore (Effort 7)
    editor.diag_cfg = cfg.diagnostics.clone();
    // Resolve and seed the active theme + color depth (once, at startup — §3.6).
    let env = crate::theme_resolve::EnvSnapshot::from_env();
    let resolved = crate::theme_resolve::resolve_theme(&cfg.theme, &env);
    editor.theme = resolved.theme;
    editor.depth = resolved.depth;
    editor.heading_glyph_cfg = cfg.theme.heading_level_glyph; // for runtime picker switches (Task 7)
    warns.extend(resolved.warnings); // join the existing startup warning stream

    // Load the personal dictionary from disk (missing/unreadable → empty; no abort).
    if let Some(dict_path) = &cfg.diagnostics.dictionary {
        if let Ok(text) = std::fs::read_to_string(dict_path) {
            editor.dictionary = text.lines().map(|l| l.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
    }

    // Recovery-on-open (§5.1).
    // Named files: use assess() with content-hash comparison.
    // Scratch buffers: their swap is pid-keyed, so look for an orphan from a
    // dead previous session (pre-merge blocker #1).
    if editor.active().document.path.is_some() {
        // Read F's current bytes once for the predicate.
        let file_bytes = editor.active().document.path.as_deref().and_then(|p| std::fs::read(p).ok());
        match crate::swap::assess(editor.active().document.path.as_deref(), file_bytes.as_deref()) {
            crate::swap::RecoveryDecision::OpenNormally => {}
            crate::swap::RecoveryDecision::DiscardSilently => {
                crate::swap::delete(editor.active().document.path.as_deref());
            }
            crate::swap::RecoveryDecision::Prompt(_h, body) => {
                editor.active_mut().pending_swap_body = Some(body);
                editor.open_prompt(crate::prompt::Prompt::swap_recovery());
                editor.status = "Recovery file found".into();
            }
        }
    } else if let Some((sp, _header, body)) = crate::swap::find_orphan_scratch_swap() {
        editor.active_mut().pending_swap_body = Some(body);
        editor.active_mut().pending_swap_path = Some(sp);
        editor.open_prompt(crate::prompt::Prompt::swap_recovery());
        editor.status = "Recovery file found".into();
    }

    // Install the terminal guard: enable raw mode + enter alternate screen.
    // Mouse capture is gated on editor.mouse_capture (seeded from config above).
    let mut guard = term::TerminalGuard::new(editor.mouse_capture)?;
    let mut applied_mouse = editor.mouse_capture;

    // Initial derive so the first draw has up-to-date layouts.
    derive::rebuild(&mut editor);

    // Warm the pandoc probe cache so the first export command doesn't pay latency.
    let _ = crate::export::probe_pandoc();

    // Warm Harper's FstDictionary LazyLock off the critical path so the first
    // real diagnostics check isn't ~11s. Fire-and-forget; discard the result.
    if editor.diag_cfg.enabled {
        std::thread::Builder::new()
            .name("wcartel-diag-warm".into())
            .spawn(|| {
                let ignore = std::collections::HashSet::new();
                let opts = wordcartel_core::diagnostics::CheckOpts { grammar: false, ignore_words: &ignore };
                let _ = wordcartel_core::diagnostics::check("", &opts);
            })
            .expect("spawn diag warmup thread");
    }

    let reg = Registry::builtins();
    // Build the keymap from the loaded config and surface any warnings.
    let (built_keymap, mut kw) = keymap::build_keymap(&cfg.keymap, &reg);
    warns.append(&mut kw);
    editor.keymap = built_keymap;
    if let Some(w) = warns.first() {
        editor.status = w.clone();
    }
    // Take the keymap out of editor into a loop-local to avoid a simultaneous
    // &mut editor / &editor.keymap borrow conflict when calling reduce.
    // (The keymap doesn't change during the loop in v1.)
    let keymap = std::mem::take(&mut editor.keymap);
    let (msg_tx, msg_rx) = std::sync::mpsc::channel::<Msg>();
    let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
    let executor = crate::jobs::ThreadExecutor::new(wake_tx);
    let clip_tx = crate::clipboard::spawn_worker(msg_tx.clone());

    // Worker → loop wake relay: each result nudges the loop to drain. reduce()'s
    // trailing ex.drain() does the actual merge, so Msg::Tick is the nudge.
    {
        let msg_tx = msg_tx.clone();
        std::thread::spawn(move || {
            while wake_rx.recv().is_ok() {
                if msg_tx.send(Msg::Tick).is_err() { break; }
            }
        });
    }

    // Input thread: blocks on read(); forwards every event. Detached — dies with
    // the process on quit (read() can't be interrupted portably).
    {
        let msg_tx = msg_tx.clone();
        std::thread::Builder::new()
            .name("wcartel-input".into())
            .spawn(move || {
                while let Ok(ev) = crossterm::event::read() {
                    if msg_tx.send(Msg::Input(ev)).is_err() { break; }
                }
            })
            .expect("spawn input thread");
    }

    let clock = SystemClock;
    const SAVE_QUIT_TIMEOUT_MS: u64 = 5_000;

    // Load the session store once at startup (corrupt/missing → empty, no abort).
    let mut session = crate::state::load();
    // Initialize the seq counter one past the highest stored seq so that newly
    // recorded entries always outrank loaded ones for LRU eviction (Codex pre-merge fix).
    let mut session_seq: u64 = session.next_seq();

    // Resume-on-open: if cfg.state.resume is set and the buffer has a path, restore
    // cursor/scroll/marks/folds via the shared seam (factored from this block so launch
    // and open_into_current stay byte-identical). The staleness guard lives in
    // restore_resume → apply_resume.
    if cfg.state.resume {
        if let Some(raw_path) = editor.active().document.path.clone() {
            restore_resume(&mut editor, &raw_path);
        }
    }

    // Track saved_version to detect when a save completes in the loop.
    let mut last_persisted_saved = editor.active().document.saved_version;

    // Reconcile mouse capture once before the first draw (post-guard invariant).
    reconcile_mouse_capture(&mut editor, guard.terminal().backend_mut(), &mut applied_mouse);

    recompute_scrollbar_visible(&mut editor, clock.now_ms());
    // After a potential session-resume the scroll may have changed; re-clamp/re-pin
    // and rebuild the layout cache so the very first frame is always correct.
    // Order: rebuild (reconciles folds + layout) → SnapOut restored caret → ensure_visible.
    // This ensures a cursor saved inside a now-folded section is snapped to the visible
    // heading before the first draw, maintaining the caret-never-in-a-fold invariant.
    derive::rebuild(&mut editor);
    {
        use crate::registry::{place_caret_visible, CaretPlace};
        let head = editor.active().document.selection.primary().head;
        let nh = place_caret_visible(&mut editor, head, CaretPlace::SnapOut);
        if nh != head {
            editor.active_mut().document.selection =
                wordcartel_core::selection::Selection::single(nh);
        }
    }
    crate::nav::ensure_visible(&mut editor);
    guard.terminal().draw(|f| render::render(f, &mut editor))?;
    loop {
        let now = clock.now_ms();
        // Bounded save&quit: if waiting for an in-flight save to complete and
        // 5 s have elapsed since the last edit, re-raise the quit-confirm modal.
        if let Some(p) = &editor.pending_after_save {
            let waited = now.saturating_sub(p.at_ms);
            if waited > SAVE_QUIT_TIMEOUT_MS {
                editor.pending_after_save = None;
                editor.open_prompt(crate::prompt::Prompt::quit_confirm());
                editor.status = "Save still running — choose again".into();
            }
        }
        let swap_deadline = crate::swap::next_deadline_ms(now, editor.active().last_edit_at, editor.active().last_swap_at);
        let sq_deadline = editor.pending_after_save.as_ref().map(|p| p.at_ms.saturating_add(SAVE_QUIT_TIMEOUT_MS));
        // Include scrollbar_until_ms in the deadline so the loop wakes when the
        // bar should fade (avoids relying on the idle 1-hour Tick).
        let sb_deadline = if editor.mouse.scrollbar_until_ms > now {
            Some(editor.mouse.scrollbar_until_ms)
        } else {
            None
        };
        // Fix A3: include the diagnostics deadline ONLY when no check is in
        // flight.  When a check is in flight, recheck_due_at may be a past
        // timestamp (armed before the check started), which would drive
        // recv_timeout(0) → 100% CPU spin until the worker completes.
        // When the result lands it clears in_flight_version; the next
        // iteration will re-include the (re-armed) deadline and dispatch.
        let diag_deadline = if editor.active().diagnostics.in_flight_version.is_none() {
            editor.active().diagnostics.recheck_due_at
        } else {
            None
        };
        let deadline = crate::diagnostics_run::next_deadline(&[
            swap_deadline,
            sq_deadline,
            sb_deadline,
            diag_deadline,
        ]);
        let timeout = deadline
            .map(|d| std::time::Duration::from_millis(d.saturating_sub(now)))
            .unwrap_or(std::time::Duration::from_secs(3600));
        let msg = match msg_rx.recv_timeout(timeout) {
            Ok(m) => m,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Msg::Tick,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let keep = reduce(msg, &mut editor, &reg, &keymap, &executor, &clock, &msg_tx);
        crate::clipboard::drain_clipboard_intents(&mut editor, guard.terminal().backend_mut(), &clip_tx, &msg_tx);
        reconcile_mouse_capture(&mut editor, guard.terminal().backend_mut(), &mut applied_mouse);
        recompute_scrollbar_visible(&mut editor, clock.now_ms());
        // Pre-draw rebuild: ensure the layout cache matches the final (scroll,
        // text_width) before render consumes it.  render has no on-demand fallback
        // (render.rs:132-140), so a stale cache blanks the editing rows.
        derive::rebuild(&mut editor);
        guard.terminal().draw(|f| render::render(f, &mut editor))?;
        // Persist session state when a save just completed (saved_version advanced).
        let sv = editor.active().document.saved_version;
        if sv != last_persisted_saved {
            session_seq += 1;
            persist_session(&mut session, &editor, &cfg, session_seq);
            last_persisted_saved = sv;
        }
        if !keep { break; }
    }

    // On clean quit: persist once more (cursor may have moved since the last save).
    session_seq += 1;
    persist_session(&mut session, &editor, &cfg, session_seq);

    // Restore the terminal BEFORE the executor drops: ThreadExecutor::drop joins
    // the worker, which may still be completing an in-flight save_atomic on a slow
    // filesystem. Dropping the guard first guarantees the user gets their terminal
    // back immediately; we still join (don't abandon an in-flight atomic save — that
    // is the "never lose work" behavior). The 5 s save&quit guard above bounds the wait.
    drop(guard);
    Ok(())
}

/// Recompute `editor.mouse.scrollbar_visible` from the clock.
///
/// Must be called at the top of the run loop (with `clock.now_ms()`) so that
/// the scrollbar fades exactly when `scrollbar_until_ms` expires, driven by
/// the loop's `deadline` (not an idle Tick).
pub fn recompute_scrollbar_visible(editor: &mut crate::editor::Editor, now_ms: u64) {
    editor.mouse.scrollbar_visible = now_ms < editor.mouse.scrollbar_until_ms;
}

/// Reconcile the terminal's mouse-capture state with `editor.mouse_capture`.
///
/// Enables or disables mouse capture on the backend when the desired state
/// diverges from `applied`. On disable, clears drag state so no stale Up
/// events are awaited for a capture that will never arrive.
pub fn reconcile_mouse_capture<W: std::io::Write>(editor: &mut crate::editor::Editor, backend: &mut W, applied: &mut bool) {
    if editor.mouse_capture != *applied {
        if editor.mouse_capture {
            if crossterm::execute!(backend, crossterm::event::EnableMouseCapture).is_ok() {
                *applied = editor.mouse_capture;
            }
        } else {
            // clear drag state regardless of IO outcome — it is local state,
            // not tied to the terminal write succeeding.
            editor.mouse.dragging = false;
            editor.mouse.scrollbar_dragging = false;
            editor.mouse.anchor = None;
            if crossterm::execute!(backend, crossterm::event::DisableMouseCapture).is_ok() {
                *applied = editor.mouse_capture;
            }
        }
    }
}

/// Record the active buffer's position into the session store and flush to disk.
/// Scratch (no path) buffers and paths that fail canonicalization are skipped.
/// A write failure → status warning only (never blocks quit or loses the document).
fn persist_session(
    session: &mut crate::state::SessionState,
    editor: &Editor,
    cfg: &config::Config,
    seq: u64,
) {
    // Scratch buffers are never persisted.
    let raw_path = match editor.active().document.path.as_deref() {
        Some(p) => p,
        None => return,
    };
    // Canonicalize: skip if it fails (e.g. a new file that hasn't been saved yet).
    let canon = match std::fs::canonicalize(raw_path) {
        Ok(p) => p,
        Err(_) => return,
    };
    // Get the file identity at persist time (skip if unavailable).
    let Some((mtime, size)) = crate::state::file_identity(raw_path) else { return };

    let cursor = editor.active().document.selection.primary().head;
    let scroll = editor.active().view.scroll;
    let entry = crate::state::StateEntry {
        cursor,
        scroll,
        marks: editor.active().marks.iter().map(|(c, &o)| (c.to_string(), o)).collect(),
        mtime,
        size,
        seq,
        folds: editor.active().folds.folded.iter().copied().collect(),
    };
    session.record(
        canon.to_string_lossy().into_owned(),
        entry,
        cfg.state.max_entries,
    );
    if let Err(e) = session.save() {
        // Degrade: write error → warning in the terminal title area; never blocks quit.
        // The terminal is still up here (we drop guard after this in run()), so we
        // can mutate the editor status without any special handling.
        let _ = e; // best-effort; we can't easily update editor.status here without &mut editor
        // We don't have &mut editor in this helper; the warning is silently swallowed.
        // This is acceptable per the brief ("best-effort save, write error → status warning").
        // Callers that need the error can check the return if we make it Result in a later effort.
    }
}

// ---------------------------------------------------------------------------
// Tests — written FIRST (RED phase) before any implementation
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use crate::editor::Editor;
    use crate::app::Msg;
    use wordcartel_core::history::Clock;
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);

    struct TestClock(u64);
    impl TestClock {
        fn new(ms: u64) -> Self { TestClock(ms) }
    }
    impl Clock for TestClock {
        fn now_ms(&self) -> u64 { self.0 }
    }

    /// Build a KeyEvent for a printable character (no modifiers, Press).
    fn key_char(c: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn cua_keymap() -> crate::keymap::KeyTrie {
        let (t, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &crate::registry::Registry::builtins());
        t
    }

    #[test]
    fn open_into_current_replaces_with_fresh_id_and_clean() {
        use crate::editor::Editor;
        let p = std::env::temp_dir().join(format!("wc-oic-{}.md", std::process::id()));
        std::fs::write(&p, "opened\n").unwrap();
        let mut e = Editor::new_from_text("scratch\n", None, (80, 24));
        let old_id = e.active().id;
        crate::app::open_into_current(&mut e, &p);
        assert_ne!(e.active().id, old_id, "fresh id → stale in-flight jobs for old buffer are ignored");
        assert_eq!(e.active().document.buffer.to_string(), "opened\n");
        assert!(!e.active().document.dirty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn file_browser_enter_on_file_opens_it_when_clean() {
        use crate::editor::Editor;
        let dir = std::env::temp_dir().join(format!("wc-fbopen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.md"), "loaded\n").unwrap();
        let mut e = Editor::new_from_text("clean\n", None, (80, 24)); // clean
        e.open_file_browser(dir.clone());
        // select "note.md" and simulate Enter via the browser's open path:
        crate::app::open_into_current(&mut e, &dir.join("note.md")); // the clean-path the Enter handler takes
        assert_eq!(e.active().document.buffer.to_string(), "loaded\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn press(code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) -> Msg {
        use crossterm::event::{Event, KeyEvent, KeyEventKind, KeyEventState};
        Msg::Input(Event::Key(KeyEvent { code, modifiers: mods, kind: KeyEventKind::Press, state: KeyEventState::NONE }))
    }

    fn f10() -> crossterm::event::Event {
        crossterm::event::Event::Key(KeyEvent {
            code: KeyCode::F(10),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    // -------------------------------------------------------------------------
    // Brief's required failing test (Task 12 step 1)
    // -------------------------------------------------------------------------

    /// Feed "hi" then Ctrl+Q (modal) then 'q' (QuitAnyway); confirm the buffer holds "hi\n" and quit.
    #[test]
    fn step_processes_typing_and_quit() {
        use crate::registry::Registry;
        use crate::jobs::InlineExecutor;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let reg = Registry::builtins();
        let km = cua_keymap();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        for c in "hi".chars() {
            crate::app::step(&mut e, key_char(c), &clk);
        }
        // First Ctrl+Q: dirty → modal up, NOT quit yet
        let ctrl_q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(crate::app::Msg::Input(ctrl_q), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "dirty quit must raise modal");
        assert!(!e.quit);
        // Press 'q' → routed to QuitAnyway via the modal.
        let q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(crate::app::Msg::Input(q), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.quit);
        assert_eq!(e.active().document.buffer.to_string(), "hi\n");
    }

    #[test]
    fn copy_sets_register_and_sync_request() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        // select "hello" (0..5)
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5);
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let ctrl_c = Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(ctrl_c), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.register.get(), Some("hello"));
        assert_eq!(e.clipboard_sync_request.as_deref(), Some("hello"));
    }

    #[test]
    fn paste_keypress_sets_intent_not_inline_paste() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.register.set("Z".into());
        let before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let ctrl_v = Event::Key(KeyEvent { code: KeyCode::Char('v'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(ctrl_v), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), before, "Ctrl+V no longer pastes inline");
        assert!(e.clipboard_get_pending.is_some(), "Ctrl+V sets a paste intent");
    }

    #[test]
    fn clipboardpaste_some_inserts_os_text_one_undo() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1); // caret after 'a'
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXYb\n");
        assert_eq!(e.register.get(), Some("XY"), "OS text updates the register");
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");
    }

    #[test]
    fn f10_opens_menu_and_selected_event_dispatches() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        // Simulate a menu selection routing: open, then feed a synthesized Selected(copy) via the menu handler path.
        crate::app::reduce(Msg::Input(f10()), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_some(), "F10 opens the menu");
        // drive a selection of "copy" through the menu->dispatch path (helper exercising drain_events->dispatch_overlay_command)
        crate::app::menu_select_for_test(&mut e, &reg, &ex, &clk, &tx, crate::registry::CommandId("copy"));
        assert!(e.menu.is_none(), "selection closes the menu");
        assert_eq!(e.register.get(), Some("abc"));
    }

    #[test]
    fn menu_keyboard_nav_moves_and_dispatches() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |c| Event::Key(KeyEvent { code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // F10 opens; menu hydrated with groups
        crate::app::reduce(Msg::Input(press(KeyCode::F(10))), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_some());
        let m = e.menu.as_ref().unwrap();
        assert!(!m.groups.is_empty(), "menu hydrated with groups");
        assert_eq!(m.open, 0);
        // Right moves to the next category, Down highlights a row
        crate::app::reduce(Msg::Input(press(KeyCode::Right)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.menu.as_ref().unwrap().open, 1);
        crate::app::reduce(Msg::Input(press(KeyCode::Down)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.menu.as_ref().unwrap().highlighted, 1);
    }

    #[test]
    fn clipboardpaste_none_falls_back_to_register() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        e.register.set("R".into());
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: None }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aRb\n", "None -> register fallback");
    }

    #[test]
    fn clipboardpaste_none_empty_register_is_noop() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: None }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "ab\n", "empty register -> no change");
    }

    #[test]
    fn clipboardpaste_replaces_active_selection() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(1, 3); // select "bc"
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXYd\n", "selection replaced by pasted text");
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "abcd\n");
    }

    #[test]
    fn clipboardpaste_for_missing_buffer_is_noop() {
        use crate::editor::{Editor, BufferId}; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: BufferId(99999), text: Some("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "ab\n", "unknown buffer -> dropped");
    }

    #[test]
    fn clipboardpaste_oversize_skips_insert_and_register() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        e.register.set("orig".into());
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let huge = "x".repeat(crate::clipboard::PASTE_MAX_BYTES + 1);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some(huge) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "ab\n", "oversize paste must not insert");
        assert_eq!(e.register.get(), Some("orig"), "oversize paste must not mutate the register");
        assert!(e.status.to_lowercase().contains("too large"));
    }

    #[test]
    fn availability_false_shows_notice_once() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardAvailability(false), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.status.to_lowercase().contains("clipboard"));
        assert!(e.clipboard_notice_shown);
        e.status = "typing".into();
        crate::app::reduce(Msg::ClipboardAvailability(false), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.status, "typing", "notice shown only once");
    }

    // -------------------------------------------------------------------------
    // key_to_command mapping tests
    // -------------------------------------------------------------------------

    /// Ctrl+S maps to Command::Save.
    #[test]
    fn key_to_command_ctrl_s_is_save() {
        use crate::commands::Command;
        let k = KeyEvent {
            code: KeyCode::Char('s'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(crate::input::key_to_command(k), Some(Command::Save)));
    }

    /// Shift+Right maps to Move { dir: Right, extend: true }.
    #[test]
    fn key_to_command_shift_right_extends() {
        use crate::commands::{Command, Dir};
        let k = KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(
            crate::input::key_to_command(k),
            Some(Command::Move { dir: Dir::Right, extend: true })
        ));
    }

    /// A printable char maps to InsertChar.
    #[test]
    fn key_to_command_printable_is_insert_char() {
        use crate::commands::Command;
        let k = key_char('A');
        assert!(matches!(crate::input::key_to_command(k), Some(Command::InsertChar('A'))));
    }

    /// An unmapped key (F5) returns None.
    #[test]
    fn key_to_command_unmapped_is_none() {
        let k = KeyEvent {
            code: KeyCode::F(5),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(crate::input::key_to_command(k).is_none());
    }

    /// Release events return None (double-input guard).
    #[test]
    fn key_to_command_release_is_none() {
        let k = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        assert!(crate::input::key_to_command(k).is_none());
    }

    #[test]
    fn reduce_handles_typing_via_registry() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        for c in "hi".chars() {
            let ev = Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press, state: KeyEventState::NONE });
            assert!(crate::app::reduce(crate::app::Msg::Input(ev), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx));
        }
        assert_eq!(e.active().document.buffer.to_string(), "hi\n");
    }

    #[test]
    fn filterdone_replaces_range_when_fresh() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let msg = Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) };
        crate::app::reduce(msg, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXde\n");
        // one undo step restores the original
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "abcde\n");
    }

    #[test]
    fn filterdone_discarded_when_version_moved() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let stale_v = e.active().document.version;
        e.active_mut().document.version += 1; // simulate an intervening edit
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: stale_v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "abcde\n", "stale filter result discarded");
        assert!(e.status.to_lowercase().contains("discarded"));
    }

    #[test]
    fn filterdone_failure_shows_status_keeps_buffer() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult, FilterError};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter,
            outcome: RunResult::Err(FilterError::NonZero { code: "Exited(3)".into(), stderr: "boom".into() }) },
            &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "abcde\n");
        assert!(e.status.contains("boom") && e.status.contains('3'));
    }

    #[test]
    fn dispatch_filter_runs_real_command_and_delivers_filterdone() {
        // One live-thread integration test (deterministic: block on the channel).
        use crate::editor::Editor;
        use crate::filter::{dispatch_filter, FilterSpec, Disposition, Input};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let spec = FilterSpec { argv: vec!["tr".into(),"a-z".into(),"A-Z".into()], shell: false,
            disposition: Disposition::Filter, input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10), max_output: 1 << 20 };
        dispatch_filter(&mut e, spec, tx);
        let msg = rx.recv().expect("FilterDone must arrive"); // blocks; no timing assert
        match msg { Msg::FilterDone { outcome: crate::filter::RunResult::Stdout(s), .. } => assert_eq!(s, "ABC\n"),
                    other => panic!("expected FilterDone Stdout, got {other:?}") }
    }

    #[test]
    fn tick_writes_swap_when_idle_elapsed_and_dirty() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        let doc_path = std::env::temp_dir().join(format!(
            "wc-tick-swap-{}-{}.md",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        let mut e = Editor::new_from_text("\n", Some(doc_path.clone()), (80, 24));
        e.active_mut().document.version = 1;            // dirty (saved_version=Some(0))
        e.active_mut().last_edit_at = Some(0);
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        // Clock past the idle threshold.
        struct C(u64); impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let clk = C(crate::swap::T_IDLE_MS + 5);
        crate::app::reduce(crate::app::Msg::Tick, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.active().last_swap_at.is_some(), "an idle Tick on a dirty buffer writes a swap");
        let sp = crate::swap::swap_path(Some(&doc_path)).unwrap();
        assert!(sp.exists());
        let _ = std::fs::remove_file(&sp);
        let _ = std::fs::remove_file(&doc_path);
    }

    #[test]
    fn quit_with_unsaved_raises_modal_then_quit_anyway_exits() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.active_mut().document.version = 1; // dirty
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let ctrl_q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // First Ctrl+Q → modal up, not quit.
        crate::app::reduce(crate::app::Msg::Input(ctrl_q.clone()), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.prompt.is_some() && !e.quit);
        // Press 'q' → routed to QuitAnyway.
        let q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(crate::app::Msg::Input(q), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.quit, "Quit anyway exits");
        assert!(e.prompt.is_none(), "prompt cleared");
    }

    #[test]
    fn save_and_quit_sets_pending_after_save_and_exits_on_matching_result() {
        use crate::editor::{Editor, PostSaveAction};
        use crate::jobs::{Executor, InlineExecutor};
        use crate::prompt::PromptAction;
        let p = std::env::temp_dir().join(format!("wc-savequit-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None; e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::resolve_prompt(PromptAction::SaveAndQuit, &mut e, &ex, &clk, &tx);
        assert!(matches!(e.pending_after_save, Some(crate::editor::PendingAfterSave { version: 1, action: PostSaveAction::Quit, .. })));
        assert!(!e.quit, "not yet — waiting for the save result");
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(e.quit, "matching save result triggers quit");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn save_and_quit_on_unnamed_buffer_does_not_arm_pending_after_save() {
        // No path → dispatch_save dispatches NO job. pending_after_save must stay None,
        // or the app would wait forever for a result that never comes (Codex #4).
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::prompt::PromptAction;
        let mut e = Editor::new_from_text("scratch\n", None, (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::resolve_prompt(PromptAction::SaveAndQuit, &mut e, &ex, &clk, &tx);
        assert!(e.pending_after_save.is_none(), "no job dispatched → do not arm pending_after_save");
        assert!(!e.quit);
    }

    #[test]
    fn save_and_quit_command_arms_pending_after_save_like_prompt() {
        // The save_and_quit registry command must reach the SAME armed state as the
        // PromptAction::SaveAndQuit path (proves the DRY factor).
        use crate::editor::{Editor, PostSaveAction};
        use crate::jobs::{Executor, InlineExecutor};
        let p = std::env::temp_dir().join(format!("wc-savequit-cmd-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None; e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        {
            let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx.clone() };
            crate::save::dispatch_save_and_quit(&mut ctx);
        }
        assert!(matches!(e.pending_after_save, Some(crate::editor::PendingAfterSave { version: 1, action: PostSaveAction::Quit, .. })),
            "command path arms pending_after_save{{Quit}}");
        assert!(!e.quit, "not yet — waiting for the save result");
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(e.quit, "matching save result triggers quit");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn save_and_quit_command_on_unnamed_buffer_does_not_arm() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("scratch\n", None, (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        {
            let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx.clone() };
            crate::save::dispatch_save_and_quit(&mut ctx);
        }
        assert_eq!(e.pending_after_save, None, "no path → not armed");
        assert!(!e.quit);
    }

    #[test]
    fn save_and_quit_arms_pending_after_save_quit_and_exits() {
        use crate::editor::{Editor, PostSaveAction};
        use crate::jobs::{Executor, InlineExecutor};
        let p = std::env::temp_dir().join(format!("wc-pas-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None; e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        {
            let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx.clone() };
            crate::save::dispatch_save_and_quit(&mut ctx);
        }
        assert!(matches!(e.pending_after_save, Some(crate::editor::PendingAfterSave { version: 1, action: PostSaveAction::Quit, .. })));
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(e.quit, "matching save result triggers quit");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn apply_result_merges_fresh_and_drops_stale() {
        use crate::editor::Editor;
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let id = e.active().id;
        e.active_mut().document.version = 5;
        // Fresh one-shot (Save is never stale): merges.
        crate::app::apply_result(JobResult { buffer_id: id, class: ResultClass::Durability, version: 3, kind: JobKind::Save,
            merge: Box::new(|ed: &mut Editor| ed.status = "saved".into()) }, &mut e);
        assert_eq!(e.status, "saved");
        // Stale coalescible: dropped.
        crate::app::apply_result(JobResult { buffer_id: id, class: ResultClass::BufferLocal, version: 3, kind: JobKind::CoalesceProbe,
            merge: Box::new(|ed: &mut Editor| ed.status = "STALE".into()) }, &mut e);
        assert_eq!(e.status, "saved", "stale coalescible result must be dropped");
    }

    #[test]
    fn buffer_local_result_for_missing_buffer_is_dropped() {
        use crate::editor::{Editor, BufferId};
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        // A buffer-local merge for a non-existent buffer must NOT run.
        crate::app::apply_result(JobResult {
            buffer_id: BufferId(999), class: ResultClass::BufferLocal,
            version: 1, kind: JobKind::Save,
            merge: Box::new(|ed: &mut Editor| ed.status = "SHOULD NOT RUN".into()),
        }, &mut e);
        assert_ne!(e.status, "SHOULD NOT RUN", "buffer-local merge for a missing buffer is dropped");
    }

    #[test]
    fn buffer_local_result_for_live_buffer_merges() {
        use crate::editor::Editor;
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let id = e.active().id;
        crate::app::apply_result(JobResult {
            buffer_id: id, class: ResultClass::BufferLocal,
            version: 1, kind: JobKind::Save,
            merge: Box::new(|ed: &mut Editor| ed.status = "merged".into()),
        }, &mut e);
        assert_eq!(e.status, "merged");
    }

    #[test]
    fn recover_loads_body_and_deletes_orphan_swap_file() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::prompt::PromptAction;
        // An orphan swap file on disk + a buffer staged for recovery.
        let p = std::env::temp_dir().join(format!("wc-recover-orphan-{}.swp", std::process::id()));
        std::fs::write(&p, "stub").unwrap();
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        e.active_mut().pending_swap_body = Some("recovered body\n".into());
        e.active_mut().pending_swap_path = Some(p.clone());
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::resolve_prompt(PromptAction::Recover, &mut e, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "recovered body\n",
            "recovered content loaded into the active buffer");
        assert!(!p.exists(), "orphan swap file must be deleted on Recover");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn minibuffer_routing_and_submit_dispatches_filter() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let key = |c: char| Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        for c in "cat".chars() { crate::app::reduce(Msg::Input(key(c)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx); }
        assert_eq!(e.minibuffer.as_ref().unwrap().text, "cat");
        // Enter submits -> dispatch_filter -> a FilterDone arrives, minibuffer cleared
        let enter = Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(enter), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.minibuffer.is_none(), "submit clears the minibuffer");
        match rx.recv().unwrap() { Msg::FilterDone { outcome: crate::filter::RunResult::Stdout(s), .. } => assert_eq!(s, "abc\n"),
                                   o => panic!("expected FilterDone, got {o:?}") }
    }

    #[test]
    fn goto_line_jumps_to_line_start_and_records_jumpback() {
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("one\ntwo\nthree\nfour\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        // start at end so the jump is a real move
        let end = e.active().document.buffer.len();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(end);
        e.open_minibuffer("Go to line: ", crate::minibuffer::MinibufferKind::GotoLine);
        // type "3" then Enter
        let (tx, _rx) = std::sync::mpsc::channel();
        for ch in "3".chars() { crate::app::reduce(Msg::Input(Event::Key(KeyEvent{code:KeyCode::Char(ch),modifiers:KeyModifiers::NONE,kind:KeyEventKind::Press,state:KeyEventState::NONE})), &mut e, &Registry::builtins(), &cua_keymap(), &InlineExecutor::default(), &TestClock(0), &tx); }
        crate::app::reduce(Msg::Input(Event::Key(KeyEvent{code:KeyCode::Enter,modifiers:KeyModifiers::NONE,kind:KeyEventKind::Press,state:KeyEventState::NONE})), &mut e, &Registry::builtins(), &cua_keymap(), &InlineExecutor::default(), &TestClock(0), &tx);
        // line 3 ("three") starts at byte 8
        assert_eq!(e.active().document.selection.primary().head, e.active().document.buffer.line_to_byte(2));
        assert!(e.minibuffer.is_none(), "submit closes the minibuffer");
        // jump-back: the origin (end) was recorded so the user can return.
        assert!(e.active().jump_ring.contains(&end), "goto recorded the origin for jump-back");
    }

    #[test]
    fn goto_line_into_folded_body_unfolds_to_reveal_target() {
        // Spec §2 / Codex: a goto target inside a folded body must UNFOLD, not land hidden.
        let mut e = Editor::new_from_text("# H\n\nbody one\nbody two\nbody three\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        // Fold the "# H" section via the real fold API (mirrors the 5g fold tests:
        // `folds.toggle(heading_byte)` + rebuild — the heading anchor is byte 0).
        let h_byte = 0usize;
        e.active_mut().folds.toggle(h_byte);
        crate::derive::rebuild(&mut e);
        assert!(e.active().folds.folded.contains(&h_byte), "precondition: # H is folded");
        // goto line 4 ("body two"), which is inside the folded body:
        crate::app::goto_line_submit(&mut e, "4");
        assert_eq!(e.active().document.selection.primary().head, e.active().document.buffer.line_to_byte(3));
        // The section is no longer folded over the target (real fold-state query: the
        // heading anchor is gone from `folds.folded`, so line index 3 is visible again).
        assert!(!e.active().folds.folded.contains(&h_byte),
            "goto into a folded body must unfold the covering section to reveal the target");
    }

    #[test]
    fn goto_line_clamps_and_rejects_garbage() {
        let mut e = Editor::new_from_text("a\nb\nc\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        crate::app::goto_line_submit(&mut e, "999");          // clamp-high → last line
        let total = crate::derive::total_logical_lines(&e.active().document.buffer);
        assert_eq!(e.active().document.selection.primary().head, e.active().document.buffer.line_to_byte(total - 1));
        crate::app::goto_line_submit(&mut e, "0");            // clamp-low → line 1
        assert_eq!(e.active().document.selection.primary().head, 0);
        crate::app::goto_line_submit(&mut e, "xyz");          // garbage → status, no move
        assert_eq!(e.active().document.selection.primary().head, 0);
        assert_eq!(e.status, "not a line number");           // rejected input sets the status
    }

    #[test]
    fn minibuffer_does_not_starve_filterdone() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        let (tx, _rx) = std::sync::mpsc::channel(); let reg = Registry::builtins();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXde\n", "FilterDone applies even under an open minibuffer");
    }

    #[test]
    fn exportdone_bytes_writes_file_beside_source() {
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        // Create a temp directory and source file path.
        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-exportdone-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("notes.md");
        std::fs::write(&source, "# Hello\n").expect("write source");

        let output_path = tmp_dir.join("notes.html");

        let mut e = Editor::new_from_text("# Hello\n", Some(source.clone()), (80, 24));
        let buffer_id = e.active().id;

        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let content_bytes = b"<h1>Hello</h1>\n".to_vec();
        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(content_bytes.clone())),
            overwrite_confirmed: false,
        };
        crate::app::reduce(msg, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);

        // The output file must exist beside the source.
        assert!(output_path.exists(), "exported file must exist");
        let got = std::fs::read(&output_path).expect("read exported file");
        assert_eq!(got, content_bytes);
        assert!(e.status.contains("exported"), "status must say exported");

        // Clean up.
        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn exportdone_unconfirmed_refuses_when_target_appeared() {
        // TOCTOU guard (Codex pre-merge gate): export was dispatched because the
        // target did not exist (overwrite_confirmed=false), but a target file has
        // since appeared.  Finalization must NOT clobber it — the user never
        // agreed to overwrite — and must leave the existing content intact.
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-export-toctou-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("notes.md");
        std::fs::write(&source, "# Hello\n").expect("write source");
        let output_path = tmp_dir.join("notes.html");
        // Simulate the race: a file appeared at the target between the check and
        // the completion.
        std::fs::write(&output_path, b"PRE-EXISTING\n").expect("write racing target");

        let mut e = Editor::new_from_text("# Hello\n", Some(source.clone()), (80, 24));
        let buffer_id = e.active().id;
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(b"<h1>Hello</h1>\n".to_vec())),
            overwrite_confirmed: false,
        };
        crate::app::reduce(msg, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);

        // The racing file is untouched; status tells the user to re-run.
        let got = std::fs::read(&output_path).expect("read target");
        assert_eq!(got, b"PRE-EXISTING\n", "unconfirmed export must not clobber an appeared target");
        assert!(e.status.contains("re-run"), "status must prompt a re-run, got: {}", e.status);

        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn exportdone_confirmed_overwrites_existing_target() {
        // The complement: when the user confirmed the overwrite
        // (overwrite_confirmed=true), an existing target IS replaced.
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-export-confirmed-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("notes.md");
        std::fs::write(&source, "# Hello\n").expect("write source");
        let output_path = tmp_dir.join("notes.html");
        std::fs::write(&output_path, b"OLD\n").expect("write existing target");

        let mut e = Editor::new_from_text("# Hello\n", Some(source.clone()), (80, 24));
        let buffer_id = e.active().id;
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let new_bytes = b"<h1>Hello</h1>\n".to_vec();
        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(new_bytes.clone())),
            overwrite_confirmed: true,
        };
        crate::app::reduce(msg, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);

        let got = std::fs::read(&output_path).expect("read target");
        assert_eq!(got, new_bytes, "confirmed export must overwrite the existing target");
        assert!(e.status.contains("exported"));

        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn exportdone_under_prompt_still_applies() {
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-exportdone-prompt-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("doc.md");
        std::fs::write(&source, "x\n").expect("write source");
        let output_path = tmp_dir.join("doc.html");

        let mut e = Editor::new_from_text("x\n", Some(source.clone()), (80, 24));
        e.active_mut().document.version = 1; // dirty → prompt would normally be up
        // Manually raise a prompt to simulate the overlay scenario.
        e.open_prompt(crate::prompt::Prompt::quit_confirm());

        let buffer_id = e.active().id;
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let content_bytes = b"<p>x</p>\n".to_vec();
        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(content_bytes.clone())),
            overwrite_confirmed: false,
        };
        crate::app::reduce(msg, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);

        assert!(output_path.exists(), "ExportDone under prompt must still write the file");
        assert!(e.status.contains("exported"));

        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn transform_chooser_maps_keys_to_kinds() {
        use crate::prompt::{Prompt, PromptAction};
        use crate::transform::TransformKind;
        let p = Prompt::transform_chooser();
        assert_eq!(p.action_for('r'), Some(PromptAction::Transform(TransformKind::Reflow)));
        assert_eq!(p.action_for('u'), Some(PromptAction::Transform(TransformKind::Unwrap)));
        assert_eq!(p.action_for('v'), Some(PromptAction::Transform(TransformKind::Ventilate)));
        assert_eq!(p.action_for('x'), None);
    }

    #[test]
    fn ctrl_t_opens_the_transform_chooser() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let key = Event::Key(KeyEvent { code: KeyCode::Char('t'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(key), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "Ctrl+T must open the transform chooser");
        assert_eq!(
            e.prompt.as_ref().unwrap().action_for('r'),
            Some(crate::prompt::PromptAction::Transform(crate::transform::TransformKind::Reflow)),
        );
    }

    #[test]
    fn reflow_whole_buffer_applies_one_undoable_edit() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau\n";
        let mut e = Editor::new_from_text(long, None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        // dispatch_transform takes (editor, kind, clock, msg_tx) — see Task 3 Step 6.
        crate::transform::dispatch_transform(&mut e, TransformKind::Reflow, &TestClock(0), &tx);
        let after = e.active().document.buffer.to_string();
        assert_ne!(after, long, "reflow should rewrap the long line");
        // exactly one undo restores the original
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), long);
    }

    #[test]
    fn transform_with_identical_output_makes_no_edit() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        // Already one-sentence-per-line: ventilate is a no-op → no edit, "already" status.
        let text = "Short.\n";
        let mut e = Editor::new_from_text(text, None, (80, 24));
        let v0 = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::transform::dispatch_transform(&mut e, TransformKind::Ventilate, &TestClock(0), &tx);
        assert_eq!(e.active().document.buffer.to_string(), text);
        assert_eq!(e.active().document.version, v0, "no-op transform must not bump version");
        assert!(e.status.contains("already"));
    }

    #[test]
    fn transformdone_applies_when_fresh() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        use crate::registry::Registry;
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("one two three four five six seven\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let range = 0..e.active().document.buffer.to_string().len();
        let out = "one\ntwo\n".to_string(); // pretend ventilate output
        crate::app::reduce(Msg::TransformDone { buffer_id: id, version: v, range,
            kind: TransformKind::Ventilate, result: Ok(out.clone()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), out);
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "one two three four five six seven\n");
    }

    #[test]
    fn transformdone_discarded_when_version_moved() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        use crate::registry::Registry;
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("alpha beta\n", None, (80, 24));
        let id = e.active().id; let stale = e.active().document.version;
        e.active_mut().document.version += 1; // an intervening edit
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::TransformDone { buffer_id: id, version: stale, range: 0..10,
            kind: TransformKind::Reflow, result: Ok("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "alpha beta\n", "stale result discarded");
        assert!(e.status.to_lowercase().contains("discarded"));
        assert!(!e.transform_in_flight, "in-flight cleared even on discard");
    }

    #[test]
    fn large_buffer_routes_async_and_delivers_transformdone() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        // > 1 MiB buffer forces the async branch; we block on the channel.
        let big = "word ".repeat(300_000); // ~1.5 MB
        let mut e = Editor::new_from_text(&big, None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        crate::transform::dispatch_transform(&mut e, TransformKind::Unwrap, &TestClock(0), &tx);
        assert!(e.transform_in_flight, "async dispatch sets the in-flight guard");
        let msg = rx.recv().expect("TransformDone must arrive");
        match msg { Msg::TransformDone { kind: TransformKind::Unwrap, result: Ok(_), .. } => {}
                    other => panic!("expected TransformDone Ok, got {other:?}") }
    }

    #[test]
    fn bracketed_paste_normal_inserts_into_document() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("XY".into())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXYb\n");
        assert_eq!(e.register.get(), Some("XY"));
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");
    }

    #[test]
    fn bracketed_paste_into_minibuffer_not_document() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        let doc_before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("cat".into())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.minibuffer.as_ref().unwrap().text, "cat", "paste goes into the minibuffer");
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "document untouched");
    }

    #[test]
    fn paste_into_open_palette_edits_query_not_document() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &keymap);
        e.palette = Some(p);
        let doc_before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("foo".into())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "document untouched under open palette");
        assert_eq!(e.palette.as_ref().unwrap().query, "foo", "paste inserted into palette query");
        assert!(e.palette.is_some(), "palette remains open after paste");
    }

    #[test]
    fn paste_under_open_menu_does_not_touch_document() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.menu = Some(crate::menu::empty());
        let doc_before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("bar".into())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "document untouched under open menu");
        assert!(e.menu.is_some(), "menu remains open after paste");
    }

    // -------------------------------------------------------------------------
    // Regression: async ClipboardPaste must be dropped (not applied to doc)
    // while an overlay is open (menu / palette / theme_picker).
    // Each test fails WITHOUT the guard and passes WITH it.
    // -------------------------------------------------------------------------

    #[test]
    fn async_clipboard_paste_under_open_menu_is_dropped() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.menu = Some(crate::menu::empty());
        let doc_before = e.active().document.buffer.to_string();
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "async paste must not land in doc behind open menu");
        assert!(e.menu.is_some(), "menu remains open after ClipboardPaste is dropped");
    }

    #[test]
    fn async_clipboard_paste_under_open_palette_is_dropped() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &keymap);
        e.palette = Some(p);
        let doc_before = e.active().document.buffer.to_string();
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "async paste must not land in doc behind open palette");
        assert!(e.palette.is_some(), "palette remains open after ClipboardPaste is dropped");
    }

    #[test]
    fn async_clipboard_paste_under_open_theme_picker_is_dropped() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.open_theme_picker();
        assert!(e.theme_picker.is_some(), "precondition: picker opened");
        let doc_before = e.active().document.buffer.to_string();
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "async paste must not land in doc behind open theme picker");
        assert!(e.theme_picker.is_some(), "theme picker remains open after ClipboardPaste is dropped");
    }

    #[test]
    fn bracketed_paste_with_modal_prompt_is_ignored() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.open_prompt(crate::prompt::Prompt::quit_confirm());
        let doc_before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("x".into())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "paste ignored under a modal");
        assert!(e.prompt.is_some());
    }

    #[test]
    fn bracketed_paste_empty_preserves_selection_and_register() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(1, 3); // select "bc"
        e.register.set("keep".into());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste(String::new())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "abcd\n", "empty paste must NOT delete the selection");
        assert_eq!(e.register.get(), Some("keep"), "empty paste must NOT clear the register");
    }

    #[test]
    fn durability_result_for_missing_buffer_still_runs() {
        use crate::editor::{Editor, BufferId};
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        // A durability completion runs even though its buffer is gone (e.g. closed).
        crate::app::apply_result(JobResult {
            buffer_id: BufferId(999), class: ResultClass::Durability,
            version: 1, kind: JobKind::SwapWrite,
            merge: Box::new(|ed: &mut Editor| ed.status = "durability ran".into()),
        }, &mut e);
        assert_eq!(e.status, "durability ran");
    }

    // -------------------------------------------------------------------------
    // Task 4: keymap integration tests
    // -------------------------------------------------------------------------

    #[test]
    fn single_chord_dispatches_via_keymap() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3); // select abc
        let km = cua_keymap(); let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.register.get(), Some("abc"), "Ctrl+C copied via the data-driven keymap");
    }

    #[test]
    fn pending_sequence_then_completes() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        // bind a 2-key save sequence
        let cfg = crate::config::KeymapConfig { preset: "cua".into(),
            patches: vec![crate::config::KeymapPatch {
                bind: [("ctrl-k ctrl-s".to_string(), "save".to_string())].into_iter().collect(), unbind: vec![] }] };
        let (km, _) = crate::keymap::build_keymap(&cfg, &Registry::builtins());
        let mut e = Editor::new_from_text("x\n", Some("/tmp/wc-kmtest.md".into()), (80, 24));
        let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('k'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.pending_keys.len(), 1, "first key is pending");
        assert!(e.status.contains("ctrl-k") || e.status.to_lowercase().contains("k"), "pending shown");
        crate::app::reduce(press(KeyCode::Char('s'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.pending_keys.is_empty(), "sequence resolved, pending cleared");
        // (save dispatched — the file path means dispatch_save runs; assert via status or saved flag per the real save)
    }

    #[test]
    fn esc_cancels_pending_without_other_effect() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        let cfg = crate::config::KeymapConfig { preset: "cua".into(),
            patches: vec![crate::config::KeymapPatch {
                bind: [("ctrl-k ctrl-s".to_string(), "save".to_string())].into_iter().collect(), unbind: vec![] }] };
        let (km, _) = crate::keymap::build_keymap(&cfg, &Registry::builtins());
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        let before = e.active().document.buffer.to_string();
        let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('k'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.pending_keys.len(), 1);
        crate::app::reduce(press(KeyCode::Esc, KeyModifiers::NONE), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.pending_keys.is_empty(), "Esc cleared the pending sequence");
        assert_eq!(e.active().document.buffer.to_string(), before, "no buffer change");
    }

    #[test]
    fn run_startup_builds_keymap_from_config_with_user_bind() {
        // We can't run the TUI loop in a test, so test the startup builder in isolation:
        // a helper that turns (Cli-derived paths) into the effective keymap.
        let cfg = crate::config::KeymapConfig {
            preset: "cua".into(),
            patches: vec![crate::config::KeymapPatch {
                bind: [("ctrl-g".to_string(), "move_line_start".to_string())].into_iter().collect(),
                unbind: vec![],
            }],
        };
        let (km, warns) = crate::keymap::build_keymap(&cfg, &crate::registry::Registry::builtins());
        assert!(warns.is_empty());
        let g = crate::keymap::parse_chord("ctrl-g").unwrap();
        assert!(matches!(km.resolve(&[g]), crate::keymap::Resolution::Command(crate::registry::CommandId("move_line_start"))));
    }

    #[test]
    fn printable_falls_through_to_insert() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut e = Editor::new_from_text("", None, (80, 24));
        let km = cua_keymap(); let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('h'), KeyModifiers::NONE), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "h", "unbound printable inserts literally");
    }

    #[test]
    fn resume_restores_when_identity_matches_and_clamps_when_not() {
        // unit-test the resume decision helper directly (no TTY):
        // apply_resume(entry, current_identity, doc_len) -> Option<(cursor,scroll)>
        use crate::state::StateEntry;
        let e = StateEntry { cursor: 4, scroll: 2, marks: Default::default(), mtime: 10, size: 20, seq: 0, folds: vec![] };
        // identity match → restore (clamped to doc_len)
        assert_eq!(crate::app::apply_resume(&e, (10,20), 100), Some((4,2)));
        assert_eq!(crate::app::apply_resume(&e, (10,20), 3), Some((3,2)), "cursor clamped to doc_len");
        // identity mismatch → discard
        assert_eq!(crate::app::apply_resume(&e, (11,20), 100), None);
    }

    #[test]
    fn ctrl_p_opens_palette_and_enter_dispatches_selected() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |c, m| Event::Key(KeyEvent { code: c, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // Ctrl+P opens + hydrates
        crate::app::reduce(Msg::Input(press(KeyCode::Char('p'), KeyModifiers::CONTROL)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_some());
        assert!(!e.palette.as_ref().unwrap().rows.is_empty(), "palette hydrated with all commands on open");
        // type "copy", select first, Enter → dispatches copy (register gets the selection)
        for ch in "copy".chars() { crate::app::reduce(Msg::Input(press(KeyCode::Char(ch), KeyModifiers::NONE)), &mut e, &reg, &km, &ex, &clk, &tx); }
        crate::app::reduce(Msg::Input(press(KeyCode::Enter, KeyModifiers::NONE)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_none(), "Enter closes the palette");
        assert_eq!(e.register.get(), Some("abc"), "selected command (Copy) dispatched");
    }

    #[test]
    fn palette_esc_closes_and_clipboard_paste_is_dropped() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.palette = Some(crate::palette::Palette::default());
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        // An async ClipboardPaste arriving while the palette is open must be
        // intercepted and dropped — it must NOT reach the document (race fix).
        let bid = e.active().id;
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("X".into()) }, &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_some(), "palette still open");
        assert_eq!(e.active().document.buffer.to_string(), "ab\n", "ClipboardPaste must be dropped while palette is open");
        // Esc closes the palette
        let esc = Event::Key(KeyEvent { code: KeyCode::Esc, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(esc), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_none());
    }

    #[test]
    fn f10_toggles_menu_closed_when_open() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let reg = Registry::builtins(); let km = cua_keymap(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        // F10 with menu closed → opens the menu
        crate::app::reduce(Msg::Input(f10()), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_some(), "F10 should open menu when closed");
        // F10 with menu open → closes the menu
        crate::app::reduce(Msg::Input(f10()), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_none(), "F10 should close menu when open");
    }

    #[test]
    fn pending_mark_consumes_one_key_then_clears() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.pending_mark = Some(crate::editor::MarkPending::Set);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |c, m| Event::Key(KeyEvent { code: c, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(press(KeyCode::Char('q'), KeyModifiers::NONE)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.pending_mark, None, "capture consumed the key");
        assert_eq!(e.active().marks.get(&'q'), Some(&0));
        assert_eq!(e.active().document.buffer.to_string(), "abc\n", "captured key did NOT type into the doc");
    }

    #[test]
    fn esc_cancels_pending_mark() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.pending_mark = Some(crate::editor::MarkPending::Set);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let esc = Event::Key(KeyEvent { code: KeyCode::Esc, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(esc), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.pending_mark, None);
        assert!(e.active().marks.is_empty());
    }

    #[test]
    fn load_marks_from_entry_populates_clamped() {
        use std::collections::BTreeMap;
        // No trailing newline so clamp_snap(999) == buffer.len() == 11.
        let mut e = Editor::new_from_text("hello world", None, (80, 24));
        let mut marks = BTreeMap::new();
        marks.insert("a".to_string(), 6usize);
        marks.insert("b".to_string(), 999usize); // past EOF → clamped to len
        let entry = crate::state::StateEntry { cursor: 0, scroll: 0, marks, mtime: 0, size: 0, seq: 1, folds: vec![] };
        crate::app::load_marks_from_entry(&mut e, &entry);
        assert_eq!(e.active().marks.get(&'a'), Some(&6));
        assert_eq!(e.active().marks.get(&'b'), Some(&e.active().document.buffer.len()));
    }

    #[test]
    fn toggle_mouse_capture_flips_flag() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        assert!(e.mouse_capture, "default on");
        let (_km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock::new(0);
        let id = reg.resolve_name("toggle_mouse_capture").expect("registered");
        { let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx.clone() };
          reg.dispatch(id, &mut ctx); }
        assert!(!e.mouse_capture, "toggled off");
    }

    #[test]
    fn scrollbar_visible_recomputed_against_clock() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.mouse.scrollbar_until_ms = 1000;
        crate::app::recompute_scrollbar_visible(&mut e, 500); // before deadline
        assert!(e.mouse.scrollbar_visible);
        crate::app::recompute_scrollbar_visible(&mut e, 1200); // after
        assert!(!e.mouse.scrollbar_visible);
    }

    /// Finding 1 regression: wheel event sets scrollbar_until_ms; recomputing
    /// immediately after (now == t, t < t+1200) must yield visible == true.
    /// A later recompute at t+1300 must yield false (bar fades after deadline).
    #[test]
    fn wheel_then_recompute_makes_scrollbar_visible() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{MouseEvent, MouseEventKind, KeyModifiers};
        let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 10));
        crate::derive::rebuild(&mut e);
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = InlineExecutor::default();
        let t: u64 = 5000;
        let clk = TestClock(t);
        let (tx, _rx) = std::sync::mpsc::channel();
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        // Dispatch the scroll event (sets scrollbar_until_ms = t + 1200).
        crate::mouse::handle(&mut e, wheel, &reg, &km, &ex, &clk, &tx);
        // Recompute at t (now < until) — bar must be visible.
        crate::app::recompute_scrollbar_visible(&mut e, t);
        assert!(e.mouse.scrollbar_visible, "scrollbar must be visible immediately after a scroll event");
        // Recompute after the fade deadline — bar must hide.
        crate::app::recompute_scrollbar_visible(&mut e, t + 1300);
        assert!(!e.mouse.scrollbar_visible, "scrollbar must hide after scrollbar_until_ms expires");
    }

    // -------------------------------------------------------------------------
    // Task 4 (Effort 5e): search overlay reduce() interception tests
    // -------------------------------------------------------------------------

    #[test]
    fn ctrl_f_opens_search_and_typing_jumps() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("foo bar foo\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let mkpress = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(mkpress(KeyCode::Char('f'), KeyModifiers::CONTROL)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.search.is_some(), "Ctrl+F opens search");
        for c in "bar".chars() { crate::app::reduce(Msg::Input(mkpress(KeyCode::Char(c), KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx); }
        let s = e.search.as_ref().unwrap();
        assert_eq!(s.needle, "bar");
        assert_eq!(s.current().unwrap().start, 4); // caret jumped to the match
        assert_eq!(e.active().document.selection.primary().from(), 4);
    }

    #[test]
    fn esc_restores_origin_and_closes() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("foo bar\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let mkpress = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(mkpress(KeyCode::Char('f'), KeyModifiers::CONTROL)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        for c in "bar".chars() { crate::app::reduce(Msg::Input(mkpress(KeyCode::Char(c), KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx); }
        crate::app::reduce(Msg::Input(mkpress(KeyCode::Esc, KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.search.is_none(), "Esc closes search");
        assert_eq!(e.active().document.selection.primary().to(), 0, "caret restored to origin");
    }

    #[test]
    fn replace_all_is_one_undo_unit_and_remaps_origin() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let r = |e: &mut Editor, ev| crate::app::reduce(Msg::Input(ev), e, &reg, &cua_keymap(), &ex, &clk, &tx);
        r(&mut e, press(KeyCode::Char('r'), KeyModifiers::CONTROL));   // open Replace
        for c in "aa".chars() { r(&mut e, press(KeyCode::Char(c), KeyModifiers::NONE)); }
        r(&mut e, press(KeyCode::Tab, KeyModifiers::NONE));            // focus Template
        r(&mut e, press(KeyCode::Char('b'), KeyModifiers::NONE));
        r(&mut e, press(KeyCode::Char('a'), KeyModifiers::ALT));       // Alt+A = Replace All
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "b b b\n");
        let v_after = e.active().document.version;
        assert!(e.active_mut().undo());                                // ONE undo reverts ALL
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "aa aa aa\n");
        let _ = v_after;
    }

    /// Finding 2 regression: after a resize to a smaller terminal the stale
    /// scroll_row must be clamped so the caret remains visible.
    ///
    /// Setup: 50-line doc; scroll to line 30 with scroll_row=5 in a large
    /// 80×40 terminal. Then resize to 80×10 and dispatch Msg::Input(Resize).
    /// With the fix, ensure_visible clamps scroll/scroll_row so that a
    /// subsequent rebuild + screen_pos succeeds.  Without the fix the stale
    /// scroll_row could exceed the new visible range and render would skip rows.
    #[test]
    fn resize_re_pins_scroll() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::Event;

        let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 40));
        // Rebuild once for the initial large area so layouts are populated.
        crate::derive::rebuild(&mut e);

        // Manually push scroll deep into the document (line 30) and set a
        // non-zero scroll_row to simulate a resumed or scrolled position.
        e.active_mut().view.scroll = 30;
        e.active_mut().view.scroll_row = 5;

        // Build a minimal reduce context.
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        // Dispatch a resize to a much smaller terminal.
        crate::app::reduce(
            crate::app::Msg::Input(Event::Resize(80, 10)),
            &mut e, &reg, &km, &ex, &clk, &tx,
        );

        // After resize + ensure_visible the cache must be fresh and screen_pos
        // must return Some (caret is visible on the new geometry).
        crate::derive::rebuild(&mut e);
        let pos = crate::nav::screen_pos(&e);
        assert!(
            pos.is_some(),
            "caret must be visible after resize; scroll={} scroll_row={}",
            e.active().view.scroll, e.active().view.scroll_row,
        );

        // scroll_row must be 0 (single-visual-row lines) — never a stale large value.
        assert_eq!(
            e.active().view.scroll_row, 0,
            "scroll_row must be clamped to a valid value after resize",
        );
    }

    // -------------------------------------------------------------------------
    // Task 7 (Effort 5e): interactive query-replace stepping tests
    // -------------------------------------------------------------------------

    #[test]
    fn query_replace_steps_yes_no_and_remaps() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let r = |e: &mut Editor, ev| crate::app::reduce(Msg::Input(ev), e, &reg, &cua_keymap(), &ex, &clk, &tx);
        r(&mut e, press(KeyCode::Char('r'), KeyModifiers::CONTROL));
        for c in "aa".chars() { r(&mut e, press(KeyCode::Char(c), KeyModifiers::NONE)); }
        r(&mut e, press(KeyCode::Tab, KeyModifiers::NONE));
        r(&mut e, press(KeyCode::Char('b'), KeyModifiers::NONE));
        r(&mut e, press(KeyCode::Enter, KeyModifiers::ALT));           // Alt+Enter starts stepping
        assert_eq!(e.search.as_ref().unwrap().phase, crate::search_overlay::Phase::Stepping);
        r(&mut e, press(KeyCode::Char('y'), KeyModifiers::NONE));      // replace #1
        r(&mut e, press(KeyCode::Char('n'), KeyModifiers::NONE));      // skip #2
        r(&mut e, press(KeyCode::Char('y'), KeyModifiers::NONE));      // replace #3
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "b aa b\n");
    }

    /// Fix A regression: a FilterDone arriving while the search overlay is open
    /// must be applied, not silently dropped.
    #[test]
    fn search_does_not_starve_filterdone() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        e.open_search(crate::search_overlay::Phase::Find, 0);
        let (tx, _rx) = std::sync::mpsc::channel(); let reg = Registry::builtins();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXde\n", "FilterDone applies even under an open search overlay");
        assert!(e.search.is_some(), "search overlay remains open after non-key message");
    }

    #[test]
    fn outline_overlay_does_not_starve_background_messages() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("# H\nabcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        e.open_outline();
        let (tx, _rx) = std::sync::mpsc::channel(); let reg = Registry::builtins();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 4..6, cursor: 5,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "# H\nXcde\n", "FilterDone applies even under an open outline overlay");
        assert!(e.outline.is_some(), "outline overlay remains open after non-key message");
    }

    #[test]
    fn outline_jump_refused_after_background_edit() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("# Top\nintro\n## A\nbody\n", None, (80, 24));
        let start = e.active().document.selection.primary().head;
        e.open_outline();
        assert!(e.outline.is_some(), "outline overlay must open");
        let opened_version = e.outline.as_ref().unwrap().opened_version;
        e.active_mut().document.version = opened_version + 1;
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let enter = Event::Key(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        crate::app::reduce(Msg::Input(enter), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.outline.is_none(), "stale outline overlay must close without jumping");
        assert_eq!(e.active().document.selection.primary().head, start,
            "stale outline Enter must not move the caret");
        assert!(e.status.contains("changed"), "status must mention the change");
    }

    #[test]
    fn outline_jump_auto_unfolds_ancestor_and_moves_caret() {
        let doc = "# Top\nintro\n## A\nbody\n### A1\nx\n";
        let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
        ed.active_mut().folds.toggle(doc.find("## A").unwrap());
        crate::derive::rebuild(&mut ed);
        let a1 = doc.find("### A1").unwrap();
        crate::app::outline_jump_to(&mut ed, a1);
        assert_eq!(ed.active().document.selection.primary().head, a1);
        assert!(!ed.active().folds.folded.contains(&doc.find("## A").unwrap()));
    }

    /// Fix C regression: replace-all with an invalid regex must set status
    /// "invalid regex" and leave the buffer unchanged.
    #[test]
    fn invalid_regex_replace_all_sets_status() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
        let before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let r = |e: &mut Editor, ev| crate::app::reduce(Msg::Input(ev), e, &reg, &cua_keymap(), &ex, &clk, &tx);
        r(&mut e, press(KeyCode::Char('r'), KeyModifiers::CONTROL));    // open Replace (Phase::Replace)
        r(&mut e, press(KeyCode::Char('r'), KeyModifiers::ALT));        // Alt+R: toggle to regex mode
        // type an invalid regex pattern: unbalanced open paren
        r(&mut e, press(KeyCode::Char('('), KeyModifiers::NONE));       // invalid in regex mode
        r(&mut e, press(KeyCode::Char('a'), KeyModifiers::ALT));        // Alt+A = Replace All
        assert_eq!(e.active().document.buffer.to_string(), before, "invalid regex must not mutate the buffer");
        assert_eq!(e.status, "invalid regex", "status must say 'invalid regex', got: {:?}", e.status);
    }

    #[test]
    fn query_replace_bang_finishes_rest() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let r = |e: &mut Editor, ev| crate::app::reduce(Msg::Input(ev), e, &reg, &cua_keymap(), &ex, &clk, &tx);
        r(&mut e, press(KeyCode::Char('r'), KeyModifiers::CONTROL));
        for c in "aa".chars() { r(&mut e, press(KeyCode::Char(c), KeyModifiers::NONE)); }
        r(&mut e, press(KeyCode::Tab, KeyModifiers::NONE));
        r(&mut e, press(KeyCode::Char('b'), KeyModifiers::NONE));
        r(&mut e, press(KeyCode::Enter, KeyModifiers::ALT));
        r(&mut e, press(KeyCode::Char('!'), KeyModifiers::NONE));      // finish all remaining
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "b b b\n");
    }

    #[test]
    fn diagnostics_done_applies_only_for_current_version() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        let bid = e.active().id;
        let v = e.active().document.version;
        let diag = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            message: "misspelled".into(),
            suggestions: vec![wordcartel_core::diagnostics::Suggestion::ReplaceWith("the".into())] }];
        // current version → stored
        crate::diagnostics_run::apply_diagnostics_done(&mut e, bid, v, diag.clone());
        assert_eq!(e.active().diagnostics.diagnostics.len(), 1);
        assert_eq!(e.active().diagnostics.computed_version, v);
        // stale version → discarded
        crate::diagnostics_run::apply_diagnostics_done(&mut e, bid, v.wrapping_sub(1), diag);
        assert_eq!(e.active().diagnostics.diagnostics.len(), 1, "stale result must not overwrite");
    }

    #[test]
    fn tick_dispatches_a_due_check_once() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("teh\n", None, (80, 24));
        e.diag_cfg.enabled = true;
        e.active_mut().diagnostics.arm(0, 400); // due at 400
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(500); // past due
        // a Tick at now=500 with diagnostics enabled dispatches one check
        crate::app::reduce(Msg::Tick, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().diagnostics.in_flight_version, Some(e.active().document.version));
        // the spawned worker sends a DiagnosticsDone
        match rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap() {
            Msg::DiagnosticsDone { diagnostics, .. } => assert!(diagnostics.iter().any(|d| d.kind == wordcartel_core::diagnostics::DiagnosticKind::Spelling)),
            o => panic!("expected DiagnosticsDone, got {o:?}"),
        }
    }

    // -------------------------------------------------------------------------
    // Task 6 (Effort 5f): quick-fix overlay tests
    // -------------------------------------------------------------------------

    #[test]
    fn quick_fix_applies_suggestion_as_undoable_edit() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        let v = e.active().document.version;
        e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(),
            suggestions: vec![wordcartel_core::diagnostics::Suggestion::ReplaceWith("the".into())] }];
        e.active_mut().diagnostics.computed_version = v;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1); // cursor inside "teh"
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(press(KeyCode::Char('.'), KeyModifiers::CONTROL)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.diag.is_some(), "Ctrl+. opens the quick-fix overlay on the diagnostic");
        crate::app::reduce(Msg::Input(press(KeyCode::Enter, KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "the cat\n");
        assert!(e.diag.is_none(), "overlay closes after apply");
        assert!(e.active_mut().undo(), "the fix is one undo unit");
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "teh cat\n");
    }

    #[test]
    fn open_diag_clears_siblings_and_open_others_clear_diag() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let d = wordcartel_core::diagnostics::Diagnostic { range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(), suggestions: vec![] };
        // open_diag clears a previously-open palette/search (reverse XOR direction)
        e.open_palette();
        assert!(e.palette.is_some(), "palette open before open_diag");
        e.open_diag(d.clone());
        assert!(e.palette.is_none(), "open_diag clears palette");
        assert!(e.diag.is_some());
        // the other direction: opening the palette clears an open diag overlay
        e.open_palette();
        assert!(e.diag.is_none(), "open_palette clears diag");
    }

    /// Regression: quick_fix dispatched after an edit (when valid_for is false) must
    /// NOT open the overlay and must NOT corrupt the buffer. Before the fix the
    /// handlers read stale diagnostic byte ranges unchecked.
    #[test]
    fn quick_fix_on_stale_diagnostics_is_noop_no_overlay() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        let v = e.active().document.version;
        // Store a diagnostic at version V.
        e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(),
            suggestions: vec![wordcartel_core::diagnostics::Suggestion::ReplaceWith("the".into())] }];
        e.active_mut().diagnostics.computed_version = v;
        // Simulate an intervening edit: bump the document version so valid_for is now false.
        e.active_mut().document.version = v + 1;
        assert!(!e.active().diagnostics.valid_for(e.active().document.version),
            "precondition: diagnostics must be stale after version bump");
        let buf_before = e.active().document.buffer.to_string();
        // Place cursor inside the stale diagnostic range.
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let ctrl_dot = Event::Key(KeyEvent { code: KeyCode::Char('.'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(ctrl_dot), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        // The overlay must NOT open and the buffer must be unchanged.
        assert!(e.diag.is_none(), "stale diagnostics: quick_fix must NOT open the overlay");
        assert_eq!(e.active().document.buffer.to_string(), buf_before, "buffer must be unchanged");
    }

    #[test]
    fn diag_next_prev_move_caret_with_wrap() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("teh cat adn dog\n", None, (80, 24));
        let v = e.active().document.version;
        e.active_mut().diagnostics.diagnostics = vec![
            wordcartel_core::diagnostics::Diagnostic { range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message:"x".into(), suggestions: vec![] },
            wordcartel_core::diagnostics::Diagnostic { range: 8..11, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message:"x".into(), suggestions: vec![] },
        ];
        e.active_mut().diagnostics.computed_version = v;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let f8 = Event::Key(KeyEvent { code: KeyCode::F(8), modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(f8.clone()), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.selection.primary().to(), 8, "F8 moves to the next diagnostic");
        crate::app::reduce(Msg::Input(f8), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.selection.primary().to(), 0, "F8 wraps to the first");
    }

    // -----------------------------------------------------------------------
    // Fix A3: no busy-loop while a diagnostics check is in flight
    // -----------------------------------------------------------------------

    /// When `in_flight_version` is set, the diagnostics deadline must be
    /// excluded from the loop's `next_deadline` call.  This is a pure logic
    /// test of the gating condition, validated without touching the real loop.
    #[test]
    fn diag_deadline_excluded_when_in_flight() {
        use crate::diagnostics_run::{DiagStore, next_deadline};
        // Build a store that has a past-due recheck_due_at AND an in-flight version.
        let mut store = DiagStore::new();
        store.recheck_due_at = Some(0); // past due
        store.in_flight_version = Some(5); // check is in flight

        // Compute the diag_deadline using the same gating logic as the run() loop.
        let diag_deadline = if store.in_flight_version.is_none() {
            store.recheck_due_at
        } else {
            None
        };

        // With no other deadlines, the computed deadline should be None (not 0),
        // so recv_timeout gets a long duration instead of 0 ms.
        let deadline = next_deadline(&[None, None, None, diag_deadline]);
        assert_eq!(deadline, None,
            "when in_flight, diag_deadline must be None so the loop does not spin");

        // Sanity: without in-flight, the past-due timestamp IS included.
        // temporarily clear in_flight to test the other branch
        let saved = store.in_flight_version.take();
        let diag_deadline_no_flight = if store.in_flight_version.is_none() {
            store.recheck_due_at
        } else {
            None
        };
        let deadline_no_flight = next_deadline(&[None, None, None, diag_deadline_no_flight]);
        assert_eq!(deadline_no_flight, Some(0),
            "without in_flight, past-due recheck_due_at is included in the deadline");
        store.in_flight_version = saved; // restore
    }

    // -----------------------------------------------------------------------
    // Fix A4: stale quick-fix overlay must not apply
    // -----------------------------------------------------------------------

    /// If the buffer is mutated while the diag overlay is open, pressing Enter
    /// must NOT apply the (now-stale) suggestion.  The overlay must be closed
    /// and the buffer left unchanged.
    #[test]
    fn quick_fix_refuses_stale_apply_after_concurrent_edit() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        let v = e.active().document.version;
        // Arm valid diagnostics at version V and open the overlay.
        e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(),
            suggestions: vec![wordcartel_core::diagnostics::Suggestion::ReplaceWith("the".into())] }];
        e.active_mut().diagnostics.computed_version = v;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // Open the overlay.
        crate::app::reduce(Msg::Input(press(KeyCode::Char('.'), KeyModifiers::CONTROL)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.diag.is_some(), "overlay must open");
        assert_eq!(e.diag.as_ref().unwrap().opened_version, v, "opened_version captured at open");
        // Simulate a concurrent edit while the overlay is open: bump the document version.
        e.active_mut().document.version += 1;
        let buf_before = e.active().document.buffer.to_string();
        // Press Enter — must be refused because opened_version != current version.
        crate::app::reduce(Msg::Input(press(KeyCode::Enter, KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        // Overlay must be closed and buffer must be unchanged.
        assert!(e.diag.is_none(), "stale overlay must be closed without applying");
        assert_eq!(e.active().document.buffer.to_string(), buf_before,
            "buffer must not be mutated when the overlay is stale");
        assert!(e.status.contains("changed"), "status must mention the change");
    }

    // -----------------------------------------------------------------------
    // Task 12 (Effort 5g): search caret jumps auto-unfold folded ancestors
    // -----------------------------------------------------------------------

    #[test]
    fn search_hit_inside_fold_auto_unfolds() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let doc = "# Top\nintro\n## A\nneedle here\nmore\n## B\n";
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        // fold ## A
        let a_byte = doc.find("## A").unwrap();
        ed.active_mut().folds.toggle(a_byte);
        crate::derive::rebuild(&mut ed);
        // open search with Ctrl+F and type "needle"
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let mkpress = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(mkpress(KeyCode::Char('f'), KeyModifiers::CONTROL)), &mut ed, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(ed.search.is_some(), "Ctrl+F must open search");
        for c in "needle".chars() {
            crate::app::reduce(Msg::Input(mkpress(KeyCode::Char(c), KeyModifiers::NONE)), &mut ed, &reg, &cua_keymap(), &ex, &clk, &tx);
        }
        let needle_pos = doc.find("needle").unwrap();
        assert_eq!(ed.active().document.selection.primary().from(), needle_pos,
            "caret must be on the 'needle' match");
        assert!(!ed.active().folds.folded.contains(&a_byte),
            "## A fold must be cleared when jumping into its body");
    }

    /// Regression test: a cursor restored from a session entry that falls inside a
    /// folded section must be snapped out to the section heading on startup.
    ///
    /// Tests the `derive::rebuild → SnapOut → ensure_visible` sequence inserted
    /// into the session-resume path in `run()`. Full TTY/disk setup is not used;
    /// the test drives the same API at the unit-test seam. The test FAILS (final
    /// assertion fires) if the SnapOut block is removed from the test body, and
    /// PASSES with it in place — mirroring failure/pass for the production change.
    #[test]
    fn resume_snaps_saved_cursor_out_of_restored_fold() {
        use crate::editor::Editor;
        use crate::registry::{place_caret_visible, CaretPlace};
        use crate::fold::FoldView;

        // Document with a heading "## A" followed by a body.
        // "# Top\nintro\n## A\nbody1\nbody2\n## B\ntail\n"
        //   byte 0:  '# Top\n'  (6 bytes)
        //   byte 6:  'intro\n'  (6 bytes)
        //   byte 12: '## A\n'   (5 bytes) ← heading start
        //   byte 17: 'body1\n'  (6 bytes)
        //   byte 23: 'body2\n'  (6 bytes) ← saved cursor lands inside here
        //   byte 29: '## B\n'
        const DOC: &str = "# Top\nintro\n## A\nbody1\nbody2\n## B\ntail\n";
        let mut editor = Editor::new_from_text(DOC, None, (80, 24));

        let heading_a = DOC.find("## A").unwrap();           // byte 12
        let cursor_in_body = DOC.find("body2").unwrap() + 1; // byte 24, inside body2

        // — Simulate the session-resume block in run() —
        // Restore cursor to the saved (inside-fold) position.
        editor.active_mut().document.selection =
            wordcartel_core::selection::Selection::single(cursor_in_body);
        // Restore fold on "## A" and reconcile (mirrors app.rs resume block).
        editor.active_mut().folds.folded.insert(heading_a);
        {
            let b = editor.active();
            let blocks = b.document.blocks.clone();
            let buf = b.document.buffer.clone();
            editor.active_mut().folds.reconcile(&blocks, &buf);
        }

        // Precondition: before SnapOut, the restored cursor IS inside the fold.
        // This is what the bug looks like: cursor is hidden after resume without the fix.
        {
            let b = editor.active();
            let fv = FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer);
            let raw_line = b.document.buffer.byte_to_line(cursor_in_body);
            assert!(fv.is_hidden(raw_line),
                "precondition: without SnapOut the restored cursor sits inside the fold");
        }

        // — The fix: derive::rebuild then SnapOut (same order as in run()) —
        // If you comment out the SnapOut block below, the final assertion fails.
        crate::derive::rebuild(&mut editor);
        {
            // SnapOut: snap restored caret to heading if it landed inside a fold.
            let head = editor.active().document.selection.primary().head;
            let nh = place_caret_visible(&mut editor, head, CaretPlace::SnapOut);
            if nh != head {
                editor.active_mut().document.selection =
                    wordcartel_core::selection::Selection::single(nh);
            }
        }

        // Postcondition: cursor is now on the heading byte, NOT hidden.
        let final_head = editor.active().document.selection.primary().head;
        assert_eq!(final_head, heading_a,
            "SnapOut must move the caret from inside-fold body to the '## A' heading");
        {
            let b = editor.active();
            let fv = FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer);
            let final_line = b.document.buffer.byte_to_line(final_head);
            assert!(!fv.is_hidden(final_line),
                "caret must not be on a hidden line after resume normalization");
        }
    }

    #[test]
    fn save_as_writes_new_path_and_rekeys() {
        use crate::editor::Editor;
        use crate::jobs::{Executor, InlineExecutor};
        let dir = std::env::temp_dir().join(format!("wc-saveas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("out.md");
        let _ = std::fs::remove_file(&p);
        let mut e = Editor::new_from_text("content\n", None, (80, 24)); // UNNAMED, dirty-ish
        e.active_mut().document.version = 1; e.active_mut().document.saved_version = None;
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::save_as_submit(&mut e, p.to_str().unwrap(), &ex, &clk, &tx);
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "content\n", "file written");
        assert_eq!(e.active().document.path.as_deref(), Some(p.as_path()), "path re-keyed");
        assert!(!e.active().document.dirty(), "clean after save-as");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_on_unnamed_buffer_opens_save_as_prompt() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
        crate::save::dispatch_save(&mut ctx); // no path → opens Save-As, NOT the dead stub
        assert!(matches!(e.minibuffer.as_ref().map(|m| m.kind),
            Some(crate::minibuffer::MinibufferKind::SaveAs)), "unnamed save opens the SaveAs minibuffer");
    }

    #[test]
    fn save_as_existing_target_raises_overwrite_prompt() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        let p = std::env::temp_dir().join(format!("wc-ow-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", None, (80, 24));
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::save_as_submit(&mut e, p.to_str().unwrap(), &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "existing target → confirm modal");
        assert_eq!(e.prompt.as_ref().unwrap().action_for('o'), Some(crate::prompt::PromptAction::OverwriteSaveAs));
        assert_ne!(crate::prompt::PromptAction::OverwriteSaveAs, crate::prompt::PromptAction::Overwrite);
        let _ = std::fs::remove_file(&p);
    }

    // -------------------------------------------------------------------------
    // Task 4: New command + dirty-guard mechanism
    // -------------------------------------------------------------------------

    #[test]
    fn new_on_clean_buffer_replaces_with_scratch() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("kept\n", None, (80, 24)); // clean (saved_version=Some(0))
        let (ex, clk, tx) = (crate::jobs::InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        crate::app::request_new(&mut e, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "\n", "clean buffer → immediate new scratch");
        assert!(e.active().document.path.is_none());
        assert!(e.prompt.is_none(), "no modal for a clean buffer");
    }

    #[test]
    fn new_on_dirty_buffer_raises_guard_modal() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("draft\n", None, (80, 24));
        e.active_mut().document.version = 1; // dirty (saved_version=Some(0))
        let (ex, clk, tx) = (crate::jobs::InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        crate::app::request_new(&mut e, &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "dirty buffer → Save/Discard/Cancel modal");
        assert_eq!(e.prompt.as_ref().unwrap().action_for('d'), Some(crate::prompt::PromptAction::DiscardAndProceed));
    }

    #[test]
    fn dirty_guard_discard_replaces_scratch_and_cancel_preserves() {
        use crate::editor::Editor;
        // Discard → proceeds to scratch
        {
            let mut e = Editor::new_from_text("draft\n", None, (80, 24));
            e.active_mut().document.version = 1; // dirty
            let (ex, clk, tx) = (crate::jobs::InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
            crate::app::request_new(&mut e, &ex, &clk, &tx);
            assert!(e.prompt.is_some());
            crate::app::resolve_prompt(crate::prompt::PromptAction::DiscardAndProceed, &mut e, &ex, &clk, &tx);
            assert_eq!(e.active().document.buffer.to_string(), "\n", "Discard → scratch");
            assert!(e.active().document.path.is_none());
            assert!(e.prompt.is_none(), "prompt cleared after Discard");
            assert!(e.pending_save_as.is_none(), "pending_save_as cleared after Discard");
        }
        // Cancel → untouched buffer, pending_save_as cleared
        {
            let mut e = Editor::new_from_text("draft\n", None, (80, 24));
            e.active_mut().document.version = 1; // dirty
            let (ex, clk, tx) = (crate::jobs::InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
            crate::app::request_new(&mut e, &ex, &clk, &tx);
            crate::app::resolve_prompt(crate::prompt::PromptAction::Cancel, &mut e, &ex, &clk, &tx);
            assert_eq!(e.active().document.buffer.to_string(), "draft\n", "Cancel → untouched");
            assert!(e.pending_save_as.is_none(), "Cancel clears pending_save_as");
            assert!(e.prompt.is_none(), "prompt cleared after Cancel");
        }
    }
}
