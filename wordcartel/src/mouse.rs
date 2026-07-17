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
/// Set a range selection [f,t), rebuild, and ensure the caret is visible.
///
/// Ctrl+W (ExpandSelection) is stateless (S4 T3): it re-derives from the CURRENT selection
/// rather than a pushed history, so a following expand grows from whatever this call leaves
/// selected — no explicit seeding needed. A mouse/hand-made selection is therefore no longer a
/// shrink TARGET (stateless shrink lands on canonical LADDER rungs, not exact prior ranges) —
/// F4's already-accepted cost, not a regression.
fn seed_and_select(editor: &mut Editor, f: usize, t: usize) {
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

/// True when NO overlay/modal is open — the shared predicate for dwell suppression.
/// Derived from the overlay table (H21); now counts `splash` too (Q4), so a mouse move
/// under the splash routes to the overlay path instead of arming dwell timers.
fn no_overlay_open(editor: &Editor) -> bool {
    !crate::overlays::any_active(editor)
}

/// Route a mouse event to the active overlay's mouse slot. PRECONDITION: an overlay is open
/// (`!no_overlay_open`). Consumes the event (the caller returns unconditionally after this).
fn route_overlay(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
                 ctx: &crate::overlays::DispatchCtx) {
    if let Some(id) = crate::overlays::OverlayId::ALL.iter()
        .find(|id| (id.row().is_active)(editor))
    {
        (id.row().mouse)(editor, ev, area, ctx);
    }
}

/// Palette mouse slot: wheel moves + windows the selection; `Down(Left)` dispatches the hit
/// row (buffer rows switch buffers) or, on a click outside the rect, closes the palette (and
/// its content-linked search/diag siblings via `close_all`).
pub(crate) fn mouse_palette(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    ctx: &crate::overlays::DispatchCtx) {
    // Hover: move the highlight to the row under the pointer (dedupe: only on row change; I2:
    // off-rect leaves it as-is because the hit-tester returns None).
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.palette.as_ref()
            .and_then(|p| crate::chrome_geom::palette_row_at(area, p, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            if let Some(p) = editor.palette.as_mut() {
                if p.selected != idx {
                    p.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, p.rows.len(), &mut p.scroll_top);
                }
            }
        }
        return;
    }
    // Wheel: the viewport scrolls every notch (wheel_list moves scroll_top); the SELECTION-derived
    // side effect (window-follows-selection) fires ONLY when the row actually changes (I5 dedupe).
    // Empty list is a total no-op (I3b).
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.palette.as_ref() { Some(p) => p.selected, None => return };
        let scrolled = if let Some(p) = editor.palette.as_mut() {
            let n = p.rows.len();
            if n == 0 { return; } // I3b: empty list is a total no-op
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut p.selected, &mut p.scroll_top)
        } else { return };
        if scrolled {
            // Re-hover: the pointer is stationary, so pin the highlight to its row (ruling 1a).
            if let Some(idx) = editor.palette.as_ref()
                .and_then(|p| crate::chrome_geom::palette_row_at(area, p, ev.column, ev.row))
            {
                if let Some(p) = editor.palette.as_mut() { p.selected = idx; }
            }
        }
        // I5 dedupe: re-window from the selection ONLY when the row moved. Skips the redundant
        // re-derive at a clamp boundary that would re-compute scroll_top FROM selection and fight
        // the wheel. In the scroll path `after` is already in-window, so keep_overlay_visible is a
        // no-op on scroll_top; in the short-step path it pins scroll_top for the fully-visible list.
        let after = editor.palette.as_ref().map(|p| p.selected).unwrap_or(before);
        if after != before {
            let n = editor.palette.as_ref().map(|p| p.rows.len()).unwrap_or(0);
            if let Some(p) = editor.palette.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut p.scroll_top);
            }
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
            crate::chrome_geom::palette_row_at(area, p, ev.column, ev.row)
                .and_then(|idx| p.rows.get(idx).map(|r| (r.id, r.buffer)))
        };
        // was the click inside the overlay rect at all?
        let inside = {
            let row_count = editor.palette.as_ref().unwrap().rows.len();
            let r = crate::chrome_geom::palette_overlay_rect(area, row_count);
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
                crate::app::dispatch_overlay_command(editor, ctx.reg, ctx.keymap, ctx.ex, ctx.clock, ctx.msg_tx, id);
            }
        } else if !inside {
            crate::overlays::close_all(editor); // click outside closes
        }
    }
}

/// Menu mouse slot: hover switches the live category and moves the dropdown highlight; wheel
/// moves and windows the dropdown selection against the overflow-adjusted EFFECTIVE item budget;
/// `Down(Left)` on a bar label switches category, on a dropdown action row dispatches it, and on
/// any other cell (including non-action cells INSIDE the dropdown, e.g. the overflow indicator)
/// closes the menu.
pub(crate) fn mouse_menu(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    ctx: &crate::overlays::DispatchCtx) {
    // Hover: (1) onto a DIFFERENT bar category → switch the open dropdown live (reset triple,
    // dedupe on cat != open); (2) onto a dropdown row → set highlighted. Off both → no-op (I2).
    if let MouseEventKind::Moved = ev.kind {
        let hit_area = crate::chrome_geom::menu_area(area);
        let bar_hit: Option<usize> = {
            let groups = &editor.menu.as_ref().unwrap().groups;
            crate::chrome_geom::menu_bar_layout(hit_area, groups).into_iter()
                .find(|(_, r)| ev.column >= r.x && ev.column < r.x + r.width && ev.row == r.y)
                .map(|(cat, _)| cat)
        };
        if let Some(cat) = bar_hit {
            let m = editor.menu.as_mut().unwrap();
            if cat != m.open {
                // Reset triple — identical to menu::intercept's ←/→ arms and the Down bar arm.
                m.open = cat; m.highlighted = 0; m.scroll_top = 0;
            }
            return;
        }
        let (open, scroll_top) = { let m = editor.menu.as_ref().unwrap(); (m.open, m.scroll_top) };
        let row_hit: Option<usize> = {
            let groups = &editor.menu.as_ref().unwrap().groups;
            crate::chrome_geom::menu_dropdown_row_at(hit_area, groups, open, scroll_top, ev.column, ev.row)
        };
        if let Some(idx) = row_hit {
            // menu_dropdown_row_at only returns in-window rows, so no keep_visible needed.
            let m = editor.menu.as_mut().unwrap();
            if m.highlighted != idx { m.highlighted = idx; }
        }
        return;
    }
    // Wheel: viewport scrolls every notch over the dropdown, windowed by the EFFECTIVE item budget
    // (overflow-adjusted, mirroring paint_menu_dropdown + menu_dropdown_row_at); the SELECTION-
    // derived re-window fires ONLY on row-change (I5). Empty → total no-op (I3b).
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let hit_area = crate::chrome_geom::menu_area(area);
        let before = match editor.menu.as_ref() { Some(m) => m.highlighted, None => return };
        // Effective item budget: raw dropdown window minus the reserved indicator row on overflow —
        // identical to paint_menu_dropdown's keep_h + menu_dropdown_row_at's item_rows.
        let (scrolled, n, list_h) = if let Some(m) = editor.menu.as_mut() {
            let n = m.groups.get(m.open).map(|g| g.1.len()).unwrap_or(0);
            if n == 0 { return; }
            let raw_window = n.min(15).min(hit_area.height.saturating_sub(1) as usize);
            let list_h = if n > raw_window { raw_window.saturating_sub(1) } else { raw_window };
            let s = crate::list_window::wheel_list(down, n, list_h, &mut m.highlighted, &mut m.scroll_top);
            (s, n, list_h)
        } else { return };
        if scrolled {
            let (open, scroll_top) = { let m = editor.menu.as_ref().unwrap(); (m.open, m.scroll_top) };
            let row_hit = {
                let groups = &editor.menu.as_ref().unwrap().groups;
                crate::chrome_geom::menu_dropdown_row_at(hit_area, groups, open, scroll_top, ev.column, ev.row)
            };
            if let Some(idx) = row_hit { editor.menu.as_mut().unwrap().highlighted = idx; }
        }
        // I5 dedupe: window-follows-selection only when the row moved (uses the SAME effective
        // budget, so scroll_top never disagrees with the painter or lands on the indicator row).
        let after = editor.menu.as_ref().map(|m| m.highlighted).unwrap_or(before);
        if after != before {
            if let Some(m) = editor.menu.as_mut() {
                crate::list_window::keep_visible(after, n, list_h, &mut m.scroll_top);
            }
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        let open = editor.menu.as_ref().unwrap().open;
        let scroll_top = editor.menu.as_ref().unwrap().scroll_top;
        // Both bar and dropdown hit-tests must use menu_area (frame minus the status row)
        // so the dropdown windowing (avail_below) is identical to the painter's — no drift.
        let hit_area = crate::chrome_geom::menu_area(area);
        // scoped borrows → owned hit results
        let bar_hit: Option<usize> = {
            let groups = &editor.menu.as_ref().unwrap().groups;
            crate::chrome_geom::menu_bar_layout(hit_area, groups).into_iter()
                .find(|(_, r)| ev.column >= r.x && ev.column < r.x + r.width && ev.row == r.y)
                .map(|(cat, _)| cat)
        };
        let row_action: Option<crate::menu::MenuRowAction> = {
            let groups = &editor.menu.as_ref().unwrap().groups;
            crate::chrome_geom::menu_dropdown_row_at(hit_area, groups, open, scroll_top, ev.column, ev.row)
                .and_then(|row| groups.get(open).and_then(|g| g.1.get(row)).map(|(_, action)| *action))
        };
        // all borrows dropped — now mutate/dispatch/clear
        if let Some(cat) = bar_hit {
            // category switch — reset scroll_top so stale window never carries into shorter category
            let m = editor.menu.as_mut().unwrap();
            m.open = cat; m.highlighted = 0; m.scroll_top = 0;
        } else if let Some(action) = row_action {
            crate::menu::dispatch_row_action(editor, ctx.reg, ctx.keymap, ctx.ex, ctx.clock, ctx.msg_tx, action);
        } else {
            crate::overlays::close_all(editor); // outside (and non-action cells) → close
        }
    }
}

/// Theme-picker mouse slot: hover and wheel both move + LIVE-PREVIEW the selection (decision
/// 3A embraces hover-preview; I5 dedupes so a row that doesn't change fires no preview);
/// `Down(Left)` on a row commits it, on a click-away restores the captured original theme
/// (Esc-equivalent).
pub(crate) fn mouse_theme_picker(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    _ctx: &crate::overlays::DispatchCtx) {
    // Hover: move the highlight to the row under the pointer AND fire the live preview funnel —
    // but ONLY when the row differs from `selected` (dedupe-on-row-change, I5; decision 3A
    // embraces hover-preview, so the dedupe is what keeps the theme re-derive off the hot path).
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.theme_picker.as_ref()
            .and_then(|tp| crate::chrome_geom::theme_picker_row_at(area, tp, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            let changed = editor.theme_picker.as_ref().is_some_and(|tp| tp.selected != idx);
            if changed {
                if let Some(tp) = editor.theme_picker.as_mut() {
                    tp.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, tp.rows.len(), &mut tp.scroll_top);
                }
                crate::theme_cmds::preview_selected_theme(editor); // dedupe: only on row change
            }
        }
        return;
    }
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.theme_picker.as_ref() { Some(tp) => tp.selected, None => return };
        let scrolled = if let Some(tp) = editor.theme_picker.as_mut() {
            let n = tp.rows.len();
            if n == 0 { return; } // I3b: no step/scroll/preview on an empty list
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut tp.selected, &mut tp.scroll_top)
        } else { return };
        if scrolled {
            // Re-hover: the pointer is stationary, so pin the highlight to its row (ruling 1a).
            if let Some(idx) = editor.theme_picker.as_ref()
                .and_then(|tp| crate::chrome_geom::theme_picker_row_at(area, tp, ev.column, ev.row))
            {
                if let Some(tp) = editor.theme_picker.as_mut() { tp.selected = idx; }
            }
        }
        // I5 dedupe: re-window AND fire the preview funnel ONLY when the row actually moved —
        // a clamp-boundary notch that leaves `selected` unchanged fires NO preview.
        let after = editor.theme_picker.as_ref().map(|tp| tp.selected).unwrap_or(before);
        if after != before {
            let n = editor.theme_picker.as_ref().map(|tp| tp.rows.len()).unwrap_or(0);
            if let Some(tp) = editor.theme_picker.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut tp.scroll_top);
            }
            crate::theme_cmds::preview_selected_theme(editor);
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // Scoped borrows → owned hit values before any mutation.
        let row_idx: Option<usize> = {
            let tp = editor.theme_picker.as_ref().unwrap();
            crate::chrome_geom::theme_picker_row_at(area, tp, ev.column, ev.row)
        };
        let inside = {
            let tp = editor.theme_picker.as_ref().unwrap();
            let r = crate::chrome_geom::palette_overlay_rect(area, tp.rows.len());
            ev.column >= r.x && ev.column < r.x + r.width
                && ev.row >= r.y && ev.row < r.y + r.height
        };
        if let Some(idx) = row_idx {
            // Set selected to the clicked row, preview, then commit — same
            // identity logic as the keyboard Enter arm (via shared helper).
            let ah = editor.active().view.area.1;
            if let Some(tp) = editor.theme_picker.as_mut() {
                tp.selected = idx;
                crate::app::keep_overlay_visible(ah, idx, tp.rows.len(), &mut tp.scroll_top);
            }
            crate::theme_cmds::preview_selected_theme(editor);
            crate::theme_cmds::commit_theme_picker(editor);
        } else if !inside {
            // Click-away: restore the original theme and close — same as Esc.
            if let Some(tp) = editor.theme_picker.take() {
                editor.apply_theme(tp.original);
            }
        }
    }
}

