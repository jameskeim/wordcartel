//! Caret screen placement and viewport scroll.
//!
//! The caret is `selection.primary().head` — the *moving* end of the
//! primary range.  `from()`/`to()` are the normalised (min/max) ends; those
//! are for copy/delete range bounds (Task 9), not for caret placement.

use crate::derive;
use crate::editor::{Editor, RenderMode};
use wordcartel_core::block_tree::{Block, BlockTree};
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::layout;

// ---------------------------------------------------------------------------
// Measure / centered-text geometry
// ---------------------------------------------------------------------------

/// The text column's left edge (relative to area.x) and width for this frame.
pub struct TextGeometry {
    pub text_left: u16,
    pub text_width: u16,
}

/// Compute the text column geometry for the active editor view.
///
/// When `measure` is enabled and the viewport is wider than `wrap_column`,
/// text is centered: `text_left = (vp - wrap_column) / 2`, `text_width = wrap_column`.
/// Otherwise the full viewport is used: `text_left = 0`, `text_width = vp.max(1)`.
pub fn text_geometry(editor: &Editor) -> TextGeometry {
    let vp = editor.active().view.area.0;
    let o = &editor.view_opts;
    if o.measure && vp > o.wrap_column && o.wrap_column > 0 {
        let text_width = o.wrap_column;
        TextGeometry { text_left: (vp - text_width) / 2, text_width }
    } else {
        TextGeometry { text_left: 0, text_width: vp.max(1) }
    }
}

/// The raw caret byte-offset: `selection.primary().head`.
pub fn head(editor: &Editor) -> usize {
    editor.active().document.selection.primary().head
}

/// Logical line index of the caret.
pub fn caret_line(editor: &Editor) -> usize {
    let buf = &editor.active().document.buffer;
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
/// Honors `view.mode`: in source modes, all lines render raw (is_active=true).
fn layout_line_on_demand(editor: &Editor, l: usize) -> wordcartel_core::layout::ColMap {
    let buf = &editor.active().document.buffer;
    let text = derive::line_text(buf, l);
    let role = editor.active().document.blocks.role_at(derive::line_start(buf, l));
    let source_mode = editor.active().view.mode != RenderMode::LivePreview;
    let is_active_effective = (l == caret_line(editor)) || source_mode;
    let vp_width = text_geometry(editor).text_width as usize;
    let (_rows, map) = layout::layout(&text, role, is_active_effective, vp_width);
    map
}

/// Caret cell `(col, row)` within the editing area, or `None` if the caret
/// line is scrolled off-screen or the computed screen row >= area height.
///
/// Algorithm:
/// 1. Find the caret logical line `L`.
/// 2. Bail if `L < scroll` (caret above visible range), or if `L == scroll`
///    and the caret's visual row is above `scroll_row`.
/// 3. Compute `in_off = head - line_start(L)` and look up (or derive on demand)
///    the `ColMap` for line `L`.
/// 4. `let (vrow, vcol) = map.source_to_visual(snapped_in_off)`.
/// 5. Screen row = visible visual rows from `(scroll, scroll_row)` to `(L, vrow)`.
/// 6. Return `None` if screen row >= area height.
pub fn screen_pos(editor: &Editor) -> Option<(u16, u16)> {
    let buf = &editor.active().document.buffer;
    let scroll = editor.active().view.scroll;
    let scroll_row = editor.active().view.scroll_row;
    // Editing area excludes the bottom status row (render reserves frame_h - 1),
    // so nav must reserve it too — else a caret on the last row is deemed visible
    // but never painted/cursor-placed. view.area is the FULL terminal size.
    let area_height = (editor.active().view.area.1 as usize).saturating_sub(1);
    let h = head(editor);
    let l = caret_line(editor);

    if l < scroll {
        return None;
    }

    // Get ColMap for the caret line
    let map_owned;
    let map: &wordcartel_core::layout::ColMap =
        if let Some((_, map)) = editor.active().view.line_layouts.get(&l) {
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

    if l == scroll && vrow < scroll_row {
        return None;
    }

    let final_row = rows_before_caret(editor, l, vrow)?;
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
    let buf = &editor.active().document.buffer;
    let text = derive::line_text(buf, l);
    let role = editor.active().document.blocks.role_at(derive::line_start(buf, l));
    let vp_width = text_geometry(editor).text_width as usize;
    let (_rows, map) = layout::layout(&text, role, true, vp_width);
    map
}

/// Get the ColMap for line `l` from the cache if available, else lay it out
/// with the appropriate `is_active` flag.
fn get_or_layout(editor: &Editor, l: usize) -> wordcartel_core::layout::ColMap {
    if let Some((_, map)) = editor.active().view.line_layouts.get(&l) {
        map.clone()
    } else {
        layout_line_on_demand(editor, l)
    }
}

/// Clamp `off` to `0..=len` and snap it to a grapheme stop on ITS OWN line
/// (a mark/ring/session offset may be on a different line than the caret).
pub fn clamp_snap(editor: &Editor, off: usize) -> usize {
    let buf = &editor.active().document.buffer;
    let len = buf.len();
    let off = off.min(len);
    if len == 0 { return 0; }
    let line = buf.byte_to_line(off.min(len.saturating_sub(1)));
    let ls = derive::line_start(buf, line);
    let map = get_or_layout(editor, line);
    ls + map.snap_to_stop(off.saturating_sub(ls))
}

/// Move the caret one grapheme to the right.
///
/// Returns the new global byte offset. Sets `editor.desired_col = None` to
/// re-anchor vertical motion.
///
/// At the end of line L (and L is not the last line), crosses to line L+1.
pub fn move_right(editor: &mut Editor) -> usize {
    let buf = &editor.active().document.buffer;
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
        let next_ls = derive::line_start(&editor.active().document.buffer, l + 1);
        // Snap to the first valid cursor stop on the next line.
        let first_stop = layout::cursor_at(&next_map, 0);
        next_ls + first_stop.offset
    } else {
        ls + nxt.offset
    };

    editor.active_mut().desired_col = None;
    new_offset
}

/// Move the caret one grapheme to the left.
///
/// Returns the new global byte offset. Sets `editor.desired_col = None`.
///
/// At the start of line L (and L > 0), crosses to the end of line L-1.
pub fn move_left(editor: &mut Editor) -> usize {
    let buf = &editor.active().document.buffer;
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
        let prev_ls = derive::line_start(&editor.active().document.buffer, l - 1);
        let eol_cur = layout::cursor_at(&prev_map, prev_map.eol);
        prev_ls + eol_cur.offset
    } else {
        ls + nxt.offset
    };

    editor.active_mut().desired_col = None;
    new_offset
}

