//! Process death probes validate restart visibility, not simulated power-loss durability.
#[cfg(test)]
mod tests {
    use crate::fsx::{Fs, RealFs, WriteSync, RecoveryRead, RecoveryLease, FileStat, DirListing};
    use crate::recovery_discovery::{self as discovery, ScanScope, PrepareOutcome};
    use crate::recovery_store::{self as store, RecoverySlot, CheckpointRecord, TaggedPath};
    use crate::jobs::{Executor, InlineExecutor};
    use std::{path::{Path, PathBuf}, process::{Child, Command, Stdio}, sync::Arc, time::{Duration, Instant}};
    const BODY: &str = "the only unsaved recovery text\n";
    const HELPER: &str = "recovery_process_tests::tests::recovery_process_helper";

    struct OwnedChild { child: Child, marker: PathBuf }
    impl OwnedChild {
        fn spawn(root: &Path, mode: &str, marker: &str) -> Self {
            let marker = root.join(marker);
            let child = Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", HELPER, "--nocapture"])
                .env("WORDCARTEL_PROCESS_ROOT", root).env("WORDCARTEL_PROCESS_MODE", mode)
                .env("WORDCARTEL_PROCESS_MARKER", &marker)
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit())
                .spawn().unwrap();
            Self { child, marker }
        }
        fn ready(&mut self) {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !self.marker.exists() {
                assert!(self.child.try_wait().unwrap().is_none(), "child exited before boundary");
                assert!(Instant::now() < deadline, "child did not reach boundary");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        fn finish(&mut self) {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if let Some(status) = self.child.try_wait().unwrap() { assert!(status.success()); return; }
                assert!(Instant::now() < deadline, "child failed to finish");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        fn kill(&mut self) { self.child.kill().unwrap(); self.child.wait().unwrap(); }
    }
    impl Drop for OwnedChild {
        fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
    }
    fn park(marker: &Path) -> ! {
        std::fs::write(marker, "reached").unwrap();
        // A bounded backstop even if the supervising test disappears unexpectedly.
        std::thread::sleep(Duration::from_secs(45));
        panic!("supervisor did not terminate parked child");
    }
    fn seed(root: &Path, slot: &RecoverySlot, named: bool) -> store::CheckpointAck {
        let association = named.then(|| TaggedPath::from_path(&root.join("same-original.md")));
        let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "process-lineage".into(), 1, association, None);
        store::checkpoint(&RealFs, root, slot, &record, BODY).unwrap()
    }
    fn rows(root: &Path) -> Vec<discovery::Candidate> {
        discovery::scan(&RealFs, root, &ScanScope::All).unwrap()
    }

    #[test]
    fn recovery_process_named_and_unnamed_owners_survive_process_death_independently() {
        let root = tempfile::tempdir().unwrap();
        let mut a = OwnedChild::spawn(root.path(), "owners", "a.ready"); a.ready();
        let mut b = OwnedChild::spawn(root.path(), "owners", "b.ready"); b.ready();
        let active = rows(root.path());
        assert_eq!(active.len(), 4);
        assert!(active.iter().all(|r| r.busy));
        let paths: std::collections::HashSet<_> = active.iter().map(|r| &r.source_path).collect();
        assert_eq!(paths.len(), 4, "same named target and unnamed lineage never share a slot");
        a.kill(); b.kill();
        let recovered = rows(root.path());
        assert_eq!(recovered.len(), 4);
        assert_eq!(recovered.iter().filter(|r| r.association.is_some()).count(), 2);
        for row in recovered {
            assert!(!row.busy && row.unavailable.is_none());
            let PrepareOutcome::Ready(p) = discovery::prepare(&RealFs, root.path(), &row).unwrap() else { panic!("unchanged"); };
            assert_eq!(p.body, BODY);
        }
    }

    #[test]
    fn recovery_process_queued_job_retains_last_slot_lease_after_buffer_owner_drops() {
        let root = tempfile::tempdir().unwrap();
        let mut child = OwnedChild::spawn(root.path(), "queued", "queued.ready"); child.ready();
        assert!(rows(root.path())[0].busy, "queued closure must keep the lease alive");
        std::fs::write(root.path().join("release"), "release queue").unwrap();
        child.finish();
        let found = rows(root.path());
        assert_eq!(found.len(), 1); assert!(!found[0].busy);
        assert_eq!(found[0].preview, BODY);
    }

    #[test]
    fn recovery_process_concurrent_selection_is_exclusive_until_process_death() {
        let root = tempfile::tempdir().unwrap();
        let slot = RecoverySlot::new(); seed(root.path(), &slot, false); drop(slot);
        let mut holder = OwnedChild::spawn(root.path(), "prepare", "holder.ready"); holder.ready();
        let mut contender = OwnedChild::spawn(root.path(), "contend", "contender.ready"); contender.finish();
        assert_eq!(std::fs::read_to_string(&contender.marker).unwrap(), "busy");
        holder.kill();
        let selected = rows(root.path()).remove(0);
        assert!(matches!(discovery::prepare(&RealFs, root.path(), &selected), Ok(PrepareOutcome::Ready(_))));
    }

    #[test]
    fn recovery_process_crash_boundaries_preserve_source_or_successor_on_restart() {
        for boundary in ["write", "file-sync", "rename", "owner-sync", "retire-before", "retire-after", "retire-sync"] {
            let root = tempfile::tempdir().unwrap();
            let slot = RecoverySlot::new(); let source = seed(root.path(), &slot, false).path().to_owned(); drop(slot);
            let mut child = OwnedChild::spawn(root.path(), boundary, "boundary.ready"); child.ready(); child.kill();
            let recovered = rows(root.path());
            assert!(recovered.iter().all(|r| !r.busy), "dead process retains no locks: {boundary}");
            let valid: Vec<_> = recovered.iter().filter(|r| r.unavailable.is_none()).collect();
            assert!(!valid.is_empty(), "source or durable successor must survive: {boundary}");
            for row in &valid {
                let PrepareOutcome::Ready(p) = discovery::prepare(&RealFs, root.path(), row).unwrap() else { panic!("unchanged"); };
                assert_eq!(p.body, BODY, "{boundary}");
            }
            if matches!(boundary, "retire-after" | "retire-sync") {
                assert!(!source.exists());
                assert!(valid.iter().any(|r| r.source_path != source));
            } else { assert!(source.exists(), "source cannot retire before successor Ack: {boundary}"); }
        }
    }

    #[test]
    #[ignore = "invoked only as a bounded owned subprocess"]
    fn recovery_process_helper() {
        let Ok(root) = std::env::var("WORDCARTEL_PROCESS_ROOT") else { return; };
        let root = PathBuf::from(root);
        let mode = std::env::var("WORDCARTEL_PROCESS_MODE").unwrap();
        let marker = PathBuf::from(std::env::var_os("WORDCARTEL_PROCESS_MARKER").unwrap());
        match mode.as_str() {
            "owners" => {
                let named = RecoverySlot::new(); let unnamed = RecoverySlot::new();
                seed(&root, &named, true); seed(&root, &unnamed, false); park(&marker);
            },
            "queued" => queued_child(&root, &marker),
            "prepare" => {
                let row = rows(&root).remove(0);
                let PrepareOutcome::Ready(_held) = discovery::prepare(&RealFs, &root, &row).unwrap() else { panic!("unchanged"); };
                park(&marker);
            },
            "contend" => {
                let mut row = rows(&root).remove(0); assert!(row.busy);
                // Reconstruct the previously selectable token without acquiring its lease,
                // as another already-open picker would retain it during contention.
                let bytes = std::fs::read(&row.source_path).unwrap();
                let (metadata, _) = store::decode(&bytes).unwrap();
                row.token = Some(discovery::SelectionToken::V2 {
                    owner: metadata.owner().to_owned(), generation: metadata.record().generation() });
                assert!(matches!(discovery::prepare(&RealFs, &root, &row),
                    Err(store::RecoveryError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock));
                std::fs::write(marker, "busy").unwrap();
            },
            _ => handoff_child(&root, &marker, &mode),
        }
    }
    fn queued_child(root: &Path, marker: &Path) {
        let slot = RecoverySlot::new(); seed(root, &slot, false);
        let held = slot.clone();
        let ex = crate::recovery_regressions::DeferredRecoveryExecutor::default();
        ex.try_dispatch(crate::jobs::Job { buffer_id: crate::editor::BufferId(123), version: 0,
            class: crate::jobs::ResultClass::Durability, kind: crate::jobs::JobKind::Save,
            save_request: None, run: Box::new(move || { drop(held); panic!("queue must be dropped without executing"); }) }).unwrap();
        drop(slot);
        std::fs::write(marker, "queued").unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !root.join("release").exists() {
            assert!(Instant::now() < deadline); std::thread::sleep(Duration::from_millis(5));
        }
        drop(ex);
    }
    fn handoff_child(root: &Path, marker: &Path, mode: &str) {
        let row = rows(root).remove(0);
        let fs: Arc<dyn Fs + Send + Sync> = Arc::new(BoundaryFs {
            mode: mode.into(), marker: marker.to_owned(), source: row.source_path.clone() });
        let mut editor = crate::editor::Editor::new_from_text("disk", None, (80,24));
        editor.recovery.set_root(root.to_owned());
        let executor = InlineExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::recovery_flow::begin_selected(&mut crate::registry::Ctx { editor: &mut editor,
            executor: &executor, clock: &crate::test_support::TestClock(0), msg_tx: tx.clone(), fs: fs.clone() }, vec![row]);
        for _ in 0..8 {
            for outcome in executor.drain() {
                crate::jobs_apply::apply_job_outcome(outcome, &mut editor, &executor,
                    &crate::test_support::TestClock(0), &tx, &fs);
            }
        }
        panic!("handoff failed to reach boundary {mode}");
    }

    struct BoundaryFs { mode: String, marker: PathBuf, source: PathBuf }
    struct BoundaryWriter { inner: Box<dyn WriteSync>, mode: String, marker: PathBuf }
    impl WriteSync for BoundaryWriter {
        fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.inner.write_all(bytes)?;
            if self.mode == "write" { park(&self.marker); } Ok(())
        }
        fn flush(&mut self) -> std::io::Result<()> { self.inner.flush() }
        fn set_mode(&self, mode: u32) -> std::io::Result<()> { self.inner.set_mode(mode) }
        fn sync_all(&self) -> std::io::Result<()> {
            self.inner.sync_all()?;
            if self.mode == "file-sync" { park(&self.marker); } Ok(())
        }
    }
    impl Fs for BoundaryFs {
        fn create_excl(&self, p: &Path, mode: u32) -> std::io::Result<Box<dyn WriteSync>> {
            Ok(Box::new(BoundaryWriter { inner: RealFs.create_excl(p,mode)?, mode: self.mode.clone(), marker: self.marker.clone() }))
        }
        fn rename(&self, a: &Path, b: &Path) -> std::io::Result<()> {
            RealFs.rename(a,b)?; if self.mode == "rename" { park(&self.marker); } Ok(())
        }
        fn sync_dir_strict(&self, p: &Path) -> std::io::Result<()> {
            RealFs.sync_dir_strict(p)?;
            if (self.mode == "owner-sync" && p.join("checkpoint.wcr") != self.source && p.join("owner.lock").exists())
                || (self.mode == "retire-sync" && Some(p) == self.source.parent()) { park(&self.marker); }
            Ok(())
        }
        fn remove_file(&self, p: &Path) -> std::io::Result<()> {
            if p == self.source && self.mode == "retire-before" { park(&self.marker); }
            RealFs.remove_file(p)?;
            if p == self.source && self.mode == "retire-after" { park(&self.marker); } Ok(())
        }
        fn canonicalize_existing(&self, p: &Path) -> std::io::Result<PathBuf> { RealFs.canonicalize_existing(p) }
        fn create_dir_excl(&self, p: &Path, m: u32) -> std::io::Result<()> { RealFs.create_dir_excl(p,m) }
        fn validate_private_dir(&self, p: &Path) -> std::io::Result<()> { RealFs.validate_private_dir(p) }
        fn try_recovery_lock(&self, p: &Path, c: bool) -> std::io::Result<Box<dyn RecoveryLease>> { RealFs.try_recovery_lock(p,c) }
        fn open_regular_nofollow(&self, p: &Path) -> std::io::Result<Box<dyn RecoveryRead>> { RealFs.open_regular_nofollow(p) }
        fn existing_mode(&self, p: &Path) -> Option<u32> { RealFs.existing_mode(p) }
        fn read_capped(&self, p: &Path, n: u64) -> std::io::Result<Option<Vec<u8>>> { RealFs.read_capped(p,n) }
        fn stat(&self, p: &Path) -> std::io::Result<FileStat> { RealFs.stat(p) }
        fn sync_dir(&self, p: &Path) -> std::io::Result<()> { RealFs.sync_dir(p) }
        fn list_dir(&self, p: &Path, n: Option<usize>) -> std::io::Result<DirListing> { RealFs.list_dir(p,n) }
    }
}
