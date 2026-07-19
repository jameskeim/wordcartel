//! Prompt submits & file dialogs: Save-As / Write-Block / New / go-to-line, and the
//! modal PromptAction resolver. Extracted verbatim from app.rs (Effort H1).

use crate::editor::Editor;
use crate::jobs::Executor;
use crate::registry::Ctx;
use crate::prompt::PromptAction;
use crate::app::Msg;
use crossterm::event::Event;
use wordcartel_core::history::Clock;

/// Active modal prompt intercepts KEY INPUT only (§5.3). Background results and ticks
/// must still be processed — a JobDone arriving while a modal is up (e.g. an
/// in-flight save completing during the quit-confirm prompt) must not be
/// dropped, or save&quit would hang waiting for a result it already discarded.
/// Consumes every message once admitted (Key + the five background-result arms + `_`) —
/// never returns Pass (§8.1-J).
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
    if editor.prompt.is_none() { return crate::app::Handled::Pass(msg); }
    match msg {
        Msg::Input(Event::Key(key)) if key.kind == crossterm::event::KeyEventKind::Press => {
            if key.code == crossterm::event::KeyCode::Esc {
                editor.prompt = None; // Esc cancels any prompt
                editor.pending_export = None;
                editor.pending_save_overwrite = None;
                // PAIRED with `pending_save_overwrite` above — see the doc comment on the
                // field. Esc on the OverwriteSaveAs modal must clear both, or a stale
                // `chosen` could pair with a different `resolved` on a later round trip.
                editor.pending_save_as_chosen = None;
                editor.pending_save_as = None;
                editor.pending_write_block = None;
                editor.pending_clean.clear(); // H5: Esc abandons the clean-recovery snapshot; delete nothing
                // Effort 6 / Codex gate #2: Esc on a per-buffer review prompt (raised
                // by drive_quit_drain) must abort the quit drain, just like Cancel does.
                // Without this, quit_drain stays Some-but-inert: the drain is
                // stranded with no in-flight save and no re-drive pending.
                if editor.quit_drain.is_some() {
                    editor.quit_drain = None;
                    editor.quit_drain_advance = false;
                }
            } else if let crossterm::event::KeyCode::Char(ch) = key.code {
                if let Some(action) = editor.prompt.as_ref().unwrap().action_for(ch) {
                    resolve_prompt(action, editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs);
                }
            }
        }
        // Merge a directly-delivered background result even under a modal.
        Msg::JobDone(o) => crate::jobs_apply::apply_job_outcome(o, editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs),
        Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => {
            crate::jobs_apply::apply_filter_done(editor, buffer_id, version, range, cursor, disposition, outcome, ctx.clock);
        }
        Msg::ExportDone { target, result, overwrite_confirmed, .. } => {
            crate::jobs_apply::apply_export_done(editor, target, result, overwrite_confirmed, &**ctx.fs);
        }
        Msg::TransformDone { buffer_id, version, range, kind, result } => {
            crate::jobs_apply::apply_transform_done(editor, buffer_id, version, range, kind, result, ctx.clock);
        }
        Msg::DiagnosticsDone { buffer_id, version, source, diagnostics } => {
            crate::diagnostics_run::apply_diagnostics_done(editor, buffer_id, version, source, diagnostics);
        }
        // Effort A: a provider lifecycle event (Degraded/Restarted) must reach the status line even
        // under an open modal (e.g. harper crashes during a quit/save prompt). Second delivery site
        // beside reduce_dispatch's arm — the intercept's `_ => {}` would otherwise swallow it.
        Msg::DiagProviderEvent { source, event } =>
            crate::diag_provider::apply_provider_event(editor, source, event, ctx.clock),
        Msg::ClipboardPaste { buffer_id, text, .. } => crate::jobs_apply::apply_clipboard_paste(editor, buffer_id, text, ctx.clock),
        Msg::ClipboardAvailability(ok) => crate::jobs_apply::apply_clipboard_availability(editor, ok),
        // Resize/Tick/other input: ignored for the modal, but results still drain below.
        _ => {}
    }
    // Always drain ready results (merges the awaited save&quit result).
    crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs))
}

/// Execute the action chosen in a modal prompt, then clear the prompt.
/// Open the Save-As destination picker, seeded at the active document's directory.
pub fn open_save_as(editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> bool
{
    let dir = editor.active().document.path.as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    editor.open_destination_picker(fs, msg_tx,
        crate::file_browser::DestinationPurpose::SaveAs, dir, String::new())
}

/// H5 `clean_recovery` command entry: enumerate the provably-valueless recovery files ONCE,
/// snapshot them into `pending_clean`, and raise a count-confirm prompt. TOCTOU-safe — the
/// confirm deletes the snapshot, not a re-scan. An empty enumeration (or no state dir) sets a
/// status and raises NO prompt, so the user is never asked to confirm deleting nothing.
pub fn open_clean_recovery(editor: &mut crate::editor::Editor, fs: &dyn crate::fsx::Fs) {
    let files = match crate::swap::state_dir() {
        Ok(dir) => crate::swap::cleanable_recovery_files(fs, &dir, &crate::swap::open_swap_paths(editor)),
        Err(_) => Vec::new(),
    };
    raise_clean_recovery(editor, files);
}

/// Snapshot-and-raise core of `open_clean_recovery`, split out so the count-0 / count-N
/// branch is testable without depending on the shared real state dir. An empty snapshot sets
/// a status and raises NO prompt; a non-empty one is stored verbatim into `pending_clean`
/// (the TOCTOU-safe deletion unit) before the count-confirm modal opens.
fn raise_clean_recovery(editor: &mut crate::editor::Editor, files: Vec<std::path::PathBuf>) {
    if files.is_empty() {
        editor.set_status(crate::status::StatusKind::Info, "No recovery files to clean");
        return;
    }
    let n = files.len();
    editor.pending_clean = files;
    editor.open_prompt(crate::prompt::Prompt::clean_recovery(n));
}

/// Expand a user-typed path: `~/` prefix → home dir; relative → joined onto cwd.
/// Mirrors the `~` handling used by the dictionary/config path loaders.
pub fn expand_path(text: &str) -> std::path::PathBuf {
    let expanded = if let Some(rest) = text.strip_prefix("~/") {
        dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| std::path::PathBuf::from(text))
    } else { std::path::PathBuf::from(text) };
    if expanded.is_absolute() { expanded }
    else { std::env::current_dir().map(|d| d.join(&expanded)).unwrap_or(expanded) }
}

