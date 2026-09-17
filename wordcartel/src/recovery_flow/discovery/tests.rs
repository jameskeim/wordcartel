#[cfg(test)]
mod tests {
    use super::super::*;
    #[test]
    fn recovery_picker_scan_cancelled_epoch_cannot_reopen() {
        let mut e=Editor::new_from_text("disk",None,(80,24));
        let ex=crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock=crate::test_support::TestClock(0); let (tx,_)=std::sync::mpsc::channel(); let fs=crate::test_support::test_fs();
        review(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        crate::recovery_picker::close(&mut e);
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx,fs});
        assert!(e.recovery_picker.is_none());
    }

    #[test]
    fn recovery_picker_bootstrap_waits_for_prompt_but_replaces_splash_without_input() {
        let dir=tempfile::tempdir().unwrap(); std::fs::write(dir.path().join("recovered-first.md"),"first").unwrap();
        let mut e=Editor::new_from_text("disk",None,(80,24)); e.recovery.set_root(dir.path().to_owned());
        let ex=crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock=crate::test_support::TestClock(0); let (tx,_)=std::sync::mpsc::channel(); let fs=crate::test_support::test_fs();
        e.open_prompt(crate::prompt::Prompt::external_mod());
        bootstrap(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        assert!(e.prompt.is_some()); assert!(e.recovery_picker.is_none());
        crate::overlays::close_all(&mut e); e.splash=Some(crate::splash::Splash::new(&crate::keymap::KeyTrie::default(),"0.0.0"));
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        assert!(e.splash.is_none()); assert!(e.recovery_picker.is_some());
        bootstrap(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx,fs}); assert_eq!(ex.pending_len(),0);
    }
    #[test]
    fn recovery_picker_context_coalesces_and_stale_origin_never_interrupts() {
        let dir=tempfile::tempdir().unwrap(); std::fs::write(dir.path().join("recovered-first.md"),"first").unwrap();
        let path=dir.path().join("disk.md"); let mut e=Editor::new_from_text("disk",Some(path.clone()),(80,24)); e.recovery.set_root(dir.path().to_owned());
        let fs_source=crate::test_support::test_fs(); let slot=RecoverySlot::new();
        let record=CheckpointRecord::new(slot.reserve_generation().unwrap(),"0123456789abcdef".into(),0,Some(TaggedPath::from_path(&path)),None);
        recovery_store::checkpoint(&*fs_source,dir.path(),&slot,&record,"associated recovery").unwrap(); drop(slot);
        let id=e.active().id; opened(&mut e,id,&path); opened(&mut e,id,&path);
        assert_eq!(e.recovery.scans.queue.len(),1);
        let ex=crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock=crate::test_support::TestClock(0); let (tx,_)=std::sync::mpsc::channel(); let fs=crate::test_support::test_fs();
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        opened(&mut e,id,&path); assert_eq!(e.recovery.scans.queue.len(),1);
        let new=e.alloc_id(); e.buffers.push(crate::editor::Buffer::from_text(new,"other",None,(80,24))); e.switch_to_index(1);
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        assert!(e.recovery_picker.is_none());
        // Drain the coalesced follow-up before the independent manual all-scan.
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        review(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx,fs});
        assert!(e.recovery_picker.is_some());
    }
    #[test]
    fn recovery_picker_manual_busy_guard_preserves_prompt() {
        let mut e=Editor::new_from_text("disk",None,(80,24)); e.open_prompt(crate::prompt::Prompt::external_mod());
        let ex=crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock=crate::test_support::TestClock(0); let (tx,_)=std::sync::mpsc::channel();
        review(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx,fs:crate::test_support::test_fs()});
        assert!(e.prompt.is_some()); assert!(e.recovery_picker.is_none()); assert_eq!(ex.pending_len(),0);
    }
    #[test]
    fn recovery_picker_scan_panic_routes_exact_id_and_old_scan_cannot_replace_new_ui() {
        let mut e=Editor::new_from_text("disk",None,(80,24));
        let ex=crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock=crate::test_support::TestClock(0); let (tx,_)=std::sync::mpsc::channel(); let fs=crate::test_support::test_fs();
        review(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        let old=*e.recovery.scans.requests.keys().next().unwrap();
        crate::recovery_picker::close(&mut e);
        review(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        let new=*e.recovery.scans.requests.keys().next().unwrap(); assert_ne!(old,new);
        assert!(!panic_request(&mut e,old,"old failure"));
        assert!(e.recovery_picker.as_ref().unwrap().is_loading());
        assert!(panic_request(&mut e,new,"new failure"));
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        assert!(!e.recovery_picker.as_ref().unwrap().is_loading());
        assert!(!panic_request(&mut e,new,"duplicate failure"));
    }
    #[test]
    fn recovery_picker_scan_rejection_is_visible_and_idle_does_not_scan() {
        let mut e=Editor::new_from_text("disk",None,(80,24));
        let ex=crate::recovery_regressions::RejectingRecoveryExecutor;
        let clock=crate::test_support::TestClock(0); let (tx,_)=std::sync::mpsc::channel();
        review(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx,fs:crate::test_support::test_fs()});
        assert!(e.recovery_picker.is_some()); assert!(!e.recovery_picker.as_ref().unwrap().is_loading());
        assert!(e.recovery.scans.requests.is_empty());
        let ex=crate::recovery_regressions::DeferredRecoveryExecutor::default(); let (tx,_)=std::sync::mpsc::channel();
        for _ in 0..5 { drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:crate::test_support::test_fs()}); }
        assert_eq!(ex.pending_len(),0);
    }
    #[test]
    fn recovery_picker_corrupt_row_dismisses_across_contexts_but_manual_remains_visible() {
        let dir=tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("junk.swp"),"invalid header").unwrap();
        let first=dir.path().join("first.md"); let second=dir.path().join("second.md");
        let mut e=Editor::new_from_text("disk",Some(first.clone()),(80,24));
        e.recovery.set_root(dir.path().to_owned()); let id=e.active().id;
        let ex=crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock=crate::test_support::TestClock(0); let (tx,_)=std::sync::mpsc::channel();
        let fs=crate::test_support::test_fs();
        opened(&mut e,id,&first);
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        assert!(e.recovery_picker.is_some());
        // Closing this offer must retain another document's queued assessment.
        opened(&mut e,id,&second); crate::recovery_picker::close(&mut e);
        assert_eq!(e.recovery.scans.queue.len(),1);
        e.active_mut().document.path=Some(second);
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        assert_eq!(ex.pending_len(),1);
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        assert!(e.recovery_picker.is_none(),"same unavailable row does not interrupt again");
        review(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx,fs});
        assert!(e.recovery_picker.is_some()); assert!(!e.recovery_picker.as_ref().unwrap().is_loading());
    }

    #[test]
    fn recovery_picker_manual_scan_completes_ahead_of_deferred_automatic_offer() {
        let dir=tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("junk.swp"),"invalid header").unwrap();
        let path=dir.path().join("first.md");
        let mut e=Editor::new_from_text("disk",Some(path.clone()),(80,24));
        e.recovery.set_root(dir.path().to_owned()); let id=e.active().id;
        let ex=crate::recovery_regressions::DeferredRecoveryExecutor::default();
        let clock=crate::test_support::TestClock(0); let (tx,_)=std::sync::mpsc::channel();
        let fs=crate::test_support::test_fs();
        opened(&mut e,id,&path);
        drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        review(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        assert!(e.recovery_picker.as_ref().unwrap().is_loading());
        for _ in 0..2 {
            crate::jobs_apply::apply_job_outcome(ex.run_next(),&mut e,&ex,&clock,&tx,&fs);
            drive(&mut Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx.clone(),fs:fs.clone()});
        }
        assert!(!e.recovery_picker.as_ref().unwrap().is_loading());
        assert_eq!(ex.pending_len(),0);
    }

}
