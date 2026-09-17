//! Shared deterministic recovery job controls and end-to-end regression requirements.

use std::cell::RefCell;
use std::collections::VecDeque;
use crate::jobs::{DispatchError, Executor, Job, JobOutcome};

/// Separates FIFO worker execution from foreground outcome application.
#[derive(Default)]
pub(crate) struct DeferredRecoveryExecutor { pending: RefCell<VecDeque<Job>> }

impl Executor for DeferredRecoveryExecutor {
    fn try_dispatch(&self, job: Job) -> Result<(), DispatchError> {
        self.pending.borrow_mut().push_back(job); Ok(())
    }
    fn drain(&self) -> Vec<JobOutcome> { Vec::new() }
}

impl DeferredRecoveryExecutor {
    pub(crate) fn run_next(&self) -> JobOutcome {
        let job = self.pending.borrow_mut().pop_front().expect("queued recovery job");
        job.execute()
    }
    pub(crate) fn pending_len(&self) -> usize { self.pending.borrow().len() }
}

/// Models immediate queue rejection; captured ownership is released before return.
pub(crate) struct RejectingRecoveryExecutor;
impl Executor for RejectingRecoveryExecutor {
    fn try_dispatch(&self, job: Job) -> Result<(), DispatchError> {
        drop(job); Err(DispatchError::Closed)
    }
    fn drain(&self) -> Vec<JobOutcome> { Vec::new() }
}


#[cfg(test)]
mod tests {
    /// Reacquire only after a test deliberately dropped its last owner. Parallel fork/exec
    /// can briefly retain CLOEXEC descriptors; this must never weaken a held-lease assertion.
    pub(super) fn released_lock(path: &std::path::Path) -> Box<dyn crate::fsx::RecoveryLease> {
        use crate::fsx::Fs;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match crate::fsx::RealFs.try_recovery_lock(path, false) {
                Ok(guard) => return guard,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline =>
                    std::thread::sleep(std::time::Duration::from_millis(2)),
                Err(e) => panic!("released lease unavailable: {e}"),
            }
        }
    }

    use super::*;
    use crate::editor::BufferId;
    use crate::jobs::{JobKind, ResultClass};
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

    #[test]
    fn recovery_dispatch_harness_defers_execution_and_preserves_panic_transport() {
        let ex = DeferredRecoveryExecutor::default();
        let ran = Arc::new(AtomicBool::new(false));
        let worker = ran.clone();
        ex.try_dispatch(Job { buffer_id: BufferId(1), version: 0,
            kind: JobKind::Recovery(crate::jobs::RecoveryRequestId::for_test(1)), class: ResultClass::Durability, save_request: None,
            run: Box::new(move || { worker.store(true, Ordering::SeqCst); panic!("test job") }),
        }).unwrap();
        assert_eq!(ex.pending_len(), 1);
        assert!(!ran.load(Ordering::SeqCst));
        assert!(ex.drain().is_empty(), "the caller explicitly controls worker completion");
        assert!(matches!(ex.run_next(), JobOutcome::Panicked { buffer_id: BufferId(1), .. }));
        assert!(ran.load(Ordering::SeqCst));
        assert_eq!(ex.pending_len(), 0);
    }

    #[test]
    fn recovery_dispatch_rejecting_harness_releases_captured_ownership() {
        let ex = RejectingRecoveryExecutor;
        let capture = Arc::new(());
        let worker = capture.clone();
        let job = Job { buffer_id: BufferId(1), version: 0,
            kind: JobKind::Recovery(crate::jobs::RecoveryRequestId::for_test(1)), class: ResultClass::Durability, save_request: None,
            run: Box::new(move || { drop(worker); panic!("must not execute") }) };
        assert_eq!(Arc::strong_count(&capture), 2);
        assert_eq!(ex.try_dispatch(job), Err(DispatchError::Closed));
        assert_eq!(Arc::strong_count(&capture), 1);
        assert!(ex.drain().is_empty());
    }
}

#[cfg(test)]
mod store_integration {
    use crate::fsx::{Fs, RealFs};
    use crate::recovery_store::{self as store, CheckpointRecord, RecoverySlot, TaggedPath};
    use std::path::Path;

    fn capture(slot: &RecoverySlot, association: &Path) -> CheckpointRecord {
        CheckpointRecord::new(slot.reserve_generation().unwrap(), "same-lineage".into(), 1,
            Some(TaggedPath::from_path(association)), None)
    }

    #[test]
    fn recovery_ownership_same_path_slots_preserve_each_other_and_queued_lease() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("state");
        let original = temp.path().join("shared.md");
        std::fs::write(&original, "disk").unwrap();
        let a = RecoverySlot::new();
        let b = RecoverySlot::new();
        let aa = store::checkpoint(&RealFs, &root, &a, &capture(&a, &original), "A").unwrap();
        let bb = store::checkpoint(&RealFs, &root, &b, &capture(&b, &original), "B").unwrap();
        assert_ne!(aa.owner(), bb.owner());
        assert_ne!(aa.record_path(), bb.record_path());
        let a_bytes = std::fs::read(aa.record_path()).unwrap();
        assert_eq!(store::decode(&a_bytes).unwrap().1, "A");
        let queued_a = a.clone();
        assert!(a.same_instance(&queued_a));
        drop(a);
        let lock = aa.record_path().parent().unwrap().join("owner.lock");
        assert_eq!(RealFs.try_recovery_lock(&lock, false).err().unwrap().kind(),
            std::io::ErrorKind::WouldBlock, "queued work retains owner lease");
        store::checkpoint(&RealFs, &root, &b, &capture(&b, &original), "new B").unwrap();
        assert_eq!(std::fs::read(aa.record_path()).unwrap(), a_bytes);
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "disk");
        drop(queued_a);
    }

    #[test]
    fn recovery_ownership_late_capture_cannot_replace_newer_same_version_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let slot = RecoverySlot::new();
        let association = temp.path().join("draft.md");
        let old = capture(&slot, &association);
        let new = capture(&slot, &association);
        let ack = store::checkpoint(&RealFs, temp.path(), &slot, &new, "new").unwrap();
        let before = std::fs::read(ack.record_path()).unwrap();
        assert!(store::checkpoint(&RealFs, temp.path(), &slot, &old, "old").is_err());
        assert_eq!(std::fs::read(ack.record_path()).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn recovery_ownership_trusted_root_alias_writes_under_resolved_private_root() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        let alias = temp.path().join("alias");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let slot = RecoverySlot::new();
        let record = capture(&slot, &temp.path().join("draft.md"));
        let ack = store::checkpoint(&RealFs, &alias, &slot, &record, "recover me").unwrap();
        assert!(ack.record_path().starts_with(RealFs.canonicalize_existing(&real).unwrap()));
        let bytes = std::fs::read(ack.record_path()).unwrap();
        assert_eq!(store::decode(&bytes).unwrap().1, "recover me");
        assert!(!alias.join("draft.md").exists());
    }
    fn assert_full_sync_chain(fs: &crate::test_support::FaultFs, ack: &store::CheckpointAck) {
        use crate::test_support::FaultAt;
        let actual: Vec<_> = fs.path_operations().into_iter()
            .filter(|(op, _)| *op == FaultAt::StrictDirSync).map(|(_, path)| path).collect();
        let expected: Vec<_> = ack.record_path().parent().unwrap().ancestors()
            .map(Path::to_path_buf).collect();
        assert_eq!(actual, expected, "first Ack syncs every directory, even when already present");
    }

    #[test]
    fn recovery_ownership_fresh_slot_retries_every_failed_ancestor_barrier() {
        use crate::test_support::{FaultAt, FaultFs};
        let depth_probe = tempfile::tempdir().unwrap();
        let depth = RealFs.canonicalize_existing(depth_probe.path()).unwrap().ancestors().count() + 3;
        for nth in 1..=depth {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("state");
            let original = temp.path().join("draft.md");
            let first = RecoverySlot::new();
            let failed = FaultFs::on_occurrence(FaultAt::StrictDirSync, nth);
            assert!(store::checkpoint(&failed, &root, &first, &capture(&first, &original), "first").is_err());
            assert_eq!(failed.operations().iter().filter(|&&op| op == FaultAt::StrictDirSync).count(), nth);
            drop(first);
            let fresh = RecoverySlot::new();
            let retry = FaultFs::on_occurrence(FaultAt::StrictDirSync, usize::MAX);
            let ack = store::checkpoint(&retry, &root, &fresh, &capture(&fresh, &original), "fresh").unwrap();
            assert_full_sync_chain(&retry, &ack);
            assert_eq!(store::decode(&std::fs::read(ack.record_path()).unwrap()).unwrap().1, "fresh");
        }
    }

    #[test]
    fn recovery_ownership_partial_mkdir_retries_keep_full_sync_obligation() {
        use crate::test_support::{FaultAt, FaultFs};
        for nth in 1..=3 {
            for new_slot in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().join("state");
                let original = temp.path().join("draft.md");
                let first = RecoverySlot::new();
                let failed = FaultFs::on_occurrence(FaultAt::CreateDir, nth);
                assert!(store::checkpoint(&failed, &root, &first, &capture(&first, &original), "first").is_err());
                assert_eq!(failed.operations().iter().filter(|&&op| op == FaultAt::CreateDir).count(), nth);
                let slot = if new_slot { RecoverySlot::new() } else { first.clone() };
                drop(first);
                let retry = FaultFs::on_occurrence(FaultAt::StrictDirSync, usize::MAX);
                let ack = store::checkpoint(&retry, &root, &slot, &capture(&slot, &original), "protected").unwrap();
                assert_full_sync_chain(&retry, &ack);
            }
        }
    }

}

