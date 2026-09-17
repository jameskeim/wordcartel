#[cfg(test)]
mod tests {
    use super::super::*;
    #[test]
    fn recovery_picker_dismiss_suppresses_exact_tokens_without_changing_documents() {
        let mut e = Editor::new_from_text("disk", None, (80,24));
        let row = candidate(); let token = row.token.clone().unwrap();
        e.recovery_picker = Some(RecoveryPicker::ready(vec![row]));
        close(&mut e);
        assert!(e.recovery_picker.is_none());
        assert!(e.recovery.scans.dismissed.contains(&token));
        assert_eq!(e.active().document.buffer.to_string(), "disk");
    }
    fn candidate() -> Candidate {
        Candidate { source_path: "/tmp/recovered-test.md".into(), token: Some(crate::recovery_discovery::SelectionToken::Legacy {
            path: "/tmp/recovered-test.md".into(), len: 4, mtime: None, body_hash: 7 }),
            association: None, provenance: None, lineage: None, timestamp: crate::recovery_discovery::CandidateTime::Unknown,
            preview: "test".into(), unavailable: None, busy: false }
    }
    #[test]
    fn recovery_picker_zero_selection_stays_open() {
        let mut e = Editor::new_from_text("disk",None,(80,24));
        e.recovery_picker = Some(RecoveryPicker::ready(vec![candidate()]));
        let ex = crate::jobs::InlineExecutor::default(); let clock = crate::test_support::TestClock(0);
        let (tx,_) = std::sync::mpsc::channel();
        accept(&mut crate::registry::Ctx {editor:&mut e,executor:&ex,clock:&clock,msg_tx:tx,fs:crate::test_support::test_fs()});
        assert_eq!(e.recovery_picker.as_ref().unwrap().message,"Select a recovery file");
        assert_eq!(e.buffers.len(),1);
    }

