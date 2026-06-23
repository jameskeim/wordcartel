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

/// A single visual (soft-wrapped) row, ready to be drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualRow {
    /// String to paint (concealed markers removed, tabs expanded to spaces).
    pub display: String,
    /// Total display columns occupied.
    pub width: usize,
    /// Source byte range covered by the *visible* content of this row.
    pub src_span: Range<usize>,
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

    // Greedy soft-wrap.
    let mut placed: Vec<Placed> = Vec::new();
    let mut row = 0usize;
    let mut col = 0usize;
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
        if col + vg.width > vw && col > 0 {
            row_end_col.push(col);
            row += 1;
            col = 0;
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
        vec![VisualRow { display: String::new(), width: 0, src_span: 0..0 }; rows];
    let mut row_min: Vec<Option<usize>> = vec![None; rows];
    let mut row_max: Vec<Option<usize>> = vec![None; rows];
    for p in &placed {
        let vr = &mut visual_rows[p.row];
        if p.text == "\t" {
            vr.display.push_str(&" ".repeat(TAB_WIDTH));
        } else {
            vr.display.push_str(&p.text);
        }
        vr.width += p.width;
        row_min[p.row] = Some(row_min[p.row].map_or(p.src.start, |m: usize| m.min(p.src.start)));
        row_max[p.row] = Some(row_max[p.row].map_or(p.src.end, |m: usize| m.max(p.src.end)));
    }
    for r in 0..rows {
        if let (Some(a), Some(b)) = (row_min[r], row_max[r]) {
            visual_rows[r].src_span = a..b;
        }
    }

    let map = ColMap {
        placed,
        rows,
        eol: line.len(),
        row_end_col,
        is_active,
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
    let prev = stops.iter().copied().rev().find(|&s| s < cur.offset).unwrap_or(0);
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
    // snap up to the nearest cursor stop (>= raw)
    let off = map
        .cursor_stops()
        .into_iter()
        .find(|&s| s >= raw)
        .unwrap_or(map.eol);
    Cursor { offset: off, row, desired_col: end_col }
}

/// Move down one visual row within this logical line, preserving desired_col.
/// Uses the cursor's explicit row affinity (not the offset) so soft-wrap
/// boundaries don't cause drift.
pub fn move_down_within(map: &ColMap, cur: Cursor) -> Option<Cursor> {
    let row = cur.row;
    if row + 1 >= map.rows {
        return None;
    }
    let target = row + 1;
    let want = cur.desired_col;
    let off = map.visual_to_source(target, want);
    let col = map.col_on_row(off, target);
    let _ = col;
    Some(Cursor { offset: off, row: target, desired_col: want })
}

/// Move up one visual row, preserving desired_col.
pub fn move_up_within(map: &ColMap, cur: Cursor) -> Option<Cursor> {
    if cur.row == 0 {
        return None;
    }
    let target = cur.row - 1;
    let want = cur.desired_col;
    let off = map.visual_to_source(target, want);
    Some(Cursor { offset: off, row: target, desired_col: want })
}

/// Enter a logical line from above at `desired_col` (first row).
pub fn enter_from_top(map: &ColMap, desired_col: usize) -> Cursor {
    let off = map.visual_to_source(0, desired_col);
    Cursor { offset: off, row: 0, desired_col }
}

/// Enter a logical line from below at `desired_col` (last row).
pub fn enter_from_bottom(map: &ColMap, desired_col: usize) -> Cursor {
    let last = map.rows.saturating_sub(1);
    let off = map.visual_to_source(last, desired_col);
    Cursor { offset: off, row: last, desired_col }
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
    fn down_then_up_preserves_desired_col() {
        // "aaaaa" width 3 -> rows ["aaa","aa"]. start at col 2 row 0, down then up.
        let (_r, map) = layout("aaaaa", BlockRole::Paragraph, true, 3);
        let start = Cursor { offset: 2, row: 0, desired_col: 2 };
        let down = move_down_within(&map, start).unwrap();
        let up = move_up_within(&map, down).unwrap();
        assert_eq!(up.offset, start.offset);
    }
}
