//! Pure heading extraction over the block tree. No IO, no threads.
use crate::block_tree::{Block, BlockKind, BlockTree};
use ropey::Rope;
use std::collections::BTreeSet;
use std::ops::Range;

/// A heading in the document.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Heading {
    /// 1..=6, from `BlockKind::Heading(level)`.
    pub level: u8,
    /// Byte offset of the heading's start (the block span start).
    pub byte: usize,
    /// Byte offset of the heading block's end (span end). Covers the heading's
    /// own line(s) — ONE line for ATX, TWO for setext (title + underline) — and
    /// is how `body_range` finds where the foldable body begins.
    pub end: usize,
    /// Title text only (ATX `#`/trailing `#` and setext underline stripped).
    pub text: String,
}

/// All headings in document order (pre-order over the block tree, descending
/// into containers such as block quotes and lists).
pub fn headings(blocks: &BlockTree, rope: &Rope) -> Vec<Heading> {
    let mut out = Vec::new();
    for b in blocks.top_level() {
        collect(b, rope, &mut out);
    }
    out
}

fn collect(b: &Block, rope: &Rope, out: &mut Vec<Heading>) {
    // Match by reference: BlockKind is Clone but not Copy, so `b.kind` would move.
    if let BlockKind::Heading(level) = &b.kind {
        out.push(Heading {
            level: *level,
            byte: b.span.start,
            end: b.span.end,
            text: heading_title(rope, b.span.clone()),
        });
    }
    for c in &b.children {
        collect(c, rope, out);
    }
}

/// Extract a heading's title from its source span, stripping ATX leading `#`s
/// (and optional trailing `#`s) or, for a setext heading, dropping the
/// underline line. Operates on the raw span slice; the result is trimmed.
fn heading_title(rope: &Rope, span: Range<usize>) -> String {
    let raw = rope.byte_slice(span).to_string();
    // First line is the title text for both ATX and setext.
    let first = raw.lines().next().unwrap_or("");
    let t = first.trim();
    if let Some(rest) = t.strip_prefix('#') {
        // ATX: strip the run of leading '#', then a single space, then trailing '#'s.
        let rest = rest.trim_start_matches('#');
        rest.trim().trim_end_matches('#').trim().to_string()
    } else {
        // Setext: the title is the first line verbatim (trimmed).
        t.to_string()
    }
}

/// Document-order list reused by `heading_starts` and `sections`.
fn ordered(blocks: &BlockTree, rope: &Rope) -> Vec<Heading> {
    headings(blocks, rope)
}

/// The canonical set of heading-start byte offsets. `FoldState::reconcile`
/// validates anchors against THIS set (not `block_tree::role_at`, which only
/// classifies a byte's role and cannot prove a byte is a heading *start*).
pub fn heading_starts(blocks: &BlockTree, rope: &Rope) -> BTreeSet<usize> {
    ordered(blocks, rope).into_iter().map(|h| h.byte).collect()
}

/// A heading paired with its foldable body byte-range.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Section {
    /// The section's own heading — the [`Heading`] this section is keyed by, giving its
    /// level and byte range.
    pub heading: Heading,
    /// Body range: first line AFTER the heading's own line(s) .. section end.
    /// Empty (`start == end`) when the section has no body.
    pub body: Range<usize>,
}

/// All sections in document order, each with its body range — computed in ONE
/// pass over the heading list. This is the per-frame-friendly batch API used by
/// `FoldView::compute`; it avoids the
/// O(folds × headings) blow-up of calling `body_range` per folded anchor.
pub fn sections(blocks: &BlockTree, rope: &Rope) -> Vec<Section> {
    let hs = ordered(blocks, rope);
    let doc_end = rope.len_bytes();
    let mut out = Vec::with_capacity(hs.len());
    for (i, h) in hs.iter().enumerate() {
        // section end = next heading with level <= this level, else doc end.
        let section_end = hs[i + 1..]
            .iter()
            .find(|n| n.level <= h.level)
            .map(|n| n.byte)
            .unwrap_or(doc_end);
        // body begins on the first line strictly after the heading's last own
        // line (h.end-1's line). Correct for ATX (1 line) and setext (2 lines).
        let heading_last_line = rope.byte_to_line(h.end.saturating_sub(1).max(h.byte));
        let body_start = rope.line_to_byte(heading_last_line + 1).min(section_end);
        out.push(Section { heading: h.clone(), body: body_start..section_end });
    }
    out
}