    struct Harness {
        e: Editor, ex: crate::recovery_regressions::DeferredRecoveryExecutor,
        clock: crate::test_support::TestClock, tx: std::sync::mpsc::Sender<Msg>,
        fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>, _dir: tempfile::TempDir,
    }
    impl Harness {
        fn new() -> Self {
            let dir=tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("recovered-one.md"),"first rescued text").unwrap();
            std::fs::write(dir.path().join("recovered-two.md"),"second rescued text").unwrap();
            let mut e=Editor::new_from_text("disk",None,(80,24)); e.recovery.set_root(dir.path().to_owned());
            let (tx,_)=std::sync::mpsc::channel();
            Self {e,ex:Default::default(),clock:crate::test_support::TestClock(0),tx,fs:crate::test_support::test_fs(),_dir:dir}
        }
        fn run(&mut self, f: fn(&mut Ctx)) { f(&mut Ctx {editor:&mut self.e,executor:&self.ex,clock:&self.clock,msg_tx:self.tx.clone(),fs:self.fs.clone()}); }
        fn tick(&mut self) { self.run(crate::recovery_flow::after_callbacks); }
        fn job(&mut self) { crate::jobs_apply::apply_job_outcome(self.ex.run_next(),&mut self.e,&self.ex,&self.clock,&self.tx,&self.fs); self.tick(); }
        fn review(&mut self) { self.run(crate::recovery_flow::review); self.job(); }
        fn key(&mut self, code: KeyCode) {
            let reg=crate::registry::Registry::builtins(); let km=crate::keymap::KeyTrie::default();
            let dc=crate::overlays::DispatchCtx {reg:&reg,keymap:&km,ex:&self.ex,clock:&self.clock,msg_tx:&self.tx,fs:&self.fs};
            intercept(Msg::Input(Event::Key(crossterm::event::KeyEvent::new(code,crossterm::event::KeyModifiers::NONE))),&mut self.e,&dc);
        }
    }
    #[test]
    fn recovery_picker_subset_opening_cancel_and_manual_reopen() {
        let mut h=Harness::new(); h.review();
        let untouched=h.e.recovery_picker.as_ref().unwrap().rows[1].token.clone().unwrap();
        h.key(KeyCode::Char(' ')); h.key(KeyCode::Enter);
        assert!(h.e.recovery.scans.dismissed.contains(&untouched));
        assert_eq!(h.ex.pending_len(),1);
        h.job(); // installs separate buffer and dispatches first checkpoint
        let imported=h.e.active().id;
        assert_eq!(h.e.buffers.len(),2); assert_eq!(h.e.recovery_picker.as_ref().unwrap().phase,Phase::Opening);
        h.key(KeyCode::Esc); h.job();
        assert!(h.e.recovery_picker.is_none()); assert!(h.e.by_id(imported).is_some());
        assert_eq!(std::fs::read_dir(h._dir.path()).unwrap().filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x=="md")).count(),2);
        h.review(); assert_eq!(h.e.recovery_picker.as_ref().unwrap().rows.iter().filter(|r| r.token.is_some()).count(),2);
        assert!(h.e.recovery_picker.as_ref().unwrap().selected.iter().all(|s| !s));
    }
    #[test]
    fn recovery_picker_registry_replacement_cancels_prepare_before_install() {
        let mut h=Harness::new(); h.review(); h.key(KeyCode::Char(' ')); h.key(KeyCode::Enter);
        let reg=crate::registry::Registry::builtins();
        reg.dispatch(crate::registry::CommandId("palette"),&mut Ctx {editor:&mut h.e,executor:&h.ex,clock:&h.clock,msg_tx:h.tx.clone(),fs:h.fs.clone()});
        h.job(); assert_eq!(h.e.buffers.len(),1); assert!(h.e.palette.is_some());
    }
    #[test]
    fn recovery_picker_loading_close_does_not_cancel_unrelated_installed_handoff() {
        let mut h=Harness::new();
        let rows=crate::recovery_discovery::scan(&*h.fs,h._dir.path(),&crate::recovery_discovery::ScanScope::All).unwrap();
        crate::recovery_flow::begin_selected(&mut Ctx {editor:&mut h.e,executor:&h.ex,clock:&h.clock,msg_tx:h.tx.clone(),fs:h.fs.clone()},vec![rows[0].clone()]);
        h.job(); h.e.recovery_picker=Some(RecoveryPicker::loading()); close(&mut h.e); h.job();
        assert!(h.e.active().recovery_ack.is_some());
    }
    #[test]
    fn recovery_picker_changed_row_requires_new_selection_and_preview() {
        let mut h=Harness::new(); h.review();
        let source=h.e.recovery_picker.as_ref().unwrap().rows[0].source_path.clone();
        h.key(KeyCode::Char(' ')); h.key(KeyCode::Enter);
        std::fs::write(&source,"new generation, require consent").unwrap();
        h.job(); let p=h.e.recovery_picker.as_ref().unwrap();
        assert_eq!(h.e.buffers.len(),1); assert_eq!(p.rows[0].preview,"new generation, require consent");
        assert!(!p.selected[0]); assert_eq!(p.phase,Phase::Selecting);
    }
    #[test]
    fn recovery_picker_multi_selection_imports_sequentially_and_focuses_live_legacy() {
        let mut h=Harness::new(); h.review();
        h.key(KeyCode::Char(' ')); h.key(KeyCode::Down); h.key(KeyCode::Char(' ')); h.key(KeyCode::Enter);
        for expected in [2,2,3,3] { assert_eq!(h.ex.pending_len(),1); h.job(); assert_eq!(h.e.buffers.len(),expected); }
        assert_eq!(h.ex.pending_len(),0); h.key(KeyCode::Esc); h.review();
        h.key(KeyCode::Char(' ')); h.key(KeyCode::Enter);
        assert_eq!(h.e.buffers.len(),3); assert_eq!(h.ex.pending_len(),0);
    }
    #[test]
    fn recovery_picker_actual_render_and_mouse_share_scrolled_geometry() {
        for (w,height) in [(1,1),(12,4),(80,24)] {
            let mut h=Harness::new(); h.e.active_mut().view.area=(w,height);
            let mut rows=vec![candidate();40];
            for (i,row) in rows.iter_mut().enumerate() { row.preview=format!("preview {i}"); }
            h.e.recovery_picker=Some(RecoveryPicker::ready(rows));
            for _ in 0..30 { h.key(KeyCode::Down); }
            let mut terminal=ratatui::Terminal::new(ratatui::backend::TestBackend::new(w,height)).unwrap();
            terminal.draw(|f| crate::render::render(f,&mut h.e)).unwrap();
            let (_,list,_,_)=geometry(Rect::new(0,0,w,height));
            if list.height==0 { continue; }
            let top=h.e.recovery_picker.as_ref().unwrap().top;
            let reg=crate::registry::Registry::builtins(); let km=crate::keymap::KeyTrie::default();
            let dc=crate::overlays::DispatchCtx {reg:&reg,keymap:&km,ex:&h.ex,clock:&h.clock,msg_tx:&h.tx,fs:&h.fs};
            mouse(&mut h.e,MouseEvent {kind:MouseEventKind::Down(MouseButton::Left),column:list.x,row:list.y,modifiers:crossterm::event::KeyModifiers::NONE},Rect::new(0,0,w,height),&dc);
            let p=h.e.recovery_picker.as_ref().unwrap(); assert_eq!(p.cursor,top); assert!(p.selected[top]);
            if w>12 { let output=format!("{:?}",terminal.backend().buffer()); assert!(output.contains("Review Recovery Files")); }
        }
    }
    #[test]
    fn recovery_picker_click_away_uses_dismissal_and_preserves_source() {
        let mut h=Harness::new(); h.review();
        let reg=crate::registry::Registry::builtins(); let km=crate::keymap::KeyTrie::default();
        let dc=crate::overlays::DispatchCtx {reg:&reg,keymap:&km,ex:&h.ex,clock:&h.clock,msg_tx:&h.tx,fs:&h.fs};
        mouse(&mut h.e,MouseEvent {kind:MouseEventKind::Down(MouseButton::Left),column:0,row:0,modifiers:crossterm::event::KeyModifiers::NONE},Rect::new(0,0,100,30),&dc);
        assert!(h.e.recovery_picker.is_none()); assert_eq!(h.e.recovery.scans.dismissed.len(),2);
        assert!(h._dir.path().join("recovered-one.md").exists());
    }
    #[test]
    fn recovery_picker_civil_time_and_legacy_labels_are_honest() {
        assert_eq!(utc_time(0),"1970-01-01 00:00 UTC");
        assert_eq!(utc_time(951782400),"2000-02-29 00:00 UTC");
        let mut row=candidate(); row.timestamp=CandidateTime::LegacyMtime(None);
        assert!(timestamp(&row).contains("legacy time unknown"));
        row.timestamp=CandidateTime::LegacyMtime(Some(std::time::UNIX_EPOCH));
        assert!(timestamp(&row).contains("legacy file mtime 1970-01-01"));
    }

    fn rendered_picker(e: &mut Editor) -> Vec<String> {
        let mut terminal=ratatui::Terminal::new(ratatui::backend::TestBackend::new(88,24)).unwrap();
        terminal.draw(|frame| crate::render::render(frame,e)).unwrap();
        let buffer=terminal.backend().buffer();
        (0..24).map(|y| (0..88).map(|x| buffer[(x,y)].symbol()).collect()).collect()
    }
    #[test]
    fn recovery_picker_foreign_original_path_remains_visible() {
        let mut e=Editor::new_from_text("disk",None,(88,24));
        let mut row=candidate();
        #[cfg(unix)]
        { row.association=Some(crate::recovery_store::TaggedPath::Windows(r"C:\Drafts\foreign-draft.md".encode_utf16().collect())); }
        #[cfg(windows)]
        { row.association=Some(crate::recovery_store::TaggedPath::Unix(b"/drafts/foreign-draft.md".to_vec())); }
        let expected=row.association.as_ref().unwrap().escaped();
        e.recovery_picker=Some(RecoveryPicker::ready(vec![row.clone()]));
        let rendered=rendered_picker(&mut e).join("\n");
        assert!(rendered.contains("foreign-draft.md"),"{rendered}");
        assert!(rendered.contains(&expected),"escaped original path: {rendered}");
        row.provenance=row.association.take();
        e.recovery_picker=Some(RecoveryPicker::ready(vec![row]));
        assert!(rendered_picker(&mut e).join("\n").contains(&expected));
    }
    #[test]
    fn recovery_picker_long_path_cannot_hide_row_time_or_unavailability() {
        let mut e=Editor::new_from_text("disk",None,(88,24));
        let mut row=candidate();
        row.association=Some(crate::recovery_store::TaggedPath::from_path(&std::path::PathBuf::from(format!("/{}/draft.md","long-directory/".repeat(20)))));
        row.timestamp=CandidateTime::Checkpoint(951782400000);
        row.unavailable=Some("busy: another editor owns this copy".into()); row.busy=true;
        e.recovery_picker=Some(RecoveryPicker::ready(vec![row]));
        let rendered=rendered_picker(&mut e);
        let list=geometry(Rect::new(0,0,88,24)).1;
        assert!(rendered[list.y as usize].contains("2000-02-29"),"{}",rendered[list.y as usize]);
        assert!(rendered[list.y as usize].contains("busy"),"{}",rendered[list.y as usize]);
        assert!(rendered.join("\n").contains("An editor is using these recovery files"));
        assert!(rendered[list.y as usize].contains("draft.md"),"known filename remains visible");
        for _ in 0..30 { scroll_path(&mut e,true); }
        let rendered=rendered_picker(&mut e);
        let details=geometry(Rect::new(0,0,88,24)).2;
        assert!(rendered[details.y as usize].contains("draft.md"),"long original path can scroll to its end: {} offset {}",rendered[details.y as usize],e.recovery_picker.as_ref().unwrap().path_scroll);
    }
    #[test]
    fn recovery_polish_busy_owner_has_clear_explanation_and_cannot_open() {
        let mut h=Harness::new();
        let mut row=candidate();
        row.source_path="/tmp/recovery-v2/0123456789abcdef/checkpoint.wcr".into();
        row.token=None; row.busy=true; row.preview.clear();
        row.unavailable=Some("active: internal recovery lease contention".into());
        h.e.recovery_picker=Some(RecoveryPicker::ready(vec![row]));
        let rendered=rendered_picker(&mut h.e).join("\n");
        assert!(rendered.contains("Recovery files in use"),"{rendered}");
        assert!(rendered.contains("An editor is using these recovery files"),"{rendered}");
        let list=geometry(Rect::new(0,0,88,24)).1;
        assert!(!rendered.lines().nth(list.y as usize).unwrap().contains("checkpoint.wcr"),"{rendered}");
        assert!(rendered.contains("checkpoint.wcr"),"source path remains inspectable: {rendered}");
        assert!(!rendered.contains("internal recovery lease contention"),"{rendered}");
        h.key(KeyCode::Char(' ')); h.key(KeyCode::Enter);
        assert!(!h.e.recovery_picker.as_ref().unwrap().selected[0]);
        assert_eq!(h.ex.pending_len(),0); assert_eq!(h.e.buffers.len(),1);
    }

}
