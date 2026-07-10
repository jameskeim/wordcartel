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

/// True when NO overlay/modal is open — the shared predicate for dwell suppression.
fn no_overlay_open(editor: &Editor) -> bool {
    editor.menu.is_none() && editor.palette.is_none() && editor.theme_picker.is_none()
        && editor.file_browser.is_none() && editor.outline.is_none() && editor.diag.is_none()
        && editor.prompt.is_none() && editor.minibuffer.is_none() && editor.search.is_none()
}

/// Route a mouse event to the open overlay layer. PRECONDITION: at least one overlay
/// is open (`!no_overlay_open`). Consumes the event (the caller returns unconditionally
/// after this). Text-input modals (minibuffer/search/prompt for non-choice clicks)
/// consume without acting; list overlays scroll/click/click-away (Tasks 10-13).
// 8 args mirror `handle`'s dispatch context (reg/keymap/ex/clock/msg_tx) plus the
// precomputed overlay `area`; an args-struct would just duplicate `handle`'s params.
#[allow(clippy::too_many_arguments)]
fn route_overlay(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
                 reg: &crate::registry::Registry, keymap: &crate::keymap::KeyTrie,
                 ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
                 msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
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
        if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
            if let Some(m) = editor.menu.as_mut() {
                let n = m.groups.get(m.open).map(|g| g.1.len()).unwrap_or(0);
                if n > 0 {
                    if matches!(ev.kind, MouseEventKind::ScrollDown) {
                        m.highlighted = (m.highlighted + 1).min(n - 1);
                    } else {
                        m.highlighted = m.highlighted.saturating_sub(1);
                    }
                    // Coarse follow-the-selection layer — the paint re-windows against the true
                    // item-row budget every frame (list_window two-layer invariant), so this
                    // estimate need not reserve the indicator row.  Derive from menu_area so
                    // keep_visible scrolls at the same boundary the dropdown rect and painter use.
                    let avail_below = crate::chrome_geom::menu_area(area).height.saturating_sub(1) as usize;
                    let list_h = n.min(15).min(avail_below);
                    crate::list_window::keep_visible(m.highlighted, n, list_h, &mut m.scroll_top);
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
            let row_id: Option<crate::registry::CommandId> = {
                let groups = &editor.menu.as_ref().unwrap().groups;
                crate::chrome_geom::menu_dropdown_row_at(hit_area, groups, open, scroll_top, ev.column, ev.row)
                    .and_then(|row| groups.get(open).and_then(|g| g.1.get(row)).map(|(_, id)| *id))
            };
            // all borrows dropped — now mutate/dispatch/clear
            if let Some(cat) = bar_hit {
                // category switch — reset scroll_top so stale window never carries into shorter category
                let m = editor.menu.as_mut().unwrap();
                m.open = cat; m.highlighted = 0; m.scroll_top = 0;
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
            crate::theme_cmds::preview_selected_theme(editor);
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
        return;
    }
    if editor.outline.is_some() {
        if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
            let ah = editor.active().view.area.1;
            if let Some(o) = editor.outline.as_mut() {
                if matches!(ev.kind, MouseEventKind::ScrollDown) {
                    o.selected = (o.selected + 1).min(o.rows.len().saturating_sub(1));
                } else {
                    o.selected = o.selected.saturating_sub(1);
                }
                crate::app::keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
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
                    editor.status = "document changed; outline closed".into();
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
        return;
    }
    if editor.diag.is_some() {
        if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
            let ah = editor.active().view.area.1;
            if let Some(d) = editor.diag.as_mut() {
                let rc = d.row_count();
                if matches!(ev.kind, MouseEventKind::ScrollDown) {
                    d.selected = (d.selected + 1).min(rc.saturating_sub(1));
                } else {
                    d.selected = d.selected.saturating_sub(1);
                }
                crate::app::keep_overlay_visible(ah, d.selected, rc, &mut d.scroll_top);
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
                crate::search_ui::diag_apply_selected(editor, clock);
            } else if !inside {
                editor.diag = None; // click-away closes
            }
        }
        return;
    }
    // Task 13: prompt choice clicks — on Down(Left) over a `[K]` marker, dispatch
    // via the shared keyboard resolver; all other events (including off-marker
    // clicks) are consumed so the prompt stays open and nothing leaks to the editor.
    if editor.prompt.is_some() {
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            // Scoped borrow → owned action (PromptAction: Copy) before mutable dispatch.
            let action: Option<crate::prompt::PromptAction> = editor.prompt.as_ref()
                .and_then(|p| crate::chrome_geom::prompt_choice_at(area, p, ev.column, ev.row));
            if let Some(action) = action {
                // resolve_prompt clears editor.prompt in its arms — do NOT clear it here.
                crate::prompts::resolve_prompt(action, editor, ex, clock, msg_tx);
            }
        }
        return;
    }
    // Text-input modals: consume, no row action (you type). Tail branch — the fn
    // ends here, so an empty body suffices (no `return` needed).
    if editor.minibuffer.is_some() || editor.search.is_some() {}
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
    // Overlay routing runs BEFORE the dwell-arming block: while any modal is open
    // the event is consumed by route_overlay and nothing leaks to the dwell timers
    // or the editor. The dwell block + editor match below therefore run ONLY when
    // no overlay is open.
    let (w, h) = editor.active().view.area;
    let area = ratatui::layout::Rect::new(0, 0, w, h);
    if !no_overlay_open(editor) {
        route_overlay(editor, ev, area, reg, keymap, ex, clock, msg_tx);
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
        let bar = crate::chrome_geom::menu_bar_layout_cats(menu_area, &crate::registry::MENU_ORDER);
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
        let rect = crate::chrome_geom::palette_overlay_rect(area, 2);
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

    /// A wheel event on the outline moves `selected` and keeps the window visible.
    #[test]
    fn outline_wheel_scroll_moves_selection() {
        let text: String = (0..20).map(|i| format!("# H{i}\n\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_outline();
        let (reg, ex, clk, tx, km) = ctx();
        let scroll_down = MouseEvent {
            kind: MouseEventKind::ScrollDown, column: 40, row: 10,
            modifiers: KeyModifiers::NONE,
        };
        for _ in 0..10 {
            handle(&mut e, scroll_down, &reg, &km, &ex, &clk, &tx);
        }
        let o = e.outline.as_ref().expect("outline must remain open after wheel");
        assert_eq!(o.selected, 10, "selected must be 10 after 10 scroll-downs");
        let lh = crate::list_window::list_h_for(o.rows.len(), 24);
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
        let leaves: Vec<(String, crate::registry::CommandId)> =
            (0..20).map(|i| (format!("item{i}"), crate::registry::CommandId("move_right"))).collect();
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

        let leaves: Vec<(String, crate::registry::CommandId)> =
            (0..9).map(|i| (format!("item{i:02}      "), crate::registry::CommandId("move_right"))).collect();
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
}
