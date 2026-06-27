//! Soft-wrap + conceal layout and the source↔visual ColMap.
//! Ported from the validated spike (~/projects/wordcartel-layout-spike).
use crate::style::{BlockRole, Style};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// How many columns we expand a literal tab to.
pub const TAB_WIDTH: usize = 4;

/// One placed, *visible* grapheme cluster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    /// Source byte range of this grapheme in the logical line.
    pub src: Range<usize>,
    /// Visual row (soft-wrap row index within this logical line).
    pub row: usize,
    /// Starting display column on that row.
    pub col: usize,
    /// Display width in columns (>= 0).
    pub width: usize,
    /// Raw grapheme text.
    pub text: String,
    /// Inline style for this grapheme.
    pub style: crate::style::Style,
}

/// A contiguous run of same-style cells on a visual row.
///
/// A terminal renderer can emit one SGR span per `StyledSeg`.
/// `text` is the display text (tabs expanded to spaces, matching `VisualRow::display`).
/// `width` is the sum of display widths of the graphemes in this segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledSeg {
    pub text: String,
    pub style: crate::style::Style,
    pub width: usize,
}

/// A single visual (soft-wrapped) row, ready to be drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualRow {
    /// String to paint (concealed markers removed, tabs expanded to spaces).
    pub display: String,
    /// Total display columns occupied.
    pub width: usize,
    /// Source byte range covered by the *visible* content of this row.
    pub src_span: Range<usize>,
    /// Contiguous runs of same-style cells (concatenation of seg texts == display).
    pub segs: Vec<StyledSeg>,
    /// Block role of the logical line this row belongs to.
    pub role: crate::style::BlockRole,
    /// Prefix glyph for the block style (first row only; others are `None`).
    pub prefix_glyph: Option<String>,
}

/// Maps between source byte offsets and visual `(row, col)` positions.
///
/// Canonical unit: the **grapheme**. `placed` is in visual reading order.
/// `eol` is the source byte offset one past the last byte of the logical line.
#[derive(Clone, Debug)]
pub struct ColMap {
    /// Visible graphemes in visual order.
    pub placed: Vec<Placed>,
    /// Number of visual rows (>= 1 always).
    pub rows: usize,
    /// Source length of the logical line in bytes (== EOL sentinel offset).
    pub eol: usize,
    /// For each visual row, the display column one past the last visible cell.
    pub row_end_col: Vec<usize>,
    /// True if produced for the *active* line (raw, no conceal).
    pub is_active: bool,
    /// Display width of this line's prefix glyph (`• `, `▎ `, …); 0 if none.
    ///
    /// Every `Placed.col` is offset by this amount, and continuation rows hang-
    /// indent to it. A click at a column `< prefix_width` clamps up to it, so a
    /// hit on the prefix lands on the line's first text glyph.
    pub prefix_width: usize,
}

impl ColMap {
    /// Source byte offset -> visual `(row, col)`.
    pub fn source_to_visual(&self, offset: usize) -> (usize, usize) {
        if offset >= self.eol {
            let row = self.rows.saturating_sub(1);
            return (row, *self.row_end_col.get(row).unwrap_or(&0));
        }
        for p in &self.placed {
            if p.src.start >= offset {
                return (p.row, p.col);
            }
            if offset > p.src.start && offset < p.src.end {
                return (p.row, p.col);
            }
        }
        let row = self.rows.saturating_sub(1);
        (row, *self.row_end_col.get(row).unwrap_or(&0))
    }

    /// Visual `(row, col)` -> source byte offset.
    ///
    /// Wide-cell policy: a grapheme of width w "owns" columns [col, col+w);
    /// querying any of them returns the grapheme's start.
    ///
    /// Zero-width policy: a zero-width grapheme (combining mark, ZWSP, ZWJ
    /// fragment) shares the column of the following base grapheme. When a
    /// positive-width grapheme also covers that column, the POSITIVE-WIDTH one
    /// wins — the cursor lands on the visible base, never on a stray zero-width
    /// mark sharing the cell. (Empirically required by Law 5: otherwise
    /// down->up collapses onto a leading zero-width grapheme.)
    pub fn visual_to_source(&self, row: usize, col: usize) -> usize {
        // Clamp the prefix region: a click at col < prefix_width is treated as
        // col == prefix_width, so it lands on the line's first text glyph rather
        // than under the bullet/quote glyph.
        let col = col.max(self.prefix_width);
        // First pass: a positive-width grapheme that covers the column.
        for p in &self.placed {
            if p.row == row && p.width > 0 && col >= p.col && col < p.col + p.width {
                return p.src.start;
            }
        }
        // Second pass: a zero-width grapheme exactly at the column, only if no
        // positive-width grapheme claimed it above.
        for p in &self.placed {
            if p.row == row && p.width == 0 && col == p.col {
                return p.src.start;
            }
        }
        // The requested column is past this row's content. CLAMP to the end of
        // THIS row, do not fall through to a later row. The end-of-row position
        // is the source offset just after the last grapheme on this row — which
        // is the start of the first grapheme on the next row, or EOL. This
        // distinction matters for desired-column vertical motion (Law 5): a
        // column that overshoots a short row must land at that row's end, not
        // teleport to a later row.
        let last_on_row = self
            .placed
            .iter()
            .filter(|p| p.row == row)
            .map(|p| p.src.end)
            .max();
        if let Some(end) = last_on_row {
            return end;
        }
        // This row has no graphemes at all (empty row): fall to next row's
        // first grapheme, else EOL.
        for p in &self.placed {
            if p.row > row {
                return p.src.start;
            }
        }
        self.eol
    }

