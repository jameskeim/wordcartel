//! Caret screen placement and viewport scroll.
//!
//! The caret is `selection.primary().head` — the *moving* end of the
//! primary range.  `from()`/`to()` are the normalised (min/max) ends; those
//! are for copy/delete range bounds (Task 9), not for caret placement.

use crate::derive;
use crate::editor::Editor;
use wordcartel_core::layout;

/// The raw caret byte-offset: `selection.primary().head`.
pub fn head(editor: &Editor) -> usize {
    editor.document.selection.primary().head
}

/// Logical line index of the caret.
pub fn caret_line(editor: &Editor) -> usize {
    let buf = &editor.document.buffer;
    let h = head(editor);
    if buf.len() == 0 {
        return 0;
    }
    // clamp to last valid byte if head is past end (shouldn't normally happen)
    buf.byte_to_line(h.min(buf.len().saturating_sub(1)))
}

/// Lay out logical line `L` on demand (if not already in the cache).
///
/// Used when `screen_pos` needs the caret line but it isn't in `line_layouts`
/// (e.g. before `ensure_visible` + `derive::rebuild` have run).
fn layout_line_on_demand(editor: &Editor, l: usize) -> wordcartel_core::layout::ColMap {
    let buf = &editor.document.buffer;
    let text = derive::line_text(buf, l);
    let role = editor.document.blocks.role_at(derive::line_start(buf, l));
    let is_active = l == caret_line(editor);
    let vp_width = (editor.view.area.0 as usize).max(1);
    let (_rows, map) = layout::layout(&text, role, is_active, vp_width);
    map
}

/// Caret cell `(col, row)` within the editing area, or `None` if the caret
/// line is scrolled off-screen or the computed screen row >= area height.
///
/// Algorithm:
/// 1. Find the caret logical line `L`.
/// 2. Bail if `L < scroll` (caret above visible range).
/// 3. Compute `in_off = head - line_start(L)` and look up (or derive on demand)
///    the `ColMap` for line `L`.
/// 4. `let (vrow, vcol) = map.source_to_visual(snapped_in_off)`.
/// 5. Screen row = sum of `ColMap.rows` for visible lines `[scroll..L)` + vrow.
/// 6. Return `None` if screen row >= area height.
pub fn screen_pos(editor: &Editor) -> Option<(u16, u16)> {
    let buf = &editor.document.buffer;
    let scroll = editor.view.scroll;
    // Editing area excludes the bottom status row (render reserves frame_h - 1),
    // so nav must reserve it too — else a caret on the last row is deemed visible
    // but never painted/cursor-placed. view.area is the FULL terminal size.
    let area_height = (editor.view.area.1 as usize).saturating_sub(1);
    let h = head(editor);
    let l = caret_line(editor);

    if l < scroll {
        return None;
    }

    // Accumulate visual rows for lines [scroll..L)
    let mut screen_row: usize = 0;
    for line_idx in scroll..l {
        let rows = if let Some((_, map)) = editor.view.line_layouts.get(&line_idx) {
            map.rows
        } else {
            // lay out on demand
            layout_line_on_demand(editor, line_idx).rows
        };
        screen_row += rows;
        if screen_row >= area_height {
            // caret is below visible area before we even reach L
            return None;
        }
    }

    // Get ColMap for the caret line
    let map_owned;
    let map: &wordcartel_core::layout::ColMap =
        if let Some((_, map)) = editor.view.line_layouts.get(&l) {
            map
        } else {
            map_owned = layout_line_on_demand(editor, l);
            &map_owned
        };

    let line_off = derive::line_start(buf, l);
    let in_off = h.saturating_sub(line_off);
    // Snap to a valid cursor stop before calling source_to_visual
    let snapped = map.snap_to_stop(in_off);
    let (vrow, vcol) = map.source_to_visual(snapped);

    let final_row = screen_row + vrow;
    if final_row >= area_height {
        return None;
    }

    Some((vcol as u16, final_row as u16))
}

// ---------------------------------------------------------------------------
// Horizontal navigation
// ---------------------------------------------------------------------------

