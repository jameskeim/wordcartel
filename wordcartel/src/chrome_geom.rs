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
pub(crate) fn menu_bar_layout(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::menu::MenuRowAction)>)]) -> Vec<(usize, Rect)> {
    let cats: Vec<crate::registry::MenuCategory> = groups.iter().map(|g| g.0).collect();
    menu_bar_layout_cats(area, &cats)
}

pub(crate) fn menu_dropdown_rect(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::menu::MenuRowAction)>)], open: usize) -> Option<Rect> {
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

pub(crate) fn menu_dropdown_row_at(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::menu::MenuRowAction)>)], open: usize, scroll_top: usize, col: u16, row: u16) -> Option<usize> {
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

/// Bounding rect for a prompt's `detail` disclosure box, or `None` when there is no room
/// (or nothing to show). The box is horizontally sized and centred exactly like
/// `palette_overlay_rect` — same width ladder, same centring — but is **bottom-anchored so
/// its lower border sits immediately above `status_row`**, because the prompt's question and
/// choices stay on the status row itself (spec §5.3 as amended by C5 §11.3).
///
/// Height is `lines + 2` (the two borders), **clamped to the rows actually available above
/// the status row**. That clamp is what makes the box safe on a short terminal: it can never
/// grow past the frame, never overlap the status row it is disclosing for, and — since a
/// prompt with more `detail` than fits is truncated rather than expanded — never push the
/// choices off-screen. `None` is returned rather than a degenerate rect whenever fewer than
/// three rows are free (two borders plus at least one line of content) or the width ladder
/// cannot yield an interior, so callers have no zero-sized rect to paint into.
pub(crate) fn prompt_detail_rect(area: Rect, status_row: u16, lines: usize) -> Option<Rect> {
    if lines == 0 { return None; }
    // Rows strictly above the status row and inside `area`. `checked_sub` (not saturating)
    // because a status row above `area.y` means the caller's geometry is nonsense, not that
    // the box should be pinned to the top.
    let avail = status_row.checked_sub(area.y)?;
    if avail < 3 { return None; }
    let ov_w = palette_overlay_rect(area, lines).width;
    if ov_w < 3 { return None; } // two borders plus at least one interior column
    // `avail >= 3` ⇒ `body >= 1` and `ov_h <= avail`, so `status_row - ov_h >= area.y`.
    let body = u16::try_from(lines).unwrap_or(u16::MAX).min(avail - 2);
    let ov_h = body + 2;
    let ov_x = area.x.saturating_add((area.width.saturating_sub(ov_w)) / 2);
    Some(Rect::new(ov_x, status_row - ov_h, ov_w, ov_h))
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

/// Return the ABSOLUTE list-row index that `(col, row)` hits in the cursor picker, or
/// `None` when the click is outside the list interior. Mirrors `theme_picker_row_at`. The
/// cursor picker has NO query row, so its list starts one row below the top border
/// (`ov_y + 1`) — this MUST stay in lockstep with the `render_overlays` picker arm. The
/// overlay box is sized via `n + 1` rows (reserving room for the sample row below the
/// list), but the resulting visible-list height still equals
/// `list_window::list_h_for(n, area.height)` exactly — the `+1`/`+3`/`-3`/`-2` terms that
/// separate the box height from the list height cancel algebraically, so windowing reuses
/// the SAME formula as every sibling overlay (Finding 1).
pub(crate) fn cursor_picker_row_at(area: Rect, cp: &crate::cursor_picker::CursorPicker, col: u16, row: u16) -> Option<usize> {
    let n = crate::cursor_picker::ROW_ACTIONS.len();
    let r = palette_overlay_rect(area, n + 1);
    let list_top = r.y.saturating_add(1);
    let list_h = crate::list_window::list_h_for(n, area.height) as u16;
    if col >= r.x.saturating_add(1) && col < r.x.saturating_add(r.width).saturating_sub(1)
        && row >= list_top && row < list_top.saturating_add(list_h) {
        Some((row - list_top) as usize + cp.scroll_top)
    } else { None }
}

/// Whether a resolved-target footer row exists for `fb` — mirrors the guard
/// `file_browser::footer_target` opens with (destination mode + non-empty field), duplicated
/// here because geometry has no `Fs` handle: a filesystem probe can change the footer's TEXT,
/// never WHETHER the row exists, so this half of the guard needs none.
fn file_browser_has_footer_row(fb: &crate::file_browser::FileBrowser) -> bool {
    matches!(&fb.mode, crate::file_browser::BrowseMode::Destination { field, .. }
        if !field.trim().is_empty())
}

/// How many dedicated interior rows the picker's footer area wants: the resolved-target line
/// (destination mode with a non-empty field) plus one line per withholding disclosure
/// (§7.4/§6.2). MUST stay equal to the number of lines the painter stacks there, or the box
/// would be sized for rows nothing fills — the painter builds its stack in the same order and
/// from the same two sources.
pub(crate) fn file_browser_footer_rows(fb: &crate::file_browser::FileBrowser) -> usize {
    usize::from(file_browser_has_footer_row(fb))
        + crate::file_browser_listing::disclosure_line_count(&fb.disclosure)
}

/// The picker's row ledger: `(interior content rows, entry-list rows)`. THE single source
/// `file_browser_overlay_rect`, `file_browser_list_h`, the painter and the mouse hit-test all
/// read their geometry from, so a footer row can never shift one of them without the others
/// (the A21 mouse/render divergence this effort exists to prevent).
///
/// The footer lines are ADDITIVE where the terminal has room: the box grows by `reserved`
/// rows rather than confiscating them from the list. Confiscating was the old behaviour and
/// it cost a writer real information — a filtered destination listing with two entries showed
/// only one of them plus a `1/2` indicator, on a 100x30 terminal with rows to spare. Growth is
/// capped by the SAME `list_h_for` ceiling the painter honours (content cap 15, frame cap
/// `height - 4`), so a genuinely tiny terminal still degrades to the cramped border-title
/// fallback instead of painting out of bounds; and a listing already at that ceiling still
/// lends the footer one of its own rows, because there is nowhere left to grow.
fn file_browser_rows(area: Rect, fb: &crate::file_browser::FileBrowser) -> (usize, usize) {
    let reserved = file_browser_footer_rows(fb);
    let raw = crate::list_window::list_h_for(fb.entries.len(), area.height);
    let box_rows = crate::list_window::list_h_for(fb.entries.len() + reserved, area.height).max(raw);
    (box_rows, box_rows.saturating_sub(reserved))
}

/// How many of the reserved footer rows the box ACTUALLY got — `reserved` on any terminal
/// with room, and 0 on one too small to grow, where the painter falls back to the cramped
/// border-title footer. The painter reads this rather than re-deriving a height guard of its
/// own, so what it paints and what the box was sized for stay the same number.
pub(crate) fn file_browser_footer_rows_shown(area: Rect, fb: &crate::file_browser::FileBrowser) -> usize {
    let (box_rows, list_h) = file_browser_rows(area, fb);
    box_rows - list_h
}

/// Row budget for the file browser's ENTRY LIST — the second half of
/// [`file_browser_rows`]'s ledger.
pub(crate) fn file_browser_list_h(area: Rect, fb: &crate::file_browser::FileBrowser) -> u16 {
    file_browser_rows(area, fb).1 as u16
}

/// The file browser's overlay box — like `palette_overlay_rect`, but sized from
/// [`file_browser_rows`] so the footer's rows are part of the box rather than borrowed from
/// the list.
///
/// Single-sourced with the hit-test (`file_browser_row_at`/`file_browser_row_origin`) so the
/// two can never disagree on where the box's bottom edge is (the A21 hazard).
pub(crate) fn file_browser_overlay_rect(area: Rect, fb: &crate::file_browser::FileBrowser) -> Rect {
    palette_overlay_rect(area, file_browser_rows(area, fb).0)
}

/// Return the absolute list-row index that `(col, row)` hits in the file browser,
/// or `None` when the click is outside the list interior. Mirrors `palette_row_at`.
pub(crate) fn file_browser_row_at(area: Rect, fb: &crate::file_browser::FileBrowser, col: u16, row: u16) -> Option<usize> {
    let r = file_browser_overlay_rect(area, fb);
    let list_top = r.y.saturating_add(2);
    let list_h = file_browser_list_h(area, fb);
    if col >= r.x.saturating_add(1) && col < r.x.saturating_add(r.width).saturating_sub(1)
        && row >= list_top && row < list_top.saturating_add(list_h) {
        Some((row - list_top) as usize + fb.scroll_top)
    } else { None }
}

/// The inverse of `file_browser_row_at`: the screen cell the painter draws WINDOW-RELATIVE
/// row `row_index` at (i.e. the `row_index`-th visible row, not an absolute entry index), so
/// tests (and any future caller) can address the cell the painter drew a given row at without
/// duplicating the geometry.
#[allow(dead_code)] // test-only today — no production caller needs the inverse of the hit-test
pub(crate) fn file_browser_row_origin(area: Rect, fb: &crate::file_browser::FileBrowser, row_index: usize) -> (u16, u16) {
    let r = file_browser_overlay_rect(area, fb);
    let list_top = r.y.saturating_add(2);
    (r.x.saturating_add(1), list_top + row_index as u16)
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

/// Map a minibuffer click `(col, row)` to a byte offset in `mb.text`, or `None`
/// when the click is outside the input line (any row but the status row) or on
/// the prompt itself.
///
/// Mirrors `render.rs::place_cursor`'s minibuffer arm: the caret column is
/// `prompt.chars().count() + text[..cursor].chars().count()` — one terminal
/// column per char, byte offset only used for the actual string index. This
/// walks the inverse: subtract `prompt_cols` from the clicked column to get a
/// char-index into `mb.text`, then resolve that char-index to its byte offset
/// via `char_indices` (never a mid-char byte — `char_indices` only yields
/// boundaries). A click at or past the last char clamps to `mb.text.len()`.
pub(crate) fn minibuffer_click_byte(area: Rect, mb: &crate::minibuffer::Minibuffer, col: u16, row: u16) -> Option<usize> {
    if row != area.y + area.height.saturating_sub(1) {
        return None;
    }
    let prompt_end = area.x.saturating_add(mb.prompt.chars().count() as u16);
    if col < prompt_end {
        return None;
    }
    let click_char_col = (col - prompt_end) as usize;
    let byte = mb.text.char_indices().nth(click_char_col)
        .map_or(mb.text.len(), |(b, _)| b);
    Some(byte)
}

/// Column width of the label rendered BEFORE `field`'s text on the search status
/// row: `"Find: "` for the needle field, `"Find: {needle}  Replace: "` for the
/// template field. SINGLE SOURCE for `render.rs::place_cursor` (painter) and
/// `search_field_click` (hit-test below) — they must never drift.
pub(crate) fn search_field_prefix_cols(s: &crate::search_overlay::SearchState, field: crate::search_overlay::Field) -> usize {
    match field {
        crate::search_overlay::Field::Needle => "Find: ".chars().count(),
        crate::search_overlay::Field::Template => format!("Find: {}  Replace: ", s.needle).chars().count(),
    }
}

/// Map a search-bar click `(col, row)` to `(Field, byte_cursor)`, or `None` when
/// the click misses the status row or lands outside both fields' rendered text
/// (the label gaps, or the trailing mode/case/count/wrapped suffix — consumed as
/// a no-op by the caller). The template field is only a valid hit target when
/// `s.phase` is `Replace`/`Stepping` (it isn't rendered in `Phase::Find` —
/// mirrors `render_status::format_search_bar`). Byte cursor is char-count-mapped
/// (multibyte-safe, mirrors `minibuffer_click_byte`) and end-clamped: a click at
/// or past the last char of a field lands on `field.len()`.
pub(crate) fn search_field_click(area: Rect, s: &crate::search_overlay::SearchState, col: u16, row: u16)
    -> Option<(crate::search_overlay::Field, usize)>
{
    use crate::search_overlay::{Field, Phase};
    if row != area.y + area.height.saturating_sub(1) {
        return None;
    }
    let click_col = (col.checked_sub(area.x)?) as usize;

    let has_template = matches!(s.phase, Phase::Replace | Phase::Stepping);
    if has_template {
        let prefix = search_field_prefix_cols(s, Field::Template);
        let chars = s.template.chars().count();
        if click_col >= prefix && click_col <= prefix + chars {
            let idx = click_col - prefix;
            let byte = s.template.char_indices().nth(idx).map_or(s.template.len(), |(b, _)| b);
            return Some((Field::Template, byte));
        }
    }
    let prefix = search_field_prefix_cols(s, Field::Needle);
    let chars = s.needle.chars().count();
    if click_col >= prefix && click_col <= prefix + chars {
        let idx = click_col - prefix;
        let byte = s.needle.char_indices().nth(idx).map_or(s.needle.len(), |(b, _)| b);
        return Some((Field::Needle, byte));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- prompt_detail_rect ------------------------------------------------------

    #[test]
    fn prompt_detail_rect_sits_directly_above_the_status_row_and_shares_the_overlay_width() {
        let area = Rect::new(0, 0, 80, 24);
        let r = prompt_detail_rect(area, 23, 5).expect("ample room");
        assert_eq!(r.y + r.height, 23,
            "the box's bottom border sits immediately above the status row, never on it");
        assert_eq!(r.height, 7, "5 lines plus two borders");
        assert_eq!(r.width, palette_overlay_rect(area, 5).width,
            "same width ladder as every other overlay box");
        assert_eq!(r.x, palette_overlay_rect(area, 5).x, "and the same centring");
    }

    #[test]
    fn prompt_detail_rect_is_none_when_there_is_nothing_to_show() {
        assert_eq!(prompt_detail_rect(Rect::new(0, 0, 80, 24), 23, 0), None);
    }

    #[test]
    fn prompt_detail_rect_clamps_to_the_rows_above_the_status_row() {
        // A 7-line detail on a 7-row terminal: the status row is row 6, leaving 6 rows, so
        // the box may occupy at most 4 content rows. It must SHRINK, never overlap the
        // status row and never grow past the top of the frame.
        let area = Rect::new(0, 0, 80, 7);
        let r = prompt_detail_rect(area, 6, 7).expect("four rows still fit");
        assert_eq!(r.height, 6, "clamped to the six rows above the status row");
        assert_eq!(r.y, 0, "and pinned at the top of the frame, not above it");
        assert_eq!(r.y + r.height, 6);
    }

    #[test]
    fn prompt_detail_rect_refuses_degenerate_geometry_rather_than_returning_a_bad_rect() {
        // Fewer than three free rows cannot hold two borders and a line of content; a
        // zero-height box would be a painter's out-of-bounds waiting to happen.
        for (w, h, status_row) in [(80u16, 3u16, 2u16), (80, 2, 1), (80, 1, 0), (1, 24, 23)] {
            let area = Rect::new(0, 0, w, h);
            let got = prompt_detail_rect(area, status_row, 5);
            if let Some(r) = got {
                assert!(r.width >= 3 && r.height >= 3, "{w}x{h}: no degenerate rect: {r:?}");
                assert!(r.y + r.height <= status_row, "{w}x{h}: never over the status row");
                assert!(r.x + r.width <= w, "{w}x{h}: never past the right edge");
            }
        }
        // A status row above the area's own origin is nonsense geometry, not a reason to
        // pin the box to the top: refuse it.
        assert_eq!(prompt_detail_rect(Rect::new(0, 10, 80, 14), 4, 3), None);
    }

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

    /// `cursor_picker_row_at` at a tall terminal (all 7 rows visible, `scroll_top == 0`):
    /// hit index equals the row offset from the list top; a click on the sample row
    /// (below the list) misses.
    #[test]
    fn cursor_picker_row_at_no_scroll_maps_to_offset() {
        let area = Rect::new(0, 0, 60, 24);
        let cp = crate::cursor_picker::CursorPicker {
            selected: 0, original_shape: crate::config::CaretShape::Default,
            original_blink: false, scroll_top: 0,
        };
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let r = palette_overlay_rect(area, n + 1);
        let list_top = r.y + 1;
        // Row 3 (Beam · blinking) — well within the fully-visible list.
        assert_eq!(cursor_picker_row_at(area, &cp, r.x + 1, list_top + 3), Some(3));
        // Click on the sample row (below the list) must miss.
        let sample_row = r.y + r.height.saturating_sub(2);
        assert_eq!(cursor_picker_row_at(area, &cp, r.x + 1, sample_row), None,
            "sample row is not a list row");
    }

    /// `cursor_picker_row_at` at a SHORT terminal with a non-zero `scroll_top`: the hit
    /// index is ABSOLUTE — `(row - list_top) + scroll_top` — mirroring
    /// `theme_picker_row_at` (Finding 1/2 mouse-path coverage: the tail row must be
    /// clickable once scrolled into view, and geometry outside the window must miss).
    #[test]
    fn cursor_picker_row_at_scrolled_returns_absolute_index() {
        let area = Rect::new(0, 0, 60, 9); // short terminal — list_h_for(7, 9) == 5
        let cp = crate::cursor_picker::CursorPicker {
            selected: 6, original_shape: crate::config::CaretShape::Default,
            original_blink: false, scroll_top: 2,
        };
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let r = palette_overlay_rect(area, n + 1);
        let list_top = r.y + 1;
        let list_h = crate::list_window::list_h_for(n, area.height);
        assert_eq!(list_h, 5, "precondition: window shows 5 of 7 rows");
        // Screen row 0 of the window shows absolute row scroll_top (2), not row 0.
        assert_eq!(cursor_picker_row_at(area, &cp, r.x + 1, list_top), Some(2));
        // The LAST visible screen row shows absolute row scroll_top + list_h - 1 == 6.
        let last_row = list_top + (list_h as u16 - 1);
        assert_eq!(cursor_picker_row_at(area, &cp, r.x + 1, last_row), Some(6),
            "the tail row (6) must be clickable once scrolled into view");
        // One row past the window (the sample row) misses.
        assert_eq!(cursor_picker_row_at(area, &cp, r.x + 1, last_row + 1), None);
        // Outside the box entirely (far left) misses.
        assert_eq!(cursor_picker_row_at(area, &cp, 0, list_top), None);
    }

    /// Build a synthetic groups list: one category (Edit) with `n` leaves.
    #[cfg(test)]
    fn tall_menu_groups(n: usize)
        -> Vec<(crate::registry::MenuCategory, Vec<(String, crate::menu::MenuRowAction)>)>
    {
        let leaves: Vec<(String, crate::menu::MenuRowAction)> = (0..n)
            .map(|i| (format!("item{i}"), crate::menu::MenuRowAction::Command(crate::registry::CommandId("move_right"))))
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

    #[test]
    fn hit_testing_and_the_painter_agree_on_the_last_row_in_destination_mode() {
        // The footer takes the row immediately below the last entry, so the list interior
        // stops one row short of the block's bottom edge. If `file_browser_row_at` kept the
        // old height, a click on the last visible row would select the row BELOW the one drawn
        // there — off-by-one on a surface where the next keystroke can commit a write.
        //
        // 20 entries, not a dozen: since the footer's row is ADDITIVE (the box grows for it
        // where the terminal allows — see `file_browser_rows`), the list only genuinely
        // windows once the content itself reaches the `list_h_for` ceiling. A fixture below
        // that ceiling would leave `list_h == entries.len()` and the "last visible row" this
        // test is about would not exist — the precondition below is what catches that.
        //
        // FAIL-VERIFY: leave `file_browser_row_at` computing its own height instead of
        // calling `file_browser_list_h`, watch this fail.
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let mut fb = crate::file_browser::FileBrowser {
            dir: std::env::temp_dir(), query: String::new(),
            mode: crate::file_browser::BrowseMode::Destination {
                purpose: crate::file_browser::DestinationPurpose::SaveAs,
                field: "x".into(), field_cursor: 1 },
            listing: Vec::new(), total_seen: 0, unreadable: 0,
            entries: (0..20).map(|i| crate::file_browser::FileEntry {
                name: format!("f{i:02}.md"), kind: crate::fsx::EntryKind::File,
                is_symlink: false, broken: false }).collect(),
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None, navigated_name: None,
        };
        let list_h = file_browser_list_h(area, &fb) as usize;
        assert!(list_h > 0 && list_h < fb.entries.len(),
            "precondition: the list is windowed, so a last visible row exists");
        let last = list_h - 1;
        let (col, row) = file_browser_row_origin(area, &fb, last);
        assert_eq!(file_browser_row_at(area, &fb, col, row), Some(last),
            "a click on the cell the painter drew row {last} at must select row {last}");
        // And one row further down is OUTSIDE the list — that cell belongs to the footer.
        assert_eq!(file_browser_row_at(area, &fb, col, row + 1), None,
            "the row below the last entry is the footer, not a selectable entry");
        fb.selected = last;
    }
}
