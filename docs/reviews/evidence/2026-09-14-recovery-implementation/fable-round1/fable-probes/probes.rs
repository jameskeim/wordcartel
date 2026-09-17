//! Fable whole-branch review probes (2026-09-14 recovery safety). Compiled against the
//! real branch via a TEMPORARY `mod` line in lib.rs that is reverted byte-exactly afterwards.
//! Each probe documents a suspected behaviour; assertions encode what the code DOES, so a
//! passing probe CONFIRMS the finding (see FABLE_REVIEW.md).
#![allow(clippy::all)]
use crate::editor::Editor;
use crate::fsx::{Fs, RealFs};
use crate::jobs::{Executor, InlineExecutor};
use crate::registry::Ctx;
use crate::test_support::{TestClock, test_fs};
use std::sync::Arc;

fn ctx_call<R>(e: &mut Editor, ex: &dyn Executor, fs: &Arc<dyn Fs + Send + Sync>, now: u64,
    f: impl FnOnce(&mut Ctx) -> R) -> R
{
    let (tx, _rx) = std::sync::mpsc::channel();
    let clock = TestClock(now);
    f(&mut Ctx { editor: e, executor: ex, clock: &clock, msg_tx: tx, fs: fs.clone() })
}
fn settle(e: &mut Editor, ex: &dyn Executor, fs: &Arc<dyn Fs + Send + Sync>, now: u64) {
    let (tx, _rx) = std::sync::mpsc::channel();
    for _ in 0..6 {
        for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, e, ex, &TestClock(now), &tx, fs); }
        crate::app::finish_iteration(e, ex, &TestClock(now), &tx, fs);
    }
}

/// P1 — the 5 s "pending work" timeout is NOT quit-scoped: an ORDINARY idle checkpoint that
/// takes longer than 5 s is cancelled, its successful Ack is discarded, a sticky warning is
/// shown, and the buffer re-checkpoints on the next idle tick (a loop for slow disks).
#[test]
fn probe_p1_ordinary_checkpoint_timeout_discards_ack_and_rechecksums() {
    let root = tempfile::tempdir().unwrap();
    let mut e = Editor::new_from_text("draft", None, (80, 24));
    e.recovery.set_root(root.path().to_owned());
    e.active_mut().document.saved_version = None;
    e.active_mut().last_edit_at = Some(0);
    let ex = crate::recovery_regressions::DeferredRecoveryExecutor::default();
    let fs = test_fs();
    let id = e.active().id;
    let out = ctx_call(&mut e, &ex, &fs, 0, |c| crate::recovery_flow::dispatch_checkpoint(c, id));
    assert_eq!(out, crate::recovery_flow::DispatchOutcome::Accepted);
    assert!(!crate::recovery_flow::has_pending_work(&e), "not a quit blocker …");
    assert_eq!(crate::timers::next_wake(&e, 0), Some(5_000), "… yet it arms a 5 s wake");
    // No quit is in progress. Time passes; the worker is slow.
    crate::timers::pre_recv(&mut e, 5_010);
    assert_eq!(e.status_text(), "Recovery IO is still pending", "sticky warning without any quit");
    // Worker now finishes successfully; the Ack is thrown away.
    let (tx, _rx) = std::sync::mpsc::channel();
    crate::jobs_apply::apply_job_outcome(ex.run_next(), &mut e, &ex, &TestClock(5_020), &tx, &fs);
    assert!(e.active().recovery_ack.is_none(), "successful checkpoint Ack discarded");
    assert_eq!(e.active().swapped_version, None, "latch never set → still 'pending'");
    let ack_on_disk = std::fs::read_dir(root.path().join("recovery-v2")).unwrap().count();
    assert_eq!(ack_on_disk, 1, "…although the record IS durably on disk");
    // And the swap timer immediately wants another checkpoint of the identical content.
    assert_eq!(crate::timers::next_wake(&e, 5_020), Some(5_020), "immediate re-checkpoint wake");
}

