//! Mouse coordinate translation and gesture dispatch.
use crossterm::event::{MouseEvent, MouseEventKind, MouseButton, KeyModifiers};
use crate::editor::Editor;

/// Classification of a terminal cell hit relative to the editing layout.
pub enum CellHit {
    Text { col: u16, erow: u16 },
    MenuBar,
    Status,
    #[allow(dead_code)] // wired in Task 6
    Scrollbar,
    #[allow(dead_code)] // wired in Task 5
    Outside,
}

/// Classify a terminal cell `(col, row)` into the editing layout regions.
pub fn editing_cell(editor: &Editor, col: u16, row: u16) -> CellHit {
    let (w, h) = editor.active().view.area;
    let menu_rows: u16 = u16::from(editor.menu.is_some());
    if h == 0 {
        return CellHit::Outside;
    }
    if row == h - 1 {
        return CellHit::Status;
    }
    if menu_rows == 1 && row == 0 {
        return CellHit::MenuBar;
    }
    if editor.mouse.scrollbar_visible && col == w.saturating_sub(1) {
        return CellHit::Scrollbar;
    }
    let erow = row.saturating_sub(menu_rows);
    let edit_height = h.saturating_sub(1 + menu_rows);
    if erow < edit_height {
        CellHit::Text { col, erow }
    } else {
        CellHit::Outside
    }
}

/// Dispatch a mouse event, updating editor state for the current gesture.
///
/// Early-returns when `pending_mark` is Some (mark-capture in progress) or
/// when `mouse_capture` is disabled.  Left-click → caret placement, Shift+click
/// → extend selection, Drag → drag-select with edge auto-scroll, Up → clear
/// dragging.
pub fn handle(
    editor: &mut Editor,
    ev: MouseEvent,
    _reg: &crate::registry::Registry,
    _keymap: &crate::keymap::KeyTrie,
    _ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    _msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    if editor.pending_mark.is_some() || !editor.mouse_capture {
        return;
    }
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let CellHit::Text { col, erow } = editing_cell(editor, ev.column, ev.row) {
                let buf_len = editor.active().document.buffer.len();
                let off = match crate::nav::offset_at_cell(editor, col, erow) {
                    Some(o) => crate::nav::clamp_snap(editor, o),
                    None => buf_len,
                };
                editor.active_mut().sel_history.clear();
                if ev.modifiers.contains(KeyModifiers::SHIFT) {
                    let anchor = editor.active().document.selection.primary().anchor;
                    editor.active_mut().document.selection =
                        wordcartel_core::selection::Selection::range(anchor, off);
                    editor.mouse.anchor = Some(anchor);
                } else {
                    editor.active_mut().document.selection =
                        wordcartel_core::selection::Selection::single(off);
                    editor.mouse.anchor = Some(off);
                    // (multi-click in Task 5)
                }
                editor.mouse.dragging = true;
                let _ = clock; // multi-click timing wired in Task 5
                crate::derive::rebuild(editor);
                crate::nav::ensure_visible(editor);
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if !editor.mouse.dragging { return; }
            let (_w, h) = editor.active().view.area;
            let menu_rows = u16::from(editor.menu.is_some());
            let edit_top = menu_rows;
            let edit_bottom = h.saturating_sub(1); // status row excluded
            // edge auto-scroll
            if ev.row < edit_top { crate::nav::scroll_up_one(editor); }
            else if ev.row >= edit_bottom { crate::nav::scroll_down_one(editor); }
            let erow = ev.row.clamp(edit_top, edit_bottom.saturating_sub(1)).saturating_sub(menu_rows);
            let head = match crate::nav::offset_at_cell(editor, ev.column, erow) {
                Some(o) => crate::nav::clamp_snap(editor, o),
                None => editor.active().document.buffer.len(),
            };
            if let Some(anchor) = editor.mouse.anchor {
                editor.active_mut().document.selection =
                    wordcartel_core::selection::Selection::range(anchor, head);
                crate::derive::rebuild(editor);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => { editor.mouse.dragging = false; }
        _ => {} // wheel/etc wired in Tasks 5+
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::jobs::InlineExecutor;
    use crate::registry::Registry;
    use crossterm::event::{MouseEvent, MouseEventKind, MouseButton, KeyModifiers};

    // app's TestClock is private to its test module — define a local one here.
    struct TestClock(u64);
    impl wordcartel_core::history::Clock for TestClock {
        fn now_ms(&self) -> u64 {
            self.0
        }
    }

    fn ctx() -> (
        Registry,
        InlineExecutor,
        TestClock,
        std::sync::mpsc::Sender<crate::app::Msg>,
        crate::keymap::KeyTrie,
    ) {
        let reg = Registry::builtins();
        let (km, _) =
            crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let (tx, _rx) = std::sync::mpsc::channel();
        (reg, InlineExecutor::default(), TestClock(0), tx, km)
    }

    fn down(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn click_places_caret_at_cell_offset() {
        let mut e = Editor::new_from_text("abc\ndef\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        // cell (1,1) = 'e' in "def" → offset 5 (no menu, so screen row == editing row)
        handle(&mut e, down(1, 1), &reg, &km, &ex, &clk, &tx);
        assert_eq!(crate::nav::head(&e), 5);
    }

    #[test]
    fn click_below_content_goes_to_doc_end() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, down(0, 10), &reg, &km, &ex, &clk, &tx); // row past content
        assert_eq!(crate::nav::head(&e), e.active().document.buffer.len());
    }

    #[test]
    fn mouse_ignored_during_pending_mark() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.pending_mark = Some(crate::editor::MarkPending::Set);
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, down(1, 0), &reg, &km, &ex, &clk, &tx);
        assert_eq!(crate::nav::head(&e), 0, "click ignored while mark capture pending");
        assert!(e.pending_mark.is_some());
    }

    #[test]
    fn drag_selects_range_from_anchor() {
        let mut e = Editor::new_from_text("abcdef\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, down(1, 0), &reg, &km, &ex, &clk, &tx); // anchor at offset 1
        let drag = MouseEvent { kind: MouseEventKind::Drag(MouseButton::Left), column: 4, row: 0, modifiers: KeyModifiers::NONE };
        handle(&mut e, drag, &reg, &km, &ex, &clk, &tx); // head at offset 4
        let r = e.active().document.selection.primary();
        assert_eq!((r.from(), r.to()), (1, 4));
    }
    #[test]
    fn shift_click_extends_keeping_anchor() {
        let mut e = Editor::new_from_text("abcdef\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        let (reg, ex, clk, tx, km) = ctx();
        let shift_down = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 4, row: 0, modifiers: KeyModifiers::SHIFT };
        handle(&mut e, shift_down, &reg, &km, &ex, &clk, &tx);
        let r = e.active().document.selection.primary();
        assert_eq!((r.from(), r.to()), (1, 4), "extends from existing anchor to click");
    }
}
