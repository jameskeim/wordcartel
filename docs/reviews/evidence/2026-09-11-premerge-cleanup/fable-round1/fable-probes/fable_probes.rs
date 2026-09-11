//! Fable review probes for the R8/R12 pre-merge cleanup snapshot.
//! Compiled against the real branch via a temporary `#[path]` registration in lib.rs.
//! Each probe resolves one concrete hypothesis from the review; the assertions record
//! OBSERVED behavior (some characterize limitations rather than assert a fix).

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use crate::editor::{Buffer, Editor};
    use crate::jobs::{Executor, Job, JobOutcome};
    use crate::registry::{CommandId, Ctx, Registry};
    use crate::test_support::{test_fs, TestClock};
    use crate::prompt::PromptAction;

    #[derive(Default)]
    struct DeferredExecutor { jobs: RefCell<VecDeque<Job>> }
    impl Executor for DeferredExecutor {
        fn dispatch(&self, job: Job) { self.jobs.borrow_mut().push_back(job); }
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
    fn action(editor: &mut Editor, ex: &dyn Executor, a: PromptAction) {
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(a, editor, ex, &TestClock(2), &tx, &test_fs());
    }
    fn finish(e: &mut Editor, ex: &dyn Executor) -> bool {
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::finish_iteration(e, ex, &TestClock(20), &tx, &test_fs())
    }
    fn destination(e: &mut Editor, path: &std::path::Path) {
        let crate::file_browser::BrowseMode::Destination { field, field_cursor, .. } =
            &mut e.file_browser.as_mut().unwrap().mode else { panic!("destination picker required"); };
        *field = path.to_string_lossy().into_owned();
        *field_cursor = field.len();
    }
    fn submit(e: &mut Editor, ex: &dyn Executor) {
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::file_browser_commit::commit_destination_with_probe(e, &test_fs(), ex, &TestClock(1), &tx, || true);
    }
    fn named_clean(dir: &std::path::Path, name: &str, body: &str) -> (std::path::PathBuf, Editor) {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        let e = Editor::new_from_text(body, Some(p.clone()), (80, 24));
        (p, e)
    }

    // A. Plain Quit sets a PROVISIONAL quit; a late (callback-style) edit must convert the
    //    exit into a review prompt, and saving from that prompt must then exit.
    #[test]
    fn a_plain_quit_provisional_then_late_edit_asks_for_review() {
        let dir = tempfile::tempdir().unwrap();
        let (p, mut e) = named_clean(dir.path(), "a.md", "body");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "quit");
        assert!(e.quit && e.quit_drain.is_none(), "clean workspace: provisional exit, no drain");
        insert(&mut e, "late ");
        assert!(finish(&mut e, &ex), "barrier must keep running after a late edit");
        assert!(!e.quit);
        assert!(e.prompt.is_some(), "a review prompt is raised for the late edit");
        assert_eq!(e.quit_drain.as_ref().unwrap().mode, crate::editor::QuitMode::ReviewEach);
        action(&mut e, &ex, PromptAction::ReviewSave);
        assert_eq!(ex.jobs.borrow().len(), 1);
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert!(!finish(&mut e, &ex));
        assert_eq!(std::fs::read_to_string(p).unwrap(), "late body");
        assert!(e.quit_drain.is_none() && e.saves_in_flight.is_empty());
    }

    // B. Review Each on an UNNAMED dirty buffer: ReviewSave must open a quit-owned picker,
    //    and the picker's submission must be accepted and lead to exit.
    #[test]
    fn b_review_each_unnamed_buffer_save_uses_quit_owned_picker() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("named.md");
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "x ");
        let id = e.active().id;
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, PromptAction::QuitReviewEach);
        assert!(e.prompt.is_some());
        action(&mut e, &ex, PromptAction::ReviewSave);
        let fb = e.file_browser.as_ref().expect("quit-owned picker opened");
        assert_eq!(fb.quit_save_owner, Some(id));
        assert_eq!(e.pending_save_as, Some(crate::editor::PostSaveAction::ContinueQuitDrain));
        destination(&mut e, &target);
        submit(&mut e, &ex);
        assert_eq!(ex.jobs.borrow().len(), 1, "quit-owned Save As dispatched");
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert!(!finish(&mut e, &ex));
        assert_eq!(std::fs::read_to_string(target).unwrap(), "x body");
    }

    // C. Save All where the SECOND buffer is unnamed: the drain must switch to it, open a
    //    picker owned by it, accept the name, and exit after the merge.
    #[test]
    fn c_save_all_second_unnamed_buffer_gets_quit_owned_picker_then_exits() {
        let dir = tempfile::tempdir().unwrap();
        let (a, mut e) = named_clean(dir.path(), "a.md", "A");
        insert(&mut e, "edited ");
        let bid = e.alloc_id();
        e.buffers.push(Buffer::from_text(bid, "B", None, (80, 24)));
        e.switch_to_index(1);
        insert(&mut e, "edited ");
        e.switch_to_index(0);
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        ex.complete_next(&mut e); // A saved; drain moves on to B (unnamed)
        assert!(!e.quit);
        assert_eq!(e.active().id, bid, "drain switched to the unnamed buffer");
        assert_eq!(e.file_browser.as_ref().and_then(|f| f.quit_save_owner), Some(bid));
        let b = dir.path().join("b.md");
        destination(&mut e, &b);
        submit(&mut e, &ex);
        assert_eq!(ex.jobs.borrow().len(), 1);
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert!(!finish(&mut e, &ex));
        assert_eq!(std::fs::read_to_string(a).unwrap(), "edited A");
        assert_eq!(std::fs::read_to_string(b).unwrap(), "edited B");
        assert!(e.saves_in_flight.is_empty());
    }

    // D. Quit-owned Save As onto an EXISTING file: the overwrite confirm outlives the picker
    //    (close_all nulls it); [O]verwrite must still be accepted as quit-owned and exit.
    #[test]
    fn d_quit_owned_overwrite_confirm_accepts_and_exits() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.md");
        std::fs::write(&target, "old").unwrap();
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "x ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        destination(&mut e, &target);
        submit(&mut e, &ex);
        assert!(e.file_browser.is_none() && e.prompt.is_some(), "overwrite confirm replaced the picker");
        assert_eq!(e.pending_save_as, Some(crate::editor::PostSaveAction::ContinueQuitDrain));
        action(&mut e, &ex, PromptAction::OverwriteSaveAs);
        assert_eq!(ex.jobs.borrow().len(), 1, "quit-owned overwrite accepted");
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert!(!finish(&mut e, &ex));
        assert_eq!(std::fs::read_to_string(target).unwrap(), "x body");
    }

    // E. CHARACTERIZATION: the overlay table's file-browser `close` row nulls the picker without
    //    `cancel_destination`. If something calls `close_all` (open_prompt/open_palette/plugin
    //    dispatch under an overlay) while a quit-owned picker is up, the quit ownership is
    //    orphaned: manual Save is refused as "unavailable while quitting" until the user goes
    //    through Quit's summary (Cancel) or a prompt Esc.
    #[test]
    fn e_orphaned_quit_owned_picker_after_close_all_blocks_manual_save_until_summary() {
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "x ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert!(e.file_browser.is_some());
        crate::overlays::close_all(&mut e); // what open_palette / open_prompt / plugin dispatch do
        assert!(e.file_browser.is_none());
        assert_eq!(e.pending_save_as, Some(crate::editor::PostSaveAction::ContinueQuitDrain), "ownership orphaned");
        assert!(e.quit_drain.is_some());
        command(&mut e, &ex, "save");
        assert!(e.file_browser.is_none(), "manual Save cannot open a picker");
        assert_eq!(e.status_text(), "Save As is unavailable while quitting");
        command(&mut e, &ex, "save_and_quit");
        assert_eq!(e.status_text(), "another save or quit is in progress — try again");
        // Recovery path: Quit → summary → Cancel clears the orphaned state.
        command(&mut e, &ex, "quit");
        assert!(e.prompt.is_some());
        action(&mut e, &ex, PromptAction::Cancel);
        assert!(e.pending_save_as.is_none() && e.quit_drain.is_none());
        command(&mut e, &ex, "save");
        assert!(e.file_browser.is_some(), "manual Save works again after the summary's Cancel");
        assert!(ex.jobs.borrow().is_empty());
    }

    // F. CHARACTERIZATION (pre-existing): a manual save whose write already LANDED on disk but
    //    whose merge has not run yet makes an immediate Save and Quit see a fingerprint mismatch
    //    and raise the external-mod modal, cancelling the quit. Not introduced by this branch
    //    (the fingerprint check in dispatch_save_reporting predates it).
    #[test]
    fn f_preexisting_landed_but_unmerged_manual_save_makes_save_and_quit_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let (_p, mut e) = named_clean(dir.path(), "a.md", "body");
        insert(&mut e, "x ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save");
        let job = ex.jobs.borrow_mut().pop_front().unwrap();
        let outcome = job.execute(); // bytes are on disk; merge not yet applied
        command(&mut e, &ex, "save_and_quit");
        assert!(e.prompt.is_some(), "external-mod modal raised by the stale stored_fp");
        assert!(e.quit_drain.is_none() && e.pending_after_save.is_none(), "quit cancelled cleanly");
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::jobs_apply::apply_job_outcome(outcome, &mut e, &ex, &TestClock(3), &tx, &test_fs());
        assert!(!e.active().document.dirty());
        assert!(!e.quit);
    }

    // G. The waiting phase must arm the timed-subsystem wake (no silent hang) and disarm it
    //    after cancellation (idle stays free).
    #[test]
    fn g_waiting_quit_arms_timeout_wake_and_disarms_after_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let (_p, mut e) = named_clean(dir.path(), "a.md", "body");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save"); // clean doc: a save is still queued, workspace stays clean
        assert!(crate::timers::next_wake(&e, 0).is_none(), "a plain in-flight save arms no wake");
        let (tx, _rx) = std::sync::mpsc::channel();
        Registry::builtins().dispatch(CommandId("quit"), &mut Ctx {
            editor: &mut e, executor: &ex, clock: &TestClock(100), msg_tx: tx, fs: test_fs() });
        assert!(!e.quit);
        assert_eq!(crate::timers::next_wake(&e, 100), Some(100 + crate::timers::SAVE_QUIT_TIMEOUT_MS + 1));
        crate::timers::pre_recv(&mut e, 100 + crate::timers::SAVE_QUIT_TIMEOUT_MS);
        assert!(e.quit_drain.is_some(), "not yet overdue");
        crate::timers::pre_recv(&mut e, 100 + crate::timers::SAVE_QUIT_TIMEOUT_MS + 1);
        assert!(e.quit_drain.is_none(), "overdue: quit cancelled");
        assert!(crate::timers::next_wake(&e, 6000).is_none(), "no wake armed after cancel");
        ex.complete_next(&mut e);
        assert!(!e.quit && e.saves_in_flight.is_empty());
    }

    // H. Save and Quit on a CLEAN named document preserves the explicit-save step and exits.
    #[test]
    fn h_save_and_quit_on_clean_named_doc_writes_and_exits() {
        let dir = tempfile::tempdir().unwrap();
        let (_p, mut e) = named_clean(dir.path(), "a.md", "body");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert_eq!(ex.jobs.borrow().len(), 1, "explicit save still dispatched for a clean doc");
        assert!(e.pending_after_save.is_some());
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert!(!finish(&mut e, &ex));
    }

    // I. Repeating Quit while already waiting for merges is idempotent (no panic, single drain).
    #[test]
    fn i_repeat_quit_while_waiting_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let (_p, mut e) = named_clean(dir.path(), "a.md", "body");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save");
        command(&mut e, &ex, "quit");
        let since = e.quit_drain.as_ref().unwrap().waiting_since;
        command(&mut e, &ex, "quit");
        command(&mut e, &ex, "quit");
        assert_eq!(e.quit_drain.as_ref().unwrap().waiting_since, since, "first wait origin retained");
        assert!(!e.quit);
        ex.complete_next(&mut e);
        assert!(e.quit);
    }

    // J. Review Each: after discarding both, a buffer opened+dirtied before the barrier is
    //    rescanned and asked about; the earlier discards are not re-asked.
    #[test]
    fn j_review_discard_then_new_buffer_dirtied_is_rescanned_once() {
        let dir = tempfile::tempdir().unwrap();
        let (a, mut e) = named_clean(dir.path(), "a.md", "A");
        insert(&mut e, "edited ");
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, PromptAction::QuitReviewEach);
        action(&mut e, &ex, PromptAction::ReviewDiscard);
        assert!(e.quit, "provisional exit after discarding the only dirty buffer");
        let c = dir.path().join("c.md");
        std::fs::write(&c, "C").unwrap();
        crate::workspace::open_as_new_buffer(&mut e, &*test_fs(), &c);
        insert(&mut e, "edited ");
        let cid = e.active().id;
        assert!(finish(&mut e, &ex), "barrier catches the new dirty buffer");
        assert!(e.prompt.is_some());
        assert_eq!(e.quit_drain.as_ref().unwrap().reviewing.map(|r| r.0), Some(cid), "asks about C, not A again");
        action(&mut e, &ex, PromptAction::ReviewSave);
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert!(!finish(&mut e, &ex));
        assert_eq!(std::fs::read_to_string(a).unwrap(), "A", "A's discard honored");
        assert_eq!(std::fs::read_to_string(c).unwrap(), "edited C");
    }

    // K. Save All barrier retains SaveAll mode: a late edit after the last merge triggers a
    //    NEW save (not a review prompt), and exit follows that merge.
    #[test]
    fn k_save_all_barrier_resaves_late_edit_in_save_all_mode() {
        let dir = tempfile::tempdir().unwrap();
        let (a, mut e) = named_clean(dir.path(), "a.md", "A");
        insert(&mut e, "edited ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        ex.complete_next(&mut e);
        assert!(e.quit && e.quit_drain.is_some(), "provisional exit keeps the drain");
        insert(&mut e, "late ");
        assert!(finish(&mut e, &ex));
        assert!(e.prompt.is_none(), "SaveAll mode: no review prompt");
        assert_eq!(ex.jobs.borrow().len(), 1, "late edit re-saved");
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert!(!finish(&mut e, &ex));
        assert_eq!(std::fs::read_to_string(a).unwrap(), "late edited A");
    }

    // L. CHARACTERIZATION: a changed-destination completion during a plain-Quit WAIT (clean
    //    workspace, queued Save As + old-path save) cancels the quit even though the current
    //    path is already clean; the only user-visible text is the destination warning.
    #[test]
    fn l_changed_destination_during_plain_quit_wait_cancels_quit_with_only_the_warning() {
        let dir = tempfile::tempdir().unwrap();
        let (a, mut e) = named_clean(dir.path(), "a.md", "body");
        let b = dir.path().join("b.md");
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, b.clone(), b.clone(), &ex, &TestClock(0), &tx, &test_fs());
        command(&mut e, &ex, "save"); // Normal save captured with path A (pre-rekey), same version
        command(&mut e, &ex, "quit");
        assert!(e.prompt.is_none() && !e.quit, "clean workspace → wait phase");
        assert!(e.quit_drain.as_ref().unwrap().waiting_since.is_some());
        ex.complete_next(&mut e); // SaveAs → path B, clean
        assert!(!e.quit && e.quit_drain.is_some(), "still waiting for the old-path save");
        ex.complete_next(&mut e); // old-path normal save → changed_destination
        assert!(!e.quit);
        assert!(e.quit_drain.is_none(), "wait cancelled by the foreign changed-destination completion");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert!(e.status_text().contains("current document was not saved"), "{}", e.status_text());
        assert!(!e.active().document.dirty(), "B is clean: the cancellation was conservative");
        assert_eq!(e.active().document.path.as_deref(), Some(b.as_path()));
        assert_eq!(std::fs::read_to_string(a).unwrap(), "body");
        assert!(e.saves_in_flight.is_empty());
    }

    // L2. Same completion while a Save All AWAITS ITS OWN later request (dispatched after the
    //     rekey): the foreign old-path warning must NOT cancel the awaited save; exit follows.
    #[test]
    fn l2_foreign_changed_destination_does_not_cancel_awaited_later_save() {
        let dir = tempfile::tempdir().unwrap();
        let (_a, mut e) = named_clean(dir.path(), "a.md", "body");
        let b = dir.path().join("b.md");
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, b.clone(), b.clone(), &ex, &TestClock(0), &tx, &test_fs());
        insert(&mut e, "x ");
        command(&mut e, &ex, "save"); // Normal, chosen A, v2
        ex.complete_next(&mut e); // SaveAs merges → path B, saved v1, still dirty
        command(&mut e, &ex, "quit");
        action(&mut e, &ex, PromptAction::QuitSaveAll); // dispatches R3 to B (v2), awaited
        assert_eq!(ex.jobs.borrow().len(), 2);
        ex.complete_next(&mut e); // R2 old-path → warning; foreign to the awaited R3
        assert!(e.quit_drain.is_some() && e.pending_after_save.is_some(), "awaited save protected");
        assert!(e.status_text().contains("current document was not saved"), "{}", e.status_text());
        ex.complete_next(&mut e); // R3 merges
        assert!(e.quit);
        assert!(!finish(&mut e, &ex));
        assert_eq!(std::fs::read_to_string(b).unwrap(), "x body");
    }

    // M. Real ThreadExecutor end-to-end: FIFO worker, wake-driven drain, both files land,
    //    in-flight accounting empties, barrier exits.
    #[test]
    fn m_thread_executor_save_all_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let (a, mut e) = named_clean(dir.path(), "a.md", "A");
        insert(&mut e, "edited ");
        let bid = e.alloc_id();
        let b = dir.path().join("b.md");
        std::fs::write(&b, "B").unwrap();
        e.buffers.push(Buffer::from_text(bid, "B", Some(b.clone()), (80, 24)));
        e.switch_to_index(1);
        insert(&mut e, "edited ");
        e.switch_to_index(0);
        let (wake_tx, wake_rx) = std::sync::mpsc::channel();
        let ex = crate::jobs::ThreadExecutor::new(wake_tx);
        command(&mut e, &ex, "save_and_quit");
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut rounds = 0;
        while !e.quit {
            wake_rx.recv_timeout(std::time::Duration::from_secs(10)).expect("worker wake");
            for o in ex.drain() {
                crate::jobs_apply::apply_job_outcome(o, &mut e, &ex, &TestClock(1), &tx, &test_fs());
            }
            rounds += 1; assert!(rounds < 16, "did not converge");
        }
        assert!(!finish(&mut e, &ex));
        assert!(e.saves_in_flight.is_empty() && e.pending_after_save.is_none());
        assert_eq!(std::fs::read_to_string(a).unwrap(), "edited A");
        assert_eq!(std::fs::read_to_string(b).unwrap(), "edited B");
    }

    // O. Quit summary raised DURING a Save All wait (Ctrl+Q is reachable then): Cancel from the
    //    summary aborts the drain; already-dispatched writes still land; no stranded state.
    #[test]
    fn o_quit_summary_during_save_all_wait_then_cancel_leaves_no_stranded_state() {
        let dir = tempfile::tempdir().unwrap();
        let (a, mut e) = named_clean(dir.path(), "a.md", "A");
        insert(&mut e, "edited ");
        let bid = e.alloc_id();
        let b = dir.path().join("b.md");
        std::fs::write(&b, "B").unwrap();
        e.buffers.push(Buffer::from_text(bid, "B", Some(b.clone()), (80, 24)));
        e.switch_to_index(1);
        insert(&mut e, "edited ");
        e.switch_to_index(0);
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        command(&mut e, &ex, "quit"); // reachable: no modal is up while a SaveAll save runs
        assert!(e.prompt.is_some(), "summary over the running drain");
        ex.complete_next(&mut e); // A merges under the prompt; drain advances to B
        assert_eq!(ex.jobs.borrow().len(), 1, "B's save dispatched while the summary is up");
        action(&mut e, &ex, PromptAction::Cancel);
        assert!(e.quit_drain.is_none() && e.pending_after_save.is_none());
        ex.complete_next(&mut e);
        assert!(!e.quit && finish(&mut e, &ex));
        assert!(e.saves_in_flight.is_empty());
        assert_eq!(std::fs::read_to_string(a).unwrap(), "edited A");
        assert_eq!(std::fs::read_to_string(b).unwrap(), "edited B");
        assert!(!e.is_dirty(bid));
    }

    // P. Panic in the awaited request under the REAL ThreadExecutor: quit cancelled, the worker
    //    survives, and a subsequent save on the same worker completes.
    #[test]
    fn p_awaited_save_panic_on_thread_executor_cancels_quit_and_worker_survives() {
        let dir = tempfile::tempdir().unwrap();
        let (a, mut e) = named_clean(dir.path(), "a.md", "A");
        insert(&mut e, "edited ");
        let deferred = DeferredExecutor::default();
        command(&mut e, &deferred, "save_and_quit");
        let mut job = deferred.jobs.borrow_mut().pop_front().unwrap();
        job.run = Box::new(|| panic!("probe"));
        let (wake_tx, wake_rx) = std::sync::mpsc::channel();
        let ex = crate::jobs::ThreadExecutor::new(wake_tx);
        ex.dispatch(job);
        wake_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, &mut e, &ex, &TestClock(1), &tx, &test_fs()); }
        assert!(!e.quit && e.quit_drain.is_none() && e.pending_after_save.is_none());
        assert!(e.saves_in_flight.is_empty(), "panicked request removed from in-flight");
        assert!(e.active().document.dirty());
        assert!(e.status_text().contains("save failed"));
        // Same worker: a fresh Save and Quit now succeeds end to end.
        command(&mut e, &ex, "save_and_quit");
        wake_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, &mut e, &ex, &TestClock(2), &tx, &test_fs()); }
        assert!(e.quit);
        assert_eq!(std::fs::read_to_string(a).unwrap(), "edited A");
    }
}
