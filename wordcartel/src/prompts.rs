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
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled {
    if editor.prompt.is_none() { return crate::app::Handled::Pass(msg); }
    match msg {
        Msg::Input(Event::Key(key)) if key.kind == crossterm::event::KeyEventKind::Press => {
            if key.code == crossterm::event::KeyCode::Esc {
                editor.prompt = None; // Esc cancels any prompt
                editor.pending_export = None;
                editor.pending_save_overwrite = None;
                editor.pending_save_as = None;
                editor.pending_write_block = None;
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
                    resolve_prompt(action, editor, ex, clock, msg_tx);
                }
            }
        }
        // Merge a directly-delivered background result even under a modal.
        Msg::JobDone(o) => crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx),
        Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => {
            crate::jobs_apply::apply_filter_done(editor, buffer_id, version, range, cursor, disposition, outcome, clock);
        }
        Msg::ExportDone { target, result, overwrite_confirmed, .. } => {
            crate::jobs_apply::apply_export_done(editor, target, result, overwrite_confirmed);
        }
        Msg::TransformDone { buffer_id, version, range, kind, result } => {
            crate::jobs_apply::apply_transform_done(editor, buffer_id, version, range, kind, result, clock);
        }
        Msg::DiagnosticsDone { buffer_id, version, diagnostics } => {
            crate::diagnostics_run::apply_diagnostics_done(editor, buffer_id, version, diagnostics);
        }
        Msg::ClipboardPaste { buffer_id, text, .. } => crate::jobs_apply::apply_clipboard_paste(editor, buffer_id, text, clock),
        Msg::ClipboardAvailability(ok) => crate::jobs_apply::apply_clipboard_availability(editor, ok),
        // Resize/Tick/other input: ignored for the modal, but results still drain below.
        _ => {}
    }
    // Always drain ready results (merges the awaited save&quit result).
    crate::app::Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx))
}

