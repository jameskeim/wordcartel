use crate::editor::{Editor, RenderMode};
use wordcartel_core::block_tree;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::layout;

// ---------------------------------------------------------------------------
// Logical-line helpers
// ---------------------------------------------------------------------------

/// Total number of logical lines in `buf`.
///
/// Edge-case rules:
///   ""    → 1   (the document always has at least one line)
///   "a"   → 1
///   "a\n" → 2   (trailing newline creates a real empty line after it)
///   "\n"  → 2
///   "a\nb"→ 2
///
/// ropey's `len_lines()` follows the convention we want: it returns the number
/// of LF-delimited lines where a trailing `\n` contributes an extra empty line.
pub fn total_logical_lines(buf: &TextBuffer) -> usize {
    // ropey uses LF-only semantics for len_lines when the unicode_lines feature
    // is disabled (the default). We double-check: for a buffer whose content
    // ends in '\n', ropey's len_lines is len_lines_lf = text.split('\n').count()
    // which counts the trailing empty field. That matches our spec.
    let rope = buf.snapshot();
    rope.len_lines()
}

/// Byte offset of the start of logical line `L` in `buf`.
///
/// For `L < total_logical_lines(buf)`:  `buf.line_to_byte(L)`.
/// For `L == total_logical_lines(buf)`: clamped to `buf.len()` (one-past-end guard).
pub fn line_start(buf: &TextBuffer, line: usize) -> usize {
    let total = total_logical_lines(buf);
    if line < total {
        buf.line_to_byte(line)
    } else {
        buf.len()
    }
}

/// Content of logical line `L` as a `String`, **without** its trailing `\n`.
///
/// For any `L` in `0..total_logical_lines(buf)`.
pub fn line_text(buf: &TextBuffer, line: usize) -> String {
    let start = line_start(buf, line);
    let total = total_logical_lines(buf);
    let raw_end = if line + 1 < total {
        line_start(buf, line + 1)
    } else {
        buf.len()
    };
    // Strip a single trailing '\n' if present (it's the line separator, not content).
    let text = buf.slice(start..raw_end);
    if text.ends_with('\n') {
        text[..text.len() - 1].to_string()
    } else {
        text
    }
}

// ---------------------------------------------------------------------------
// derive::rebuild
// ---------------------------------------------------------------------------