    /// Is this offset a valid cursor stop (visible grapheme start, or EOL)?
    pub fn is_cursor_stop(&self, offset: usize) -> bool {
        offset == self.eol || self.placed.iter().any(|p| p.src.start == offset)
    }

    /// All cursor-stop source offsets in source order.
    pub fn cursor_stops(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.placed.iter().map(|p| p.src.start).collect();
        v.push(self.eol);
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Snap a raw offset up to the nearest valid cursor stop (>= offset), else EOL.
    ///
    /// Used after `visual_to_source` to guard against landing inside a concealed
    /// trailing marker (e.g. the closing `**` of `**a**`).
    pub fn snap_to_stop(&self, raw: usize) -> usize {
        self.cursor_stops().into_iter().find(|&s| s >= raw).unwrap_or(self.eol)
    }

    /// Visual column of `offset` *on a specified row*. Used when the cursor's
    /// row affinity is known, to avoid the boundary ambiguity. Returns the
    /// end-of-row column if the offset is the row's end sentinel.
    pub fn col_on_row(&self, offset: usize, row: usize) -> usize {
        for p in &self.placed {
            if p.row == row && p.src.start == offset {
                return p.col;
            }
        }
        // offset is the end-of-row position for `row`
        *self.row_end_col.get(row).unwrap_or(&0)
    }
}

/// Display width of a single grapheme, applying our tab policy.
fn grapheme_width(g: &str) -> usize {
    if g == "\t" {
        TAB_WIDTH
    } else {
        UnicodeWidthStr::width(g)
    }
}

/// The core: lay out one logical line.
pub fn layout(
    line: &str,
    role: BlockRole,
    is_active: bool,
    viewport_width: usize,
) -> (Vec<VisualRow>, ColMap) {
    let vw = viewport_width.max(1);
    let analysis = crate::md_parse::analyze(line, role, is_active);

    struct VG {
        src: Range<usize>,
        text: String,
        width: usize,
        style: Style,
    }
    let mut vgs: Vec<VG> = Vec::new();
    for run in &analysis.runs {
        if !run.visible {
            continue;
        }
        let slice = &line[run.src.clone()];
        for (off, g) in slice.grapheme_indices(true) {
            let start = run.src.start + off;
            let byte_start = start;
            let style = analysis
                .styles
                .iter()
                .find(|s| s.src.contains(&byte_start))
                .map(|s| s.style)
                .unwrap_or(Style::Plain);
            vgs.push(VG {
                src: start..start + g.len(),
                text: g.to_string(),
                width: grapheme_width(g),
                style,
            });
        }
    }

    // Display width of the block's prefix glyph (e.g. `• `, `▎ `, `─── `). Every
    // placed column is offset by this, and continuation rows hang-indent to it,
    // so the effective wrap capacity is `vw - prefix_width`. Computed over the
    // glyph's GRAPHEMES (matching the painted width), not a char count.
    let prefix_width: usize = analysis
        .prefix_glyph
        .as_deref()
        .map(|g| g.graphemes(true).map(grapheme_width).sum())
        .unwrap_or(0);

    // Greedy soft-wrap.
    let mut placed: Vec<Placed> = Vec::new();
    let mut row = 0usize;
    let mut col = prefix_width;
    let mut row_end_col: Vec<usize> = Vec::new();

    for vg in &vgs {
        if vg.width == 0 {
            placed.push(Placed {
                src: vg.src.clone(),
                row,
                col,
                width: 0,
                text: vg.text.clone(),
                style: vg.style,
            });
            continue;
        }
        if col + vg.width > vw && col > prefix_width {
            row_end_col.push(col);
            row += 1;
            col = prefix_width;
        }
        placed.push(Placed {
            src: vg.src.clone(),
            row,
            col,
            width: vg.width,
            text: vg.text.clone(),
            style: vg.style,
        });
        col += vg.width;
    }
    row_end_col.push(col);
    let rows = row + 1;

    let mut visual_rows: Vec<VisualRow> =
        vec![VisualRow { display: String::new(), width: 0, src_span: 0..0, segs: Vec::new(), role: BlockRole::Paragraph, prefix_glyph: None }; rows];
    let mut row_min: Vec<Option<usize>> = vec![None; rows];
    let mut row_max: Vec<Option<usize>> = vec![None; rows];
    for p in &placed {
        let vr = &mut visual_rows[p.row];
        let seg_text = if p.text == "\t" {
            " ".repeat(TAB_WIDTH)
        } else {
            p.text.clone()
        };
        if p.text == "\t" {
            vr.display.push_str(&" ".repeat(TAB_WIDTH));
        } else {
            vr.display.push_str(&p.text);
        }
        vr.width += p.width;
        row_min[p.row] = Some(row_min[p.row].map_or(p.src.start, |m: usize| m.min(p.src.start)));
        row_max[p.row] = Some(row_max[p.row].map_or(p.src.end, |m: usize| m.max(p.src.end)));
        // Accumulate styled segments: extend the last seg if same style, else start a new one.
        match vr.segs.last_mut() {
            Some(seg) if seg.style == p.style => {
                seg.text.push_str(&seg_text);
                seg.width += p.width;
            }
            _ => {
                vr.segs.push(StyledSeg { text: seg_text, style: p.style, width: p.width });
            }
        }
    }
    for r in 0..rows {
        if let (Some(a), Some(b)) = (row_min[r], row_max[r]) {
            visual_rows[r].src_span = a..b;
        }
    }

    // Propagate block role to every row and prefix glyph to the first row only.
    for vr in visual_rows.iter_mut() {
        vr.role = role;
    }
    visual_rows[0].prefix_glyph = analysis.prefix_glyph;

    let map = ColMap {
        placed,
        rows,
        eol: line.len(),
        row_end_col,
        is_active,
        prefix_width,
    };
    (visual_rows, map)
}

// ---------------------------------------------------------------------------
// Cursor navigation
// ---------------------------------------------------------------------------

/// A cursor.
///
/// FINDING: a byte offset alone is NOT enough. At a soft-wrap boundary the same
/// offset is both "end of row N" and "start of row N+1"; `source_to_visual`
/// must pick one, so vertical motion derived purely from the offset drifts.
/// The cursor therefore carries an explicit visual `row` (its *affinity*) plus
/// the source `offset` and the remembered `desired_col`. Horizontal motion
/// recomputes `row` from the offset; vertical motion sets `row` directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub offset: usize,
    /// Visual row affinity (resolves the soft-wrap boundary ambiguity).
    pub row: usize,
    /// Desired visual column, preserved across vertical motion.
    pub desired_col: usize,
}