/// Cursor-picker mouse slot. Fixed 7-row list, windowed like every sibling overlay (Finding 1).
/// Hover and wheel both move + LIVE-PREVIEW the selection via the shared setter funnel (I5
/// dedupes so a row that doesn't change fires no preview); a row click selects + previews +
/// commits; a click-away restores the captured originals and closes (Esc-equivalent). No
/// setter bypass.
pub(crate) fn mouse_cursor_picker(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    _ctx: &crate::overlays::DispatchCtx) {
    // Hover: move the highlight to the row under the pointer AND fire the live preview funnel —
    // but ONLY when the row differs from `selected` (dedupe-on-row-change, I5).
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.cursor_picker.as_ref()
            .and_then(|cp| crate::chrome_geom::cursor_picker_row_at(area, cp, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            let changed = editor.cursor_picker.as_ref().is_some_and(|cp| cp.selected != idx);
            if changed {
                if let Some(cp) = editor.cursor_picker.as_mut() {
                    cp.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, crate::cursor_picker::ROW_ACTIONS.len(), &mut cp.scroll_top);
                }
                crate::cursor_picker::preview_selected(editor); // dedupe: only on row change
            }
        }
        return;
    }
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.cursor_picker.as_ref() { Some(cp) => cp.selected, None => return };
        let scrolled = if let Some(cp) = editor.cursor_picker.as_mut() {
            let n = crate::cursor_picker::ROW_ACTIONS.len();
            if n == 0 { return; } // I3b: defensive-uniform; ROW_ACTIONS is a fixed non-empty table
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut cp.selected, &mut cp.scroll_top)
        } else { return };
        if scrolled {
            if let Some(idx) = editor.cursor_picker.as_ref()
                .and_then(|cp| crate::chrome_geom::cursor_picker_row_at(area, cp, ev.column, ev.row))
            {
                if let Some(cp) = editor.cursor_picker.as_mut() { cp.selected = idx; }
            }
        }
        // I5 dedupe: re-window AND fire the preview funnel ONLY when the row actually moved.
        let after = editor.cursor_picker.as_ref().map(|cp| cp.selected).unwrap_or(before);
        if after != before {
            let n = crate::cursor_picker::ROW_ACTIONS.len();
            if let Some(cp) = editor.cursor_picker.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut cp.scroll_top);
            }
            crate::cursor_picker::preview_selected(editor);
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // Scoped borrow → owned hit value before any mutation.
        let row_idx: Option<usize> = {
            let cp = editor.cursor_picker.as_ref().unwrap();
            crate::chrome_geom::cursor_picker_row_at(area, cp, ev.column, ev.row)
        };
        let inside = {
            let n = crate::cursor_picker::ROW_ACTIONS.len();
            let r = crate::chrome_geom::palette_overlay_rect(area, n + 1);
            ev.column >= r.x && ev.column < r.x + r.width
                && ev.row >= r.y && ev.row < r.y + r.height
        };
        if let Some(idx) = row_idx {
            let ah = editor.active().view.area.1;
            if let Some(cp) = editor.cursor_picker.as_mut() {
                cp.selected = idx;
                crate::app::keep_overlay_visible(ah, idx, crate::cursor_picker::ROW_ACTIONS.len(), &mut cp.scroll_top);
            }
            crate::cursor_picker::preview_selected(editor);
            crate::cursor_picker::commit_cursor_picker(editor);
        } else if !inside {
            // Click-away: restore the captured originals and close — same as Esc.
            if let Some(cp) = editor.cursor_picker.take() {
                editor.set_caret_shape(cp.original_shape);
                editor.set_caret_blink(cp.original_blink);
            }
        }
    }
}

/// File-browser mouse slot: wheel moves + windows the selection; `Down(Left)` on a row enters
/// it (dir descend / file open), on a click-away closes the browser.
pub(crate) fn mouse_file_browser(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    _ctx: &crate::overlays::DispatchCtx) {
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.file_browser.as_ref()
            .and_then(|fb| crate::chrome_geom::file_browser_row_at(area, fb, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            if let Some(fb) = editor.file_browser.as_mut() {
                if fb.selected != idx {
                    fb.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, fb.entries.len(), &mut fb.scroll_top);
                }
            }
        }
        return;
    }
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.file_browser.as_ref() { Some(fb) => fb.selected, None => return };
        let scrolled = if let Some(fb) = editor.file_browser.as_mut() {
            let n = fb.entries.len();
            if n == 0 { return; }
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut fb.selected, &mut fb.scroll_top)
        } else { return };
        if scrolled {
            if let Some(idx) = editor.file_browser.as_ref()
                .and_then(|fb| crate::chrome_geom::file_browser_row_at(area, fb, ev.column, ev.row))
            {
                if let Some(fb) = editor.file_browser.as_mut() { fb.selected = idx; }
            }
        }
        let after = editor.file_browser.as_ref().map(|fb| fb.selected).unwrap_or(before);
        if after != before {
            let n = editor.file_browser.as_ref().map(|fb| fb.entries.len()).unwrap_or(0);
            if let Some(fb) = editor.file_browser.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut fb.scroll_top);
            }
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // Scoped borrows → owned hit values before any mutation.
        let row_idx: Option<usize> = {
            let fb = editor.file_browser.as_ref().unwrap();
            crate::chrome_geom::file_browser_row_at(area, fb, ev.column, ev.row)
        };
        let inside = {
            let fb = editor.file_browser.as_ref().unwrap();
            let r = crate::chrome_geom::palette_overlay_rect(area, fb.entries.len());
            ev.column >= r.x && ev.column < r.x + r.width
                && ev.row >= r.y && ev.row < r.y + r.height
        };
        if let Some(idx) = row_idx {
            // Set selected to the clicked row, then execute — same
            // dir/file logic as the keyboard Enter arm (via shared helper).
            let ah = editor.active().view.area.1;
            if let Some(fb) = editor.file_browser.as_mut() {
                fb.selected = idx;
                crate::app::keep_overlay_visible(ah, idx, fb.entries.len(), &mut fb.scroll_top);
            }
            crate::file_browser::file_browser_enter(editor);
        } else if !inside {
            editor.file_browser = None; // click-away closes
        }
    }
}

/// Outline mouse slot: wheel moves + windows the selection; `Down(Left)` on a row jumps to its
/// heading (guarded by the stale-version check), on a click-away closes the outline.
pub(crate) fn mouse_outline(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    _ctx: &crate::overlays::DispatchCtx) {
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.outline.as_ref()
            .and_then(|o| crate::chrome_geom::outline_row_at(area, o, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            if let Some(o) = editor.outline.as_mut() {
                if o.selected != idx {
                    o.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, o.rows.len(), &mut o.scroll_top);
                }
            }
        }
        return;
    }
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.outline.as_ref() { Some(o) => o.selected, None => return };
        let scrolled = if let Some(o) = editor.outline.as_mut() {
            let n = o.rows.len();
            if n == 0 { return; }
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut o.selected, &mut o.scroll_top)
        } else { return };
        if scrolled {
            if let Some(idx) = editor.outline.as_ref()
                .and_then(|o| crate::chrome_geom::outline_row_at(area, o, ev.column, ev.row))
            {
                if let Some(o) = editor.outline.as_mut() { o.selected = idx; }
            }
        }
        let after = editor.outline.as_ref().map(|o| o.selected).unwrap_or(before);
        if after != before {
            let n = editor.outline.as_ref().map(|o| o.rows.len()).unwrap_or(0);
            if let Some(o) = editor.outline.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut o.scroll_top);
            }
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // Scoped borrows → owned hit values before any mutation.
        let row_idx: Option<usize> = {
            let o = editor.outline.as_ref().unwrap();
            crate::chrome_geom::outline_row_at(area, o, ev.column, ev.row)
        };
        let inside = {
            let o = editor.outline.as_ref().unwrap();
            let r = crate::chrome_geom::palette_overlay_rect(area, o.rows.len());
            ev.column >= r.x && ev.column < r.x + r.width
                && ev.row >= r.y && ev.row < r.y + r.height
        };
        if let Some(idx) = row_idx {
            let ah = editor.active().view.area.1;
            if let Some(o) = editor.outline.as_mut() {
                o.selected = idx;
                crate::app::keep_overlay_visible(ah, idx, o.rows.len(), &mut o.scroll_top);
            }
            // Stale-version guard — mirrors the keyboard Enter arm (app.rs:1018):
            // refuse a jump when the outline was opened against an older document version.
            if editor.outline.as_ref().map(|o| o.opened_version)
                != Some(editor.active().document.version)
            {
                editor.set_status_full(crate::status::StatusKind::Warning, "document changed; outline closed",
                    crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                editor.outline = None;
                return;
            }
            let target = editor.outline.as_ref()
                .and_then(|o| o.rows.get(o.selected))
                .map(|r| r.byte);
            if let Some(byte) = target {
                crate::outline_overlay::outline_jump_to(editor, byte);
            }
        } else if !inside {
            editor.outline = None; // click-away closes
        }
    }
}

/// Diagnostics mouse slot: wheel moves + windows the selection; `Down(Left)` on a row applies
/// it (via `diag_apply_selected`, which owns the stale-version guard), on a click-away closes.
pub(crate) fn mouse_diag(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    ctx: &crate::overlays::DispatchCtx) {
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.diag.as_ref()
            .and_then(|d| crate::chrome_geom::diag_row_at(area, d, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            if let Some(d) = editor.diag.as_mut() {
                let rc = d.row_count();
                if d.selected != idx {
                    d.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, rc, &mut d.scroll_top);
                }
            }
        }
        return;
    }
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.diag.as_ref() { Some(d) => d.selected, None => return };
        let scrolled = if let Some(d) = editor.diag.as_mut() {
            let n = d.row_count();
            if n == 0 { return; }
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut d.selected, &mut d.scroll_top)
        } else { return };
        if scrolled {
            if let Some(idx) = editor.diag.as_ref()
                .and_then(|d| crate::chrome_geom::diag_row_at(area, d, ev.column, ev.row))
            {
                if let Some(d) = editor.diag.as_mut() { d.selected = idx; }
            }
        }
        let after = editor.diag.as_ref().map(|d| d.selected).unwrap_or(before);
        if after != before {
            let n = editor.diag.as_ref().map(|d| d.row_count()).unwrap_or(0);
            if let Some(d) = editor.diag.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut d.scroll_top);
            }
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // Scoped borrows → owned hit values before any mutation.
        let row_idx: Option<usize> = {
            let d = editor.diag.as_ref().unwrap();
            crate::chrome_geom::diag_row_at(area, d, ev.column, ev.row)
        };
        let inside = {
            let d = editor.diag.as_ref().unwrap();
            let r = crate::chrome_geom::palette_overlay_rect(area, d.row_count());
            ev.column >= r.x && ev.column < r.x + r.width
                && ev.row >= r.y && ev.row < r.y + r.height
        };
        if let Some(idx) = row_idx {
            // Set selected to the clicked row, then apply — reuse
            // diag_apply_selected which owns the stale-version guard.
            let ah = editor.active().view.area.1;
            if let Some(d) = editor.diag.as_mut() {
                let rc = d.row_count();
                d.selected = idx;
                crate::app::keep_overlay_visible(ah, idx, rc, &mut d.scroll_top);
            }
            crate::search_ui::diag_apply_selected(editor, ctx.clock);
        } else if !inside {
            editor.diag = None; // click-away closes
        }
    }
}