/// Recompute the block tree and per-visible-line layout cache from truth.
///
/// This is the O(visible)+O(edited) derive step described in Effort 4a Task 3.
///
/// # Block tree
/// If `editor.last_edit` and `editor.pre_edit_rope` are both `Some` (set by
/// `apply`), we use the O(region) incremental reparse.  Otherwise (initial
/// load, undo, redo) we fall back to a full parse.  Either way, we clear
/// `last_edit` and `pre_edit_rope` before returning so they are not reused.
///
/// # Visible range + layout cache
/// We walk logical lines starting at `view.scroll`, accumulating the visual-row
/// heights reported by `ColMap.rows`, until we have filled the editing area
/// height (+1 row of overscan).  For each visible logical line we call
/// `layout::layout` and store the result in `view.line_layouts`.
pub fn rebuild(editor: &mut Editor) {
    // ------------------------------------------------------------------
    // 1. Block tree (incremental or full)
    // ------------------------------------------------------------------
    let new_rope = editor.document.buffer.snapshot(); // O(1) ropey clone

    // Take the option values out so we can clear them unconditionally after.
    let maybe_old_rope = editor.pre_edit_rope.take();
    let maybe_edit = editor.last_edit.take();

    editor.document.blocks = match (maybe_old_rope, maybe_edit) {
        (Some(old_rope), Some(edit)) => {
            // Normal edit path: O(region) incremental reparse.
            block_tree::incremental_update_rope(
                &editor.document.blocks,
                &old_rope,
                &edit,
                &new_rope,
            )
        }
        _ => {
            // Initial load, undo, redo, or any state where we lack edit info.
            block_tree::full_parse_rope(&new_rope)
        }
    };
    // last_edit and pre_edit_rope were already cleared by .take() above.

    // ------------------------------------------------------------------
    // 2. Visible range
    // ------------------------------------------------------------------
    let buf = &editor.document.buffer;
    let total_lines = total_logical_lines(buf);
    let (area_width, area_height) = (editor.view.area.0 as usize, editor.view.area.1 as usize);
    let vp_width = area_width.max(1);

    // Determine the active logical line from the caret position.
    let caret_byte = editor.document.selection.primary().head;
    let active_line = if buf.len() == 0 {
        0
    } else {
        buf.byte_to_line(caret_byte.min(buf.len().saturating_sub(1)))
    };

    // Walk from scroll, accumulating visual rows until we fill area_height + 1 overscan.
    let first_line = editor.view.scroll.min(total_lines.saturating_sub(1));
    let mut visual_rows_accumulated: usize = 0;
    let overscan_budget = area_height.saturating_add(1);

    // Clear the old cache and fill for the visible range.
    editor.view.line_layouts.clear();

    // In source modes (SourceHighlighted, SourcePlain), ALL lines render raw
    // (markers visible, conceal off). Laying out with is_active=true achieves
    // this because active-line layout uses the raw/identity-ish col_map.
    // §3.11: source modes are cheaper — pass is_active_effective = true for all lines.
    let source_mode = editor.view.mode != RenderMode::LivePreview;

    let mut l = first_line;
    while l < total_lines && visual_rows_accumulated < overscan_budget {
        let text = line_text(buf, l);
        let role = editor.document.blocks.role_at(line_start(buf, l));
        let is_active_effective = (l == active_line) || source_mode;
        let (rows, map) = layout::layout(&text, role, is_active_effective, vp_width);
        visual_rows_accumulated += rows.len();
        editor.view.line_layouts.insert(l, (rows, map));
        l += 1;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use wordcartel_core::style::BlockRole;

    // ------------------------------------------------------------------
    // Brief's failing tests (write RED first, then implement GREEN)
    // ------------------------------------------------------------------

    /// Inactive heading line shows concealed display (e.g. "Title", not "# Title").
    #[test]
    fn derive_lays_out_visible_lines_with_roles() {
        let mut e = Editor::new_from_text("# Title\n\nplain body\n", None, (80, 24));
        // Move cursor to the blank line (byte 8 = '\n' of blank line) so that
        // line 0 (the heading) is NOT the active line — it should show concealed.
        // "# Title\n" is 8 bytes; the blank line '\n' starts at byte 8.
        e.document.selection = wordcartel_core::selection::Selection::single(8);
        rebuild(&mut e);
        let (rows0, _) = &e.view.line_layouts[&0];
        // inactive heading line -> "# " concealed -> "Title"
        assert_eq!(rows0[0].display, "Title");
        assert_eq!(rows0[0].role, BlockRole::Heading(1));
    }

    /// The cursor's line (active) shows raw markdown, not concealed display.
    #[test]
    fn active_line_renders_raw() {
        let mut e = Editor::new_from_text("# Title\n", None, (80, 24));
        // cursor at 0 -> line 0 active -> raw "# Title"
        rebuild(&mut e);
        let (rows0, _) = &e.view.line_layouts[&0];
        assert_eq!(rows0[0].display, "# Title");
    }

    // ------------------------------------------------------------------
    // Wrap: a long line at narrow width produces multiple visual rows.
    // ------------------------------------------------------------------

    #[test]
    fn long_line_wraps_at_small_width() {
        // 20-char line, viewport width 5 -> at least 4 rows
        let mut e = Editor::new_from_text("abcdefghijklmnopqrst\n", None, (5, 24));
        rebuild(&mut e);
        let (rows, _) = &e.view.line_layouts[&0];
        assert!(rows.len() > 1, "expected wrapping, got {} row(s)", rows.len());
    }

    // ------------------------------------------------------------------
    // Logical-line edge-case helpers
    // ------------------------------------------------------------------

    fn buf(s: &str) -> TextBuffer {
        TextBuffer::from_str(s)
    }

    #[test]
    fn total_lines_empty_is_one() {
        assert_eq!(total_logical_lines(&buf("")), 1);
    }

    #[test]
    fn total_lines_no_newline() {
        assert_eq!(total_logical_lines(&buf("a")), 1);
    }

    #[test]
    fn total_lines_trailing_newline_is_two() {
        assert_eq!(total_logical_lines(&buf("a\n")), 2);
    }

    #[test]
    fn total_lines_lone_newline() {
        assert_eq!(total_logical_lines(&buf("\n")), 2);
    }

    #[test]
    fn total_lines_two_lines_no_trailing_newline() {
        assert_eq!(total_logical_lines(&buf("a\nb")), 2);
    }

    #[test]
    fn line_start_positions() {
        let b = buf("a\nb\n");
        // 4 bytes: a(0) \n(1) b(2) \n(3)
        // line 0 starts at 0, line 1 at 2, line 2 (trailing empty) at 4 (== len)
        assert_eq!(line_start(&b, 0), 0);
        assert_eq!(line_start(&b, 1), 2);
        assert_eq!(line_start(&b, 2), 4); // total_logical_lines == 2, so line 2 == buf.len()
    }

    #[test]
    fn line_text_strips_newline() {
        let b = buf("hello\nworld\n");
        assert_eq!(line_text(&b, 0), "hello");
        assert_eq!(line_text(&b, 1), "world");
        assert_eq!(line_text(&b, 2), ""); // trailing empty line
    }

    #[test]
    fn line_text_empty_buffer() {
        let b = buf("");
        assert_eq!(line_text(&b, 0), "");
    }

    #[test]
    fn line_text_no_trailing_newline() {
        let b = buf("abc");
        assert_eq!(line_text(&b, 0), "abc");
    }

    #[test]
    fn line_text_lone_newline() {
        let b = buf("\n");
        assert_eq!(line_text(&b, 0), "");
        assert_eq!(line_text(&b, 1), "");
    }

    #[test]
    fn line_text_multibyte() {
        // "é\nz\n" — é is 2 bytes
        let b = buf("é\nz\n");
        assert_eq!(line_text(&b, 0), "é");
        assert_eq!(line_text(&b, 1), "z");
        assert_eq!(line_text(&b, 2), "");
        // total = 3 lines
        assert_eq!(total_logical_lines(&b), 3);
    }

    // ------------------------------------------------------------------
    // Incremental path: last_edit+pre_edit_rope → incremental_update_rope
    // Full parse path:  neither Some → full_parse_rope
    // ------------------------------------------------------------------

    #[test]
    fn rebuild_uses_full_parse_when_no_edit() {
        // On a fresh Editor (no prior apply), rebuild must not panic and the
        // block tree must reflect the document content.
        let mut e = Editor::new_from_text("# Hi\n\nbody\n", None, (80, 24));
        assert!(e.last_edit.is_none());
        assert!(e.pre_edit_rope.is_none());
        rebuild(&mut e);
        // After rebuild, the two option fields must be cleared (take() consumed them).
        assert!(e.last_edit.is_none());
        assert!(e.pre_edit_rope.is_none());
        // Block tree must reflect the heading.
        use wordcartel_core::style::BlockRole;
        assert_eq!(e.document.blocks.role_at(0), BlockRole::Heading(1));
    }

    #[test]
    fn rebuild_clears_pre_edit_rope_and_last_edit() {
        // After any rebuild call the two option fields must be None.
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        // Manually set them to Some to simulate a post-apply state.
        e.pre_edit_rope = Some(e.document.buffer.snapshot());
        e.last_edit = Some(wordcartel_core::block_tree::Edit { range: 0..0, new_len: 0 });
        rebuild(&mut e);
        assert!(e.pre_edit_rope.is_none(), "pre_edit_rope should be cleared after rebuild");
        assert!(e.last_edit.is_none(), "last_edit should be cleared after rebuild");
    }
}