#[cfg(test)]
mod discovery_integration {
    use crate::fsx::{Fs, RealFs};
    use crate::recovery_store::{self as store, CheckpointRecord, RecoverySlot, TaggedPath};
    use crate::recovery_discovery::{self as discovery, ScanScope, PrepareOutcome};
    use crate::test_support::{FaultAt, FaultFs};
    use std::path::Path;

    fn checkpoint(root: &Path, slot: &RecoverySlot, association: Option<&Path>,
        provenance: Option<&Path>, body: &str) -> store::CheckpointAck
    {
        let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "lineage".into(), 1,
            association.map(TaggedPath::from_path), provenance.map(TaggedPath::from_path));
        store::checkpoint(&RealFs, root, slot, &record, body).unwrap()
    }
    fn scan_released(root: &Path, scope: &ScanScope, count: usize) -> Vec<discovery::Candidate> {
        crate::test_support::recovery_scan_released(&RealFs, root, scope, count)
    }
    fn prepare_released(root: &Path, row: &discovery::Candidate) -> PrepareOutcome {
        crate::test_support::recovery_prepare_released(&RealFs, root, row).unwrap()
    }
    fn assert_released(record: &Path) {
        drop(super::tests::released_lock(&record.parent().unwrap().join("owner.lock")));
    }

    #[test]
    fn recovery_discovery_prepare_failures_release_source_and_preserve_bytes() {
        let root = tempfile::tempdir().unwrap();
        let slot = RecoverySlot::new();
        let ack = checkpoint(root.path(), &slot, None, None, "only copy");
        drop(slot);
        let row = scan_released(root.path(), &ScanScope::All, 1).remove(0);
        let original = std::fs::read(ack.path()).unwrap();
        for fault in [FaultAt::RecoveryOpen, FaultAt::RecoveryRead] {
            assert!(discovery::prepare(&FaultFs::new(fault), root.path(), &row).is_err());
            assert_released(ack.path());
            assert_eq!(std::fs::read(ack.path()).unwrap(), original);
        }
        std::fs::write(ack.path(), "corrupt record").unwrap();
        assert!(discovery::prepare(&RealFs, root.path(), &row).is_err());
        assert_released(ack.path());
        assert_eq!(std::fs::read_to_string(ack.path()).unwrap(), "corrupt record");
    }

    #[test]
    fn recovery_discovery_changed_v2_requires_reselection_and_releases_lease() {
        let root = tempfile::tempdir().unwrap();
        let slot = RecoverySlot::new();
        let ack = checkpoint(root.path(), &slot, None, None, "old");
        drop(slot);
        let row = scan_released(root.path(), &ScanScope::All, 1).remove(0);
        let lock = ack.path().parent().unwrap().join("owner.lock");
        let guard = super::tests::released_lock(&lock);
        let record = CheckpointRecord::new(ack.generation() + 1, "lineage".into(), 1, None, None);
        let meta = store::Metadata::new(ack.owner().into(), record, 3).unwrap();
        std::fs::write(ack.path(), store::encode(&meta, "new").unwrap()).unwrap();
        drop(guard);
        match prepare_released(root.path(), &row) {
            PrepareOutcome::Changed(updated) => {
                assert_ne!(updated.token, row.token);
                assert_eq!(updated.preview, "new");
            },
            PrepareOutcome::Ready(_) => panic!("changed content must not silently import"),
        }
        assert_released(ack.path());
        assert_eq!(store::decode(&std::fs::read(ack.path()).unwrap()).unwrap().1, "new");
    }

    #[test]
    fn recovery_discovery_empty_body_is_ready_and_lease_moves_with_preparation() {
        let root = tempfile::tempdir().unwrap();
        let slot = RecoverySlot::new();
        let ack = checkpoint(root.path(), &slot, None, None, "");
        drop(slot);
        let row = scan_released(root.path(), &ScanScope::All, 1).remove(0);
        assert!(row.unavailable.is_none());
        let PrepareOutcome::Ready(prepared) = prepare_released(root.path(), &row)
            else { panic!("unchanged empty record is recoverable"); };
        let (_, body, lease) = prepared.into_parts();
        assert!(body.is_empty());
        assert_eq!(RealFs.try_recovery_lock(&ack.path().parent().unwrap().join("owner.lock"), false)
            .err().unwrap().kind(), std::io::ErrorKind::WouldBlock);
        drop(lease);
        assert_released(ack.path());
        assert!(ack.path().exists());
    }

    #[test]
    fn recovery_discovery_current_association_takes_precedence_over_provenance() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a.md");
        let b = root.path().join("b.md");
        std::fs::write(&a, "disk A").unwrap();
        std::fs::write(&b, "disk B").unwrap();
        let unnamed = RecoverySlot::new();
        let named = RecoverySlot::new();
        let aa = checkpoint(root.path(), &unnamed, None, Some(&a), "recovered A");
        let bb = checkpoint(root.path(), &named, Some(&b), Some(&a), "now B");
        drop((unnamed, named));
        let rows = scan_released(root.path(), &ScanScope::Associated(a), 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source_path, aa.path());
        let rows = scan_released(root.path(), &ScanScope::Associated(b), 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source_path, bb.path());
    }

    #[test]
    fn recovery_discovery_exact_body_cap_then_oversize_and_io_failures_are_distinct() {
        let root = tempfile::tempdir().unwrap();
        let slot = RecoverySlot::new();
        let body = "x".repeat(crate::limits::MAX_OPEN_BYTES as usize);
        let ack = checkpoint(root.path(), &slot, None, None, &body);
        drop((body, slot));
        let rows = scan_released(root.path(), &ScanScope::All, 1);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].unavailable.is_none());
        assert_eq!(rows[0].preview.len(), 240);
        std::fs::OpenOptions::new().write(true).open(ack.path()).unwrap()
            .set_len(store::MAX_RECORD_BYTES + 1).unwrap();
        let rows = scan_released(root.path(), &ScanScope::All, 1);
        assert!(rows[0].unavailable.as_ref().unwrap().contains("size limit"));
        let rows = discovery::scan(&FaultFs::new(FaultAt::RecoveryRead), root.path(), &ScanScope::All).unwrap();
        assert!(rows[0].unavailable.as_ref().unwrap().contains("injected"));
        assert!(ack.path().exists());
    }

    #[test]
    fn recovery_discovery_empty_owner_tombstone_is_not_a_scan_error() {
        let root = tempfile::tempdir().unwrap();
        let slot = RecoverySlot::new();
        let ack = checkpoint(root.path(), &slot, None, None, "saved elsewhere");
        drop(slot);
        // Model an authorized successful save that removed its checkpoint, never the lock inode.
        std::fs::remove_file(ack.path()).unwrap();
        assert!(scan_released(root.path(), &ScanScope::All, 0).is_empty());
        assert!(ack.path().parent().unwrap().join("owner.lock").exists());
    }
}

