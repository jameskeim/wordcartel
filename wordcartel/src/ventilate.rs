//! S6 — the ventilate lens: non-destructive sentence-per-line layout of paragraph prose.
//! Pure classification/gather/segment helpers here; the cache wiring is Task 3/5, the gutter
//! render Task 6. The lens SEGMENTS THE RAW block text (so the semantic-hard-break veto governs
//! the view identically to `select-sentence`) and normalizes interior `\n`→space ONLY in each
//! span's DISPLAY string (byte-length-preserving — ColMap `src` offsets stay valid). §5.1.

use wordcartel_core::block_tree::BlockTree;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::textobj::sentence_spans;

/// Columns reserved on the left for the rhythm gutter: `NNN │ ` (3-digit count, space, rule,
/// space). A fixed reservation subtracted from the wrap width (§3.4) and painted by render (Task 6).
pub const GUTTER_COLS: usize = 6;

/// The 3-digit gutter saturates here — a ≥1000-word "sentence" is not real prose (§7, L7).
pub const GUTTER_MAX: u16 = 999;

/// `Some((ps, pe))` — the WINDOW of the prose block containing `line_start_byte`, iff it is PROSE
/// (a Markdown paragraph); `None` for every verbatim block (heading, list, code, table, thematic
/// break, and — S6 — blockquote, F4/L2). The window is `nav::paragraph_range_at`'s return — **the
/// IDENTICAL call `select-sentence` (`commands.rs` `Scope::Sentence`) and focus-Sentence
/// (`render.rs:503`) make** — so `ps` is the gather/segment origin the selector uses, and
/// SEE==SELECT and focus-window-identity hold by construction (indented, hard-wrapped, AND
/// gap-fallback cases; §5.2/§6.4). The block tree's `role_at` is used ONLY to CLASSIFY prose vs
/// verbatim; the WINDOW and ORIGIN are `paragraph_range_at`'s — NEVER `block.span.start` (which
/// diverges from `ps` on the physical `line_start`-based gap fallback, `nav.rs:662-685`).
pub fn prose_block_at(blocks: &BlockTree, buf: &TextBuffer, line_start_byte: usize) -> Option<(usize, usize)> {
    if blocks.role_at(line_start_byte) != wordcartel_core::style::BlockRole::Paragraph {
        return None;
    }
    Some(crate::nav::paragraph_range_at(blocks, buf, line_start_byte))
}

/// The DISPLAY string of one already-segmented sentence span: interior `\n` (the author's hard
/// newlines) → a single space, so `layout()` (which treats its input as ONE logical line) wraps it
/// as flowing prose. **Byte-length-preserving** — `\n` and `' '` are both one byte, so every
/// resulting `ColMap.src` offset still indexes the live buffer (§5.1). This is the ONLY permitted
/// normalization, and it runs AFTER segmentation (never before — that would defeat the
/// hard-break veto, §5.1).
///
/// # Examples
///
/// ```
/// use wordcartel::ventilate::sentence_display;
///
/// let raw = "The committee met\nand voted.";
/// let disp = sentence_display(raw);
/// assert_eq!(disp, "The committee met and voted.");
/// assert_eq!(disp.len(), raw.len());
/// ```
pub fn sentence_display(raw_span: &str) -> String {
    raw_span.replace('\n', " ")
}

/// The RAW sentence spans of a gathered window (offsets window-relative to `ps`). A thin,
/// intent-named re-export of `sentence_spans`: the lens segments the RAW window text so the semantic-hard-break
/// veto governs the view identically to `select-sentence` (§5.1, §3.3 step 2).
///
/// # Examples
///
/// ```
/// use wordcartel::ventilate::segment_block;
///
/// let spans: Vec<_> = segment_block("One. Two.").collect();
/// assert_eq!(spans.len(), 2);
/// ```
pub fn segment_block(block_text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    sentence_spans(block_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    #[test]
    fn classify_paragraph_vs_verbatim() {
        // A paragraph, a heading, and a fenced code block.
        let e = Editor::new_from_text("Para one. Para two.\n\n# Heading\n\n```\ncode\n```\n", None, (80, 24));
        let buf = &e.active().document.buffer;
        let blocks = e.active().document.blocks();
        // Byte 0 is inside the paragraph → Some(span covering "Para one. Para two.").
        let p = prose_block_at(blocks, buf, 0).expect("paragraph is prose");
        // pulldown-cmark's Paragraph span includes the trailing `\n` of the block's last
        // line (verified against the real parser) — trim_end so the assertion checks the
        // prose content without depending on that incidental byte, which segment_block's
        // sentence_spans already ignores (sentence_spans("\n").count() == 0).
        assert_eq!(buf.slice(p.0..p.1).trim_end(), "Para one. Para two.");
        // The heading line start → None (verbatim).
        let h_start = buf.slice(0..buf.len()).find("# Heading").unwrap();
        assert!(prose_block_at(blocks, buf, h_start).is_none(), "heading is verbatim");
        // Inside the code fence → None (verbatim).
        let c_start = buf.slice(0..buf.len()).find("code").unwrap();
        assert!(prose_block_at(blocks, buf, c_start).is_none(), "code block is verbatim");
    }

    #[test]
    fn segment_raw_preserves_hard_break_veto() {
        // A two-space hard break (verse) must remain TWO sentences — the RAW text carries the
        // "  \n" the veto reads. Stripping \n first would merge them (SEE≠SELECT).
        let raw = "Roses are red,  \nViolets are blue.";
        assert_eq!(segment_block(raw).count(), 2, "hard-break veto keeps two spans on RAW text");
        // A soft wrap (single trailing space) merges to one.
        let soft = "The soft wrap ends here \nand continues on.";
        assert_eq!(segment_block(soft).count(), 1);
    }

    #[test]
    fn display_normalizes_newline_length_preserving() {
        let raw = "The committee met\nand voted."; // one soft-wrapped sentence
        let disp = sentence_display(raw);
        assert_eq!(disp, "The committee met and voted."); // \n → single space
        assert_eq!(disp.len(), raw.len(), "byte-length-preserving (\\n and space are both 1 byte)");
        assert!(!disp.contains('\n'));
    }
}
