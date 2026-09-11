
#[cfg(test)]
mod review_probes {
    use super::*;
    use crate::jobs::{Executor, InlineExecutor};
    use crate::test_support::{FaultAt, FaultFs, TestClock};
    fn dirty(e: &mut Editor) {
        e.active_mut().document.version = 1;
        e.active_mut().last_edit_at = Some(0);
    }
    #[test]
    fn untitled_checkpoints_collide() {
        let mut e = Editor::new_from_text("first document", None, (80,24));
        dirty(&mut e);
        let ex = InlineExecutor::default();
        let c = TestClock::new(3000);
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> = std::sync::Arc::new(crate::fsx::RealFs);
        let first = e.active().id;
        dispatch_swap_write(&mut Ctx {editor: &mut e, clock: &c, executor: &ex, msg_tx: tx.clone(), fs: fs.clone()});
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        let id = e.alloc_id();
        e.buffers.push(crate::editor::Buffer::from_text(id, "second document", None, (80,24)));
        e.switch_to_index(e.buffers.len()-1);
        dirty(&mut e);
        dispatch_swap_write(&mut Ctx {editor: &mut e, clock: &c, executor: &ex, msg_tx: tx, fs});
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        let path = swap_path(None).unwrap();
        let (_, body) = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(body, "second document");
        assert_eq!(e.by_id(first).unwrap().swapped_version, Some(1));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn failed_checkpoint_immediately_rearms() {
        let mut e = Editor::new_from_text("unsaved", None, (80,24));
        dirty(&mut e);
        let ex = InlineExecutor::default();
        let c = TestClock::new(3000);
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> = std::sync::Arc::new(FaultFs::new(FaultAt::Create));
        let deadline = crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "swap").unwrap().deadline;
        for _ in 0..3 {
            assert_eq!(deadline(&e, 3000), Some(3000));
            crate::timers::on_tick(&mut e, &ex, &c, &tx, &fs);
            let outcomes = ex.drain();
            assert!(!outcomes.is_empty());
            for o in outcomes { crate::jobs_apply::apply_outcome(o, &mut e); }
            assert_eq!(e.status_text(), "swap write failed");
        }
    }
    #[test]
    fn inactive_dirty_buffer_has_no_checkpoint_deadline() {
        let mut e = Editor::new_from_text("unsaved", None, (80,24));
        dirty(&mut e);
        let first = e.active().id;
        let deadline = crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "swap").unwrap().deadline;
        assert_eq!(deadline(&e, 3000), Some(3000));
        crate::workspace::new_empty_buffer(&mut e);
        assert_ne!(e.active().id, first);
        assert_eq!(deadline(&e, 60_000), None);
        assert_eq!(e.by_id(first).unwrap().swapped_version, None);
    }

    fn send(e: &mut Editor, msg: crate::app::Msg, ms: u64, ex: &dyn Executor) {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::reduce(msg, e, &reg, &km, ex, &TestClock(ms), &tx, &crate::test_support::test_fs());
        crate::app::advance(e, &TestClock(ms)); // production pre-render stage
    }
    fn type_a(e: &mut Editor, ms: u64, ex: &dyn Executor) {
        send(e, crate::test_support::press(crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE), ms, ex);
    }
    fn checkpoint(e: &mut Editor, ex: &dyn Executor) {
        let (tx, _rx) = std::sync::mpsc::channel();
        dispatch_swap_write(&mut Ctx { editor: e, clock: &TestClock(3000), executor: ex,
            msg_tx: tx, fs: crate::test_support::test_fs() });
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, e); }
    }

    #[test]
    fn opening_during_session_skips_recovery_and_save_deletes_carrier() {
        // Exercise both additive open and replacement of an empty throwaway buffer.
        for initial in ["other document", "\n"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("crashed.md");
            std::fs::write(&path, "on disk").unwrap();
            let body = "unsaved work from a crashed session";
            let mut crashed = Editor::new_from_text(body, Some(path.clone()), (80,24));
            dirty(&mut crashed);
            let carrier = swap_path(Some(&path)).unwrap();
            let mut h = build_header(&crashed, body, 1);
            h.pid = 999_999;
            assert!(!pid_is_live(h.pid));
            write_atomic(&carrier, &serialize(&h, body)).unwrap();
            assert!(matches!(assess(&crate::fsx::RealFs, Some(&path), Some(b"on disk")),
                RecoveryDecision::Prompt(_, ref recovered) if recovered == body));
            let mut e = Editor::new_from_text(initial, None, (80,24));
            e.resume_enabled = false;
            crate::workspace::open_as_new_buffer(&mut e, &crate::fsx::RealFs, &path);
            assert_eq!(e.active().document.buffer.to_string(), "on disk");
            assert!(e.prompt.is_none());
            assert!(e.active().pending_swap_body.is_none());
            assert!(carrier.exists());
            let ex = InlineExecutor::default();
            let (tx, _rx) = std::sync::mpsc::channel();
            crate::save::dispatch_save(&mut Ctx { editor: &mut e, clock: &TestClock(5000),
                executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() });
            for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
            assert!(!carrier.exists(), "ordinary Save destroyed unoffered recovery content");
        }
    }

    #[test]
    fn recovering_unnamed_work_deletes_carrier_without_arming_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let orphan = dir.path().join("scratch-999999.swp");
        let body = "recovered unnamed work";
        let mut e = Editor::new_from_text("\n", None, (80,24));
        let mut h = build_header(&e, body, 1);
        h.pid = 999_999;
        write_atomic(&orphan, &serialize(&h, body)).unwrap();
        // Mirror the production startup staging, then use the actual modal input path.
        e.active_mut().pending_swap_body = Some(body.into());
        e.active_mut().pending_swap_path = Some(orphan.clone());
        e.open_prompt(crate::prompt::Prompt::swap_recovery());
        let ex = InlineExecutor::default();
        send(&mut e, crate::test_support::press(crossterm::event::KeyCode::Char('r'),
            crossterm::event::KeyModifiers::NONE), 1000, &ex);
        assert_eq!(e.active().document.buffer.to_string(), body);
        assert!(e.active().document.dirty());
        assert!(e.prompt.is_none());
        assert!(!orphan.exists());
        assert_eq!(e.active().last_edit_at, None);
        send(&mut e, crate::app::Msg::Tick, 60_000, &ex);
        let deadline = crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "swap").unwrap().deadline;
        assert_eq!(deadline(&e, 60_000), None);
        assert_eq!(e.active().swapped_version, None);
    }

    #[test]
    fn uninterrupted_typing_never_creates_first_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("typing.md");
        std::fs::write(&path, "start").unwrap();
        let mut e = Editor::new_from_text("start", Some(path.clone()), (80,24));
        let ex = InlineExecutor::default();
        // No artificial initial pause: type every second for two minutes.
        for sec in 0..=120 {
            type_a(&mut e, sec * 1000, &ex);
            send(&mut e, crate::app::Msg::Tick, sec * 1000 + 900, &ex);
        }
        assert!(e.active().document.version >= 121);
        assert_eq!(e.active().last_swap_at, None);
        assert!(!swap_path(Some(&path)).unwrap().exists());
        // Control: a real pause allows a checkpoint, proving timers are wired.
        send(&mut e, crate::app::Msg::Tick, 123_000, &ex);
        assert!(e.active().last_swap_at.is_some());
        delete(Some(&path));
    }

    #[derive(Default)]
    struct Deferred {
        queue: std::cell::RefCell<std::collections::VecDeque<crate::jobs::Job>>,
    }
    impl Executor for Deferred {
        fn dispatch(&self, job: crate::jobs::Job) { self.queue.borrow_mut().push_back(job); }
        fn drain(&self) -> Vec<crate::jobs::JobOutcome> { Vec::new() }
    }
    impl Deferred {
        fn run_one(&self, e: &mut Editor) {
            let job = self.queue.borrow_mut().pop_front().unwrap();
            crate::jobs_apply::apply_result((job.run)(), e);
        }
    }

    #[test]
    fn queued_save_overwrites_external_edit_after_foreground_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queued.md");
        std::fs::write(&path, "original").unwrap();
        let mut e = Editor::new_from_text("my edit", Some(path.clone()), (80,24));
        dirty(&mut e);
        let ex = Deferred::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::save::dispatch_save(&mut Ctx { editor: &mut e, clock: &TestClock(1),
            executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() });
        assert!(e.prompt.is_none());
        assert_eq!(ex.queue.borrow().len(), 1);
        std::fs::write(&path, "external edit before worker runs").unwrap();
        ex.run_one(&mut e);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "my edit");
        assert!(!e.active().document.dirty());
        assert!(e.prompt.is_none());
    }

    #[test]
    fn save_as_of_one_unnamed_buffer_deletes_anothers_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("saved.md");
        let mut e = Editor::new_from_text("A", None, (80,24));
        dirty(&mut e);
        let a = e.active().id;
        let b = e.alloc_id();
        e.buffers.push(crate::editor::Buffer::from_text(b, "B", None, (80,24)));
        e.switch_to_index(1);
        dirty(&mut e);
        let ex = InlineExecutor::default();
        checkpoint(&mut e, &ex);
        let carrier = swap_path(None).unwrap();
        assert_eq!(parse(&std::fs::read_to_string(&carrier).unwrap()).unwrap().1, "B");
        e.switch_to_index(0);
        assert_eq!(e.active().id, a);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, saved.clone(), saved.clone(), &ex,
            &TestClock(4000), &tx, &crate::test_support::test_fs());
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert_eq!(std::fs::read_to_string(saved).unwrap(), "A");
        assert!(!carrier.exists());
        assert_eq!(e.by_id(b).unwrap().swapped_version, Some(1));
        assert!(e.by_id(b).unwrap().document.dirty());
    }

    #[test]
    fn normal_save_queued_after_save_as_marks_wrong_path_clean() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "original").unwrap();
        let mut e = Editor::new_from_text("original", Some(a.clone()), (80,24));
        let ex = Deferred::default();
        type_a(&mut e, 0, &ex);
        let first = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::perform_save_as(&mut e, b.clone(), b.clone(), &ex, &TestClock(1),
            &tx, &crate::test_support::test_fs());
        type_a(&mut e, 2, &ex);
        let second = e.active().document.buffer.to_string();
        assert_ne!(first, second);
        // The first job has not returned; Document.path is still A.
        crate::save::dispatch_save(&mut Ctx { editor: &mut e, executor: &ex,
            clock: &TestClock(3), msg_tx: tx, fs: crate::test_support::test_fs() });
        assert_eq!(ex.queue.borrow().len(), 2);
        ex.run_one(&mut e);
        ex.run_one(&mut e); // exact production FIFO, no invented result reordering
        assert_eq!(e.active().document.path.as_deref(), Some(b.as_path()));
        assert!(!e.active().document.dirty(), "BUG: current path is incorrectly reported clean");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), first);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), second);
        assert_ne!(std::fs::read_to_string(&b).unwrap(), e.active().document.buffer.to_string());
    }

    #[test]
    fn save_failure_matrix_preserves_dirty_state_and_original_before_rename() {
        for failure in [FaultAt::Create, FaultAt::Write { after: 2 }, FaultAt::SetMode,
            FaultAt::Flush, FaultAt::Sync, FaultAt::Rename, FaultAt::SyncDir] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("fault.md");
            std::fs::write(&path, "original").unwrap();
            let mut e = Editor::new_from_text("edited body", Some(path.clone()), (80,24));
            dirty(&mut e);
            let fp = e.active().document.stored_fp;
            let ex = InlineExecutor::default();
            checkpoint(&mut e, &ex); // preserve a real recovery copy through failed save
            let carrier = swap_path(Some(&path)).unwrap();
            let (tx, _rx) = std::sync::mpsc::channel();
            let fs = std::sync::Arc::new(FaultFs::new(failure));
            crate::save::do_save_to(&mut Ctx { editor: &mut e, executor: &ex,
                clock: &TestClock(0), msg_tx: tx, fs },
                crate::save::SaveTarget::same(path.clone()), crate::save::SaveMode::Normal);
            for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
            assert!(e.active().document.dirty(), "{failure:?}");
            assert_eq!(e.active().document.stored_fp, fp, "{failure:?}");
            let expected = if matches!(failure, FaultAt::SyncDir) { "edited body" } else { "original" };
            assert_eq!(std::fs::read_to_string(&path).unwrap(), expected, "{failure:?}");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "temp litter: {failure:?}");
            assert!(!e.quit);
            assert_eq!(parse(&std::fs::read_to_string(&carrier).unwrap()).unwrap().1, "edited body");
            delete(Some(&path));
        }
    }

    #[test]
    fn named_buffers_share_checkpoint_and_saving_one_removes_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.md");
        std::fs::write(&path, "disk").unwrap();
        let mut a = Editor::new_from_text("A's unsaved text", Some(path.clone()), (80,24));
        let mut b = Editor::new_from_text("B's unsaved text", Some(path.clone()), (80,24));
        dirty(&mut a);
        dirty(&mut b);
        let ex = InlineExecutor::default();
        checkpoint(&mut a, &ex);
        checkpoint(&mut b, &ex);
        let carrier = swap_path(Some(&path)).unwrap();
        assert_eq!(parse(&std::fs::read_to_string(&carrier).unwrap()).unwrap().1, "B's unsaved text");
        assert_eq!(a.active().swapped_version, Some(1));
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::save::dispatch_save(&mut Ctx { editor: &mut a, executor: &ex,
            clock: &TestClock(4), msg_tx: tx, fs: crate::test_support::test_fs() });
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut a); }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "A's unsaved text");
        assert!(!carrier.exists());
        assert!(b.active().document.dirty());
        assert_eq!(b.active().swapped_version, Some(1));
    }

    #[test]
    fn palette_edit_has_no_checkpoint_until_an_unrelated_normal_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("palette.md");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let mut e = Editor::new_from_text("alpha\nbeta\n", Some(path.clone()), (80,24));
        let mut palette = crate::palette::Palette::default();
        palette.rows = vec![crate::palette::PaletteRow {
            id: crate::registry::CommandId("delete_line"), label: "Delete Line".into(),
            chord: String::new(), buffer: None,
        }];
        e.palette = Some(palette);
        let ex = InlineExecutor::default();
        send(&mut e, crate::test_support::press(crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE), 0, &ex);
        assert_eq!(e.active().document.buffer.to_string(), "beta\n");
        assert!(e.active().document.dirty());
        assert!(e.palette.is_none());
        send(&mut e, crate::app::Msg::Tick, 60_000, &ex);
        assert_eq!(e.active().last_edit_at, None);
        assert!(!swap_path(Some(&path)).unwrap().exists());
        type_a(&mut e, 61_000, &ex);
        send(&mut e, crate::app::Msg::Tick, 64_000, &ex);
        assert!(swap_path(Some(&path)).unwrap().exists());
        delete(Some(&path));
    }

    #[test]
    fn header_makes_max_size_valid_document_unrecoverable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.md");
        let max = crate::limits::MAX_OPEN_BYTES as usize;
        let body = "a".repeat(max);
        std::fs::write(&path, &body).unwrap();
        assert_eq!(crate::file::open(&path).unwrap().len(), max,
            "a document this large is accepted by the actual open path");
        let h = SwapHeader { realpath: Some(path.to_string_lossy().into_owned()),
            content_hash: fnv1a64(body.as_bytes()), version: 1, ts_ms: 1, pid: 999_999,
            ..Default::default() };
        let raw = serialize(&h, &body);
        assert!(raw.len() > max);
        assert_eq!(parse(&raw).unwrap().1.len(), max, "valid complete swap format");
        let carrier = swap_path(Some(&path)).unwrap();
        write_atomic(&carrier, &raw).unwrap();
        drop(raw);
        drop(body);
        // Simulate a smaller older on-disk version, with the large text existing only in swap.
        std::fs::write(&path, "disk").unwrap();
        assert!(matches!(assess(&crate::fsx::RealFs, Some(&path), Some(b"disk")),
            RecoveryDecision::OpenNormally), "BUG: valid recovery body hidden by header overhead");
        delete(Some(&path));
    }

    #[test]
    fn modal_discards_due_swap_ticks_and_leaves_immediate_wakeup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("modal.md");
        std::fs::write(&path, "text").unwrap();
        let mut e = Editor::new_from_text("text", Some(path.clone()), (80,24));
        let ex = InlineExecutor::default();
        type_a(&mut e, 0, &ex);
        e.open_prompt(crate::prompt::Prompt::quit_confirm());
        for ms in [3000, 30_000, 60_000] {
            send(&mut e, crate::app::Msg::Tick, ms, &ex);
            assert!(e.prompt.is_some());
            let deadline = crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "swap").unwrap().deadline;
            assert_eq!(deadline(&e, ms), Some(ms));
            assert!(crate::timers::next_wake(&e, ms).is_some_and(|d| d <= ms),
                "the loop's saturating deadline subtraction produces a zero timeout");
            assert!(!swap_path(Some(&path)).unwrap().exists());
        }
        send(&mut e, crate::test_support::press(crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE), 61_000, &ex);
        send(&mut e, crate::app::Msg::Tick, 61_001, &ex);
        assert!(swap_path(Some(&path)).unwrap().exists());
        delete(Some(&path));
    }

    #[test]
    fn save_and_quit_leaves_another_dirty_document_unsaved() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "A disk").unwrap();
        std::fs::write(&b, "B disk").unwrap();
        let mut e = Editor::new_from_text("A edited", Some(a.clone()), (80,24));
        dirty(&mut e);
        let bid = e.alloc_id();
        e.buffers.push(crate::editor::Buffer::from_text(bid, "B edited", Some(b.clone()), (80,24)));
        e.by_id_mut(bid).unwrap().document.version = 1;
        let ex = InlineExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = crate::registry::Registry::builtins();
        reg.dispatch(crate::registry::CommandId("save_and_quit"), &mut Ctx {
            editor: &mut e, executor: &ex, clock: &TestClock(1), msg_tx: tx,
            fs: crate::test_support::test_fs(),
        });
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(e.quit, "BUG: Save and Quit bypassed the multi-buffer dirty guard");
        assert!(e.is_dirty(bid));
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "A edited");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "B disk");
        assert!(!swap_path(Some(&b)).unwrap().exists());
        assert!(e.prompt.is_none());
    }

    #[test]
    fn save_all_quit_misses_a_buffer_dirtied_after_queue_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "A disk").unwrap();
        std::fs::write(&b, "B disk").unwrap();
        let mut e = Editor::new_from_text("A edited", Some(a.clone()), (80,24));
        dirty(&mut e);
        let bid = e.alloc_id();
        e.buffers.push(crate::editor::Buffer::from_text(bid, "B disk", Some(b.clone()), (80,24)));
        let ex = Deferred::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs = crate::test_support::test_fs();
        crate::prompts::resolve_prompt(crate::prompt::PromptAction::QuitSaveAll,
            &mut e, &ex, &TestClock(0), &tx, &fs);
        assert_eq!(e.quit_drain.as_ref().unwrap().queue.len(), 1);
        e.switch_to_index(1);
        type_a(&mut e, 1, &ex);
        assert!(e.is_dirty(bid));
        let job = ex.queue.borrow_mut().pop_front().unwrap();
        crate::jobs_apply::apply_job_outcome(crate::jobs::JobOutcome::Done((job.run)()),
            &mut e, &ex, &TestClock(2), &tx, &fs);
        assert!(e.quit, "BUG: draining A's save exited with newly dirty B");
        assert!(e.is_dirty(bid));
        assert_eq!(std::fs::read_to_string(b).unwrap(), "B disk");
        assert!(e.prompt.is_none());
    }
}