/// Helper: lay out line `l` for the ColMap, treating it as the active caret
/// line (is_active=true). Used during line transitions where the target line
/// will become the new caret line.
fn layout_line_active(editor: &Editor, l: usize) -> wordcartel_core::layout::ColMap {
    let buf = &editor.document.buffer;
    let text = derive::line_text(buf, l);
    let role = editor.document.blocks.role_at(derive::line_start(buf, l));
    let vp_width = (editor.view.area.0 as usize).max(1);
    let (_rows, map) = layout::layout(&text, role, true, vp_width);
    map
}

/// Get the ColMap for line `l` from the cache if available, else lay it out
/// with the appropriate `is_active` flag.
fn get_or_layout(editor: &Editor, l: usize) -> wordcartel_core::layout::ColMap {
    if let Some((_, map)) = editor.view.line_layouts.get(&l) {
        map.clone()
    } else {
        layout_line_on_demand(editor, l)
    }
}

/// Move the caret one grapheme to the right.
///
/// Returns the new global byte offset. Sets `editor.desired_col = None` to
/// re-anchor vertical motion.
///
/// At the end of line L (and L is not the last line), crosses to line L+1.
pub fn move_right(editor: &mut Editor) -> usize {
    let buf = &editor.document.buffer;
    let h = head(editor);
    let l = caret_line(editor);
    let ls = derive::line_start(buf, l);
    let in_off = h.saturating_sub(ls);
    let total = derive::total_logical_lines(buf);

    // Get the ColMap for the caret line (from cache, using the already-computed
    // is_active flag, or on-demand).
    let map = get_or_layout(editor, l);

    let cur = layout::cursor_at(&map, in_off);
    let nxt = layout::move_right(&map, cur);

    let new_offset = if nxt.offset == cur.offset && cur.offset == map.eol && l + 1 < total {
        // At line end and not the last line → transition to next line.
        // The target line becomes the new caret line, so lay it out as active.
        let next_map = layout_line_active(editor, l + 1);
        let next_ls = derive::line_start(&editor.document.buffer, l + 1);
        // Snap to the first valid cursor stop on the next line.
        let first_stop = layout::cursor_at(&next_map, 0);
        next_ls + first_stop.offset
    } else {
        ls + nxt.offset
    };

    editor.desired_col = None;
    new_offset
}

/// Move the caret one grapheme to the left.
///
/// Returns the new global byte offset. Sets `editor.desired_col = None`.
///
/// At the start of line L (and L > 0), crosses to the end of line L-1.
pub fn move_left(editor: &mut Editor) -> usize {
    let buf = &editor.document.buffer;
    let h = head(editor);
    let l = caret_line(editor);
    let ls = derive::line_start(buf, l);
    let in_off = h.saturating_sub(ls);

    let map = get_or_layout(editor, l);

    let cur = layout::cursor_at(&map, in_off);
    let nxt = layout::move_left(&map, cur);

    let new_offset = if nxt.offset == cur.offset && in_off == 0 && l > 0 {
        // At line start and not the first line → transition to end of line L-1.
        let prev_map = layout_line_active(editor, l - 1);
        let prev_ls = derive::line_start(&editor.document.buffer, l - 1);
        let eol_cur = layout::cursor_at(&prev_map, prev_map.eol);
        prev_ls + eol_cur.offset
    } else {
        ls + nxt.offset
    };

    editor.desired_col = None;
    new_offset
}

/// Move the caret to the start of the current visual row (does not cross lines).
///
/// Returns the new global byte offset. Sets `editor.desired_col = None`.
pub fn move_home(editor: &mut Editor) -> usize {
    let buf = &editor.document.buffer;
    let h = head(editor);
    let l = caret_line(editor);
    let ls = derive::line_start(buf, l);
    let in_off = h.saturating_sub(ls);

    let map = get_or_layout(editor, l);
    let cur = layout::cursor_at(&map, in_off);
    let result = layout::move_home(&map, cur);

    editor.desired_col = None;
    ls + result.offset
}

/// Move the caret to the end of the current visual row (does not cross lines).
///
/// Returns the new global byte offset. Sets `editor.desired_col = None`.
pub fn move_end(editor: &mut Editor) -> usize {
    let buf = &editor.document.buffer;
    let h = head(editor);
    let l = caret_line(editor);
    let ls = derive::line_start(buf, l);
    let in_off = h.saturating_sub(ls);

    let map = get_or_layout(editor, l);
    let cur = layout::cursor_at(&map, in_off);
    let result = layout::move_end(&map, cur);

    editor.desired_col = None;
    ls + result.offset
}

