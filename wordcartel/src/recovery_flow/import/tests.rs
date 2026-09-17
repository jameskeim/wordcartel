#[cfg(test)]
mod tests {
    use super::super::*;
    #[test]
    fn recovery_handoff_import_preserves_disk_and_immediately_protects_legacy_body() {
        let fs = crate::test_support::test_fs();
        let temp = tempfile::tempdir().unwrap(); let root = temp.path().to_owned();
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("recovered-draft.md");
        std::fs::write(&source, "rescued body").unwrap();
        let rows = crate::recovery_discovery::scan(&*fs, &root, &crate::recovery_discovery::ScanScope::All).unwrap();
        let mut e = Editor::new_from_text("disk", Some(root.join("disk.md")), (80,24));
        e.recovery.set_root(root);
        let disk = e.active().id;
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock::new(7);
        let (tx,_) = std::sync::mpsc::channel();
        begin_selected(&mut Ctx {editor: &mut e, executor: &ex, clock: &clock, msg_tx:tx.clone(), fs:fs.clone()}, rows);
        use crate::jobs::Executor;
        for _ in 0..3 { for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o,&mut e,&ex,&clock,&tx,&fs); } }
        assert_ne!(e.active().id,disk);
        assert_eq!(e.by_id(disk).unwrap().document.buffer.to_string(),"disk");
        assert!(e.active().document.path.is_none());
        assert!(e.active().document.dirty());
        let ack = e.active().recovery_ack.as_ref().expect("immediate checkpoint");
        assert_eq!(recovery_store::decode(&std::fs::read(ack.path()).unwrap()).unwrap().1,"rescued body");
        assert!(source.exists());
    }

    fn v2_source(fs: &dyn crate::fsx::Fs, root: &Path) -> Candidate {
        let slot = RecoverySlot::new();
        let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "0123456789abcdef".into(), 0, None, None);
        recovery_store::checkpoint(fs,root,&slot,&record,"original rescued text").unwrap();
        drop(slot);
        crate::test_support::recovery_scan_released(fs,root,&crate::recovery_discovery::ScanScope::All,1).remove(0)
    }
    #[test]
    fn recovery_handoff_v2_source_retires_only_after_successor_and_initial_capture() {
        let fs = crate::test_support::test_fs(); let temp = tempfile::tempdir().unwrap(); let root = temp.path().to_owned();
        let candidate = v2_source(&*fs,&root); let source = candidate.source_path.clone();
        let mut e = Editor::new_from_text("disk",None,(80,24)); e.recovery.set_root(root);
        let ex = crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock = crate::test_support::TestClock::new(7); let (tx,_) = std::sync::mpsc::channel();
        begin_selected(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()},vec![candidate]);
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        assert!(source.exists()); assert!(e.active().document.dirty());
        let captured_version = e.active().document.version;
        e.active_mut().document.version += 1;
        let outcome = ex.run_next();
        assert!(!source.exists(),"retirement is in worker transaction, before merge");
        crate::jobs_apply::apply_job_outcome(outcome,&mut e,&ex,&clock,&tx,&fs);
        let ack = e.active().recovery_ack.as_ref().unwrap();
        assert_eq!(recovery_store::decode(&std::fs::read(ack.path()).unwrap()).unwrap().1,"original rescued text");
        assert_eq!(e.active().swapped_version,Some(captured_version));
        assert!(!has_pending_work(&e));
    }
    #[test]
    fn recovery_handoff_cancel_before_worker_retains_source_and_releases_lock() {
        let fs = crate::test_support::test_fs(); let temp = tempfile::tempdir().unwrap(); let root = temp.path().to_owned();
        let candidate = v2_source(&*fs,&root); let source = candidate.source_path.clone();
        let mut e = Editor::new_from_text("disk",None,(80,24)); e.recovery.set_root(root.clone());
        let ex = crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock = crate::test_support::TestClock::new(7); let (tx,_) = std::sync::mpsc::channel();
        begin_selected(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()},vec![candidate.clone()]);
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        cancel_batch(&mut e);
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        assert!(source.exists());
        assert!(crate::test_support::recovery_prepare_released(&*fs,&root,&candidate).is_ok());
        assert!(!has_pending_work(&e));
    }
    #[test]
    fn recovery_handoff_quit_rejects_prepared_installation() {
        let fs = crate::test_support::test_fs(); let temp = tempfile::tempdir().unwrap(); let root = temp.path().to_owned();
        let candidate = v2_source(&*fs,&root); let source = candidate.source_path.clone();
        let mut e = Editor::new_from_text("disk",None,(80,24)); e.recovery.set_root(root);
        let original = e.active().id; let count = e.buffers.len();
        let ex = crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock = crate::test_support::TestClock::new(7); let (tx,_) = std::sync::mpsc::channel();
        begin_selected(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()},vec![candidate]);
        let ready = ex.run_next(); e.quit=true;
        crate::jobs_apply::apply_job_outcome(ready,&mut e,&ex,&clock,&tx,&fs);
        assert_eq!(e.active().id,original); assert_eq!(e.buffers.len(),count); assert!(source.exists());
        assert!(!has_pending_work(&e)); assert_eq!(ex.pending_len(),0);
    }
    #[test]
    fn recovery_handoff_repeat_retired_v2_focuses_protected_buffer_without_source_io() {
        let fs = crate::test_support::test_fs(); let temp = tempfile::tempdir().unwrap();
        let candidate = v2_source(&*fs,temp.path()); let source = candidate.source_path.clone();
        let mut e = Editor::new_from_text("disk",None,(80,24)); e.recovery.set_root(temp.path().to_owned());
        let ex = crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock = crate::test_support::TestClock::new(7); let (tx,_) = std::sync::mpsc::channel();
        begin_selected(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()},vec![candidate.clone()]);
        for _ in 0..2 { crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs); }
        assert!(!source.exists()); let recovered = e.active().id;
        let successor = e.active().recovery_ack.as_ref().unwrap().path().to_owned();
        let generation = e.active().recovery_generation;
        let count = e.buffers.len(); let events = e.pending_plugin_events.len();
        assert!(events > 0,"initial recovered Open event was emitted");
        e.switch_to_index(0); assert_ne!(e.active().id,recovered);
        begin_selected(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()},vec![candidate.clone()]);
        assert_eq!(e.active().id,recovered,"retained row focuses the already protected copy");
        assert_eq!(ex.pending_len(),0,"no prepare or checkpoint is dispatched");
        assert_eq!(e.buffers.len(),count); assert_eq!(e.pending_plugin_events.len(),events);
        assert_eq!(e.active().recovery_generation,generation);
        assert!(!source.exists()); assert!(successor.exists());
        // A focus must not cancel or replace an already queued checkpoint request.
        dispatch_checkpoint(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()},recovered);
        let pending = e.active().recovery_request;
        let pending_generation = e.active().recovery_generation;
        e.switch_to_index(0);
        begin_selected(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()},vec![candidate.clone()]);
        assert_eq!(e.active().id,recovered); assert_eq!(ex.pending_len(),1);
        assert_eq!(e.active().recovery_request,pending); assert_eq!(e.active().recovery_generation,pending_generation);
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        assert!(e.active().recovery_request.is_none());
        assert_eq!(e.pending_plugin_events.len(),events);
        // Saving the document can remove its checkpoint; clean documents still focus.
        e.active_mut().document.saved_version=Some(e.active().document.version);
        e.active_mut().recovery_ack=None; e.active_mut().swapped_version=None;
        e.switch_to_index(0);
        begin_selected(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx,fs},vec![candidate]);
        assert_eq!(e.active().id,recovered); assert_eq!(ex.pending_len(),0);
        assert_eq!(e.pending_plugin_events.len(),events); assert!(!source.exists());
    }

    #[test]
    fn recovery_handoff_first_save_as_retires_successor_without_warning() {
        let fs=crate::test_support::test_fs(); let temp=tempfile::tempdir().unwrap();
        let candidate=v2_source(&*fs,temp.path()); let source=candidate.source_path.clone();
        let mut e=Editor::new_from_text("disk",None,(80,24)); e.recovery.set_root(temp.path().to_owned());
        let ex=crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock=crate::test_support::TestClock::new(7); let (tx,_)=std::sync::mpsc::channel();
        begin_selected(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()},vec![candidate]);
        for _ in 0..2 { crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs); }
        let successor=e.active().recovery_ack.as_ref().unwrap().path().to_owned();
        let target=temp.path().join("named.md");
        crate::save::do_save_to(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()},
            crate::save::SaveTarget::same(target.clone()),crate::save::SaveMode::SaveAs);
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        assert!(!source.exists()); assert!(!successor.exists()); assert!(!e.active().document.dirty());
        assert_eq!(e.status().unwrap().kind(),crate::status::StatusKind::Info);
        assert_eq!(std::fs::read_to_string(target).unwrap(),"original rescued text");
        assert_eq!(ex.pending_len(),0);
    }

    #[test]
    fn recovery_polish_cancelled_handoff_latches_only_matching_durable_snapshot() {
        for change in ["none", "edit", "path", "generation", "replacement", "closed", "failure", "panic_after_ack"] {
            let fs=crate::test_support::test_fs(); let temp=tempfile::tempdir().unwrap();
            let candidate=v2_source(&*fs,temp.path()); let source=candidate.source_path.clone();
            let mut e=Editor::new_from_text("disk",None,(80,24)); e.recovery.set_root(temp.path().to_owned());
            let ex=crate::recovery_regressions::DeferredRecoveryExecutor::default();
            let clock=crate::test_support::TestClock::new(7); let (tx,_)=std::sync::mpsc::channel();
            begin_selected(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()},
                vec![candidate.clone(),candidate]);
            crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
            let version=e.active().document.version;
            let request=e.active().recovery_request.unwrap();
            cancel_batch(&mut e);
            e.active_mut().recovery_protection_failure=Some("earlier failure".into());
            match change {
                "edit" => {
                    let len=e.active().document.buffer.len();
                    crate::transact::submit_transaction(&mut e,wordcartel_core::history::Transaction::new(
                        wordcartel_core::change::ChangeSet::insert(len," later edit",len)),&clock).unwrap();
                },
                "path" => e.active_mut().document.path=Some(temp.path().join("changed.md")),
                "generation" => e.active_mut().recovery_generation+=1,
                "closed" => { e.switch_to_index(0); e.buffers.pop(); },
                "replacement" => { let id=e.active().id; *e.active_mut()=crate::editor::Buffer::from_text(id,"replacement",None,(80,24)); },
                _ => (),
            }
            e.quit=true;
            let history=e.status_history().entries().len();
            if change=="failure" {
                assert!(panic_request(&mut e,request,"cancelled worker failed"));
            } else if change=="panic_after_ack" {
                let _outcome=ex.run_next();
                assert!(panic_request(&mut e,request,"late failure after durable checkpoint"));
            } else {
                crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
            }
            assert!(source.exists(),"cancelled retirement: {change}");
            assert!(e.quit,"late completion must not cancel quit: {change}");
            assert_eq!(e.status_history().entries().len(),history,"late status: {change}");
            assert_eq!(e.buffers.len(),if change=="closed" {1} else {2},"remaining selection cancelled");
            assert!(!has_pending_work(&e));
            if matches!(change,"none"|"edit"|"panic_after_ack") {
                let ack=e.active().recovery_ack.as_ref().expect("cancelled durable snapshot is reflected");
                assert_eq!(recovery_store::decode(&std::fs::read(ack.path()).unwrap()).unwrap().1,"original rescued text");
                assert_eq!(e.active().swapped_version,Some(version));
                assert!(e.active().recovery_protection_failure.is_none());
                assert_eq!(protected(e.active()),change!="edit","later edits remain unprotected");
            } else { assert!(e.active().recovery_ack.is_none(),"nonmatching or absent ack: {change}"); }
        }
    }

}
