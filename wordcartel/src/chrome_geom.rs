//! Extracted verbatim from render.rs (Effort H1 round 2, T11).
//! Shared geometry + hit-testing — render AND mouse both call these.

use ratatui::layout::Rect;

/// Compute bar label rects from a raw category slice (static MENU_ORDER or dynamic group list).
/// Returns `(index_into_cats, rect)` for each category.
pub(crate) fn menu_bar_layout_cats(area: Rect, cats: &[crate::registry::MenuCategory]) -> Vec<(usize, Rect)> {
    let mut out = Vec::new();
    let mut x = area.x;
    for (i, cat) in cats.iter().enumerate() {
        let label = crate::menu::category_label_pub(*cat);
        let wgt = label.chars().count() as u16 + 2; // 1 space padding each side
        out.push((i, Rect::new(x, area.y, wgt, 1)));
        x = x.saturating_add(wgt);
    }
    out
}

/// Compute bar label rects from the built groups list.  Thin wrapper over `menu_bar_layout_cats`.
pub(crate) fn menu_bar_layout(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)]) -> Vec<(usize, Rect)> {
    let cats: Vec<crate::registry::MenuCategory> = groups.iter().map(|g| g.0).collect();
    menu_bar_layout_cats(area, &cats)
}

pub(crate) fn menu_dropdown_rect(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)], open: usize) -> Option<Rect> {
    let bar = menu_bar_layout(area, groups);
    let (_, label_rect) = bar.get(open)?;
    let leaves = &groups.get(open)?.1;
    if leaves.is_empty() { return None; }
    let width = leaves.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0) as u16 + 2;
    let avail_below = area.height.saturating_sub(1) as usize; // rows under the bar
    let list_h = leaves.len().min(15).min(avail_below);
    if list_h == 0 { return None; } // cramped terminal: no room — never paint past the boundary
    Some(Rect::new(label_rect.x, area.y + 1,
        width.min(area.width.saturating_sub(label_rect.x.saturating_sub(area.x))),
        list_h as u16))
}

pub(crate) fn menu_dropdown_row_at(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)], open: usize, scroll_top: usize, col: u16, row: u16) -> Option<usize> {
    let r = menu_dropdown_rect(area, groups, open)?;
    let leaves_len = groups.get(open).map(|g| g.1.len()).unwrap_or(0);
    let list_h = r.height as usize;
    // Mirror the paint's overflows condition exactly (render_overlays.rs):
    // when the category overflows the window the bottom row is reserved for the n/total
    // indicator — a click there must NOT dispatch a hidden off-screen command.
    let overflows = leaves_len > list_h;
    let item_rows = if overflows { list_h.saturating_sub(1) } else { list_h };
    if col >= r.x && col < r.x + r.width && row >= r.y {
        let row_in_window = (row - r.y) as usize;
        if row_in_window < item_rows {
            let abs = scroll_top + row_in_window;
            // Defensive guard — keep_visible clamps scroll_top, but never dispatch a
            // non-existent row if geometry and state are somehow mismatched.
            if abs < leaves_len { Some(abs) } else { None }
        } else { None }
    } else { None }
}

/// The area the menu bar and dropdown are laid out against: the frame area with the
/// reserved status row excluded.  Both the painter (`render_overlays::paint`) and the
/// mouse hit-test path (`mouse::route_overlay`) MUST derive dropdown geometry through
/// this helper — so `avail_below` in `menu_dropdown_rect` evaluates against the same
/// height in both call sites and the two windows can never drift (Fable whole-branch fix).
pub(crate) fn menu_area(area: Rect) -> Rect {
    Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1))
}

/// Indicator title for a windowed overlay list — `" {n}/{total} "` right-aligned when
/// the list is taller than the visible window; `None` when everything fits (A6).
pub(crate) fn windowed_indicator(selected: usize, total: usize, list_h: usize)
    -> Option<ratatui::text::Line<'static>>
{
    if total > list_h {
        Some(ratatui::text::Line::from(format!(" {}/{} ", selected + 1, total)).right_aligned())
    } else {
        None
    }
}