impl Cursor {
    pub fn new(offset: usize) -> Self {
        Cursor { offset, row: 0, desired_col: 0 }
    }
}

/// Construct a cursor, snapping `offset` to the nearest valid cursor stop on
/// or after it (or EOL). REQUIRED whenever a raw byte offset enters the cursor
/// system, because a line may *begin* with a concealed span (e.g. `**a**`),
/// making offset 0 itself invalid. Empirically surfaced by Law 2.
pub fn cursor_at(map: &ColMap, offset: usize) -> Cursor {
    let stops = map.cursor_stops();
    let snapped = stops
        .iter()
        .copied()
        .find(|&s| s >= offset)
        .unwrap_or(map.eol);
    let (row, col) = map.source_to_visual(snapped);
    Cursor { offset: snapped, row, desired_col: col }
}

/// Move right by one grapheme, skipping concealed bytes. Recomputes row
/// affinity from the new offset (horizontal motion resolves boundary toward the
/// start of the row the grapheme begins on).
pub fn move_right(map: &ColMap, cur: Cursor) -> Cursor {
    let stops = map.cursor_stops();
    let next = stops.iter().copied().find(|&s| s > cur.offset).unwrap_or(map.eol);
    let (row, col) = map.source_to_visual(next);
    Cursor { offset: next, row, desired_col: col }
}

/// Move left by one grapheme, skipping concealed bytes.
pub fn move_left(map: &ColMap, cur: Cursor) -> Cursor {
    let stops = map.cursor_stops();
    let prev = stops
        .iter()
        .copied()
        .rev()
        .find(|&s| s < cur.offset)
        .unwrap_or_else(|| stops.first().copied().unwrap_or(map.eol));
    let (row, col) = map.source_to_visual(prev);
    Cursor { offset: prev, row, desired_col: col }
}

/// Home: start of the cursor's *visual row* (using its row affinity).
pub fn move_home(map: &ColMap, cur: Cursor) -> Cursor {
    let row = cur.row.min(map.rows.saturating_sub(1));
    let off = map.visual_to_source(row, 0);
    Cursor { offset: off, row, desired_col: 0 }
}

/// End: end of the cursor's *visual row* (using its row affinity). The cursor
/// keeps row affinity `row` even though `offset` is the boundary offset that
/// `source_to_visual` would otherwise read as the next row.
///
/// FINDING: the raw end-of-row byte position can be a *concealed* trailing
/// marker (e.g. `**a**` at width 1: visible cell "a" ends at byte 3, which is a
/// `*`). We snap the result UP to the nearest valid cursor stop so the cursor
/// never rests on a concealed byte.
pub fn move_end(map: &ColMap, cur: Cursor) -> Cursor {
    let row = cur.row.min(map.rows.saturating_sub(1));
    let end_col = *map.row_end_col.get(row).unwrap_or(&0);
    let raw = map.visual_to_source(row, end_col);
    let off = map.snap_to_stop(raw);
    Cursor { offset: off, row, desired_col: end_col }
}