#[cfg(test)]
mod save_integration {
    use crate::editor::Editor;
    use crate::fsx::{Fs, RealFs, FileStat, WriteSync, DirListing, RecoveryRead};
    use crate::jobs::{Executor, InlineExecutor, JobOutcome};
    use crate::registry::Ctx;
    use crate::test_support::TestClock;
    use std::{path::Path, sync::Arc};

    struct PanicCleanupFs;
    impl Fs for PanicCleanupFs {
        fn create_excl(&self, p: &Path, m: u32) -> std::io::Result<Box<dyn WriteSync>> { RealFs.create_excl(p, m) }
        fn existing_mode(&self, p: &Path) -> Option<u32> { RealFs.existing_mode(p) }
        fn read_capped(&self, p: &Path, n: u64) -> std::io::Result<Option<Vec<u8>>> { RealFs.read_capped(p, n) }
        fn stat(&self, p: &Path) -> std::io::Result<FileStat> { RealFs.stat(p) }
        fn rename(&self, a: &Path, b: &Path) -> std::io::Result<()> { RealFs.rename(a, b) }
        fn sync_dir(&self, p: &Path) -> std::io::Result<()> { RealFs.sync_dir(p) }
        fn remove_file(&self, p: &Path) -> std::io::Result<()> { RealFs.remove_file(p) }
        fn list_dir(&self, p: &Path, n: Option<usize>) -> std::io::Result<DirListing> { RealFs.list_dir(p, n) }
        fn open_regular_nofollow(&self, _: &Path) -> std::io::Result<Box<dyn RecoveryRead>> { panic!("cleanup probe"); }
    }

    fn checkpoint(e: &mut Editor, ex: &dyn Executor, fs: &Arc<dyn Fs + Send + Sync>) {
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::swap::dispatch_swap_write(&mut Ctx { editor: e, executor: ex,
            clock: &TestClock(3000), msg_tx: tx.clone(), fs: fs.clone() });
        for result in ex.drain() {
            crate::jobs_apply::apply_job_outcome(result, e, ex, &TestClock(3000), &tx, fs);
        }
    }

