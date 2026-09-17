//! Fable polish re-review probes (2026-09-15). Compiled against the real branch through a
//! TEMPORARY `#[cfg(test)] #[path = "../../review-probes/probes.rs"] mod review_probes;` line
//! in `wordcartel/src/lib.rs`, removed afterwards (lib.rs re-hashed against the manifest).
//! Each probe asserts the behavior it observed; `--nocapture` prints the evidence.
use crate::app::Msg;
use crate::editor::Editor;
use crate::recovery_discovery::{Candidate, ScanScope};
use crate::recovery_regressions::DeferredRecoveryExecutor;
use crate::recovery_store::{self, CheckpointRecord, RecoverySlot};
use crate::registry::Ctx;
use crate::test_support::{test_fs, TestClock};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn v2_source(fs: &dyn crate::fsx::Fs, root: &Path, body: &str) -> Candidate {
    let slot = RecoverySlot::new();
    let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "0123456789abcdef".into(), 0, None, None);
    recovery_store::checkpoint(fs, root, &slot, &record, body).unwrap();
    drop(slot);
    crate::test_support::recovery_scan_released(fs, root, &ScanScope::All, 1).remove(0)
}

struct H {
    e: Editor, ex: DeferredRecoveryExecutor, clock: TestClock, tx: std::sync::mpsc::Sender<Msg>,
    fs: Arc<dyn crate::fsx::Fs + Send + Sync>, dir: tempfile::TempDir, source: PathBuf,
}
impl H {
    fn new() -> Self {
        let fs = test_fs(); let dir = tempfile::tempdir().unwrap();
        let candidate = v2_source(&*fs, dir.path(), "original rescued text");
        let source = candidate.source_path.clone();
        let mut e = Editor::new_from_text("disk", None, (88, 24)); e.recovery.set_root(dir.path().to_owned());
        let (tx, _) = std::sync::mpsc::channel();
        let mut h = H { e, ex: Default::default(), clock: TestClock::new(7), tx, fs, dir, source };
        crate::recovery_flow::begin_selected(&mut h.ctx(), vec![candidate]);
        h.job(); // prepare -> install; handoff dispatched, not yet run
        assert!(h.e.active().recovery_request.is_some(), "handoff in flight after install");
        h
    }
    fn ctx(&mut self) -> Ctx<'_> {
        Ctx { editor: &mut self.e, executor: &self.ex, clock: &self.clock, msg_tx: self.tx.clone(), fs: self.fs.clone() }
    }
    fn job(&mut self) {
        crate::jobs_apply::apply_job_outcome(self.ex.run_next(), &mut self.e, &self.ex, &self.clock, &self.tx, &self.fs);
    }
}

#[derive(Debug, PartialEq)]
struct Latched {
    ack_generation: Option<u64>, ack_exists: bool, swapped_version: Option<u64>, last_swap_at: Option<u64>,
    failure: Option<String>, in_flight: bool, request: bool, retry_ready: bool, generation: u64,
    dirty: bool, swap_pending: bool, pending_work: bool, buffers: usize, status: Option<String>,
}
fn latched(e: &Editor) -> Latched {
    let b = e.active();
    Latched {
        ack_generation: b.recovery_ack.as_ref().map(|a| a.generation()),
        ack_exists: b.recovery_ack.as_ref().is_some_and(|a| a.path().exists()),
        swapped_version: b.swapped_version, last_swap_at: b.last_swap_at,
        failure: b.recovery_protection_failure.clone(), in_flight: b.swap_in_flight,
        request: b.recovery_request.is_some(), retry_ready: b.recovery_retry.ready(0),
        generation: b.recovery_generation, dirty: b.document.dirty(),
        swap_pending: crate::swap::pending(b.document.dirty(), b.document.version, b.swapped_version),
        pending_work: crate::recovery_flow::has_pending_work(e), buffers: e.buffers.len(),
        status: e.status().map(|s| s.text().to_owned()),
    }
}

/// P1 — a cancelled handoff (Esc during Opening after install) now ends in exactly the state
/// of an uncancelled one, except that the source is retained (CAS lost to cancel).
#[test]
fn probe_p1_cancelled_latch_equals_uncancelled_state_except_source_retention() {
    let mut a = H::new(); a.job();
    let mut b = H::new(); crate::recovery_flow::cancel_batch(&mut b.e); b.job();
    let (la, lb) = (latched(&a.e), latched(&b.e));
    println!("P1 uncancelled: {la:?}\nP1 cancelled:   {lb:?}");
    assert_eq!(la, lb, "cancelled completion must mirror the uncancelled bookkeeping");
    assert!(la.ack_generation.is_some() && la.ack_exists && !la.swap_pending && !la.pending_work);
    assert!(!a.source.exists(), "uncancelled handoff retires the source");
    assert!(b.source.exists(), "cancelled handoff keeps the source (no retirement authority)");
    assert_eq!(a.ex.pending_len(), 0); assert_eq!(b.ex.pending_len(), 0);
}

