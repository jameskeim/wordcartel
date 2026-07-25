//! Landmark visibility (Effort ④): the ONE seam for marked-block interior/boundary/
//! pending paint, char-mark/bookmark presence cells, and the landmark status helpers.
//! B-lite: styles EXISTING cells only — no injected glyphs, no ColMap/layout impact.
//! Classification is EXCLUSIVE per cell (block boundary > pending > landmark >
//! interior) — landmark faces never stack, so add-only compose cannot accumulate
//! modifier soup in the near-full mono cue-space.

use ratatui::style::Style as RStyle;
use ratatui::text::Span;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::theme::{Depth, SemanticElement as SE, Theme};
use crate::editor::{Editor, MarkedBlock};
use crate::render::overlaps;

/// Per-frame landmark snapshot — gathered ONCE in `gather_row_ctx`, carried in `RowCtx`.
pub(crate) struct BlockPaint {
    /// The completed block, already filtered to visible (`!hidden`).
    block: Option<MarkedBlock>,
    /// The ^KB anchor awaiting ^KK.
    pending: Option<usize>,
    /// Mark positions (values of `Buffer.marks`), sorted ascending, deduplicated.
    landmarks: Vec<usize>,
}

/// Exclusive cell classification (locked decision 8). Exactly ONE face patches per cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CellKind { Boundary, Pending, Landmark, Interior }

/// Gather the active buffer's landmark state. O(#marks log #marks), once per frame.
pub(crate) fn gather(editor: &Editor) -> BlockPaint {
    let b = editor.active();
    let mut landmarks: Vec<usize> = b.marks.values().copied().collect();
    landmarks.sort_unstable();
    landmarks.dedup();
    BlockPaint { block: b.marked_block.filter(|mb| !mb.hidden), pending: b.pending_block_begin, landmarks }
}

/// The logical content end of `line`: the newline byte's own offset, or `buf.len()` on
/// a final line without one. BYTE-level newline test — `slice` asserts char boundaries
/// and would panic on a multibyte final char (spec §3.3, Codex round 2).
pub(crate) fn logical_content_end(buf: &TextBuffer, line: usize) -> usize {
    let start = crate::derive::line_start(buf, line);
    let next = crate::derive::line_start(buf, line + 1); // clamps to buf.len() past the last line
    if next > start && buf.byte(next - 1) == b'\n' { next - 1 } else { next }
}

impl BlockPaint {
    /// Whether the placed row builder is required. Fixes the B12 gate gap: a lone
    /// pending anchor, or bare marks, now force the placed path.
    pub(crate) fn wants_placed(&self) -> bool {
        self.block.is_some() || self.pending.is_some() || !self.landmarks.is_empty()
    }

    /// First-match-wins exclusive classification for the glyph `[g_from, g_to)`.
    fn classify(&self, g_from: usize, g_to: usize) -> Option<CellKind> {
        if let Some(b) = self.block {
            // b.start < b.end is a model invariant (`set_block` rejects empty;
            // `Buffer::apply` clears collapsed) — `b.end - 1` cannot underflow past start.
            if overlaps(g_from, g_to, b.start, b.start + 1)
                || overlaps(g_from, g_to, b.end - 1, b.end) {
                return Some(CellKind::Boundary);
            }
        }
        if let Some(p) = self.pending {
            if overlaps(g_from, g_to, p, p + 1) { return Some(CellKind::Pending); }
        }
        let i = self.landmarks.partition_point(|&m| m < g_from);
        if self.landmarks.get(i).is_some_and(|&m| m < g_to) { return Some(CellKind::Landmark); }
        if let Some(b) = self.block {
            if overlaps(g_from, g_to, b.start, b.end) { return Some(CellKind::Interior); }
        }
        None
    }