    #[test]
    fn recovery_save_cleanup_panic_does_not_turn_successful_write_into_failed_save() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("draft.md");
        std::fs::write(&path, "disk").unwrap();
        let mut e = Editor::new_from_text("new bytes", Some(path.clone()), (80,24));
        e.recovery.set_root(root.path().join("state"));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        checkpoint(&mut e, &ex, &fs);
        let record = e.active().recovery_ack.as_ref().unwrap().record_path().to_owned();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(PanicCleanupFs);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::save::do_save_to(&mut Ctx { editor: &mut e, executor: &ex, clock: &TestClock(4000),
            msg_tx: tx.clone(), fs: fs.clone() }, crate::save::SaveTarget::same(path.clone()), crate::save::SaveMode::Normal);
        let outcomes = ex.drain();
        assert!(matches!(outcomes.as_slice(), [JobOutcome::Done(_)]), "cleanup failure is not save failure");
        for outcome in outcomes { crate::jobs_apply::apply_job_outcome(outcome, &mut e, &ex, &TestClock(4000), &tx, &fs); }
        assert_eq!(std::fs::read_to_string(path).unwrap(), "new bytes");
        assert!(!e.active().document.dirty());
        assert!(e.status_text().contains("Saved"));
        assert!(e.status_text().contains("cleanup interrupted"));
        assert!(record.exists());
    }
    fn save(e: &mut Editor, ex: &dyn Executor, fs: &Arc<dyn Fs + Send + Sync>,
        path: &Path, mode: crate::save::SaveMode)
    {
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::save::do_save_to(&mut Ctx { editor: e, executor: ex, clock: &TestClock(4000),
            msg_tx: tx.clone(), fs: fs.clone() }, crate::save::SaveTarget::same(path.to_owned()), mode);
        for outcome in ex.drain() {
            crate::jobs_apply::apply_job_outcome(outcome, e, ex, &TestClock(4000), &tx, fs);
        }
    }

    #[test]
    fn recovery_save_only_removes_own_covered_checkpoint_and_keeps_legacy_copy() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("shared.md");
        std::fs::write(&path, "disk").unwrap();
        let mut e = Editor::new_from_text("A", Some(path.clone()), (80,24));
        e.recovery.set_root(root.path().join("state"));
        e.active_mut().document.version = 1;
        let a = e.active().id;
        let ex = InlineExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        checkpoint(&mut e, &ex, &fs);
        let aa = e.active().recovery_ack.as_ref().unwrap().record_path().to_owned();
        let b = e.alloc_id();
        let mut buffer = crate::editor::Buffer::from_text(b, "B", Some(path.clone()), (80,24));
        buffer.document.version = 1;
        e.buffers.push(buffer);
        e.switch_to_index(1);
        checkpoint(&mut e, &ex, &fs);
        let bb = e.active().recovery_ack.as_ref().unwrap().record_path().to_owned();
        let b_bytes = std::fs::read(&bb).unwrap();
        let legacy = crate::swap::swap_path(Some(&path)).unwrap();
        std::fs::write(&legacy, "unreviewed legacy copy").unwrap();
        e.switch_to_index(0);
        assert_eq!(e.active().id, a);
        save(&mut e, &ex, &fs, &path, crate::save::SaveMode::Normal);
        assert!(!aa.exists());
        assert_eq!(std::fs::read(&bb).unwrap(), b_bytes);
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "unreviewed legacy copy");
        assert!(!e.active().document.dirty());
        assert!(e.by_id(b).unwrap().document.dirty());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "A");
        std::fs::remove_file(legacy).unwrap();
    }

    #[test]
    fn recovery_save_unchanged_target_sync_failure_keeps_checkpoint_and_save_success() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("draft.md");
        std::fs::write(&path, "same bytes").unwrap();
        let mut e = Editor::new_from_text("same bytes", Some(path.clone()), (80,24));
        e.recovery.set_root(root.path().join("state"));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        checkpoint(&mut e, &ex, &fs);
        let record = e.active().recovery_ack.as_ref().unwrap().record_path().to_owned();
        let bytes = std::fs::read(&record).unwrap();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(crate::test_support::FaultFs::new(crate::test_support::FaultAt::RecoverySync));
        save(&mut e, &ex, &fs, &path, crate::save::SaveMode::Normal);
        assert!(!e.active().document.dirty());
        assert!(e.status_text().contains("Saved"));
        assert!(e.status_text().contains("retained"));
        assert_eq!(std::fs::read(record).unwrap(), bytes);
    }

    #[test]
    fn recovery_save_late_completion_cannot_mark_replacement_instance_saved() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("draft.md");
        std::fs::write(&path, "disk").unwrap();
        let mut e = Editor::new_from_text("old instance", Some(path.clone()), (80,24));
        e.recovery.set_root(root.path().join("state"));
        e.active_mut().document.version = 1;
        let ex = super::DeferredRecoveryExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        save(&mut e, &ex, &fs, &path, crate::save::SaveMode::Normal);
        let id = e.active().id;
        let mut replacement = crate::editor::Buffer::from_text(id, "replacement", Some(path.clone()), (80,24));
        replacement.document.version = 2;
        assert!(e.replace_buffer(e.active, replacement));
        let before = e.active().document.saved_version;
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::jobs_apply::apply_job_outcome(ex.run_next(), &mut e, &ex, &TestClock(4000), &tx, &fs);
        assert_eq!(e.active().document.saved_version, before);
        assert_eq!(e.active().document.buffer.to_string(), "replacement");
        assert!(e.status_text().contains("previous document"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "old instance");
    }

    #[test]
    fn recovery_save_as_clean_rekey_retires_exact_owned_checkpoint() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a.md");
        let b = root.path().join("b.md");
        std::fs::write(&a, "disk").unwrap();
        let mut e = Editor::new_from_text("new bytes", Some(a.clone()), (80,24));
        e.recovery.set_root(root.path().join("state"));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        checkpoint(&mut e, &ex, &fs);
        let slot = e.active().recovery_slot.clone();
        let record = e.active().recovery_ack.as_ref().unwrap().record_path().to_owned();
        save(&mut e, &ex, &fs, &b, crate::save::SaveMode::SaveAs);
        assert_eq!(e.active().document.path.as_deref(), Some(b.as_path()));
        assert!(slot.same_instance(&e.active().recovery_slot));
        assert!(!e.active().document.dirty());
        assert!(!record.exists());
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Info);
        assert!(ex.drain().is_empty(), "clean rekey does not create another checkpoint");
    }

    #[test]
    fn recovery_save_and_close_preserves_cleanup_warnings() {
        for case in 0..3 {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("draft.md");
            std::fs::write(&path, "disk").unwrap();
            let mut e = Editor::new_from_text("saved bytes", Some(path.clone()), (80,24));
            e.recovery.set_root(root.path().join("state"));
            e.active_mut().document.version = 1;
            let id = e.active().id;
            let ex = InlineExecutor::default();
            let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
            checkpoint(&mut e, &ex, &fs);
            let (fs, needle): (Arc<dyn Fs + Send + Sync>, &str) = match case {
                0 => (Arc::new(crate::test_support::FaultFs::new(crate::test_support::FaultAt::RecoverySync)), "retained"),
                1 => (Arc::new(crate::test_support::FaultFs::on_occurrence(crate::test_support::FaultAt::StrictDirSync, 2)), "uncertain"),
                _ => (Arc::new(PanicCleanupFs), "interrupted"),
            };
            let (tx, _rx) = std::sync::mpsc::channel();
            let request = crate::save::do_save_to(&mut Ctx { editor: &mut e, executor: &ex,
                clock: &TestClock(4000), msg_tx: tx.clone(), fs: fs.clone() },
                crate::save::SaveTarget::same(path.clone()), crate::save::SaveMode::Normal);
            e.pending_after_save = Some(crate::editor::PendingAfterSave { buffer_id: id, version: 1,
                action: crate::editor::PostSaveAction::CloseBuffer { id }, at_ms: 4000,
                save_request: Some(request), completed: false });
            for outcome in ex.drain() {
                crate::jobs_apply::apply_job_outcome(outcome, &mut e, &ex, &TestClock(4000), &tx, &fs);
            }
            assert!(e.by_id(id).is_none());
            assert_eq!(std::fs::read_to_string(path).unwrap(), "saved bytes");
            assert!(e.status_text().contains("closed"));
            assert!(e.status_text().contains(needle), "cleanup warning survived close: {}", e.status_text());
            assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        }
    }

}

#[cfg(test)]
mod recovery_save_as {
    use crate::editor::{Editor, QuitDrain, QuitMode};
    use crate::recovery_discovery::SelectionToken;
    use crate::recovery_store::TaggedPath;
    use crate::test_support::test_fs;

    fn recovered(original: Option<&std::path::Path>, dir: Option<std::path::PathBuf>) -> Editor {
        let mut e = Editor::new_from_text("recovered work", None, (80,24));
        let b = e.active_mut();
        b.document.saved_version = None;
        b.recovery_source = Some(SelectionToken::Legacy { path: "old.swp".into(), len: 1,
            mtime: None, body_hash: 0 });
        b.recovery_provenance = original.map(TaggedPath::from_path);
        b.recovery_save_dir = dir;
        e
    }

    #[test]
    fn recovery_save_as_prefills_distinct_name_for_manual_and_quit_owned_picker() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("chapter.md");
        for quit_owned in [false, true] {
            let mut e = recovered(Some(&original), Some(root.path().to_owned()));
            let id = e.active().id;
            let (tx, _rx) = std::sync::mpsc::channel();
            let opened = if quit_owned {
                e.quit_drain = Some(QuitDrain::new([id].into(), QuitMode::SaveAll));
                crate::prompts::open_save_as_for_quit(&mut e, &test_fs(), &tx)
            } else { crate::prompts::open_save_as(&mut e, &test_fs(), &tx) };
            assert!(opened);
            let fb = e.file_browser.as_ref().unwrap();
            assert_eq!(fb.mode.filter_text(&fb.query), "chapter-recovered.md");
            assert_eq!(fb.dir, root.path());
            assert_eq!(fb.quit_save_owner, quit_owned.then_some(id));
            assert!(e.active().document.path.is_none());
            let label = crate::workspace::buffer_display_name(&e, id);
            assert!(label.contains("Recovered") && label.contains("chapter.md"));
        }
    }

    #[test]
    fn recovery_save_as_unknown_origin_has_distinct_name_and_normal_directory_fallback() {
        let mut e = recovered(None, None);
        let (tx, _rx) = std::sync::mpsc::channel();
        assert!(crate::prompts::open_save_as(&mut e, &test_fs(), &tx));
        let fb = e.file_browser.as_ref().unwrap();
        assert_eq!(fb.mode.filter_text(&fb.query), "recovered-untitled.md");
        assert_eq!(fb.dir, std::env::current_dir().unwrap());
        assert!(e.active().document.path.is_none());
    }
}

#[cfg(test)]
mod handoff_integration {
    use crate::editor::Editor;
    use crate::fsx::{Fs, RealFs};
    use crate::jobs::{Executor, InlineExecutor};
    use crate::registry::Ctx;
    use crate::recovery_discovery::{self as discovery, Candidate, ScanScope};
    use crate::recovery_store::{self as store, RecoverySlot, CheckpointRecord, TaggedPath};
    use crate::test_support::{TestClock, FaultFs, FaultAt};
    use std::{path::PathBuf, sync::Arc};