/// Prompt mouse slot. Task 13: prompt choice clicks — on Down(Left) over a `[K]` marker,
/// dispatch via the shared keyboard resolver; all other events (including off-marker clicks)
/// are consumed so the prompt stays open and nothing leaks to the editor.
pub(crate) fn mouse_prompt(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    ctx: &crate::overlays::DispatchCtx) {
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // Scoped borrow → owned action (PromptAction: Copy) before mutable dispatch.
        let action: Option<crate::prompt::PromptAction> = editor.prompt.as_ref()
            .and_then(|p| crate::chrome_geom::prompt_choice_at(area, p, ev.column, ev.row));
        if let Some(action) = action {
            // resolve_prompt clears editor.prompt in its arms — do NOT clear it here.
            crate::prompts::resolve_prompt(action, editor, ctx.ex, ctx.clock, ctx.msg_tx);
        }
    }
}

/// Minibuffer mouse slot. A13 Task 5.1: minibuffer click → caret. `Down(Left)` inside the
/// input line positions the caret at the clicked byte; all other events (incl. clicks on the
/// prompt or off the status row) are consumed no-ops — outside-click-to-dismiss is deliberately
/// out of scope (per the task brief).
pub(crate) fn mouse_minibuffer(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    _ctx: &crate::overlays::DispatchCtx) {
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        if let Some(mb) = editor.minibuffer.as_mut() {
            if let Some(byte) = crate::chrome_geom::minibuffer_click_byte(area, mb, ev.column, ev.row) {
                mb.cursor = byte;
            }
        }
    }
}

/// Search mouse slot. A13 Task 5.2: search overlay click — two targets. `Down(Left)` on the
/// status row inside either field focuses it + positions the caret (chrome_geom's
/// search_field_click, sharing the painter's prefix-width source). `Down(Left)` in the edit
/// band, on a highlighted match, selects that match — strict three-step order (spec §5.2):
/// (1) cache-only refresh via `SearchState::recompute` DIRECTLY — NOT `search_ui::search_sync`,
/// whose unfold/select/rebuild/ensure_visible would move the viewport BEFORE the click is
/// mapped; (2) map the click to a document byte via the same `nav::offset_at_cell` path the
/// no-overlay click uses; (3) if the byte lands inside a match, `set_current_at_or_after` +
/// the shared placement tail (`search_ui::search_pin`). The overlay STAYS OPEN either way. All
/// other events, and clicks that hit neither target, are consumed no-ops.
pub(crate) fn mouse_search(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    _ctx: &crate::overlays::DispatchCtx) {
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        let field_hit: Option<(crate::search_overlay::Field, usize)> = editor.search.as_ref()
            .and_then(|s| crate::chrome_geom::search_field_click(area, s, ev.column, ev.row));
        if let Some((field, cursor)) = field_hit {
            if let Some(s) = editor.search.as_mut() { s.field = field; s.cursor = cursor; }
            return;
        }
        if let CellHit::Text { col, erow } = editing_cell(editor, ev.column, ev.row) {
            // Step 1 — cache-only refresh (spec §5.3): current buffer/version, NOT search_sync.
            let (rope, version) = { let d = &editor.active().document; (d.buffer.snapshot(), d.version) };
            if let Some(s) = editor.search.as_mut() { s.recompute(&rope, version); }
            // Step 2 — map the click to a document byte on the (now-current) layout.
            if let Some(byte) = crate::nav::offset_at_cell(editor, col, erow) {
                // Step 3 — choose + place.
                let hit_match = editor.search.as_ref()
                    .and_then(|s| s.matches().iter().copied().find(|m| m.start <= byte && byte < m.end));
                if let Some(m) = hit_match {
                    if let Some(s) = editor.search.as_mut() { s.set_current_at_or_after(m.start); }
                    crate::search_ui::search_pin(editor);
                }
            }
        }
    }
}

