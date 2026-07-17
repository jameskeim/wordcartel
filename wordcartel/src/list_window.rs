//! Windowed-list helpers for overlay lists (palette, outline, theme picker,
//! file browser — A6). Two layers keep the "selection is always visible"
//! invariant: key/mouse handlers call `keep_visible` after every selection
//! change (the window follows the SELECTION); each render painter calls it
//! again with the live frame's `list_h` (the window respects the GEOMETRY —
//! survives resize without an event hook).

/// Visible row budget for a windowed overlay list — the single source of the
/// min(rows, 15, h-4) computation (previously duplicated by
/// `palette_overlay_rect` and `palette_row_at`).
pub(crate) fn list_h_for(row_count: usize, area_h: u16) -> usize {
    row_count.min(15).min(area_h.saturating_sub(4) as usize)
}

/// Slide the window so `selected` is visible: on exit (for `list_h > 0`),
/// `selected ∈ [scroll_top, scroll_top + list_h)` and
/// `scroll_top <= row_count.saturating_sub(list_h)` (no over-scroll after a
/// shrink). `list_h == 0` (degenerate terminal) resets the window to 0.
pub(crate) fn keep_visible(selected: usize, row_count: usize, list_h: usize, scroll_top: &mut usize) {
    if list_h == 0 {
        *scroll_top = 0;
        return;
    }
    if selected < *scroll_top {
        *scroll_top = selected;
    } else if selected >= *scroll_top + list_h {
        *scroll_top = selected + 1 - list_h;
    }
    *scroll_top = (*scroll_top).min(row_count.saturating_sub(list_h));
}

/// Wheel step: rows the viewport slides per notch, uniform across overlays (A21).
#[allow(dead_code)] // wired in Task 2
pub(crate) const WHEEL_STEP: usize = 3;

/// Wheel notch → viewport scroll. `list_h` is the caller's effective ITEM-ROW budget (the menu
/// passes its overflow-adjusted value, not the raw dropdown height). Slide `scroll_top` by
/// ±WHEEL_STEP, clamped to `[0, max_top]` where `max_top == 0` when `list_h == 0` (nothing
/// renders) and `row_count.saturating_sub(list_h)` otherwise — the SAME bound `keep_visible`
/// enforces, including its `list_h == 0 → scroll_top = 0` reset, so wheel state never diverges
/// from the per-frame painter at any window height. Selection is untouched; the caller drags the
/// highlight in via `clamp_into_window`, then re-hovers. All-saturating: no height can panic.
#[allow(dead_code)] // wired in Task 2
pub(crate) fn wheel_scroll(down: bool, row_count: usize, list_h: usize, scroll_top: &mut usize) {
    if list_h == 0 {
        *scroll_top = 0;
        return;
    }
    let max_top = row_count.saturating_sub(list_h);
    *scroll_top = if down { scroll_top.saturating_add(WHEEL_STEP).min(max_top) }
                  else { scroll_top.saturating_sub(WHEEL_STEP) };
}

/// Pull `highlight` into the visible item window `[scroll_top, scroll_top + list_h)` after a
/// wheel scroll (ruling 1a) — an active wheel gesture drags the highlight to the window edge;
/// a no-op when it is already inside. `list_h` is the effective item budget (menu =
/// overflow-adjusted), so the highlight can never land on the menu's reserved indicator row.
/// `row_count == 0` or `list_h == 0` (empty window, nothing renders): leave `highlight`
/// untouched — the position is moot until a real window exists, and this avoids the `list_h - 1`
/// underflow. All arithmetic saturating.
#[allow(dead_code)] // wired in Task 2
pub(crate) fn clamp_into_window(highlight: &mut usize, scroll_top: usize, list_h: usize, row_count: usize) {
    if row_count == 0 || list_h == 0 { return; }
    let last = row_count - 1;
    let lo = scroll_top.min(last);
    let hi = scroll_top.saturating_add(list_h - 1).min(last);
    *highlight = (*highlight).clamp(lo, hi);
}