// `pub(crate)`, NOT private: `file_browser_commit::commit_destination` calls this across
// module boundaries. Routes through the SEAM (`save_atomic_with_fs`), not the `RealFs`
// wrapper — the same reasoning as `save.rs`'s worker-side save: an injected `FaultFs` must
// reach this write, not silently bypass it.
pub(crate) fn perform_block_write(editor: &mut crate::editor::Editor,
    target: &std::path::Path, start: usize, end: usize,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>)
{
    let text = editor.active().document.buffer.slice(start..end);
    match crate::file::save_atomic_with_fs(&**fs, target, &text) {
        Ok(_)  => editor.set_status(crate::status::StatusKind::Info, format!("wrote block to {}", target.display())),
        Err(e) => editor.set_status_full(crate::status::StatusKind::Error, e.to_string(),
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None),
    }
}

// `pub(crate)`, NOT private: `file_browser_commit::commit_destination` calls this across
// module boundaries. It was `fn` in the tree and must be widened here or the commit arm
// does not compile.
pub(crate) fn perform_save_as(editor: &mut crate::editor::Editor, chosen: std::path::PathBuf,
                   resolved: std::path::PathBuf,
                   executor: &dyn crate::jobs::Executor,
                   clock: &dyn wordcartel_core::history::Clock,
                   msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
                   // The CALLER's handle. Constructing `Arc::new(RealFs)` here would make
                   // Save-As — the most durability-critical user path in this effort —
                   // unreachable by an injected `FaultFs`, silently undoing the seam at the
                   // one place it matters most. Every caller already holds `ctx.fs`.
                   fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>) {
    let v = editor.active().document.version;
    let buffer_id = editor.active().id;
    {
        let mut ctx = crate::registry::Ctx {
            editor, clock, executor, msg_tx: msg_tx.clone(),
            fs: std::sync::Arc::clone(fs),
        };
        crate::save::do_save_to(&mut ctx,
            crate::save::SaveTarget { chosen, resolved }, crate::save::SaveMode::SaveAs);
    }
    if let Some(action) = editor.pending_save_as.take() {
        editor.pending_after_save = Some(crate::editor::PendingAfterSave { buffer_id, version: v, action, at_ms: clock.now_ms() });
    }
}

/// Request a new empty untitled buffer additively (never raises a dirty-guard modal).
pub fn request_new(
    editor: &mut Editor,
    _ex: &dyn Executor,
    _clock: &dyn Clock,
    _msg_tx: &std::sync::mpsc::Sender<Msg>,
) {
    crate::workspace::new_empty_buffer(editor);
}

