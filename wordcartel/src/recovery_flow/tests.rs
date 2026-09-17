use super::*;
use crate::jobs::{Executor, InlineExecutor};
use crate::test_support::{test_fs, TestClock};
#[test]
fn recovery_ownership_live_producer_assigns_independent_unnamed_records() {
    let mut editor = Editor::new_from_text("first draft", None, (80, 24));
    let first = editor.active().id;
    let second = editor.alloc_id();
    editor.buffers.push(crate::editor::Buffer::from_text(second, "second draft", None, (80, 24)));
    let ex = InlineExecutor::default();
    let clock = TestClock::new(123);
    let fs = test_fs();
    let (tx, _) = std::sync::mpsc::channel();
    for id in [first, second] {
        editor.active = editor.buffers.iter().position(|b| b.id == id).unwrap();
        crate::swap::dispatch_swap_write(&mut Ctx { editor: &mut editor, executor: &ex,
            clock: &clock, msg_tx: tx.clone(), fs: fs.clone() });
        for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, &mut editor, &ex, &clock, &tx, &fs); }
    }
    let a = editor.by_id(first).unwrap().recovery_ack.as_ref().expect("first checkpoint acknowledged");
    let b = editor.by_id(second).unwrap().recovery_ack.as_ref().expect("second checkpoint acknowledged");
    assert_ne!(a.path(), b.path());
    for (ack, expected) in [(a, "first draft"), (b, "second draft")] {
        let bytes = fs.open_regular_nofollow(ack.path()).unwrap().read_capped(recovery_store::MAX_RECORD_BYTES).unwrap().unwrap();
        assert_eq!(recovery_store::decode(&bytes).unwrap().1, expected);
    }
}

fn dispatch_one(editor: &mut Editor, ex: &dyn Executor) -> DispatchOutcome {
    let id = editor.active().id;
    let (tx, _) = std::sync::mpsc::channel();
    dispatch_checkpoint(&mut Ctx { editor, executor: ex, clock: &TestClock::new(7), msg_tx: tx, fs: test_fs() }, id)
}
#[test]
fn recovery_ownership_rejection_consumes_identity_without_sticking_latch() {
    struct Reject;
    impl Executor for Reject {
        fn try_dispatch(&self, job: Job) -> Result<(), crate::jobs::DispatchError> {
            drop(job); Err(crate::jobs::DispatchError::Closed)
        }
        fn drain(&self) -> Vec<crate::jobs::JobOutcome> { Vec::new() }
    }
    let mut e = Editor::new_from_text("draft", None, (80,24));
    assert_eq!(dispatch_one(&mut e, &Reject), DispatchOutcome::Rejected);
    assert!(e.recovery.requests.is_empty());
    assert!(e.active().recovery_request.is_none());
    assert!(!e.active().swap_in_flight);
    assert!(e.active().recovery_protection_failure.is_some());
    let consumed = e.active().recovery_generation;
    let ex = InlineExecutor::default();
    assert_eq!(dispatch_one(&mut e, &ex), DispatchOutcome::Accepted);
    assert!(e.active().recovery_generation > consumed);
    for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
    assert!(e.active().recovery_ack.is_some());
}
#[test]
fn recovery_ownership_foreign_panic_does_not_clear_current_request() {
    let mut e = Editor::new_from_text("draft", None, (80,24));
    let ex = crate::recovery_regressions::DeferredRecoveryExecutor::default();
    dispatch_one(&mut e, &ex);
    let own = e.active().recovery_request.unwrap();
    let foreign = RecoveryRequestId(own.0 + 1);
    on_panic(&mut e, foreign, "foreign");
    assert_eq!(e.active().recovery_request, Some(own));
    crate::jobs_apply::apply_outcome(ex.run_next(), &mut e);
    assert!(e.active().recovery_ack.is_some());
}
#[test]
fn recovery_ownership_replacement_same_id_cannot_acknowledge_old_slot() {
    let mut e = Editor::new_from_text("old", None, (80,24));
    let ex = crate::recovery_regressions::DeferredRecoveryExecutor::default();
    dispatch_one(&mut e, &ex);
    let id = e.active().id;
    let old = e.active().recovery_slot.clone();
    e.replace_buffer(0, crate::editor::Buffer::from_text(id, "new", None, (80,24)));
    assert!(!old.same_instance(&e.active().recovery_slot));
    crate::jobs_apply::apply_outcome(ex.run_next(), &mut e);
    assert!(e.active().recovery_ack.is_none());
    assert!(e.recovery.requests.is_empty());
}
#[test]
fn recovery_ownership_association_worker_resolves_missing_relative_suffix() {
    let fs = test_fs();
    let path = resolve_association(&*fs, Path::new("missing-recovery-parent/untitled.md")).unwrap();
    assert!(path.is_absolute());
    assert!(path.ends_with("missing-recovery-parent/untitled.md"));
}
#[test]
fn recovery_ownership_default_roots_are_private_to_each_editor() {
    let mut a = Editor::new_from_text("a", None, (80,24));
    let mut b = Editor::new_from_text("b", None, (80,24));
    assert_ne!(a.recovery.root().unwrap(), b.recovery.root().unwrap());
    let root = crate::test_support::scratch_path("shared-recovery");
    a.recovery.set_root(root.clone()); b.recovery.set_root(root);
    assert_eq!(a.recovery.root().unwrap(), b.recovery.root().unwrap());
}