/// P2 — after a cancelled latch, Save As retires only the buffer's OWN successor through the
/// save receipt; the retained source is untouched (the latched Ack confers no deletion authority).
#[test]
fn probe_p2_cancelled_latch_then_save_as_retires_own_successor_only() {
    let mut h = H::new(); crate::recovery_flow::cancel_batch(&mut h.e); h.job();
    let successor = h.e.active().recovery_ack.as_ref().unwrap().path().to_owned();
    let target = h.dir.path().join("named.md");
    crate::save::do_save_to(&mut h.ctx(), crate::save::SaveTarget::same(target.clone()), crate::save::SaveMode::SaveAs);
    h.job();
    println!("P2 status: {:?} source={} successor={}", h.e.status().map(|s| (s.kind(), s.text().to_owned())),
        h.source.exists(), successor.exists());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "original rescued text");
    assert!(!h.e.active().document.dirty());
    assert_eq!(h.e.status().unwrap().kind(), crate::status::StatusKind::Info, "no cleanup warning");
    assert!(!successor.exists(), "own successor retired by the strict save receipt");
    assert!(h.source.exists(), "source retained: Save As never retires a foreign v2 source (M-B class, deferred)");
    assert!(h.e.active().recovery_ack.is_none()); assert_eq!(h.ex.pending_len(), 0);
}

/// P3 — an edit made while the cancelled handoff was in flight stays unprotected, is picked up
/// by the ordinary cadence, and a fresh checkpoint (new generation) then covers it.
#[test]
fn probe_p3_edit_during_cancelled_handoff_needs_and_gets_a_fresh_checkpoint() {
    let mut h = H::new(); crate::recovery_flow::cancel_batch(&mut h.e);
    let captured = h.e.active().document.version;
    let len = h.e.active().document.buffer.len();
    crate::transact::submit_transaction(&mut h.e, wordcartel_core::history::Transaction::new(
        wordcartel_core::change::ChangeSet::insert(len, " later edit", len)), &h.clock).unwrap();
    // `app::reduce` arms last_edit_at when a message changed the version (app.rs `if version != before`);
    // the probe submits the transaction directly, so model that one reducer line here.
    h.e.active_mut().last_edit_at = Some(h.clock.0);
    assert_eq!(h.ex.pending_len(), 1, "no second checkpoint while the handoff request is live");
    h.job();
    let b = h.e.active();
    let old_generation = b.recovery_ack.as_ref().unwrap().generation();
    println!("P3 captured={captured} version={} swapped={:?} last_edit_at={:?} last_swap_at={:?}",
        b.document.version, b.swapped_version, b.last_edit_at, b.last_swap_at);
    assert_eq!(b.swapped_version, Some(captured));
    assert!(crate::swap::pending(b.document.dirty(), b.document.version, b.swapped_version), "later edit unprotected");
    assert!(crate::swap::next_deadline_ms(h.clock.0 + 1, b.last_edit_at, b.last_swap_at).is_some(), "cadence armed");
    let id = b.id;
    let outcome = crate::recovery_flow::dispatch_checkpoint(&mut h.ctx(), id);
    assert_eq!(outcome, crate::recovery_flow::DispatchOutcome::Accepted);
    h.job();
    let b = h.e.active();
    let ack = b.recovery_ack.as_ref().unwrap();
    assert!(ack.generation() > old_generation); assert_eq!(b.recovery_generation, ack.generation());
    assert_eq!(b.swapped_version, Some(b.document.version));
    assert_eq!(recovery_store::decode(&std::fs::read(ack.path()).unwrap()).unwrap().1, "original rescued text later edit");
    assert!(h.source.exists(), "ordinary checkpoints never retire the source");
}

/// P4 — a cancelled latch has no effect on a later quit: the drain treats the recovered buffer
/// exactly as after an uncancelled handoff (dirty -> review prompt), and nothing is pending.
#[test]
fn probe_p4_later_quit_identical_after_cancelled_and_uncancelled_handoff() {
    let mut a = H::new(); a.job();
    let mut b = H::new(); crate::recovery_flow::cancel_batch(&mut b.e); b.job();
    let now = 8;
    let observe = |h: &mut H| {
        assert!(!crate::quit::wait_for_saves(&mut h.e, now), "nothing to wait for");
        h.e.quit = true;
        let exited = crate::quit::after_callbacks(&mut h.ctx());
        (exited, h.e.quit, h.e.quit_drain.is_some(), h.e.prompt.is_some(), h.e.status().map(|s| s.text().to_owned()))
    };
    let (ra, rb) = (observe(&mut a), observe(&mut b));
    println!("P4 uncancelled quit: {ra:?}\nP4 cancelled quit:   {rb:?}");
    assert_eq!(ra, rb);
    // after_callbacks returns true = keep running (quit not confirmed); the drain shows the review prompt.
    assert!(ra.0 && !ra.1 && ra.3, "a dirty recovered buffer is reviewed, not silently exited");
}