#[allow(clippy::too_many_lines)] // prompt resolution — one arm per prompt kind
pub fn resolve_prompt(
    action: PromptAction,
    editor: &mut Editor,
    ex: &dyn Executor,
    clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
) {
    match action {
        PromptAction::Cancel => {
            editor.pending_export = None;
            editor.pending_save_overwrite = None;
            // PAIRED with `pending_save_overwrite` above — see the doc comment on the field.
            editor.pending_save_as_chosen = None;
            editor.pending_save_as = None;
            editor.pending_write_block = None;
            editor.pending_clean.clear(); // H5: Cancel abandons the snapshot; delete nothing
            // Effort 6: abort an in-progress multi-buffer quit (no data loss; the
            // user backed out). Leave `quit` false.
            editor.quit_drain = None;
            editor.quit_drain_advance = false;
        }
        PromptAction::QuitSaveAll | PromptAction::QuitReviewEach => {
            editor.prompt = None;
            let mode = if matches!(action, PromptAction::QuitSaveAll) { crate::editor::QuitMode::SaveAll } else { crate::editor::QuitMode::ReviewEach };
            let queue: std::collections::VecDeque<_> = editor.buffers.iter().filter(|b| editor.is_dirty(b.id)).map(|b| b.id).collect();
            editor.quit_drain = Some(crate::editor::QuitDrain { queue, mode });
            crate::jobs_apply::drive_quit_drain(editor, ex, clock, msg_tx, fs);
            return;
        }
        PromptAction::ReviewSave => {
            editor.prompt = None;
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone(), fs: std::sync::Arc::clone(fs) };
            crate::save::dispatch_save_then(&mut ctx, crate::editor::PostSaveAction::ContinueQuitDrain);
            return;
        }
        PromptAction::ReviewDiscard => {
            editor.prompt = None;
            if let Some(d) = editor.quit_drain.as_mut() { d.queue.pop_front(); }
            crate::jobs_apply::drive_quit_drain(editor, ex, clock, msg_tx, fs);
            return;
        }
        PromptAction::CloseSave { id } => {
            editor.prompt = None;
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone(), fs: std::sync::Arc::clone(fs) };
            crate::save::dispatch_save_then(&mut ctx, crate::editor::PostSaveAction::CloseBuffer { id });
            return;
        }
        PromptAction::CloseDiscard { id } => {
            editor.prompt = None;
            crate::workspace::close_buffer_now(editor, id);
            return;
        }
        PromptAction::QuitAnyway => { editor.quit = true; }
        PromptAction::SaveAndQuit => {
            editor.prompt = None; // dismiss the quit-confirm modal first
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone(), fs: std::sync::Arc::clone(fs) };
            crate::save::dispatch_save_and_quit(&mut ctx);
            return; // prompt handled; must NOT clear an external-mod modal
        }
        PromptAction::Reload => crate::save::reload_from_disk(editor),
        PromptAction::Overwrite => {
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone(), fs: std::sync::Arc::clone(fs) };
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
                // Delete AFTER load_recovered — `pending_swap_path` is the orphan-scratch
                // recovery carrier, and load_recovered replaces the whole Buffer.
                if let Some(p) = orphan { let _ = fs.remove_file(&p); }
            }
        }
        PromptAction::DiscardSwap => {
            if let Some(p) = editor.active_mut().pending_swap_path.take() {
                let _ = fs.remove_file(&p);
            } else {
                crate::swap::delete_with_fs(&**fs, editor.active().document.path.as_deref());
            }
        }
        PromptAction::OpenOriginal => {
            editor.active_mut().pending_swap_body = None;
            editor.active_mut().pending_swap_path = None;
        }
        PromptAction::OverwriteExport => {
            if let Some(pe) = editor.pending_export.take() {
                // User explicitly confirmed clobbering the existing target.
                crate::export::do_export(editor, &pe.ext, &pe.target, msg_tx, true,
                    std::sync::Arc::clone(fs));
            }
        }
        PromptAction::OverwriteSaveAs => {
            // Both paths were already resolved BEFORE this prompt was raised — by
            // `file_browser_commit::commit_destination`, which stores `chosen` alongside
            // `pending_save_overwrite`'s `resolved` for exactly this reconstruction. No
            // re-resolution here: re-running `resolve_write_destination` against a target
            // that no longer matches what the writer confirmed would be the wrong check.
            if let (Some(resolved), Some(chosen)) =
                (editor.pending_save_overwrite.take(), editor.pending_save_as_chosen.take())
            {
                perform_save_as(editor, chosen, resolved, ex, clock, msg_tx, fs);
            }
        }
        PromptAction::OverwriteWriteBlock => {
            if let Some(t) = editor.pending_write_block.take() {
                if let Some(b) = editor.active().marked_block {
                    perform_block_write(editor, &t, b.start, b.end, fs);
                } else {
                    editor.set_status(crate::status::StatusKind::Info, "no marked block");
                }
            }
        }
        PromptAction::Transform(kind) => {
            crate::transform::dispatch_transform(editor, kind, None, clock, msg_tx);
        }
        PromptAction::CleanRecovery => {
            // TOCTOU-safe in BOTH directions. Forward: the snapshot is the CEILING — we delete only
            // from `pending_clean` (`std::mem::take` also clears it), never a fresh re-scan, so a
            // file that appeared after the prompt opened can never be swept. Inverse: before deleting
            // each snapshot path we RE-RUN the enumerator's safety oracle (`recovery_path_still_
            // cleanable`), so a swap/temp whose CONTENT became recoverable while the modal was open is
            // SKIPPED — this can only ever delete a SUBSET of the snapshot (fail-closed). Best-effort
            // per file; a vanished/undeletable/no-longer-cleanable file is simply not counted, and the
            // status reports the ACTUAL number removed (may be < the confirmed count).
            let protected = crate::swap::open_swap_paths(editor);
            let mut n = 0usize;
            for p in std::mem::take(&mut editor.pending_clean) {
                if !crate::swap::recovery_path_still_cleanable(&**fs, &p, &protected) { continue; }
                if fs.remove_file(&p).is_ok() { n += 1; }
            }
            editor.set_status(crate::status::StatusKind::Info, format!("Cleaned {n} file(s)"));
        }
    }
    editor.prompt = None;
}

