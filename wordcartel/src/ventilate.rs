//! S6 — the ventilate lens: non-destructive sentence-per-line layout of paragraph prose.
//! Pure classification/gather/segment helpers here; the cache wiring is Task 3/5, the gutter
//! render Task 6. The lens SEGMENTS THE RAW block text (so the semantic-hard-break veto governs
//! the view identically to `select-sentence`) and normalizes interior `\n`→space ONLY in each
//! span's DISPLAY string (byte-length-preserving — ColMap `src` offsets stay valid). §5.1.

use wordcartel_core::block_tree::BlockTree;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::layout::{ColMap, VisualRow};
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

/// One gutter cell for a ventilated paragraph's visual row (Task 6 fills these).
/// `Count(n)` is a row-group's FIRST row (the word count, `n` already clamped to `GUTTER_MAX`);
/// `Continuation` is a soft-wrap row (blank numeric field, dim `│` only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GutterCell { Count(u16), Continuation }

/// Metadata for one ventilated PARAGRAPH window, keyed in `View.vent_blocks` by its FIRST logical
/// line. Separates the two axes the resolver needs: `last_line` for line-index LOOKUP, `byte_origin`
/// (= `ps`, the `paragraph_range_at` window start — the selector's origin) for the byte OFFSET.
/// `gutter[i]` is the cell for `line_layouts[anchor].0[i]`.
#[derive(Clone, Debug)]
pub struct VentBlock {
    pub last_line: usize,
    pub byte_origin: usize,
    pub gutter: Vec<GutterCell>,
}

/// A resolved cached layout for a logical line: the row-group's rows + ColMap, plus the byte ORIGIN
/// every consumer must reconstruct global offsets against (`origin + vr.src_span`, `head − origin`).
pub struct Resolved<'a> {
    pub rows: &'a [VisualRow],
    pub map: &'a ColMap,
    pub byte_origin: usize,
    pub first_line: usize,
    pub last_line: usize,
}

/// `(first_line, last_line)` covered by a window `[ps, pe)` — LOOKUP-range endpoints. `last_line` is
/// the line containing the window's last CONTENT byte (`pe` is exclusive; guard degenerate windows).
///
/// # Examples
///
/// ```
/// use wordcartel::ventilate::vent_block_range;
/// use wordcartel_core::buffer::TextBuffer;
///
/// let buf = TextBuffer::from_str("alpha\nbeta\ngamma\n");
/// // The window spanning lines 0..=1 ("alpha\nbeta\n") maps to (0, 1).
/// assert_eq!(vent_block_range(&buf, 0, 11), (0, 1));
/// ```
pub fn vent_block_range(buf: &TextBuffer, ps: usize, pe: usize) -> (usize, usize) {
    let first = buf.byte_to_line(ps.min(buf.len()));
    let last_byte = pe.saturating_sub(1).max(ps).min(buf.len().saturating_sub(1));
    (first, buf.byte_to_line(last_byte))
}