/// One wheel notch over a windowed list — the spec §5 branch, factored so the seven overlay
/// slots do not each repeat it. Empty list (`row_count == 0`) is a total no-op (returns false).
/// Short list (`row_count <= list_h`, nothing to scroll) steps `selected` ±1 (returns false).
/// Long list scrolls the viewport by `WHEEL_STEP` then drags `selected` into the new window
/// (returns `true` — the caller then re-hovers at the pointer, which overrides). `list_h` is the
/// caller's effective item budget; name-agnostic via `&mut usize` (the menu passes `highlighted`,
/// the others `selected`). Pure — the caller owns keep-visible and any per-overlay side effect.
#[allow(dead_code)] // wired in Task 2
pub(crate) fn wheel_list(down: bool, row_count: usize, list_h: usize,
    selected: &mut usize, scroll_top: &mut usize) -> bool {
    if row_count == 0 { return false; }
    if row_count <= list_h {
        *selected = if down { selected.saturating_add(1).min(row_count.saturating_sub(1)) }
                    else { selected.saturating_sub(1) };
        false
    } else {
        wheel_scroll(down, row_count, list_h, scroll_top);
        clamp_into_window(selected, *scroll_top, list_h, row_count);
        true
    }
}

/// The six list-motion keys shared by every windowed overlay (palette, theme
/// picker, file browser, outline) — menu and diag are excluded (different nav
/// semantics; T10).
pub(crate) enum ListNav { Up, Down, PageUp, PageDown, Home, End }

/// Classify a key code as a list-nav motion, or `None` if it isn't one.
pub(crate) fn list_nav_key(code: crossterm::event::KeyCode) -> Option<ListNav> {
    use crossterm::event::KeyCode;
    match code {
        KeyCode::Up => Some(ListNav::Up), KeyCode::Down => Some(ListNav::Down),
        KeyCode::PageUp => Some(ListNav::PageUp), KeyCode::PageDown => Some(ListNav::PageDown),
        KeyCode::Home => Some(ListNav::Home), KeyCode::End => Some(ListNav::End),
        _ => None,
    }
}