/// P2 — first Save As of a recovered document (the D2 happy path) leaves its handoff
/// checkpoint on disk with association None; the save reports "retained"; after exit the
/// record is a fresh, non-busy, automatically-offered candidate on the next launch.
#[test]
fn probe_p2_first_save_as_of_recovered_doc_leaves_a_reoffered_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let source = state.join("recovered-crash.md");
    std::fs::write(&source, "rescued prose").unwrap();
    let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
    let rows = crate::recovery_discovery::scan(&*fs, &state, &crate::recovery_discovery::ScanScope::All).unwrap();
    let ex = InlineExecutor::default();
    let target = root.path().join("saved.md");
    let record_path = {
        let mut e = Editor::new_from_text("disk", None, (80, 24));
        e.recovery.set_root(state.clone());
        ctx_call(&mut e, &ex, &fs, 1, |c| crate::recovery_flow::begin_selected(c, rows));
        settle(&mut e, &ex, &fs, 1);
        assert_eq!(e.active().document.buffer.to_string(), "rescued prose");
        let ack = e.active().recovery_ack.clone().expect("handoff protected the import");
        // First Save As (clean content, no further edits).
        ctx_call(&mut e, &ex, &fs, 2, |c| { crate::save::do_save_to(c,
            crate::save::SaveTarget::same(target.clone()), crate::save::SaveMode::SaveAs); });
        settle(&mut e, &ex, &fs, 2);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "rescued prose");
        assert!(!e.active().document.dirty());
        assert!(e.status_text().contains("retained"), "status: {}", e.status_text());
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert!(ack.record_path().exists(), "handoff checkpoint survives the successful Save As");
        assert!(ex.drain().is_empty(), "no association checkpoint is queued for a clean rekey");
        ack.record_path().to_owned()
    };
    // "Next launch": the editor is gone, the owner lease is released.
    let rows = crate::recovery_discovery::scan(&*fs, &state, &crate::recovery_discovery::ScanScope::All).unwrap();
    let row = rows.iter().find(|r| r.source_path == record_path).expect("stale record is a candidate");
    assert!(!row.busy && row.unavailable.is_none() && row.token.is_some(), "…and an OFFERABLE one");
    assert_eq!(row.preview, "rescued prose");
    // Bootstrap in a fresh editor auto-offers it (no dismissal survives a restart).
    let mut e2 = Editor::new_from_text("\n", None, (80, 24));
    e2.recovery.set_root(state.clone());
    ctx_call(&mut e2, &ex, &fs, 3, |c| crate::recovery_flow::bootstrap(c));
    settle(&mut e2, &ex, &fs, 3);
    assert!(e2.recovery_picker.is_some(), "picker pops on every launch until the doc is edited+saved");
}

/// P3 — one corrupt legacy entry in the state dir (no token → never dismissable, and not
/// cleanable) makes the recovery picker open on EVERY contextual file open.
#[test]
fn probe_p3_corrupt_entry_interrupts_every_open_and_is_uncleanable() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::write(state.join("junk.swp"), b"not a swap header\xff\xfe").unwrap();
    let a = root.path().join("a.md"); std::fs::write(&a, "a").unwrap();
    let b = root.path().join("b.md"); std::fs::write(&b, "b").unwrap();
    let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
    let ex = InlineExecutor::default();
    let mut e = Editor::new_from_text("other", None, (80, 24));
    e.recovery.set_root(state.clone()); e.resume_enabled = false;
    crate::workspace::open_as_new_buffer(&mut e, &*fs, &a);
    settle(&mut e, &ex, &fs, 1);
    assert!(e.recovery_picker.is_some(), "open A → picker (only an unavailable row)");
    crate::recovery_picker::close(&mut e);
    crate::workspace::open_as_new_buffer(&mut e, &*fs, &b);
    settle(&mut e, &ex, &fs, 2);
    assert!(e.recovery_picker.is_some(), "open B → picker AGAIN; dismissal cannot suppress a token-less row");
    crate::recovery_picker::close(&mut e);
    // Reopen A a second time (new buffer): again.
    crate::workspace::open_as_new_buffer(&mut e, &*fs, &a);
    settle(&mut e, &ex, &fs, 3);
    assert!(e.recovery_picker.is_some());
    let protected = crate::swap::open_swap_paths(&e, &*fs).unwrap();
    let cleanable = crate::swap::cleanable_recovery_files(&*fs, &state, &protected);
    assert!(cleanable.is_empty(), "Clean Recovery Files offers nothing: the interruption is permanent");
}