    /// The per-glyph patch — replaces render.rs's inline MarkedBlock arm. ONE face,
    /// chosen by exclusive classification; add-only compose stays sound.
    pub(crate) fn patch_glyph(&self, style: RStyle, g_from: usize, g_to: usize,
                              theme: &Theme, depth: Depth) -> RStyle {
        let el = match self.classify(g_from, g_to) {
            Some(CellKind::Boundary | CellKind::Pending) => SE::MarkedBlockBoundary,
            Some(CellKind::Landmark) => SE::LandmarkGlyph,
            Some(CellKind::Interior) => SE::MarkedBlock,
            None => return style,
        };
        style.patch(crate::compose::face_to_ratatui(&theme.face(el), depth))
    }

    /// EOL/empty-line marker cell for an entry's LAST visual row (spec §3.3): fires iff
    /// a marker byte equals the entry's final logical line's content end — a position
    /// never covered by a placed glyph. Priority Boundary > Pending > Landmark; a
    /// one-glyph-at-EOL block resolves end-first (`]`).
    pub(crate) fn trailing_marker(&self, editor: &Editor, l: usize) -> Option<Span<'static>> {
        let b = editor.active();
        let buf = &b.document.buffer;
        let end_line = crate::ventilate::resolve(&b.view, buf, l)
            .map(|r| r.last_line).unwrap_or(l);
        let cend = logical_content_end(buf, end_line);
        let (glyph, el) = if self.block.is_some_and(|mb| mb.end - 1 == cend) {
            ("]", SE::MarkedBlockBoundary)
        } else if self.block.is_some_and(|mb| mb.start == cend) || self.pending == Some(cend) {
            ("[", SE::MarkedBlockBoundary)
        } else if self.landmarks.binary_search(&cend).is_ok() {
            ("·", SE::LandmarkGlyph)
        } else {
            return None;
        };
        let style = crate::compose::face_to_ratatui(&editor.theme.face(el), editor.depth);
        Some(Span::styled(glyph.to_string(), style))
    }
}

/// Directional off-screen hint for the (non-hidden) block — line-granular (spec §3.4).
#[allow(dead_code)] // wired into the status line later; this module's tests exercise it directly
pub(crate) fn blk_direction(editor: &Editor, b: MarkedBlock) -> &'static str {
    let buf = &editor.active().document.buffer;
    let view = &editor.active().view;
    let Some((&max_line, _)) = view.line_layouts.last_key_value() else { return "" };
    let lo = crate::derive::line_start(buf, view.scroll);
    let end_line = crate::ventilate::resolve(view, buf, max_line)
        .map(|r| r.last_line).unwrap_or(max_line);
    let hi = crate::derive::line_start(buf, end_line + 1);
    if b.end <= lo { "↑" } else if b.start >= hi { "↓" } else { "" }
}

