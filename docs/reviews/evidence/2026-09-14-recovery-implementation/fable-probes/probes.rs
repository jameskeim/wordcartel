//! Fable round-2 review probes. Compiled against the real branch through a TEMPORARY
//! `mod` line in lib.rs (restored and hash-verified afterwards). Each probe prints what it
//! observed; assertions pin the observation so the log is unambiguous.
#![allow(clippy::all)]
use crate::editor::Editor;
use crate::fsx::{Fs, RealFs};
use crate::jobs::Executor;
use crate::recovery_discovery::{self as discovery, ScanScope};
use crate::recovery_store::{self as store, CheckpointRecord, RecoverySlot, TaggedPath};
use crate::recovery_regressions::DeferredRecoveryExecutor;
use crate::registry::Ctx;
use crate::test_support::{test_fs, TestClock, press};
use std::sync::Arc;
use crossterm::event::{KeyCode, KeyModifiers};

fn v2_source(root: &std::path::Path, body: &str) -> discovery::Candidate {
    let slot = RecoverySlot::new();
    let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "probe-lineage".into(), 0, None, None);
    store::checkpoint(&RealFs, root, &slot, &record, body).unwrap();
    drop(slot);
    crate::test_support::recovery_scan_released(&RealFs, root, &ScanScope::All, 1).remove(0)
}

/// P1 — a LIVE owner whose checkpoint was retired by a save still shows up in an All
/// (manual review) scan as a busy "unavailable" row, because read_v2 takes the lock
/// before it looks for checkpoint.wcr.
#[test]
fn probe_p1_live_owner_with_retired_record_is_listed_as_busy_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("doc.md");
    std::fs::write(&path, "disk").unwrap();
    let mut e = Editor::new_from_text("edited", Some(path.clone()), (80, 24));
    e.recovery.set_root(root.path().join("state"));
    e.active_mut().document.version = 1;
    let ex = crate::jobs::InlineExecutor::default();
    let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
    let (tx, _rx) = std::sync::mpsc::channel();
    crate::swap::dispatch_swap_write(&mut Ctx { editor: &mut e, executor: &ex, clock: &TestClock(3000), msg_tx: tx.clone(), fs: fs.clone() });
    for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, &mut e, &ex, &TestClock(3000), &tx, &fs); }
    let record = e.active().recovery_ack.as_ref().unwrap().record_path().to_owned();
    assert!(record.exists());
    // Ordinary save retires the owned checkpoint (receipt succeeds on this filesystem).
    crate::save::do_save_to(&mut Ctx { editor: &mut e, executor: &ex, clock: &TestClock(4000), msg_tx: tx.clone(), fs: fs.clone() },
        crate::save::SaveTarget::same(path.clone()), crate::save::SaveMode::Normal);
    for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, &mut e, &ex, &TestClock(4000), &tx, &fs); }
    assert!(!record.exists(), "save retired the record");
    assert!(!e.active().document.dirty());
    // The buffer is still open: its slot holds the owner lease. Manual review scan:
    let rows = discovery::scan(&RealFs, &root.path().join("state"), &ScanScope::All).unwrap();
    println!("P1 rows after save with live owner: {rows:#?}");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].busy);
    assert!(rows[0].unavailable.as_deref().unwrap().contains("active"));
    assert!(rows[0].association.is_none(), "row carries no association, so the user cannot tell which document it is");
    // Drop the editor (releases the lease): the same directory is now a tombstone and vanishes.
    drop(e);
    let rows = crate::test_support::recovery_scan_released(&RealFs, &root.path().join("state"), &ScanScope::All, 0);
    assert!(rows.is_empty());
}