/// Compute the palette overlay bounding rect for a given terminal area and row count.
/// The overlay height is sized to the actual number of palette rows (capped at 15).
/// Both the render code and mouse hit-testing call this to share geometry.
pub(crate) fn palette_overlay_rect(area: Rect, row_count: usize) -> Rect {
    let w = area.width;
    let h = area.height;
    let ov_w = (w * 3 / 5).clamp(30, 80).min(w);
    let list_h: u16 = crate::list_window::list_h_for(row_count, h) as u16;
    let ov_h = (list_h + 3).min(h);
    let ov_x = area.x.saturating_add((w.saturating_sub(ov_w)) / 2);
    let ov_y = area.y.saturating_add((h.saturating_sub(ov_h)) / 4);
    Rect::new(ov_x, ov_y, ov_w, ov_h)
}

/// Return the zero-based list row index that `(col, row)` hits, or `None`.
/// The list starts at `ov_y + 2` and has at most `palette.rows.len()` entries.
/// Returns an ABSOLUTE row index (accounting for `scroll_top`).
pub(crate) fn palette_row_at(area: Rect, palette: &crate::palette::Palette, col: u16, row: u16) -> Option<usize> {
    let r = palette_overlay_rect(area, palette.rows.len());
    let list_top = r.y.saturating_add(2);
    let list_h = crate::list_window::list_h_for(palette.rows.len(), area.height) as u16;
    if col >= r.x.saturating_add(1) && col < r.x.saturating_add(r.width).saturating_sub(1)
        && row >= list_top && row < list_top.saturating_add(list_h)
    {
        Some((row - list_top) as usize + palette.scroll_top)
    } else {
        None
    }
}

/// Return the absolute list-row index that `(col, row)` hits in the theme picker,
/// or `None` when the click is outside the list interior. Mirrors `palette_row_at`.
pub(crate) fn theme_picker_row_at(area: Rect, tp: &crate::theme_picker::ThemePicker, col: u16, row: u16) -> Option<usize> {
    let r = palette_overlay_rect(area, tp.rows.len());
    let list_top = r.y.saturating_add(2);
    let list_h = crate::list_window::list_h_for(tp.rows.len(), area.height) as u16;
    if col >= r.x.saturating_add(1) && col < r.x.saturating_add(r.width).saturating_sub(1)
        && row >= list_top && row < list_top.saturating_add(list_h) {
        Some((row - list_top) as usize + tp.scroll_top)
    } else { None }
}

/// Return the absolute list-row index that `(col, row)` hits in the file browser,
/// or `None` when the click is outside the list interior. Mirrors `palette_row_at`.
pub(crate) fn file_browser_row_at(area: Rect, fb: &crate::file_browser::FileBrowser, col: u16, row: u16) -> Option<usize> {
    let r = palette_overlay_rect(area, fb.entries.len());
    let list_top = r.y.saturating_add(2);
    let list_h = crate::list_window::list_h_for(fb.entries.len(), area.height) as u16;
    if col >= r.x.saturating_add(1) && col < r.x.saturating_add(r.width).saturating_sub(1)
        && row >= list_top && row < list_top.saturating_add(list_h) {
        Some((row - list_top) as usize + fb.scroll_top)
    } else { None }
}

/// Return the absolute list-row index that `(col, row)` hits in the outline overlay,
/// or `None` when the click is outside the list interior. Mirrors `palette_row_at`.
pub(crate) fn outline_row_at(area: Rect, outline: &crate::outline_overlay::OutlineOverlay, col: u16, row: u16) -> Option<usize> {
    let r = palette_overlay_rect(area, outline.rows.len());
    let list_top = r.y.saturating_add(2);
    let list_h = crate::list_window::list_h_for(outline.rows.len(), area.height) as u16;
    if col >= r.x.saturating_add(1) && col < r.x.saturating_add(r.width).saturating_sub(1)
        && row >= list_top && row < list_top.saturating_add(list_h) {
        Some((row - list_top) as usize + outline.scroll_top)
    } else { None }
}

