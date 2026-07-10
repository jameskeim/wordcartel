//! Pure logical-line helpers over `TextBuffer`: line count, line-start byte offset,
//! line text (trailing '\n' stripped), and the render-mode → `LineRender` mapping.
//! No dependence on the derive pipeline (`rebuild`/`LayoutKey`/caches).

use wordcartel_core::buffer::TextBuffer;

/// Map the view's render mode + whether this is the caret line → LineRender.
/// LivePreview conceals inactive lines and shows the active line raw+plain;
/// SourceHighlighted styles every line raw; SourcePlain shows every line raw+plain.
pub(crate) fn line_render_for(mode: crate::editor::RenderMode, is_active_line: bool)
    -> wordcartel_core::style::LineRender
{
    use crate::editor::RenderMode::*;
    use wordcartel_core::style::LineRender::*;
    match mode {
        LivePreview       => if is_active_line { RawPlain } else { Concealed },
        SourceHighlighted => RawStyled,
        SourcePlain       => RawPlain,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