/// Move down one visual row within this logical line, preserving desired_col.
/// Uses the cursor's explicit row affinity (not the offset) so soft-wrap
/// boundaries don't cause drift.
pub fn move_down_within(map: &ColMap, cur: Cursor) -> Option<Cursor> {
    if cur.row.saturating_add(1) >= map.rows {
        return None;
    }
    let target = cur.row.saturating_add(1);
    let want = cur.desired_col;
    let raw = map.visual_to_source(target, want);
    let off = map.snap_to_stop(raw);
    Some(Cursor { offset: off, row: target, desired_col: want })
}

/// Move up one visual row, preserving desired_col.
pub fn move_up_within(map: &ColMap, cur: Cursor) -> Option<Cursor> {
    if cur.row == 0 {
        return None;
    }
    let target = cur.row - 1;
    let want = cur.desired_col;
    let raw = map.visual_to_source(target, want);
    let off = map.snap_to_stop(raw);
    Some(Cursor { offset: off, row: target, desired_col: want })
}

/// Enter a logical line from above at `desired_col` (first row).
pub fn enter_from_top(map: &ColMap, desired_col: usize) -> Cursor {
    let raw = map.visual_to_source(0, desired_col);
    let off = map.snap_to_stop(raw);
    Cursor { offset: off, row: 0, desired_col }
}

/// Enter a logical line from below at `desired_col` (last row).
pub fn enter_from_bottom(map: &ColMap, desired_col: usize) -> Cursor {
    let last = map.rows.saturating_sub(1);
    let raw = map.visual_to_source(last, desired_col);
    let off = map.snap_to_stop(raw);
    Cursor { offset: off, row: last, desired_col }
}

// ---------------------------------------------------------------------------
// Helpers for property tests (also useful for callers inspecting layout).
// ---------------------------------------------------------------------------

/// Total visible display width for a logical line (sum of visible grapheme widths).
pub fn visible_width(line: &str, role: BlockRole, is_active: bool) -> usize {
    let analysis = crate::md_parse::analyze(line, role, is_active);
    let mut w = 0;
    for run in &analysis.runs {
        if run.visible {
            for g in line[run.src.clone()].graphemes(true) {
                w += grapheme_width(g);
            }
        }
    }
    w
}

