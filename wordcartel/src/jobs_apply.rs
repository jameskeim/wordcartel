//! Job/message application: merging finished job results and outcomes into the editor,
//! quit-drain advancement, and the async filter/transform/export/clipboard completion
//! handlers. Extracted verbatim from app.rs (Effort H1).

use crate::{editor::Editor, file};
use crate::jobs::{is_stale, Executor, JobResult};
use crate::registry::Ctx;
use crate::app::Msg;
use wordcartel_core::history::Clock;

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
    // Post-save dispatch: fire the pending action when its awaited save lands.
    // Only Quit remains — Open/New are now additive (no save-then-act needed).
    if kind == crate::jobs::JobKind::Save {
        let fire = editor.pending_after_save.as_ref()
            .map(|p| p.buffer_id == buffer_id && p.version == version)
            .unwrap_or(false);
        if fire {
            let action = editor.pending_after_save.as_ref().unwrap().action.clone();
            let saved_this = editor.by_id(buffer_id).map(|b| b.document.saved_version) == Some(Some(version));
            match action {
                crate::editor::PostSaveAction::Quit => {
                    if saved_this {
                        editor.pending_after_save = None;
                        if !editor.is_dirty(buffer_id) {
                            editor.quit = true;
                        } else {
                            // Saved, but the user typed during the in-flight save → buffer
                            // dirty again. Do NOT quit (would lose those edits). User
                            // re-issues quit when ready.
                            editor.status = "edited during save — quit cancelled".into();
                        }
                    }
                }
                crate::editor::PostSaveAction::ContinueQuitDrain => {
                    editor.pending_after_save = None;
                    if saved_this && !editor.is_dirty(buffer_id) {
                        // Saved clean: drop this buffer from the queue and advance.
                        if let Some(d) = editor.quit_drain.as_mut() { d.queue.pop_front(); }
                        editor.quit_drain_advance = true; // apply_job_result re-drives with ctx
                    } else if saved_this {
                        // Codex C-new: the user typed DURING the in-flight save → this buffer
                        // is dirty again. Do NOT pop/skip it (that would silently lose the new
                        // edits). Re-drive WITHOUT popping → drive_quit_drain re-saves the newer
                        // version of the same front buffer. Converges once the user stops typing.
                        editor.quit_drain_advance = true;
                    } else {
                        // Save failed (Codex 8d): abort the drain so it can't linger with no
                        // in-flight save and no re-drive. The merge's error status stands.
                        editor.quit_drain = None;
                        editor.quit_drain_advance = false;
                    }
                }
            }
        }
    }
}

/// Apply a finished job's result, then advance a multi-buffer quit drain if one
/// is waiting on this completion. The single funnel for all JobDone handling so
/// the re-drive cannot be skipped on an early-returning reduce branch (Codex C1).
pub fn apply_job_result(r: JobResult, editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock, msg_tx: &std::sync::mpsc::Sender<Msg>) {
    apply_result(r, editor);
    if editor.quit_drain_advance {
        editor.quit_drain_advance = false;
        drive_quit_drain(editor, ex, clock, msg_tx);
    }
}

/// Apply a job outcome: a normal Done routes to the existing apply_result; a Panicked outcome
/// runs that kind's explicit failure cleanup (a panic is a failed completion).
pub fn apply_outcome(outcome: crate::jobs::JobOutcome, editor: &mut Editor) {
    match outcome {
        crate::jobs::JobOutcome::Done(r) => apply_result(r, editor),
        crate::jobs::JobOutcome::Panicked { buffer_id, version, kind, msg } =>
            apply_panic(buffer_id, version, kind, &msg, editor),
    }
}