/// Return the absolute list-row index that `(col, row)` hits in the diagnostic
/// quick-fix overlay, or `None` when the click is outside the list interior.
/// Mirrors `palette_row_at` — note the list starts at `ov_y + 1` (no query row).
pub(crate) fn diag_row_at(area: Rect, diag: &crate::diag_overlay::DiagOverlay, col: u16, row: u16) -> Option<usize> {
    let row_count = diag.row_count();
    let r = palette_overlay_rect(area, row_count);
    let list_top = r.y.saturating_add(1);
    let list_h = crate::list_window::list_h_for(row_count, area.height) as u16;
    if col >= r.x.saturating_add(1) && col < r.x.saturating_add(r.width).saturating_sub(1)
        && row >= list_top && row < list_top.saturating_add(list_h) {
        Some((row - list_top) as usize + diag.scroll_top)
    } else { None }
}

/// Map a click on the status row to a prompt choice.
///
/// Column-based, case-insensitive, marker-to-next-marker span model:
/// — chars are iterated and each is counted as 1 display column (width-1 assumption;
///   prompt messages are ASCII-mostly — `·` U+00B7 is 1 terminal column, matching
///   the assumption; `unicode-width` is not a direct dependency of this crate);
/// — each choice's `[K]` marker is found case-insensitively (`[k]` and `[K]` both
///   match), so prompts like `transform_chooser` with lowercase markers are clickable;
/// — the clickable span for choice i runs from its marker's start column to the start
///   column of the NEXT choice's marker (or end-of-message), making the hit-test
///   separator-agnostic: `·`, double-space, or any separator all work uniformly;
/// — returns `None` when the row is not the status row, or the click falls before the
///   first marker.
pub(crate) fn prompt_choice_at(area: Rect, prompt: &crate::prompt::Prompt, col: u16, row: u16)
    -> Option<crate::prompt::PromptAction> {
    if row != area.y + area.height.saturating_sub(1) { return None; } // status row only
    let rel = col.saturating_sub(area.x) as usize;
    let msg = &prompt.message;

    // Collect chars once — char index == column offset (width-1 assumption).
    let chars: Vec<char> = msg.chars().collect();

    // Build (start_col, action) for each choice by sliding a 3-char window.
    let mut spans: Vec<(usize, crate::prompt::PromptAction)> = Vec::new();
    for choice in &prompt.choices {
        let key_lc = choice.key.to_ascii_lowercase();
        let key_uc = choice.key.to_ascii_uppercase();
        for (col_idx, window) in chars.windows(3).enumerate() {
            if window[0] == '[' && (window[1] == key_lc || window[1] == key_uc) && window[2] == ']' {
                spans.push((col_idx, choice.action));
                break;
            }
        }
    }

    // Sort by column so spans are in message order.
    spans.sort_by_key(|s| s.0);

    // Span i runs from its start column to the next span's start column (or message end).
    for (i, &(start, action)) in spans.iter().enumerate() {
        let end = spans.get(i + 1).map(|s| s.0).unwrap_or(usize::MAX);
        if rel >= start && rel < end {
            return Some(action);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// palette_overlay_rect sizes height to the actual row count, not fixed-15.
    #[test]
    fn palette_overlay_rect_sizes_to_row_count() {
        let area = Rect::new(0, 0, 80, 40);
        // 3 rows → list_h=3, ov_h=3+3=6
        let r3 = palette_overlay_rect(area, 3);
        assert_eq!(r3.height, 6, "3 rows: expected height 6 (3 list + 3 chrome)");
        // 30 rows → list_h capped at 15, ov_h=15+3=18
        let r30 = palette_overlay_rect(area, 30);
        assert_eq!(r30.height, 18, "30 rows: expected height 18 (15 capped + 3 chrome)");
    }

    /// Build a synthetic groups list: one category (Edit) with `n` leaves.
    #[cfg(test)]
    fn tall_menu_groups(n: usize)
        -> Vec<(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)>
    {
        let leaves: Vec<(String, crate::registry::CommandId)> = (0..n)
            .map(|i| (format!("item{i}"), crate::registry::CommandId("move_right")))
            .collect();
        vec![(crate::registry::MenuCategory::Edit, leaves)]
    }

    /// T14-a: dropdown height is `leaves.min(15).min(avail_below)`, NOT the raw leaf count.
    /// avail_below = area.height - 1 (rows under the bar, no border/query chrome).
    #[test]
    fn menu_dropdown_windows_a_tall_category() {
        let area = Rect::new(0, 0, 80, 8);       // avail_below = height - 1 = 7
        let groups = tall_menu_groups(20);       // helper: a category with 20 leaves
        let rect = menu_dropdown_rect(area, &groups, 0).expect("dropdown rect");
        let avail_below = (area.height - 1) as usize;
        let n_leaves: usize = 20;
        let expected = n_leaves.min(15).min(avail_below); // = 7 here (NOT list_h_for's h-4 = 4)
        assert_eq!(rect.height as usize, expected,
            "dropdown height = leaves.min(15).min(avail_below), not the raw leaf count (20)");
    }

    /// T14-d: clicking the reserved indicator row of an overflowing dropdown must return
    /// `None` — not the index of a hidden off-screen item the user cannot see.
    ///
    /// Geometry: 80×8 terminal, 20-leaf category.  avail_below = 7, drop_rect.height = 7,
    /// item_rows = 6.  The bottom row (drop_rect.y + 6) is the indicator row.  Under the
    /// old code `menu_dropdown_row_at` returned `Some(scroll_top + 6)` — dispatching the
    /// 7th item (index 6) which is hidden behind the indicator.  After the fix it returns
    /// `None`, so the click-outside arm closes the menu instead.
    #[test]
    fn dropdown_indicator_row_hit_test_returns_none() {
        // 80×8 terminal: avail_below=7, drop_rect.height=7, item_rows=6.
        let area      = Rect::new(0, 0, 80, 8);
        let menu_area = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        let groups    = tall_menu_groups(20);

        let drop_rect = menu_dropdown_rect(menu_area, &groups, 0)
            .expect("tall category must produce a dropdown rect");
        // list_h=7, item_rows=6 — the indicator row is at the bottom of drop_rect.
        let list_h   = drop_rect.height as usize;
        let item_rows = list_h.saturating_sub(1); // = 6
        let indicator_row = drop_rect.y + drop_rect.height - 1;
        let col = drop_rect.x;    // any column within the dropdown
        let scroll_top = 0usize;

        // A click on the indicator row must return None — no hidden command dispatched.
        assert_eq!(
            menu_dropdown_row_at(menu_area, &groups, 0, scroll_top, col, indicator_row),
            None,
            "indicator row click must return None (the reserved row is not a visible item)",
        );

        // A click on the last real item row must return the correct absolute index.
        let last_item_row = drop_rect.y + (item_rows as u16 - 1);
        assert_eq!(
            menu_dropdown_row_at(menu_area, &groups, 0, scroll_top, col, last_item_row),
            Some(scroll_top + item_rows - 1),
            "last real item row must return the correct absolute index",
        );

        // A click on the first item row must also work correctly.
        assert_eq!(
            menu_dropdown_row_at(menu_area, &groups, 0, scroll_top, col, drop_rect.y),
            Some(scroll_top),
            "first item row must return scroll_top",
        );
    }
}