/// `MK <ids>` for marks on the caret's line (identity segment, spec §3.4). Stale
/// (post-undo) positions are CLAMPED before `byte_to_line` — a drifted mark past EOF
/// must never panic the status path (accepted undo drift, spec §4.2).
#[allow(dead_code)] // wired into the status line later; this module's tests exercise it directly
pub(crate) fn marks_on_caret_line(editor: &Editor) -> Option<String> {
    let b = editor.active();
    if b.marks.is_empty() { return None; }
    let buf = &b.document.buffer;
    let line = buf.byte_to_line(crate::nav::head(editor));
    let ids: Vec<String> = b.marks.iter()
        .filter(|&(_, &pos)| buf.byte_to_line(pos.min(buf.len())) == line)
        .map(|(c, _)| c.to_string())
        .collect();
    if ids.is_empty() { None } else { Some(format!("MK {}", ids.join(","))) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Editor, MarkedBlock};
    use wordcartel_core::buffer::TextBuffer;

    fn bp(block: Option<MarkedBlock>, pending: Option<usize>, marks: &[usize]) -> BlockPaint {
        let mut landmarks = marks.to_vec();
        landmarks.sort_unstable(); landmarks.dedup();
        BlockPaint { block, pending, landmarks }
    }
    fn blk(start: usize, end: usize) -> Option<MarkedBlock> {
        Some(MarkedBlock { start, end, hidden: false })
    }

    // --- logical_content_end: the spec §3.3 vectors, verbatim ---
    #[test]
    fn content_end_trailing_newline() {
        let buf = TextBuffer::from_str("abc\n");
        assert_eq!(logical_content_end(&buf, 0), 3); // the newline's own offset
    }
    #[test]
    fn content_end_concealed_markup_is_not_the_visible_end() {
        // `**a**\n`: visible content ends at byte 3, but the LOGICAL content end is 5
        // (the newline) — a concealed-byte mark at 3 is NOT at cend (spec round-1 fold).
        let buf = TextBuffer::from_str("**a**\n");
        assert_eq!(logical_content_end(&buf, 0), 5);
    }
    #[test]
    fn content_end_final_line_without_newline() {
        let buf = TextBuffer::from_str("abc");
        assert_eq!(logical_content_end(&buf, 0), 3); // == len
    }
    #[test]
    fn content_end_multibyte_eof_does_not_panic() {
        // Round-2 vector: "é" = 2 bytes; byte(1) is the U+00E9 continuation byte —
        // the old slice(1..2) would trip the char-boundary assert.
        let buf = TextBuffer::from_str("é");
        assert_eq!(logical_content_end(&buf, 0), 2);
    }
    #[test]
    fn content_end_empty_line() {
        let buf = TextBuffer::from_str("\n");
        assert_eq!(logical_content_end(&buf, 0), 0);
    }
    #[test]
    fn content_end_past_last_line_clamps() {
        let buf = TextBuffer::from_str("abc\n");
        // rope line 1 is the empty final line: start == next == len → cend == len.
        assert_eq!(logical_content_end(&buf, 1), 4);
    }

    // --- classification: priority + exclusivity ---
    #[test]
    fn boundary_wins_over_landmark_and_interior() {
        let p = bp(blk(0, 5), None, &[0, 2]);
        assert_eq!(p.classify(0, 1), Some(CellKind::Boundary)); // mark at b.start loses
        assert_eq!(p.classify(4, 5), Some(CellKind::Boundary)); // end = last interior glyph
        assert_eq!(p.classify(2, 3), Some(CellKind::Landmark)); // mark inside beats interior
        assert_eq!(p.classify(1, 2), Some(CellKind::Interior));
        assert_eq!(p.classify(6, 7), None);
    }
    #[test]
    fn pending_beats_landmark_loses_to_boundary() {
        let p = bp(blk(0, 2), Some(1), &[1]);
        assert_eq!(p.classify(1, 2), Some(CellKind::Boundary)); // b.end-1 == 1 wins over pending
        let p2 = bp(None, Some(3), &[3]);
        assert_eq!(p2.classify(3, 4), Some(CellKind::Pending));
    }
    #[test]
    fn one_glyph_block_is_a_single_boundary_cell() {
        let p = bp(blk(2, 3), None, &[]);
        assert_eq!(p.classify(2, 3), Some(CellKind::Boundary));
    }
    #[test]
    fn wants_placed_gates() {
        assert!(!bp(None, None, &[]).wants_placed());
        assert!(bp(blk(0, 2), None, &[]).wants_placed());
        assert!(bp(None, Some(0), &[]).wants_placed()); // the B12 gap, closed
        assert!(bp(None, None, &[7]).wants_placed());
    }
    #[test]
    fn gather_filters_hidden_and_sorts_marks() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 5, hidden: true });
        e.active_mut().marks.insert('b', 7);
        e.active_mut().marks.insert('a', 2);
        e.active_mut().marks.insert('c', 7); // dup position
        let p = gather(&e);
        assert!(p.block.is_none(), "hidden block filtered at gather");
        assert_eq!(p.landmarks, vec![2, 7], "sorted + deduped");
    }

    // --- trailing marker (via a real Editor; LivePreview is the default mode) ---
    #[test]
    fn trailing_fires_for_eol_mark_and_not_concealed_byte() {
        let mut e = Editor::new_from_text("**a**\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        // concealed trailing `*` at byte 3: cend is 5 → NO cell.
        e.active_mut().marks.insert('x', 3);
        assert!(gather(&e).trailing_marker(&e, 0).is_none(), "concealed byte: no trailing cell");
        // true EOL (the newline byte, 5): fires with the landmark glyph.
        e.active_mut().marks.insert('y', 5);
        let sp = gather(&e).trailing_marker(&e, 0).expect("EOL mark fires");
        assert_eq!(sp.content.as_ref(), "·");
    }
    #[test]
    fn trailing_block_end_on_newline_yields_close_bracket() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 4, hidden: false });
        let sp = gather(&e).trailing_marker(&e, 0).expect("end-1 == newline byte fires");
        assert_eq!(sp.content.as_ref(), "]");
    }
    #[test]
    fn trailing_pending_at_empty_line_yields_open_bracket() {
        let mut e = Editor::new_from_text("para\n\nnext\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().pending_block_begin = Some(5); // the empty line
        let sp = gather(&e).trailing_marker(&e, 1).expect("empty-line anchor fires");
        assert_eq!(sp.content.as_ref(), "[");
    }
    /// (Codex plan-gate round 1, finding 2) The end-first tie-break WITHIN Boundary,
    /// actually pinned: a one-glyph block whose single byte IS the logical content end
    /// satisfies BOTH `mb.end - 1 == cend` and `mb.start == cend`; the `]` branch must
    /// win (spec §3.3 "a one-glyph-at-EOL block resolves end-first").
    #[test]
    fn trailing_one_glyph_block_at_eol_resolves_end_first() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        // block = the newline byte alone: [3, 4) → start == end-1 == cend == 3.
        e.active_mut().marked_block = Some(MarkedBlock { start: 3, end: 4, hidden: false });
        let sp = gather(&e).trailing_marker(&e, 0).expect("one-glyph EOL block fires");
        assert_eq!(sp.content.as_ref(), "]", "end-first tie-break within Boundary");
    }

    #[test]
    fn trailing_priority_one_cell_boundary_over_landmark() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 4, hidden: false });
        e.active_mut().marks.insert('m', 3); // same cend
        let sp = gather(&e).trailing_marker(&e, 0).expect("fires once");
        assert_eq!(sp.content.as_ref(), "]", "boundary outranks landmark");
    }

    // --- status helpers ---
    #[test]
    fn blk_direction_above_below_in_view() {
        let text = (0..50).map(|i| format!("line {i}\n")).collect::<String>();
        let mut e = Editor::new_from_text(&text, None, (40, 10));
        crate::derive::rebuild(&mut e);
        let b = MarkedBlock { start: 0, end: 6, hidden: false }; // "line 0"
        assert_eq!(blk_direction(&e, b), "", "in view at scroll 0");
        e.active_mut().view.scroll = 30;
        crate::derive::rebuild(&mut e);
        assert_eq!(blk_direction(&e, b), "↑", "scrolled past → above");
        let tail_start = text.rfind("line 49").unwrap();
        let b2 = MarkedBlock { start: tail_start, end: tail_start + 4, hidden: false };
        e.active_mut().view.scroll = 0;
        crate::derive::rebuild(&mut e);
        assert_eq!(blk_direction(&e, b2), "↓", "below the 10-row viewport");
    }
    #[test]
    fn marks_on_caret_line_lists_ids_in_order_and_clamps_stale() {
        let mut e = Editor::new_from_text("one\ntwo\n", None, (40, 10));
        e.active_mut().marks.insert('a', 5); // line 1
        e.active_mut().marks.insert('3', 4); // line 1
        e.active_mut().marks.insert('z', 0); // line 0
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(6);
        assert_eq!(marks_on_caret_line(&e), Some("MK 3,a".into()), "BTreeMap order: digits first");
        // stale mark PAST EOF (accepted undo drift): clamped, never a panic.
        e.active_mut().marks.insert('q', 999);
        let _ = marks_on_caret_line(&e); // must not panic; 'q' clamps to len (line 2)
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert_eq!(marks_on_caret_line(&e), Some("MK z".into()));
    }
}
