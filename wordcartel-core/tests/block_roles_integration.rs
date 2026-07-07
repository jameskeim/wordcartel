//! End-to-end block-role rendering: parse a multi-block doc, derive each logical
//! line's role from block_tree, lay it out, and verify prefixes are concealed and
//! roles/glyphs are correct.
use wordcartel_core::block_tree::full_parse;
use wordcartel_core::layout::layout;
use wordcartel_core::style::{BlockRole, LineRender};

#[test]
fn multi_block_doc_renders_with_roles() {
    let doc = "# Title\n\n> a quote\n\n- first\n- second\n\nplain para\n";
    let tree = full_parse(doc);
    // iterate logical lines (\n-delimited), compute each line's role at its start byte.
    let mut offset = 0usize;
    let mut got: Vec<(BlockRole, String, Option<String>)> = Vec::new();
    for line in doc.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if !trimmed.is_empty() {
            let role = tree.role_at(offset);
            let (rows, _m) = layout(trimmed, role, LineRender::Concealed, 80, false);
            let display: String = rows.iter().map(|r| r.display.clone()).collect();
            got.push((role, display, rows[0].prefix_glyph.clone()));
        }
        offset += line.len();
    }
    // Title heading: "# " concealed -> "Title", role Heading(1)
    assert_eq!(got[0], (BlockRole::Heading(1), "Title".into(), None));
    // quote: "> " concealed, prefix glyph ▎ (Task 5)
    assert_eq!(got[1], (BlockRole::BlockQuote, "a quote".into(), Some("▎ ".into())));
    // list items: marker -> bullet glyph
    assert_eq!(got[2], (BlockRole::ListItem, "first".into(), Some("• ".into())));
    assert_eq!(got[3], (BlockRole::ListItem, "second".into(), Some("• ".into())));
    // paragraph: unchanged
    assert_eq!(got[4], (BlockRole::Paragraph, "plain para".into(), None));
}
