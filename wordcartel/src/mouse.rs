//! Mouse coordinate translation and gesture dispatch.
use crossterm::event::{MouseEvent, MouseEventKind, MouseButton, KeyModifiers};
use crate::editor::Editor;
use crate::registry::{place_caret_visible, CaretPlace};

/// Pointer must REST on row 0 this long before the auto-mode bar reveals.
pub(crate) const MENU_DWELL_MS: u64 = 250;
/// A revealed bar survives leaving row 0 this long (aim-wobble forgiveness).
pub(crate) const MENU_LEAVE_GRACE_MS: u64 = 400;

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
    let menu_rows: u16 = editor.menu_bar_rows();
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
    // A1 auto-mode dwell tracking. Runs on every motion frame — keep it trivial
    // (integer compares + stores only; the reveal/hide fire later in advance()).
    // The two timers are deliberately ASYMMETRIC: the dwell re-arms on every
    // row-0 motion (reveal after REST), the grace arms ONCE on the first leave.
    if editor.menu_bar_mode == crate::config::MenuBarMode::Auto {
        if let MouseEventKind::Moved = ev.kind {
            if ev.row > 0 {
                editor.mouse.menu_reveal_due = None;
                if editor.mouse.menu_bar_revealed && editor.mouse.menu_hide_due.is_none() {
                    editor.mouse.menu_hide_due = Some(clock.now_ms() + MENU_LEAVE_GRACE_MS);
                }
            } else {
                editor.mouse.menu_hide_due = None; // re-entry cancels a pending hide
                if editor.menu.is_none()
                    && editor.palette.is_none()
                    && editor.theme_picker.is_none()
                    && editor.file_browser.is_none()
                    && !editor.mouse.dragging
                    && !editor.mouse.scrollbar_dragging
                    && !editor.mouse.menu_bar_revealed
                {
                    editor.mouse.menu_reveal_due = Some(clock.now_ms() + MENU_DWELL_MS);
                }
            }
        }
    }
    let (w, h) = editor.active().view.area;
    let area = ratatui::layout::Rect::new(0, 0, w, h);
    if editor.palette.is_some() {
        // `if matches!` — a `match` with a lone arm + `_ => {}` trips
        // clippy::single_match under the deny gate (Codex plan r1).
        if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
            let ah = editor.active().view.area.1;
            if let Some(p) = editor.palette.as_mut() {
                if matches!(ev.kind, MouseEventKind::ScrollDown) {
                    p.selected = (p.selected + 1).min(p.rows.len().saturating_sub(1));
                } else {
                    p.selected = p.selected.saturating_sub(1);
                }
                crate::app::keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
            }
            return;
        }
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            // scoped borrow → owned hit values (id + optional buffer) — mirrors
            // the keyboard Enter arm (app.rs) which checks row.buffer first so
            // that Buffers-kind rows switch buffers rather than dispatching their
            // sentinel CommandId("palette").
            let hit: Option<(crate::registry::CommandId, Option<crate::editor::BufferId>)> = {
                let p = editor.palette.as_ref().unwrap();
                crate::render::palette_row_at(area, p, ev.column, ev.row)
                    .and_then(|idx| p.rows.get(idx).map(|r| (r.id, r.buffer)))
            };
            // was the click inside the overlay rect at all?
            let inside = {
                let row_count = editor.palette.as_ref().unwrap().rows.len();
                let r = crate::render::palette_overlay_rect(area, row_count);
                ev.column >= r.x && ev.column < r.x + r.width && ev.row >= r.y && ev.row < r.y + r.height
            };
            if let Some((id, buffer)) = hit {
                if let Some(bid) = buffer {
                    // Buffer-switcher row: dismiss palette, jump to buffer — same
                    // path as the keyboard Enter arm. Pre-existing bug: the old
                    // code dispatched CommandId("palette") for every buffer row,
                    // reopening the picker instead of switching.
                    editor.palette = None;
                    if let Some(idx) = editor.buffers.iter().position(|b| b.id == bid) {
                        crate::workspace::switch_to(editor, idx);
                    }
                } else {
                    crate::app::dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id);
                }
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
        if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
            let ah = editor.active().view.area.1;
            if let Some(tp) = editor.theme_picker.as_mut() {
                if matches!(ev.kind, MouseEventKind::ScrollDown) {
                    tp.selected = (tp.selected + 1).min(tp.rows.len().saturating_sub(1));
                } else {
                    tp.selected = tp.selected.saturating_sub(1);
                }
                crate::app::keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
            }
            crate::app::preview_selected_theme(editor);
        }
        return;
    }
    if editor.file_browser.is_some() {
        if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
            let ah = editor.active().view.area.1;
            if let Some(fb) = editor.file_browser.as_mut() {
                if matches!(ev.kind, MouseEventKind::ScrollDown) {
                    fb.selected = (fb.selected + 1).min(fb.entries.len().saturating_sub(1));
                } else {
                    fb.selected = fb.selected.saturating_sub(1);
                }
                crate::app::keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
            }
        }
        return;
    }
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let hit = editing_cell(editor, ev.column, ev.row);
            if let CellHit::MenuBar = hit {
                // Inactive bar: open the dropdown AT the clicked category (hydrated
                // by reduce's post-handle hydrate_overlays call).
                let cats_hit = crate::render::menu_bar_layout_cats(area, &crate::registry::MENU_ORDER)
                    .into_iter()
                    .find(|(_, r)| ev.column >= r.x && ev.column < r.x + r.width && ev.row == r.y)
                    .map(|(i, _)| i);
                if let Some(order_idx) = cats_hit {
                    editor.menu = Some(crate::menu::empty_at(order_idx));
                }
                // A row-0 click OFF the labels does nothing (the fill area is inert).
            } else if let CellHit::Scrollbar = hit {
                let (_w, h) = editor.active().view.area;
                let menu_rows = editor.menu_bar_rows();
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
                let menu_rows = editor.menu_bar_rows();
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
            let menu_rows = editor.menu_bar_rows();
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
            crate::fold::FoldView::compute(&b.folds, b.document.blocks(), &b.document.buffer)
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
        // A6 precondition: scroll_top must be 0 for this test's geometry to hold.
        assert_eq!(e.palette.as_ref().unwrap().scroll_top, 0);
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

    /// A6: clicking the first visible row when scroll_top > 0 must dispatch the
    /// row at `rows[scroll_top]`, not `rows[0]`. The contract: the absolute row
    /// index returned by `palette_row_at` accounts for `scroll_top`.
    ///
    /// rows[0] = move_left, rows[6] = select_left (registration order). With
    /// selected=20, list_h=15 on an 80×24 terminal, keep_overlay_visible sets
    /// scroll_top=6. From caret 1, select_left yields a non-empty selection
    /// ([0,1]); move_left would leave an empty selection (caret at 0). This
    /// distinguishes which row was actually dispatched.
    #[test]
    fn scrolled_click_maps_to_absolute_row() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        // Seed caret at 1 so select_left (rows[6]) is distinguishable from move_left (rows[0]).
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        let (reg, ex, clk, tx, km) = ctx();
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &km);
        p.selected = 20;
        crate::app::keep_overlay_visible(24, p.selected, p.rows.len(), &mut p.scroll_top);
        let scroll_top = p.scroll_top;
        assert!(scroll_top > 0, "scroll_top must be non-zero for this test to be meaningful");
        e.palette = Some(p);
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::render::palette_overlay_rect(area, e.palette.as_ref().unwrap().rows.len());
        // Click the FIRST visible list row (visual row 0, absolute row scroll_top).
        let click_row = rect.y + 2; // ov_y + 2 = first list entry
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: click_row, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_none(), "palette closed after click");
        // select_left from caret 1 → non-empty selection [0,1].
        // move_left from caret 1 → caret 0, selection empty.
        // Non-empty selection proves rows[scroll_top] was dispatched, not rows[0].
        assert!(!e.active().document.selection.primary().is_empty(),
            "dispatched rows[scroll_top] (select_left), not rows[0] (move_left)");
    }

    /// A6: 20 ScrollDown wheel events move selected to 20 and scroll the window
    /// so the selection stays visible.
    #[test]
    fn wheel_moves_selection_and_window() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &km);
        e.palette = Some(p);
        // 20 scroll-downs.
        let scroll_down = MouseEvent { kind: MouseEventKind::ScrollDown, column: 40, row: 12, modifiers: KeyModifiers::NONE };
        for _ in 0..20 {
            handle(&mut e, scroll_down, &reg, &km, &ex, &clk, &tx);
        }
        let p = e.palette.as_ref().expect("palette still open after wheel");
        assert_eq!(p.selected, 20, "selected moved to 20");
        let lh = crate::list_window::list_h_for(p.rows.len(), 24);
        assert!(p.selected.saturating_sub(p.scroll_top) < lh,
            "selection is within the visible window (selected - scroll_top < list_h)");
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

    // -----------------------------------------------------------------------
    // A1 Task 2: CellHit::MenuBar + click-to-open
    // -----------------------------------------------------------------------

    /// row 0 in Pinned mode (menu None) → MenuBar; in Hidden mode → NOT MenuBar.
    #[test]
    fn editing_cell_row0_is_menubar_only_when_bar_visible() {
        use crate::config::MenuBarMode;
        let mut e = Editor::new_from_text("hello\n", None, (40, 8));

        // Pinned: menu_bar_rows() == 1 → row 0 is MenuBar.
        e.menu_bar_mode = MenuBarMode::Pinned;
        e.menu = None;
        assert!(matches!(editing_cell(&e, 0, 0), CellHit::MenuBar), "Pinned + menu None → MenuBar");

        // Hidden: menu_bar_rows() == 0 → row 0 is NOT MenuBar.
        e.menu_bar_mode = MenuBarMode::Hidden;
        e.menu = None;
        assert!(!matches!(editing_cell(&e, 0, 0), CellHit::MenuBar), "Hidden + menu None → not MenuBar");
    }

    /// A click on the inactive bar (Pinned, menu None) at the Format label column
    /// opens a placeholder with open == MENU_ORDER index of Format (== 2).
    #[test]
    fn click_on_inactive_bar_opens_that_category() {
        use crate::config::MenuBarMode;
        let mut e = Editor::new_from_text("hello\n", None, (80, 8));
        crate::derive::rebuild(&mut e);
        e.menu_bar_mode = MenuBarMode::Pinned;
        e.menu = None;
        let (reg, ex, clk, tx, km) = ctx();
        // Compute the Format label column dynamically (MENU_ORDER[2] = Format).
        let (w, h) = e.active().view.area;
        let area = ratatui::layout::Rect::new(0, 0, w, h);
        let menu_area = ratatui::layout::Rect::new(area.x, area.y, w, h.saturating_sub(1));
        let bar = crate::render::menu_bar_layout_cats(menu_area, &crate::registry::MENU_ORDER);
        let (_, format_rect) = bar.iter().find(|(i, _)| *i == 2).expect("Format at index 2");
        let col = format_rect.x + 1; // somewhere inside the label

        // Click on the Format label while the bar is inactive (menu None).
        handle(&mut e, down(col, 0), &reg, &km, &ex, &clk, &tx);
        let menu = e.menu.as_ref().expect("click must set editor.menu to Some placeholder");
        assert!(!menu.built, "placeholder must not be built (hydration happens in reduce)");
        assert_eq!(menu.open, 2, "placeholder open must be the MENU_ORDER index of Format");

        // After hydrate_overlays: built and mapped to the correct group.
        crate::app::hydrate_overlays(&mut e, &reg, &km);
        let menu = e.menu.as_ref().unwrap();
        assert!(menu.built, "hydrated menu must be built");
        let format_pos = menu.groups.iter().position(|(cat, _)| *cat == crate::registry::MenuCategory::Format)
            .expect("Format group must exist after build");
        assert_eq!(menu.open, format_pos, "hydrated open must map to Format's group position");
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

    // -----------------------------------------------------------------------
    // A1 Task 3: auto-mode dwell/grace predicate table
    // -----------------------------------------------------------------------

    fn moved(col: u16, row: u16) -> MouseEvent {
        MouseEvent { kind: MouseEventKind::Moved, column: col, row, modifiers: KeyModifiers::NONE }
    }

    /// Case 1: a Moved onto row 0 arms the reveal deadline at now + DWELL.
    #[test]
    fn dwell_arms_on_row0_rest() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 8));
        crate::derive::rebuild(&mut e);
        let (reg, ex, _, tx, km) = ctx();
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        handle(&mut e, moved(5, 0), &reg, &km, &ex, &TestClock(0), &tx);
        assert_eq!(e.mouse.menu_reveal_due, Some(MENU_DWELL_MS),
            "Moved onto row 0 must arm reveal at now + DWELL_MS");
    }

    /// Case 2 (asymmetry side 1): each row-0 motion re-arms; the deadline tracks
    /// the LAST motion — reveal fires only after the pointer RESTS.
    #[test]
    fn dwell_rearm_tracks_last_motion() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 8));
        crate::derive::rebuild(&mut e);
        let (reg, ex, _, tx, km) = ctx();
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        handle(&mut e, moved(5, 0), &reg, &km, &ex, &TestClock(0), &tx);
        handle(&mut e, moved(6, 0), &reg, &km, &ex, &TestClock(100), &tx);
        assert_eq!(e.mouse.menu_reveal_due, Some(100 + MENU_DWELL_MS),
            "second row-0 motion must re-arm the deadline to the LAST motion time");
    }

    /// Case 3: EACH gate condition alone blocks the dwell arm.
    #[test]
    fn dwell_never_arms_during_drag_or_overlay() {
        // Helper: fresh Auto editor at row 0, Moved dispatched at t=0.
        // Returns the menu_reveal_due after the move.
        let fire = |setup: &dyn Fn(&mut Editor)| -> Option<u64> {
            let mut e = Editor::new_from_text("hello\n", None, (40, 8));
            crate::derive::rebuild(&mut e);
            e.menu_bar_mode = crate::config::MenuBarMode::Auto;
            setup(&mut e);
            let (reg, ex, _, tx, km) = ctx();
            handle(&mut e, moved(5, 0), &reg, &km, &ex, &TestClock(0), &tx);
            e.mouse.menu_reveal_due
        };
        assert!(fire(&|e| { e.mouse.dragging = true; }).is_none(),
            "dragging=true must block arming");
        assert!(fire(&|e| { e.mouse.scrollbar_dragging = true; }).is_none(),
            "scrollbar_dragging=true must block arming");
        assert!(fire(&|e| { e.palette = Some(crate::palette::Palette::default()); }).is_none(),
            "palette open must block arming");
        assert!(fire(&|e| { e.open_theme_picker(); }).is_none(),
            "theme_picker open must block arming");
        assert!(fire(&|e| { e.file_browser = Some(crate::file_browser::FileBrowser {
            dir: std::path::PathBuf::from("."), query: String::new(),
            entries: vec![], selected: 0, scroll_top: 0,
        }); }).is_none(), "file_browser open must block arming");
        assert!(fire(&|e| { e.menu = Some(crate::menu::empty_at(0)); }).is_none(),
            "dropdown open must block arming");
        assert!(fire(&|e| { e.menu_bar_mode = crate::config::MenuBarMode::Pinned; }).is_none(),
            "mode=Pinned must block arming (arm-side mode gate)");
    }

    /// Case 4 (asymmetry side 2): the grace arms ONCE on the first leave; a
    /// second leave motion must NOT re-arm (that would defer the hide forever).
    #[test]
    fn leave_arms_grace_once() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 8));
        crate::derive::rebuild(&mut e);
        let (reg, ex, _, tx, km) = ctx();
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        e.mouse.menu_bar_revealed = true;
        handle(&mut e, moved(5, 5), &reg, &km, &ex, &TestClock(0), &tx);
        assert_eq!(e.mouse.menu_hide_due, Some(MENU_LEAVE_GRACE_MS),
            "first leave motion must arm the grace deadline");
        handle(&mut e, moved(6, 5), &reg, &km, &ex, &TestClock(100), &tx);
        assert_eq!(e.mouse.menu_hide_due, Some(MENU_LEAVE_GRACE_MS),
            "second leave motion must NOT re-arm — grace must stay at the FIRST leave time");
    }

    /// Case 5: re-entering row 0 cancels a pending hide; the bar stays revealed.
    #[test]
    fn reentry_cancels_grace() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 8));
        crate::derive::rebuild(&mut e);
        let (reg, ex, _, tx, km) = ctx();
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        e.mouse.menu_bar_revealed = true;
        handle(&mut e, moved(5, 5), &reg, &km, &ex, &TestClock(0), &tx);
        assert!(e.mouse.menu_hide_due.is_some(), "precondition: grace must be armed");
        handle(&mut e, moved(5, 0), &reg, &km, &ex, &TestClock(100), &tx);
        assert!(e.mouse.menu_hide_due.is_none(), "re-entry must cancel the pending hide");
        assert!(e.mouse.menu_bar_revealed, "bar must still be revealed after re-entry");
    }

    /// Case 6: leave-bookkeeping runs even while the dropdown is open (the arm
    /// sits before the overlay return — spec I1).
    #[test]
    fn leave_bookkeeping_runs_while_dropdown_open() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 8));
        crate::derive::rebuild(&mut e);
        let (reg, ex, _, tx, km) = ctx();
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        e.mouse.menu_bar_revealed = true;
        e.menu = Some(crate::menu::empty_at(0)); // dropdown open
        handle(&mut e, moved(5, 5), &reg, &km, &ex, &TestClock(0), &tx);
        assert_eq!(e.mouse.menu_hide_due, Some(MENU_LEAVE_GRACE_MS),
            "leave-bookkeeping must run even with the dropdown open");
    }

    /// Case 9: a wheel event (ScrollUp) at row 0 must never arm the dwell —
    /// the arm is gated on Moved only (pins a refactor that could loosen the kind gate).
    #[test]
    fn wheel_never_arms() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 8));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        let wheel_up = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 5,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        handle(&mut e, wheel_up, &reg, &km, &ex, &clk, &tx);
        assert!(e.mouse.menu_reveal_due.is_none(),
            "ScrollUp at row 0 must not arm the dwell (Moved-kind gate)");
    }

    // -----------------------------------------------------------------------
    // A6 Task 2: tp/fb mouse wheel
    // -----------------------------------------------------------------------

    /// A6: ScrollDown wheel on the theme picker moves selected, keeps the window
    /// visible, and previews the correct row (ordering pin).
    ///
    /// TDD RED: without the wheel block (just `return`), selected stays 0 and
    /// the theme is not previewed.
    #[test]
    fn tp_wheel_scroll_moves_selection_and_previews() {
        let mut e = Editor::new_from_text("# Hello\n\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_theme_picker();
        // There are 19 builtins — pad to 20 by cycling real builtin names so
        // the list exceeds the 15-row window cap. Navigation-only (no Char/Backspace)
        // so rebuild_rows is never called and the padding stays in place.
        {
            let names = wordcartel_core::theme::Theme::builtin_names();
            let tp = e.theme_picker.as_mut().unwrap();
            tp.rows.clear();
            for i in 0..20 { tp.rows.push(names[i % names.len()].to_string()); }
        }
        assert_eq!(e.theme_picker.as_ref().unwrap().rows.len(), 20);
        let lh = crate::list_window::list_h_for(20, 24);
        assert_eq!(lh, 15, "list_h = 15 for 20 rows on 24-row terminal");
        let (reg, ex, clk, tx, km) = ctx();
        let scroll_down = MouseEvent {
            kind: MouseEventKind::ScrollDown, column: 40, row: 10,
            modifiers: KeyModifiers::NONE,
        };
        // 16 scroll-downs — pushes past the 15-row window.
        for _ in 0..16 {
            handle(&mut e, scroll_down, &reg, &km, &ex, &clk, &tx);
        }
        let tp = e.theme_picker.as_ref().expect("picker must remain open");
        assert_eq!(tp.selected, 16, "selected must be 16 after 16 scroll-downs");
        assert!(tp.selected.saturating_sub(tp.scroll_top) < lh,
            "tp wheel: selection visible (selected={}, scroll_top={}, lh={})",
            tp.selected, tp.scroll_top, lh);
        // The applied theme must equal tp.rows[tp.selected] (wheel previews correct row).
        let expected_name = tp.rows[tp.selected].clone();
        assert_eq!(e.theme.name, expected_name,
            "tp wheel: applied theme={:?} must equal tp.rows[selected]={expected_name:?}",
            e.theme.name);
    }

    /// A6: ScrollDown wheel on the file browser moves selected and keeps the window
    /// visible. The unconditional `return` still prevents text-area events.
    ///
    /// TDD RED: without the wheel block (just `return`), selected stays 0.
    #[test]
    fn fb_wheel_scroll_moves_selection() {
        // 20 directories → 21 entries (.., d00..d19).
        let dir = std::env::temp_dir().join(format!("wc-a6-fbwheel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..20usize {
            std::fs::create_dir(dir.join(format!("d{i:02}"))).unwrap();
        }
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        e.open_file_browser(dir.clone());
        let total = e.file_browser.as_ref().unwrap().entries.len();
        assert_eq!(total, 21, "precondition: 21 entries");
        let (reg, ex, clk, tx, km) = ctx();
        let scroll_down = MouseEvent {
            kind: MouseEventKind::ScrollDown, column: 40, row: 10,
            modifiers: KeyModifiers::NONE,
        };
        for _ in 0..20 {
            handle(&mut e, scroll_down, &reg, &km, &ex, &clk, &tx);
        }
        let fb = e.file_browser.as_ref().expect("browser must remain open");
        assert_eq!(fb.selected, 20, "selected must be 20 after 20 scroll-downs");
        let lh = crate::list_window::list_h_for(fb.entries.len(), 24);
        assert!(fb.selected.saturating_sub(fb.scroll_top) < lh,
            "fb wheel: selection visible (selected={}, scroll_top={}, lh={})",
            fb.selected, fb.scroll_top, lh);
        // Verify the file browser is still open (unconditional return preserved).
        assert!(e.file_browser.is_some(), "file browser must still be open after wheel");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pre-existing bug (aligned here): clicking a Buffers-kind palette row must
    /// switch to that buffer and close the palette — not dispatch
    /// CommandId("palette") which reopens the picker. The mouse path is aligned
    /// with the keyboard Enter arm (app.rs ~1238) that checks row.buffer first.
    ///
    /// Unscrolled click (2 buffers — doc + scratch): the list fits in the window
    /// without scrolling. A scrolled variant is not needed here — the abs-row
    /// mapping for scrolled clicks is already covered by
    /// `scrolled_click_maps_to_absolute_row`; the bug is in the dispatch branch,
    /// not in the hit-test, so an unscrolled click exercises the full fix path.
    #[test]
    fn click_buffers_palette_row_switches_buffer_not_reopens() {
        let mut e = Editor::new_from_text(
            "doc\n", Some(std::path::PathBuf::from("/tmp/a.md")), (80, 24));
        e.install_scratch();
        // buffers[0] = doc (active), buffers[1] = scratch.
        let scratch_id = e.scratch_id.unwrap();
        assert_eq!(e.active, 0, "precondition: doc is active before the click");
        e.open_buffer_switcher();
        // rows[0] = doc (MRU front), rows[1] = scratch — both carry buffer: Some(id).
        assert_eq!(e.palette.as_ref().unwrap().rows.len(), 2,
            "precondition: exactly 2 rows in the Buffers palette");
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::render::palette_overlay_rect(area, 2);
        // Click the second list row (rows[1] = scratch) at ov_y + 2 + 1.
        let click_row = rect.y + 2 + 1;
        let d = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: click_row,
            modifiers: KeyModifiers::NONE,
        };
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_none(), "palette must close after clicking a buffer row");
        assert_eq!(e.active().id, scratch_id,
            "click must switch to the clicked row's buffer (scratch), not reopen the palette");
    }
}