/// Move the caret to the start of the current visual row (does not cross lines).
///
/// Returns the new global byte offset. Sets `editor.desired_col = None`.
pub fn move_home(editor: &mut Editor) -> usize {
    let buf = &editor.active().document.buffer;
    let h = head(editor);
    let l = caret_line(editor);
    let ls = derive::line_start(buf, l);
    let in_off = h.saturating_sub(ls);

    let map = get_or_layout(editor, l);
    let cur = layout::cursor_at(&map, in_off);
    let result = layout::move_home(&map, cur);

    editor.active_mut().desired_col = None;
    ls + result.offset
}

/// Move the caret to the end of the current visual row (does not cross lines).
///
/// Returns the new global byte offset. Sets `editor.desired_col = None`.
pub fn move_end(editor: &mut Editor) -> usize {
    let buf = &editor.active().document.buffer;
    let h = head(editor);
    let l = caret_line(editor);
    let ls = derive::line_start(buf, l);
    let in_off = h.saturating_sub(ls);

    let map = get_or_layout(editor, l);
    let cur = layout::cursor_at(&map, in_off);
    let result = layout::move_end(&map, cur);

    editor.active_mut().desired_col = None;
    ls + result.offset
}

// ---------------------------------------------------------------------------
// Vertical navigation
// ---------------------------------------------------------------------------

/// Move the caret down one visual row.
///
/// - If the caret is on a wrapped logical line with more rows below, stays
///   within that logical line (the wrapped lower row).
/// - Otherwise, if there is a next logical line, crosses into it landing at
///   the desired visual column in the top row.
/// - If already on the last line's last visual row, no-op.
///
/// Returns the new global byte offset. Preserves `editor.desired_col` across
/// the motion (computes it from the current visual column on the first vertical
/// move when it is `None`).
pub fn move_down(editor: &mut Editor) -> usize {
    let buf = &editor.active().document.buffer;
    let h = head(editor);
    let l = caret_line(editor);
    let ls = derive::line_start(buf, l);
    let in_off = h.saturating_sub(ls);
    let total = derive::total_logical_lines(buf);

    let map = get_or_layout(editor, l);
    let cur0 = layout::cursor_at(&map, in_off);

    // Anchor desired_col on the first vertical move.
    let desired = editor.active().desired_col.unwrap_or_else(|| map.col_on_row(in_off, cur0.row));
    editor.active_mut().desired_col = Some(desired);

    // Build the cursor with the STORED desired_col so move_down_within reads it.
    let mut cur = cur0;
    cur.desired_col = desired;

    match layout::move_down_within(&map, cur) {
        Some(c) => {
            // Stayed within the same (wrapped) logical line.
            ls + c.offset
        }
        None => {
            // At the bottom visual row of this logical line.
            if l + 1 >= total {
                // Already on the last line — no-op.
                h
            } else {
                // Cross into the next logical line.
                let next_map = layout_line_active(editor, l + 1);
                let next_ls = derive::line_start(&editor.active().document.buffer, l + 1);
                let c = layout::enter_from_top(&next_map, desired);
                next_ls + c.offset
            }
        }
    }
}