/// P5 — closing the recovered buffer through the REAL close path while its handoff is in flight:
/// batch cancelled, no latch onto a dead slot, no panic, source AND orphan successor both remain.
#[test]
fn probe_p5_real_close_during_handoff_latches_nothing_and_keeps_both_copies() {
    let mut h = H::new();
    let recovered = h.e.active().id;
    let owner_dir_count = |root: &Path| std::fs::read_dir(root.join("recovery-v2")).unwrap().count();
    crate::workspace::close_buffer_now(&mut h.e, recovered);
    assert!(h.e.by_id(recovered).is_none()); assert_eq!(h.e.buffers.len(), 1);
    let history = h.e.status_history().entries().len();
    h.job();
    println!("P5 owner dirs after close+late completion: {}", owner_dir_count(h.dir.path()));
    assert!(h.source.exists(), "cancelled retirement");
    assert_eq!(owner_dir_count(h.dir.path()), 2, "source owner + orphaned successor owner (M-B class, deferred)");
    assert!(h.e.active().recovery_ack.is_none(), "the surviving 'disk' buffer receives no foreign Ack");
    assert_eq!(h.e.status_history().entries().len(), history, "no late status");
    assert!(!crate::recovery_flow::has_pending_work(&h.e)); assert_eq!(h.ex.pending_len(), 0);
}

fn rendered(e: &mut Editor) -> Vec<String> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(88, 24)).unwrap();
    terminal.draw(|frame| crate::render::render(frame, e)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..24).map(|y| (0..88).map(|x| buffer[(x, y)].symbol()).collect()).collect()
}

/// P6 — a REAL busy row (live in-process owner lease, produced by the actual scan) through the
/// actual manual-review path: readable label, no internal filename/raw error on the list row,
/// path still inspectable, no ownership/cleanliness claims, unselectable, filtered from auto-offer.
#[test]
fn probe_p6_real_busy_row_wording_selection_and_auto_offer_filter() {
    let fs = test_fs(); let dir = tempfile::tempdir().unwrap();
    let slot = RecoverySlot::new(); // held for the whole probe: the lease stays live
    let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "0123456789abcdef".into(), 0, None, None);
    recovery_store::checkpoint(&*fs, dir.path(), &slot, &record, "held by a live owner").unwrap();
    let mut e = Editor::new_from_text("disk", None, (88, 24)); e.recovery.set_root(dir.path().to_owned());
    let ex = DeferredRecoveryExecutor::default(); let clock = TestClock::new(0); let (tx, _) = std::sync::mpsc::channel::<Msg>();
    let mut ctx = Ctx { editor: &mut e, executor: &ex, clock: &clock, msg_tx: tx.clone(), fs: fs.clone() };
    crate::recovery_flow::review(&mut ctx);
    crate::jobs_apply::apply_job_outcome(ex.run_next(), &mut e, &ex, &clock, &tx, &fs);
    crate::recovery_flow::after_callbacks(&mut Ctx { editor: &mut e, executor: &ex, clock: &clock, msg_tx: tx.clone(), fs: fs.clone() });
    assert!(e.recovery_picker.is_some(), "manual review lists the busy row");
    let lines = rendered(&mut e);
    for l in &lines[1..23] { println!("P6 |{l}"); }
    let row = lines.iter().find(|l| l.contains("busy: in use")).expect("busy row painted");
    assert!(row.contains("Recovery files in use"), "{row}");
    assert!(!row.contains("checkpoint.wcr") && !row.contains("recovery record is active"), "{row}");
    let picker = lines[1..23].join("\n");
    assert!(picker.contains("An editor is using these recovery files"), "{picker}");
    assert!(picker.contains("checkpoint.wcr"), "path remains inspectable in details: {picker}");
    let lower = picker.to_lowercase();
    for claim in ["this session", "another editor", "owns", "clean", "no checkpoint", "empty", "current document"] {
        assert!(!lower.contains(claim), "wording must not claim {claim:?}: {picker}");
    }
    // Space + Enter through the real intercept: nothing selectable, nothing dispatched.
    let reg = crate::registry::Registry::builtins(); let km = crate::keymap::KeyTrie::default();
    let dc = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clock, msg_tx: &tx, fs: &fs };
    for code in [crossterm::event::KeyCode::Char(' '), crossterm::event::KeyCode::Enter] {
        crate::recovery_picker::intercept(Msg::Input(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE))), &mut e, &dc);
    }
    assert!(e.recovery_picker.is_some()); assert_eq!(e.buffers.len(), 1); assert_eq!(ex.pending_len(), 0);
    assert!(rendered(&mut e).join("\n").contains("Select a recovery file"), "zero selection stays open");
    // Automatic offer path filters the busy row entirely.
    let mut e2 = Editor::new_from_text("disk", None, (88, 24)); e2.recovery.set_root(dir.path().to_owned());
    crate::recovery_flow::bootstrap(&mut Ctx { editor: &mut e2, executor: &ex, clock: &clock, msg_tx: tx.clone(), fs: fs.clone() });
    crate::jobs_apply::apply_job_outcome(ex.run_next(), &mut e2, &ex, &clock, &tx, &fs);
    crate::recovery_flow::after_callbacks(&mut Ctx { editor: &mut e2, executor: &ex, clock: &clock, msg_tx: tx.clone(), fs: fs.clone() });
    assert!(e2.recovery_picker.is_none(), "busy rows are never auto-offered");
    drop(slot);
}
