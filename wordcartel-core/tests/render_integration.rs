//! Render-core integration: split a buffer into logical lines, lay each out,
//! and verify cross-line vertical cursor motion preserves desired column.
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::layout::{enter_from_top, layout, Cursor, move_down_within};
use wordcartel_core::style::BlockRole;

#[test]
fn cursor_crosses_logical_lines_at_desired_col() {
    // Two logical lines (paragraphs separated by \n). Treat each \n-delimited
    // line as one logical line (block role Paragraph for this test).
    let buf = TextBuffer::from_str("hello world\ngoodbye");
    let text = buf.to_string();
    let lines: Vec<&str> = text.split('\n').collect();

    let (_r0, map0) = layout(lines[0], BlockRole::Paragraph, true, 80, false);
    let (_r1, map1) = layout(lines[1], BlockRole::Paragraph, false, 80, false);

    // Cursor on line 0 at byte 6 = 'w' of "hello world", visual col 6. Move down
    // off the end of line 0 (single visual row) -> enter line 1 from the top,
    // preserving desired_col 6.
    let on0 = Cursor { offset: 6, row: 0, desired_col: 6 };
    assert!(move_down_within(&map0, on0).is_none()); // line 0 is one visual row
    let on1 = enter_from_top(&map1, on0.desired_col);
    // "goodbye": g0 o1 o2 d3 b4 y5 e6 (len 7). col 6 -> 'e' at byte 6.
    assert_eq!(on1.offset, 6);
    assert_eq!(on1.row, 0);
}

#[test]
fn concealed_line_renders_styled() {
    let buf = TextBuffer::from_str("a **bold** end");
    let line = buf.to_string();
    let (rows, _map) = layout(&line, BlockRole::Paragraph, false, 80, false);
    assert_eq!(rows[0].display, "a bold end");
    assert!(rows[0].segs.iter().any(|s| s.style == wordcartel_core::style::Style::Strong));
}