/// Move the caret up one visual row.
///
/// Symmetric with `move_down`: moves within a wrapped line if possible,
/// otherwise crosses to the bottom row of the previous logical line.
/// No-op if already on line 0's first visual row.
///
/// Returns the new global byte offset. Preserves `editor.desired_col`.
pub fn move_up(editor: &mut Editor) -> usize {
    let buf = &editor.active().document.buffer;
    let h = head(editor);
    let l = caret_line(editor);
    let ls = derive::line_start(buf, l);
    let in_off = h.saturating_sub(ls);

    let map = get_or_layout(editor, l);
    let cur0 = layout::cursor_at(&map, in_off);

    // Anchor desired_col on the first vertical move.
    let desired = editor.active().desired_col.unwrap_or_else(|| map.col_on_row(in_off, cur0.row));
    editor.active_mut().desired_col = Some(desired);

    // Build the cursor with the STORED desired_col so move_up_within reads it.
    let mut cur = cur0;
    cur.desired_col = desired;

    match layout::move_up_within(&map, cur) {
        Some(c) => {
            // Stayed within the same (wrapped) logical line.
            ls + c.offset
        }
        None => {
            // At the top visual row of this logical line.
            if l == 0 {
                // Already on line 0 — no-op.
                h
            } else {
                // Cross into the previous logical line.
                let prev_map = layout_line_active(editor, l - 1);
                let prev_ls = derive::line_start(&editor.active().document.buffer, l - 1);
                let c = layout::enter_from_bottom(&prev_map, desired);
                prev_ls + c.offset
            }
        }
    }
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
    if editor.view_opts.typewriter {
        let edit_height = (editor.active().view.area.1 as usize).saturating_sub(1);
        if edit_height == 0 { return; }
        let anchor = editor.view_opts.typewriter_anchor.clamp(0.0, 1.0);
        let anchor_row = ((edit_height as f32 * anchor).round() as usize).min(edit_height - 1);
        // caret's absolute visual row = (visual rows of all logical lines before its line) + its vrow
        let l = caret_line(editor);
        let cvr = caret_visual_row(editor, l);
        let mut caret_abs = cvr;
        for li in 0..l { caret_abs += rows_of_line(editor, li); }
        // desired viewport-top absolute visual row
        let target_top = caret_abs.saturating_sub(anchor_row);
        // convert target_top → (scroll, scroll_row), walking logical lines
        let mut acc = 0usize; let mut scroll = 0usize; let mut scroll_row = 0usize;
        let total = derive::total_logical_lines(&editor.active().document.buffer);
        'outer: for li in 0..total {
            let rows = rows_of_line(editor, li);
            if acc + rows > target_top { scroll = li; scroll_row = target_top - acc; break 'outer; }
            acc += rows; scroll = li; scroll_row = rows.saturating_sub(1);
        }
        editor.active_mut().view.scroll = scroll;
        editor.active_mut().view.scroll_row = scroll_row;
        return;
    }
    let l = caret_line(editor);
    let total = derive::total_logical_lines(&editor.active().document.buffer);
    // Editing area excludes the bottom status row (render reserves frame_h - 1),
    // so nav must reserve it too — else a caret on the last row is deemed visible
    // but never painted/cursor-placed. view.area is the FULL terminal size.
    let area_height = (editor.active().view.area.1 as usize).saturating_sub(1);

    // Clamp scroll to valid range first
    let max_scroll = total.saturating_sub(1);
    if editor.active().view.scroll > max_scroll {
        editor.active_mut().view.scroll = max_scroll;
    }
    let scroll_rows = rows_of_line(editor, editor.active().view.scroll);
    if editor.active().view.scroll_row >= scroll_rows {
        editor.active_mut().view.scroll_row = scroll_rows.saturating_sub(1);
    }

    let cvr = caret_visual_row(editor, l);

    // If caret is above the scroll, scroll up to caret line
    if l < editor.active().view.scroll || (l == editor.active().view.scroll && cvr < editor.active().view.scroll_row) {
        editor.active_mut().view.scroll = l;
        editor.active_mut().view.scroll_row = cvr;
        return;
    }

    if area_height == 0 {
        return;
    }

    let Some(mut rows_before) = rows_before_caret(editor, l, cvr) else {
        editor.active_mut().view.scroll = l;
        editor.active_mut().view.scroll_row = cvr;
        return;
    };

    while rows_before >= area_height {
        advance_view_top_one_row(editor, max_scroll);
        rows_before = rows_before.saturating_sub(1);
    }
}