/// The foldable BODY range of the heading at `heading_byte`. Single source of
/// body-start math for the COLD paths (`normalize_caret`/`unfold_ancestors_of`/
/// `hidden_count_lines`). For the per-frame path, use `sections` once instead.
/// Returns an empty range when `heading_byte` isn't a heading or has no body.
pub fn body_range(blocks: &BlockTree, rope: &Rope, heading_byte: usize) -> Range<usize> {
    sections(blocks, rope)
        .into_iter()
        .find(|s| s.heading.byte == heading_byte)
        .map(|s| s.body)
        .unwrap_or(heading_byte..heading_byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_tree::full_parse;

    fn rope(s: &str) -> Rope { Rope::from_str(s) }

    #[test]
    fn headings_in_document_order_with_levels_and_text() {
        let doc = "# Title\n\nintro\n\n## A\n\nbody\n\n### A.1\n\n## B\n";
        let t = full_parse(doc);
        let hs = headings(&t, &rope(doc));
        let got: Vec<(u8, &str)> = hs.iter().map(|h| (h.level, h.text.as_str())).collect();
        assert_eq!(got, vec![(1, "Title"), (2, "A"), (3, "A.1"), (2, "B")]);
        // byte offsets are real heading-start offsets
        assert_eq!(hs[0].byte, doc.find("# Title").unwrap());
        assert_eq!(hs[3].byte, doc.find("## B").unwrap());
    }

    #[test]
    fn headings_strip_atx_and_setext_markers() {
        let doc = "Setext Title\n===\n\nbody\n\n## ATX\n";
        let t = full_parse(doc);
        let hs = headings(&t, &rope(doc));
        assert_eq!(hs[0].level, 1);
        assert_eq!(hs[0].text, "Setext Title");
        assert_eq!(hs[1].level, 2);
        assert_eq!(hs[1].text, "ATX");
    }

    #[test]
    fn headings_multibyte_title_offsets_are_char_boundaries() {
        let doc = "## café ☕ end\n\nbody\n";
        let t = full_parse(doc);
        let hs = headings(&t, &rope(doc));
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].text, "café ☕ end");
        assert_eq!(hs[0].byte, 0);
    }

    #[test]
    fn headings_empty_doc_and_no_headings() {
        assert!(headings(&full_parse(""), &rope("")).is_empty());
        assert!(headings(&full_parse("just a paragraph\n"), &rope("just a paragraph\n")).is_empty());
    }

    #[test]
    fn heading_starts_matches_heading_offsets() {
        let doc = "# A\n\n## B\n\n### C\n";
        let t = full_parse(doc);
        let starts = heading_starts(&t, &rope(doc));
        let expect: BTreeSet<usize> = headings(&t, &rope(doc)).iter().map(|h| h.byte).collect();
        assert_eq!(starts, expect);
        assert!(!starts.contains(&doc.find("##").unwrap().saturating_sub(1)));
    }

    #[test]
    fn body_range_atx_starts_after_the_single_heading_line() {
        let doc = "## A\nbody1\nbody2\n## B\n";
        let t = full_parse(doc);
        let r = rope(doc);
        let a = doc.find("## A").unwrap();
        // body begins at "body1", ends at "## B"; the "## A" line stays visible.
        assert_eq!(body_range(&t, &r, a), doc.find("body1").unwrap()..doc.find("## B").unwrap());
    }

    #[test]
    fn body_range_setext_keeps_both_heading_lines_visible() {
        // setext heading occupies TWO lines: "Title" + "---". Using "---" makes
        // this a LEVEL-2 setext heading, so the following "## next" (also level 2)
        // terminates the section — letting us assert a bounded body. ("===" would
        // be level 1, whose section correctly runs past an h2 to doc end.)
        let doc = "Title\n---\nbody1\nbody2\n## next\n";
        let t = full_parse(doc);
        let r = rope(doc);
        let h = 0usize; // setext heading starts at byte 0
        // body must start at "body1", NOT at the "---" underline line.
        assert_eq!(body_range(&t, &r, h), doc.find("body1").unwrap()..doc.find("## next").unwrap());
    }

    #[test]
    fn body_range_empty_when_heading_has_no_body() {
        let doc = "## A\n## B\n";
        let t = full_parse(doc);
        let r = rope(doc);
        let a = doc.find("## A").unwrap();
        let br = body_range(&t, &r, a);
        assert_eq!(br.start, br.end); // no body
    }
}