#[allow(clippy::too_many_lines)] // mouse event dispatch — one branch per screen region
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
    // Overlay routing runs BEFORE the dwell-arming block: while any modal is open
    // the event is consumed by route_overlay and nothing leaks to the dwell timers
    // or the editor. The dwell block + editor match below therefore run ONLY when
    // no overlay is open.
    let (w, h) = editor.active().view.area;
    let area = ratatui::layout::Rect::new(0, 0, w, h);
    if !no_overlay_open(editor) {
        let ctx = crate::overlays::DispatchCtx { reg, keymap, ex, clock, msg_tx };
        route_overlay(editor, ev, area, &ctx);
        return;
    }
    // ── from here down: no overlay open — dwell arming + editor gestures ──
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
                if no_overlay_open(editor)
                    && !editor.mouse.dragging
                    && !editor.mouse.scrollbar_dragging
                    && !editor.mouse.menu_bar_revealed
                {
                    editor.mouse.menu_reveal_due = Some(clock.now_ms() + MENU_DWELL_MS);
                }
            }
        }
    }
    // Scrollbar right-edge dwell (mirror of the menu-bar dwell; col w-1 is the track).
    if editor.scrollbar_mode == crate::config::TransientMode::Auto {
        if let MouseEventKind::Moved = ev.kind {
            let w = editor.active().view.area.0;
            let at_right_edge = ev.column == w.saturating_sub(1);
            if at_right_edge {
                editor.mouse.scrollbar_hide_due = None;
                if !editor.mouse.scrollbar_revealed
                    && editor.mouse.scrollbar_reveal_due.is_none()
                    && no_overlay_open(editor)
                    && !editor.mouse.dragging && !editor.mouse.scrollbar_dragging
                {
                    editor.mouse.scrollbar_reveal_due = Some(clock.now_ms() + MENU_DWELL_MS);
                }
            } else {
                editor.mouse.scrollbar_reveal_due = None;
                if editor.mouse.scrollbar_revealed && editor.mouse.scrollbar_hide_due.is_none() {
                    editor.mouse.scrollbar_hide_due = Some(clock.now_ms() + MENU_LEAVE_GRACE_MS);
                }
            }
        }
    }
    // Status-line bottom-row dwell (mirror of scrollbar dwell; row h-1 is the reserved row).
    if editor.status_line_mode == crate::config::TransientMode::Auto {
        if let MouseEventKind::Moved = ev.kind {
            let h = editor.active().view.area.1;
            let at_bottom = h > 0 && ev.row == h - 1;
            if at_bottom {
                editor.mouse.status_hide_due = None;
                if !editor.mouse.status_revealed
                    && editor.mouse.status_reveal_due.is_none()
                    && no_overlay_open(editor)
                {
                    editor.mouse.status_reveal_due = Some(clock.now_ms() + MENU_DWELL_MS);
                }
            } else {
                editor.mouse.status_reveal_due = None;
                if editor.mouse.status_revealed && editor.mouse.status_hide_due.is_none() {
                    editor.mouse.status_hide_due = Some(clock.now_ms() + MENU_LEAVE_GRACE_MS);
                }
            }
        }
    }
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let hit = editing_cell(editor, ev.column, ev.row);
            if let CellHit::MenuBar = hit {
                // Inactive bar: open the dropdown AT the clicked category (hydrated
                // by reduce's post-handle hydrate_overlays call).
                let cats_hit = crate::chrome_geom::menu_bar_layout_cats(area, &crate::registry::MENU_ORDER)
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
        let rect = crate::chrome_geom::palette_overlay_rect(area, e.palette.as_ref().unwrap().rows.len());
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
        let rect = crate::chrome_geom::palette_overlay_rect(area, e.palette.as_ref().unwrap().rows.len());
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

    /// A21 T2: 20 ScrollDown notches slide the viewport by WHEEL_STEP each (through-list
    /// wheel, not a ±1 step) and drag the highlight along with it, keeping it visible.
    /// Pointer stays OFF the overlay rect (column 0, row 0) so no re-hover overrides the
    /// clamp_into_window result — this isolates the pure wheel_list mechanics from I5.
    #[test]
    fn wheel_moves_selection_and_window() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &km);
        e.palette = Some(p);
        // 20 scroll-downs, pointer off-rect.
        let scroll_down = MouseEvent { kind: MouseEventKind::ScrollDown, column: 0, row: 0, modifiers: KeyModifiers::NONE };
        for _ in 0..20 {
            handle(&mut e, scroll_down, &reg, &km, &ex, &clk, &tx);
        }
        let p = e.palette.as_ref().expect("palette still open after wheel");
        let lh = crate::list_window::list_h_for(p.rows.len(), 24);
        let max_top = p.rows.len().saturating_sub(lh);
        let expected = (20 * crate::list_window::WHEEL_STEP).min(max_top);
        assert_eq!(p.scroll_top, expected, "viewport slid by WHEEL_STEP per notch, clamped to max_top");
        assert_eq!(p.selected, expected, "highlight tracks the window's lower edge (clamp_into_window)");
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
        // (1, 0) is outside the overlay rect — click-away closes the picker and
        // restores the original theme; the caret must not move regardless.
        handle(&mut e, down(1, 0), &reg, &km, &ex, &clk, &tx);
        assert_eq!(crate::nav::head(&e), 0, "click absorbed by theme picker — caret must not move");
        assert!(e.theme_picker.is_none(), "click-away closes the theme picker");
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
    /// opens a placeholder with open == MENU_ORDER index of Format (== 3).
    #[test]
    fn click_on_inactive_bar_opens_that_category() {
        use crate::config::MenuBarMode;
        let mut e = Editor::new_from_text("hello\n", None, (80, 8));
        crate::derive::rebuild(&mut e);
        e.menu_bar_mode = MenuBarMode::Pinned;
        e.menu = None;
        let (reg, ex, clk, tx, km) = ctx();
        // Compute the Format label column dynamically (MENU_ORDER[3] = Format).
        let (w, h) = e.active().view.area;
        let area = ratatui::layout::Rect::new(0, 0, w, h);
        let menu_area = ratatui::layout::Rect::new(area.x, area.y, w, h.saturating_sub(1));
        let bar = crate::chrome_geom::menu_bar_layout_cats(menu_area, &crate::registry::MENU_ORDER);
        let (_, format_rect) = bar.iter().find(|(i, _)| *i == 3).expect("Format at index 3");
        let col = format_rect.x + 1; // somewhere inside the label

        // Click on the Format label while the bar is inactive (menu None).
        handle(&mut e, down(col, 0), &reg, &km, &ex, &clk, &tx);
        let menu = e.menu.as_ref().expect("click must set editor.menu to Some placeholder");
        assert!(!menu.built, "placeholder must not be built (hydration happens in reduce)");
        assert_eq!(menu.open, 3, "placeholder open must be the MENU_ORDER index of Format");

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

    /// New invariant (overlay-route-before-dwell): leave-bookkeeping does NOT run while
    /// the dropdown is open — dwell is suppressed for every open overlay (no-leak guard).
    #[test]
    fn dwell_suppressed_while_dropdown_open() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 8));
        crate::derive::rebuild(&mut e);
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        e.mouse.menu_bar_revealed = true;
        e.menu = Some(crate::menu::empty_at(0)); // dropdown open
        let (reg, ex, _, tx, km) = ctx();
        handle(&mut e, moved(5, 5), &reg, &km, &ex, &TestClock(0), &tx);
        assert!(e.mouse.menu_hide_due.is_none(),
            "dwell (incl. leave-bookkeeping) must not run while an overlay is open");
    }

    #[test]
    fn scrollbar_dwell_arms_on_right_edge_rest() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 8));
        crate::derive::rebuild(&mut e);
        e.scrollbar_mode = crate::config::TransientMode::Auto;
        let (reg, ex, _, tx, km) = ctx();
        handle(&mut e, moved(39, 4), &reg, &km, &ex, &TestClock(0), &tx); // col w-1 = 39
        assert_eq!(e.mouse.scrollbar_reveal_due, Some(MENU_DWELL_MS));
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

    /// A21 T4: ScrollDown wheel on the theme picker slides the viewport by WHEEL_STEP per
    /// notch (through-list wheel, not a ±1 step) and previews the correct row. Pointer stays
    /// OFF the overlay rect (column 0, row 0) so no re-hover overrides the clamp_into_window
    /// result — isolates the pure wheel_list mechanics from I5 (same precedent as A21 T2's
    /// palette/outline/file_browser fixes).
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
            kind: MouseEventKind::ScrollDown, column: 0, row: 0,
            modifiers: KeyModifiers::NONE,
        };
        // 16 scroll-downs — pushes past the 15-row window.
        for _ in 0..16 {
            handle(&mut e, scroll_down, &reg, &km, &ex, &clk, &tx);
        }
        let tp = e.theme_picker.as_ref().expect("picker must remain open");
        let max_top = 20usize.saturating_sub(lh);
        let expected = (16 * crate::list_window::WHEEL_STEP).min(max_top);
        assert_eq!(tp.scroll_top, expected, "viewport slid by WHEEL_STEP per notch, clamped to max_top");
        assert_eq!(tp.selected, expected, "highlight tracks the window's leading edge (clamp_into_window)");
        assert!(tp.selected.saturating_sub(tp.scroll_top) < lh,
            "tp wheel: selection visible (selected={}, scroll_top={}, lh={})",
            tp.selected, tp.scroll_top, lh);
        // The applied theme must equal tp.rows[tp.selected] (wheel previews correct row).
        let expected_name = tp.rows[tp.selected].clone();
        assert_eq!(e.theme.name, expected_name,
            "tp wheel: applied theme={:?} must equal tp.rows[selected]={expected_name:?}",
            e.theme.name);
    }

    /// A21 T2: ScrollDown wheel on the file browser slides the viewport by WHEEL_STEP per
    /// notch and keeps the window visible; the unconditional `return` still prevents
    /// text-area events. Pointer stays OFF the overlay rect (column 0, row 0) so no
    /// re-hover overrides the clamp_into_window result.
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
            kind: MouseEventKind::ScrollDown, column: 0, row: 0,
            modifiers: KeyModifiers::NONE,
        };
        for _ in 0..20 {
            handle(&mut e, scroll_down, &reg, &km, &ex, &clk, &tx);
        }
        let fb = e.file_browser.as_ref().expect("browser must remain open");
        let lh = crate::list_window::list_h_for(fb.entries.len(), 24);
        let max_top = fb.entries.len().saturating_sub(lh);
        let expected = (20 * crate::list_window::WHEEL_STEP).min(max_top);
        assert_eq!(fb.scroll_top, expected, "viewport slid by WHEEL_STEP per notch, clamped to max_top");
        assert_eq!(fb.selected, expected, "highlight tracks the window's lower edge (clamp_into_window)");
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
    /// Unscrolled click (2 buffers — doc + a second ordinary buffer B; scratch is
    /// excluded from the switcher per A12): the list fits in the window without
    /// scrolling. A scrolled variant is not needed here — the abs-row mapping for
    /// scrolled clicks is already covered by `scrolled_click_maps_to_absolute_row`;
    /// the bug is in the dispatch branch, not in the hit-test, so an unscrolled
    /// click exercises the full fix path.
    #[test]
    fn click_buffers_palette_row_switches_buffer_not_reopens() {
        let mut e = Editor::new_from_text(
            "doc\n", Some(std::path::PathBuf::from("/tmp/a.md")), (80, 24));
        e.install_scratch();
        // buffers[0] = doc (active), buffers[1] = scratch, buffers[2] = B (ordinary).
        let b_id = e.alloc_id();
        let area_b = e.active().view.area;
        e.buffers.push(crate::editor::Buffer::from_text(b_id, "b\n", None, area_b));
        assert_eq!(e.active, 0, "precondition: doc is active before the click");
        e.open_buffer_switcher();
        // rows[0] = doc (MRU front), rows[1] = B (appended, scratch excluded) —
        // both carry buffer: Some(id).
        assert_eq!(e.palette.as_ref().unwrap().rows.len(), 2,
            "precondition: exactly 2 rows in the Buffers palette (scratch excluded)");
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, 2);
        // Click the second list row (rows[1] = B) at ov_y + 2 + 1.
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
        assert_eq!(e.active().id, b_id,
            "click must switch to the clicked row's buffer (B), not reopen the palette");
    }

    // -----------------------------------------------------------------------
    // Task 9: universal no-leak guard + dwell ordering
    // -----------------------------------------------------------------------

    /// Dwell must never arm the menu-bar reveal while ANY modal is open — the
    /// overlay route sits before the dwell-arming block, so no modal can leak a
    /// motion event into the dwell timers.
    #[test]
    // A table of (label, setup) pairs — the tuple type is intentionally literal here.
    #[allow(clippy::type_complexity)]
    fn dwell_never_arms_under_any_modal() {
        let modal_setups: Vec<(&str, fn(&mut Editor))> = vec![
            ("prompt", |e| e.prompt = Some(crate::prompt::Prompt::quit_confirm())),
            ("minibuffer", |e| e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter)),
            ("search", |e| e.search = Some(crate::search_overlay::SearchState::open(
                crate::search_overlay::Phase::Find, 0, crate::editor::BufferId(1)))),
            ("outline", |e| e.open_outline()),
            ("diag", |e| {
                let d = wordcartel_core::diagnostics::Diagnostic {
                    range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                    source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                    message: "x".into(), suggestions: vec![] };
                let (id, ver) = (e.active().id, e.active().document.version);
                e.diag = Some(crate::diag_overlay::DiagOverlay::new(d, id, ver));
            }),
        ];
        for (name, setup) in modal_setups {
            let mut e = Editor::new_from_text("hello\n", None, (40, 8));
            crate::derive::rebuild(&mut e);
            e.menu_bar_mode = crate::config::MenuBarMode::Auto;
            setup(&mut e);
            let (reg, ex, _, tx, km) = ctx();
            handle(&mut e, moved(5, 0), &reg, &km, &ex, &TestClock(0), &tx);
            assert!(e.mouse.menu_reveal_due.is_none(), "{name}: modal open must suppress menu dwell");
        }
    }

    /// A click while a prompt is open must be consumed by the overlay route and
    /// never leak to the editor (the caret must not move).
    #[test]
    fn click_under_prompt_is_consumed_not_leaked_to_editor() {
        let mut e = Editor::new_from_text("abcdef\n", None, (40, 8));
        crate::derive::rebuild(&mut e);
        e.prompt = Some(crate::prompt::Prompt::quit_confirm());
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, down(3, 4), &reg, &km, &ex, &clk, &tx);
        assert_eq!(crate::nav::head(&e), 0, "click must not move the caret while a prompt is open");
    }

    // -----------------------------------------------------------------------
    // Task 10: theme-picker + file-browser click-to-commit + click-away
    // -----------------------------------------------------------------------

    /// A click on a visible theme-picker row applies that theme and closes the picker.
    #[test]
    fn click_theme_row_applies_and_closes() {
        let mut e = Editor::new_from_text("# H\n\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_theme_picker();
        // Rows are displayed alphabetically; locate the target in the DISPLAYED rows (not the
        // registration order) and pick one inside the initial 15-row window. forever-blue-jeans-dark
        // sorts near the top, so it is visible at scroll_top 0.
        let target = e.theme_picker.as_ref().unwrap().rows.iter()
            .position(|n| n == "forever-blue-jeans-dark").unwrap();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, e.theme_picker.as_ref().unwrap().rows.len());
        let click_row = rect.y + 2 + target as u16; // list starts ov_y+2 (scroll_top 0)
        let (reg, ex, clk, tx, km) = ctx();
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: click_row, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.theme_picker.is_none(), "picker closes on row click");
        assert_eq!(e.theme.name, "forever-blue-jeans-dark", "clicked theme applied");
    }

    /// A click outside the theme-picker overlay closes it and restores the original theme.
    #[test]
    fn click_outside_theme_picker_closes() {
        let mut e = Editor::new_from_text("# H\n\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_theme_picker();
        let original_name = e.theme.name.clone();
        let (reg, ex, clk, tx, km) = ctx();
        // (0, 0) is well outside the centered overlay — click-away.
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 0, row: 0, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.theme_picker.is_none(), "click-away closes the picker");
        assert_eq!(e.theme.name, original_name, "click-away restores the original theme");
    }

    // -----------------------------------------------------------------------
    // C1 T7 fix: cursor-picker mouse-path tests (Finding 2 — previously zero coverage).
    // Mirrors the theme-picker mouse tests above.
    // -----------------------------------------------------------------------

    /// A click on a visible cursor-picker row selects + previews (via the shared
    /// setters) and commits — same "select, preview, close" shape as the theme picker's
    /// row click.
    #[test]
    fn click_cursor_picker_row_applies_and_closes() {
        use crate::config::CaretShape;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_cursor_picker();
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, n + 1);
        let list_top = rect.y + 1; // no query row on the cursor picker
        let click_row = list_top + 3; // row 3 = Beam · blinking
        let (reg, ex, clk, tx, km) = ctx();
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: click_row, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.cursor_picker.is_none(), "picker closes on row click");
        assert_eq!(e.caret_shape, CaretShape::Beam, "clicked row applied via the shared caret_shape setter");
        assert!(e.caret_blink, "row 3 (Beam · blinking) applied via the shared caret_blink setter");
    }

    /// A click outside the cursor-picker overlay closes it and restores the captured
    /// originals — same as Esc — even after a live preview moved the caret settings away.
    #[test]
    fn click_outside_cursor_picker_closes_and_restores() {
        use crate::config::CaretShape;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.set_caret_shape(CaretShape::Beam);
        e.set_caret_blink(true);
        e.open_cursor_picker();
        let original = (e.cursor_picker.as_ref().unwrap().original_shape,
                        e.cursor_picker.as_ref().unwrap().original_blink);
        assert_eq!(original, (CaretShape::Beam, true), "picker captures the originals on open");
        // Preview a different row first — proves click-away UNDOES the live preview.
        if let Some(cp) = e.cursor_picker.as_mut() { cp.selected = 6; } // Underline · steady
        crate::cursor_picker::preview_selected(&mut e);
        assert_eq!(e.caret_shape, CaretShape::Underline, "preview applied before the click-away");
        let (reg, ex, clk, tx, km) = ctx();
        // (0, 0) is well outside the centered overlay — click-away.
        handle(&mut e, down(0, 0), &reg, &km, &ex, &clk, &tx);
        assert!(e.cursor_picker.is_none(), "click-away closes the cursor picker");
        assert_eq!(e.caret_shape, CaretShape::Beam, "click-away restores the original shape");
        assert!(e.caret_blink, "click-away restores the original blink");
    }

    /// A21 T4: wheel scroll slides the cursor-picker viewport by WHEEL_STEP per notch
    /// (`scroll_top` threaded — Finding 1) and re-previews via the shared setters. Uses a
    /// SHORT terminal so the scroll_top threading is actually exercised. Pointer stays OFF
    /// the overlay rect (column 0, row 0) so no re-hover overrides the clamp_into_window
    /// result — isolates the pure wheel_list mechanics from I5 (same precedent as A21 T2's
    /// palette/outline/file_browser fixes).
    #[test]
    fn wheel_moves_cursor_picker_selection_and_previews() {
        let mut e = Editor::new_from_text("x\n", None, (60, 9)); // short — list_h_for(7, 9) == 5
        crate::derive::rebuild(&mut e);
        e.open_cursor_picker();
        assert_eq!(e.cursor_picker.as_ref().unwrap().selected, 0,
            "initial row = 0 (F2: the picker opens on the row matching the current — Default — caret)");
        let (reg, ex, clk, tx, km) = ctx();
        let scroll_down = MouseEvent { kind: MouseEventKind::ScrollDown, column: 0, row: 0, modifiers: KeyModifiers::NONE };
        for _ in 0..6 {
            handle(&mut e, scroll_down, &reg, &km, &ex, &clk, &tx);
        }
        let cp = e.cursor_picker.as_ref().expect("picker still open after wheel");
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let lh = crate::list_window::list_h_for(n, 9);
        let max_top = n.saturating_sub(lh);
        let expected = (6 * crate::list_window::WHEEL_STEP).min(max_top);
        assert_eq!(cp.scroll_top, expected, "viewport slid by WHEEL_STEP per notch, clamped to max_top");
        assert_eq!(cp.selected, expected, "highlight tracks the window's leading edge (clamp_into_window)");
        assert!(cp.selected.saturating_sub(cp.scroll_top) < lh,
            "selection is within the visible window (selected - scroll_top < list_h)");
        let (_, _, shape, blink) = crate::cursor_picker::ROW_ACTIONS[expected];
        assert_eq!(e.caret_shape, shape, "wheel re-previews via the shared setter");
        if let Some(b) = blink { assert_eq!(e.caret_blink, b, "row {expected}'s blink applied"); }
    }

    // -----------------------------------------------------------------------
    // A21 Task 4: preview overlays — hover fires the preview funnel (dedupe-bounded)
    // -----------------------------------------------------------------------

    #[test]
    fn cursor_picker_hover_previews_the_row() {
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_cursor_picker();
        let (reg, ex, clk, tx, km) = ctx();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let r = crate::chrome_geom::palette_overlay_rect(area, n + 1);
        // Hover row 3 (Beam · blinking) — list starts at r.y + 1 (no query row).
        handle(&mut e, moved(r.x + 1, r.y + 1 + 3), &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.cursor_picker.as_ref().unwrap().selected, 3, "hover set the highlight");
        let (_, _, shape, _) = crate::cursor_picker::ROW_ACTIONS[3];
        assert_eq!(e.caret_shape, shape, "hover fired the preview funnel (caret shape changed live)");
    }

    #[test]
    fn cursor_picker_hover_same_row_does_not_re_preview() {
        // A repeated Moved at the SAME row must be a no-op (dedupe I5). We prove it by mutating
        // caret_shape out from under the picker and asserting a same-row hover does NOT restore it.
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_cursor_picker();
        let (reg, ex, clk, tx, km) = ctx();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let r = crate::chrome_geom::palette_overlay_rect(area, n + 1);
        handle(&mut e, moved(r.x + 1, r.y + 1 + 3), &reg, &km, &ex, &clk, &tx); // preview row 3
        e.set_caret_shape(crate::config::CaretShape::Default); // tamper
        handle(&mut e, moved(r.x + 1, r.y + 1 + 3), &reg, &km, &ex, &clk, &tx); // SAME row again
        assert_eq!(e.caret_shape, crate::config::CaretShape::Default,
            "same-row hover did NOT re-fire the preview (dedupe on row-change)");
    }

    #[test]
    fn cursor_picker_wheel_empty_guard_and_theme_restore() {
        // cursor_picker has a fixed 7-row list (never empty); assert Esc-restore after a hover
        // sweep leaves the ORIGINAL caret. open_cursor_picker captures original_shape/blink.
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let orig = e.caret_shape;
        e.open_cursor_picker();
        let (reg, ex, clk, tx, km) = ctx();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let r = crate::chrome_geom::palette_overlay_rect(area, n + 1);
        // Sweep across rows 1,2,3.
        for row in 1..=3u16 { handle(&mut e, moved(r.x + 1, r.y + 1 + row), &reg, &km, &ex, &clk, &tx); }
        // Esc through the intercept restores original + closes.
        crate::app::reduce(crate::app::Msg::Input(crossterm::event::Event::Key(
            crossterm::event::KeyEvent { code: crossterm::event::KeyCode::Esc,
                modifiers: KeyModifiers::NONE, kind: crossterm::event::KeyEventKind::Press,
                state: crossterm::event::KeyEventState::NONE })),
            &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.cursor_picker.is_none(), "Esc closed the picker");
        assert_eq!(e.caret_shape, orig, "Esc after a hover sweep restored the original caret");
    }

    #[test]
    fn theme_picker_wheel_empty_list_no_preview() {
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_theme_picker();
        // Filter the picker to zero rows.
        if let Some(tp) = e.theme_picker.as_mut() {
            tp.query = "zzz_no_theme_zzz".into();
            crate::theme_picker::rebuild_rows(tp);
            assert!(tp.rows.is_empty(), "precondition: zero theme rows");
            tp.selected = 0; tp.scroll_top = 0;
        }
        let theme_before = e.theme.clone();
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, wheel_ev(true, 40, 12), &reg, &km, &ex, &clk, &tx);
        handle(&mut e, wheel_ev(false, 40, 12), &reg, &km, &ex, &clk, &tx);
        let tp = e.theme_picker.as_ref().unwrap();
        assert_eq!((tp.selected, tp.scroll_top), (0, 0), "empty theme list: wheel is a total no-op");
        assert_eq!(e.theme, theme_before, "empty list fired NO preview (theme unchanged)");
    }

    #[test]
    fn cursor_picker_wheel_boundary_notch_fires_no_preview() {
        // I5 dedupe on the WHEEL path — MUTATION-DETECTING. Park `selected` at the BOTTOM row
        // (6 = Underline·steady) and wheel DOWN: a true boundary (wheel_list's `.min(n-1)` keeps
        // it at 6, so after == before). Pre-set the caret to a SENTINEL (Block) that DIFFERS from
        // row 6's action — so a spurious re-preview would overwrite it with Underline and the test
        // would catch it. Pointer (0,0) is off the (centered) overlay, so no re-hover interferes.
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_cursor_picker();
        let last = crate::cursor_picker::ROW_ACTIONS.len() - 1; // 6 (Underline·steady)
        { e.cursor_picker.as_mut().unwrap().selected = last; }
        e.set_caret_shape(crate::config::CaretShape::Block); // sentinel ≠ row 6's Underline
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, wheel_ev(true, 0, 0), &reg, &km, &ex, &clk, &tx); // down at bottom → no move
        assert_eq!(e.cursor_picker.as_ref().unwrap().selected, last, "still at the bottom boundary");
        assert_eq!(e.caret_shape, crate::config::CaretShape::Block,
            "boundary wheel notch did NOT re-fire preview (sentinel Block survives; a spurious \
             re-preview would set row 6's Underline)");
    }

    #[test]
    fn theme_picker_wheel_boundary_notch_fires_no_preview() {
        // I5 dedupe on the WHEEL path for the theme overlay — MUTATION-DETECTING via `previewed`.
        // Park at the TOP row and wheel UP: a true boundary (saturating_sub keeps selected at 0,
        // after == before). Pre-set `previewed` to a SENTINEL distinct from row 0's name — a
        // spurious re-preview would overwrite it with Some(rows[0]). Pointer (0,0) is off the
        // (centered) overlay, so no re-hover interferes.
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_theme_picker();
        assert!(!e.theme_picker.as_ref().unwrap().rows.is_empty(), "precondition: builtin themes present");
        { e.theme_picker.as_mut().unwrap().selected = 0; }
        let row0 = e.theme_picker.as_ref().unwrap().rows[0].clone();
        let sentinel = format!("__sentinel_not_{row0}");
        { e.theme_picker.as_mut().unwrap().previewed = Some(sentinel.clone()); }
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, wheel_ev(false, 0, 0), &reg, &km, &ex, &clk, &tx); // up at 0 → no move
        assert_eq!(e.theme_picker.as_ref().unwrap().selected, 0, "still at the top boundary");
        assert_eq!(e.theme_picker.as_ref().unwrap().previewed.as_deref(), Some(sentinel.as_str()),
            "boundary wheel notch did NOT re-fire preview (sentinel in `previewed` survives; a \
             spurious re-preview would set Some(rows[0]))");
    }

    /// N=3: hovering three DISTINCT rows fires the preview funnel EXACTLY three times — a
    /// mutation-guarded count (not merely "at least once" or "vacuously zero"). Each hover lands
    /// on a row whose caret shape DIFFERS from the row before it, so a missed or duplicated fire
    /// is visible in the final shape even without an instrumented counter.
    #[test]
    fn cursor_picker_hover_fires_preview_exactly_once_per_row_crossed() {
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_cursor_picker();
        let (reg, ex, clk, tx, km) = ctx();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let r = crate::chrome_geom::palette_overlay_rect(area, n + 1);
        for row in [1u16, 3, 5] {
            e.set_caret_shape(crate::config::CaretShape::Default); // tamper before each hover
            handle(&mut e, moved(r.x + 1, r.y + 1 + row), &reg, &km, &ex, &clk, &tx);
            let (_, _, shape, _) = crate::cursor_picker::ROW_ACTIONS[row as usize];
            assert_eq!(e.caret_shape, shape, "hover on row {row} fired exactly once (tamper undone)");
        }
    }

    // -----------------------------------------------------------------------
    // Task 11: outline scroll + click-to-jump + click-away
    // -----------------------------------------------------------------------

    /// A click on an outline row jumps the caret to that heading and closes the
    /// overlay. The stale-version guard is NOT tested here (see the keyboard test
    /// `outline_jump_refused_after_background_edit` in app.rs — mouse reuses the
    /// same guard).
    #[test]
    fn click_outline_row_jumps_and_closes() {
        let mut e = Editor::new_from_text("# A\n\n## B\n\nbody\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_outline();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rows_len = e.outline.as_ref().unwrap().rows.len();
        let rect = crate::chrome_geom::palette_overlay_rect(area, rows_len);
        let click_row = rect.y + 2 + 1; // second heading "## B"
        let target_byte = e.outline.as_ref().unwrap().rows[1].byte;
        let (reg, ex, clk, tx, km) = ctx();
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: click_row, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.outline.is_none(), "outline closes on click");
        assert_eq!(crate::nav::head(&e), target_byte, "caret jumps to the clicked heading");
    }

    /// A21 T2: a wheel event on the outline slides the viewport by WHEEL_STEP per notch and
    /// keeps the window visible. Pointer stays OFF the overlay rect (column 0, row 0) so no
    /// re-hover overrides the clamp_into_window result.
    #[test]
    fn outline_wheel_scroll_moves_selection() {
        let text: String = (0..20).map(|i| format!("# H{i}\n\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_outline();
        let (reg, ex, clk, tx, km) = ctx();
        let scroll_down = MouseEvent {
            kind: MouseEventKind::ScrollDown, column: 0, row: 0,
            modifiers: KeyModifiers::NONE,
        };
        for _ in 0..10 {
            handle(&mut e, scroll_down, &reg, &km, &ex, &clk, &tx);
        }
        let o = e.outline.as_ref().expect("outline must remain open after wheel");
        let lh = crate::list_window::list_h_for(o.rows.len(), 24);
        let max_top = o.rows.len().saturating_sub(lh);
        let expected = (10 * crate::list_window::WHEEL_STEP).min(max_top);
        assert_eq!(o.scroll_top, expected, "viewport slid by WHEEL_STEP per notch, clamped to max_top");
        assert_eq!(o.selected, expected, "highlight tracks the window's lower edge (clamp_into_window)");
        assert!(o.selected.saturating_sub(o.scroll_top) < lh,
            "outline wheel: selection visible (selected={}, scroll_top={}, lh={})",
            o.selected, o.scroll_top, lh);
    }

    /// A click outside the outline overlay closes it without jumping.
    #[test]
    fn click_outside_outline_closes() {
        let mut e = Editor::new_from_text("# A\n\n## B\n\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_outline();
        let before = crate::nav::head(&e);
        let (reg, ex, clk, tx, km) = ctx();
        // (0, 0) is well outside the centered overlay.
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 0, row: 0, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.outline.is_none(), "click-away closes the outline");
        assert_eq!(crate::nav::head(&e), before, "caret unchanged on click-away");
    }

    // -----------------------------------------------------------------------
    // Task 12: diag overlay — scroll + click-apply + click-away
    // -----------------------------------------------------------------------

    /// A mouse click on a suggestion row applies it and closes the overlay.
    /// The click-apply path reuses `diag_apply_selected` (with its stale guard).
    #[test]
    fn diag_click_applies_selected_row() {
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let v = e.active().document.version;
        e.diag = Some(crate::diag_overlay::DiagOverlay::new(
            wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                message: "misspelled".into(),
                suggestions: vec![
                    wordcartel_core::diagnostics::Suggestion::ReplaceWith("the".into()),
                ],
            },
            e.active().id,
            v,
        ));
        let (reg, ex, clk, tx, km) = ctx();
        // Diag list starts at ov_y + 1 (no query row). Click first row (index 0 = "the").
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let r = crate::chrome_geom::palette_overlay_rect(area, e.diag.as_ref().unwrap().row_count());
        let d = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: r.x + 1,
            row: r.y + 1,
            modifiers: KeyModifiers::NONE,
        };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.diag.is_none(), "overlay closed after click-apply");
        assert_eq!(e.active().document.buffer.to_string(), "the cat\n",
            "first suggestion was applied via click");
    }

    /// A click outside the diag overlay closes it without applying.
    #[test]
    fn click_outside_diag_closes() {
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let v = e.active().document.version;
        e.diag = Some(crate::diag_overlay::DiagOverlay::new(
            wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                message: "m".into(),
                suggestions: vec![
                    wordcartel_core::diagnostics::Suggestion::ReplaceWith("the".into()),
                ],
            },
            e.active().id,
            v,
        ));
        let buf_before = e.active().document.buffer.to_string();
        let (reg, ex, clk, tx, km) = ctx();
        // (0, 0) is outside the centered overlay.
        handle(&mut e, down(0, 0), &reg, &km, &ex, &clk, &tx);
        assert!(e.diag.is_none(), "click-away closes the diag overlay");
        assert_eq!(e.active().document.buffer.to_string(), buf_before,
            "buffer unchanged on click-away");
    }

    // -----------------------------------------------------------------------
    // Task 13: prompt choice clicks
    // -----------------------------------------------------------------------

    /// Clicking a choice marker on the status row while a prompt is open dispatches
    /// its action via `resolve_prompt` — the keyboard path is untouched.
    #[test]
    fn click_prompt_choice_dispatches_action() {
        let mut e = Editor::new_from_text("x\n", None, (80, 8));
        crate::derive::rebuild(&mut e);
        e.prompt = Some(crate::prompt::Prompt::quit_confirm());
        let area = ratatui::layout::Rect::new(0, 0, 80, 8);
        // Locate the `[Q]` marker by counting CHARS, not bytes. quit_confirm contains
        // `·` (U+00B7, 2 UTF-8 bytes, 1 terminal column) before `[Q]`, so the byte
        // offset overestimates the column by 1. Using char index = column (width-1)
        // ensures clicking exactly ON the `[` glyph, catching a byte-vs-column regression.
        let msg = e.prompt.as_ref().unwrap().message.clone();
        let chars: Vec<char> = msg.chars().collect();
        let q_col = chars.windows(3)
            .position(|w| w[0] == '[' && w[1] == 'Q' && w[2] == ']')
            .expect("quit marker present") as u16;
        let status_row = area.y + area.height - 1; // = 7 for this geometry
        let (reg, ex, clk, tx, km) = ctx();
        // Click at q_col + 1 — the `Q` glyph, inside the [Q]uit span.
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: q_col + 1, row: status_row, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.quit, "clicking [Q]uit anyway must trigger the QuitAnyway action");
    }

    /// transform_chooser uses lowercase markers `[r]/[u]/[v]` and double-space
    /// separators. The old hit-test searched only for `[U]` and split on `·`, so the
    /// transform prompt was entirely non-clickable. After the fix, clicking [u]nwrap
    /// resolves via `resolve_prompt` and closes the prompt.
    #[test]
    fn click_transform_prompt_choice_dispatches() {
        let mut e = Editor::new_from_text("hello world\n", None, (80, 8));
        crate::derive::rebuild(&mut e);
        e.prompt = Some(crate::prompt::Prompt::transform_chooser());
        let area = ratatui::layout::Rect::new(0, 0, 80, 8);
        let msg = e.prompt.as_ref().unwrap().message.clone();
        // Compute the char-column of the `[u]nwrap` marker (width-1; message is ASCII).
        let chars: Vec<char> = msg.chars().collect();
        let u_col = chars.windows(3)
            .position(|w| w[0] == '[' && w[1] == 'u' && w[2] == ']')
            .expect("must find [u] in transform_chooser message") as u16;
        let status_row = area.y + area.height - 1;
        let (reg, ex, clk, tx, km) = ctx();
        // Click at u_col + 1 — the `u` glyph, inside the [u]nwrap span.
        let d = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: u_col + 1,
            row: status_row,
            modifiers: KeyModifiers::NONE,
        };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        // resolve_prompt(Transform(Unwrap)) falls through to `editor.prompt = None` —
        // prompt dismissed confirms the click hit a valid choice.
        assert!(e.prompt.is_none(),
            "clicking [u]nwrap in transform_chooser must dismiss the prompt via resolve_prompt");
    }

    /// A click on the status row at a column with no marker span is consumed
    /// (the prompt overlay `return`s) — the prompt stays open, no action fires,
    /// and the caret does not move.
    #[test]
    fn click_off_marker_keeps_prompt_open() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 8));
        crate::derive::rebuild(&mut e);
        e.prompt = Some(crate::prompt::Prompt::quit_confirm());
        let caret_before = crate::nav::head(&e);
        let area = ratatui::layout::Rect::new(0, 0, 80, 8);
        let status_row = area.y + area.height - 1;
        let (reg, ex, clk, tx, km) = ctx();
        // Column 0 is before all marker spans in quit_confirm → no-op click.
        let d = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: status_row,
            modifiers: KeyModifiers::NONE,
        };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "click before first marker must keep the prompt open");
        assert_eq!(crate::nav::head(&e), caret_before,
            "click on status row while prompt open must not move the caret");
    }

    /// `·` (U+00B7) is 2 UTF-8 bytes but 1 terminal column. The old byte-offset
    /// hit-test introduced a 1-column dead zone after each separator. This test pins
    /// the char-based (width-1) fix: clicking at the CHAR column of `[Q]` (34) must
    /// fire QuitAnyway, while the BYTE offset (35) differs — proving the regression
    /// would be caught if byte offsets crept back in.
    #[test]
    fn click_prompt_choice_with_dot_separator_second_choice() {
        let mut e = Editor::new_from_text("x\n", None, (80, 8));
        crate::derive::rebuild(&mut e);
        e.prompt = Some(crate::prompt::Prompt::quit_confirm());
        let area = ratatui::layout::Rect::new(0, 0, 80, 8);
        let msg = e.prompt.as_ref().unwrap().message.clone();
        // Char-column of `[Q]` (34 for this message; byte offset is 35 due to `·`).
        let chars: Vec<char> = msg.chars().collect();
        let q_char_col = chars.windows(3)
            .position(|w| w[0] == '[' && w[1] == 'Q' && w[2] == ']')
            .expect("must find [Q] in quit_confirm message") as u16;
        let q_byte_col = msg.find("[Q]").expect("must find [Q] by str::find") as u16;
        // Precondition: `·` makes the byte offset exceed the char column.
        assert!(q_byte_col > q_char_col,
            "precondition: byte offset ({q_byte_col}) must exceed char col ({q_char_col}) — \
             `·` before [Q] is 2 bytes but 1 column");
        let status_row = area.y + area.height - 1;
        let (reg, ex, clk, tx, km) = ctx();
        // Click at the true char column — must hit [Q]uit anyway.
        let d = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: q_char_col,
            row: status_row,
            modifiers: KeyModifiers::NONE,
        };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.quit,
            "clicking at char col of [Q] (col {q_char_col}) must trigger QuitAnyway; \
             byte-offset col ({q_byte_col}) would miss the span start");
    }

    /// A click on a directory entry in the file browser descends into that directory.
    #[test]
    fn click_dir_enters() {
        let dir = std::env::temp_dir().join(format!("wc-t10-fbclick-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sub = dir.join("subdir");
        std::fs::create_dir(&sub).unwrap();
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        e.open_file_browser(dir.clone());
        let idx = e.file_browser.as_ref().unwrap().entries.iter()
            .position(|en| en.name == "subdir" && en.is_dir)
            .expect("subdir must appear in entries");
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, e.file_browser.as_ref().unwrap().entries.len());
        let click_row = rect.y + 2 + idx as u16; // scroll_top is 0
        let (reg, ex, clk, tx, km) = ctx();
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: click_row, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.file_browser.as_ref().is_some_and(|fb| fb.dir == sub),
            "click on dir must descend into it; dir={:?}", e.file_browser.as_ref().map(|fb| &fb.dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // Task 14: menu dropdown wheel scroll
    // -----------------------------------------------------------------------

    /// T14: ScrollDown on an open menu moves the highlight and scrolls the window
    /// once the highlight passes the visible rows (avail_below = 7 for h=8).
    #[test]
    fn menu_wheel_scrolls_dropdown() {
        let mut e = Editor::new_from_text("x\n", None, (80, 8)); // avail_below = 7
        crate::derive::rebuild(&mut e);
        // Synthetic 20-leaf category so the dropdown overflows the 7-row window and
        // scroll_top must advance (Codex plan gate round 2 — prove windowing, not just highlight).
        let leaves: Vec<(String, crate::menu::MenuRowAction)> =
            (0..20).map(|i| (format!("item{i}"), crate::menu::MenuRowAction::Command(crate::registry::CommandId("move_right")))).collect();
        e.menu = Some(crate::menu::MenuView {
            groups: vec![(crate::registry::MenuCategory::Edit, leaves)],
            open: 0, highlighted: 0, built: true, scroll_top: 0 });
        let (reg, ex, clk, tx, km) = ctx();
        let wheel = MouseEvent { kind: MouseEventKind::ScrollDown, column: 2, row: 3, modifiers: KeyModifiers::NONE };
        for _ in 0..10 { handle(&mut e, wheel, &reg, &km, &ex, &clk, &tx); }
        let m = e.menu.as_ref().unwrap();
        assert!(m.highlighted > 0, "wheel moves the highlight");
        assert!(m.scroll_top > 0, "wheel scrolls the window once the highlight passes the visible rows");
    }

    // -----------------------------------------------------------------------
    // Fable whole-branch regression: menu_area drift fix (Task 14 blocker)
    // -----------------------------------------------------------------------

    /// Geometry for the two Fable probes: 30×10 terminal, 9-leaf category so the
    /// dropdown overflows the paint window (avail_below = menu_area.height - 1 = 8
    /// after the fix → list_h = 8, item_rows = 7, indicator at row 8).
    ///
    /// Under the OLD code the mouse path passed the full-height area (h=10) to
    /// menu_dropdown_row_at, giving avail_below=9, list_h=9, overflows=false,
    /// item_rows=9 — so click row 8 (the PAINTED indicator) dispatched leaf 7.
    ///
    /// Helper: build a 9-leaf Edit groups vec and return the rendered terminal.
    #[cfg(test)]
    fn fable_menu_setup() -> (Editor, crate::registry::Registry, crate::keymap::KeyTrie,
                              InlineExecutor, TestClock,
                              std::sync::mpsc::Sender<crate::app::Msg>,
                              ratatui::Terminal<ratatui::backend::TestBackend>) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use crate::render::render;

        let leaves: Vec<(String, crate::menu::MenuRowAction)> =
            (0..9).map(|i| (format!("item{i:02}      "), crate::menu::MenuRowAction::Command(crate::registry::CommandId("move_right")))).collect();
        let mut e = Editor::new_from_text("abc\n", None, (30, 10));
        crate::derive::rebuild(&mut e);
        e.menu = Some(crate::menu::MenuView {
            groups: vec![(crate::registry::MenuCategory::Edit, leaves)],
            open: 0, highlighted: 0, built: true, scroll_top: 0,
        });

        let (reg, ex, clk, tx, km) = ctx();

        let mut term = Terminal::new(TestBackend::new(30, 10)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        (e, reg, km, ex, clk, tx, term)
    }

    /// Paint-truth: confirm the indicator row IS on the screen and the off-screen
    /// leaf label is NOT — so the two dispatch assertions below have a solid
    /// geometric foundation.
    ///
    /// 30×10, 9 leaves: menu_area.height = 9, avail_below = 8 → list_h = 8,
    /// item_rows = 7, indicator at row 1+7 = 8.  item 7 ("item07") is NOT painted.
    #[test]
    fn fable_menu_paint_truth_indicator_on_row8_item7_not_visible() {
        let (e, _, _, _, _, _, term) = fable_menu_setup();
        let buf = term.backend().buffer();
        // Indicator row (row 8) must not be blank — it carries "n/total" text.
        // We check the area.height-1 guard: row 9 is the status line, row 8 is the indicator.
        let row8_text: String = (0..30u16).map(|x| buf[(x, 8)].symbol().chars().next().unwrap_or(' ')).collect();
        assert!(row8_text.trim().contains('/'), "indicator row 8 must contain '/' from the n/total widget: got {row8_text:?}");
        // item07 label must NOT appear anywhere in the painted buffer.
        let all_text: String = (0..10u16).flat_map(|y| (0..30u16).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().chars().next().unwrap_or(' ')).collect();
        assert!(!all_text.contains("item07"), "item07 is off-screen and must not be painted: buf={all_text:?}");
        // Suppress unused-variable warning from destructuring — drop the editor.
        let _ = e;
    }

    /// Fable regression 1 — click the PAINTED indicator row must NOT dispatch.
    ///
    /// Row 8 is the n/total indicator (not an item).  Before the fix the mouse path
    /// used the full-height area (h=10 → list_h=9, overflows=false, item_rows=9) and
    /// dispatched the 8th leaf (move_right → caret 0→1).  After the fix both paths
    /// use menu_area (h=9 → list_h=8, overflows=true, item_rows=7) → None → close.
    #[test]
    fn menu_click_on_painted_indicator_row_does_not_dispatch() {
        let (mut e, reg, km, ex, clk, tx, _term) = fable_menu_setup();
        let caret_before = crate::nav::head(&e);
        // Drop column — inside the dropdown (x=0 is within the Edit label / dropdown area).
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1, row: 8, // row 8 = painted indicator row
            modifiers: KeyModifiers::NONE,
        };
        handle(&mut e, click, &reg, &km, &ex, &clk, &tx);
        assert_eq!(caret_before, crate::nav::head(&e),
            "click on painted indicator row (row 8) must NOT dispatch move_right — caret must not advance");
    }

    /// Fable regression 2 — click ONE ROW BELOW the painted dropdown must NOT dispatch.
    ///
    /// Row 9 is below the 8-row painted dropdown.  Before the fix the mouse path's
    /// hit-test used a 9-row drop_rect (full-height area) and dispatched leaf 8
    /// (abs = scroll_top + 8 = 8 → move_right → caret 0→1).  After the fix the
    /// hit-test uses the same 8-row rect as the painter and returns None → close.
    #[test]
    fn menu_click_below_painted_dropdown_does_not_dispatch() {
        let (mut e, reg, km, ex, clk, tx, _term) = fable_menu_setup();
        let caret_before = crate::nav::head(&e);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1, row: 9, // row 9 = one below the 8-row painted dropdown
            modifiers: KeyModifiers::NONE,
        };
        handle(&mut e, click, &reg, &km, &ex, &clk, &tx);
        assert_eq!(caret_before, crate::nav::head(&e),
            "click below painted dropdown (row 9) must NOT dispatch move_right — caret must not advance");
    }

    // -----------------------------------------------------------------------
    // A13 Task 5.1: minibuffer click → caret
    // -----------------------------------------------------------------------

    /// Open a Filter minibuffer with the given prompt/text/cursor on an 80x24 editor.
    fn open_minibuffer(e: &mut Editor, prompt: &str, text: &str, cursor: usize) {
        e.minibuffer = Some(crate::minibuffer::Minibuffer {
            prompt: prompt.into(),
            text: text.into(),
            cursor,
            kind: crate::minibuffer::MinibufferKind::Filter,
        });
    }

    /// Clicking inside the minibuffer's text on the status row positions the caret
    /// at the exact byte offset of the clicked char — multibyte-safe (é is 2 bytes).
    /// "sh> " prompt = 4 char-columns; text "éxx" → char-col 1 ('x') = byte 2.
    #[test]
    fn minibuffer_click_positions_caret() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        open_minibuffer(&mut e, "sh> ", "\u{e9}xx", 0);
        let (reg, ex, clk, tx, km) = ctx();
        let (_w, h) = e.active().view.area;
        let status_row = h - 1;
        handle(&mut e, down(4 + 1, status_row), &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.minibuffer.as_ref().unwrap().cursor, '\u{e9}'.len_utf8(),
            "click at char-col 1 must land on the byte boundary AFTER the multibyte 'é'");
        assert!(e.minibuffer.is_some(), "minibuffer stays open");
    }

    /// A click past the end of the text clamps the caret to `text.len()` — never
    /// panics on an out-of-range char index.
    #[test]
    fn minibuffer_click_past_end_clamps() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        open_minibuffer(&mut e, "sh> ", "\u{e9}xx", 0);
        let (reg, ex, clk, tx, km) = ctx();
        let (_w, h) = e.active().view.area;
        let status_row = h - 1;
        handle(&mut e, down(4 + 50, status_row), &reg, &km, &ex, &clk, &tx); // far past the text
        let mb = e.minibuffer.as_ref().unwrap();
        assert_eq!(mb.cursor, mb.text.len(), "click past end clamps to text.len()");
    }

    /// A click on the prompt itself (before `prompt_cols`) is a consumed no-op —
    /// caret unchanged, minibuffer stays open.
    #[test]
    fn minibuffer_click_prompt_is_noop() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        open_minibuffer(&mut e, "sh> ", "\u{e9}xx", 2);
        let (reg, ex, clk, tx, km) = ctx();
        let (_w, h) = e.active().view.area;
        let status_row = h - 1;
        handle(&mut e, down(1, status_row), &reg, &km, &ex, &clk, &tx); // col 1 < prompt_cols (4)
        assert_eq!(e.minibuffer.as_ref().unwrap().cursor, 2, "click on the prompt must not move the caret");
        assert!(e.minibuffer.is_some(), "minibuffer stays open");
    }

    // -----------------------------------------------------------------------
    // A13 Task 5.2: search overlay click (field focus + match click)
    // -----------------------------------------------------------------------

    /// Clicking inside the needle field on the status row focuses `Field::Needle`
    /// and positions the char-count-mapped byte cursor; the overlay stays open.
    #[test]
    fn search_needle_click_focuses_and_positions() {
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "wor".chars() { e.search.as_mut().unwrap().insert(c); }
        // reset focus so the click is what moves it, not the prior insert loop
        e.search.as_mut().unwrap().field = crate::search_overlay::Field::Needle;
        e.search.as_mut().unwrap().cursor = 0;
        let (reg, ex, clk, tx, km) = ctx();
        let status_row = e.active().view.area.1 - 1;
        // "Find: " = 6 cols; col 8 → char idx 2 within "wor" ('w','o' consumed).
        let d = down(8, status_row);
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        let s = e.search.as_ref().unwrap();
        assert_eq!(s.field, crate::search_overlay::Field::Needle);
        assert_eq!(s.cursor, 2, "cursor lands at the char-mapped byte offset within the needle");
        assert!(e.search.is_some(), "search overlay stays open");
    }

    /// In `Phase::Replace`, clicking inside the template field (after `"Find:
    /// {needle}  Replace: "`) focuses `Field::Template`, even when the needle
    /// field was previously focused.
    #[test]
    fn search_template_click_in_replace_phase() {
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_search(crate::search_overlay::Phase::Replace, 0);
        for c in "wor".chars() { e.search.as_mut().unwrap().insert(c); }
        e.search.as_mut().unwrap().field = crate::search_overlay::Field::Template;
        e.search.as_mut().unwrap().cursor = 0;
        for c in "cat".chars() { e.search.as_mut().unwrap().insert(c); }
        // Focus back on Needle before the click — proves the click MOVES focus.
        e.search.as_mut().unwrap().field = crate::search_overlay::Field::Needle;
        e.search.as_mut().unwrap().cursor = 0;
        let (reg, ex, clk, tx, km) = ctx();
        let status_row = e.active().view.area.1 - 1;
        let prefix = "Find: wor  Replace: ".chars().count() as u16;
        let d = down(prefix + 2, status_row); // char idx 2 within "cat" ('c','a' consumed)
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        let s = e.search.as_ref().unwrap();
        assert_eq!(s.field, crate::search_overlay::Field::Template);
        assert_eq!(s.cursor, 2);
    }

    /// Clicking a highlighted match in the buffer body selects it (current match
    /// + selection == the clicked match's range) and the overlay STAYS OPEN.
    #[test]
    fn search_match_click_selects_that_match_stays_open() {
        // line0 = "foo abc bar\n" (bytes 0..12, "abc" at 4..7)
        // line1 = "baz abc qux\n" (bytes 12..24, "abc" at 16..19)
        let text = "foo abc bar\nbaz abc qux\n";
        let mut e = Editor::new_from_text(text, None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "abc".chars() { e.search.as_mut().unwrap().insert(c); }
        let (rope, version) = { let d = &e.active().document; (d.buffer.snapshot(), d.version) };
        e.search.as_mut().unwrap().recompute(&rope, version);
        assert_eq!(e.search.as_ref().unwrap().count(), 2, "precondition: two matches");

        let (reg, ex, clk, tx, km) = ctx();
        // row1 col5 sits inside the second "abc" (16..19: col4..7 on line1).
        let d = down(5, 1);
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);

        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (16, 19), "selection == clicked match's range");
        assert_eq!(e.search.as_ref().unwrap().current_ordinal(), Some(2), "current match is the clicked one");
        assert!(e.search.is_some(), "search overlay stays open after a match click");
    }

    /// Regression (spec §5.3): an async edit (mirroring `jobs_apply::apply_filter_done`
    /// — mutate + rebuild + ensure_visible, but NEVER touching the search cache) can
    /// land while search stays open, leaving `editor.search`'s cached match offsets
    /// stale relative to the live buffer/version. The match-click path's step-1
    /// cache-only refresh (`SearchState::recompute`, NOT `search_ui::search_sync`)
    /// must run BEFORE the click is mapped to a byte offset, so the FRESH post-edit
    /// match is selected — not a stale one, and not silently dropped.
    #[test]
    fn search_match_click_refreshes_stale_cache() {
        let text = "foo abc bar\nbaz abc qux\n";
        let mut e = Editor::new_from_text(text, None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "abc".chars() { e.search.as_mut().unwrap().insert(c); }
        let (rope, version) = { let d = &e.active().document; (d.buffer.snapshot(), d.version) };
        e.search.as_mut().unwrap().recompute(&rope, version);
        assert_eq!(e.search.as_ref().unwrap().count(), 2, "precondition: two matches");

        let (reg, ex, clk, tx, km) = ctx();

        // Async edit: insert "XX" at byte 0 (line0 only) — shifts line1's absolute
        // byte offsets +2 while its ON-SCREEN column position is untouched. Apply
        // directly + rebuild + ensure_visible (mirrors apply_filter_done) WITHOUT
        // touching editor.search — the cache is now stale by construction.
        let doc_len = e.active().document.buffer.len();
        let (cs, edit) = crate::commands::build_range_replace(0, 0, "XX", doc_len);
        let txn = wordcartel_core::history::Transaction::new(cs)
            .with_selection(wordcartel_core::selection::Selection::single(0));
        // H24: outcome dropped — this test asserts on the stale-cache precondition below, not the outcome.
        let _ = e.apply(txn, edit, wordcartel_core::history::EditKind::Other, &clk);
        crate::derive::rebuild(&mut e);
        crate::nav::ensure_visible(&mut e);

        // Precondition: the cache still holds the PRE-edit offsets.
        assert_eq!(e.search.as_ref().unwrap().matches(),
            &[wordcartel_core::search::Match { start: 4, end: 7 },
              wordcartel_core::search::Match { start: 16, end: 19 }],
            "precondition: cache is stale relative to the post-edit buffer/version");

        // Click the second "abc" at its CURRENT screen position (row1, unchanged —
        // only line0 was edited); the LIVE (post-edit) match is now 18..21.
        let d = down(6, 1);
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);

        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (18, 21),
            "match-click must select against FRESH post-edit offsets, not the stale 16..19");
        assert_eq!(e.search.as_ref().unwrap().current_ordinal(), Some(2));
        assert!(e.search.is_some(), "overlay stays open");

        // Control: a click that lands on NO match leaves selection untouched.
        let before = e.active().document.selection.clone();
        let miss = down(0, 0); // 'X' of the inserted "XX" prefix — not inside any match
        handle(&mut e, miss, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.active().document.selection, before, "non-match click leaves selection unchanged");
    }

    // -----------------------------------------------------------------------
    // A21 Task 2: hover + through-list wheel on the four side-effect-free slots
    // -----------------------------------------------------------------------

    fn wheel_ev(down_dir: bool, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: if down_dir { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp },
            column: col, row, modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn palette_hover_moves_highlight_to_pointer_row() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.palette = Some(crate::palette::Palette::default());
        let (reg, ex, clk, tx, km) = ctx();
        crate::app::hydrate_overlays(&mut e, &reg, &km);
        assert_eq!(e.palette.as_ref().unwrap().scroll_top, 0);
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, e.palette.as_ref().unwrap().rows.len());
        // Hover the 4th visible list row (list starts at rect.y + 2).
        handle(&mut e, moved(rect.x + 1, rect.y + 2 + 3), &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.palette.as_ref().unwrap().selected, 3, "hover set highlight to the pointer row");
    }

    #[test]
    fn palette_hover_off_rect_leaves_highlight() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.palette = Some(crate::palette::Palette::default());
        let (reg, ex, clk, tx, km) = ctx();
        crate::app::hydrate_overlays(&mut e, &reg, &km);
        e.palette.as_mut().unwrap().selected = 2; // a keyboard-set highlight
        handle(&mut e, moved(0, 0), &reg, &km, &ex, &clk, &tx); // top-left, off the overlay
        assert_eq!(e.palette.as_ref().unwrap().selected, 2, "off-rect hover leaves the highlight as-is");
    }

    #[test]
    fn palette_wheel_scrolls_and_re_hovers() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &crate::registry::Registry::builtins(),
            &{ let (_r, _e2, _c, _t, km) = ctx(); km });
        e.palette = Some(p);
        let (reg, ex, clk, tx, km) = ctx();
        let n = e.palette.as_ref().unwrap().rows.len();
        let list_h = crate::list_window::list_h_for(n, 24);
        assert!(n > list_h, "precondition: palette overflows its window");
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, n);
        // Wheel down with the pointer over the top visible row → scroll by 3, re-hover pins the
        // highlight to that top row (absolute row scroll_top).
        handle(&mut e, wheel_ev(true, rect.x + 1, rect.y + 2), &reg, &km, &ex, &clk, &tx);
        let p = e.palette.as_ref().unwrap();
        assert_eq!(p.scroll_top, 3, "wheel scrolled the viewport by WHEEL_STEP");
        assert_eq!(p.selected, p.scroll_top, "re-hover pinned the highlight to the pointer's top row");
    }

    #[test]
    fn palette_wheel_empty_list_is_total_noop() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let mut p = crate::palette::Palette {
            query: "zzz_no_such_command_zzz".into(), // filter to zero rows
            ..crate::palette::Palette::default()
        };
        crate::palette::rebuild_rows(&mut p, &crate::registry::Registry::builtins(),
            &{ let (_r, _e2, _c, _t, km) = ctx(); km });
        assert!(p.rows.is_empty(), "precondition: zero rows");
        p.scroll_top = 0; p.selected = 0;
        e.palette = Some(p);
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, wheel_ev(true, 40, 12), &reg, &km, &ex, &clk, &tx);
        handle(&mut e, wheel_ev(false, 40, 12), &reg, &km, &ex, &clk, &tx);
        let p = e.palette.as_ref().unwrap();
        assert_eq!((p.selected, p.scroll_top), (0, 0), "empty-list wheel is a total no-op (I3b)");
    }

    #[test]
    fn outline_hover_does_not_jump() {
        let doc = "# A\n\ntext\n\n# B\n\nmore\n";
        let mut e = Editor::new_from_text(doc, None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_outline();
        let (reg, ex, clk, tx, km) = ctx();
        let scroll_before = e.active().view.scroll;
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let n = e.outline.as_ref().unwrap().rows.len();
        assert!(n >= 2, "precondition: two headings");
        let rect = crate::chrome_geom::palette_overlay_rect(area, n);
        handle(&mut e, moved(rect.x + 1, rect.y + 2 + 1), &reg, &km, &ex, &clk, &tx);
        assert!(e.outline.is_some(), "hover keeps the outline open (no jump)");
        assert_eq!(e.outline.as_ref().unwrap().selected, 1, "hover moved the highlight");
        assert_eq!(e.active().view.scroll, scroll_before, "hover did NOT jump the document");
    }

    #[test]
    fn diag_hover_does_not_apply() {
        let mut e = Editor::new_from_text("helo world\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let d = wordcartel_core::diagnostics::Diagnostic {
            range: 0..4, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "spelling".into(),
            suggestions: vec![wordcartel_core::diagnostics::Suggestion::ReplaceWith("hello".into())],
        };
        e.open_diag(d);
        let (reg, ex, clk, tx, km) = ctx();
        let v0 = e.active().document.version;
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let n = e.diag.as_ref().unwrap().row_count();
        let rect = crate::chrome_geom::palette_overlay_rect(area, n);
        // diag list starts at rect.y + 1 (no query row).
        handle(&mut e, moved(rect.x + 1, rect.y + 1 + 1), &reg, &km, &ex, &clk, &tx);
        assert!(e.diag.is_some(), "hover keeps the diag overlay open (no apply)");
        assert_eq!(e.active().document.version, v0, "hover did NOT apply a fix (buffer unchanged)");
    }

    // -----------------------------------------------------------------------
    // A21 Task 3: menu dropdown hover, effective-budget wheel, bar hover-to-switch
    // -----------------------------------------------------------------------

    /// Helper: open a real built menu on category 0 (File), hydrated.
    fn open_menu(e: &mut Editor, reg: &crate::registry::Registry, km: &crate::keymap::KeyTrie) {
        e.menu = Some(crate::menu::empty_at(0));
        crate::app::hydrate_overlays(e, reg, km);
    }

    #[test]
    fn menu_hover_bar_switches_category_with_reset_triple() {
        let mut e = Editor::new_from_text("hi\n", None, (100, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        open_menu(&mut e, &reg, &km);
        // Move the highlight/scroll off zero so we can prove the reset.
        { let m = e.menu.as_mut().unwrap(); m.highlighted = 2; m.scroll_top = 1; }
        let open0 = e.menu.as_ref().unwrap().open;
        let area = ratatui::layout::Rect::new(0, 0, 100, 24);
        let hit_area = crate::chrome_geom::menu_area(area);
        let groups = e.menu.as_ref().unwrap().groups.clone();
        // Find a DIFFERENT category's bar label rect.
        let bar = crate::chrome_geom::menu_bar_layout(hit_area, &groups);
        let (other_cat, other_rect) = bar.iter().find(|(c, _)| *c != open0).copied()
            .expect("a second category exists");
        handle(&mut e, moved(other_rect.x, other_rect.y), &reg, &km, &ex, &clk, &tx);
        let m = e.menu.as_ref().unwrap();
        assert_eq!(m.open, other_cat, "hover onto a different bar label switched the open category");
        assert_eq!((m.highlighted, m.scroll_top), (0, 0), "switch reset the highlight + scroll (triple)");
    }

    #[test]
    fn menu_hover_same_category_does_not_reset() {
        let mut e = Editor::new_from_text("hi\n", None, (100, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        open_menu(&mut e, &reg, &km);
        { let m = e.menu.as_mut().unwrap(); m.highlighted = 2; m.scroll_top = 0; }
        let open0 = e.menu.as_ref().unwrap().open;
        let area = ratatui::layout::Rect::new(0, 0, 100, 24);
        let hit_area = crate::chrome_geom::menu_area(area);
        let groups = e.menu.as_ref().unwrap().groups.clone();
        let bar = crate::chrome_geom::menu_bar_layout(hit_area, &groups);
        let (_, own_rect) = bar.iter().find(|(c, _)| *c == open0).copied().unwrap();
        handle(&mut e, moved(own_rect.x, own_rect.y), &reg, &km, &ex, &clk, &tx);
        let m = e.menu.as_ref().unwrap();
        assert_eq!(m.open, open0, "hover on the SAME open label keeps the category");
        assert_eq!(m.highlighted, 2, "cat == open dedupe: no reset of the highlight");
    }

    #[test]
    fn menu_hover_dropdown_row_sets_highlight() {
        let mut e = Editor::new_from_text("hi\n", None, (100, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        open_menu(&mut e, &reg, &km);
        let area = ratatui::layout::Rect::new(0, 0, 100, 24);
        let hit_area = crate::chrome_geom::menu_area(area);
        let (open, scroll_top) = { let m = e.menu.as_ref().unwrap(); (m.open, m.scroll_top) };
        let groups = e.menu.as_ref().unwrap().groups.clone();
        // Only run the row assertion if the open category has ≥ 2 rows.
        if groups.get(open).map(|g| g.1.len()).unwrap_or(0) >= 2 {
            let drop = crate::chrome_geom::menu_dropdown_rect(hit_area, &groups, open).unwrap();
            handle(&mut e, moved(drop.x, drop.y + 1), &reg, &km, &ex, &clk, &tx);
            let want = crate::chrome_geom::menu_dropdown_row_at(hit_area, &groups, open, scroll_top, drop.x, drop.y + 1).unwrap();
            assert_eq!(e.menu.as_ref().unwrap().highlighted, want, "dropdown hover set highlighted to the pointer row");
        }
    }

    #[test]
    fn menu_hover_bar_with_no_menu_open_does_not_open() {
        let mut e = Editor::new_from_text("hi\n", None, (100, 24));
        crate::derive::rebuild(&mut e);
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        let (reg, ex, clk, tx, km) = ctx();
        // No overlay open → the event routes to the DWELL path, not the menu slot.
        handle(&mut e, moved(2, 0), &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_none(), "first-open stays deliberate: bar hover with no menu open does not auto-open");
    }

    /// Build a menu opened on ONE category (Edit) with `n` synthetic leaves — the mouse-test
    /// analogue of chrome_geom's `tall_menu_groups`. `built: true` so hydrate leaves it alone.
    fn tall_menu(n: usize) -> crate::menu::MenuView {
        let leaves: Vec<(String, crate::menu::MenuRowAction)> = (0..n)
            .map(|i| (format!("item{i}"),
                crate::menu::MenuRowAction::Command(crate::registry::CommandId("move_right"))))
            .collect();
        crate::menu::MenuView {
            groups: vec![(crate::registry::MenuCategory::Edit, leaves)],
            open: 0, highlighted: 0, built: true, scroll_top: 0,
        }
    }

    #[test]
    fn menu_wheel_tall_category_scrolls_without_landing_on_indicator_row() {
        // 100×8 terminal: menu_area.height = 7, raw_window = 20.min(15).min(6) = 6, overflow →
        // effective budget = 5, item_rows = 5, indicator row reserved at the dropdown bottom.
        let mut e = Editor::new_from_text("hi\n", None, (100, 8));
        crate::derive::rebuild(&mut e);
        e.menu = Some(tall_menu(20));
        let (reg, ex, clk, tx, km) = ctx();
        let area = ratatui::layout::Rect::new(0, 0, 100, 8);
        let hit_area = crate::chrome_geom::menu_area(area);
        let groups = e.menu.as_ref().unwrap().groups.clone();
        let drop = crate::chrome_geom::menu_dropdown_rect(hit_area, &groups, 0).expect("dropdown rect");
        assert_eq!(drop.height, 6, "raw dropdown window is 6 (min(20,15,6))");
        let item_rows = drop.height as usize - 1; // = 5 (overflow reserves the indicator row)
        // Wheel down several notches with the pointer OFF the dropdown (re-hover finds nothing;
        // the highlight is driven by the wheel's clamp — the fragile path).
        for _ in 0..3 { handle(&mut e, wheel_ev(true, 0, 7), &reg, &km, &ex, &clk, &tx); }
        let (st, hl) = { let m = e.menu.as_ref().unwrap(); (m.scroll_top, m.highlighted) };
        assert!(st > 0, "tall category scrolled the dropdown viewport");
        assert!(hl >= st && hl < st + item_rows,
            "highlight stays within the item window [{st}, {}), never the reserved indicator row", st + item_rows);
        // Ground it against the REAL hit-tester: the indicator row is not a dispatchable item.
        let indicator_row = drop.y + drop.height - 1;
        assert_eq!(
            crate::chrome_geom::menu_dropdown_row_at(hit_area, &groups, 0, st, drop.x, indicator_row),
            None, "the reserved indicator row returns None (never a hidden dispatch)");
    }

    #[test]
    fn menu_wheel_short_category_steps_by_one() {
        // A 3-leaf category on a tall terminal fits entirely (no overflow) → wheel STEPS ±1 (2ii).
        let mut e = Editor::new_from_text("hi\n", None, (100, 24));
        crate::derive::rebuild(&mut e);
        e.menu = Some(tall_menu(3));
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, wheel_ev(true, 0, 5), &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.menu.as_ref().unwrap().highlighted, 1, "short category: wheel down steps the highlight to 1");
        assert_eq!(e.menu.as_ref().unwrap().scroll_top, 0, "short category does not scroll");
        handle(&mut e, wheel_ev(false, 0, 5), &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.menu.as_ref().unwrap().highlighted, 0, "wheel up steps back to 0");
    }
}
