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
                            editor.set_status_full(crate::status::StatusKind::Warning, "edited during save — quit cancelled",
                                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
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
                crate::editor::PostSaveAction::CloseBuffer { id } => {
                    editor.pending_after_save = None;
                    if saved_this && !editor.is_dirty(id) {
                        // is_dirty on the ACTION's id, not the result's buffer_id —
                        // only the Save-As divergence separates them, and the
                        // buffer_id misreading would close a still-dirty buffer
                        // (spec D3). close_buffer_now re-reads counts at apply time.
                        crate::workspace::close_buffer_now(editor, id);
                        editor.set_status(crate::status::StatusKind::Info, "saved — closed");
                    } else if saved_this {
                        // Edited during the in-flight save: do NOT close.
                        editor.set_status_full(crate::status::StatusKind::Warning, "edited during save — close cancelled",
                            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                    }
                    // !saved_this: no close; the merge's own status stands (error
                    // text, or empty for a vanished target) — mirrors
                    // ContinueQuitDrain's abort, NOT Quit's leave-armed (an armed
                    // close would fire on the user's next manual save). (The
                    // saved-branch's "saved — closed" harmlessly shadows the
                    // vanished-id status — unreachable for the pending's own id.)
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
            // A panic is a failed Save completion: collapse the same Save(buffer_id, version) start
            // this job posted (the merge never ran, but the "Saving…" progress entry is live). The
            // `buffer_id`/`version` in hand reconstruct the identical topic key.
            editor.finish_topic(crate::status::StatusTopic::Save(buffer_id, version),
                crate::status::StatusKind::Error, format!("save failed (internal error: {msg})"));
        }
        JobKind::SwapWrite => {
            if let Some(b) = editor.by_id_mut(buffer_id) { b.swap_in_flight = false; }
            editor.set_status_full(crate::status::StatusKind::Error, format!("swap failed (internal error: {msg})"),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
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
        JobKind::CoalesceProbe => {
            editor.set_status_full(crate::status::StatusKind::Error, format!("job failed (internal error: {msg})"),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        }
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
            editor.finish_topic(crate::status::StatusTopic::Filter, crate::status::StatusKind::Warning,
                "filter discarded - buffer changed");
        }
        crate::filter::RunResult::Err(err) => {
            editor.finish_topic(crate::status::StatusTopic::Filter, crate::status::StatusKind::Error,
                crate::filter::describe_error(&err));
        }
        crate::filter::RunResult::Stdout(text) => {
            let (from, to, at) = match disposition {
                crate::filter::Disposition::Filter => (range.start, range.end, range.start),
                crate::filter::Disposition::Insert => (cursor, cursor, cursor),
            };
            let doc_len = editor.by_id(buffer_id).map(|b| b.document.buffer.len());
            if let Some(doc_len) = doc_len {
                let (cs, edit) = crate::commands::build_range_replace(from, to, &text, doc_len);
                let txn = wordcartel_core::history::Transaction::new(cs)
                    .with_selection(wordcartel_core::selection::Selection::single(at + text.len()));
                if crate::edit_apply::apply_edit(editor, buffer_id, txn, edit,
                       wordcartel_core::history::EditKind::Other, clock)
                    == crate::edit_apply::EditOutcome::Applied
                {
                    editor.finish_topic(crate::status::StatusTopic::Filter,
                        crate::status::StatusKind::Info, "filter applied");
                }
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
        editor.finish_topic(crate::status::StatusTopic::Transform, crate::status::StatusKind::Warning,
            "transform discarded — buffer changed");
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
        editor.set_status_full(crate::status::StatusKind::Warning, format!(
            "export target {} appeared — re-run export to overwrite",
            target.display()
        ), crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return;
    }
    match result {
        Ok(crate::export::ExportResult::Bytes(bytes)) => {
            match file::save_atomic_bytes(&target, &bytes) {
                Ok(()) => {
                    let status = format!("exported {}", target.display());
                    editor.set_status(crate::status::StatusKind::Info, status);
                }
                Err(e) => {
                    editor.set_status_full(crate::status::StatusKind::Error, format!("export write failed: {e}"),
                        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                }
            }
        }
        Ok(crate::export::ExportResult::TempReady(tmp)) => {
            match std::fs::rename(&tmp, &target) {
                Ok(()) => {
                    let status = format!("exported {}", target.display());
                    editor.set_status(crate::status::StatusKind::Info, status);
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    editor.set_status_full(crate::status::StatusKind::Error, format!("export rename failed: {e}"),
                        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                }
            }
        }
        Err(e) => {
            editor.set_status_full(crate::status::StatusKind::Error, crate::filter::describe_error(&e),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        }
    }
}

pub(crate) fn insert_paste_text(editor: &mut Editor, buffer_id: crate::editor::BufferId, text: &str, clock: &dyn Clock) -> bool {
    // A17 F5 (final gate): paste into a read-only buffer is a LOUD reject, not a silent no-op.
    // `Buffer::apply` already no-ops safely on a read-only slot, but this path is not a registry
    // command (the completeness sweep can't see it) and previously issued no `reject_read_only` and
    // still set the register in its callers. Reject here — returning `false` also makes both callers
    // skip their register set (they gate on the return value).
    if editor.by_id(buffer_id).is_some_and(|b| b.read_only) {
        editor.reject_read_only();
        return false;
    }
    if text.len() > crate::clipboard::PASTE_MAX_BYTES {
        editor.set_status_full(crate::status::StatusKind::Warning,
            format!("paste too large ({} MiB) — skipped", text.len() / (1 << 20)),
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return false;
    }
    let sel_from = editor.by_id(buffer_id).map(|b| {
        let sel = b.document.selection.primary(); (sel.from(), sel.to())
    });
    let Some((from, to)) = sel_from else { return false; };
    let doc_len = editor.by_id(buffer_id).map(|b| b.document.buffer.len()).unwrap_or(0);
    let (cs, edit) = crate::commands::build_range_replace(from, to, text, doc_len);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(from + text.len()));
    matches!(
        crate::edit_apply::apply_edit(editor, buffer_id, txn, edit,
            wordcartel_core::history::EditKind::Other, clock),
        crate::edit_apply::EditOutcome::Applied
    )
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
        editor.set_status_full(crate::status::StatusKind::Warning,
            "system clipboard unavailable — copy/paste work in-editor (register only)",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        editor.clipboard_notice_shown = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestClock;
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn quit_tmp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "wc-c4-{}-{}-{}.md",
            tag, std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)))
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
        // A17 T5 (F4 Warning table): "edited during save — quit cancelled" is a Sticky Warning.
        assert_eq!(e.status_text(), "edited during save — quit cancelled");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
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
            merge: Box::new(|ed: &mut Editor| ed.set_status(crate::status::StatusKind::Info, "saved")) }, &mut e);
        assert_eq!(e.status_text(), "saved");
        // Stale coalescible: dropped.
        apply_result(JobResult { buffer_id: id, class: ResultClass::BufferLocal, version: 3, kind: JobKind::CoalesceProbe,
            merge: Box::new(|ed: &mut Editor| ed.set_status(crate::status::StatusKind::Info, "STALE")) }, &mut e);
        assert_eq!(e.status_text(), "saved", "stale coalescible result must be dropped");
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
            merge: Box::new(|ed: &mut Editor| ed.set_status(crate::status::StatusKind::Info, "SHOULD NOT RUN")),
        }, &mut e);
        assert_ne!(e.status_text(), "SHOULD NOT RUN", "buffer-local merge for a missing buffer is dropped");
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
            merge: Box::new(|ed: &mut Editor| ed.set_status(crate::status::StatusKind::Info, "merged")),
        }, &mut e);
        assert_eq!(e.status_text(), "merged");
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
            merge: Box::new(|ed: &mut Editor| ed.set_status(crate::status::StatusKind::Info, "durability ran")),
        }, &mut e);
        assert_eq!(e.status_text(), "durability ran");
    }

    // -----------------------------------------------------------------------
    // C4: close-buffer Save/Discard/Cancel state-machine battery
    // -----------------------------------------------------------------------

    #[test]
    fn close_after_save_closes_on_matching_result() {
        // CloseSave on a dirty named buffer → drain → buffer count drops, correct neighbor
        // active by ID, file on disk updated, status "saved — closed".
        use crate::editor::{Editor, Buffer};
        use crate::jobs::{Executor, InlineExecutor};
        use crate::prompt::PromptAction;
        let p = quit_tmp("close-match");
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new content\n", Some(p.clone()), (80, 24));
        e.active_mut().document.version = 1;
        e.active_mut().document.saved_version = None; // dirty
        let x_id = e.active().id;
        // Add a neighbor so we can check which becomes active after close.
        let y_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(Buffer::from_text(y_id, "neighbor\n", None, area));
        e.install_scratch();
        e.mru = vec![x_id, y_id, e.scratch_id.unwrap()];
        e.active = 0; // x_id active
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(PromptAction::CloseSave { id: x_id }, &mut e, &ex, &clk, &tx);
        assert!(e.pending_after_save.is_some(), "pending armed");
        let pre_count = e.buffers.len();
        for o in ex.drain() { apply_outcome(o, &mut e); }
        assert!(e.by_id(x_id).is_none(), "closed buffer gone");
        assert!(e.buffers.len() < pre_count, "buffer count drops");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new content\n", "file updated");
        assert_eq!(e.status_text(), "saved — closed");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn close_cancelled_when_edited_during_flight() {
        // Buffer edited during the in-flight save: result fires (saved_this=true) but buffer
        // is dirty again → do NOT close; status verbatim "edited during save — close cancelled".
        use crate::editor::{Editor, PostSaveAction};
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let p = quit_tmp("flight");
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        let id = e.active().id;
        e.active_mut().document.version = 1;
        e.active_mut().document.saved_version = None; // dirty at dispatch time
        e.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id: id, version: 1,
            action: PostSaveAction::CloseBuffer { id },
            at_ms: 0,
        });
        // Simulate edit during the in-flight save: version advances
        e.active_mut().document.version = 2;
        // Deliver the save result for version=1 (sets saved_version=1, but version=2 → dirty)
        let save_result = JobResult {
            buffer_id: id,
            class: ResultClass::Durability,
            version: 1,
            kind: JobKind::Save,
            merge: Box::new(move |editor: &mut Editor| {
                if let Some(b) = editor.by_id_mut(id) { b.document.saved_version = Some(1); }
            }),
        };
        apply_result(save_result, &mut e);
        assert!(e.by_id(id).is_some(), "buffer NOT closed — still dirty");
        assert_eq!(e.status_text(), "edited during save — close cancelled");
        // A17 T5 (F4 Warning table): a Sticky Warning.
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        assert!(e.pending_after_save.is_none(), "pending consumed");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn close_not_performed_on_save_failure() {
        #[cfg(not(unix))] { return; }
        #[cfg(unix)]
        {
            // Symlink-target trick: the save fails → buffer stays, pending cleared,
            // error status contains "symlink".
            use crate::jobs::{Executor, InlineExecutor};
            use crate::prompt::PromptAction;
            let real = quit_tmp("real");
            std::fs::write(&real, "real\n").unwrap();
            let link = quit_tmp("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let mut e = Editor::new_from_text("x\n", Some(link.clone()), (80, 24));
            e.active_mut().document.saved_version = None;
            e.active_mut().document.version = 1; // dirty
            let id = e.active().id;
            e.install_scratch();
            let ex = InlineExecutor::default();
            let clk = TestClock(0);
            let (tx, _rx) = std::sync::mpsc::channel();
            crate::prompts::resolve_prompt(PromptAction::CloseSave { id }, &mut e, &ex, &clk, &tx);
            for o in ex.drain() { apply_outcome(o, &mut e); }
            assert!(e.by_id(id).is_some(), "buffer NOT closed — save failed");
            assert!(e.pending_after_save.is_none(), "pending cleared on save failure");
            assert!(e.status_text().to_lowercase().contains("symlink"), "error status: {:?}", e.status_text());
            let _ = std::fs::remove_file(&link);
            let _ = std::fs::remove_file(&real);
        }
    }

    #[test]
    fn close_result_for_wrong_buffer_is_stale_noop() {
        // Arm for buffer A, deliver a matching-versioned result for buffer B → nothing closes;
        // the fire predicate checks buffer_id AND version — a mismatched buffer_id → fire=false.
        use crate::editor::{Editor, PostSaveAction, Buffer};
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let p_a = quit_tmp("wrong-a");
        std::fs::write(&p_a, "a\n").unwrap();
        let mut e = Editor::new_from_text("new_a\n", Some(p_a.clone()), (80, 24));
        e.active_mut().document.version = 1;
        e.active_mut().document.saved_version = None;
        let a_id = e.active().id;
        let b_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(Buffer::from_text(b_id, "b\n", None, area));
        // Arm pending_after_save for A
        e.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id: a_id, version: 1,
            action: PostSaveAction::CloseBuffer { id: a_id },
            at_ms: 0,
        });
        // Deliver a save result for B (not A) at version 1
        let save_result = JobResult {
            buffer_id: b_id,
            class: ResultClass::Durability,
            version: 1,
            kind: JobKind::Save,
            merge: Box::new(move |editor: &mut Editor| {
                if let Some(b) = editor.by_id_mut(b_id) { b.document.saved_version = Some(1); }
            }),
        };
        apply_result(save_result, &mut e);
        // A must still be present (fire=false because buffer_id mismatch)
        assert!(e.by_id(a_id).is_some(), "A not closed — wrong buffer in result");
        assert!(e.pending_after_save.is_some(), "pending still armed — not consumed by wrong result");
        let _ = std::fs::remove_file(&p_a);
    }

    #[test]
    fn close_after_save_last_ordinary_while_scratch_active() {
        // Fable C1 corruption pin: last-ordinary close while scratch is active must NOT
        // overwrite scratch. D2 MRU pin: fresh untitled at BACK, scratch stays at FRONT.
        use crate::editor::{Editor, PostSaveAction};
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let p = quit_tmp("c1-scratch");
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.version = 1;
        e.active_mut().document.saved_version = None; // X dirty
        let x_id = e.active().id;
        e.install_scratch();
        let scratch_id = e.scratch_id.unwrap();
        let scratch_content = e.by_id(scratch_id).unwrap().document.buffer.to_string();
        // Arm pending_after_save for CloseBuffer{X}
        e.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id: x_id, version: 1,
            action: PostSaveAction::CloseBuffer { id: x_id },
            at_ms: 0,
        });
        // Switch to scratch during the in-flight save (C1 scenario)
        crate::workspace::goto_scratch(&mut e);
        assert_eq!(e.active().id, scratch_id, "precondition: scratch active");
        // Apply X's save result (marks X clean so close_buffer_now will close it)
        let save_result = JobResult {
            buffer_id: x_id,
            class: ResultClass::Durability,
            version: 1,
            kind: JobKind::Save,
            merge: Box::new(move |editor: &mut Editor| {
                if let Some(b) = editor.by_id_mut(x_id) { b.document.saved_version = Some(1); }
            }),
        };
        apply_result(save_result, &mut e);
        // Scratch must be intact
        assert!(e.scratch_id.is_some(), "scratch_id still valid");
        let scratch = e.by_id(scratch_id).expect("scratch buffer still present");
        assert_eq!(scratch.document.buffer.to_string(), scratch_content, "scratch content untouched");
        // X is gone; a fresh untitled replaced its slot
        assert!(e.by_id(x_id).is_none(), "X closed");
        assert_eq!(e.buffers.len(), 2, "[fresh_untitled, scratch]");
        // D2 MRU: scratch at FRONT, fresh at BACK
        let fresh_id = e.buffers.iter().find(|b| !e.is_scratch(b.id)).map(|b| b.id).expect("fresh untitled");
        assert_eq!(e.mru.first(), Some(&scratch_id), "scratch at MRU front");
        assert_eq!(e.mru.last(), Some(&fresh_id), "fresh untitled at MRU back");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn close_after_save_last_ordinary_recheck() {
        // Codex plan r1: ordinary count re-read at APPLY time. [X, Y, scratch], CloseSave{X},
        // Y closed during flight → count=1 at apply → last-ordinary path fires (fresh untitled).
        use crate::editor::{Editor, PostSaveAction, Buffer};
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let p0 = quit_tmp("recheck-x");
        std::fs::write(&p0, "x\n").unwrap();
        let p1 = quit_tmp("recheck-y");
        std::fs::write(&p1, "y\n").unwrap();
        let mut e = Editor::new_from_text("new_x\n", Some(p0.clone()), (80, 24));
        e.active_mut().document.version = 1;
        e.active_mut().document.saved_version = None; // X dirty
        let x_id = e.active().id;
        let y_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(Buffer::from_text(y_id, "y\n", Some(p1.clone()), area)); // Y clean
        e.install_scratch();
        // Arm CloseBuffer{X}
        e.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id: x_id, version: 1,
            action: PostSaveAction::CloseBuffer { id: x_id },
            at_ms: 0,
        });
        // Close Y during the flight (Y is clean)
        crate::workspace::close_buffer_now(&mut e, y_id);
        assert!(e.by_id(y_id).is_none(), "Y closed during flight");
        // Now [X, scratch] — ordinary count = 1 at apply time
        let save_result = JobResult {
            buffer_id: x_id,
            class: ResultClass::Durability,
            version: 1,
            kind: JobKind::Save,
            merge: Box::new(move |editor: &mut Editor| {
                if let Some(b) = editor.by_id_mut(x_id) { b.document.saved_version = Some(1); }
            }),
        };
        apply_result(save_result, &mut e);
        // X was the last ordinary buffer at apply time → last-ordinary path fires
        assert!(e.by_id(x_id).is_none(), "X closed");
        assert_eq!(e.buffers.len(), 2, "fresh untitled + scratch");
        assert!(e.buffers.iter().any(|b| b.document.path.is_none() && !e.is_scratch(b.id)),
            "fresh untitled present in X's slot");
        assert_eq!(e.status_text(), "saved — closed");
        let _ = std::fs::remove_file(&p0);
        let _ = std::fs::remove_file(&p1);
    }

    #[test]
    fn close_save_on_conflicted_file_raises_external_mod_and_does_not_arm() {
        // dispatch_save_then skips arming pending_after_save when the external-mod
        // modal is raised (dispatch_save_then's guard: prompt.is_some() → no arm).
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::prompt::PromptAction;
        let p = quit_tmp("conflict");
        std::fs::write(&p, "v0\n").unwrap();
        let mut e = Editor::new_from_text("edited\n", Some(p.clone()), (80, 24));
        // stored_fp captured at construction = fingerprint of "v0\n"
        e.active_mut().document.version = 1;
        e.active_mut().document.saved_version = None; // dirty
        let id = e.active().id;
        // External change (different size → guaranteed fingerprint divergence)
        std::fs::write(&p, "external change — much longer content here\n").unwrap();
        e.install_scratch();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(PromptAction::CloseSave { id }, &mut e, &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "external-mod conflict raises the modal");
        assert!(e.pending_after_save.is_none(), "pending_after_save NOT armed on conflict");
        let _ = std::fs::remove_file(&p);
    }

    // -----------------------------------------------------------------------
    // A17 T4: F4 Error-table sites — a genuine failure lands Sticky/Error and
    // survives a later Info ack (Q1). One test per row.
    // -----------------------------------------------------------------------

    fn assert_sticky_error_survives_info(e: &mut Editor) {
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        e.set_status(crate::status::StatusKind::Info, "later ack");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error, "Q1: Info must not displace a held Error");
    }

    /// A17 T5 (F4 Warning table): mirrors `assert_sticky_error_survives_info` for Warning sites.
    fn assert_sticky_warning_survives_info(e: &mut Editor) {
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        e.set_status(crate::status::StatusKind::Info, "later ack");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning, "Q1: Info must not displace a held Warning");
    }

    #[test]
    fn apply_panic_save_is_a_sticky_error() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let id = e.active().id;
        apply_outcome(crate::jobs::JobOutcome::Panicked {
            buffer_id: id, version: 0, kind: crate::jobs::JobKind::Save, msg: "boom".into(),
        }, &mut e);
        assert!(e.status_text().contains("save failed"));
        assert_sticky_error_survives_info(&mut e);
    }

    #[test]
    fn apply_panic_swap_write_is_a_sticky_error() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let id = e.active().id;
        apply_outcome(crate::jobs::JobOutcome::Panicked {
            buffer_id: id, version: 0, kind: crate::jobs::JobKind::SwapWrite, msg: "boom".into(),
        }, &mut e);
        assert!(e.status_text().contains("swap failed"));
        assert_sticky_error_survives_info(&mut e);
    }

    #[test]
    fn apply_panic_coalesce_probe_is_a_sticky_error() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let id = e.active().id;
        apply_outcome(crate::jobs::JobOutcome::Panicked {
            buffer_id: id, version: 0, kind: crate::jobs::JobKind::CoalesceProbe, msg: "boom".into(),
        }, &mut e);
        assert!(e.status_text().contains("job failed"));
        assert_sticky_error_survives_info(&mut e);
    }

    #[test]
    fn apply_export_done_write_failure_is_a_sticky_error() {
        use crate::editor::Editor;
        // A target whose parent is a regular FILE (not a dir) → save_atomic_bytes fails
        // (ENOTDIR), driving apply_export_done's Bytes/Err(e) "export write failed" arm.
        let parent = std::env::temp_dir().join(format!("wc-c4-exportwrite-{}.md", std::process::id()));
        std::fs::write(&parent, "i am a file\n").unwrap();
        let target = parent.join("out.html");
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        apply_export_done(&mut e, target, Ok(crate::export::ExportResult::Bytes(b"<p>x</p>".to_vec())), true);
        assert!(e.status_text().contains("export write failed"));
        assert_sticky_error_survives_info(&mut e);
        let _ = std::fs::remove_file(&parent);
    }

    #[test]
    fn apply_export_done_rename_failure_is_a_sticky_error() {
        use crate::editor::Editor;
        // TempReady names a tmp file that does not exist → std::fs::rename fails,
        // driving apply_export_done's TempReady/Err(e) "export rename failed" arm.
        let missing_tmp = std::env::temp_dir().join(format!("wc-c4-exportrename-missing-{}.tmp", std::process::id()));
        let target = std::env::temp_dir().join(format!("wc-c4-exportrename-target-{}.html", std::process::id()));
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        apply_export_done(&mut e, target, Ok(crate::export::ExportResult::TempReady(missing_tmp)), true);
        assert!(e.status_text().contains("export rename failed"));
        assert_sticky_error_survives_info(&mut e);
    }

    #[test]
    fn apply_export_done_pandoc_failure_is_a_sticky_error() {
        use crate::editor::Editor;
        let target = std::env::temp_dir().join(format!("wc-c4-exportpandoc-{}.html", std::process::id()));
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        apply_export_done(&mut e, target, Err(crate::filter::FilterError::Panicked("boom".into())), true);
        assert_sticky_error_survives_info(&mut e);
    }

    /// A17 T5 (F4 Warning table, Codex-r1 #1 row): the export TOCTOU refusal — the target
    /// appeared on disk between the overwrite check and completion — is a recoverable Sticky
    /// Warning, not an Error (the user just re-runs export).
    #[test]
    fn apply_export_done_toctou_target_appeared_is_a_sticky_warning() {
        use crate::editor::Editor;
        let target = std::env::temp_dir().join(format!("wc-c4-exporttoctou-{}.html", std::process::id()));
        std::fs::write(&target, "existing\n").unwrap();
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        apply_export_done(&mut e, target.clone(), Ok(crate::export::ExportResult::Bytes(b"<p>x</p>".to_vec())), false);
        assert!(e.status_text().contains("appeared — re-run export to overwrite"));
        assert_sticky_warning_survives_info(&mut e);
        let _ = std::fs::remove_file(&target);
    }

    /// A17 T5 (F4 Warning table): a paste over the size cap is a recoverable Sticky Warning.
    #[test]
    fn insert_paste_text_too_large_is_a_sticky_warning() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let id = e.active().id;
        let clk = TestClock(0);
        let huge = "x".repeat(crate::clipboard::PASTE_MAX_BYTES + 1);
        let ok = insert_paste_text(&mut e, id, &huge, &clk);
        assert!(!ok, "oversized paste must not be inserted");
        assert!(e.status_text().contains("paste too large"));
        assert_sticky_warning_survives_info(&mut e);
    }

    /// A17 F5 (final gate): a paste into a read-only buffer is a LOUD reject — no mutation AND the
    /// canonical "buffer is read-only" Sticky Warning (the paste path is not a registry command, so
    /// the completeness sweep can't cover it; the guard lives at the paste entry). It also skips the
    /// callers' register set by returning `false`.
    #[test]
    fn paste_into_a_read_only_buffer_is_a_loud_reject() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("keep\n", None, (80, 24));
        let id = e.active().id;
        e.active_mut().read_only = true;
        let clk = TestClock(0);
        let before = e.active().document.buffer.to_string();
        let ok = insert_paste_text(&mut e, id, "nope", &clk);
        assert!(!ok, "paste into a read-only buffer must not report success");
        assert_eq!(e.active().document.buffer.to_string(), before, "read-only: no mutation");
        assert_eq!(e.status_text(), "buffer is read-only");
    }

    /// A17 T5 (F4 Warning table): the system-clipboard-unavailable notice is a recoverable
    /// Sticky Warning (copy/paste still work in-editor via the register).
    #[test]
    fn apply_clipboard_availability_unavailable_is_a_sticky_warning() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        apply_clipboard_availability(&mut e, false);
        assert!(e.status_text().contains("system clipboard unavailable"));
        assert_sticky_warning_survives_info(&mut e);
    }

    /// H22 Task 3 (INV-GUARD false-ack pin): `apply_filter_done`'s Stdout arm must gate its
    /// "filter applied" ack on the edit actually applying — a read-only target must reject
    /// loudly (canonical status, no mutation), NOT silently no-op-then-ack.
    #[test]
    fn apply_filter_done_into_read_only_does_not_false_ack() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("keep\n", None, (80, 24));
        let id = e.active().id;
        let v = e.active().document.version;
        e.active_mut().read_only = true;
        let before = e.active().document.buffer.to_string();
        apply_filter_done(&mut e, id, v, 0..1, 0, crate::filter::Disposition::Filter,
            crate::filter::RunResult::Stdout("X".into()), &TestClock(0));
        assert_ne!(e.status_text(), "filter applied", "no success ack on a read-only reject");
        assert_eq!(e.status_text(), "buffer is read-only");
        assert_eq!(e.active().document.buffer.to_string(), before, "no mutation");
    }
}
