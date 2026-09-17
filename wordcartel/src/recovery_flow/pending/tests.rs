#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::recovery_regressions::DeferredRecoveryExecutor;
    use crate::test_support::{test_fs, TestClock};
    fn editor() -> (tempfile::TempDir, Editor) {
        let tmp = tempfile::tempdir().unwrap();
        let mut e = Editor::new_from_text("disk", None, (80,24));
        e.recovery.set_root(tmp.path().to_owned()); (tmp,e)
    }
    fn start_import(e: &mut Editor, ex: &DeferredRecoveryExecutor, root: &Path) -> PathBuf {
        let fs = test_fs(); let slot = RecoverySlot::new();
        let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "0123456789abcdef".into(),0,None,None);
        let source = recovery_store::checkpoint(&*fs,root,&slot,&record,"rescued").unwrap().path().to_owned(); drop(slot);
        let rows = crate::recovery_discovery::scan(&*fs,root,&crate::recovery_discovery::ScanScope::All).unwrap();
        let (tx,_) = std::sync::mpsc::channel();
        begin_selected(&mut Ctx {editor:e,executor:ex,clock:&TestClock::new(10),msg_tx:tx,fs},rows); source
    }
    fn merge(e: &mut Editor, ex: &DeferredRecoveryExecutor) {
        let (tx,_) = std::sync::mpsc::channel();
        crate::jobs_apply::apply_job_outcome(ex.run_next(),e,ex,&TestClock::new(10),&tx,&test_fs());
    }
    fn exit_barrier(e: &mut Editor, ex: &DeferredRecoveryExecutor) {
        let (tx,_) = std::sync::mpsc::channel();
        let mut ctx = Ctx {editor:e,executor:ex,clock:&TestClock::new(10),msg_tx:tx,fs:test_fs()};
        after_callbacks(&mut ctx); crate::quit::after_callbacks(&mut ctx);
    }
    #[test]
    fn recovery_quit_handoff_waits_until_merge_and_rechecks_dirty_buffers() {
        let (tmp,mut e) = editor(); let ex = DeferredRecoveryExecutor::default();
        start_import(&mut e,&ex,tmp.path()); assert!(!has_pending_work(&e));
        merge(&mut e,&ex); assert!(has_pending_work(&e));
        // A successful save may have cleaned the recovered document while IO is outstanding.
        e.active_mut().document.saved_version = Some(e.active().document.version);
        e.quit=true; exit_barrier(&mut e,&ex); assert!(!e.quit); assert!(e.quit_drain.is_some());
        merge(&mut e,&ex); assert!(e.quit); exit_barrier(&mut e,&ex); assert!(e.quit);
    }
    #[test]
    fn recovery_quit_timeout_detaches_once_and_late_success_cannot_reinstate_quit() {
        let (tmp,mut e) = editor(); let ex = DeferredRecoveryExecutor::default();
        let source = start_import(&mut e,&ex,tmp.path()); merge(&mut e,&ex);
        e.quit=true;
        let timer = crate::timers::SUBSYSTEMS.iter().find(|s|s.name=="recovery").unwrap();
        assert_eq!((timer.deadline)(&e,10),Some(5010));
        crate::timers::pre_recv(&mut e,5009); assert!(e.quit); assert!(has_pending_work(&e));
        crate::timers::pre_recv(&mut e,5010); assert!(!e.quit); assert!(!has_pending_work(&e));
        assert_eq!(e.status_text(),"Recovery IO is still pending; quit cancelled");
        assert_eq!((timer.deadline)(&e,5010),None);
        e.quit=true; crate::timers::pre_recv(&mut e,9000); assert!(e.quit,"timeout cannot cancel next quit");
        crate::quit::cancel(&mut e); merge(&mut e,&ex); assert!(!e.quit); assert!(source.exists());
        assert!(e.active().recovery_request.is_none()); assert!(!imports_busy(&e));
    }
    #[test]
    fn recovery_quit_preparation_cancellation_never_installs_after_quit_cancelled() {
        let (tmp,mut e) = editor(); let ex = DeferredRecoveryExecutor::default();
        let source = start_import(&mut e,&ex,tmp.path()); let count=e.buffers.len();
        e.quit=true; exit_barrier(&mut e,&ex);
        assert_eq!(pending_deadline(&e,5010),None,"preparation does not block exit");
        crate::quit::cancel(&mut e);
        merge(&mut e,&ex); assert_eq!(e.buffers.len(),count); assert!(source.exists());
    }
    #[test]
    fn recovery_quit_discard_cancels_retirement_but_waits_for_dispatched_worker() {
        let (tmp,mut e) = editor(); let ex=DeferredRecoveryExecutor::default();
        let source=start_import(&mut e,&ex,tmp.path()); merge(&mut e,&ex);
        let id=e.active().id; let version=e.active().document.version;
        let mut drain=crate::editor::QuitDrain::new([id].into(),crate::editor::QuitMode::ReviewEach);
        drain.reviewing=Some((id,version)); e.quit_drain=Some(drain);
        let (tx,_) = std::sync::mpsc::channel();
        crate::quit::review_discard(&mut Ctx {editor:&mut e,executor:&ex,clock:&TestClock::new(10),msg_tx:tx,fs:test_fs()});
        assert!(!e.quit); assert!(has_pending_work(&e)); merge(&mut e,&ex);
        assert!(source.exists()); assert!(e.quit);
        e.active_mut().document.version+=1; exit_barrier(&mut e,&ex);
        assert!(!e.quit,"post-discard edit must be reviewed"); assert!(e.prompt.is_some());
    }
    #[test]
    fn recovery_quit_association_wait_failure_panic_and_timeout_are_exact() {
        for disposition in ["success","panic","timeout"] {
            let (_tmp,mut e)=editor(); let ex=DeferredRecoveryExecutor::default();
            e.active_mut().document.saved_version=None; let id=e.active().id;
            association_changed(&mut e,id); let (tx,_) = std::sync::mpsc::channel();
            after_callbacks(&mut Ctx {editor:&mut e,executor:&ex,clock:&TestClock::new(10),msg_tx:tx,fs:test_fs()});
            assert!(has_pending_work(&e)); e.active_mut().document.saved_version=Some(e.active().document.version);
            e.quit=true; exit_barrier(&mut e,&ex); assert!(!e.quit);
            match disposition {
                "success" => { merge(&mut e,&ex); assert!(e.quit); },
                "panic" => { let request=e.active().recovery_request.unwrap(); on_panic(&mut e,request,"injected"); assert!(e.quit_drain.is_none()); assert!(e.status_text().contains("quit cancelled")); },
                _ => { crate::timers::pre_recv(&mut e,5010); assert!(e.quit_drain.is_none()); merge(&mut e,&ex); assert!(!e.quit); }
            }
            assert!(!has_pending_work(&e));
        }
    }
    #[test]
    fn recovery_quit_timeout_clears_blocked_intent_but_preserves_unrelated_intent() {
        let (_tmp,mut e)=editor(); let ex=DeferredRecoveryExecutor::default();
        let first=e.active().id; e.active_mut().document.saved_version=None;
        let (tx,_) = std::sync::mpsc::channel();
        dispatch_checkpoint(&mut Ctx {editor:&mut e,executor:&ex,clock:&TestClock::new(10),msg_tx:tx,fs:test_fs()},first);
        assert!(!has_pending_work(&e),"ordinary checkpoint is not an exit blocker");
        association_changed(&mut e,first);
        let second=e.alloc_id(); let mut buffer=crate::editor::Buffer::from_text(second,"second",None,(80,24));
        buffer.document.saved_version=None; e.buffers.push(buffer); association_changed(&mut e,second);
        assert!(has_pending_work(&e)); e.quit=true; timeout_tick(&mut e,5010);
        assert_eq!(e.recovery.associations.len(),1); assert_eq!(e.recovery.associations[0].0,second);
        merge(&mut e,&ex); assert_eq!(ex.pending_len(),1,"unrelated intent still dispatches");
        merge(&mut e,&ex); assert!(!has_pending_work(&e));
    }
    #[test]
    fn recovery_quit_cancellation_releases_filename_continuation_and_preserves_close() {
        for disposition in ["timeout", "panic", "failure"] {
            let (_tmp,mut e)=editor(); let ex=DeferredRecoveryExecutor::default();
            e.active_mut().document.saved_version=None; let id=e.active().id;
            association_changed(&mut e,id); let (tx,_) = std::sync::mpsc::channel();
            after_callbacks(&mut Ctx {editor:&mut e,executor:&ex,clock:&TestClock::new(10),msg_tx:tx.clone(),fs:test_fs()});
            e.quit_drain=Some(crate::editor::QuitDrain::new([id].into(),crate::editor::QuitMode::SaveAll));
            e.pending_save_as=Some(crate::editor::PostSaveAction::ContinueQuitDrain);
            assert!(crate::prompts::open_save_as_for_quit(&mut e,&test_fs(),&tx));
            assert_eq!(e.file_browser.as_ref().unwrap().quit_save_owner,Some(id));
            let request=e.active().recovery_request.unwrap();
            match disposition {
                "timeout" => timeout_tick(&mut e,5010),
                "panic" => on_panic(&mut e,request,"injected panic"),
                _ => complete(&mut e,request,Err("injected failure".into())),
            }
            assert!(!crate::quit::in_progress(&e),"{disposition}: cancelled quit must release Save As continuation");
            assert!(e.pending_save_as.is_none());
            assert!(e.file_browser.as_ref().unwrap().quit_save_owner.is_none());
            assert!(crate::quit::allow_manual_save_as(&mut e));
            assert!(crate::quit::allow_save_as_picker(&mut e,None),"existing filename picker can continue as a manual save");
            let close=crate::editor::PostSaveAction::CloseBuffer {id};
            e.pending_save_as=Some(close.clone()); crate::quit::cancel(&mut e);
            assert_eq!(e.pending_save_as,Some(close),"nonquit continuation must survive");
        }
    }
    #[test]
    fn recovery_quit_cancellation_downgrades_existing_overwrite_to_manual_save() {
        let (tmp,mut e)=editor(); let ex=DeferredRecoveryExecutor::default();
        let target=tmp.path().join("named.md"); std::fs::write(&target,"old").unwrap();
        e.active_mut().document.saved_version=None;
        e.pending_save_as=Some(crate::editor::PostSaveAction::ContinueQuitDrain);
        e.pending_save_overwrite=Some(target.clone()); e.pending_save_as_chosen=Some(target.clone());
        e.prompt=Some(crate::prompt::Prompt::save_overwrite(&target));
        crate::quit::cancel(&mut e);
        let (tx,_) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(crate::prompt::PromptAction::OverwriteSaveAs,&mut e,&ex,&TestClock::new(10),&tx,&test_fs());
        assert_eq!(ex.pending_len(),1); assert!(e.pending_after_save.is_none()); assert!(!crate::quit::in_progress(&e));
        merge(&mut e,&ex); assert_eq!(std::fs::read_to_string(target).unwrap(),"disk"); assert!(!e.quit);
    }
    #[test]
    fn recovery_background_slow_checkpoint_latches_without_warning_or_retry() {
        let (_tmp,mut e)=editor(); let ex=DeferredRecoveryExecutor::default();
        e.active_mut().document.saved_version=None; e.active_mut().last_edit_at=Some(0);
        let id=e.active().id; let (tx,_) = std::sync::mpsc::channel();
        dispatch_checkpoint(&mut Ctx {editor:&mut e,executor:&ex,clock:&TestClock::new(0),msg_tx:tx,fs:test_fs()},id);
        assert_eq!(pending_deadline(&e,6000),None); timeout_tick(&mut e,6000);
        assert!(e.status_text().is_empty()); merge(&mut e,&ex);
        assert!(e.active().recovery_ack.is_some()); assert_eq!(e.active().swapped_version,Some(e.active().document.version));
        assert_eq!(crate::timers::next_wake(&e,6000),None); assert_eq!(ex.pending_len(),0);
    }
    #[test]
    fn recovery_background_slow_handoff_does_not_cancel_batch_or_retirement() {
        let (tmp,mut e)=editor(); let ex=DeferredRecoveryExecutor::default();
        let source=start_import(&mut e,&ex,tmp.path()); merge(&mut e,&ex);
        assert_eq!(pending_deadline(&e,6000),None); timeout_tick(&mut e,6000);
        assert!(e.status_text().is_empty()); merge(&mut e,&ex);
        assert!(!source.exists()); assert!(e.active().recovery_ack.is_some());
    }
    #[test]
    fn recovery_quit_detached_association_latches_success_but_late_error_leaves_new_quit() {
        for success in [false,true] {
            let (_tmp,mut e)=editor(); let ex=DeferredRecoveryExecutor::default();
            e.active_mut().document.saved_version=None; let id=e.active().id;
            association_changed(&mut e,id); let (tx,_) = std::sync::mpsc::channel();
            after_callbacks(&mut Ctx {editor:&mut e,executor:&ex,clock:&TestClock::new(0),msg_tx:tx,fs:test_fs()});
            e.quit=true; timeout_tick(&mut e,6000); assert!(!e.quit);
            let request=e.active().recovery_request.unwrap();
            if success { merge(&mut e,&ex); assert!(e.active().recovery_ack.is_some()); }
            else { e.quit=true; complete(&mut e,request,Err("late error".into())); assert!(e.quit); }
        }
    }

}