fn rows_of_line(editor: &Editor, line_idx: usize) -> usize {
    if let Some((_, map)) = editor.active().view.line_layouts.get(&line_idx) {
        map.rows.max(1)
    } else {
        layout_line_on_demand(editor, line_idx).rows.max(1)
    }
}

fn caret_visual_row(editor: &Editor, line_idx: usize) -> usize {
    let buf = &editor.active().document.buffer;
    let map = get_or_layout(editor, line_idx);
    let line_off = derive::line_start(buf, line_idx);
    let in_off = head(editor).saturating_sub(line_off);
    let snapped = map.snap_to_stop(in_off);
    map.source_to_visual(snapped).0
}

fn rows_before_caret(editor: &Editor, caret_line: usize, caret_vrow: usize) -> Option<usize> {
    let scroll = editor.active().view.scroll;
    let scroll_row = editor.active().view.scroll_row;

    if caret_line < scroll {
        return None;
    }
    if caret_line == scroll {
        return caret_vrow.checked_sub(scroll_row);
    }

    let mut rows_before = rows_of_line(editor, scroll).saturating_sub(scroll_row);
    for line_idx in (scroll + 1)..caret_line {
        rows_before += rows_of_line(editor, line_idx);
    }
    Some(rows_before + caret_vrow)
}

fn advance_view_top_one_row(editor: &mut Editor, max_scroll: usize) {
    let rows = rows_of_line(editor, editor.active().view.scroll);
    editor.active_mut().view.scroll_row += 1;
    if editor.active().view.scroll_row >= rows && editor.active().view.scroll < max_scroll {
        editor.active_mut().view.scroll += 1;
        editor.active_mut().view.scroll_row = 0;
    } else if editor.active().view.scroll_row >= rows {
        editor.active_mut().view.scroll_row = rows.saturating_sub(1);
    }
}

/// Advance the viewport top down by one visual row (wraps `advance_view_top_one_row`).
pub fn scroll_down_one(editor: &mut Editor) {
    let total = derive::total_logical_lines(&editor.active().document.buffer);
    let max_scroll = total.saturating_sub(1);
    advance_view_top_one_row(editor, max_scroll);
}

/// Move the viewport top up by one visual row: decrement scroll_row, or cross
/// to the previous logical line's last visual row.
pub fn scroll_up_one(editor: &mut Editor) {
    let (scroll, scroll_row) = { let v = &editor.active().view; (v.scroll, v.scroll_row) };
    if scroll_row > 0 {
        editor.active_mut().view.scroll_row = scroll_row - 1;
    } else if scroll > 0 {
        let prev = scroll - 1;
        let rows = rows_of_line(editor, prev); // immutable borrow ends here
        let v = &mut editor.active_mut().view;
        v.scroll = prev;
        v.scroll_row = rows.saturating_sub(1);
    }
}

// ---------------------------------------------------------------------------
// Paragraph span helpers (consumed by Tasks 5/6/7)
// ---------------------------------------------------------------------------

/// Deepest block whose span contains `pos`, searching children first so a
/// list item / blockquote paragraph wins over its container.
fn deepest_block_at(block: &Block, pos: usize) -> Option<&Block> {
    if !(pos >= block.span.start && pos < block.span.end) {
        return None;
    }
    for child in &block.children {
        if let Some(b) = deepest_block_at(child, pos) {
            return Some(b);
        }
    }
    Some(block)
}