/// Execute the action chosen in a modal prompt, then clear the prompt.
/// Open the Save-As minibuffer pre-filled with the active doc's directory.
pub fn open_save_as(editor: &mut crate::editor::Editor) {
    let pre = editor.active().document.path.as_ref()
        .and_then(|p| p.parent()).map(|d| format!("{}/", d.display())).unwrap_or_default();
    editor.open_minibuffer("Save as: ", crate::minibuffer::MinibufferKind::SaveAs);
    if let Some(mb) = editor.minibuffer.as_mut() { mb.cursor = pre.len(); mb.text = pre; }
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

/// Submit the Save-As minibuffer line: expand the path, raise an overwrite
/// confirmation if the target exists, else perform the save-as immediately.
pub fn save_as_submit(editor: &mut crate::editor::Editor, text: &str,
                      executor: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
                      msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
    let t = text.trim();
    if t.is_empty() {
        editor.status = "save-as: empty path".into();
        editor.pending_save_as = None;
        // Effort 6 (Codex C2): backing out of a drain's Save-As aborts the quit.
        editor.quit_drain = None;
        editor.quit_drain_advance = false;
        return;
    }
    let target = expand_path(t);
    if target.exists() {
        editor.pending_save_overwrite = Some(target.clone());
        editor.open_prompt(crate::prompt::Prompt::save_overwrite(&target));
        return;
    }
    perform_save_as(editor, target, executor, clock, msg_tx);
}

/// Submit the Write-Block minibuffer line: expand the path, raise an overwrite
/// confirmation if the target exists, else write the block text immediately.
/// Synchronous: uses `file::save_atomic` directly; does NOT touch document state.
pub fn block_write_submit(editor: &mut crate::editor::Editor, text: &str) {
    let Some(b) = editor.active().marked_block else { editor.status = "no marked block".into(); return; };
    let t = text.trim();
    if t.is_empty() { editor.status = "write block: empty path".into(); return; }
    let target = expand_path(t);
    if target.exists() {
        editor.pending_write_block = Some(target.clone());
        editor.open_prompt(crate::prompt::Prompt::write_block_overwrite(&target));
        return;
    }
    perform_block_write(editor, &target, b.start, b.end);
}

fn perform_block_write(editor: &mut crate::editor::Editor, target: &std::path::Path, start: usize, end: usize) {
    let text = editor.active().document.buffer.slice(start..end);
    match crate::file::save_atomic(target, &text) {
        Ok(_)  => editor.status = format!("wrote block to {}", target.display()),
        Err(e) => editor.status = e.to_string(),
    }
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

/// Request a new empty untitled buffer additively (never raises a dirty-guard modal).
pub fn request_new(
    editor: &mut Editor,
    _ex: &dyn Executor,
    _clock: &dyn Clock,
    _msg_tx: &std::sync::mpsc::Sender<Msg>,
) {
    crate::workspace::new_empty_buffer(editor);
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
            editor.pending_write_block = None;
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
            crate::jobs_apply::drive_quit_drain(editor, ex, clock, msg_tx);
            return;
        }
        PromptAction::ReviewSave => {
            editor.prompt = None;
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
            crate::save::dispatch_save_then(&mut ctx, crate::editor::PostSaveAction::ContinueQuitDrain);
            return;
        }
        PromptAction::ReviewDiscard => {
            editor.prompt = None;
            if let Some(d) = editor.quit_drain.as_mut() { d.queue.pop_front(); }
            crate::jobs_apply::drive_quit_drain(editor, ex, clock, msg_tx);
            return;
        }
        PromptAction::CloseSave { id } => {
            editor.prompt = None;
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
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
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
            crate::save::dispatch_save_and_quit(&mut ctx);
            return; // prompt handled; must NOT clear an external-mod modal
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
        PromptAction::OverwriteWriteBlock => {
            if let Some(t) = editor.pending_write_block.take() {
                if let Some(b) = editor.active().marked_block {
                    perform_block_write(editor, &t, b.start, b.end);
                } else {
                    editor.status = "no marked block".into();
                }
            }
        }
        PromptAction::Transform(kind) => {
            crate::transform::dispatch_transform(editor, kind, None, clock, msg_tx);
        }
    }
    editor.prompt = None;
}

/// Submit a minibuffer line as a filter command.
///
/// Splits the line on whitespace to build the argv (no shell, no quoting —
/// `shell: false` is the security default; shell invocation is opt-in only).
/// An empty line sets a status message and returns without dispatching.
pub(crate) fn submit_filter_line(
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
        max_output: crate::limits::MAX_FILTER_OUTPUT,
    };
    crate::filter::dispatch_filter(editor, spec, msg_tx.clone());
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
        Err(_) => { editor.status = "wrap column: not a number".to_string(); return; }
    };
    let (value, msg) = if n < 20 { (20, "wrap column: 20 (minimum)".to_string()) }
                       else if n > 9999 { (9999, "wrap column: 9999 (maximum)".to_string()) }
                       else { (n, format!("wrap column: {n}")) };
    editor.view_opts.wrap_column = value;
    editor.status = msg;
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestClock;

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
        resolve_prompt(PromptAction::SaveAndQuit, &mut e, &ex, &clk, &tx);
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
        resolve_prompt(PromptAction::Recover, &mut e, &ex, &clk, &tx);
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
        assert_eq!(e.status, "wrap column: not a number");
        wrap_column_submit(&mut e, "99999");               // u16 overflow → UNCHANGED
        assert_eq!(e.view_opts.wrap_column, initial);
        assert_eq!(e.status, "wrap column: not a number");
        wrap_column_submit(&mut e, "15");                  // below min → CLAMPED SET
        assert_eq!(e.view_opts.wrap_column, 20);
        assert_eq!(e.status, "wrap column: 20 (minimum)");
        wrap_column_submit(&mut e, "55");                  // success
        assert_eq!(e.view_opts.wrap_column, 55);
        assert_eq!(e.status, "wrap column: 55");
        wrap_column_submit(&mut e, "12000");               // above repar's ceiling → CLAMPED SET
        assert_eq!(e.view_opts.wrap_column, 9999);
        assert_eq!(e.status, "wrap column: 9999 (maximum)");
        wrap_column_submit(&mut e, "65535");               // u16-max, still above ceiling
        assert_eq!(e.view_opts.wrap_column, 9999);
        assert_eq!(e.status, "wrap column: 9999 (maximum)");
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
        assert_eq!(e.status, "not a line number");           // rejected input sets the status
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
        save_as_submit(&mut e, p.to_str().unwrap(), &ex, &clk, &tx);
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

    #[test]
    fn block_write_writes_block_text_only_doc_unchanged() {
        use crate::editor::Editor;
        let p = std::env::temp_dir().join(format!("wc-blkw-{}.md", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false }); // "hello"
        let before_doc = e.active().document.buffer.to_string();
        block_write_submit(&mut e, p.to_str().unwrap());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello", "block text written");
        assert_eq!(e.active().document.buffer.to_string(), before_doc, "document unchanged");
        assert!(e.active().marked_block.is_some(), "block stays after write");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn block_write_existing_target_raises_overwrite() {
        use crate::editor::Editor;
        let p = std::env::temp_dir().join(format!("wc-blkw-ow-{}.md", std::process::id()));
        std::fs::write(&p, "old").unwrap();
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 3, hidden: false });
        block_write_submit(&mut e, p.to_str().unwrap());
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
        resolve_prompt(crate::prompt::PromptAction::CloseSave { id }, &mut e, &ex, &clk, &tx);
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
        resolve_prompt(crate::prompt::PromptAction::CloseDiscard { id }, &mut e, &ex, &clk, &tx);
        // Buffer is gone (last ordinary → slot replaced with fresh untitled, old id absent)
        assert!(e.by_id(id).is_none(), "discarded buffer is gone");
        // Swap file must NOT be deleted by Discard (decision 1)
        assert!(sp.exists(), "swap file survives Discard");
        let _ = std::fs::remove_file(&sp);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn close_save_on_unnamed_buffer_opens_save_as_with_carry() {
        // Unnamed dirty buffer: CloseSave opens the Save-As minibuffer and carries
        // CloseBuffer into pending_save_as.
        use crate::editor::{Editor, PostSaveAction};
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("draft\n", None, (80, 24));
        e.active_mut().document.version = 1; // dirty
        let id = e.active().id;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        resolve_prompt(crate::prompt::PromptAction::CloseSave { id }, &mut e, &ex, &clk, &tx);
        assert_eq!(e.minibuffer.as_ref().map(|m| m.kind),
            Some(crate::minibuffer::MinibufferKind::SaveAs),
            "Save-As minibuffer must open for unnamed buffer");
        assert!(matches!(e.pending_save_as, Some(PostSaveAction::CloseBuffer { .. })),
            "pending_save_as must carry CloseBuffer");
    }
}