/// Adjust `view.scroll` so the caret line's visual rows fall within the
/// visible area height.
///
/// - If the caret line is above the viewport (`caret_line < scroll`):
///   `scroll = caret_line`.
/// - If the caret line's last visual row is below the viewport:
///   find the largest scroll value that keeps the caret line visible.
/// - Clamp scroll to `[0, total_logical_lines - 1]`.
pub fn ensure_visible(editor: &mut Editor) {
    let l = caret_line(editor);
    let total = derive::total_logical_lines(&editor.document.buffer);
    // Editing area excludes the bottom status row (render reserves frame_h - 1),
    // so nav must reserve it too — else a caret on the last row is deemed visible
    // but never painted/cursor-placed. view.area is the FULL terminal size.
    let area_height = (editor.view.area.1 as usize).saturating_sub(1);

    // Clamp scroll to valid range first
    let max_scroll = total.saturating_sub(1);
    if editor.view.scroll > max_scroll {
        editor.view.scroll = max_scroll;
    }

    // If caret is above the scroll, scroll up to caret line
    if l < editor.view.scroll {
        editor.view.scroll = l;
        return;
    }

    // Check if caret line is below the visible area.
    // Count how many visual rows the lines [scroll..=l] occupy.
    let visual_rows_up_to_caret = count_visual_rows(editor, editor.view.scroll, l + 1);
    if visual_rows_up_to_caret <= area_height {
        // caret is visible already
        return;
    }

    // Caret is below: we need to increase scroll so that the caret line
    // fits within the viewport. Find the largest scroll s such that:
    //   sum of visual rows for lines [s..=l] <= area_height
    //
    // Walk from l downward (increasing scroll) until we fit.
    let caret_rows = count_visual_rows(editor, l, l + 1);
    if caret_rows >= area_height {
        // Even the caret line alone overflows; just show it from its start
        editor.view.scroll = l.min(max_scroll);
        return;
    }

    // Try to include as many lines before caret as possible.
    // Start from l going back.
    let mut accumulated = caret_rows;
    let mut new_scroll = l;
    for s in (0..l).rev() {
        let rows = count_visual_rows(editor, s, s + 1);
        if accumulated + rows > area_height {
            break;
        }
        accumulated += rows;
        new_scroll = s;
    }

    editor.view.scroll = new_scroll.min(max_scroll);
}