/// The shared window-aware resolver. Given any logical line `l`, return the cached entry that covers
/// it AND its byte ORIGIN — **line-index LOOKUP, `ps` OFFSET (the `paragraph_range_at` window start);
/// `line_start(l)` used for NEITHER in the ventilated path** (§5.2). `None` when no cached entry
/// covers `l` (the caller then lays the window out on-demand, Task 4).
///
/// LOOKUP: `range(..=l).next_back()` finds the candidate anchor; if it is a ventilated window
/// (`vent_blocks`), confirm `l ∈ first_line..=last_line` (a LINE-INDEX comparison, never a byte
/// comparison). Otherwise it is an ordinary per-line entry, which covers `l` only when keyed exactly
/// at `l`.
///
/// **Fill obligation (binds the Task 5 fill):** `line_layouts` is authoritative for the anchor
/// lookup above, so a ventilated window's `vent_blocks` entry MUST be keyed at the SAME first-line
/// anchor as its `line_layouts` entry, AND every interior per-line `line_layouts` entry within the
/// window MUST be removed — a stale interior `line_layouts` key would short-circuit `range(..=l)
/// .next_back()` before it ever reaches the window anchor, silently resolving that line as an
/// ordinary per-line entry with a `line_start` origin instead of the window's `ps`.
pub fn resolve<'a>(view: &'a crate::editor::View, buf: &TextBuffer, l: usize) -> Option<Resolved<'a>> {
    let (&anchor, (rows, map)) = view.line_layouts.range(..=l).next_back()?;
    if let Some(vb) = view.vent_blocks.get(&anchor) {
        if l <= vb.last_line {
            return Some(Resolved { rows, map, byte_origin: vb.byte_origin, first_line: anchor, last_line: vb.last_line });
        }
        return None; // past this block; not covered by it
    }
    if anchor == l {
        return Some(Resolved { rows, map, byte_origin: buf.line_to_byte(l), first_line: l, last_line: l });
    }
    None // an ordinary per-line entry keyed below l does not cover l
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

    #[test]
    fn resolver_resolves_interior_line_and_origin_is_line_start_when_off() {
        // Ordinary (non-ventilated) per-line entry: keyed exactly at l, origin == line_start.
        let mut e = Editor::new_from_text("alpha\nbeta\ngamma\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let buf = e.active().document.buffer.clone();
        let view = &e.active().view;
        let r = resolve(view, &buf, 1).expect("per-line entry for line 1 resolves");
        assert_eq!(r.byte_origin, buf.line_to_byte(1), "per-line origin is line_start");
        assert_eq!(r.first_line, 1);
        assert_eq!(r.last_line, 1);
    }

    #[test]
    fn resolver_resolves_ventilated_window_origin_is_byte_origin_not_line_start() {
        // Hand-construct a ventilated window BEFORE Task 5 exists, so the keystone
        // `if let Some(vb)` branch in `resolve` is provably exercised NOW: rebuild fills the
        // ordinary per-line cache, then we simulate what the Task 5 fill will do — collapse
        // the interior per-line entries into one window anchor and register a `VentBlock`.
        let mut e = Editor::new_from_text("alpha\nbeta\ngamma\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let buf = e.active().document.buffer.clone();
        {
            let view = &mut e.active_mut().view;
            // Remove the interior per-line entries the window swallows, so
            // `range(..=1).next_back()` lands on the anchor (line 0), not a stale line-1 entry.
            view.line_layouts.remove(&1);
            view.line_layouts.remove(&2);
            // A sentinel byte_origin, deliberately far from any real line_start in this
            // 17-byte buffer — a resolver that fell back to `line_to_byte`/byte-containment
            // instead of the `ps` passthrough could not produce this value by accident.
            view.vent_blocks.insert(0, VentBlock { last_line: 2, byte_origin: 999, gutter: vec![] });
        }
        // Line 1 is INTERIOR to the window (anchor 0, last_line 2) — only reachable via the
        // ventilated branch, since no per-line entry remains keyed at 1.
        let r = resolve(&e.active().view, &buf, 1).expect("interior line resolves via the window anchor");
        assert_eq!(r.first_line, 0, "resolves to the window anchor, not line 1 itself");
        assert_eq!(r.last_line, 2, "last_line is the VentBlock's, not the per-line default of l");
        assert_eq!(r.byte_origin, 999, "origin is the VentBlock's byte_origin (the `ps` passthrough)");
        assert_ne!(
            r.byte_origin,
            buf.line_to_byte(0),
            "origin is NOT line_start(anchor) — a byte-containment/line_start resolver would fail this"
        );
    }

    #[test]
    #[ignore = "needs Task 5 fill to populate vent_blocks"]
    fn t_indent_origin_lens_spans_equal_select_sentence_for_indented_paragraph() {
        // A 2-space-INDENTED, multi-line paragraph. paragraph_range_at's ps is AFTER the two spaces,
        // so a byte-containment test against line_start(anchor) would FAIL; line-index membership must
        // succeed. The origin must be ps, and the lens's global sentence spans must be byte-identical
        // to what select-sentence selects (the SEE==SELECT proof on the indent case).
        let text = "  The committee met on a\nsunny Tuesday afternoon. It voted.\n";
        let mut e = Editor::new_from_text(text, None, (30, 24));
        e.active_mut().view.ventilate = true;
        crate::derive::rebuild(&mut e); // Task 5 fill populates vent_blocks for the paragraph
        let buf = e.active().document.buffer.clone();
        let blocks = e.active().document.blocks().clone();
        // Line 1 ("sunny Tuesday…") is an INTERIOR line of the window (anchor is line 0).
        let r = resolve(&e.active().view, &buf, 1).expect("interior line of the ventilated window RESOLVES");
        assert_eq!(r.first_line, 0, "resolves to the window anchor");
        assert!(r.last_line >= 1, "range covers the interior line");
        // Origin == ps == paragraph_range_at start (after the 2-space indent), NOT line_start(anchor).
        let (ps, pe) = crate::nav::paragraph_range_at(&blocks, &buf, 0);
        assert_eq!(r.byte_origin, ps, "origin is ps (paragraph_range_at start)");
        assert_ne!(r.byte_origin, buf.line_to_byte(0), "origin is NOT line_start(anchor) — the indent delta");
        // SEE==SELECT: the lens's global sentence spans == sentence_spans over the SAME window select
        // uses. For each, select-sentence with the caret inside must return the identical span.
        let win = buf.slice(ps..pe);
        let lens_spans: Vec<(usize, usize)> =
            crate::ventilate::segment_block(&win).map(|(sf, st)| (ps + sf, ps + st)).collect();
        for &(gf, gt) in &lens_spans {
            // select-sentence uses scope_range_at over paragraph_range_at + sentence_bounds — identical
            // window + origin — so the selected span equals the lens span for a caret inside it.
            let (sf, st) = wordcartel_core::textobj::sentence_bounds(&win, ((gf + gt) / 2) - ps);
            assert_eq!((ps + sf, ps + st), (gf, gt), "lens span EQUALS select-sentence span (SEE==SELECT)");
        }
    }
}