/// P4 — a PERMANENT strict-sync failure (new failure class: ancestor fsync) makes the idle
/// checkpoint retry with a zero-delay wake: dispatch → fail → immediate wake → dispatch …
#[test]
fn probe_p4_permanent_strict_sync_failure_spins_zero_delay_retries() {
    struct Counting { inner: InlineExecutor, n: std::cell::Cell<usize> }
    impl Executor for Counting {
        fn try_dispatch(&self, job: crate::jobs::Job) -> Result<(), crate::jobs::DispatchError> {
            if matches!(job.kind, crate::jobs::JobKind::Recovery(_)) { self.n.set(self.n.get() + 1); }
            self.inner.try_dispatch(job)
        }
        fn drain(&self) -> Vec<crate::jobs::JobOutcome> { self.inner.drain() }
    }
    let root = tempfile::tempdir().unwrap();
    let fs: Arc<dyn Fs + Send + Sync> = Arc::new(crate::test_support::FaultFs::new(crate::test_support::FaultAt::StrictDirSync));
    let reg = crate::registry::Registry::builtins();
    let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
    let ex = Counting { inner: InlineExecutor::default(), n: std::cell::Cell::new(0) };
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut e = Editor::new_from_text("start\n", None, (80, 24));
    e.recovery.set_root(root.path().to_owned());
    crate::app::reduce(crate::test_support::press(crossterm::event::KeyCode::Char('x'), crossterm::event::KeyModifiers::NONE),
        &mut e, &reg, &km, &ex, &TestClock(0), &tx, &fs);
    let mut zero_delay_wakes = 0;
    let mut now = crate::swap::T_IDLE_MS + 1;
    for _ in 0..50 {
        crate::app::reduce(crate::app::Msg::Tick, &mut e, &reg, &km, &ex, &TestClock(now), &tx, &fs);
        // What the real loop would do next: wake = next_wake(now); Some(now) means recv_timeout(0).
        if crate::timers::next_wake(&e, now) == Some(now) { zero_delay_wakes += 1; }
        now += 1; // one millisecond later (the failing IO is instant here)
    }
    assert!(e.active().recovery_protection_failure.is_some());
    assert_eq!(ex.n.get(), 50, "50 ticks → 50 checkpoint dispatches (one per loop iteration)");
    assert_eq!(zero_delay_wakes, 50, "every iteration re-arms an already-due wake");
}

/// P5 — permission drift on the protocol dir is not self-healed: every scan shows an
/// unavailable row (→ P3 interruption) and every checkpoint fails (→ P4 spin).
#[cfg(unix)]
#[test]
fn probe_p5_protocol_dir_mode_drift_is_permanent() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let slot = crate::recovery_store::RecoverySlot::new();
    let record = crate::recovery_store::CheckpointRecord::new(slot.reserve_generation().unwrap(), "l".into(), 1, None, None);
    crate::recovery_store::checkpoint(&RealFs, &state, &slot, &record, "x").unwrap();
    let protocol = RealFs.canonicalize_existing(&state).unwrap().join("recovery-v2");
    std::fs::set_permissions(&protocol, std::fs::Permissions::from_mode(0o750)).unwrap();
    let rows = crate::recovery_discovery::scan(&RealFs, &state, &crate::recovery_discovery::ScanScope::All).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].unavailable.is_some() && rows[0].token.is_none());
    let fresh = crate::recovery_store::RecoverySlot::new();
    let record = crate::recovery_store::CheckpointRecord::new(fresh.reserve_generation().unwrap(), "l".into(), 1, None, None);
    assert!(crate::recovery_store::checkpoint(&RealFs, &state, &fresh, &record, "y").is_err());
    assert_eq!(std::fs::metadata(&protocol).unwrap().permissions().mode() & 0o777, 0o750, "nobody restores 0700");
}

/// P6 — Save As from A to B while CLEAN leaves the A-associated checkpoint (spec-conformant,
/// but note the receipt refuses although the saved bytes are byte-identical).
#[test]
fn probe_p6_clean_save_as_retains_old_association_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a.md"); std::fs::write(&a, "old").unwrap();
    let b = root.path().join("b.md");
    let mut e = Editor::new_from_text("new bytes", Some(a.clone()), (80, 24));
    e.recovery.set_root(root.path().join("state"));
    e.active_mut().document.saved_version = None;
    let ex = InlineExecutor::default();
    let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
    let id = e.active().id;
    ctx_call(&mut e, &ex, &fs, 1, |c| { crate::recovery_flow::dispatch_checkpoint(c, id); });
    settle(&mut e, &ex, &fs, 1);
    let rec = e.active().recovery_ack.as_ref().unwrap().record_path().to_owned();
    ctx_call(&mut e, &ex, &fs, 2, |c| { crate::save::do_save_to(c,
        crate::save::SaveTarget::same(b.clone()), crate::save::SaveMode::SaveAs); });
    settle(&mut e, &ex, &fs, 2);
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "new bytes");
    assert!(rec.exists());
    let bytes = std::fs::read(&rec).unwrap();
    let (m, body) = crate::recovery_store::decode(&bytes).unwrap();
    assert_eq!(body, "new bytes");
    assert_eq!(m.record().association().unwrap().local_path().unwrap(), std::fs::canonicalize(&a).unwrap());
    assert!(e.status_text().contains("retained"));
}