/// Visible source string (graphemes that survive concealment), in order.
pub fn visible_source(line: &str, role: BlockRole, is_active: bool) -> String {
    let analysis = crate::md_parse::analyze(line, role, is_active);
    let mut s = String::new();
    for run in &analysis.runs {
        if run.visible {
            s.push_str(&line[run.src.clone()]);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_line_identity_and_wrap() {
        // Active: raw, identity-ish. "abcdef" width 4 -> rows ["abcd","ef"].
        let (rows, map) = layout("abcdef", BlockRole::Paragraph, true, 4);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].display, "abcd");
        assert_eq!(rows[1].display, "ef");
        assert_eq!(map.eol, 6);
        assert!(map.is_active);
    }

    #[test]
    fn concealed_bold_drops_markers_in_display() {
        // Inactive: "**bold**" -> visible "bold".
        let (rows, _map) = layout("**bold**", BlockRole::Paragraph, false, 80);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].display, "bold");
    }

    #[test]
    fn cjk_width_two() {
        let (rows, _) = layout("中a", BlockRole::Paragraph, true, 80);
        assert_eq!(rows[0].width, 3); // 中=2, a=1
    }

    #[test]
    fn style_attached_to_placed() {
        // visible 'b' (first of bold) should carry Style::Strong.
        let (_rows, map) = layout("**bold**", BlockRole::Paragraph, false, 80);
        let first = map.placed.iter().find(|p| p.text == "b").unwrap();
        assert_eq!(first.style, Style::Strong);
    }

    #[test]
    fn roundtrip_bijection_on_visible_cells() {
        let (_rows, map) = layout("a中b", BlockRole::Paragraph, true, 80);
        for p in &map.placed {
            let (r, c) = map.source_to_visual(p.src.start);
            assert_eq!(map.visual_to_source(r, c), p.src.start);
        }
    }
    #[test]
    fn cursor_never_inside_concealed_marker() {
        // "**a**": only 'a' (byte 2) and EOL(6) are stops; the * bytes are not.
        let (_rows, map) = layout("**a**", BlockRole::Paragraph, false, 80);
        let stops = map.cursor_stops();
        assert!(stops.contains(&2));
        assert!(stops.contains(&map.eol));
        assert!(!stops.contains(&0)); // leading * concealed
        assert!(!stops.contains(&1));
    }
    #[test]
    fn end_of_row_clamps_not_teleports() {
        // width 2: "abcd" -> rows ["ab","cd"]. col 9 on row 0 clamps to end of row 0 (byte 2).
        let (_rows, map) = layout("abcd", BlockRole::Paragraph, true, 2);
        assert_eq!(map.visual_to_source(0, 9), 2);
    }

    #[test]
    fn right_skips_concealed_link_url() {
        // "ab[cd](http://x.io)ef": visible "abcdef"; moving right from start
        // visits only visible grapheme starts, never inside the hidden URL.
        let line = "ab[cd](http://x.io)ef";
        let (_r, map) = layout(line, BlockRole::Paragraph, false, 80);
        let mut cur = cursor_at(&map, 0);
        let mut visited = vec![cur.offset];
        for _ in 0..6 { cur = move_right(&map, cur); visited.push(cur.offset); }
        // none of the visited offsets fall inside the URL byte range [7,18)
        assert!(visited.iter().all(|&o| !(7..18).contains(&o)));
    }
    #[test]
    fn move_end_snaps_off_concealed_trailing_marker() {
        // "**a**" width 1: end-of-row raw position is a concealed '*'; move_end
        // must snap to a real stop (the 'a' start or EOL), never a '*'.
        let (_r, map) = layout("**a**", BlockRole::Paragraph, false, 1);
        let cur = cursor_at(&map, 2); // on 'a'
        let e = move_end(&map, cur);
        assert!(map.is_cursor_stop(e.offset));
    }
    #[test]
    fn styled_segments_split_by_style() {
        // "a **b**" inactive -> visible "a b": 'a',' ' Plain then 'b' Strong.
        let (rows, _map) = layout("a **b**", BlockRole::Paragraph, false, 80);
        let segs = &rows[0].segs;
        assert_eq!(segs.last().unwrap().style, Style::Strong);
        assert_eq!(segs.last().unwrap().text, "b");
        // concatenated segs equal display
        let joined: String = segs.iter().map(|s| s.text.clone()).collect();
        assert_eq!(joined, rows[0].display);
    }

    #[test]
    fn move_left_from_leftmost_stays_on_visible_stop() {
        // "**a**": only visible stop besides EOL is byte 2 ('a'). move_left from there
        // must NOT land on a concealed '*' (byte 0/1); it stays on byte 2.
        let (_r, map) = layout("**a**", BlockRole::Paragraph, false, 80);
        let cur = cursor_at(&map, 2);
        let left = move_left(&map, cur);
        assert!(map.is_cursor_stop(left.offset), "cursor must rest on a visible stop, not a concealed byte");
        assert_eq!(left.offset, 2); // no-op at the leftmost visible stop
    }

    #[test]
    fn enter_from_top_overshoot_concealed_lands_on_stop() {
        // "**a**": entering at an overshooting col must not land on a concealed '*'.
        let (_r, map) = layout("**a**", BlockRole::Paragraph, false, 80);
        for col in [3usize, 5, 9] {
            let c = enter_from_top(&map, col);
            assert!(map.is_cursor_stop(c.offset), "enter_from_top col {col} landed on concealed byte {}", c.offset);
        }
    }

    #[test]
    fn enter_from_bottom_overshoot_concealed_lands_on_stop() {
        let (_r, map) = layout("**a**", BlockRole::Paragraph, false, 80);
        let c = enter_from_bottom(&map, 9);
        assert!(map.is_cursor_stop(c.offset));
    }

    #[test]
    fn rows_carry_block_role_and_glyph() {
        let (rows, _m) = layout("- item", BlockRole::ListItem, false, 80);
        assert_eq!(rows[0].role, BlockRole::ListItem);
        assert_eq!(rows[0].prefix_glyph.as_deref(), Some("• "));
    }

    #[test]
    fn heading_rows_carry_heading_role() {
        let (rows, _m) = layout("## Title", BlockRole::Heading(2), false, 80);
        assert!(rows.iter().all(|r| r.role == BlockRole::Heading(2)));
    }

    #[test]
    fn prefix_offsets_columns_so_cursor_lands_on_text() {
        // A list item: prefix "• " (width 2). The first text glyph 'i' must be at col 2, not 0.
        let (_rows, map) = layout("- item", BlockRole::ListItem, false, 40);
        assert_eq!(map.prefix_width, 2, "• + space");
        let (row, col) = map.source_to_visual(2); // byte 2 = 'i' (after "- ")
        assert_eq!((row, col), (0, 2));
        // A click in the prefix region (col 0/1) lands at the first text byte, not end-of-row.
        assert_eq!(map.visual_to_source(0, 0), map.visual_to_source(0, 2));
    }

    #[test]
    fn no_prefix_is_unchanged() {
        let (_rows, map) = layout("plain text", BlockRole::Paragraph, false, 40);
        assert_eq!(map.prefix_width, 0);
        assert_eq!(map.source_to_visual(0), (0, 0)); // no offset
    }

    #[test]
    fn prefix_reduces_wrap_capacity() {
        // width-6 viewport, prefix width 2 → text wraps after 4 cols, continuation indented to col 2.
        let (_rows, map) = layout("- aaaa bbbb", BlockRole::ListItem, false, 6);
        assert!(map.rows >= 2, "should wrap");
        // A glyph on row 1 starts at col == prefix_width (hanging indent).
        let first_row1 = map.placed.iter().find(|p| p.row == 1 && p.width > 0).unwrap();
        assert_eq!(first_row1.col, 2, "continuation indented to prefix_width");
    }

    #[test]
    fn down_then_up_preserves_desired_col() {
        // "aaaaa" width 3 -> rows ["aaa","aa"]. start at col 2 row 0, down then up.
        let (_r, map) = layout("aaaaa", BlockRole::Paragraph, true, 3);
        let start = Cursor { offset: 2, row: 0, desired_col: 2 };
        let down = move_down_within(&map, start).unwrap();
        let up = move_up_within(&map, down).unwrap();
        assert_eq!(up.offset, start.offset);
    }
}