fn apply_panic(buffer_id: crate::editor::BufferId, version: u64, kind: crate::jobs::JobKind, msg: &str, editor: &mut Editor) {
    use crate::jobs::JobKind;
    match kind {
        JobKind::Save => {
            // The merge never ran, so saved_version is untouched (buffer stays dirty). A panicked
            // save must NOT quit/strand: clear any awaited-quit state explicitly (the failed-save
            // Quit path leaves pending_after_save armed — we must not).
            let awaited = editor.pending_after_save.as_ref()
                .map(|p| p.buffer_id == buffer_id && p.version == version).unwrap_or(false);
            if awaited {
                editor.pending_after_save = None;
                editor.quit_drain = None;
                editor.quit_drain_advance = false;
            }
            editor.status = format!("save failed (internal error: {msg})");
        }
        JobKind::SwapWrite => {
            if let Some(b) = editor.by_id_mut(buffer_id) { b.swap_in_flight = false; }
            editor.status = format!("swap failed (internal error: {msg})");
        }
        JobKind::Reparse => {
            // A panicked reconcile (upstream pulldown-cmark residual) is deterministic
            // for this text — clear maybe_stale so we do NOT re-arm and retry every
            // debounce interval. The next edit re-sets maybe_stale via derive::rebuild,
            // giving exactly one reconcile attempt per edit.
            if let Some(b) = editor.by_id_mut(buffer_id) {
                b.reconcile.in_flight_version = None;
                b.reconcile.maybe_stale = false;
            }
        }
        #[cfg(test)]
        JobKind::CoalesceProbe => { editor.status = format!("job failed (internal error: {msg})"); }
    }
}

/// Apply a finished job outcome, then advance a multi-buffer quit drain if one
/// is waiting on this completion. The single funnel for all JobDone handling so
/// the re-drive cannot be skipped on an early-returning reduce branch (Codex C1).
pub fn apply_job_outcome(outcome: crate::jobs::JobOutcome, editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock, msg_tx: &std::sync::mpsc::Sender<Msg>) {
    apply_outcome(outcome, editor);
    if editor.quit_drain_advance {
        editor.quit_drain_advance = false;
        drive_quit_drain(editor, ex, clock, msg_tx);
    }
}

/// Advance the quit drain by one step: pick the next dirty buffer, switch to it,
/// and either dispatch its save (SaveAll) or raise the per-buffer review prompt
/// (ReviewEach). When the queue is empty, quit. Re-driven by save completion
/// (apply_result sets `quit_drain_advance`) and by review-prompt resolution.
pub fn drive_quit_drain(editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock, msg_tx: &std::sync::mpsc::Sender<Msg>) {
    loop {
        if editor.quit_drain.is_none() { return; }
        // Pop already-clean / vanished buffers off the front. Each iteration uses a
        // SHORT immutable borrow to read the front id (Codex I-new-1: never hold a
        // `quit_drain` borrow across an `editor.is_dirty`/`switch_to`/method call),
        // then mutates the queue in a SEPARATE borrow.
        let front = editor.quit_drain.as_ref().and_then(|d| d.queue.front().copied());
        let Some(id) = front else {
            editor.quit_drain = None;
            editor.quit = true;
            return;
        };
        let gone = editor.buffers.iter().all(|b| b.id != id);
        if gone || !editor.is_dirty(id) {
            if let Some(d) = editor.quit_drain.as_mut() { d.queue.pop_front(); }
            continue;
        }
        let idx = editor.buffers.iter().position(|b| b.id == id).unwrap();
        crate::workspace::switch_to(editor, idx); // show the buffer in question
        let mode = editor.quit_drain.as_ref().unwrap().mode;
        match mode {
            crate::editor::QuitMode::SaveAll => {
                let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
                crate::save::dispatch_save_then(&mut ctx, crate::editor::PostSaveAction::ContinueQuitDrain);
                return; // wait for the save (named) or Save-As (unnamed) to complete
            }
            crate::editor::QuitMode::ReviewEach => {
                let name = crate::workspace::buffer_display_name(editor, id);
                editor.open_prompt(crate::prompt::Prompt::quit_review_buffer(&name));
                return; // wait for ReviewSave/ReviewDiscard/Cancel
            }
        }
    }
}

// 8 args mirror Msg::FilterDone's fields 1:1; an args-struct would just duplicate
// that variant. Called from only the two FilterDone match arms.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_filter_done(
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