    struct Fixture { root: tempfile::TempDir, original: PathBuf, source: PathBuf, row: Candidate }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let original = root.path().join("documents").join("original.md");
            std::fs::create_dir(original.parent().unwrap()).unwrap();
            std::fs::write(&original, "disk").unwrap();
            let slot = RecoverySlot::new();
            let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "origin".into(), 1,
                Some(TaggedPath::from_path(&original)), None);
            let ack = store::checkpoint(&RealFs, root.path(), &slot, &record, "recovered bytes").unwrap();
            let source = ack.record_path().to_owned();
            drop(slot);
            // Parallel subprocess tests may briefly inherit the descriptor between fork
            // and exec. The real owner is dropped; retry only this transient Busy result.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let row = loop {
                let row = discovery::scan(&RealFs, root.path(), &ScanScope::All).unwrap().remove(0);
                if !row.busy { break row; }
                assert!(std::time::Instant::now() < deadline, "dropped source lease remains busy");
                std::thread::sleep(std::time::Duration::from_millis(2));
            };
            Self { root, original, source, row }
        }
        fn editor(&self) -> Editor {
            let mut e = Editor::new_from_text("disk", Some(self.original.clone()), (80,24));
            e.recovery.set_root(self.root.path().to_owned()); e
        }
    }
    fn begin(e: &mut Editor, ex: &dyn Executor, fs: &Arc<dyn Fs + Send + Sync>, rows: Vec<Candidate>) {
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::recovery_flow::begin_selected(&mut Ctx { editor: e, executor: ex,
            clock: &TestClock(0), msg_tx: tx, fs: fs.clone() }, rows);
    }
    fn apply(e: &mut Editor, ex: &dyn Executor, fs: &Arc<dyn Fs + Send + Sync>, outcome: crate::jobs::JobOutcome) {
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::jobs_apply::apply_job_outcome(outcome, e, ex, &TestClock(10), &tx, fs);
    }
    fn drain(e: &mut Editor, ex: &dyn Executor, fs: &Arc<dyn Fs + Send + Sync>) {
        for _ in 0..16 {
            let outcomes = ex.drain();
            if outcomes.is_empty() { return; }
            for outcome in outcomes { apply(e, ex, fs, outcome); }
        }
        panic!("bounded fixture failed to reach a terminal state");
    }

    #[test]
    fn recovery_handoff_cleanup_failure_keeps_durable_ack_and_both_copies() {
        let f = Fixture::new();
        let mut e = f.editor();
        let ex = InlineExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(FaultFs::new(FaultAt::RemoveFile));
        begin(&mut e, &ex, &fs, vec![f.row.clone()]); drain(&mut e, &ex, &fs);
        assert_eq!(e.buffers.len(), 2);
        assert!(f.source.exists());
        let ack = e.active().recovery_ack.as_ref().expect("successful checkpoint remains acknowledged despite cleanup error");
        let bytes = std::fs::read(ack.path()).unwrap();
        assert_eq!(store::decode(&bytes).unwrap().1, "recovered bytes");
        assert!(e.active().recovery_protection_failure.is_none());
        assert!(!crate::recovery_flow::has_pending_work(&e));
    }

    #[test]
    fn recovery_handoff_records_predecessor_without_assigning_a_write_target() {
        let f = Fixture::new();
        let mut e = f.editor();
        let original_id = e.active().id;
        let ex = InlineExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        begin(&mut e, &ex, &fs, vec![f.row.clone()]); drain(&mut e, &ex, &fs);
        assert!(e.active().document.path.is_none());
        assert_eq!(e.by_id(original_id).unwrap().document.buffer.to_string(), "disk");
        let ack = e.active().recovery_ack.as_ref().unwrap();
        let bytes = std::fs::read(ack.path()).unwrap();
        let (meta, body) = store::decode(&bytes).unwrap();
        let Some(discovery::SelectionToken::V2 { owner, generation }) = &f.row.token else { panic!("v2 fixture"); };
        assert_eq!(meta.record().predecessor(), Some((owner.as_str(), *generation)));
        assert_eq!(body, "recovered bytes");
        assert!(!f.source.exists());
    }

    #[test]
    fn recovery_handoff_cancelled_prepare_failure_does_not_cancel_a_later_quit() {
        let f = Fixture::new();
        let mut e = f.editor();
        let ex = super::DeferredRecoveryExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(FaultFs::new(FaultAt::RecoveryRead));
        begin(&mut e, &ex, &fs, vec![f.row.clone()]);
        crate::recovery_flow::cancel_batch(&mut e);
        e.quit = true;
        apply(&mut e, &ex, &fs, ex.run_next());
        assert!(e.quit, "cancelled work does not own this later quit decision");
        assert_eq!(e.buffers.len(), 1);
        assert!(f.source.exists());
    }

    #[test]
    fn recovery_handoff_quit_preserves_already_dispatched_handoff() {
        let f = Fixture::new();
        let mut e = f.editor();
        let ex = super::DeferredRecoveryExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        begin(&mut e, &ex, &fs, vec![f.row.clone()]);
        apply(&mut e, &ex, &fs, ex.run_next());
        assert_eq!(e.buffers.len(), 2);
        e.quit_drain = Some(crate::editor::QuitDrain::new(Default::default(), crate::editor::QuitMode::SaveAll));
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::recovery_flow::after_job(&mut Ctx { editor: &mut e, executor: &ex,
            clock: &TestClock(11), msg_tx: tx, fs: fs.clone() });
        apply(&mut e, &ex, &fs, ex.run_next());
        assert!(e.active().recovery_ack.is_some());
        assert!(!f.source.exists(), "starting quit does not revoke the active handoff");
    }
    #[test]
    fn recovery_handoff_checkpoint_fault_matrix_preserves_source_and_terminates() {
        for fault in [FaultAt::Create, FaultAt::Write { after: 2 }, FaultAt::Flush,
            FaultAt::Sync, FaultAt::Rename, FaultAt::StrictDirSync]
        {
            let f = Fixture::new();
            let before = std::fs::read(&f.source).unwrap();
            let mut e = f.editor();
            let ex = InlineExecutor::default();
            let fs: Arc<dyn Fs + Send + Sync> = Arc::new(FaultFs::new(fault));
            begin(&mut e, &ex, &fs, vec![f.row.clone()]); drain(&mut e, &ex, &fs);
            assert_eq!(e.buffers.len(), 2, "{fault:?}");
            assert_eq!(e.active().document.buffer.to_string(), "recovered bytes");
            assert!(e.active().document.dirty());
            assert!(e.active().recovery_protection_failure.is_some(), "{fault:?}");
            assert!(e.active().recovery_request.is_none());
            assert!(!crate::recovery_flow::has_pending_work(&e));
            assert_eq!(std::fs::read(&f.source).unwrap(), before);
            let guard = super::tests::released_lock(&f.source.parent().unwrap().join("owner.lock"));
            drop(guard);
        }
    }

    #[test]
    fn recovery_handoff_rejection_before_and_after_install_releases_source() {
        for after_install in [false, true] {
            let f = Fixture::new();
            let mut e = f.editor();
            let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
            let rejected = super::RejectingRecoveryExecutor;
            if after_install {
                let accepted = super::DeferredRecoveryExecutor::default();
                begin(&mut e, &accepted, &fs, vec![f.row.clone()]);
                apply(&mut e, &rejected, &fs, accepted.run_next());
                assert_eq!(e.buffers.len(), 2);
                assert!(e.active().recovery_protection_failure.is_some());
                assert!(e.active().recovery_request.is_none());
            } else {
                begin(&mut e, &rejected, &fs, vec![f.row.clone()]);
                assert_eq!(e.buffers.len(), 1);
            }
            assert!(f.source.exists());
            assert!(!crate::recovery_flow::has_pending_work(&e));
            let guard = super::tests::released_lock(&f.source.parent().unwrap().join("owner.lock"));
            drop(guard);
        }
    }

    #[test]
    fn recovery_handoff_cancelled_failed_live_copy_can_retry_current_contents() {
        let f = Fixture::new();
        let mut e = f.editor();
        let ex = super::DeferredRecoveryExecutor::default();
        let failed: Arc<dyn Fs + Send + Sync> = Arc::new(FaultFs::new(FaultAt::Sync));
        begin(&mut e, &ex, &failed, vec![f.row.clone()]);
        apply(&mut e, &ex, &failed, ex.run_next());
        let recovered_id = e.active().id;
        crate::recovery_flow::cancel_batch(&mut e);
        apply(&mut e, &ex, &failed, ex.run_next());
        assert!(e.active().recovery_ack.is_none());
        let len = e.active().document.buffer.len();
        crate::transact::submit_transaction(&mut e, wordcartel_core::history::Transaction::new(
            wordcartel_core::change::ChangeSet::insert(len, " edited", len)), &TestClock(20)).unwrap();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        begin(&mut e, &ex, &fs, vec![f.row.clone()]);
        apply(&mut e, &ex, &fs, ex.run_next());
        assert_eq!(ex.pending_len(), 1, "unprotected live copy retries despite cancelled error being silent");
        apply(&mut e, &ex, &fs, ex.run_next());
        assert_eq!(e.buffers.len(), 2);
        assert_eq!(e.active().id, recovered_id);
        let bytes = std::fs::read(e.active().recovery_ack.as_ref().unwrap().path()).unwrap();
        assert_eq!(store::decode(&bytes).unwrap().1, "recovered bytes edited");
        assert!(f.source.exists(), "retry protects current contents without retiring predecessor");
    }

    #[test]
    fn recovery_handoff_stale_prepare_panic_cannot_clear_next_phase() {
        let f = Fixture::new();
        let mut e = f.editor();
        let original = e.active().id;
        let ex = super::DeferredRecoveryExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        begin(&mut e, &ex, &fs, vec![f.row.clone()]);
        let old_kind = ex.pending.borrow().front().unwrap().kind;
        apply(&mut e, &ex, &fs, ex.run_next());
        let handoff = e.active().recovery_request;
        assert_ne!(old_kind, crate::jobs::JobKind::Recovery(handoff.unwrap()));
        // Synthetic stale transport: the production executor emits each outcome only once.
        apply(&mut e, &ex, &fs, crate::jobs::JobOutcome::Panicked { buffer_id: original,
            version: 0, kind: old_kind, save_request: None, msg: "old phase".into() });
        assert_eq!(e.active().recovery_request, handoff);
        assert!(crate::recovery_flow::has_pending_work(&e));
        apply(&mut e, &ex, &fs, ex.run_next());
        assert!(e.active().recovery_ack.is_some());
        assert!(!f.source.exists());
    }

    #[test]
    fn recovery_handoff_first_save_routes_through_occupied_suggestion_confirmation() {
        use crate::registry::{Registry, CommandId};
        let f = Fixture::new();
        let target = f.original.parent().unwrap().join("original-recovered.md");
        std::fs::write(&target, "occupied").unwrap();
        let mut e = f.editor();
        let original_id = e.active().id;
        let ex = InlineExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        begin(&mut e, &ex, &fs, vec![f.row.clone()]); drain(&mut e, &ex, &fs);
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let (tx, _rx) = std::sync::mpsc::channel();
        reg.dispatch(CommandId("save"), &mut Ctx { editor: &mut e, executor: &ex,
            clock: &TestClock(20), msg_tx: tx.clone(), fs: fs.clone() });
        assert!(e.active().document.path.is_none());
        assert_eq!(e.file_browser.as_ref().unwrap().mode.filter_text(""), "original-recovered.md");
        crate::app::reduce(crate::test_support::press(crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE), &mut e, &reg, &km, &ex, &TestClock(21), &tx, &fs);
        assert_eq!(e.prompt.as_ref().unwrap().action_for('o'), Some(crate::prompt::PromptAction::OverwriteSaveAs));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "occupied");
        assert!(e.active().document.path.is_none());
        crate::app::reduce(crate::test_support::press(crossterm::event::KeyCode::Char('o'),
            crossterm::event::KeyModifiers::NONE), &mut e, &reg, &km, &ex, &TestClock(22), &tx, &fs);
        drain(&mut e, &ex, &fs);
        assert_eq!(e.active().document.path.as_deref(), Some(target.as_path()));
        assert_eq!(std::fs::read_to_string(target).unwrap(), "recovered bytes");
        assert_eq!(std::fs::read_to_string(&f.original).unwrap(), "disk");
        assert_eq!(e.by_id(original_id).unwrap().document.buffer.to_string(), "disk");
    }

    enum SourceRemoval {
        Panic(bool),
        Barrier(std::sync::mpsc::Sender<()>, std::sync::Mutex<std::sync::mpsc::Receiver<()>>),
    }
    struct SourceCleanupFs { source: PathBuf, removal: SourceRemoval }
    impl Fs for SourceCleanupFs {
        fn canonicalize_existing(&self, p: &std::path::Path) -> std::io::Result<PathBuf> { RealFs.canonicalize_existing(p) }
        fn create_dir_excl(&self, p: &std::path::Path, mode: u32) -> std::io::Result<()> { RealFs.create_dir_excl(p, mode) }
        fn validate_private_dir(&self, p: &std::path::Path) -> std::io::Result<()> { RealFs.validate_private_dir(p) }
        fn try_recovery_lock(&self, p: &std::path::Path, create: bool) -> std::io::Result<Box<dyn crate::fsx::RecoveryLease>> { RealFs.try_recovery_lock(p, create) }
        fn open_regular_nofollow(&self, p: &std::path::Path) -> std::io::Result<Box<dyn crate::fsx::RecoveryRead>> { RealFs.open_regular_nofollow(p) }
        fn sync_dir_strict(&self, p: &std::path::Path) -> std::io::Result<()> { RealFs.sync_dir_strict(p) }
        fn create_excl(&self, p: &std::path::Path, mode: u32) -> std::io::Result<Box<dyn crate::fsx::WriteSync>> { RealFs.create_excl(p, mode) }
        fn existing_mode(&self, p: &std::path::Path) -> Option<u32> { RealFs.existing_mode(p) }
        fn read_capped(&self, p: &std::path::Path, n: u64) -> std::io::Result<Option<Vec<u8>>> { RealFs.read_capped(p, n) }
        fn stat(&self, p: &std::path::Path) -> std::io::Result<crate::fsx::FileStat> { RealFs.stat(p) }
        fn rename(&self, a: &std::path::Path, b: &std::path::Path) -> std::io::Result<()> { RealFs.rename(a, b) }
        fn sync_dir(&self, p: &std::path::Path) -> std::io::Result<()> { RealFs.sync_dir(p) }
        fn list_dir(&self, p: &std::path::Path, n: Option<usize>) -> std::io::Result<crate::fsx::DirListing> { RealFs.list_dir(p, n) }
        fn remove_file(&self, p: &std::path::Path) -> std::io::Result<()> {
            if p == self.source {
                match &self.removal {
                    SourceRemoval::Panic(after) => {
                        if *after { RealFs.remove_file(p)?; }
                        panic!("source cleanup probe");
                    },
                    SourceRemoval::Barrier(entered, release) => {
                        entered.send(()).map_err(std::io::Error::other)?;
                        release.lock().unwrap().recv_timeout(std::time::Duration::from_secs(10))
                            .map_err(std::io::Error::other)?;
                    },
                }
            }
            RealFs.remove_file(p)
        }
    }

    #[test]
    fn recovery_handoff_cleanup_panics_before_and_after_unlink_keep_successor_ack() {
        for after_remove in [false, true] {
            let f = Fixture::new();
            let mut e = f.editor();
            let ex = InlineExecutor::default();
            let fs: Arc<dyn Fs + Send + Sync> = Arc::new(SourceCleanupFs {
                source: f.source.clone(), removal: SourceRemoval::Panic(after_remove) });
            begin(&mut e, &ex, &fs, vec![f.row.clone()]); drain(&mut e, &ex, &fs);
            let ack = e.active().recovery_ack.as_ref().expect("strict Ack survives panic transport");
            assert_eq!(store::decode(&std::fs::read(ack.path()).unwrap()).unwrap().1, "recovered bytes");
            assert_eq!(f.source.exists(), !after_remove);
            assert!(e.active().recovery_protection_failure.is_none());
            assert!(e.status_text().contains("protected"));
            assert!(!crate::recovery_flow::has_pending_work(&e));
        }
    }

    #[test]
    fn recovery_handoff_missing_original_parent_uses_normal_first_save_directory() {
        let f = Fixture::new();
        std::fs::remove_file(&f.original).unwrap();
        std::fs::remove_dir(f.original.parent().unwrap()).unwrap();
        let mut e = f.editor();
        let ex = InlineExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        begin(&mut e, &ex, &fs, vec![f.row.clone()]); drain(&mut e, &ex, &fs);
        assert!(e.active().recovery_save_dir.is_none());
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::registry::Registry::builtins().dispatch(crate::registry::CommandId("save"),
            &mut Ctx { editor: &mut e, executor: &ex, clock: &TestClock(20), msg_tx: tx, fs });
        let fb = e.file_browser.as_ref().unwrap();
        assert_eq!(fb.dir, std::env::current_dir().unwrap());
        assert_eq!(fb.mode.filter_text(""), "original-recovered.md");
        assert!(e.active().document.path.is_none());
    }

    #[test]
    fn recovery_handoff_failed_first_selection_releases_lease_and_advances_batch() {
        let f = Fixture::new();
        let second_slot = RecoverySlot::new();
        let second_record = CheckpointRecord::new(second_slot.reserve_generation().unwrap(), "other".into(), 1,
            Some(TaggedPath::from_path(&f.original)), None);
        let second = store::checkpoint(&RealFs, f.root.path(), &second_slot, &second_record, "second").unwrap();
        drop(second_slot);
        let second_row = discovery::scan(&RealFs, f.root.path(), &ScanScope::All).unwrap().into_iter()
            .find(|row| row.source_path == second.path()).unwrap();
        let mut e = f.editor();
        let ex = InlineExecutor::default();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(FaultFs::on_occurrence(FaultAt::Sync, 1));
        begin(&mut e, &ex, &fs, vec![f.row.clone(), second_row]); drain(&mut e, &ex, &fs);
        assert_eq!(e.buffers.len(), 3);
        let first = e.buffers.iter().find(|b| b.document.buffer.to_string() == "recovered bytes").unwrap();
        assert!(first.recovery_protection_failure.is_some());
        assert_eq!(e.active().document.buffer.to_string(), "second");
        assert!(e.active().recovery_ack.is_some());
        assert!(f.source.exists());
        assert!(!second.path().exists());
        assert!(!crate::recovery_flow::has_pending_work(&e));
    }

    #[test]
    fn recovery_handoff_timeout_after_worker_authorization_keeps_successor() {
        let f = Fixture::new();
        let mut e = f.editor();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(SourceCleanupFs {
            source: f.source.clone(), removal: SourceRemoval::Barrier(entered_tx, std::sync::Mutex::new(release_rx)) });
        let (wake_tx, wake_rx) = std::sync::mpsc::channel();
        let ex = crate::jobs::ThreadExecutor::new(wake_tx);
        begin(&mut e, &ex, &fs, vec![f.row.clone()]);
        wake_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        drain(&mut e, &ex, &fs);
        entered_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(e.buffers.len(), 2);
        // This real worker is inside source removal, after the strict checkpoint and CAS.
        let successor = discovery::scan(&RealFs, f.root.path(), &ScanScope::All).unwrap().into_iter()
            .find(|row| row.source_path != f.source).unwrap().source_path;
        assert_eq!(store::decode(&std::fs::read(&successor).unwrap()).unwrap().1, "recovered bytes");
        e.quit = true;
        crate::timers::pre_recv(&mut e, 5_010);
        assert!(!e.quit);
        assert_eq!(e.status_text(), "Recovery IO is still pending; quit cancelled");
        assert!(!crate::recovery_flow::has_pending_work(&e));
        release_tx.send(()).unwrap();
        wake_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        drain(&mut e, &ex, &fs);
        assert!(!e.quit, "late completion cannot reinstate the cancelled quit");
        assert!(!f.source.exists(), "authorization already won before timeout");
        assert_eq!(store::decode(&std::fs::read(successor).unwrap()).unwrap().1, "recovered bytes");
        assert!(!crate::recovery_flow::has_pending_work(&e));
    }

}

