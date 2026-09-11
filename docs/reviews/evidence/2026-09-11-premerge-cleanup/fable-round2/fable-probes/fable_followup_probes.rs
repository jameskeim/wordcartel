//! Fable follow-up review probes (R8/R12 pre-merge cleanup delta, 2026-09-11).
//! Registered during the review via a temporary `#[path]` line in `wordcartel/src/lib.rs`
//! (removed afterwards). Each probe records OBSERVED behavior; assertions that document a
//! gap are labelled as such in the message text.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use crate::editor::{Editor, PostSaveAction};
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
            let outcome = job.execute();
            let (tx, _rx) = std::sync::mpsc::channel();
            crate::jobs_apply::apply_job_outcome(outcome, editor, self, &TestClock(10), &tx, &test_fs());
        }
        /// Deliver the next job as a worker PANIC instead of running it (identity retained).
        fn panic_next(&self, editor: &mut Editor) {
            let job = self.jobs.borrow_mut().pop_front().expect("a save is pending");
            let outcome = JobOutcome::Panicked { buffer_id: job.buffer_id, version: job.version,
                kind: job.kind, save_request: job.save_request, msg: "boom".into() };
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

    fn action(editor: &mut Editor, ex: &dyn Executor, action: crate::prompt::PromptAction) {
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(action, editor, ex, &TestClock(2), &tx, &test_fs());
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

    /// Drive a REAL mouse event through `mouse::handle` (the production entry), not the slot.
    fn click(e: &mut Editor, ex: &dyn Executor, column: u16, row: u16) {
        use crossterm::event::{MouseEvent, MouseEventKind, MouseButton, KeyModifiers};
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let clock = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs = test_fs();
        let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex, clock: &clock, msg_tx: &tx, fs: &fs };
        let ev = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column, row, modifiers: KeyModifiers::NONE };
        crate::mouse::handle(e, ev, &ctx);
    }

    // ---------------------------------------------------------------------------------
    // M1 fix — positive coverage through a second entry (Review Each -> Save on unnamed).
    // ---------------------------------------------------------------------------------
    #[test]
    fn a_review_each_unnamed_picker_replaced_by_palette_is_cancelled_cleanly() {
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "new ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "quit");
        assert!(e.prompt.is_some(), "summary");
        action(&mut e, &ex, crate::prompt::PromptAction::QuitReviewEach);
        assert!(e.prompt.is_some(), "review prompt");
        action(&mut e, &ex, crate::prompt::PromptAction::ReviewSave);
        assert!(e.file_browser.is_some(), "quit-owned picker for the unnamed buffer");
        assert!(e.file_browser.as_ref().unwrap().quit_save_owner.is_some());
        assert_eq!(e.pending_save_as, Some(PostSaveAction::ContinueQuitDrain));
        e.open_palette();
        assert!(e.file_browser.is_none());
        assert!(e.pending_save_as.is_none());
        assert!(e.quit_drain.is_none());
        assert!(!e.quit);
        // Manual save is available again.
        command(&mut e, &ex, "save_as");
        assert!(e.file_browser.is_some());
        assert!(e.file_browser.as_ref().unwrap().quit_save_owner.is_none());
    }

    // ---------------------------------------------------------------------------------
    // M1 transition validity — a successful quit-owned commit onto an EXISTING file must
    // reach the overwrite prompt with ownership intact, and confirming it must exit.
    // ---------------------------------------------------------------------------------
    #[test]
    fn b_quit_owned_commit_onto_existing_file_reaches_overwrite_and_exits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("target.md");
        std::fs::write(&path, "old").unwrap();
        let mut e = Editor::new_from_text("body", None, (80, 24));
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert!(e.file_browser.is_some());
        destination(&mut e, &path);
        submit_destination(&mut e, &ex);
        // The picker was removed BEFORE the prompt opened, so close_overlay never ran on it.
        assert!(e.file_browser.is_none());
        assert!(e.prompt.is_some(), "overwrite confirmation");
        assert_eq!(e.pending_save_as, Some(PostSaveAction::ContinueQuitDrain), "ownership survives the transition");
        assert!(e.quit_drain.is_some());
        assert_eq!(e.pending_save_overwrite.as_deref(), Some(path.as_path()));
        action(&mut e, &ex, crate::prompt::PromptAction::OverwriteSaveAs);
        assert_eq!(ex.jobs.borrow().len(), 1, "the quit-owned write was dispatched");
        assert!(e.pending_after_save.is_some());
        ex.complete_next(&mut e);
        assert!(e.quit, "the drain completed and exit is authorized");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "body");
    }

    // ---------------------------------------------------------------------------------
    // NEW GAP CANDIDATE — mouse click-away closes a quit-owned picker via
    // `mouse::mouse_file_browser`'s direct `editor.file_browser = None`, bypassing
    // `file_browser::close_overlay`. Same orphan state as prior M1, now reachable from a
    // real mouse click (no plugin or second command needed).
    // ---------------------------------------------------------------------------------
    #[test]
    fn c_mouse_click_away_on_quit_owned_picker_orphans_the_quit() {
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "new ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert!(e.file_browser.is_some());
        assert!(e.mouse_capture, "default: mouse enabled");
        // The overlay rect is centered (x >= 16 on an 80-col area); cell (0,0) is outside.
        let (w, h) = e.active().view.area;
        let area = ratatui::layout::Rect::new(0, 0, w, h);
        let n = e.file_browser.as_ref().unwrap().entries.len();
        let r = crate::chrome_geom::palette_overlay_rect(area, n);
        assert!(r.x > 0 || r.y > 0, "cell (0,0) must be outside the picker rect for this probe");
        click(&mut e, &ex, 0, 0);
        assert!(e.file_browser.is_none(), "click-away closed the picker");
        // Observed (documents the gap): ownership survives with no picker to serve it.
        let orphaned = e.pending_save_as == Some(PostSaveAction::ContinueQuitDrain) && e.quit_drain.is_some();
        assert!(orphaned, "GAP: expected the orphaned state (pending_save_as={:?}, drain={})",
            e.pending_save_as, e.quit_drain.is_some());
        assert!(!e.quit);
        // Consequence: manual Save / Save As are refused; Save and Quit is refused as busy.
        command(&mut e, &ex, "save_as");
        assert!(e.file_browser.is_none(), "manual Save As refused while the orphan persists");
        assert!(e.status_text().contains("Save As is unavailable while quitting"), "{}", e.status_text());
        command(&mut e, &ex, "save_and_quit");
        assert!(e.file_browser.is_none());
        assert!(e.status_text().contains("another save or quit is in progress"), "{}", e.status_text());
        assert!(ex.jobs.borrow().is_empty(), "no write was ever dispatched (no data loss, no quit)");
        // Recovery: Quit -> summary -> Cancel clears everything; manual Save As works again.
        command(&mut e, &ex, "quit");
        assert!(e.prompt.is_some());
        action(&mut e, &ex, crate::prompt::PromptAction::Cancel);
        assert!(e.pending_save_as.is_none() && e.quit_drain.is_none() && !e.quit);
        command(&mut e, &ex, "save_as");
        assert!(e.file_browser.is_some(), "manual Save As available after recovery");
    }

    /// Same gap driven through the PRODUCTION message path (`app::reduce` with a Mouse event),
    /// so the picker's intercept and the overlay routing are both exercised for real.
    #[test]
    fn c3_click_away_via_app_reduce_orphans_the_quit() {
        use crossterm::event::{Event, MouseEvent, MouseEventKind, MouseButton, KeyModifiers};
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "new ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        assert!(e.file_browser.is_some());
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let (tx, _rx) = std::sync::mpsc::channel();
        let ev = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 0, row: 0, modifiers: KeyModifiers::NONE };
        crate::app::reduce(crate::app::Msg::Input(Event::Mouse(ev)), &mut e, &reg, &km, &ex, &TestClock(0), &tx, &test_fs());
        assert!(e.file_browser.is_none(), "click-away closed the picker through reduce");
        assert_eq!(e.pending_save_as, Some(PostSaveAction::ContinueQuitDrain), "GAP: ownership orphaned");
        assert!(e.quit_drain.is_some(), "GAP: drain survives with nothing to drive it");
        assert!(!e.quit);
        assert!(ex.jobs.borrow().is_empty());
    }

    /// Same click-away on the close-buffer flow's (non-owned) Save As picker: Esc would clear
    /// `pending_save_as` via `cancel_destination`; the mouse path clears nothing, so a LATER
    /// manual Save As consumes the stale CloseBuffer action. Records observed behavior.
    #[test]
    fn c4_click_away_on_close_buffer_picker_leaves_stale_close_action() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "new ");
        let id = e.active().id;
        // A second buffer so closing `id` is an ordinary close, not last-ordinary replacement.
        let other = e.alloc_id();
        e.buffers.push(crate::editor::Buffer::from_text(other, "other", None, (80, 24)));
        let ex = DeferredExecutor::default();
        action(&mut e, &ex, crate::prompt::PromptAction::CloseSave { id });
        assert!(e.file_browser.is_some(), "unnamed: picker");
        assert!(e.file_browser.as_ref().unwrap().quit_save_owner.is_none(), "not quit-owned");
        assert!(matches!(e.pending_save_as, Some(PostSaveAction::CloseBuffer { .. })));
        click(&mut e, &ex, 0, 0);
        assert!(e.file_browser.is_none());
        let stale = matches!(e.pending_save_as, Some(PostSaveAction::CloseBuffer { .. }));
        assert!(stale, "OBSERVED (pre-existing): pending_save_as={:?}", e.pending_save_as);
        // A later, unrelated manual Save As consumes it and closes the buffer after saving.
        command(&mut e, &ex, "save_as");
        let path = dir.path().join("later.md");
        destination(&mut e, &path);
        submit_destination(&mut e, &ex);
        assert_eq!(ex.jobs.borrow().len(), 1);
        ex.complete_next(&mut e);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new body", "saved first — no data loss");
        assert!(e.by_id(id).is_none(), "OBSERVED: the buffer was closed by the stale action");
        assert_eq!(e.status_text(), "saved — closed");
    }

    /// Contrast: the same click-away on a MANUAL picker is a plain close with no side effect.
    #[test]
    fn c2_mouse_click_away_on_manual_picker_is_a_plain_close() {
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "new ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_as");
        assert!(e.file_browser.is_some());
        click(&mut e, &ex, 0, 0);
        assert!(e.file_browser.is_none());
        assert!(e.pending_save_as.is_none() && e.quit_drain.is_none() && !e.quit);
    }

    // ---------------------------------------------------------------------------------
    // Adjacent, pre-existing — the quit-owned OVERWRITE prompt (picker already gone) closed
    // through the overlay registry (`close: |e| e.prompt = None`) leaves the same orphan.
    // ---------------------------------------------------------------------------------
    #[test]
    fn d_close_all_over_quit_owned_overwrite_prompt_leaves_ownership_orphaned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("target.md");
        std::fs::write(&path, "old").unwrap();
        let mut e = Editor::new_from_text("body", None, (80, 24));
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        destination(&mut e, &path);
        submit_destination(&mut e, &ex);
        assert!(e.prompt.is_some() && e.file_browser.is_none());
        e.open_palette(); // any close_all caller (plugin prompt/palette, dispatch_overlay_command)
        assert!(e.prompt.is_none());
        let orphaned = e.pending_save_as == Some(PostSaveAction::ContinueQuitDrain)
            && e.quit_drain.is_some() && e.pending_save_overwrite.is_some();
        assert!(orphaned, "PRE-EXISTING GAP (documented): pending_save_as={:?} drain={} overwrite={:?}",
            e.pending_save_as, e.quit_drain.is_some(), e.pending_save_overwrite);
        command(&mut e, &ex, "save_as");
        assert!(e.file_browser.is_none(), "manual Save As refused");
        assert!(ex.jobs.borrow().is_empty(), "nothing written; nothing exits");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
        // Recovery path is the same: Quit -> summary -> Cancel.
        command(&mut e, &ex, "quit");
        action(&mut e, &ex, crate::prompt::PromptAction::Cancel);
        assert!(e.pending_save_as.is_none() && e.quit_drain.is_none() && e.pending_save_overwrite.is_none());
    }

    // ---------------------------------------------------------------------------------
    // M3 — the panic path names the cancelled plain-Quit wait too.
    // ---------------------------------------------------------------------------------
    #[test]
    fn e_worker_panic_during_plain_quit_wait_names_the_cancelled_quit() {
        // The plain Quit must see a CLEAN buffer to enter the wait phase, and a clean named
        // document's plain Save skips the unchanged write — so the in-flight save is a Save As
        // (same shape as the maintained `quit_waiting_for_saves_*` tests).
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "body").unwrap();
        let mut e = Editor::new_from_text("body", Some(a), (80, 24));
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, b.clone(), b, &ex, &TestClock(0), &tx, &test_fs());
        assert_eq!(ex.jobs.borrow().len(), 1);
        command(&mut e, &ex, "quit");
        assert!(!e.quit);
        assert!(e.quit_drain.as_ref().is_some_and(|d| d.waiting_since.is_some()), "waiting phase");
        ex.panic_next(&mut e);
        assert!(!e.quit);
        assert!(e.quit_drain.is_none());
        assert!(e.saves_in_flight.is_empty());
        assert!(e.status_text().contains("save failed (internal error: boom)"), "{}", e.status_text());
        assert!(e.status_text().contains("quit cancelled"), "{}", e.status_text());
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
    }

    /// A foreign save failure must NOT append "quit cancelled" when no quit is waiting.
    #[test]
    fn e2_failure_with_no_quit_waiting_has_no_cancellation_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("missing/b.md");
        let mut e = Editor::new_from_text("body", Some(dir.path().join("a.md")), (80, 24));
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, bad.clone(), bad, &ex, &TestClock(0), &tx, &test_fs());
        ex.complete_next(&mut e);
        assert!(!e.status_text().contains("quit cancelled"), "{}", e.status_text());
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        // And a panic with no quit waiting: same.
        let mut e = Editor::new_from_text("body", Some(dir.path().join("a.md")), (80, 24));
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save");
        ex.panic_next(&mut e);
        assert!(!e.status_text().contains("quit cancelled"), "{}", e.status_text());
    }

    /// M3 protection retained: a foreign failure does not cancel a Save All awaiting its own
    /// later request, and no "quit cancelled" suffix is emitted for it.
    #[test]
    fn e3_foreign_failure_does_not_cancel_or_label_an_awaited_save_all() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let bad = dir.path().join("missing/b.md");
        std::fs::write(&a, "body").unwrap();
        let mut e = Editor::new_from_text("body", Some(a.clone()), (80, 24));
        let ex = DeferredExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        // Foreign failing write to a bad path via Save As? That would rekey on success only; on
        // failure the path stays `a`. Dispatch it first, then a Save and Quit on the same buffer.
        crate::prompts::perform_save_as(&mut e, bad.clone(), bad, &ex, &TestClock(0), &tx, &test_fs());
        insert(&mut e, "x");
        command(&mut e, &ex, "save_and_quit");
        assert_eq!(ex.jobs.borrow().len(), 2);
        assert!(e.pending_after_save.is_some());
        ex.complete_next(&mut e); // the foreign failure
        assert!(e.quit_drain.is_some(), "awaited later request still protected");
        assert!(e.pending_after_save.is_some());
        assert!(!e.status_text().contains("quit cancelled"), "{}", e.status_text());
        ex.complete_next(&mut e); // the awaited save
        assert!(e.quit);
    }

    // ---------------------------------------------------------------------------------
    // M2 — `quit::cancel` at the awaited-panic site also revokes a provisional exit and
    // clears a picker's owner; verify no picker/prompt is disturbed in the common case.
    // ---------------------------------------------------------------------------------
    #[test]
    fn f_awaited_save_panic_in_save_all_cancels_without_stranding() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();
        let mut e = Editor::new_from_text("A", Some(a), (80, 24));
        insert(&mut e, "edited ");
        let bid = e.alloc_id();
        e.buffers.push(crate::editor::Buffer::from_text(bid, "B", Some(b.clone()), (80, 24)));
        e.switch_to_index(1);
        insert(&mut e, "edited ");
        e.switch_to_index(0);
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        ex.panic_next(&mut e);
        assert!(!e.quit);
        assert!(e.quit_drain.is_none());
        assert!(e.pending_after_save.is_none());
        assert!(!e.quit_drain_advance);
        assert!(e.saves_in_flight.is_empty());
        assert!(ex.jobs.borrow().is_empty(), "the drain did not continue to B");
        assert!(e.is_dirty(bid) && e.active().document.dirty(), "nothing lost, nothing exited");
        assert!(e.status_text().contains("save failed"), "{}", e.status_text());
        // Save and Quit is usable again immediately.
        command(&mut e, &ex, "save_and_quit");
        assert_eq!(ex.jobs.borrow().len(), 1);
    }

    // ---------------------------------------------------------------------------------
    // Interaction — Command::Quit over a quit-owned picker now cancels through close_overlay
    // BEFORE `quit::start`; Save All from the summary must still restart cleanly.
    // ---------------------------------------------------------------------------------
    #[test]
    fn g_summary_over_quit_owned_picker_restarts_save_all_with_fresh_ownership() {
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "new ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        let first_owner = e.file_browser.as_ref().unwrap().quit_save_owner;
        command(&mut e, &ex, "quit");
        assert!(e.prompt.is_some() && e.file_browser.is_none());
        // close_overlay already cancelled the first attempt.
        assert!(e.pending_save_as.is_none() && e.quit_drain.is_none());
        action(&mut e, &ex, crate::prompt::PromptAction::QuitSaveAll);
        assert!(e.file_browser.is_some());
        assert_eq!(e.file_browser.as_ref().unwrap().quit_save_owner, first_owner);
        assert_eq!(e.pending_save_as, Some(PostSaveAction::ContinueQuitDrain));
        assert!(e.quit_drain.as_ref().is_some_and(|d| d.queue.front() == Some(&e.active().id)));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.md");
        destination(&mut e, &path);
        submit_destination(&mut e, &ex);
        assert_eq!(ex.jobs.borrow().len(), 1);
        ex.complete_next(&mut e);
        assert!(e.quit);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new body");
    }
}
