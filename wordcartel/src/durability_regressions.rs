//! Regression requirements for review findings R8 and R12.
//! Jobs execute in production FIFO order; tests explicitly control delivery time.

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
        fn try_dispatch(&self, job: Job) -> Result<(), crate::jobs::DispatchError> {
            self.jobs.borrow_mut().push_back(job); Ok(())
        }
        fn drain(&self) -> Vec<JobOutcome> { Vec::new() }
    }

    impl DeferredExecutor {
        fn complete_next(&self, editor: &mut Editor) {
            let job = self.jobs.borrow_mut().pop_front().expect("a save is pending");
            let outcome = job.execute();
            let (tx, _rx) = std::sync::mpsc::channel();
            crate::jobs_apply::apply_job_outcome(outcome, editor, self, &TestClock(10), &tx, &test_fs());
        }
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

    #[test]
    fn late_save_to_old_path_does_not_mark_current_path_clean() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "original").unwrap();
        let mut e = Editor::new_from_text("original", Some(a.clone()), (80, 24));
        let ex = DeferredExecutor::default();
        insert(&mut e, "first ");
        let first = e.active().document.buffer.to_string();
        let first_version = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, b.clone(), b.clone(), &ex, &TestClock(2), &tx, &test_fs());
        insert(&mut e, "second ");
        let second = e.active().document.buffer.to_string();
        command(&mut e, &ex, "save");
        assert_eq!(ex.jobs.borrow().len(), 2);
        ex.complete_next(&mut e);
        let saved_b_fp = e.active().document.stored_fp;
        let root = dir.path().join("recovery");
        let slot = e.active().recovery_slot.clone();
        let record = crate::recovery_store::CheckpointRecord::new(slot.reserve_generation().unwrap(),
            e.active().document.id.to_hex(), e.active().document.version,
            Some(crate::recovery_store::TaggedPath::from_path(&b)), None);
        let ack = crate::recovery_store::checkpoint(&crate::fsx::RealFs, &root,
            &slot, &record, "current B recovery").unwrap();
        let checkpoint = ack.record_path().to_owned();
        let checkpoint_bytes = std::fs::read(&checkpoint).unwrap();
        e.active_mut().recovery_ack = Some(ack);
        e.active_mut().swapped_version = Some(e.active().document.version);
        let checkpoint_version = e.active().swapped_version;
        ex.complete_next(&mut e);
        assert_eq!(e.active().document.path.as_deref(), Some(b.as_path()));
        assert_eq!(std::fs::read_to_string(&a).unwrap(), second);
        assert_eq!(std::fs::read_to_string(&b).unwrap(), first);
        assert!(e.active().document.dirty(), "B still lacks the second edit");
        assert_eq!(e.active().document.saved_version, Some(first_version));
        assert_eq!(e.active().document.stored_fp, saved_b_fp);
        assert_eq!(e.active().swapped_version, checkpoint_version);
        assert_eq!(std::fs::read(&checkpoint).unwrap(), checkpoint_bytes);
        assert!(e.pending_plugin_events.iter().any(|ev| ev.kind == crate::plugin::PluginEventKind::Save
            && ev.path.as_deref() == a.to_str()), "old-path write still fires a truthful save event");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
    }

    #[test]
    fn save_and_quit_does_not_exit_with_other_dirty_buffers() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();
        let mut e = Editor::new_from_text("A", Some(a), (80, 24));
        insert(&mut e, "edited ");
        let bid = e.alloc_id();
        e.buffers.push(Buffer::from_text(bid, "B", Some(b.clone()), (80, 24)));
        e.switch_to_index(1);
        insert(&mut e, "edited ");
        e.switch_to_index(0);
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        ex.complete_next(&mut e);
        assert!(!(e.quit && e.is_dirty(bid)), "exit would discard the other document without consent");
        assert_eq!(ex.jobs.borrow().len(), 1);
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert_eq!(std::fs::read_to_string(b).unwrap(), "edited B");
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

    #[test]
    fn review_discard_is_honored_for_unchanged_versions() {
        use crate::prompt::PromptAction::*;
        let (_dir, mut e, a, b) = two_dirty();
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, QuitReviewEach);
        action(&mut e, &ex, ReviewDiscard);
        assert!(!e.quit);
        action(&mut e, &ex, ReviewDiscard);
        assert!(e.quit, "do not ask again about unchanged versions explicitly discarded");
        assert!(ex.jobs.borrow().is_empty());
        assert_eq!(std::fs::read_to_string(a).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(b).unwrap(), "B");
    }

    #[test]
    fn edits_invalidate_discard_and_review_targets_are_pinned() {
        use crate::prompt::PromptAction::*;
        let (_dir, mut e, a, b) = two_dirty();
        let aid = e.active().id;
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, QuitReviewEach);
        action(&mut e, &ex, ReviewDiscard); // A's first version, now reviewing B
        e.switch_to_index(0); // a delayed command switches focus under B's prompt
        insert(&mut e, "new ");
        action(&mut e, &ex, ReviewDiscard); // must discard B, not A's new edit
        assert!(!e.quit);
        assert_eq!(e.active().id, aid);
        assert_eq!(e.quit_drain.as_ref().unwrap().reviewing, Some((aid, e.active().document.version)));
        action(&mut e, &ex, ReviewSave);
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert_eq!(std::fs::read_to_string(a).unwrap(), "new edited A");
        assert_eq!(std::fs::read_to_string(b).unwrap(), "B");
    }

    #[test]
    fn stale_review_save_requests_a_fresh_decision() {
        use crate::prompt::PromptAction::*;
        let (_dir, mut e, _a, _b) = two_dirty();
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, QuitReviewEach);
        insert(&mut e, "changed while reviewing ");
        action(&mut e, &ex, ReviewSave);
        assert!(ex.jobs.borrow().is_empty());
        assert!(e.prompt.is_some());
        assert_eq!(e.quit_drain.as_ref().unwrap().reviewing,
            Some((e.active().id, e.active().document.version)));
    }

    #[test]
    fn cancellation_and_timeout_do_not_quit_on_late_save_completion() {
        for timeout in [false, true] {
            let (_dir, mut e, a, b) = two_dirty();
            let ex = DeferredExecutor::default();
            command(&mut e, &ex, "save_and_quit");
            if timeout { crate::timers::save_timeout_tick(&mut e, 6000); }
            else { action(&mut e, &ex, crate::prompt::PromptAction::Cancel); }
            assert!(e.quit_drain.is_none());
            assert!(e.pending_after_save.is_none());
            ex.complete_next(&mut e); // the already-authorized write still completes
            assert!(!e.quit);
            assert!(ex.jobs.borrow().is_empty());
            assert_eq!(std::fs::read_to_string(a).unwrap(), "edited A");
            assert_eq!(std::fs::read_to_string(b).unwrap(), "B");
        }
    }

    #[test]
    fn overlapping_quit_requests_do_not_replace_pending_work() {
        let (_dir, mut e, _a, _b) = two_dirty();
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        let pending = e.pending_after_save.clone();
        command(&mut e, &ex, "save_and_quit");
        assert_eq!(ex.jobs.borrow().len(), 1);
        assert_eq!(e.pending_after_save, pending);
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
    }

    #[test]
    fn conflict_cancels_quit_without_stranding_the_drain() {
        let (_dir, mut e, a, _b) = two_dirty();
        std::fs::write(a, "external edit").unwrap();
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert!(ex.jobs.borrow().is_empty());
        assert!(e.prompt.is_some());
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        action(&mut e, &ex, crate::prompt::PromptAction::Overwrite);
        ex.complete_next(&mut e);
        assert!(!e.quit, "resolving the conflict does not resurrect a cancelled quit");
    }

    #[test]
    fn failed_save_of_previously_clean_version_cannot_authorize_quit() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        std::fs::write(&a, "A").unwrap();
        let mut e = Editor::new_from_text("A", Some(a.clone()), (80, 24));
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs = std::sync::Arc::new(crate::test_support::FaultFs::new(crate::test_support::FaultAt::Rename));
        crate::save::dispatch_save_and_quit(&mut Ctx {
            editor: &mut e, executor: &ex, clock: &TestClock(0), msg_tx: tx, fs,
        });
        std::fs::write(&a, "external edit after dispatch").unwrap();
        ex.complete_next(&mut e);
        assert!(!e.quit, "an old saved_version must not turn this failed save into success");
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        assert_eq!(std::fs::read_to_string(a).unwrap(), "external edit after dispatch");
    }

    #[test]
    fn save_and_quit_preserves_clean_unnamed_save_as_and_cancel() {
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert!(e.file_browser.as_ref().is_some_and(|fb| fb.mode.is_destination()));
        assert!(e.pending_save_as.is_some());
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::test_support::press_key_fb(&mut e, &test_fs(), &tx, crossterm::event::KeyCode::Esc);
        assert!(e.file_browser.is_none());
        assert!(e.pending_save_as.is_none());
        assert!(e.quit_drain.is_none());
        assert!(!e.quit);
    }

    #[test]
    fn sequential_save_as_and_same_path_stale_saves_remain_valid() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        let c = dir.path().join("c.md");
        std::fs::write(&a, "A").unwrap();
        let mut e = Editor::new_from_text("A", Some(a.clone()), (80, 24));
        let ex = DeferredExecutor::default();
        insert(&mut e, "one ");
        command(&mut e, &ex, "save");
        let v = e.active().document.version;
        insert(&mut e, "two ");
        ex.complete_next(&mut e);
        assert_eq!(e.active().document.saved_version, Some(v));
        assert!(e.active().document.dirty());
        assert_eq!(std::fs::read_to_string(a).unwrap(), "one A");
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, b.clone(), b.clone(), &ex, &TestClock(2), &tx, &test_fs());
        insert(&mut e, "three ");
        crate::prompts::perform_save_as(&mut e, c.clone(), c.clone(), &ex, &TestClock(3), &tx, &test_fs());
        ex.complete_next(&mut e);
        ex.complete_next(&mut e);
        assert_eq!(e.active().document.path.as_deref(), Some(c.as_path()));
        assert!(!e.active().document.dirty());
        assert_eq!(std::fs::read_to_string(b).unwrap(), "two one A");
        assert_eq!(std::fs::read_to_string(c).unwrap(), "three two one A");
    }

    #[test]
    fn new_document_opened_during_quit_is_saved_too() {
        let (_dir, mut e, a, b) = two_dirty();
        let c = a.parent().unwrap().join("c.md");
        std::fs::write(&c, "C").unwrap();
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        crate::workspace::open_as_new_buffer(&mut e, &*test_fs(), &c);
        insert(&mut e, "edited ");
        for _ in 0..3 { ex.complete_next(&mut e); }
        assert!(e.quit);
        assert!(ex.jobs.borrow().is_empty());
        assert_eq!(std::fs::read_to_string(a).unwrap(), "edited A");
        assert_eq!(std::fs::read_to_string(b).unwrap(), "edited B");
        assert_eq!(std::fs::read_to_string(c).unwrap(), "edited C");
    }


    #[test]
    fn wrong_destination_completion_cancels_close_even_if_version_was_saved() {
        let (_dir, mut e, a, _b) = two_dirty();
        let target = a.parent().unwrap().join("new-a.md");
        let id = e.active().id;
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, target.clone(), target.clone(),
            &ex, &TestClock(0), &tx, &test_fs());
        crate::save::dispatch_save_then(&mut Ctx { editor: &mut e, executor: &ex,
            clock: &TestClock(1), msg_tx: tx, fs: test_fs() },
            crate::editor::PostSaveAction::CloseBuffer { id });
        ex.complete_next(&mut e);
        assert!(!e.active().document.dirty());
        ex.complete_next(&mut e);
        assert!(e.by_id(id).is_some(), "the write to old A did not authorize closing new A");
        assert_eq!(e.by_id(id).unwrap().document.path.as_deref(), Some(target.as_path()));
        assert!(e.pending_after_save.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn rejected_save_does_not_arm_an_action_or_strand_quit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.md");
        std::os::unix::fs::symlink(dir.path().join("absent.md"), &path).unwrap();
        let mut e = Editor::new_from_text("text", Some(path), (80, 24));
        insert(&mut e, "edited ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert!(ex.jobs.borrow().is_empty());
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        assert!(!e.quit);
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
    }

    #[test]
    fn save_all_quit_includes_buffers_dirtied_after_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();
        let mut e = Editor::new_from_text("A", Some(a), (80, 24));
        insert(&mut e, "edited ");
        let bid = e.alloc_id();
        e.buffers.push(Buffer::from_text(bid, "B", Some(b.clone()), (80, 24)));
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(crate::prompt::PromptAction::QuitSaveAll,
            &mut e, &ex, &TestClock(0), &tx, &test_fs());
        e.switch_to_index(1);
        insert(&mut e, "edited ");
        let expected = e.active().document.buffer.to_string();
        ex.complete_next(&mut e);
        assert!(!e.quit, "newly dirty B has not been saved or reviewed");
        assert_eq!(ex.jobs.borrow().len(), 1, "Save All must pick up B's new edit");
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert_eq!(std::fs::read_to_string(&b).unwrap(), expected);
    }

    #[test]
    fn own_edits_during_quit_are_resaved_before_exit() {
        let (_dir, mut e, a, _b) = two_dirty();
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        insert(&mut e, "new ");
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert!(e.active().document.dirty());
        ex.complete_next(&mut e); // resave A's latest version
        ex.complete_next(&mut e); // then B
        assert!(e.quit);
        assert_eq!(std::fs::read_to_string(a).unwrap(), "new edited A");
    }

    #[test]
    fn panic_cancels_only_its_own_save_request_in_both_executors() {
        for threaded in [false, true] {
            for own_request in [false, true] {
                let (_dir, mut e, _a, _b) = two_dirty();
                let queued = DeferredExecutor::default();
                if !own_request { command(&mut e, &queued, "save"); }
                command(&mut e, &queued, "save_and_quit");
                let pending = e.pending_after_save.clone();
                let mut job = queued.jobs.borrow_mut().pop_front().unwrap();
                job.run = Box::new(|| panic!("save probe"));
                let (wake_tx, wake_rx) = std::sync::mpsc::channel();
                let executor: Box<dyn Executor> = if threaded {
                    Box::new(crate::jobs::ThreadExecutor::new(wake_tx))
                } else { Box::new(crate::jobs::InlineExecutor::default()) };
                executor.dispatch(job);
                if threaded { wake_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(); }
                let outcomes = executor.drain();
                assert_eq!(outcomes.len(), 1);
                let (tx, _rx) = std::sync::mpsc::channel();
                for outcome in outcomes {
                    crate::jobs_apply::apply_job_outcome(outcome, &mut e, &queued,
                        &TestClock(1), &tx, &test_fs());
                }
                assert!(!e.quit);
                if own_request {
                    assert!(e.pending_after_save.is_none());
                    assert!(e.quit_drain.is_none());
                } else {
                    assert_eq!(e.pending_after_save, pending, "a foreign panic cannot cancel the awaited save");
                    assert!(e.quit_drain.is_some());
                    // The same executor must survive and complete the awaited request.
                    executor.dispatch(queued.jobs.borrow_mut().pop_front().unwrap());
                    if threaded { wake_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(); }
                    for outcome in executor.drain() {
                        crate::jobs_apply::apply_job_outcome(outcome, &mut e, &queued,
                            &TestClock(2), &tx, &test_fs());
                    }
                    queued.complete_next(&mut e); // remaining dirty B
                    assert!(e.quit);
                }
            }
        }
    }

    fn destination(e: &mut Editor, path: &std::path::Path) {
        let crate::file_browser::BrowseMode::Destination { field, field_cursor, .. } =
            &mut e.file_browser.as_mut().unwrap().mode else { panic!("destination picker required"); };
        *field = path.to_string_lossy().into_owned();
        *field_cursor = field.len();
    }

    fn submit_destination(e: &mut Editor, ex: &dyn Executor) {
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::file_browser_commit::commit_destination_with_probe(e, &test_fs(), ex,
            &TestClock(1), &tx, || true);
    }

    #[test]
    fn manual_save_as_is_blocked_at_open_and_write_while_quitting() {
        let (_dir, mut e, a, _b) = two_dirty();
        let target = a.parent().unwrap().join("manual.md");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        let awaited = e.pending_after_save.clone();
        command(&mut e, &ex, "save_as");
        assert!(e.file_browser.is_none());
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, target.clone(), target.clone(), &ex,
            &TestClock(1), &tx, &test_fs()); // also represents an old overwrite prompt
        assert_eq!(ex.jobs.borrow().len(), 1);
        assert_eq!(e.pending_after_save, awaited);
        assert!(!target.exists());
        ex.complete_next(&mut e);
        ex.complete_next(&mut e);
        assert!(e.quit);
    }

    #[test]
    fn old_manual_picker_cannot_submit_and_esc_cancels_awaited_quit() {
        let (_dir, mut e, a, _b) = two_dirty();
        let target = a.parent().unwrap().join("manual.md");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_as");
        destination(&mut e, &target);
        command(&mut e, &ex, "save_and_quit");
        submit_destination(&mut e, &ex);
        assert_eq!(ex.jobs.borrow().len(), 1);
        assert!(e.file_browser.is_some());
        assert!(!target.exists());
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::test_support::press_key_fb(&mut e, &test_fs(), &tx, crossterm::event::KeyCode::Esc);
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        ex.complete_next(&mut e);
        assert!(!e.quit);
    }

    #[test]
    fn quit_owned_filename_can_save_and_empty_cancel_allows_manual_retry() {
        for cancel_empty in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("named.md");
            let mut e = Editor::new_from_text("body", None, (80, 24));
            let ex = DeferredExecutor::default();
            command(&mut e, &ex, "save_and_quit");
            if cancel_empty {
                submit_destination(&mut e, &ex);
                assert!(e.quit_drain.is_none());
                assert!(e.pending_save_as.is_none());
            }
            destination(&mut e, &path);
            submit_destination(&mut e, &ex);
            ex.complete_next(&mut e);
            assert_eq!(std::fs::read_to_string(path).unwrap(), "body");
            assert_eq!(e.quit, !cancel_empty);
        }
    }

    #[test]
    fn quit_owned_export_redirect_cancels_the_quit() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Editor::new_from_text("body", None, (80, 24));
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        destination(&mut e, &dir.path().join("export.pdf"));
        submit_destination(&mut e, &ex);
        assert!(e.quit_drain.is_none());
        assert!(e.pending_save_as.is_none());
        assert!(matches!(&e.file_browser.as_ref().unwrap().mode,
            crate::file_browser::BrowseMode::Destination {
                purpose: crate::file_browser::DestinationPurpose::Export { .. }, .. }));
        assert!(ex.jobs.borrow().is_empty());
        assert!(!e.quit);
    }

    #[test]
    fn quit_waits_for_existing_save_as_merges_and_session_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        let c = dir.path().join("c.md");
        std::fs::write(&a, "body").unwrap();
        let mut e = Editor::new_from_text("body", Some(a.clone()), (80, 24));
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, b.clone(), b.clone(), &ex, &TestClock(0), &tx, &test_fs());
        crate::prompts::perform_save_as(&mut e, c.clone(), c.clone(), &ex, &TestClock(0), &tx, &test_fs());
        command(&mut e, &ex, "quit");
        assert!(!e.quit);
        ex.complete_next(&mut e);
        assert!(!e.quit, "the second save is still pending");
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert_eq!(e.active().document.path.as_deref(), Some(c.as_path()));
        assert_eq!(std::fs::read_to_string(c.clone()).unwrap(), "body");
        let migrations: Vec<_> = e.pending_session_migrations.iter()
            .map(|m| (m.from.clone(), m.to.clone())).collect();
        assert_eq!(migrations, vec![(a, b.clone()), (b, c)]);
        assert!(e.saves_in_flight.is_empty());
    }

    #[test]
    fn quit_waiting_for_saves_times_out_or_cancels_on_failure() {
        for timeout in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let a = dir.path().join("a.md");
            let bad = dir.path().join("missing/b.md");
            std::fs::write(&a, "body").unwrap();
            let mut e = Editor::new_from_text("body", Some(a), (80, 24));
            let ex = DeferredExecutor::default();
            let (tx, _rx) = std::sync::mpsc::channel();
            crate::prompts::perform_save_as(&mut e, bad.clone(), bad, &ex, &TestClock(0), &tx, &test_fs());
            command(&mut e, &ex, "quit");
            assert!(!e.quit);
            if timeout {
                crate::timers::save_timeout_tick(&mut e, 6001);
                assert!(e.quit_drain.is_none());
            }
            ex.complete_next(&mut e);
            assert!(!e.quit);
            assert!(e.quit_drain.is_none());
            assert!(e.saves_in_flight.is_empty());
            if !timeout { assert!(e.status_text().contains("quit cancelled")); }
        }
    }

    #[test]
    fn edits_during_plain_quit_wait_require_a_new_decision() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        std::fs::write(&a, "body").unwrap();
        let mut e = Editor::new_from_text("body", Some(a), (80, 24));
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save");
        command(&mut e, &ex, "quit");
        insert(&mut e, "new ");
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert!(e.prompt.is_some());
        assert!(e.active().document.dirty());
        assert!(ex.jobs.borrow().is_empty(), "plain Quit did not authorize saving these new edits");
    }

    #[test]
    fn summary_can_restart_a_quit_whose_filename_picker_it_replaced() {
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "new ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert!(e.file_browser.is_some());
        command(&mut e, &ex, "quit");
        assert!(e.prompt.is_some());
        action(&mut e, &ex, crate::prompt::PromptAction::QuitSaveAll);
        assert!(e.file_browser.is_some());
        assert!(e.pending_save_as.is_some());
        assert!(e.prompt.is_none());
        action(&mut e, &ex, crate::prompt::PromptAction::Cancel);
        assert!(!e.quit);
    }

    #[test]
    fn save_as_before_quit_retains_the_conservative_warning() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "body").unwrap();
        let mut e = Editor::new_from_text("body", Some(a), (80, 24));
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, b.clone(), b.clone(), &ex, &TestClock(0), &tx, &test_fs());
        command(&mut e, &ex, "save_and_quit");
        ex.complete_next(&mut e);
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        assert!(e.saves_in_flight.is_empty());
        assert!(!e.active().document.dirty());
        assert_eq!(e.active().document.path.as_deref(), Some(b.as_path()));
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
    }

    #[test]
    fn destination_cancel_preserves_an_unrelated_pending_close() {
        let (_dir, mut e, _a, _b) = two_dirty();
        let id = e.active().id;
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, crate::prompt::PromptAction::CloseSave { id });
        let pending = e.pending_after_save.clone();
        command(&mut e, &ex, "save_as");
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::test_support::press_key_fb(&mut e, &test_fs(), &tx, crossterm::event::KeyCode::Esc);
        assert_eq!(e.pending_after_save, pending);
        ex.complete_next(&mut e);
        assert!(e.by_id(id).is_none());
        assert!(!e.quit);
    }

    #[test]
    fn quit_filename_and_overwrite_confirmation_reject_changed_buffer() {
        for overwrite in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("target.md");
            if overwrite { std::fs::write(&path, "keep").unwrap(); }
            let mut e = Editor::new_from_text("body", None, (80, 24));
            let ex = DeferredExecutor::default();
            command(&mut e, &ex, "save_and_quit");
            destination(&mut e, &path);
            if overwrite {
                submit_destination(&mut e, &ex);
                assert!(e.prompt.is_some());
            }
            crate::workspace::new_empty_buffer(&mut e);
            insert(&mut e, "different document");
            if overwrite { action(&mut e, &ex, crate::prompt::PromptAction::OverwriteSaveAs); }
            else { submit_destination(&mut e, &ex); }
            assert!(ex.jobs.borrow().is_empty());
            assert!(e.pending_save_as.is_none());
            assert!(e.quit_drain.is_none());
            assert!(!e.quit);
            if overwrite { assert_eq!(std::fs::read_to_string(path).unwrap(), "keep"); }
            else { assert!(!path.exists()); }
        }
    }

    fn finish_iteration(e: &mut Editor, ex: &dyn Executor) -> bool {
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::finish_iteration(e, ex, &TestClock(20), &tx, &test_fs())
    }

    #[test]
    fn final_exit_preserves_discard_decisions_but_rechecks_late_edits() {
        use crate::prompt::PromptAction::*;
        for late_edit in [false, true] {
            let (_dir, mut e, a, b) = two_dirty();
            let ex = DeferredExecutor::default();
            action(&mut e, &ex, QuitReviewEach);
            action(&mut e, &ex, ReviewDiscard);
            action(&mut e, &ex, ReviewDiscard);
            assert!(e.quit);
            if late_edit { insert(&mut e, "late "); }
            assert_eq!(finish_iteration(&mut e, &ex), late_edit);
            if late_edit {
                assert!(!e.quit);
                assert_eq!(e.active().document.path.as_deref(), Some(b.as_path()));
                action(&mut e, &ex, ReviewDiscard); // only B's new version needs a decision
                assert!(!finish_iteration(&mut e, &ex));
            }
            assert!(e.quit);
            assert!(e.quit_drain.is_none());
            assert_eq!(std::fs::read_to_string(a).unwrap(), "A");
            assert_eq!(std::fs::read_to_string(b).unwrap(), "B");
        }
    }

    #[test]
    fn final_exit_honors_cancellation_after_the_last_save_merge() {
        let (_dir, mut e, _a, _b) = two_dirty();
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        ex.complete_next(&mut e);
        ex.complete_next(&mut e);
        assert!(e.quit);
        action(&mut e, &ex, crate::prompt::PromptAction::Cancel);
        assert!(finish_iteration(&mut e, &ex));
        assert!(!e.quit);
    }

    #[test]
    fn replacing_quit_owned_picker_with_palette_cancels_its_ownership() {
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "new ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert!(e.file_browser.is_some());
        e.open_palette();
        assert!(e.file_browser.is_none());
        assert!(e.pending_save_as.is_none(), "closing through the overlay registry must cancel ownership");
        assert!(e.quit_drain.is_none());
        assert!(!e.quit);
        command(&mut e, &ex, "save_as");
        assert!(e.file_browser.is_some(), "manual saving must not remain blocked");
    }

    #[test]
    fn changed_destination_explains_cancellation_of_a_plain_quit_wait() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "body").unwrap();
        let mut e = Editor::new_from_text("body", Some(a), (80, 24));
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, b.clone(), b.clone(), &ex, &TestClock(0), &tx, &test_fs());
        command(&mut e, &ex, "save");
        command(&mut e, &ex, "quit");
        ex.complete_next(&mut e);
        ex.complete_next(&mut e);
        assert!(!e.quit);
        assert!(e.status_text().contains("quit cancelled"));
        assert!(e.status_text().contains("current document was not saved"));
        assert_eq!(e.active().document.path.as_deref(), Some(b.as_path()));
    }

    #[test]
    fn mouse_click_away_cancels_a_quit_owned_filename_request() {
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "new ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        crate::derive::rebuild(&mut e);
        let reg = Registry::builtins();
        let (keys, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mouse = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 0, row: 0, modifiers: crossterm::event::KeyModifiers::NONE,
        };
        crate::app::reduce(crate::app::Msg::Input(crossterm::event::Event::Mouse(mouse)),
            &mut e, &reg, &keys, &ex, &TestClock(1), &tx, &test_fs());
        assert!(e.file_browser.is_none());
        assert!(e.pending_save_as.is_none(), "mouse cancellation must not orphan the filename request");
        assert!(e.quit_drain.is_none());
        assert!(!e.quit);
        command(&mut e, &ex, "save_as");
        assert!(e.file_browser.is_some(), "ordinary saving remains available after cancellation");
    }
}