// ---------------------------------------------------------------------------
// Property tests (proptest): the five layout invariant laws.
// Ported from ~/projects/wordcartel-layout-spike/tests/invariants.rs and
// adapted to:
//   - the crate's layout(line, role, is_active, w) signature
//   - multi-byte alphabet: ASCII + é, 中, 🙂 + concealed inline constructs
//   - visible_source/visible_width helpers using md_parse::analyze
// ---------------------------------------------------------------------------
#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;
    use unicode_segmentation::UnicodeSegmentation;

    /// Building blocks: ASCII words + multi-byte graphemes + concealed constructs.
    fn token() -> impl Strategy<Value = String> {
        prop_oneof![
            // plain ASCII words
            "[a-z]{1,6}".prop_map(|s| s),
            Just(" ".to_string()),
            // multi-byte graphemes (task-specified: é, 中, 🙂)
            Just("é".to_string()),          // U+00E9, 2 bytes, width 1
            Just("中".to_string()),          // U+4E2D, 3 bytes, width 2
            Just("🙂".to_string()),         // U+1F642, 4 bytes, width 2
            // tab (exercises tab-width policy)
            Just("\t".to_string()),
            // zero-width / multi-codepoint graphemes — exercise the
            // "zero-width shares cell, positive-width wins" policy.
            Just("e\u{0301}".to_string()),           // e + combining acute: one grapheme, width 1
            Just("\u{200b}".to_string()),             // zero-width space: width 0
            Just("🤦🏼\u{200d}♂\u{fe0f}".to_string()), // ZWJ emoji: one grapheme, width 2
            // concealed markdown constructs (well-formed)
            "[a-z]{1,5}".prop_map(|s| format!("**{}**", s)),
            "[a-z]{1,5}".prop_map(|s| format!("*{}*", s)),
            "[a-z]{1,5}".prop_map(|s| format!("~~{}~~", s)),
            "[a-z]{1,5}".prop_map(|s| format!("`{}`", s)),
            "[a-z]{1,5}".prop_map(|s| format!("[{}](http://e.x/{})", s, s)),
        ]
    }

    prop_compose! {
        fn logical_line()(toks in prop::collection::vec(token(), 0..8)) -> String {
            let s: String = toks.concat();
            // strip any accidental newlines (we only handle single logical lines)
            s.replace('\n', " ").replace('\r', " ")
        }
    }

    fn widths() -> impl Strategy<Value = usize> {
        prop_oneof![Just(1usize), Just(3), Just(5), Just(8), Just(20), Just(80)]
    }

    proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 512,
            ..Default::default()
        })]

        // -------------------------------------------------------------------
        // LAW 1: ColMap round-trip / bijection on visible cells.
        // For every visible visual cell c, source_to_visual(visual_to_source(c)) == c.
        // We test the canonical cell of each grapheme: its starting (row,col).
        // -------------------------------------------------------------------
        #[test]
        fn law1_colmap_roundtrip(
            line in logical_line(),
            w in widths(),
            active in any::<bool>()
        ) {
            let (_rows, map) = layout(&line, BlockRole::Paragraph, active, w);
            for p in &map.placed {
                let off = map.visual_to_source(p.row, p.col);
                let (r2, c2) = map.source_to_visual(off);
                // round-trip must land on the grapheme that owns that cell
                let owner = map.placed.iter()
                    .find(|q| q.src.start == off)
                    .expect("offset must be a grapheme start");
                prop_assert_eq!((r2, c2), (owner.row, owner.col),
                    "roundtrip cell ({},{}) -> off {} -> ({},{})",
                    p.row, p.col, off, r2, c2);
            }
        }

        // -------------------------------------------------------------------
        // LAW 2: No cursor inside a concealed marker.
        // Every reachable cursor source offset is a valid grapheme boundary
        // among VISIBLE content (or EOL).
        // -------------------------------------------------------------------
        #[test]
        fn law2_no_cursor_in_conceal(
            line in logical_line(),
            w in widths(),
            active in any::<bool>()
        ) {
            let (_rows, map) = layout(&line, BlockRole::Paragraph, active, w);
            // Walk right from the first valid stop.
            let mut cur = cursor_at(&map, 0);
            let mut seen = vec![cur.offset];
            for _ in 0..(line.len() + 4) {
                let n = move_right(&map, cur);
                if n.offset == cur.offset { break; }
                cur = n;
                seen.push(cur.offset);
                if cur.offset >= map.eol { break; }
            }
            let vis = visible_source(&line, BlockRole::Paragraph, active);
            // Set of valid visible-grapheme-start byte offsets + EOL:
            let valid: std::collections::HashSet<usize> =
                map.placed.iter().map(|p| p.src.start)
                    .chain(std::iter::once(map.eol))
                    .collect();
            for &o in &seen {
                prop_assert!(valid.contains(&o),
                    "cursor offset {} not a visible grapheme start (visible={:?})", o, vis);
            }
            // Also walk LEFT from EOL, asserting every visited offset is valid.
            let mut cur_left = cursor_at(&map, map.eol);
            for _ in 0..(line.len() + 4) {
                let n = move_left(&map, cur_left);
                prop_assert!(valid.contains(&n.offset),
                    "move_left produced invalid offset {} (visible={:?})", n.offset, vis);
                if n.offset == cur_left.offset { break; }
                cur_left = n;
            }
            // move_end from every row must land on a valid stop.
            for r in 0..map.rows {
                let probe = Cursor { offset: map.visual_to_source(r, 0), row: r, desired_col: 0 };
                let e = move_end(&map, probe);
                prop_assert!(valid.contains(&e.offset),
                    "move_end on row {} produced invalid offset {}", r, e.offset);
            }
        }

        // -------------------------------------------------------------------
        // LAW 3: Soft-wrap fidelity.
        // Concatenating placed graphemes (visual order) reconstructs the
        // visible content; wrapping never splits a grapheme; widths obey
        // unicode-width.
        // -------------------------------------------------------------------
        #[test]
        fn law3_softwrap_fidelity(
            line in logical_line(),
            w in widths(),
            active in any::<bool>()
        ) {
            let (rows, map) = layout(&line, BlockRole::Paragraph, active, w);

            // (a) placed graphemes reconstruct visible content
            let reconstructed: String = map.placed.iter().map(|p| p.text.as_str()).collect();
            let expected = visible_source(&line, BlockRole::Paragraph, active);
            prop_assert_eq!(&reconstructed, &expected, "placed graphemes reconstruct visible");

            // (b) every placed grapheme is a single grapheme cluster (never split)
            for p in &map.placed {
                let g_count = p.text.graphemes(true).count();
                prop_assert_eq!(g_count, 1,
                    "placed text {:?} is not a single grapheme", p.text);
            }

            // (c) widths obey unicode-width / tab policy; no row exceeds viewport
            // unless a single grapheme is itself wider than the viewport.
            for (ri, row) in rows.iter().enumerate() {
                let sum: usize = map.placed.iter()
                    .filter(|p| p.row == ri)
                    .map(|p| p.width)
                    .sum();
                prop_assert_eq!(sum, row.width, "row {} width mismatch", ri);
                let on_row: Vec<_> = map.placed.iter()
                    .filter(|p| p.row == ri && p.width > 0)
                    .collect();
                if row.width > w {
                    prop_assert_eq!(on_row.len(), 1,
                        "row {} width {} > vw {} but has {} graphemes",
                        ri, row.width, w, on_row.len());
                }
            }

            // (d) placed grapheme row indices form a contiguous range 0..rows
            let mut max_row = 0usize;
            for p in &map.placed { max_row = max_row.max(p.row); }
            if !map.placed.is_empty() {
                prop_assert_eq!(max_row + 1, map.rows.min(max_row + 1));
            }
        }

        // -------------------------------------------------------------------
        // LAW 4: Active-line identity.
        // is_active == true => visible source == raw line; placed graphemes
        // cover the line gaplessly from byte 0.
        // -------------------------------------------------------------------
        #[test]
        fn law4_active_identity(line in logical_line(), w in widths()) {
            let (_rows, map) = layout(&line, BlockRole::Paragraph, true, w);
            prop_assert!(map.is_active);
            // visible source == raw line (no concealment on active line)
            prop_assert_eq!(
                visible_source(&line, BlockRole::Paragraph, true),
                line.clone()
            );
            // placed graphemes cover the line with no gaps
            let mut expect = 0usize;
            for p in &map.placed {
                prop_assert_eq!(p.src.start, expect, "active map has a concealed gap");
                expect = p.src.end;
            }
            prop_assert_eq!(expect, line.len(), "active map does not cover whole line");
            // every grapheme start is a valid cursor stop
            for p in &map.placed {
                prop_assert!(map.is_cursor_stop(p.src.start));
            }
        }

        // -------------------------------------------------------------------
        // LAW 5: Desired-column preservation.
        // Cursor down then up returns to the same source offset when line
        // lengths allow.  Tested on active layout (no concealment), interior
        // positive-width graphemes on row 0.
        // -------------------------------------------------------------------
        #[test]
        fn law5_desired_col_preserved(line in logical_line(), w in widths()) {
            // Active layout: columns map straightforwardly; the law is about
            // desired-col bookkeeping independent of concealment.
            let (_rows, map) = layout(&line, BlockRole::Paragraph, true, w);
            if map.rows < 2 { return Ok(()); }
            // Start at each positive-width grapheme on row 0; go down then up.
            // Zero-width graphemes are excluded (documented finding: they share
            // a column with their base and are not independent round-trip stops).
            for p in map.placed.iter().filter(|p| p.row == 0 && p.width > 0) {
                let start = Cursor {
                    offset: p.src.start,
                    row: 0,
                    desired_col: p.col,
                };
                if let Some(down) = move_down_within(&map, start) {
                    if let Some(up) = move_up_within(&map, down) {
                        prop_assert_eq!(up.row, 0, "up did not return to row 0");
                        prop_assert_eq!(up.offset, start.offset,
                            "down->up changed offset: start col {} -> {}",
                            p.col, up.offset);
                    }
                }
            }
        }

        // -------------------------------------------------------------------
        // LAW 6: All cursor-nav ops on CONCEALED layouts produce valid stops.
        // Comprehensively exercises move_right/left/home/end/down/up/enter_from_top
        // /enter_from_bottom on inactive (is_active=false) layouts with concealed
        // markers and asserts every produced offset is a valid cursor stop.
        // This law MUST fail before the fix and pass after.
        // -------------------------------------------------------------------
        #[test]
        fn law6_all_nav_ops_land_on_stop_concealed(
            line in logical_line(),
            w in widths(),
        ) {
            let (_rows, map) = layout(&line, BlockRole::Paragraph, false, w);
            let valid: std::collections::HashSet<usize> =
                map.placed.iter().map(|p| p.src.start)
                    .chain(std::iter::once(map.eol))
                    .collect();

            // Helper: assert offset is a cursor stop.
            macro_rules! assert_stop {
                ($off:expr, $label:expr) => {
                    prop_assert!(
                        valid.contains(&$off),
                        "{} produced invalid offset {} (line={:?}, w={})",
                        $label, $off, line, w
                    );
                };
            }

            // Walk right from start.
            let mut cur = cursor_at(&map, 0);
            assert_stop!(cur.offset, "cursor_at(0)");
            let max_steps = line.len() + 8;
            for _ in 0..max_steps {
                let n = move_right(&map, cur);
                assert_stop!(n.offset, "move_right");
                if n.offset == cur.offset { break; }
                cur = n;
                if cur.offset >= map.eol { break; }
            }

            // Walk left from EOL.
            let mut cur_l = cursor_at(&map, map.eol);
            assert_stop!(cur_l.offset, "cursor_at(eol)");
            for _ in 0..max_steps {
                let n = move_left(&map, cur_l);
                assert_stop!(n.offset, "move_left");
                if n.offset == cur_l.offset { break; }
                cur_l = n;
            }

            // move_home and move_end on every row.
            for r in 0..map.rows {
                let probe = Cursor { offset: map.visual_to_source(r, 0), row: r, desired_col: 0 };
                let h = move_home(&map, probe);
                assert_stop!(h.offset, "move_home");
                let e = move_end(&map, probe);
                assert_stop!(e.offset, "move_end");
            }

            // move_down_within and move_up_within: walk down from first stop, then up
            // (single-column baseline, preserved from original law).
            let mut dc = cursor_at(&map, 0);
            for _ in 0..map.rows {
                match move_down_within(&map, dc) {
                    Some(n) => {
                        assert_stop!(n.offset, "move_down_within");
                        dc = n;
                    }
                    None => break,
                }
            }
            // Walk back up.
            let mut uc = dc;
            for _ in 0..map.rows {
                match move_up_within(&map, uc) {
                    Some(n) => {
                        assert_stop!(n.offset, "move_up_within");
                        uc = n;
                    }
                    None => break,
                }
            }

            // Extended vertical coverage: drive move_down_within / move_up_within
            // from MULTIPLE starting desired columns on each row.
            // This closes the vertical-overshoot gap (concealed-trailing-marker
            // path) that the single-column walk above does not exercise.
            // For each row × desired_col pair we build a starting Cursor and
            // repeatedly step down (from top rows) or up (from bottom rows),
            // asserting every produced offset is a valid cursor stop.
            let max_steps = map.rows + 2;
            let overshoot_col = line.len() + 8;
            for r in 0..map.rows {
                let row_end = *map.row_end_col.get(r).unwrap_or(&0);
                let mid_col = row_end / 2;
                // Columns to probe: 0, mid, row_end, row_end+1, large overshoot.
                let probe_cols = [0usize, mid_col, row_end,
                                  row_end.saturating_add(1), overshoot_col];
                for col in probe_cols {
                    // --- walk DOWN from (r, col) ---
                    let start_off = map.visual_to_source(r, col);
                    let mut cur_d = Cursor { offset: start_off, row: r, desired_col: col };
                    for _ in 0..max_steps {
                        match move_down_within(&map, cur_d) {
                            Some(n) => {
                                assert_stop!(n.offset,
                                    "move_down_within(multi-col)");
                                cur_d = n;
                            }
                            None => break,
                        }
                    }
                    // --- walk UP from (r, col) ---
                    let mut cur_u = Cursor { offset: start_off, row: r, desired_col: col };
                    for _ in 0..max_steps {
                        match move_up_within(&map, cur_u) {
                            Some(n) => {
                                assert_stop!(n.offset,
                                    "move_up_within(multi-col)");
                                cur_u = n;
                            }
                            None => break,
                        }
                    }
                }
            }

            // enter_from_top and enter_from_bottom at several desired_cols.
            let mid_col = map.row_end_col.first().copied().unwrap_or(0) / 2;
            let overshoot = line.len() + 8;
            for col in [0, mid_col, overshoot] {
                let t = enter_from_top(&map, col);
                assert_stop!(t.offset, "enter_from_top");
                let b = enter_from_bottom(&map, col);
                assert_stop!(b.offset, "enter_from_bottom");
            }
        }
    }
}