#[cfg(test)]
mod open_and_clean_integration {
    use crate::editor::Editor;
    use crate::fsx::{Fs, RealFs};
    use crate::jobs::{Executor, InlineExecutor};
    use crate::registry::Ctx;
    use crate::test_support::TestClock;
    use std::sync::Arc;

    fn settle(e: &mut Editor, ex: &dyn Executor, fs: &Arc<dyn Fs + Send + Sync>) {
        let (tx, _rx) = std::sync::mpsc::channel();
        for _ in 0..6 {
            for outcome in ex.drain() { crate::jobs_apply::apply_job_outcome(outcome, e, ex, &TestClock(10), &tx, fs); }
            crate::app::finish_iteration(e, ex, &TestClock(10), &tx, fs);
        }
    }

    #[test]
    fn recovery_open_both_runtime_branches_offer_preserved_copy_after_ordinary_save() {
        for initial in ["other document", "\n"] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("crashed.md");
            std::fs::write(&path, "disk").unwrap();
            let source = root.path().join("previous-session.swp");
            let body = "previous unsaved work";
            let header = crate::swap::SwapHeader { realpath: Some(path.to_string_lossy().into_owned()),
                content_hash: crate::swap::fnv1a64(body.as_bytes()), ..Default::default() };
            std::fs::write(&source, crate::swap::serialize(&header, body)).unwrap();
            let before = std::fs::read(&source).unwrap();
            let mut e = Editor::new_from_text(initial, None, (80,24));
            e.recovery.set_root(root.path().to_owned()); e.resume_enabled = false;
            let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
            let ex = InlineExecutor::default();
            crate::workspace::open_as_new_buffer(&mut e, &*fs, &path);
            let disk_id = e.active().id;
            assert_eq!(e.active().document.buffer.to_string(), "disk");
            let (tx, _rx) = std::sync::mpsc::channel();
            crate::save::dispatch_save(&mut Ctx { editor: &mut e, executor: &ex,
                clock: &TestClock(1), msg_tx: tx.clone(), fs: fs.clone() });
            settle(&mut e, &ex, &fs);
            assert_eq!(std::fs::read(&source).unwrap(), before);
            assert!(e.recovery_picker.is_some(), "both additive and throwaway open assess recovery");
            let reg = crate::registry::Registry::builtins();
            let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
            for code in [crossterm::event::KeyCode::Char(' '), crossterm::event::KeyCode::Enter] {
                crate::app::reduce(crate::test_support::press(code, crossterm::event::KeyModifiers::NONE),
                    &mut e, &reg, &km, &ex, &TestClock(12), &tx, &fs);
            }
            settle(&mut e, &ex, &fs);
            assert_ne!(e.active().id, disk_id);
            assert_eq!(e.by_id(disk_id).unwrap().document.buffer.to_string(), "disk");
            assert_eq!(e.active().document.buffer.to_string(), body);
            assert!(e.active().document.path.is_none());
            assert_eq!(std::fs::read(&source).unwrap(), before);
        }
    }

    #[test]
    fn recovery_open_bootstrap_real_worker_wakes_without_keyboard_input() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("recovered-startup.md");
        std::fs::write(&source, "startup recovery").unwrap();
        let mut e = Editor::new_from_text("disk", None, (80,24));
        e.recovery.set_root(root.path().to_owned());
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        let (wake_tx, wake_rx) = std::sync::mpsc::channel();
        let ex = crate::jobs::ThreadExecutor::new(wake_tx);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::recovery_flow::bootstrap(&mut Ctx { editor: &mut e, executor: &ex,
            clock: &TestClock(0), msg_tx: tx.clone(), fs: fs.clone() });
        wake_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        settle(&mut e, &ex, &fs);
        assert!(e.recovery_picker.is_some());
        assert_eq!(e.active().document.buffer.to_string(), "disk");
        assert!(source.exists());
    }

    #[test]
    fn recovery_clean_protects_open_dump_and_confirmation_rechecks_late_open() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("recovered-open.md");
        std::fs::write(&source, "only copy").unwrap();
        let snapshot = crate::swap::cleanable_recovery_files(&RealFs, root.path(), &Default::default());
        assert_eq!(snapshot, vec![source.clone()]);
        let mut e = Editor::new_from_text("only copy", Some(source.clone()), (80,24));
        let protected = crate::swap::open_swap_paths(&e, &RealFs).unwrap();
        assert!(crate::swap::cleanable_recovery_files(&RealFs, root.path(), &protected).is_empty());
        e.pending_clean = snapshot;
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(crate::prompt::PromptAction::CleanRecovery, &mut e,
            &InlineExecutor::default(), &TestClock(0), &tx, &crate::test_support::test_fs());
        assert_eq!(std::fs::read_to_string(source).unwrap(), "only copy");
        assert!(e.pending_clean.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn recovery_clean_protection_matches_aliases_and_pending_legacy_imports() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        let alias = root.path().join("alias");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let source = real.join("recovered-pending.md");
        std::fs::write(&source, "only copy").unwrap();
        let mut e = Editor::new_from_text("disk", None, (80,24));
        e.recovery.set_root(real.clone());
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(RealFs);
        let rows = crate::recovery_discovery::scan(&*fs, &real, &crate::recovery_discovery::ScanScope::All).unwrap();
        let ex = super::DeferredRecoveryExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::recovery_flow::begin_selected(&mut Ctx { editor: &mut e, executor: &ex,
            clock: &TestClock(0), msg_tx: tx, fs: fs.clone() }, rows);
        let protected = crate::swap::open_swap_paths(&e, &*fs).unwrap();
        assert!(crate::swap::cleanable_recovery_files(&*fs, &alias, &protected).is_empty());
        assert!(source.exists());
    }

    #[test]
    fn recovery_clean_identity_failure_preserves_snapshot_and_reports_refusal() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("recovered-preserve.md");
        std::fs::write(&source, "only copy").unwrap();
        let mut e = Editor::new_from_text("disk", None, (80,24));
        e.pending_clean = vec![source.clone()];
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(crate::test_support::FaultFs::new(crate::test_support::FaultAt::Canonicalize));
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(crate::prompt::PromptAction::CleanRecovery, &mut e,
            &InlineExecutor::default(), &TestClock(0), &tx, &fs);
        assert!(source.exists());
        assert!(e.status_text().contains("cannot verify open files"));
        assert!(e.pending_clean.is_empty());
    }
}

