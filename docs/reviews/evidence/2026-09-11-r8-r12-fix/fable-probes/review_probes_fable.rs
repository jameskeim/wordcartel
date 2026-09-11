//! Fable review probes for the R8/R12 branch (2026-09-11). Test-only.
//!
//! To run: copy to `wordcartel/src/review_probes_fable.rs`, add
//! `#[cfg(test)] mod review_probes_fable;` to `wordcartel/src/lib.rs`, then
//! `cargo test -p wordcartel --lib review_probes_fable -- --nocapture --test-threads=1`.
//! All 15 passed against the reviewed tree; P1/P4/P9/P11 print the observed state they document.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use crate::editor::{Buffer, Editor};
    use crate::jobs::{Executor, Job, JobOutcome};
    use crate::registry::{CommandId, Ctx, Registry};
    use crate::test_support::{test_fs, TestClock};

    #[derive(Default)]
    struct DeferredExecutor { jobs: RefCell<VecDeque<Job>> }

    impl Executor for DeferredExecutor {
        fn dispatch(&self, job: Job) { self.jobs.borrow_mut().push_back(job); }
        fn drain(&self) -> Vec<JobOutcome> { Vec::new() }
    }

    impl DeferredExecutor {
        fn complete_next(&self, editor: &mut Editor) {
            let job = self.jobs.borrow_mut().pop_front().expect("a save is pending");
            let outcome = JobOutcome::Done((job.run)());
            let (tx, _rx) = std::sync::mpsc::channel();
            crate::jobs_apply::apply_job_outcome(outcome, editor, self, &TestClock(10), &tx, &test_fs());
        }
        fn pending(&self) -> usize { self.jobs.borrow().len() }
    }

    fn command(editor: &mut Editor, executor: &dyn Executor, name: &'static str) {
        let (tx, _rx) = std::sync::mpsc::channel();
        Registry::builtins().dispatch(CommandId(name), &mut Ctx {
            editor, executor, clock: &TestClock(0), msg_tx: tx, fs: test_fs(),
        });
    }

    fn insert(editor: &mut Editor, text: &str) {
        let len = editor.active().document.buffer.len();
        let cs = wordcartel_core::change::ChangeSet::insert(0, text, len);
        crate::transact::submit_transaction(editor,
            wordcartel_core::history::Transaction::new(cs), &TestClock(1)).unwrap();
    }

    fn action(editor: &mut Editor, ex: &dyn Executor, action: crate::prompt::PromptAction) {
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(action, editor, ex, &TestClock(2), &tx, &test_fs());
    }

    fn two_dirty() -> (tempfile::TempDir, Editor, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();
        let mut e = Editor::new_from_text("A", Some(a.clone()), (80, 24));
        insert(&mut e, "edited ");
        let id = e.alloc_id();
        e.buffers.push(Buffer::from_text(id, "B", Some(b.clone()), (80, 24)));
        e.switch_to_index(1);
        insert(&mut e, "edited ");
        e.switch_to_index(0);
        (dir, e, a, b)
    }

    // P1: a manual Save As dispatched AFTER Save and Quit's ordinary save. The ordinary save
    // lands first (FIFO) and the drain's refill sees a clean workspace while the Save As
    // write is still queued. Observed: quit=true with one durability job still queued.
    #[test]
    fn p1_manual_save_as_after_save_and_quit_quits_with_save_as_still_queued() {
        let (_dir, mut e, a, _b) = two_dirty();
        e.by_id_mut(e.buffers[1].id).unwrap().document.saved_version = Some(e.buffers[1].document.version); // B clean
        let target = a.parent().unwrap().join("renamed.md");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, target.clone(), target.clone(), &ex, &TestClock(1), &tx, &test_fs());
        assert_eq!(ex.pending(), 2);
        ex.complete_next(&mut e); // ordinary save to A lands
        eprintln!("P1: quit={} pending_jobs={} path={:?}", e.quit, ex.pending(), e.active().document.path);
        assert!(ex.pending() == 1, "the Save As write is still queued");
    }

    // P2: Save All (summary prompt) where the second buffer is unnamed; the picker opens and the
    // user backs out with Esc. Nothing must strand, nothing must quit, A must be on disk.
    #[test]
    fn p2_save_all_unnamed_second_buffer_picker_escape_aborts_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        std::fs::write(&a, "A").unwrap();
        let mut e = Editor::new_from_text("A", Some(a.clone()), (80, 24));
        insert(&mut e, "edited ");
        let id = e.alloc_id();
        e.buffers.push(Buffer::from_text(id, "unnamed", None, (80, 24)));
        e.switch_to_index(1);
        insert(&mut e, "edited ");
        e.switch_to_index(0);
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, crate::prompt::PromptAction::QuitSaveAll);
        assert_eq!(ex.pending(), 1);
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert!(e.file_browser.as_ref().is_some_and(|fb| fb.mode.is_destination()), "picker for the unnamed buffer");
        assert_eq!(e.pending_save_as, Some(crate::editor::PostSaveAction::ContinueQuitDrain));
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::test_support::press_key_fb(&mut e, &test_fs(), &tx, crossterm::event::KeyCode::Esc);
        assert!(e.file_browser.is_none());
        assert!(e.pending_save_as.is_none());
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        assert!(!e.quit);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "edited A");
    }

    // P3: Review Each → Save on a buffer whose file changed on disk. The conflict modal must
    // show, the quit must be cancelled (no strand), and resolving with Overwrite must not quit.
    #[test]
    fn p3_review_save_conflict_cancels_quit_and_keeps_modal() {
        let (_dir, mut e, a, _b) = two_dirty();
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, crate::prompt::PromptAction::QuitReviewEach);
        assert!(e.prompt.is_some());
        std::fs::write(&a, "external").unwrap();
        action(&mut e, &ex, crate::prompt::PromptAction::ReviewSave);
        assert_eq!(ex.pending(), 0);
        assert!(e.prompt.is_some(), "external-mod modal shown");
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        action(&mut e, &ex, crate::prompt::PromptAction::Overwrite);
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "edited A");
    }

    // P4: a manual Save As picker opened during Save and Quit's in-flight save, then Esc.
    // `cancel_destination` clears the drain but not `pending_after_save`. Observed: after the
    // save lands, quit=false and the status is just "Saved" (the quit evaporates silently).
    #[test]
    fn p4_manual_picker_escape_during_save_and_quit_leaves_pending_armed() {
        let (_dir, mut e, _a, _b) = two_dirty();
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        let (tx, _rx) = std::sync::mpsc::channel();
        assert!(crate::prompts::open_save_as(&mut e, &test_fs(), &tx));
        crate::test_support::press_key_fb(&mut e, &test_fs(), &tx, crossterm::event::KeyCode::Esc);
        eprintln!("P4 after Esc: drain={:?} pending={:?}", e.quit_drain.is_some(), e.pending_after_save.is_some());
        ex.complete_next(&mut e);
        eprintln!("P4 after completion: quit={} pending_jobs={} status={:?}", e.quit, ex.pending(), e.status_text());
        assert!(!e.quit);
        assert!(e.pending_after_save.is_none());
    }

    // P5: Save and Quit from a CLEAN named active buffer with another dirty buffer.
    #[test]
    fn p5_save_and_quit_from_clean_named_active_drains_other_dirty() {
        let (_dir, mut e, a, b) = two_dirty();
        let aid = e.active().id;
        e.active_mut().document.saved_version = Some(e.active().document.version);
        assert!(!e.is_dirty(aid));
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert_eq!(ex.pending(), 1, "explicit save of the active buffer is preserved");
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert_eq!(ex.pending(), 1, "B is drained next");
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "edited A");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "edited B");
    }

    // P6: Save All where the active buffer is edited during its own save; it must be re-saved
    // (not skipped) and the drain must still reach B and quit with the newest content on disk.
    #[test]
    fn p6_save_all_reconverges_after_edit_during_save() {
        let (_dir, mut e, a, b) = two_dirty();
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, crate::prompt::PromptAction::QuitSaveAll);
        assert_eq!(ex.pending(), 1);
        insert(&mut e, "again ");
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert_eq!(ex.pending(), 1);
        assert_eq!(e.active().document.path.as_deref(), Some(a.as_path()), "A re-saved before moving on");
        ex.complete_next(&mut e);
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "again edited A");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "edited B");
    }

    // P7: Quit command re-entered during Save and Quit; picking Save All must be refused and the
    // original flow must still complete.
    #[test]
    fn p7_quit_command_reentry_is_refused_and_original_flow_completes() {
        let (_dir, mut e, _a, b) = two_dirty();
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        let pending = e.pending_after_save.clone();
        let r = crate::commands::run(crate::commands::Command::Quit, &mut e, &TestClock(0));
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert!(e.prompt.is_some(), "quit summary prompt raised over the running flow");
        action(&mut e, &ex, crate::prompt::PromptAction::QuitSaveAll);
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.pending_after_save, pending);
        assert_eq!(ex.pending(), 1);
        ex.complete_next(&mut e);
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "edited B");
    }

    // P8: Quit command re-entered during Save and Quit, then Cancel: the running flow is aborted
    // and the late completion must not quit.
    #[test]
    fn p8_quit_command_then_cancel_aborts_running_flow() {
        let (_dir, mut e, _a, _b) = two_dirty();
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        crate::commands::run(crate::commands::Command::Quit, &mut e, &TestClock(0));
        action(&mut e, &ex, crate::prompt::PromptAction::Cancel);
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert_eq!(ex.pending(), 0);
    }

    // P9: Save As dispatched BEFORE Save and Quit (not busy: a plain Save As arms nothing). The
    // ordinary save targets the old path and completes second. Observed: quit cancelled with the
    // destination warning; the buffer is clean at the new path; nothing stranded.
    #[test]
    fn p9_save_as_before_save_and_quit_cancels_quit_with_warning() {
        let (_dir, mut e, a, _b) = two_dirty();
        e.by_id_mut(e.buffers[1].id).unwrap().document.saved_version = Some(e.buffers[1].document.version); // B clean
        let target = a.parent().unwrap().join("renamed.md");
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, target.clone(), target.clone(), &ex, &TestClock(1), &tx, &test_fs());
        command(&mut e, &ex, "save_and_quit");
        assert_eq!(ex.pending(), 2);
        ex.complete_next(&mut e); // Save As rekeys to renamed.md
        assert_eq!(e.active().document.path.as_deref(), Some(target.as_path()));
        ex.complete_next(&mut e); // ordinary save to a.md: destination mismatch
        eprintln!("P9: quit={} dirty={} status={:?} drain={} pending={}", e.quit, e.active().document.dirty(),
            e.status_text(), e.quit_drain.is_some(), e.pending_after_save.is_some());
        assert!(!e.quit);
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        assert!(!e.active().document.dirty(), "buffer is clean at renamed.md");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
    }

    // P10: timeout in the middle of Save All (second buffer in flight).
    #[test]
    fn p10_timeout_mid_save_all_cancels_without_strand() {
        let (_dir, mut e, a, b) = two_dirty();
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, crate::prompt::PromptAction::QuitSaveAll);
        ex.complete_next(&mut e);
        assert_eq!(ex.pending(), 1, "B in flight");
        crate::timers::save_timeout_tick(&mut e, 6000);
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "edited A");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "edited B");
    }

    // P11: a Panicked outcome for a DIFFERENT save of the same buffer/version while a
    // Save and Quit awaits its own request. `apply_panic` matches on id/version only.
    // Observed: the awaited flow is cancelled (conservative; no data loss).
    #[test]
    fn p11_unrelated_same_version_panic_cancels_awaited_quit() {
        let (_dir, mut e, _a, _b) = two_dirty();
        let id = e.active().id;
        let v = e.active().document.version;
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::jobs_apply::apply_job_outcome(JobOutcome::Panicked {
            buffer_id: id, version: v, kind: crate::jobs::JobKind::Save, msg: "other".into() },
            &mut e, &ex, &TestClock(3), &tx, &test_fs());
        eprintln!("P11: drain={} pending={}", e.quit_drain.is_some(), e.pending_after_save.is_some());
        ex.complete_next(&mut e);
        assert!(!e.quit, "conservative: the awaited save no longer quits");
    }

    // P12: Save and Quit from an UNNAMED dirty active with another dirty named buffer; the
    // picker commit (via perform_save_as) must bind the drain continuation and drain B.
    #[test]
    fn p12_save_and_quit_unnamed_active_then_drains_named_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let b = dir.path().join("b.md");
        let target = dir.path().join("new.md");
        std::fs::write(&b, "B").unwrap();
        let mut e = Editor::new_from_text("draft", None, (80, 24));
        insert(&mut e, "edited ");
        let id = e.alloc_id();
        e.buffers.push(Buffer::from_text(id, "B", Some(b.clone()), (80, 24)));
        e.switch_to_index(1);
        insert(&mut e, "edited ");
        e.switch_to_index(0);
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert_eq!(e.pending_save_as, Some(crate::editor::PostSaveAction::ContinueQuitDrain));
        e.file_browser = None; // the commit path below is what the picker's Enter reaches
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, target.clone(), target.clone(), &ex, &TestClock(1), &tx, &test_fs());
        assert!(e.pending_after_save.as_ref().is_some_and(|p| p.save_request.is_some()));
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert_eq!(ex.pending(), 1, "B drained next");
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "edited draft");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "edited B");
    }

    // P13: Review Each: discard A, then undo A's edit (version bumps). Refill must re-ask,
    // never treat the new version as covered by the old discard.
    #[test]
    fn p13_undo_after_discard_reprompts() {
        let (_dir, mut e, _a, _b) = two_dirty();
        let aid = e.active().id;
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, crate::prompt::PromptAction::QuitReviewEach);
        action(&mut e, &ex, crate::prompt::PromptAction::ReviewDiscard); // A discarded; reviewing B
        e.switch_to_index(0);
        assert!(e.active_mut().undo());
        assert!(e.is_dirty(aid), "undo bumps version; still dirty by version");
        action(&mut e, &ex, crate::prompt::PromptAction::ReviewDiscard); // B discarded
        assert!(!e.quit);
        assert_eq!(e.quit_drain.as_ref().unwrap().reviewing.map(|r| r.0), Some(aid));
    }

    // P14: Review Each: the reviewed buffer's save fails (write fault). The flow must cancel,
    // not quit, not strand, and A stays dirty.
    #[test]
    fn p14_review_save_failure_cancels_flow() {
        let (_dir, mut e, _a, _b) = two_dirty();
        let ex = DeferredExecutor::default();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::test_support::FaultFs::new(crate::test_support::FaultAt::Rename));
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(crate::prompt::PromptAction::QuitReviewEach, &mut e, &ex, &TestClock(0), &tx, &fs);
        crate::prompts::resolve_prompt(crate::prompt::PromptAction::ReviewSave, &mut e, &ex, &TestClock(0), &tx, &fs);
        assert_eq!(ex.pending(), 1);
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        assert!(e.active().document.dirty());
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
    }

    // P15: close_buffer is refused while a quit flow is pending (busy guard still holds with the
    // new drain shape), and the flow completes afterwards.
    #[test]
    fn p15_close_buffer_refused_during_quit_flow() {
        let (_dir, mut e, _a, _b) = two_dirty();
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        e.switch_to_index(1);
        crate::workspace::close_buffer(&mut e);
        assert!(e.prompt.is_none());
        assert_eq!(e.buffers.len(), 2);
        ex.complete_next(&mut e);
        ex.complete_next(&mut e);
        assert!(e.quit);
    }
}