/// Submit a minibuffer line as a filter command.
///
/// Runs the line through `sh -c` (vi `!` / Emacs `shell-command-on-region` model),
/// so pipes/quoting/redirects work. An empty (or whitespace-only) line sets a status
/// message and returns without dispatching.
pub(crate) fn submit_filter_line(
    editor: &mut Editor,
    line: &str,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
) {
    let Some(spec) = build_filter_spec(line) else {
        editor.set_status_full(crate::status::StatusKind::Warning, "filter: no command given",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return;
    };
    crate::filter::dispatch_filter(editor, spec, msg_tx.clone());
}

/// Build the `FilterSpec` for an interactive filter line. The line is passed to the
/// shell VERBATIM as a single argv element (`run_subprocess` joins argv for `sh -c`),
/// so quoting/pipes/redirects survive — splitting+rejoining would collapse quoted
/// whitespace. Trust boundary: user-typed at an interactive prompt (vi `!`), distinct
/// from the untrusted `submit_transaction` path. Caps (timeout/max_output) + the
/// `dispatch_filter` panic isolation are kept unchanged.
fn build_filter_spec(line: &str) -> Option<crate::filter::FilterSpec> {
    if line.trim().is_empty() { return None; }
    Some(crate::filter::FilterSpec {
        argv: vec![line.to_string()],
        shell: true,
        disposition: crate::filter::Disposition::Filter,
        input: crate::filter::Input::SelectionElseBuffer,
        timeout: std::time::Duration::from_secs(10),
        max_output: crate::limits::MAX_FILTER_OUTPUT,
    })
}

/// Submit handler for Set Wrap Column (spec repar10 D2). Deliberate divergences from
/// the goto_line family: this command names its own noun and SURFACES the clamp — a
/// silently-moved formatting width is a surprise-diff class; a moved scroll target is
/// not. Parse failure leaves wrap_column unchanged; below-minimum is a SUCCESSFUL
/// clamped set. Any successful set rebuilds layout — wrap_column drives the centered
/// measure geometry, and a bare field write would leave stale layout until the next edit.
pub(crate) fn wrap_column_submit(editor: &mut crate::editor::Editor, text: &str) {
    let n: u16 = match text.trim().parse() {
        Ok(n) => n,
        Err(_) => {
            editor.set_status_full(crate::status::StatusKind::Warning, "wrap column: not a number".to_string(),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return;
        }
    };
    let (value, msg) = if n < 20 { (20, "wrap column: 20 (minimum)".to_string()) }
                       else if n > 9999 { (9999, "wrap column: 9999 (maximum)".to_string()) }
                       else { (n, format!("wrap column: {n}")) };
    editor.view_opts.wrap_column = value;
    editor.set_status(crate::status::StatusKind::Info, msg);
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}

/// Submit a minibuffer line as a go-to-line target (Effort 8). 1-based, clamped;
/// records a jump origin (jump-back), unfolds to the target, lands at column 1.
pub(crate) fn goto_line_submit(editor: &mut crate::editor::Editor, text: &str) {
    let n: usize = match text.trim().parse() {
        Ok(n) => n,
        Err(_) => {
            editor.set_status_full(crate::status::StatusKind::Warning, "not a line number".to_string(),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return;
        }
    };
    let total = crate::derive::total_logical_lines(&editor.active().document.buffer);
    let line_index = n.max(1).min(total) - 1;            // 1-based clamp → 0-based index
    let pre = crate::nav::head(editor);
    crate::marks::record_jump(editor.active_mut(), pre); // jump-back support
    let target = editor.active().document.buffer.line_to_byte(line_index);
    let caret = crate::registry::place_caret_visible(editor, target, crate::registry::CaretPlace::UnfoldTo);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(caret);
    editor.active_mut().desired_col = None;
    crate::derive::rebuild(editor);   // UnfoldTo can change fold state → relayout (mirrors registry.rs:409 / app.rs:680)
    crate::nav::ensure_visible(editor);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestClock;

    /// A17 T5 (F4 Warning table, brief's worked example): a prompt-input refusal is a
    /// recoverable Warning that holds the slot (Sticky) — the user must see and dismiss it,
    /// not lose it to the very next keystroke like an ordinary Info echo.
    #[test]
    fn wrap_column_not_a_number_is_a_sticky_warning() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (40, 6));
        wrap_column_submit(&mut e, "abc"); // non-numeric → the "wrap column: not a number" arm
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    /// A17 T5 (F4 Warning table, prompt-input refusals row): an empty Save-As path refusal
    /// is a Sticky Warning. Migrated (Task 21) from the retired `save_as_submit` to the
    /// picker path — an empty field yields `CommitOutcome::Nothing`, which
    /// `commit_destination` turns into the SAME message/kind/lifetime. Driven through the
    /// REAL intercept, not `commit_destination` directly (see the commit-arm's own
    /// end-to-end tests in `file_browser_commit.rs` for why).
    ///
    /// DELIBERATELY does NOT pump the async listing (unlike the audit applied elsewhere —
    /// see the parent-row-highlight task report). This is a SEPARATE, pre-existing property
    /// of Row 1, not the defect that audit fixed: Row 1 fires on ANY highlighted directory
    /// whenever the field is EMPTY, by design — `FileBrowser::highlight_navigated`'s gate is
    /// `navigated || trimmed.is_empty()`, and a bare Enter on an untouched highlight with
    /// nothing typed is treated as an ordinary browse gesture. Since `std::env::temp_dir()`
    /// is never filesystem root, its listing always carries a ".." row, so IF this test
    /// pumped that listing, Enter would descend into the parent directory instead of
    /// reaching `CommitOutcome::Nothing` — the "empty path" warning would never fire once a
    /// real listing has landed. Confirmed live (pump added, ran, status came back empty
    /// instead of "save-as: empty path"; reverted) — reported as a FINDING in the task
    /// report, not fixed here: whether Row 1 should ever cede to Row 2 on an untouched
    /// directory highlight with an empty field is a design question, not a mechanical one.
    #[test]
    fn save_as_empty_path_is_a_sticky_warning() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs = crate::test_support::test_fs();
        e.open_destination_picker(&fs, &tx, crate::file_browser::DestinationPurpose::SaveAs,
            std::env::temp_dir(), "   ".into());
        crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Enter);
        assert_eq!(e.status_text(), "save-as: empty path");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    /// A17 T5: an empty Write-Block path refusal is a Sticky Warning. Migrated (Task 21)
    /// from the retired `block_write_submit` — see the Save-As twin above, INCLUDING the
    /// same deliberate non-pump: confirmed to break identically if pumped (same finding).
    #[test]
    fn block_write_empty_path_is_a_sticky_warning() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 1, hidden: false });
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs = crate::test_support::test_fs();
        e.open_destination_picker(&fs, &tx, crate::file_browser::DestinationPurpose::WriteBlock,
            std::env::temp_dir(), "   ".into());
        crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Enter);
        assert_eq!(e.status_text(), "write block: empty path");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    /// A17 T5: an empty filter command refusal is a Sticky Warning.
    #[test]
    fn submit_filter_line_empty_is_a_sticky_warning() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        submit_filter_line(&mut e, "   ", &tx);
        assert_eq!(e.status_text(), "filter: no command given");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    #[test]
    fn submit_filter_line_uses_shell_single_argv() {
        // The filter line is passed to the shell VERBATIM as a single argv element —
        // NOT whitespace-split-then-rejoined, which would collapse quoted whitespace
        // (e.g. the double space inside `sed 's/a  b/c/'`).
        let spec = build_filter_spec("sed 's/a  b/c/'").expect("non-empty line builds a spec");
        assert!(spec.shell, "interactive filter must run through sh -c");
        assert_eq!(spec.argv, vec!["sed 's/a  b/c/'".to_string()]);
        assert!(matches!(spec.input, crate::filter::Input::SelectionElseBuffer));
        assert_eq!(spec.timeout, std::time::Duration::from_secs(10));
        assert_eq!(spec.max_output, crate::limits::MAX_FILTER_OUTPUT);
        assert!(matches!(spec.disposition, crate::filter::Disposition::Filter));
    }

    #[test]
    fn build_filter_spec_trimmed_empty_is_none() {
        assert!(build_filter_spec("   ").is_none());
    }

    /// Effort A: a provider lifecycle event delivered while a modal prompt is open must still reach
    /// the status line (the intercept's `_ => {}` used to swallow it). Confirms the added arm.
    #[test]
    fn intercept_delivers_diag_provider_event_under_a_modal() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::diag_provider::ProviderEvent;
        use crate::harper_ls::INSTALL_HINT;
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        e.open_prompt(crate::prompt::Prompt::close_confirm("f.md", e.active().id));
        assert!(e.prompt.is_some(), "precondition: a modal prompt is open");
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: &tx, fs: &crate::test_support::test_fs() };
        let handled = intercept(Msg::DiagProviderEvent { source: wordcartel_core::diagnostics::DiagSource::Harper,
            event: ProviderEvent::Degraded(INSTALL_HINT.into()) },
            &mut e, &ctx);
        assert!(matches!(handled, crate::app::Handled::Done(_)), "interceptor consumes and folds");
        assert_eq!(e.status_text(), INSTALL_HINT, "Degraded reached the status line despite the open modal");
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
        resolve_prompt(PromptAction::SaveAndQuit, &mut e, &ex, &clk, &tx, &crate::test_support::test_fs());
        assert!(e.pending_after_save.is_none(), "no job dispatched → do not arm pending_after_save");
        assert!(!e.quit);
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
        resolve_prompt(PromptAction::Recover, &mut e, &ex, &clk, &tx, &crate::test_support::test_fs());
        assert_eq!(e.active().document.buffer.to_string(), "recovered body\n",
            "recovered content loaded into the active buffer");
        assert!(!p.exists(), "orphan swap file must be deleted on Recover");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn wrap_column_submit_parses_clamps_and_rejects() {
        use crate::editor::Editor; // the prompts test module has only `use super::*` (Codex plan r1 F2)
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        let initial = e.view_opts.wrap_column;
        wrap_column_submit(&mut e, "xyz");                 // parse failure → UNCHANGED
        assert_eq!(e.view_opts.wrap_column, initial);
        assert_eq!(e.status_text(), "wrap column: not a number");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        wrap_column_submit(&mut e, "99999");               // u16 overflow → UNCHANGED
        assert_eq!(e.view_opts.wrap_column, initial);
        assert_eq!(e.status_text(), "wrap column: not a number");
        // A17 T5 (Q1): the refusal is a Sticky Warning — it holds the slot even across the
        // next (lower-severity Info) submit, so dismiss it before checking a success message.
        e.dismiss_status();
        wrap_column_submit(&mut e, "15");                  // below min → CLAMPED SET
        assert_eq!(e.view_opts.wrap_column, 20);
        assert_eq!(e.status_text(), "wrap column: 20 (minimum)");
        wrap_column_submit(&mut e, "55");                  // success
        assert_eq!(e.view_opts.wrap_column, 55);
        assert_eq!(e.status_text(), "wrap column: 55");
        wrap_column_submit(&mut e, "12000");               // above repar's ceiling → CLAMPED SET
        assert_eq!(e.view_opts.wrap_column, 9999);
        assert_eq!(e.status_text(), "wrap column: 9999 (maximum)");
        wrap_column_submit(&mut e, "65535");               // u16-max, still above ceiling
        assert_eq!(e.view_opts.wrap_column, 9999);
        assert_eq!(e.status_text(), "wrap column: 9999 (maximum)");
    }

    #[test]
    fn wrap_column_minibuffer_cancel_leaves_value_unchanged() {
        // Spec D4's "cancel" pin: Esc dismisses the minibuffer generically (app.rs's
        // Esc arm) and no submit ever runs — pin the observable at the editor level.
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        let initial = e.view_opts.wrap_column;
        e.open_minibuffer("Wrap column: ", crate::minibuffer::MinibufferKind::WrapColumn);
        assert!(e.minibuffer.is_some());
        e.minibuffer = None; // the Esc arm's effect — dismiss without submit
        assert_eq!(e.view_opts.wrap_column, initial, "cancel must not touch the value");
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
        assert!(e.active().folds.folded().contains(&h_byte), "precondition: # H is folded");
        // goto line 4 ("body two"), which is inside the folded body:
        goto_line_submit(&mut e, "4");
        assert_eq!(e.active().document.selection.primary().head, e.active().document.buffer.line_to_byte(3));
        // The section is no longer folded over the target (real fold-state query: the
        // heading anchor is gone from `folds.folded`, so line index 3 is visible again).
        assert!(!e.active().folds.folded().contains(&h_byte),
            "goto into a folded body must unfold the covering section to reveal the target");
    }

    #[test]
    fn goto_line_clamps_and_rejects_garbage() {
        let mut e = Editor::new_from_text("a\nb\nc\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        goto_line_submit(&mut e, "999");          // clamp-high → last line
        let total = crate::derive::total_logical_lines(&e.active().document.buffer);
        assert_eq!(e.active().document.selection.primary().head, e.active().document.buffer.line_to_byte(total - 1));
        goto_line_submit(&mut e, "0");            // clamp-low → line 1
        assert_eq!(e.active().document.selection.primary().head, 0);
        goto_line_submit(&mut e, "xyz");          // garbage → status, no move
        assert_eq!(e.active().document.selection.primary().head, 0);
        assert_eq!(e.status_text(), "not a line number");           // rejected input sets the status
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    /// Migrated (Task 21) from the retired `save_as_submit` to the picker path — the field
    /// carries an ABSOLUTE existing path, so `resolve_field` passes it through unchanged
    /// regardless of the picker's seeded directory.
    #[test]
    fn save_as_existing_target_raises_overwrite_prompt() {
        use crate::editor::Editor;
        let p = std::env::temp_dir().join(format!("wc-ow-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel();
        let fs = crate::test_support::test_fs();
        e.open_destination_picker(&fs, &tx, crate::file_browser::DestinationPurpose::SaveAs,
            std::env::temp_dir(), p.to_str().unwrap().to_string());
        // Pump the async listing to completion — the state real usage actually reaches. The
        // typed field is a non-empty ABSOLUTE path, so `FileBrowser::highlight_is_navigated()`
        // gates Row 1 off regardless of whatever `temp_dir()` happens to sort first (very
        // possibly a directory, in a shared system temp dir full of other tests' leftovers).
        crate::test_support::pump_listing(&mut e, &rx);
        crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Enter);
        assert!(e.prompt.is_some(), "existing target → confirm modal");
        assert_eq!(e.prompt.as_ref().unwrap().action_for('o'), Some(crate::prompt::PromptAction::OverwriteSaveAs));
        assert_ne!(crate::prompt::PromptAction::OverwriteSaveAs, crate::prompt::PromptAction::Overwrite);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn new_on_any_buffer_adds_empty_untitled() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("kept\n", None, (80, 24));
        let orig_id = e.active().id;
        let (ex, clk, tx) = (crate::jobs::InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        request_new(&mut e, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "\n", "additive new: active is empty untitled");
        assert!(e.active().document.path.is_none(), "new buffer has no path");
        assert!(e.prompt.is_none(), "additive new never raises a guard modal");
        assert!(e.buffers.iter().any(|b| b.id == orig_id), "original buffer still present");
    }

    #[test]
    fn new_on_dirty_buffer_is_additive_no_modal() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("draft\n", None, (80, 24));
        let orig_id = e.active().id;
        e.active_mut().document.version = 1; // dirty
        let (ex, clk, tx) = (crate::jobs::InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        request_new(&mut e, &ex, &clk, &tx);
        assert!(e.prompt.is_none(), "additive new: no dirty-guard modal even for dirty buffer");
        assert_eq!(e.active().document.buffer.to_string(), "\n", "new empty buffer is active");
        assert!(e.buffers.iter().any(|b| b.id == orig_id), "dirty buffer still present in the list");
    }

    #[test]
    fn new_additive_preserves_all_existing_buffers() {
        use crate::editor::Editor;
        // request_new is additive: calling it on a dirty buffer adds a new buffer
        // without destroying the original, which remains accessible by switching.
        let mut e = Editor::new_from_text("draft\n", None, (80, 24));
        let orig_id = e.active().id;
        e.active_mut().document.version = 1; // dirty
        let (ex, clk, tx) = (crate::jobs::InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        request_new(&mut e, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "\n", "new empty buffer active");
        assert!(e.pending_save_as.is_none(), "no pending_save_as: additive new sets none");
        assert!(e.prompt.is_none(), "no modal");
        // Switch back to the original to verify it's still there.
        let idx = e.buffers.iter().position(|b| b.id == orig_id).expect("original buffer must still exist");
        e.switch_to_index(idx);
        assert_eq!(e.active().document.buffer.to_string(), "draft\n", "original dirty buffer intact");
    }

    // `block_write_writes_block_text_only_doc_unchanged` (the successful-write scenario) is
    // now covered end-to-end, through the real Enter intercept, by
    // `file_browser_commit::tests::write_block_commits_end_to_end_from_enter` (which also
    // pins the "block stays after write" survival this test used to assert) — Task 21
    // retired the minibuffer submit path this test drove.

    /// A17 T4: a genuine write-block IO failure (target's parent is a regular FILE, so
    /// `save_atomic` fails ENOTDIR) must land Sticky/Error — surviving a later Info ack (Q1).
    /// Migrated (Task 21) from the retired `block_write_submit` to the picker path.
    #[test]
    fn block_write_failure_is_a_sticky_error_that_survives_a_later_info() {
        use crate::editor::Editor;
        let parent = std::env::temp_dir().join(format!("wc-blkw-fail-{}.md", std::process::id()));
        std::fs::write(&parent, "i am a file, not a dir\n").unwrap();
        let target = parent.join("out.txt"); // target "inside" a regular file → ENOTDIR
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false });
        let (tx, rx) = std::sync::mpsc::channel();
        let fs = crate::test_support::test_fs();
        e.open_destination_picker(&fs, &tx, crate::file_browser::DestinationPurpose::WriteBlock,
            std::env::temp_dir(), target.to_str().unwrap().to_string());
        // Pump the async listing to completion — the state real usage actually reaches.
        crate::test_support::pump_listing(&mut e, &rx);
        crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Enter);
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        e.set_status(crate::status::StatusKind::Info, "later ack");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error, "Q1: Info must not displace a held Error");
        let _ = std::fs::remove_file(&parent);
    }

    /// Migrated (Task 21) from the retired `block_write_submit` to the picker path.
    #[test]
    fn block_write_existing_target_raises_overwrite() {
        use crate::editor::Editor;
        let p = std::env::temp_dir().join(format!("wc-blkw-ow-{}.md", std::process::id()));
        std::fs::write(&p, "old").unwrap();
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 3, hidden: false });
        let (tx, rx) = std::sync::mpsc::channel();
        let fs = crate::test_support::test_fs();
        e.open_destination_picker(&fs, &tx, crate::file_browser::DestinationPurpose::WriteBlock,
            std::env::temp_dir(), p.to_str().unwrap().to_string());
        // Pump the async listing to completion — the state real usage actually reaches.
        crate::test_support::pump_listing(&mut e, &rx);
        crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Enter);
        assert_eq!(e.prompt.as_ref().unwrap().action_for('o'), Some(crate::prompt::PromptAction::OverwriteWriteBlock));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn close_save_arms_pending_after_save_with_close_variant() {
        // CloseSave for a named dirty buffer → pending_after_save armed with CloseBuffer{id}.
        use crate::editor::{Editor, PostSaveAction};
        use crate::jobs::InlineExecutor;
        let p = std::env::temp_dir().join(format!("wc-close-save-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.version = 1;
        e.active_mut().document.saved_version = None; // dirty
        let id = e.active().id;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        resolve_prompt(crate::prompt::PromptAction::CloseSave { id }, &mut e, &ex, &clk, &tx, &crate::test_support::test_fs());
        let pas = e.pending_after_save.as_ref().expect("pending_after_save must be armed");
        assert_eq!(pas.buffer_id, id);
        assert_eq!(pas.version, 1);
        assert!(matches!(pas.action, PostSaveAction::CloseBuffer { id: i } if i == id),
            "action must be CloseBuffer{{id}}");
        assert!(e.prompt.is_none(), "prompt dismissed");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn close_discard_closes_immediately_and_leaves_swap() {
        // Decision 1 pin: Discard closes the buffer immediately, leaving the swap file intact.
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        let p = std::env::temp_dir().join(format!("wc-close-discard-{}.md", std::process::id()));
        std::fs::write(&p, "on disk\n").unwrap();
        let sp = crate::swap::swap_path(Some(p.as_path())).expect("swap path ok");
        crate::swap::write_atomic(&sp, "stub swap content").expect("write stub swap");
        assert!(sp.exists(), "precondition: swap file exists");
        let mut e = Editor::new_from_text("draft\n", Some(p.clone()), (80, 24));
        e.active_mut().document.version = 1;
        e.active_mut().document.saved_version = None; // dirty
        let id = e.active().id;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        resolve_prompt(crate::prompt::PromptAction::CloseDiscard { id }, &mut e, &ex, &clk, &tx, &crate::test_support::test_fs());
        // Buffer is gone (last ordinary → slot replaced with fresh untitled, old id absent)
        assert!(e.by_id(id).is_none(), "discarded buffer is gone");
        // Swap file must NOT be deleted by Discard (decision 1)
        assert!(sp.exists(), "swap file survives Discard");
        let _ = std::fs::remove_file(&sp);
        let _ = std::fs::remove_file(&p);
    }

    // ── H5: clean-recovery flow (SAFETY-CRITICAL — no data loss / TOCTOU) ──────────────

    #[test]
    fn raise_clean_recovery_count_zero_sets_status_and_raises_no_prompt() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        raise_clean_recovery(&mut e, Vec::new());
        assert!(e.prompt.is_none(), "count 0 raises NO prompt");
        assert!(e.pending_clean.is_empty());
        assert_eq!(e.status_text(), "No recovery files to clean");
    }

    #[test]
    fn raise_clean_recovery_count_n_snapshots_and_opens_prompt() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let files = vec![std::path::PathBuf::from("/a.swp"), std::path::PathBuf::from("/b.md")];
        raise_clean_recovery(&mut e, files.clone());
        assert_eq!(e.pending_clean, files, "the exact snapshot is stored for TOCTOU-safe deletion");
        let p = e.prompt.as_ref().expect("count>0 opens a confirm prompt");
        assert!(p.message.contains('2'), "message bears the count");
        assert_eq!(p.action_for('y'), Some(crate::prompt::PromptAction::CleanRecovery));
        assert_eq!(p.action_for('c'), Some(crate::prompt::PromptAction::Cancel));
    }

    #[test]
    fn clean_recovery_confirm_deletes_exactly_the_snapshot_even_if_a_new_file_appears() {
        // TOCTOU: the confirm deletes the SNAPSHOT, never a re-scan. A file that materializes
        // in the state dir after the prompt opened must survive. (Snapshot entries are `recovered-
        // *.md` dumps — unconditionally cleanable — so they pass the confirm-time re-verify and the
        // test isolates the FORWARD-TOCTOU invariant from the inverse-TOCTOU content check.)
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        let a = std::env::temp_dir().join(format!("recovered-wc-h5-snap-a-{}.md", std::process::id()));
        let b = std::env::temp_dir().join(format!("recovered-wc-h5-snap-b-{}.md", std::process::id()));
        let latecomer = std::env::temp_dir().join(format!("recovered-wc-h5-snap-late-{}.md", std::process::id()));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.pending_clean = vec![a.clone(), b.clone()]; // snapshot taken BEFORE the latecomer exists
        e.open_prompt(crate::prompt::Prompt::clean_recovery(2));
        // A new file appears after the prompt was raised.
        std::fs::write(&latecomer, "late").unwrap();
        let (ex, clk, tx) = (InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        resolve_prompt(crate::prompt::PromptAction::CleanRecovery, &mut e, &ex, &clk, &tx, &crate::test_support::test_fs());
        assert!(!a.exists() && !b.exists(), "exactly the snapshot is deleted");
        assert!(latecomer.exists(), "a file appearing after the snapshot is NEVER swept (TOCTOU-safe)");
        assert!(e.pending_clean.is_empty(), "snapshot consumed");
        assert!(e.prompt.is_none(), "prompt dismissed");
        assert_eq!(e.status_text(), "Cleaned 2 file(s)");
        let _ = std::fs::remove_file(&latecomer);
    }

    /// INVERSE-TOCTOU (the H5 hardening): a swap enumerated as valueless (DiscardSilently) at scan
    /// time but whose CONTENT is rewritten to a recoverable (Prompt) state while the confirm modal
    /// is open must NOT be deleted at confirm — the handler re-runs the safety oracle per snapshot
    /// path and skips any that no longer passes, while still deleting the paths that remain
    /// cleanable. The snapshot stays the ceiling; only a SUBSET is ever removed.
    #[test]
    fn clean_recovery_confirm_skips_a_path_that_became_recoverable_while_prompt_open() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::swap::{fnv1a64, serialize, swap_path, write_atomic, SwapHeader};
        const DEAD_PID: u32 = 999_999; // /proc/999999 does not exist
        #[cfg(target_os = "linux")]
        assert!(!crate::swap::pid_is_live(DEAD_PID), "test invariant: pid 999999 must not be live");

        // Build a doc on disk + its CANONICAL swap whose header hash = fnv1a64(swap_body); a dead
        // pid and swap_body == on-disk bytes make `assess` return DiscardSilently (cleanable).
        let mk = |tag: &str, saved: &str, swap_body: &str| -> (std::path::PathBuf, std::path::PathBuf) {
            let doc = std::env::temp_dir()
                .join(format!("wc-h5-inv-{}-{}-{}.txt", std::process::id(), tag, TestClock(0).0));
            std::fs::write(&doc, saved).unwrap();
            let real = std::fs::canonicalize(&doc).unwrap();
            let h = SwapHeader {
                realpath: Some(real.to_string_lossy().into_owned()),
                load_mtime_secs: None, load_size: None,
                content_hash: fnv1a64(swap_body.as_bytes()), version: 1, ts_ms: 1, pid: DEAD_PID,
                ..Default::default()
            };
            let sp = swap_path(Some(&doc)).unwrap();
            write_atomic(&sp, &serialize(&h, swap_body)).unwrap();
            (doc, sp)
        };

        // (1) stays valueless through confirm → must be deleted.
        let (doc_ok, sp_ok) = mk("ok", "same\n", "same\n");
        // (2) valueless at scan (snapshot), but a second session rewrites its swap with recoverable
        //     unsaved work before confirm → must be SKIPPED.
        let (doc_bad, sp_bad) = mk("bad", "same\n", "same\n");

        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.pending_clean = vec![sp_ok.clone(), sp_bad.clone()]; // both cleanable at snapshot time
        e.open_prompt(crate::prompt::Prompt::clean_recovery(2));

        // Content race: rewrite sp_bad so its header now DIVERGES from the on-disk doc (assess → Prompt).
        let h_div = SwapHeader {
            realpath: Some(std::fs::canonicalize(&doc_bad).unwrap().to_string_lossy().into_owned()),
            load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(b"UNSAVED EDIT\n"), version: 2, ts_ms: 2, pid: DEAD_PID,
            ..Default::default()
        };
        write_atomic(&sp_bad, &serialize(&h_div, "UNSAVED EDIT\n")).unwrap();

        let (ex, clk, tx) = (InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        resolve_prompt(crate::prompt::PromptAction::CleanRecovery, &mut e, &ex, &clk, &tx, &crate::test_support::test_fs());

        assert!(!sp_ok.exists(), "the still-valueless snapshot swap IS deleted");
        assert!(sp_bad.exists(),
            "a swap that became recoverable while the prompt was open is NEVER deleted — no data loss");
        assert_eq!(e.status_text(), "Cleaned 1 file(s)", "status reports the ACTUAL deleted count (a subset)");
        assert!(e.pending_clean.is_empty(), "snapshot consumed");
        assert!(e.prompt.is_none(), "prompt dismissed");
        for f in [&sp_ok, &sp_bad, &doc_ok, &doc_bad] { let _ = std::fs::remove_file(f); }
    }

    #[test]
    fn clean_recovery_cancel_deletes_nothing_and_clears_snapshot() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        let a = std::env::temp_dir().join(format!("wc-h5-cancel-{}.swp", std::process::id()));
        std::fs::write(&a, "keep me").unwrap();
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.pending_clean = vec![a.clone()];
        e.open_prompt(crate::prompt::Prompt::clean_recovery(1));
        let (ex, clk, tx) = (InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        resolve_prompt(crate::prompt::PromptAction::Cancel, &mut e, &ex, &clk, &tx, &crate::test_support::test_fs());
        assert!(a.exists(), "Cancel deletes nothing");
        assert!(e.pending_clean.is_empty(), "Cancel clears the snapshot");
        assert!(e.prompt.is_none());
        let _ = std::fs::remove_file(&a);
    }

    #[test]
    fn clean_recovery_esc_deletes_nothing_and_clears_snapshot() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let a = std::env::temp_dir().join(format!("wc-h5-esc-{}.swp", std::process::id()));
        std::fs::write(&a, "keep me").unwrap();
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.pending_clean = vec![a.clone()];
        e.open_prompt(crate::prompt::Prompt::clean_recovery(1));
        let (ex, clk, tx) = (InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: &tx, fs: &crate::test_support::test_fs() };
        let esc = Event::Key(KeyEvent {
            code: KeyCode::Esc, modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        intercept(Msg::Input(esc), &mut e, &ctx);
        assert!(a.exists(), "Esc deletes nothing");
        assert!(e.pending_clean.is_empty(), "Esc clears the snapshot");
        assert!(e.prompt.is_none(), "Esc dismisses the prompt");
        let _ = std::fs::remove_file(&a);
    }

    #[test]
    fn close_save_on_unnamed_buffer_opens_save_as_with_carry() {
        // Unnamed dirty buffer: CloseSave opens the Save-As DESTINATION PICKER (Task 21 —
        // no longer a minibuffer) and carries CloseBuffer into pending_save_as.
        use crate::editor::{Editor, PostSaveAction};
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("draft\n", None, (80, 24));
        e.active_mut().document.version = 1; // dirty
        let id = e.active().id;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        resolve_prompt(crate::prompt::PromptAction::CloseSave { id }, &mut e, &ex, &clk, &tx, &crate::test_support::test_fs());
        assert!(e.file_browser.as_ref().is_some_and(|fb| fb.mode.is_destination()),
            "Save-As destination picker must open for unnamed buffer");
        assert!(matches!(e.pending_save_as, Some(PostSaveAction::CloseBuffer { .. })),
            "pending_save_as must carry CloseBuffer");
    }
}