/// P2 — Esc on the picker while the batch is in "Opening" phase after the recovered
/// document has ALREADY been installed: cancel_batch cancels the in-flight handoff. The
/// successor checkpoint still gets written and (depending on timing) the CAS is lost, so the
/// source is retained; the successful Ack is never latched, no status/failure indicator is
/// shown, and no automatic checkpoint is scheduled (no last_edit_at).
#[test]
fn probe_p2_esc_during_opening_cancels_installed_handoff_and_drops_its_ack() {
    let root = tempfile::tempdir().unwrap();
    let candidate = v2_source(root.path(), "rescued text");
    let source = candidate.source_path.clone();
    let mut e = Editor::new_from_text("disk", None, (80, 24));
    e.recovery.set_root(root.path().to_owned());
    let ex = DeferredRecoveryExecutor::default();
    let clock = TestClock(0);
    let (tx, _rx) = std::sync::mpsc::channel();
    let fs = test_fs();
    let reg = crate::registry::Registry::builtins();
    let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
    // Manual review → scan job → picker ready.
    crate::recovery_flow::review(&mut Ctx { editor: &mut e, executor: &ex, clock: &clock, msg_tx: tx.clone(), fs: fs.clone() });
    crate::jobs_apply::apply_job_outcome(ex.run_next(), &mut e, &ex, &clock, &tx, &fs);
    crate::app::finish_iteration(&mut e, &ex, &clock, &tx, &fs);
    assert!(e.recovery_picker.as_ref().is_some_and(|p| !p.is_loading()));
    // Space + Enter → prepare dispatched, picker in Opening phase.
    for code in [KeyCode::Char(' '), KeyCode::Enter] {
        crate::app::reduce(press(code, KeyModifiers::NONE), &mut e, &reg, &km, &ex, &clock, &tx, &fs);
    }
    assert_eq!(ex.pending_len(), 1, "prepare queued");
    // Prepare completes → install → handoff queued (buffer count 2).
    crate::jobs_apply::apply_job_outcome(ex.run_next(), &mut e, &ex, &clock, &tx, &fs);
    assert_eq!(e.buffers.len(), 2);
    assert_eq!(ex.pending_len(), 1, "handoff queued");
    let recovered = e.active().id;
    // Esc while Opening → close → cancel_batch → progress.cancel() on the installed handoff.
    crate::app::reduce(press(KeyCode::Esc, KeyModifiers::NONE), &mut e, &reg, &km, &ex, &clock, &tx, &fs);
    assert!(e.recovery_picker.is_none());
    // Worker now runs the handoff: checkpoint succeeds, CAS loses, source retained.
    crate::jobs_apply::apply_job_outcome(ex.run_next(), &mut e, &ex, &clock, &tx, &fs);
    let b = e.by_id(recovered).unwrap();
    println!("P2 after cancelled handoff: ack={:?} swapped_version={:?} failure={:?} status={:?} dirty={} last_edit_at={:?}",
        b.recovery_ack, b.swapped_version, b.recovery_protection_failure, e.status_text(), b.document.dirty(), b.last_edit_at);
    assert!(source.exists(), "source retained (cancel won the CAS)");
    let successors: Vec<_> = discovery::scan(&RealFs, root.path(), &ScanScope::All).unwrap()
        .into_iter().filter(|r| r.source_path != source).collect();
    println!("P2 successor rows: {:?}", successors.iter().map(|r| (&r.source_path, r.busy, &r.unavailable)).collect::<Vec<_>>());
    assert!(!successors.is_empty(), "successor checkpoint was written before the CAS");
    assert!(b.recovery_ack.is_none(), "successful checkpoint is NOT latched");
    assert!(b.recovery_protection_failure.is_none(), "and no failure indicator is shown");
    assert!(b.document.dirty());
    assert!(!crate::recovery_flow::has_pending_work(&e));
    assert_eq!(crate::timers::next_wake(&e, 10_000), None, "no automatic checkpoint is scheduled until an edit");
}

/// P3 — an import whose protection failed (source retained), then saved by the user via
/// Save As: the v2 source stays behind, and a NEXT session's bootstrap scan re-offers it.
/// Within the session it is neither re-offered nor retirable (focus path is IO-free).
#[test]
fn probe_p3_failed_import_then_save_as_leaves_source_reoffered_next_launch() {
    use crate::test_support::{FaultAt, FaultFs};
    let root = tempfile::tempdir().unwrap();
    let candidate = v2_source(root.path(), "rescued text");
    let source = candidate.source_path.clone();
    let mut e = Editor::new_from_text("disk", None, (80, 24));
    e.recovery.set_root(root.path().to_owned());
    let ex = crate::jobs::InlineExecutor::default();
    let clock = TestClock(0);
    let (tx, _rx) = std::sync::mpsc::channel();
    let bad: Arc<dyn Fs + Send + Sync> = Arc::new(FaultFs::new(FaultAt::Sync));
    crate::recovery_flow::begin_selected(&mut Ctx { editor: &mut e, executor: &ex, clock: &clock, msg_tx: tx.clone(), fs: bad.clone() }, vec![candidate.clone()]);
    for _ in 0..4 { for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, &mut e, &ex, &clock, &tx, &bad); } }
    assert_eq!(e.buffers.len(), 2);
    assert!(e.active().recovery_protection_failure.is_some());
    assert!(source.exists());
    let good: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
    let target = root.path().join("named.md");
    crate::save::do_save_to(&mut Ctx { editor: &mut e, executor: &ex, clock: &clock, msg_tx: tx.clone(), fs: good.clone() },
        crate::save::SaveTarget::same(target.clone()), crate::save::SaveMode::SaveAs);
    for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, &mut e, &ex, &clock, &tx, &good); }
    assert!(!e.active().document.dirty());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "rescued text");
    println!("P3 status after Save As: {:?}; source exists={}", e.status_text(), source.exists());
    assert!(source.exists(), "v2 source survives the successful Save As of its unprotected import");
    // Same session: re-selecting focuses (no IO), so the source can never be retired here.
    e.switch_to_index(0);
    let ex2 = DeferredRecoveryExecutor::default();
    crate::recovery_flow::begin_selected(&mut Ctx { editor: &mut e, executor: &ex2, clock: &clock, msg_tx: tx.clone(), fs: good.clone() }, vec![candidate.clone()]);
    assert_eq!(ex2.pending_len(), 0, "clean document → focus only");
    assert!(source.exists());
    // "Next launch": fresh editor on the same root — bootstrap offers it again.
    drop(e);
    let mut e2 = Editor::new_from_text("\n", None, (80, 24));
    e2.recovery.set_root(root.path().to_owned());
    let ex3 = DeferredRecoveryExecutor::default();
    crate::recovery_flow::bootstrap(&mut Ctx { editor: &mut e2, executor: &ex3, clock: &clock, msg_tx: tx.clone(), fs: good.clone() });
    crate::jobs_apply::apply_job_outcome(ex3.run_next(), &mut e2, &ex3, &clock, &tx, &good);
    crate::app::finish_iteration(&mut e2, &ex3, &clock, &tx, &good);
    println!("P3 next-launch picker: {:?}", e2.recovery_picker.is_some());
    assert!(e2.recovery_picker.is_some(), "re-offered at next launch");
}