/// The (from, to) paragraph span at `pos`. Total over the document: a leaf
/// block if `pos` is inside one, else the blank-line-delimited run around
/// `pos` (the gap fallback).
pub fn paragraph_range_at(blocks: &BlockTree, buf: &TextBuffer, pos: usize) -> (usize, usize) {
    let pos = pos.min(buf.len());
    for top in blocks.top_level() {
        if let Some(b) = deepest_block_at(top, pos) {
            return (b.span.start, b.span.end);
        }
    }
    // Gap fallback: expand to the maximal run of non-blank logical lines.
    let total = derive::total_logical_lines(buf);
    if total == 0 {
        return (0, 0);
    }
    // pos may equal buf.len() (past the last byte); clamp to len-1 so byte_to_line gets a valid index.
    let line = buf.byte_to_line(pos.min(buf.len().saturating_sub(1)));
    let is_blank = |l: usize| derive::line_text(buf, l).trim().is_empty();
    if is_blank(line) {
        let s = derive::line_start(buf, line);
        return (s, s); // empty range on a blank line
    }
    let mut top_line = line;
    while top_line > 0 && !is_blank(top_line - 1) {
        top_line -= 1;
    }
    let mut bot_line = line;
    while bot_line + 1 < total && !is_blank(bot_line + 1) {
        bot_line += 1;
    }
    let from = derive::line_start(buf, top_line);
    let to = derive::line_start(buf, bot_line) + derive::line_text(buf, bot_line).len();
    (from, to)
}

/// Depth-first leaf-block spans in document order (a "paragraph" for motion).
fn collect_leaf_spans(block: &Block, out: &mut Vec<(usize, usize)>) {
    if block.children.is_empty() {
        out.push((block.span.start, block.span.end));
    } else {
        for c in &block.children { collect_leaf_spans(c, out); }
    }
}

fn leaf_spans(blocks: &BlockTree) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    for top in blocks.top_level() { collect_leaf_spans(top, &mut v); }
    v.sort_by_key(|s| s.0);
    v
}

/// Start of the next leaf block beginning strictly after `pos`, else `buf.len()`.
pub fn next_paragraph_start(blocks: &BlockTree, buf: &TextBuffer, pos: usize) -> usize {
    leaf_spans(blocks).into_iter().map(|(s, _)| s).find(|&s| s > pos).unwrap_or(buf.len())
}

