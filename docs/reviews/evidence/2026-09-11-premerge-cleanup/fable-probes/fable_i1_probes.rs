//! Fable I1 verification probes (review-only; registered temporarily from `lib.rs`).
//! Each probe drives the production message path (`app::reduce` with a mouse event).

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use crate::editor::{Editor, PostSaveAction};
    use crate::jobs::{Executor, Job, JobOutcome};
    use crate::registry::{CommandId, Ctx, Registry};
    use crate::test_support::{test_fs, TestClock};

    #[derive(Default)]
    struct DeferredExecutor { jobs: RefCell<VecDeque<Job>> }

    impl Executor for DeferredExecutor {
        fn dispatch(&self, job: Job) { self.jobs.borrow_mut().push_back(job); }
        fn drain(&self) -> Vec<JobOutcome> { Vec::new() }
    }

    fn command(editor: &mut Editor, executor: &dyn Executor, name: &'static str) {
        let (tx, _rx) = std::sync::mpsc::channel();
        Registry::builtins().dispatch(CommandId(name), &mut Ctx {
            editor, executor, clock: &TestClock(0), msg_tx: tx, fs: test_fs(),
        });
    }

    fn insert(editor: &mut Editor, text: &str) {
        let len = editor.active().document.buffer.len();
        let cs = wordcartel_core::change::ChangeSet::insert(0, text, len);
        crate::transact::submit_transaction(editor,
            wordcartel_core::history::Transaction::new(cs), &TestClock(1)).unwrap();
    }

    fn click(editor: &mut Editor, ex: &dyn Executor, column: u16, row: u16) {
        let reg = Registry::builtins();
        let (keys, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mouse = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column, row, modifiers: crossterm::event::KeyModifiers::NONE,
        };
        crate::app::reduce(crate::app::Msg::Input(crossterm::event::Event::Mouse(mouse)),
            editor, &reg, &keys, ex, &TestClock(1), &tx, &test_fs());
    }

    fn quit_owned_picker() -> (Editor, DeferredExecutor) {
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "new ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_and_quit");
        crate::derive::rebuild(&mut e);
        assert!(e.file_browser.as_ref().is_some_and(|fb| fb.quit_save_owner.is_some()));
        assert_eq!(e.pending_save_as, Some(PostSaveAction::ContinueQuitDrain));
        (e, ex)
    }

    /// The delta must not broaden: a manual (non-owned) Save As picker click-away is still a
    /// plain close and leaves any unrelated pending action untouched (M3 is retained, not fixed).
    #[test]
    fn p1_manual_picker_click_away_is_a_plain_close_and_leaves_pending_action() {
        let mut e = Editor::new_from_text("body", None, (80, 24));
        insert(&mut e, "new ");
        let ex = DeferredExecutor::default();
        command(&mut e, &ex, "save_as");
        crate::derive::rebuild(&mut e);
        assert!(e.file_browser.as_ref().is_some_and(|fb| fb.quit_save_owner.is_none()));
        let id = e.active().id;
        e.pending_save_as = Some(PostSaveAction::CloseBuffer { id }); // stand-in for the close-buffer route
        click(&mut e, &ex, 0, 0);
        assert!(e.file_browser.is_none(), "click-away still closes a manual picker");
        assert_eq!(e.pending_save_as, Some(PostSaveAction::CloseBuffer { id }),
            "manual-picker closure is unchanged by the delta (M3 retained by design)");
        assert!(e.quit_drain.is_none());
        assert!(!e.quit);
    }

    /// Only the OUTSIDE branch changed: clicks inside the drawn overlay that do not land on a
    /// list row (borders, title, field, footer) must leave the quit-owned request intact.
    #[test]
    fn p2_quit_owned_clicks_inside_overlay_off_row_keep_ownership() {
        let (mut e, ex) = quit_owned_picker();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let r = {
            let fb = e.file_browser.as_ref().unwrap();
            crate::chrome_geom::file_browser_overlay_rect(area, fb)
        };
        let pal = crate::chrome_geom::palette_overlay_rect(area, e.file_browser.as_ref().unwrap().entries.len());
        eprintln!("drawn rect {r:?} vs click-away rect {pal:?}");
        let mut cells: Vec<(u16, u16)> = Vec::new();
        for row in r.y..r.y + r.height {
            cells.push((r.x, row)); // left border column: never a row hit
            cells.push((r.x + r.width - 1, row)); // right border column
        }
        cells.push((r.x + 1, r.y)); // title row
        cells.push((r.x + 1, r.y + 1)); // field / query row
        cells.push((r.x + 1, r.y + r.height - 1)); // footer row
        for (c, row) in cells {
            click(&mut e, &ex, c, row);
            assert!(e.file_browser.is_some(), "click at ({c},{row}) must not close the picker");
            assert!(e.file_browser.as_ref().unwrap().quit_save_owner.is_some(), "({c},{row})");
            assert_eq!(e.pending_save_as, Some(PostSaveAction::ContinueQuitDrain), "({c},{row})");
            assert!(e.quit_drain.is_some(), "({c},{row})");
        }
    }

    /// After a mouse cancellation nothing is left over: Save and Quit can restart with fresh
    /// ownership, and no warning about an in-progress quit is raised.
    #[test]
    fn p3_quit_owned_click_away_then_save_and_quit_restarts_cleanly() {
        let (mut e, ex) = quit_owned_picker();
        click(&mut e, &ex, 0, 0);
        assert!(e.file_browser.is_none());
        assert!(e.pending_save_as.is_none());
        assert!(e.pending_after_save.is_none());
        assert!(e.quit_drain.is_none());
        assert!(!e.quit);
        assert!(!e.quit_drain_advance);
        let status_after_cancel = e.status_text().to_string();
        eprintln!("status after mouse cancel: {status_after_cancel:?}");
        command(&mut e, &ex, "save_and_quit");
        assert!(e.file_browser.as_ref().is_some_and(|fb| fb.quit_save_owner.is_some()),
            "a fresh quit-owned picker opens; status: {:?}", e.status_text());
        assert_eq!(e.pending_save_as, Some(PostSaveAction::ContinueQuitDrain));
        assert!(!e.status_text().contains("in progress"));
    }

    /// The mouse route and the keyboard Esc route converge on the same end state.
    #[test]
    fn p4_mouse_click_away_matches_esc_end_state() {
        let (mut e_mouse, ex_m) = quit_owned_picker();
        click(&mut e_mouse, &ex_m, 0, 0);
        let (mut e_esc, _ex_e) = quit_owned_picker();
        crate::file_browser::cancel_destination(&mut e_esc);
        let snap = |e: &Editor| (e.file_browser.is_none(), e.pending_save_as.clone(), e.pending_save_overwrite.is_none(),
            e.pending_save_as_chosen.is_none(), e.pending_write_block.is_none(), e.pending_export.is_none(),
            e.quit, e.quit_drain.is_none(), e.pending_after_save.is_none(), e.status_text().to_string());
        assert_eq!(snap(&e_mouse), snap(&e_esc));
    }

    /// Geometry check: once the destination field is non-empty the drawn box reserves a
    /// footer row (`file_browser_overlay_rect`), but the click-away test in `mouse_file_browser`
    /// sizes its rect from `entries.len()` alone. Records whether a click on the drawn footer
    /// row is treated as click-away, and if so, that the quit is cancelled cleanly (never orphaned).
    #[test]
    fn p5_click_on_drawn_footer_row_of_quit_owned_picker() {
        let (mut e, ex) = quit_owned_picker();
        if let Some(fb) = e.file_browser.as_mut() {
            if let crate::file_browser::BrowseMode::Destination { field, field_cursor, .. } = &mut fb.mode {
                field.push_str("name.md");
                *field_cursor = field.len();
            }
        }
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let (r, pal, entries, reserved) = {
            let fb = e.file_browser.as_ref().unwrap();
            (crate::chrome_geom::file_browser_overlay_rect(area, fb),
             crate::chrome_geom::palette_overlay_rect(area, fb.entries.len()),
             fb.entries.len(), crate::chrome_geom::file_browser_footer_rows(fb))
        };
        eprintln!("entries={entries} reserved={reserved} drawn={r:?} click_away_rect={pal:?}");
        assert!(reserved >= 1, "a non-empty destination field reserves a footer row");
        // Click on the drawn box's last interior row (the footer), one column in from the border.
        let footer_row = r.y + r.height - 2;
        let in_click_rect = footer_row >= pal.y && footer_row < pal.y + pal.height;
        click(&mut e, &ex, r.x + 1, footer_row);
        eprintln!("footer row {footer_row} inside click-away rect: {in_click_rect}; picker open after click: {}",
            e.file_browser.is_some());
        if e.file_browser.is_none() {
            // Treated as click-away: the delta guarantees a clean cancel rather than an orphan.
            assert!(e.pending_save_as.is_none(), "must never orphan the quit-owned request");
            assert!(e.quit_drain.is_none());
            assert!(!e.quit);
        } else {
            assert_eq!(e.pending_save_as, Some(PostSaveAction::ContinueQuitDrain));
        }
        assert_eq!(r.height, pal.height + reserved as u16,
            "drawn box and click-away box differ by the reserved footer rows");
    }
}