/// Apply a motion to `(selected, scroll_top)` over `row_count` rows in an
/// `area_h`-tall buffer area — the exact math of the four duplicated overlay
/// blocks this replaces. Per-overlay SIDE EFFECTS (theme-preview, outline
/// re-query, etc.) stay in the caller, outside this pure helper.
pub(crate) fn apply_list_nav(nav: ListNav, area_h: u16, row_count: usize,
    selected: &mut usize, scroll_top: &mut usize) {
    let lh = list_h_for(row_count, area_h);
    match nav {
        ListNav::Up => *selected = selected.saturating_sub(1),
        ListNav::Down => *selected = (*selected + 1).min(row_count.saturating_sub(1)),
        ListNav::PageDown => *selected = (*selected + lh.max(1)).min(row_count.saturating_sub(1)),
        ListNav::PageUp => *selected = selected.saturating_sub(lh.max(1)),
        ListNav::Home => *selected = 0,
        ListNav::End => *selected = row_count.saturating_sub(1),
    }
    keep_visible(*selected, row_count, lh, scroll_top);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_h_for_takes_the_three_way_min() {
        assert_eq!(list_h_for(110, 24), 15, "cap wins");
        assert_eq!(list_h_for(3, 24), 3, "row count wins");
        assert_eq!(list_h_for(110, 10), 6, "terminal wins (h-4)");
        assert_eq!(list_h_for(110, 4), 0, "degenerate");
    }

    #[test]
    fn keep_visible_window_follows_selection() {
        let mut top = 0;
        keep_visible(20, 110, 15, &mut top);
        assert_eq!(top, 6, "below the window: selected becomes the last visible row");
        keep_visible(3, 110, 15, &mut top);
        assert_eq!(top, 3, "above the window: selected becomes the first visible row");
        keep_visible(10, 110, 15, &mut top);
        assert_eq!(top, 3, "inside the window: no movement");
        keep_visible(109, 110, 15, &mut top);
        assert_eq!(top, 95, "End lands the window on the tail");
    }

    #[test]
    fn keep_visible_reclamps_after_shrink_and_degenerate() {
        let mut top = 95;
        keep_visible(2, 3, 15, &mut top);
        assert_eq!(top, 0, "filter shrink: over-scroll clamped away");
        let mut top = 5;
        keep_visible(50, 110, 0, &mut top);
        assert_eq!(top, 0, "list_h == 0 resets the window");
        let mut top = 0;
        keep_visible(0, 0, 15, &mut top);
        assert_eq!(top, 0, "empty rows: no movement, no underflow");
    }

    #[test]
    fn wheel_scroll_slides_by_step_and_clamps() {
        let mut top = 0;
        wheel_scroll(true, 100, 10, &mut top);
        assert_eq!(top, 3, "one notch down slides by WHEEL_STEP");
        wheel_scroll(true, 100, 10, &mut top);
        assert_eq!(top, 6, "second notch accumulates");
        // clamp at the tail: max_top = 100 - 10 = 90.
        let mut top = 89;
        wheel_scroll(true, 100, 10, &mut top);
        assert_eq!(top, 90, "clamped to row_count - list_h, not 92");
        // up saturates at 0.
        let mut top = 2;
        wheel_scroll(false, 100, 10, &mut top);
        assert_eq!(top, 0, "up saturates at 0 (2 - 3)");
    }

    #[test]
    fn wheel_scroll_list_h_zero_pins_to_zero() {
        let mut top = 7;
        wheel_scroll(true, 5, 0, &mut top);
        assert_eq!(top, 0, "list_h == 0 → max_top 0 → scroll_top 0 (mirrors keep_visible), down");
        let mut top = 7;
        wheel_scroll(false, 5, 0, &mut top);
        assert_eq!(top, 0, "list_h == 0 → 0, up (saturating)");
    }

    #[test]
    fn clamp_into_window_pulls_highlight_to_edge() {
        // window [3, 3+10) = [3,13); a highlight of 1 is below → pulled to 3.
        let mut h = 1;
        clamp_into_window(&mut h, 3, 10, 100);
        assert_eq!(h, 3, "below window → lower edge");
        // a highlight of 50 is above [3,13) → pulled to 12.
        let mut h = 50;
        clamp_into_window(&mut h, 3, 10, 100);
        assert_eq!(h, 12, "above window → upper edge (scroll_top + list_h - 1)");
        // already inside → unchanged.
        let mut h = 7;
        clamp_into_window(&mut h, 3, 10, 100);
        assert_eq!(h, 7, "inside window → no move");
    }

    #[test]
    fn clamp_into_window_degenerate_is_noop_no_underflow() {
        let mut h = 4;
        clamp_into_window(&mut h, 0, 0, 10);
        assert_eq!(h, 4, "list_h == 0 → no-op (empty window; no underflow)");
        let mut h = 4;
        clamp_into_window(&mut h, 0, 5, 0);
        assert_eq!(h, 4, "row_count == 0 → no-op");
    }

    #[test]
    fn wheel_list_short_steps_and_long_scrolls() {
        // short list (row_count <= list_h): steps ±1, returns false (no re-hover).
        let (mut sel, mut top) = (0, 0);
        let scrolled = wheel_list(true, 5, 10, &mut sel, &mut top);
        assert!(!scrolled, "short list does not scroll");
        assert_eq!((sel, top), (1, 0), "short list steps selection down by 1");
        let scrolled = wheel_list(false, 5, 10, &mut sel, &mut top);
        assert!(!scrolled, "short list up does not scroll");
        assert_eq!((sel, top), (0, 0), "short list steps back up");
        // long list (row_count > list_h): scrolls + drags highlight, returns true.
        let (mut sel, mut top) = (0, 0);
        let scrolled = wheel_list(true, 100, 10, &mut sel, &mut top);
        assert!(scrolled, "long list scrolls");
        assert_eq!(top, 3, "scrolled by WHEEL_STEP");
        assert_eq!(sel, 3, "highlight dragged to the window's lower edge");
    }

    #[test]
    fn wheel_list_empty_is_total_noop() {
        let (mut sel, mut top) = (0, 0);
        let scrolled = wheel_list(true, 0, 0, &mut sel, &mut top);
        assert!(!scrolled, "empty list never scrolls");
        assert_eq!((sel, top), (0, 0), "empty list is a total no-op, no underflow");
    }
}