/// Start of the leaf block before the one containing `pos`, else `0`.
pub fn prev_paragraph_start(blocks: &BlockTree, _buf: &TextBuffer, pos: usize) -> usize {
    // caller passes the *current paragraph start* as `pos` boundary; pick the
    // last leaf start strictly before it.
    leaf_spans(blocks).into_iter().map(|(s, _)| s).filter(|&s| s < pos).next_back().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Paragraph / page / document navigation (Task 6)
// ---------------------------------------------------------------------------

pub fn move_paragraph_down(editor: &mut Editor) -> usize {
    let h = head(editor);
    let result = {
        let buf = &editor.active().document.buffer;
        let blocks = &editor.active().document.blocks;
        next_paragraph_start(blocks, buf, h) // next leaf-block start, else buf.len()
    };
    editor.active_mut().desired_col = None;
    result
}

pub fn move_paragraph_up(editor: &mut Editor) -> usize {
    let h = head(editor);
    let result = {
        let buf = &editor.active().document.buffer;
        let blocks = &editor.active().document.blocks;
        let (from, _to) = paragraph_range_at(blocks, buf, h);
        if from < h { from } else { prev_paragraph_start(blocks, buf, from) }
    };
    editor.active_mut().desired_col = None;
    result
}

pub fn move_doc_start(editor: &mut Editor) -> usize { editor.active_mut().desired_col = None; 0 }

pub fn move_doc_end(editor: &mut Editor) -> usize {
    let len = editor.active().document.buffer.len();
    editor.active_mut().desired_col = None;
    len
}

/// Page step: editing_height − 1 for one row of context overlap.
/// `editing_height = area.1 - 1` (the status row is reserved — matches nav.rs:62).
fn page_step(editor: &Editor) -> usize {
    let editing_height = (editor.active().view.area.1 as usize).saturating_sub(1);
    editing_height.saturating_sub(1).max(1)
}

pub fn move_page_down(editor: &mut Editor) -> usize {
    let steps = page_step(editor);
    let mut off = head(editor);
    for _ in 0..steps {
        let next = move_down(editor); // preserves desired_col across the run
        if next == off { break; }
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(next);
        off = next;
    }
    off
}

pub fn move_page_up(editor: &mut Editor) -> usize {
    let steps = page_step(editor);
    let mut off = head(editor);
    for _ in 0..steps {
        let next = move_up(editor);
        if next == off { break; }
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(next);
        off = next;
    }
    off
}

/// Move to the start of the next word, crossing block boundaries (skipping gaps).
pub fn move_word_right(editor: &mut Editor) -> usize {
    let h = head(editor);
    let new = {
        let buf = &editor.active().document.buffer;
        let blocks = &editor.active().document.blocks;
        let (wstart, wend) = paragraph_range_at(blocks, buf, h);
        let window = buf.slice(wstart..wend);
        let rel = h.saturating_sub(wstart);
        match wordcartel_core::textobj::next_word_start(&window, rel) {
            Some(r) => wstart + r,
            None => {
                // First word of the next leaf block (skips blank-line gaps), else doc end.
                let nps = next_paragraph_start(blocks, buf, wend);
                if nps >= buf.len() {
                    buf.len()
                } else {
                    let next_end = paragraph_range_at(blocks, buf, nps).1;
                    let ntext = buf.slice(nps..next_end);
                    wordcartel_core::textobj::next_word_start(&ntext, 0)
                        .map(|r| nps + r)
                        .unwrap_or(nps) // block starts with its first word
                }
            }
        }
    };
    editor.active_mut().desired_col = None;
    new
}

/// Move to the start of the previous word, crossing block boundaries (skipping gaps).
pub fn move_word_left(editor: &mut Editor) -> usize {
    let h = head(editor);
    let new = {
        let buf = &editor.active().document.buffer;
        let blocks = &editor.active().document.blocks;
        let (wstart, wend) = paragraph_range_at(blocks, buf, h);
        let window = buf.slice(wstart..wend);
        let rel = h.saturating_sub(wstart);
        match wordcartel_core::textobj::prev_word_start(&window, rel) {
            Some(r) => wstart + r,
            None if wstart > 0 => {
                let pps = prev_paragraph_start(blocks, buf, wstart);
                let prev_end = paragraph_range_at(blocks, buf, pps).1;
                let ptext = buf.slice(pps..prev_end);
                wordcartel_core::textobj::prev_word_start(&ptext, ptext.len())
                    .map(|r| pps + r)
                    .unwrap_or(pps)
            }
            None => 0,
        }
    };
    editor.active_mut().desired_col = None;
    new
}

// ---------------------------------------------------------------------------
// Cell → offset reverse map (inverse of screen_pos)
// ---------------------------------------------------------------------------

/// Inverse of `screen_pos`: the document byte offset under screen cell
/// `(col, row)` in the editing area, or `None` if `row` is past content.
pub fn offset_at_cell(editor: &Editor, col: u16, row: u16) -> Option<usize> {
    // Subtract the measure margin so callers pass raw screen columns; a click
    // left of the text column saturates to 0 (= line start of that row).
    let text_left = text_geometry(editor).text_left;
    let col = col.saturating_sub(text_left);
    let target = row as usize;
    let scroll = editor.active().view.scroll;
    let scroll_row = editor.active().view.scroll_row;
    let total = derive::total_logical_lines(&editor.active().document.buffer);
    let mut acc = 0usize; // visible rows consumed
    let mut line = scroll;
    while line < total {
        let rows = rows_of_line(editor, line);
        let first_vrow = if line == scroll { scroll_row } else { 0 };
        for vrow in first_vrow..rows {
            if acc == target {
                let map = get_or_layout(editor, line);
                let in_off = map.visual_to_source(vrow, col as usize);
                let snapped = map.snap_to_stop(in_off);
                return Some(derive::line_start(&editor.active().document.buffer, line) + snapped);
            }
            acc += 1;
        }
        line += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{ensure_visible, move_down, move_up, move_home, move_end, move_left, move_right, screen_pos};
    use crate::derive;
    use crate::editor::Editor;
    use wordcartel_core::selection::Selection;

    /// Test helper: set the caret to a raw byte offset.
    fn set_caret(e: &mut Editor, off: usize) {
        e.active_mut().document.selection = Selection::single(off);
    }

    // ------------------------------------------------------------------
    // Task 3: text_geometry + measure round-trip (RED → GREEN)
    // ------------------------------------------------------------------

    #[test]
    fn text_geometry_centers_when_measure_on() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        let g = super::text_geometry(&e);
        assert_eq!((g.text_left, g.text_width), (0, 80), "measure off → full width");
        e.view_opts.measure = true; e.view_opts.wrap_column = 40;
        let g = super::text_geometry(&e);
        assert_eq!((g.text_left, g.text_width), (20, 40), "centered 40-wide column");
        // narrow terminal: measure inert
        e.active_mut().view.area = (30, 24);
        let g = super::text_geometry(&e);
        assert_eq!((g.text_left, g.text_width), (0, 30), "vp <= column → full width");
    }

    #[test]
    fn screen_pos_and_offset_at_cell_round_trip_with_measure() {
        let mut e = Editor::new_from_text("abc\ndef\n", None, (80, 24));
        e.view_opts.measure = true; e.view_opts.wrap_column = 40; // text_left = 20
        set_caret(&mut e, 5); // 'e' in "def" (line 1, text-col 1)
        derive::rebuild(&mut e);
        let (vcol, vrow) = screen_pos(&e).unwrap();
        // the actual SCREEN cell is (text_left + vcol, vrow)
        assert_eq!(super::offset_at_cell(&e, 20 + vcol, vrow), Some(5));
        // a click in the LEFT margin clamps to line start of that row
        // "abc\n" = 4 bytes, so "def" line starts at offset 4
        let def_line_start = crate::derive::line_start(&e.active().document.buffer, 1);
        assert_eq!(super::offset_at_cell(&e, 3, vrow), Some(def_line_start));
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
        let line50_start = derive::line_start(&e.active().document.buffer, 50);
        set_caret(&mut e, line50_start);
        ensure_visible(&mut e);
        derive::rebuild(&mut e);
        assert!(screen_pos(&e).is_some());
        assert!(e.active().view.scroll <= 50 && e.active().view.scroll + 10 > 50);
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
    fn caret_in_tall_wrapped_line_stays_visible() {
        let mut e = Editor::new_from_text(&"a".repeat(30), None, (3, 5));
        set_caret(&mut e, 25);
        ensure_visible(&mut e);
        derive::rebuild(&mut e);

        let pos = screen_pos(&e);
        assert!(pos.is_some(), "expected visible caret, got None");
        let (_col, row) = pos.unwrap();
        assert!(row < 4, "caret row {row} should fit in editing height 4");
    }

    #[test]
    fn short_doc_keeps_scroll_row_zero() {
        let mut e = Editor::new_from_text("one\ntwo\n", None, (80, 10));
        set_caret(&mut e, 5);
        ensure_visible(&mut e);
        derive::rebuild(&mut e);

        assert_eq!(e.active().view.scroll_row, 0);
        assert!(screen_pos(&e).is_some());
    }

    #[test]
    fn caret_above_scroll_returns_none() {
        let mut e = Editor::new_from_text("line0\nline1\nline2\n", None, (80, 24));
        set_caret(&mut e, 0); // caret on line 0
        e.active_mut().view.scroll = 2;   // scroll past caret
        derive::rebuild(&mut e);
        assert_eq!(screen_pos(&e), None);
    }

    #[test]
    fn ensure_visible_scrolls_up_when_caret_above() {
        let text: String = (0..20).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 10));
        e.active_mut().view.scroll = 15;  // scroll to near end
        set_caret(&mut e, 0); // caret at very top
        ensure_visible(&mut e);
        assert_eq!(e.active().view.scroll, 0, "scroll should have gone back to 0");
    }

    // ------------------------------------------------------------------
    // Task 7: Vertical navigation (RED → GREEN)
    // ------------------------------------------------------------------

    #[test]
    fn down_preserves_column_across_lines() {
        let mut e = Editor::new_from_text("hello\nworld\n", None, (80, 24));
        set_caret(&mut e, 3); // col 3 on "hello" ('l'); desired_col starts None
        derive::rebuild(&mut e);
        let n = move_down(&mut e); // first vertical move computes desired_col=3 -> "world" col 3 -> offset 9
        assert_eq!(n, 9);
        assert_eq!(e.active().desired_col, Some(3));
    }

    #[test]
    fn down_within_wrapped_line_stays_in_line() {
        // narrow width forces "aaaaaa" to wrap; down moves to the 2nd visual row, same logical line
        let mut e = Editor::new_from_text("aaaaaa\nz\n", None, (3, 24));
        set_caret(&mut e, 0); // desired_col None
        derive::rebuild(&mut e);
        let n = move_down(&mut e);
        assert!(n > 0 && n < 6); // still inside the first logical line's wrapped rows
    }

    #[test]
    fn up_then_down_round_trip() {
        // Start on line 1, move up to line 0, then back down; desired_col preserved throughout.
        let mut e = Editor::new_from_text("hello\nworld\n", None, (80, 24));
        set_caret(&mut e, 8); // 'r' in "world", col 2
        derive::rebuild(&mut e);
        let up_pos = move_up(&mut e); // -> "hello" col 2 -> offset 2
        assert_eq!(up_pos, 2);
        assert_eq!(e.active().desired_col, Some(2));
        set_caret(&mut e, up_pos); // apply the move so move_down starts from offset 2
        let down_pos = move_down(&mut e); // -> back to "world" col 2 -> offset 8
        assert_eq!(down_pos, 8);
        assert_eq!(e.active().desired_col, Some(2)); // still preserved
    }

    // ------------------------------------------------------------------
    // Task 2: paragraph_range_at (RED → GREEN)
    // ------------------------------------------------------------------

    #[test]
    fn paragraph_range_selects_leaf_block_not_container() {
        // A list: paragraph_range at a list item must select the ITEM span,
        // not the whole list container.
        let mut e = Editor::new_from_text("- one\n- two\n\nAfter\n", None, (80, 24));
        derive::rebuild(&mut e);
        let buf = &e.active().document.buffer;
        let blocks = &e.active().document.blocks;
        // pos inside "two" (second list item)
        let pos = 8;
        let (from, to) = super::paragraph_range_at(blocks, buf, pos);
        let slice = buf.slice(from..to);
        assert!(slice.contains("two") && !slice.contains("one"),
            "expected the 'two' item span, got {slice:?}");
    }

    #[test]
    fn paragraph_range_gap_falls_back_to_blank_delimited_run() {
        // "A\n\nB\n" — pos on the blank line (offset 2) has no block span;
        // fallback returns an empty/whitespace range (no panic), and a pos in
        // paragraph "B" returns the B line range.
        let mut e = Editor::new_from_text("A\n\nB\n", None, (80, 24));
        derive::rebuild(&mut e);
        let buf = &e.active().document.buffer;
        let blocks = &e.active().document.blocks;
        let (bf, bt) = super::paragraph_range_at(blocks, buf, 3); // inside "B"
        assert_eq!(buf.slice(bf..bt).trim(), "B");
        // gap: must not panic and must yield a valid (from<=to<=len) range
        let (gf, gt) = super::paragraph_range_at(blocks, buf, 2);
        assert!(gf <= gt && gt <= buf.len());
    }

    #[test]
    fn desired_col_survives_ragged_short_line() {
        // Classic ragged-column case: descending through a SHORT middle line must
        // snap to its end but NOT lose the column — the next descent restores it.
        let mut e = Editor::new_from_text("hello\nhi\nworld\n", None, (80, 24));
        set_caret(&mut e, 4); // 'o' in "hello", col 4; desired_col None
        derive::rebuild(&mut e);
        let p1 = move_down(&mut e); // first vertical: desired=4; "hi" max col 2 -> offset 8
        assert_eq!(p1, 8);
        assert_eq!(e.active().desired_col, Some(4));
        set_caret(&mut e, p1);
        derive::rebuild(&mut e);
        let p2 = move_down(&mut e); // desired still 4 -> "world" col 4 -> offset 13 (NOT col 2)
        assert_eq!(p2, 13);
        assert_eq!(e.active().desired_col, Some(4));
    }

    // ------------------------------------------------------------------
    // Task 11: offset_at_cell (RED → GREEN)
    // ------------------------------------------------------------------

    #[test]
    fn offset_at_cell_inverts_screen_pos() {
        let mut e = Editor::new_from_text("abc\ndef\n", None, (80, 24));
        set_caret(&mut e, 5); // 'e' on line 1, col 1
        derive::rebuild(&mut e);
        let (col, row) = screen_pos(&e).unwrap();
        assert_eq!(super::offset_at_cell(&e, col, row), Some(5));
    }

    // ------------------------------------------------------------------
    // Task 5: typewriter scrolling
    // ------------------------------------------------------------------

    #[test]
    fn typewriter_pins_caret_to_anchor_row() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 21)); // edit_height = 20
        e.view_opts.typewriter = true; e.view_opts.typewriter_anchor = 0.5; // anchor_row = 10
        let l50 = derive::line_start(&e.active().document.buffer, 50);
        set_caret(&mut e, l50);
        ensure_visible(&mut e);
        derive::rebuild(&mut e);
        let (_c, row) = screen_pos(&e).unwrap();
        assert_eq!(row, 10, "caret pinned to anchor row 10");
        // near the top, caret sits ABOVE the anchor (can't scroll past 0)
        let l2 = derive::line_start(&e.active().document.buffer, 2);
        set_caret(&mut e, l2);
        ensure_visible(&mut e);
        derive::rebuild(&mut e);
        assert_eq!(e.active().view.scroll, 0, "top clamps; no scroll past 0");
    }
}