/// P6 — in-process lock contention on this filesystem: a second open of owner.lock in the
/// SAME process reports WouldBlock (flock semantics), which is what makes this process's own
/// live records show as busy. On lock-emulating mounts (NFS fcntl) this would not hold.
#[test]
fn probe_p6_same_process_second_lock_is_busy_here() {
    let root = tempfile::tempdir().unwrap();
    let slot = RecoverySlot::new();
    let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "probe".into(), 0, None, None);
    let ack = store::checkpoint(&RealFs, root.path(), &slot, &record, "x").unwrap();
    let lock = ack.record_path().parent().unwrap().join("owner.lock");
    let err = RealFs.try_recovery_lock(&lock, false).err().expect("second lock must not succeed");
    println!("P6 second same-process lock: {err}");
    assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
    // The flow's automatic offer does not additionally filter on live owner identity:
    let rows = discovery::scan(&RealFs, root.path(), &ScanScope::All).unwrap();
    assert!(rows[0].busy);
}

/// P7 — a contextual offer whose origin buffer is no longer active is dropped, not deferred:
/// switching away before the scan lands means that file's candidate is never offered
/// automatically again this session (manual review still lists it).
#[test]
fn probe_p7_contextual_offer_dropped_when_origin_not_active() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("a.md");
    std::fs::write(&path, "disk").unwrap();
    let slot = RecoverySlot::new();
    let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "probe".into(), 0, Some(TaggedPath::from_path(&path)), None);
    store::checkpoint(&RealFs, root.path(), &slot, &record, "unsaved a").unwrap();
    drop(slot);
    let mut e = Editor::new_from_text("disk", Some(path.clone()), (80, 24));
    e.recovery.set_root(root.path().to_owned());
    let a = e.active().id;
    let ex = DeferredRecoveryExecutor::default();
    let clock = TestClock(0);
    let (tx, _rx) = std::sync::mpsc::channel();
    let fs = test_fs();
    crate::recovery_flow::opened(&mut e, a, &path);
    crate::app::finish_iteration(&mut e, &ex, &clock, &tx, &fs); // dispatches the scan
    // User switches to another buffer before the scan lands.
    let other = e.alloc_id();
    e.buffers.push(crate::editor::Buffer::from_text(other, "other", None, (80, 24)));
    e.switch_to_index(1);
    crate::jobs_apply::apply_job_outcome(ex.run_next(), &mut e, &ex, &clock, &tx, &fs);
    crate::app::finish_iteration(&mut e, &ex, &clock, &tx, &fs);
    assert!(e.recovery_picker.is_none());
    // Switch back: nothing is pending and nothing is offered.
    e.switch_to_index(0);
    for _ in 0..3 { crate::app::finish_iteration(&mut e, &ex, &clock, &tx, &fs); }
    println!("P7 picker after switching back: {:?}; jobs pending={}",
        e.recovery_picker.is_some(), ex.pending_len());
    assert!(e.recovery_picker.is_none());
    assert_eq!(ex.pending_len(), 0);
}