/// Count the total visual rows for logical lines in `[from_line, to_line)`.
/// Uses `line_layouts` cache when available; falls back to on-demand layout.
fn count_visual_rows(editor: &Editor, from_line: usize, to_line: usize) -> usize {
    let mut total = 0;
    for idx in from_line..to_line {
        let rows = if let Some((_, map)) = editor.view.line_layouts.get(&idx) {
            map.rows
        } else {
            layout_line_on_demand(editor, idx).rows
        };
        total += rows;
    }
    total
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{ensure_visible, move_home, move_end, move_left, move_right, screen_pos};
    use crate::derive;
    use crate::editor::Editor;
    use wordcartel_core::selection::Selection;

    /// Test helper: set the caret to a raw byte offset.
    fn set_caret(e: &mut Editor, off: usize) {
        e.document.selection = Selection::single(off);
    }

    // ------------------------------------------------------------------
    // Task 6: Horizontal nav (RED → GREEN)
    // ------------------------------------------------------------------

    #[test]
    fn right_crosses_line_boundary() {
        let mut e = Editor::new_from_text("ab\ncd\n", None, (80, 24));
        set_caret(&mut e, 2); // end of "ab" (before '\n')
        derive::rebuild(&mut e);
        let n = move_right(&mut e); // should land at start of "cd" (offset 3)
        assert_eq!(n, 3);
    }

    #[test]
    fn left_crosses_line_boundary() {
        let mut e = Editor::new_from_text("ab\ncd\n", None, (80, 24));
        set_caret(&mut e, 3); // start of "cd"
        derive::rebuild(&mut e);
        let n = move_left(&mut e); // -> end of "ab" (offset 2)
        assert_eq!(n, 2);
    }

    #[test]
    fn right_crosses_line_boundary_multibyte() {
        // "é" is 2 bytes (0..2), so end of line 0 is offset 2 (before '\n' at 2).
        // After crossing: start of "z" at offset 3 (byte after '\n').
        let mut e = Editor::new_from_text("é\nz\n", None, (80, 24));
        set_caret(&mut e, 2); // EOL of "é" line (byte 2 = 'é'.len())
        derive::rebuild(&mut e);
        let n = move_right(&mut e); // should land at start of "z" (offset 3)
        assert_eq!(n, 3);
    }

    #[test]
    fn left_crosses_line_boundary_multibyte() {
        // Start of "z" line is offset 3 (after "é\n" = 3 bytes).
        // move_left should land at end of "é" = offset 2.
        let mut e = Editor::new_from_text("é\nz\n", None, (80, 24));
        set_caret(&mut e, 3); // start of "z"
        derive::rebuild(&mut e);
        let n = move_left(&mut e); // -> EOL of "é" line (offset 2)
        assert_eq!(n, 2);
    }

    #[test]
    fn move_home_within_line() {
        let mut e = Editor::new_from_text("ab\ncd\n", None, (80, 24));
        set_caret(&mut e, 4); // 'd' in "cd"
        derive::rebuild(&mut e);
        let n = move_home(&mut e);
        assert_eq!(n, 3); // start of "cd"
    }

    #[test]
    fn move_end_within_line() {
        let mut e = Editor::new_from_text("ab\ncd\n", None, (80, 24));
        set_caret(&mut e, 3); // 'c' in "cd"
        derive::rebuild(&mut e);
        let n = move_end(&mut e);
        assert_eq!(n, 5); // EOL of "cd" = line_start(1) + eol(2) = 3 + 2 = 5
    }

    // ------------------------------------------------------------------
    // Brief's required failing tests (RED → GREEN)
    // ------------------------------------------------------------------

    #[test]
    fn screen_pos_maps_caret_to_cell() {
        let mut e = Editor::new_from_text("abc\ndef\n", None, (80, 24));
        set_caret(&mut e, 5); // 'e' in "def" (line 1, col 1)
        derive::rebuild(&mut e);
        assert_eq!(screen_pos(&e), Some((1, 1)));
    }

    #[test]
    fn ensure_visible_scrolls_caret_into_view() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 10));
        let line50_start = derive::line_start(&e.document.buffer, 50);
        set_caret(&mut e, line50_start);
        ensure_visible(&mut e);
        derive::rebuild(&mut e);
        assert!(screen_pos(&e).is_some());
        assert!(e.view.scroll <= 50 && e.view.scroll + 10 > 50);
    }

    // ------------------------------------------------------------------
    // Wrapped-line case
    // ------------------------------------------------------------------

    #[test]
    fn screen_pos_wrapped_line_second_visual_row() {
        // Width 3: "abcdef\n" wraps to rows ["abc","def"].
        // Caret at byte 3 ('d') -> line 0, vrow 1, vcol 0.
        // Screen pos should be (col=0, row=1).
        let mut e = Editor::new_from_text("abcdef\n", None, (3, 24));
        set_caret(&mut e, 3); // 'd' in "abcdef"
        derive::rebuild(&mut e);
        let pos = screen_pos(&e);
        assert!(pos.is_some(), "expected Some, got None");
        let (col, row) = pos.unwrap();
        assert_eq!(row, 1, "expected visual row 1 (second wrap), got {row}");
        assert_eq!(col, 0, "expected col 0, got {col}");
    }

    #[test]
    fn caret_above_scroll_returns_none() {
        let mut e = Editor::new_from_text("line0\nline1\nline2\n", None, (80, 24));
        set_caret(&mut e, 0); // caret on line 0
        e.view.scroll = 2;   // scroll past caret
        derive::rebuild(&mut e);
        assert_eq!(screen_pos(&e), None);
    }

    #[test]
    fn ensure_visible_scrolls_up_when_caret_above() {
        let text: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 10));
        e.view.scroll = 15;  // scroll to near end
        set_caret(&mut e, 0); // caret at very top
        ensure_visible(&mut e);
        assert_eq!(e.view.scroll, 0, "scroll should have gone back to 0");
    }
}
