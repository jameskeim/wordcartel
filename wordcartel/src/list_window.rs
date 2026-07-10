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
}
