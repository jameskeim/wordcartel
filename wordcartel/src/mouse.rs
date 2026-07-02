//! Mouse coordinate translation and gesture dispatch.
use crossterm::event::{MouseEvent, MouseEventKind, MouseButton, KeyModifiers};
use crate::editor::Editor;
use crate::registry::{place_caret_visible, CaretPlace};

/// Classification of a terminal cell hit relative to the editing layout.
#[derive(Clone, Copy)]
pub enum CellHit {
    Text { col: u16, erow: u16 },
    MenuBar,
    Status,
    Scrollbar,
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
/// Push the current selection onto sel_history, then set a range selection [f,t),
/// rebuild, and ensure the caret is visible.  The sel_history push lets a
/// following Ctrl+W (ExpandSelection) grow from the mouse selection.
fn seed_and_select(editor: &mut Editor, f: usize, t: usize) {
    // Clone to a local first — avoids overlapping active()/active_mut() borrow.
    let cur_sel = editor.active().document.selection.clone();
    editor.active_mut().sel_history.push(cur_sel);
    editor.active_mut().document.selection =
        wordcartel_core::selection::Selection::range(f, t);
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}

fn visible_doc_end(editor: &mut Editor) -> usize {
    let len = editor.active().document.buffer.len();
    let probe = if len > 0 { len - 1 } else { 0 };
    let snapped = place_caret_visible(editor, probe, CaretPlace::SnapOut);
    if snapped != probe { snapped } else { len }
}

pub fn handle(
    editor: &mut Editor,
    ev: MouseEvent,
    reg: &crate::registry::Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    if editor.pending_mark.is_some() || !editor.mouse_capture {
        return;
    }
    // Universal drag clear: a button release always ends any drag, regardless of
    // whether an overlay is open. This prevents stale drag state if the user opens
    // the palette/menu via keyboard while a drag is in flight, then releases the
    // button — the Up would otherwise be consumed by the overlay branch without
    // clearing `dragging`/`scrollbar_dragging`. Fall through so normal Up handling
    // (which redundantly clears the same fields) still runs in the non-overlay path.
    if let MouseEventKind::Up(MouseButton::Left) = ev.kind {
        editor.mouse.dragging = false;
        editor.mouse.scrollbar_dragging = false;
    }
    let (w, h) = editor.active().view.area;
    let area = ratatui::layout::Rect::new(0, 0, w, h);
    if editor.palette.is_some() {
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            // scoped borrow → owned Option<CommandId>
            let hit_id: Option<crate::registry::CommandId> = {
                let p = editor.palette.as_ref().unwrap();
                crate::render::palette_row_at(area, p, ev.column, ev.row)
                    .and_then(|idx| p.rows.get(idx).map(|r| r.id))
            };
            // was the click inside the overlay rect at all?
            let inside = {
                let row_count = editor.palette.as_ref().unwrap().rows.len();
                let r = crate::render::palette_overlay_rect(area, row_count);
                ev.column >= r.x && ev.column < r.x + r.width && ev.row >= r.y && ev.row < r.y + r.height
            };
            if let Some(id) = hit_id {
                crate::app::dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id);
            } else if !inside {
                editor.palette = None; // click outside closes
                editor.search = None;
                editor.diag = None;
            }
        }
        return;
    }
    if editor.menu.is_some() {
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            let open = editor.menu.as_ref().unwrap().open;
            // scoped borrows → owned hit results
            let bar_hit: Option<usize> = {
                let groups = &editor.menu.as_ref().unwrap().groups;
                crate::render::menu_bar_layout(area, groups).into_iter()
                    .find(|(_, r)| ev.column >= r.x && ev.column < r.x + r.width && ev.row == r.y)
                    .map(|(cat, _)| cat)
            };
            let row_id: Option<crate::registry::CommandId> = {
                let groups = &editor.menu.as_ref().unwrap().groups;
                crate::render::menu_dropdown_row_at(area, groups, open, ev.column, ev.row)
                    .and_then(|row| groups.get(open).and_then(|g| g.1.get(row)).map(|(_, id)| *id))
            };
            // all borrows dropped — now mutate/dispatch/clear
            if let Some(cat) = bar_hit {
                let m = editor.menu.as_mut().unwrap(); m.open = cat; m.highlighted = 0;
            } else if let Some(id) = row_id {
                crate::app::dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id);
            } else {
                editor.menu = None; // outside → close
                editor.search = None;
                editor.diag = None;
            }
        }
        return;
    }
    if editor.theme_picker.is_some() {
        return;
    }
    if editor.file_browser.is_some() {
        return;
    }
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let hit = editing_cell(editor, ev.column, ev.row);
            if let CellHit::Scrollbar = hit {
                let (_w, h) = editor.active().view.area;
                let menu_rows = u16::from(editor.menu.is_some());
                let edit_height = h.saturating_sub(1 + menu_rows) as usize;
                let erow_in_track = ev.row.saturating_sub(menu_rows) as usize;
                let fv = editor.active_fold_view();
                let vis = fv.visible_count();
                let max_ord = vis.saturating_sub(1);
                let new_ord = (erow_in_track * max_ord).checked_div(edit_height).unwrap_or(0).min(max_ord);
                editor.active_mut().view.scroll = fv.line_at_ordinal(new_ord);
                editor.mouse.scrollbar_dragging = true;
                editor.mouse.scrollbar_until_ms = clock.now_ms() + 1200;
            } else if let CellHit::Text { col, erow } = hit {
                let off = match crate::nav::offset_at_cell(editor, col, erow) {
                    Some(o) => crate::nav::clamp_snap(editor, o),
                    None => visible_doc_end(editor),
                };
                editor.active_mut().sel_history.clear();
                if ev.modifiers.contains(KeyModifiers::SHIFT) {
                    let anchor = editor.active().document.selection.primary().anchor;
                    editor.active_mut().document.selection =
                        wordcartel_core::selection::Selection::range(anchor, off);
                    editor.mouse.anchor = Some(anchor);
                } else {
                    let now = clock.now_ms();
                    let cell = (ev.column, ev.row);
                    let count = match editor.mouse.last_click {
                        Some(ref lc) if now.saturating_sub(lc.at_ms) <= 400 && lc.cell == cell => {
                            (lc.count % 3) + 1
                        }
                        _ => 1,
                    };
                    editor.mouse.last_click = Some(crate::editor::ClickRecord { cell, at_ms: now, count });
                    match count {
                        2 => {
                            let (f, t) = crate::commands::scope_range_at(editor, off, crate::commands::Scope::Word);
                            seed_and_select(editor, f, t);
                        }
                        3 => {
                            let (f, t) = crate::commands::scope_range_at(editor, off, crate::commands::Scope::Paragraph);
                            seed_and_select(editor, f, t);
                        }
                        _ => {
                            editor.active_mut().document.selection =
                                wordcartel_core::selection::Selection::single(off);
                            editor.mouse.anchor = Some(off);
                            crate::derive::rebuild(editor);
                            crate::nav::ensure_visible(editor);
                        }
                    }
                    editor.mouse.anchor = Some(off);
                }
                editor.mouse.dragging = true;
                // rebuild+ensure_visible done per-branch above for non-shift path
                if ev.modifiers.contains(KeyModifiers::SHIFT) {
                    crate::derive::rebuild(editor);
                    crate::nav::ensure_visible(editor);
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if editor.mouse.scrollbar_dragging {
                let (_w, h) = editor.active().view.area;
                let menu_rows = u16::from(editor.menu.is_some());
                let edit_height = h.saturating_sub(1 + menu_rows) as usize;
                let erow_in_track = ev.row.saturating_sub(menu_rows) as usize;
                let fv = editor.active_fold_view();
                let vis = fv.visible_count();
                let max_ord = vis.saturating_sub(1);
                let new_ord = (erow_in_track * max_ord).checked_div(edit_height).unwrap_or(0).min(max_ord);
                editor.active_mut().view.scroll = fv.line_at_ordinal(new_ord);
                editor.mouse.scrollbar_until_ms = clock.now_ms() + 1200;
                return;
            }
            if !editor.mouse.dragging { return; }
            let (_w, h) = editor.active().view.area;
            let menu_rows = u16::from(editor.menu.is_some());
            let edit_top = menu_rows;
            let edit_bottom = h.saturating_sub(1); // status row excluded
            // edge auto-scroll
            if ev.row < edit_top { crate::nav::scroll_up_one(editor); }
            else if ev.row >= edit_bottom { crate::nav::scroll_down_one(editor); }
            let hi = edit_bottom.saturating_sub(1).max(edit_top);
            let erow = ev.row.clamp(edit_top, hi).saturating_sub(menu_rows);
            let head = match crate::nav::offset_at_cell(editor, ev.column, erow) {
                Some(o) => crate::nav::clamp_snap(editor, o),
                None => visible_doc_end(editor),
            };
            if let Some(anchor) = editor.mouse.anchor {
                editor.active_mut().document.selection =
                    wordcartel_core::selection::Selection::range(anchor, head);
                crate::derive::rebuild(editor);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            editor.mouse.dragging = false;
            editor.mouse.scrollbar_dragging = false;
        }
        MouseEventKind::ScrollDown => {
            crate::nav::scroll_down_one(editor);
            editor.mouse.scrollbar_until_ms = clock.now_ms() + 1200;
        }
        MouseEventKind::ScrollUp => {
            crate::nav::scroll_up_one(editor);
            editor.mouse.scrollbar_until_ms = clock.now_ms() + 1200;
        }
        _ => {}
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
    fn mouse_click_below_folded_tail_snaps_to_heading() {
        let doc = "intro\n## Tail\nbody1\nbody2\n";
        let mut e = Editor::new_from_text(doc, None, (80, 24));
        let tail = doc.find("## Tail").unwrap();
        e.active_mut().folds.toggle(tail);
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();

        handle(&mut e, down(0, 10), &reg, &km, &ex, &clk, &tx);

        let head = crate::nav::head(&e);
        let fv = {
            let b = e.active();
            crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer)
        };
        assert_eq!(head, tail);
        assert!(!fv.is_hidden(e.active().document.buffer.byte_to_line(head)));
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

    #[test]
    fn double_click_selects_word_triple_selects_paragraph() {
        let mut e = Editor::new_from_text("alpha beta\n\ngamma\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        // two Downs on the same cell within 400ms (TestClock fixed at 0)
        handle(&mut e, down(7, 0), &reg, &km, &ex, &clk, &tx);
        handle(&mut e, down(7, 0), &reg, &km, &ex, &clk, &tx);
        let r = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(r.from()..r.to()), "beta");
        handle(&mut e, down(7, 0), &reg, &km, &ex, &clk, &tx); // triple → paragraph
        let r2 = e.active().document.selection.primary();
        assert!(e.active().document.buffer.slice(r2.from()..r2.to()).starts_with("alpha beta"));
        assert!(!e.active().sel_history.is_empty(), "multi-click seeds the expand ladder");
    }

    #[test]
    fn wheel_scrolls_view_not_caret() {
        let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 10));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        let before = crate::nav::head(&e);
        let wheel = MouseEvent { kind: MouseEventKind::ScrollDown, column: 0, row: 0, modifiers: KeyModifiers::NONE };
        handle(&mut e, wheel, &reg, &km, &ex, &clk, &tx);
        assert!(e.active().view.scroll > 0, "view scrolled");
        assert_eq!(crate::nav::head(&e), before, "caret unchanged");
    }

    #[test]
    fn click_palette_row_dispatches_and_closes() {
        // "copy" is registered at index ~27, beyond the 15-row visible cap.
        // We use "move_right" (index 1, always within the first 15 visible rows) to
        // exercise the same dispatch path: click the row → palette closes + command runs.
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.palette = Some(crate::palette::Palette::default());
        let (reg, ex, clk, tx, km) = ctx();
        crate::app::hydrate_overlays(&mut e, &reg, &km); // fill rows (5b helper)
        let rows = &e.palette.as_ref().unwrap().rows;
        let idx = rows.iter().position(|r| r.id == crate::registry::CommandId("move_right")).unwrap();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::render::palette_overlay_rect(area, e.palette.as_ref().unwrap().rows.len());
        let click_row = rect.y + 2 + idx as u16; // list starts at ov_y+2
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: click_row, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_none(), "palette closed after click");
        // move_right from offset 0 → caret at 1 proves the command was dispatched
        assert_eq!(crate::nav::head(&e), 1, "clicked move_right dispatched");
    }

    #[test]
    fn click_outside_palette_closes_it() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.palette = Some(crate::palette::Palette::default());
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, down(0, 0), &reg, &km, &ex, &clk, &tx); // top-left, outside the centered overlay
        assert!(e.palette.is_none());
    }

    #[test]
    fn scrollbar_drag_scrubs_view() {
        let text: String = (0..100).map(|i| format!("l{i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 12));
        e.mouse.scrollbar_visible = true;
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        // Down on the scrollbar column (w-1 = 79), mid-track row
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 79, row: 6, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.mouse.scrollbar_dragging);
        assert!(e.active().view.scroll > 0, "scrubbed to a lower position");
    }

    #[test]
    fn click_with_theme_picker_open_does_not_move_caret() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_theme_picker();
        assert!(e.theme_picker.is_some());
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, down(1, 0), &reg, &km, &ex, &clk, &tx);
        assert_eq!(crate::nav::head(&e), 0, "click absorbed by theme picker — caret must not move");
        assert!(e.theme_picker.is_some(), "theme picker must remain open after click");
    }

    /// Finding 2 regression: if the palette (or menu) is open while a text or
    /// scrollbar drag is in flight and the user releases the mouse button, the
    /// Up(Left) event is consumed by the overlay branch — but drag state must
    /// still be cleared so later events aren't misrouted.
    #[test]
    fn overlay_open_mid_drag_up_clears_drag_state() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        // Simulate a drag in flight.
        e.mouse.dragging = true;
        e.mouse.scrollbar_dragging = true;
        // Open the palette (keyboard user opened it while drag was in flight).
        e.palette = Some(crate::palette::Palette::default());
        let (reg, ex, clk, tx, km) = ctx();
        // Send the Up(Left) — the overlay branch would normally `return` here without
        // clearing drag state; after the fix it must clear it first.
        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        handle(&mut e, up, &reg, &km, &ex, &clk, &tx);
        assert!(!e.mouse.dragging, "dragging must be cleared when Up(Left) arrives during overlay");
        assert!(!e.mouse.scrollbar_dragging, "scrollbar_dragging must be cleared when Up(Left) arrives during overlay");
    }
}