pub(crate) fn apply_transform_done(
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

pub(crate) fn apply_export_done(
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

pub(crate) fn insert_paste_text(editor: &mut Editor, buffer_id: crate::editor::BufferId, text: &str, clock: &dyn Clock) -> bool {
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

pub(crate) fn apply_clipboard_paste(editor: &mut Editor, buffer_id: crate::editor::BufferId, text: Option<String>, clock: &dyn Clock) {
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

pub(crate) fn apply_clipboard_availability(editor: &mut Editor, ok: bool) {
    if !ok && !editor.clipboard_notice_shown {
        editor.status = "system clipboard unavailable — copy/paste work in-editor; using OSC 52 for terminal sync".into();
        editor.clipboard_notice_shown = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestClock;

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
        for o in ex.drain() { apply_outcome(o, &mut e); }
        assert!(e.quit, "matching save result triggers quit");
        let _ = std::fs::remove_file(&p);
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
        for o in ex.drain() { apply_outcome(o, &mut e); }
        assert!(e.quit, "matching save result triggers quit");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn quit_after_save_cancelled_when_edited_during_flight() {
        // Regression (Codex gate Finding 1): if the user types DURING a single-buffer
        // save-and-quit's in-flight save, the save result fires (saved_this=true) but
        // the buffer is dirty again. The app must NOT quit and must not lose those edits.
        use crate::editor::{Editor, PostSaveAction};
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let p = std::env::temp_dir().join(format!("wc-sqflight-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        let id = e.active().id;
        // Arm pending_after_save{Quit} at version=1 (what dispatch_save_and_quit would set).
        e.active_mut().document.version = 1;
        e.active_mut().document.saved_version = None; // dirty
        e.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id: id,
            version: 1,
            action: PostSaveAction::Quit,
            at_ms: 0,
        });
        // Simulate typing during the in-flight save: version advances to 2, buffer dirty.
        e.active_mut().document.version = 2;
        // Deliver the in-flight save result for version=1. The merge sets
        // saved_version=Some(1), but document.version==2 → still dirty.
        let save_result = JobResult {
            buffer_id: id,
            class: ResultClass::Durability,
            version: 1,
            kind: JobKind::Save,
            merge: Box::new(move |editor: &mut Editor| {
                if let Some(b) = editor.by_id_mut(id) {
                    b.document.saved_version = Some(1);
                }
            }),
        };
        apply_result(save_result, &mut e);
        assert!(!e.quit, "must NOT quit — buffer is dirty again from edits typed during the save");
        assert!(e.active().document.dirty(), "buffer still holds the newer edits");
        assert!(e.pending_after_save.is_none(), "pending_after_save consumed on save match");
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
        apply_result(JobResult { buffer_id: id, class: ResultClass::Durability, version: 3, kind: JobKind::Save,
            merge: Box::new(|ed: &mut Editor| ed.status = "saved".into()) }, &mut e);
        assert_eq!(e.status, "saved");
        // Stale coalescible: dropped.
        apply_result(JobResult { buffer_id: id, class: ResultClass::BufferLocal, version: 3, kind: JobKind::CoalesceProbe,
            merge: Box::new(|ed: &mut Editor| ed.status = "STALE".into()) }, &mut e);
        assert_eq!(e.status, "saved", "stale coalescible result must be dropped");
    }

    #[test]
    fn buffer_local_result_for_missing_buffer_is_dropped() {
        use crate::editor::{Editor, BufferId};
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        // A buffer-local merge for a non-existent buffer must NOT run.
        apply_result(JobResult {
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
        apply_result(JobResult {
            buffer_id: id, class: ResultClass::BufferLocal,
            version: 1, kind: JobKind::Save,
            merge: Box::new(|ed: &mut Editor| ed.status = "merged".into()),
        }, &mut e);
        assert_eq!(e.status, "merged");
    }

    #[test]
    fn durability_result_for_missing_buffer_still_runs() {
        use crate::editor::{Editor, BufferId};
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        // A durability completion runs even though its buffer is gone (e.g. closed).
        apply_result(JobResult {
            buffer_id: BufferId(999), class: ResultClass::Durability,
            version: 1, kind: JobKind::SwapWrite,
            merge: Box::new(|ed: &mut Editor| ed.status = "durability ran".into()),
        }, &mut e);
        assert_eq!(e.status, "durability ran");
    }
}
