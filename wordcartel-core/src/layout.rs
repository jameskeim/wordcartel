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
}