#[cfg(test)]
mod retry_integration {
    use crate::editor::Editor;
    use crate::jobs::{Executor, JobOutcome};
    use crate::test_support::{FaultAt, FaultFs, TestClock};
    use super::DeferredRecoveryExecutor;
    use std::sync::Arc;

    #[test]
    fn recovery_retry_failure_uses_completion_time_and_bounds_retries_after_new_edits() {
        for disposition in ["failure", "panic", "cancelled"] {
            let root = tempfile::tempdir().unwrap();
            let mut e = Editor::new_from_text("unsaved", None, (80,24));
            e.recovery.set_root(root.path().to_owned());
            e.active_mut().document.saved_version = None;
            e.active_mut().last_edit_at = Some(0);
            let ex = DeferredRecoveryExecutor::default();
            let (tx, _) = std::sync::mpsc::channel();
            let bad: Arc<dyn crate::fsx::Fs + Send + Sync> = Arc::new(FaultFs::new(FaultAt::StrictDirSync));
            crate::timers::on_tick(&mut e, &ex, &TestClock(2_000), &tx, &bad);
            assert_eq!(ex.pending_len(), 1);
            let outcome = ex.run_next();
            let outcome = if disposition == "panic" {
                let JobOutcome::Done(result) = outcome else { panic!("fault returns a normal error"); };
                JobOutcome::Panicked { buffer_id: result.buffer_id, version: result.version,
                    kind: result.kind, save_request: None, msg: "worker panic".into() }
            } else { outcome };
            if disposition == "cancelled" {
                let id = e.active().id;
                crate::recovery_flow::cancel_buffer(&mut e, id);
            }
            // A slow failure must still wait a full 30 seconds AFTER its foreground completion.
            crate::jobs_apply::apply_job_outcome(outcome, &mut e, &ex, &TestClock(62_000), &tx, &bad);
            let swap = crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "swap").unwrap();
            assert_eq!((swap.deadline)(&e, 62_000), Some(92_000));
            e.active_mut().document.version += 1;
            e.active_mut().last_edit_at = Some(62_100);
            for now in [62_000, 62_100, 64_100, 91_999] {
                crate::timers::on_tick(&mut e, &ex, &TestClock(now), &tx, &bad);
                assert_eq!(ex.pending_len(), 0, "failure cannot cause a zero-delay retry");
            }
            let good = crate::test_support::test_fs();
            crate::timers::on_tick(&mut e, &ex, &TestClock(92_000), &tx, &good);
            assert_eq!(ex.pending_len(), 1);
            crate::jobs_apply::apply_job_outcome(ex.run_next(), &mut e, &ex, &TestClock(92_001), &tx, &good);
            assert!(e.active().recovery_ack.is_some());
            for now in [92_001, 100_000, 1_000_000] {
                crate::timers::on_tick(&mut e, &ex, &TestClock(now), &tx, &good);
                assert_eq!(ex.pending_len(), 0, "settled success remains idle");
                assert_eq!((swap.deadline)(&e, now), None);
            }
            assert!(ex.drain().is_empty());
        }
    }

    #[test]
    fn recovery_retry_queue_rejection_also_has_a_finite_backoff() {
        let mut e = Editor::new_from_text("unsaved", None, (80,24));
        e.active_mut().document.saved_version = None;
        e.active_mut().last_edit_at = Some(0);
        let (tx, _) = std::sync::mpsc::channel();
        let rejected = super::RejectingRecoveryExecutor;
        crate::timers::on_tick(&mut e, &rejected, &TestClock(2_000), &tx, &crate::test_support::test_fs());
        let swap = crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "swap").unwrap();
        assert_eq!((swap.deadline)(&e, 2_000), Some(32_000));
    }
}
