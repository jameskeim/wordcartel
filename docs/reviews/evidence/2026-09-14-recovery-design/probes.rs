#[cfg(test)]
mod recovery_design_probes {
    use super::*;
    use crate::jobs::{Executor, InlineExecutor};
    use crate::test_support::TestClock;
    fn dirty(e: &mut Editor) {
        e.active_mut().document.version = 1;
        e.active_mut().last_edit_at = Some(0);
    }
    fn send(e: &mut Editor, msg: crate::app::Msg, ms: u64, ex: &dyn Executor) {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::reduce(msg, e, &reg, &km, ex, &TestClock(ms), &tx, &crate::test_support::test_fs());
        crate::app::advance(e, &TestClock(ms)); // production pre-render stage
    }
    fn checkpoint(e: &mut Editor, ex: &dyn Executor) {
        let (tx, _rx) = std::sync::mpsc::channel();
        dispatch_swap_write(&mut Ctx { editor: e, clock: &TestClock(3000), executor: ex,
            msg_tx: tx, fs: crate::test_support::test_fs() });
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, e); }
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
}