#[test]
fn recovery_ownership_after_job_recaptures_changed_association_before_quit() {
    let mut e = Editor::new_from_text("draft", None, (80,24));
    e.active_mut().document.saved_version = None;
    let ex = crate::recovery_regressions::DeferredRecoveryExecutor::default();
    dispatch_one(&mut e, &ex);
    let old = e.active().recovery_slot.clone();
    let path = crate::test_support::scratch_path("association.md");
    e.active_mut().document.path = Some(path.clone());
    let id = e.active().id;
    association_changed(&mut e, id);
    let (tx, _) = std::sync::mpsc::channel();
    let fs = test_fs();
    let clock = TestClock::new(8);
    crate::jobs_apply::apply_job_outcome(ex.run_next(), &mut e, &ex, &clock, &tx, &fs);
    assert!(e.active().recovery_ack.is_none(), "old association cannot latch");
    assert_eq!(ex.pending_len(), 1, "merge wrapper immediately queues current association");
    crate::jobs_apply::apply_job_outcome(ex.run_next(), &mut e, &ex, &clock, &tx, &fs);
    assert!(old.same_instance(&e.active().recovery_slot));
    let ack = e.active().recovery_ack.as_ref().unwrap();
    let bytes = fs.open_regular_nofollow(ack.path()).unwrap().read_capped(recovery_store::MAX_RECORD_BYTES).unwrap().unwrap();
    let (metadata, body) = recovery_store::decode(&bytes).unwrap();
    assert_eq!(body, "draft");
    assert_eq!(metadata.record().association().unwrap().local_path(), Some(path));
}

#[test]
fn recovery_ownership_worker_panic_routes_exact_request_and_allows_retry() {
    #[derive(Default)]
    struct PanicExecutor(InlineExecutor);
    impl Executor for PanicExecutor {
        fn try_dispatch(&self, mut job: Job) -> Result<(), crate::jobs::DispatchError> {
            job.run = Box::new(|| panic!("checkpoint worker panic"));
            self.0.try_dispatch(job)
        }
        fn drain(&self) -> Vec<crate::jobs::JobOutcome> { self.0.drain() }
    }
    let mut e = Editor::new_from_text("draft", None, (80,24));
    let ex = PanicExecutor::default();
    assert_eq!(dispatch_one(&mut e, &ex), DispatchOutcome::Accepted);
    assert!(e.active().recovery_request.is_some());
    for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
    assert!(e.active().recovery_request.is_none());
    assert!(!e.active().swap_in_flight);
    assert!(e.active().recovery_ack.is_none());
    assert!(e.active().recovery_protection_failure.as_deref().unwrap().contains("checkpoint worker panic"));
    let good = InlineExecutor::default();
    dispatch_one(&mut e, &good);
    for o in good.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
    assert!(e.active().recovery_ack.is_some());
}
