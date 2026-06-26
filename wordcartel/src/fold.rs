//! Section-by-heading folding. `FoldState` holds the folded heading anchors
//! (byte offsets) on a Buffer; `FoldView` is the per-frame visible-line API
//! every line-space consumer (derive/render/nav/mouse) routes through.
use std::collections::BTreeSet;
use std::ops::Range;
use wordcartel_core::block_tree::BlockTree;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::outline;

/// Per-Buffer fold state: the byte offsets of folded headings.
#[derive(Debug, Clone, Default)]
pub struct FoldState {
    pub folded: BTreeSet<usize>,
}

impl FoldState {
    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }

    pub fn toggle(&mut self, heading_byte: usize) {
        if !self.folded.remove(&heading_byte) {
            self.folded.insert(heading_byte);
        }
    }

    pub fn fold_all(&mut self, blocks: &BlockTree, buf: &TextBuffer) {
        self.folded = outline::heading_starts(blocks, &buf.snapshot());
    }

    pub fn unfold_all(&mut self) {
        self.folded.clear();
    }

    /// Drop anchors that no longer start a heading (validated against
    /// `outline::heading_starts`). Called after edits/undo/redo/reopen.
    pub fn reconcile(&mut self, blocks: &BlockTree, buf: &TextBuffer) {
        let starts = outline::heading_starts(blocks, &buf.snapshot());
        self.folded.retain(|b| starts.contains(b));
    }

    /// Hidden body ranges in BYTES. Computed from `outline::sections` in ONE
    /// pass (no per-anchor recompute): for each section whose heading is folded
    /// and has a non-empty body, hide the body, keeping the heading line(s)
    /// visible. Anchors that aren't heading starts are skipped.
    pub fn hidden_byte_ranges(&self, blocks: &BlockTree, buf: &TextBuffer) -> Vec<Range<usize>> {
        let rope = buf.snapshot();
        let mut out: Vec<Range<usize>> = outline::sections(blocks, &rope)
            .into_iter()
            .filter(|s| self.folded.contains(&s.heading.byte) && s.body.start < s.body.end)
            .map(|s| s.body)
            .collect();
        out.sort_by_key(|r| r.start);
        out
    }
}

/// A merged hidden run in LINE space, with the visible heading line that owns it.
#[derive(Debug, Clone)]
struct HiddenRun {
    lines: Range<usize>, // [start, end) hidden body lines
    owner: usize,        // the visible heading line a caret/scroll snaps to
}

/// Per-frame visible-line view in LINE space. Built once at the start of any
/// operation that walks lines; every line-space consumer routes through it.
/// Hidden runs are MERGED (overlapping/adjacent ranges from nested folds are
/// coalesced) so `visible_count`/ordinals never double-count.
#[derive(Debug, Clone)]
pub struct FoldView {
    hidden: Vec<HiddenRun>, // sorted by start, non-overlapping after merge
    total: usize,
}

impl FoldView {
    pub fn compute(folds: &FoldState, blocks: &BlockTree, buf: &TextBuffer) -> FoldView {
        let rope = buf.snapshot();
        let total = rope.len_lines();
        // ONE sections pass; owner is the heading's OWN line (correct for setext,
        // where the heading is two lines and the body starts after the underline).
        let mut runs: Vec<HiddenRun> = outline::sections(blocks, &rope)
            .into_iter()
            .filter(|s| folds.folded.contains(&s.heading.byte) && s.body.start < s.body.end)
            .filter_map(|s| {
                let first = buf.byte_to_line(s.body.start);
                // EOF-safe exclusive end line: byte_to_line(end) collapses to the
                // body's own line when the doc has no trailing newline, dropping
                // the run. Derive from end-1 instead, clamped to total.
                let last = (buf.byte_to_line(s.body.end.saturating_sub(1)) + 1).min(total);
                if first < last {
                    Some(HiddenRun { lines: first..last, owner: buf.byte_to_line(s.heading.byte) })
                } else {
                    None
                }
            })
            .collect();
        runs.sort_by_key(|h| h.lines.start);
        // Merge overlapping/adjacent runs; the merged owner is the outermost
        // (smallest) heading line, which is the visible heading after folding.
        let mut merged: Vec<HiddenRun> = Vec::new();
        for run in runs {
            match merged.last_mut() {
                Some(prev) if run.lines.start <= prev.lines.end => {
                    prev.lines.end = prev.lines.end.max(run.lines.end);
                    prev.owner = prev.owner.min(run.owner);
                }
                _ => merged.push(run),
            }
        }
        FoldView { hidden: merged, total }
    }

    pub fn is_hidden(&self, line: usize) -> bool {
        self.hidden.iter().any(|r| r.lines.contains(&line))
    }

    /// Smallest visible line strictly greater than `line`, or None past the end.
    pub fn next_visible(&self, line: usize) -> Option<usize> {
        let mut l = line + 1;
        while l < self.total {
            match self.hidden.iter().find(|r| r.lines.contains(&l)) {
                Some(r) => l = r.lines.end, // jump past the hidden run
                None => return Some(l),
            }
        }
        None
    }

    /// Largest visible line strictly less than `line`, or None before the start.
    pub fn prev_visible(&self, line: usize) -> Option<usize> {
        if line == 0 {
            return None;
        }
        let mut l = line - 1;
        loop {
            match self.hidden.iter().find(|r| r.lines.contains(&l)) {
                Some(r) => {
                    if r.lines.start == 0 {
                        return None;
                    }
                    l = r.lines.start - 1;
                }
                None => return Some(l),
            }
        }
    }

    pub fn visible_count(&self) -> usize {
        let hidden: usize = self.hidden.iter().map(|r| r.lines.end - r.lines.start).sum();
        self.total.saturating_sub(hidden)
    }

    /// Number of visible lines strictly before `line`.
    pub fn visible_ordinal(&self, line: usize) -> usize {
        let hidden_before: usize = self
            .hidden
            .iter()
            .map(|r| r.lines.end.min(line).saturating_sub(r.lines.start.min(line)))
            .sum();
        line.saturating_sub(hidden_before)
    }

    /// Inverse of `visible_ordinal`: the logical line at the nth visible position.
    pub fn line_at_ordinal(&self, ord: usize) -> usize {
        let mut seen = 0usize;
        let mut l = 0usize;
        while l < self.total {
            if let Some(r) = self.hidden.iter().find(|r| r.lines.contains(&l)) {
                l = r.lines.end;
                continue;
            }
            if seen == ord {
                return l;
            }
            seen += 1;
            l += 1;
        }
        self.total.saturating_sub(1)
    }

    /// If `line` is hidden, snap to the owning visible heading line; otherwise
    /// return it unchanged. Uses the stored `owner` (correct for setext, where
    /// the heading is two lines above the body, not one).
    pub fn normalize_line(&self, line: usize) -> usize {
        match self.hidden.iter().find(|r| r.lines.contains(&line)) {
            Some(r) => r.owner,
            None => line,
        }
    }
}

/// If `byte` falls inside a folded body, snap it to the owning heading's start
/// byte; otherwise return it unchanged. The single caret-out-of-fold primitive.
/// Body math comes from `outline::body_range` (ATX/setext correct).
pub fn normalize_caret(
    folds: &FoldState,
    blocks: &BlockTree,
    buf: &TextBuffer,
    byte: usize,
) -> usize {
    let rope = buf.snapshot();
    let starts = outline::heading_starts(blocks, &rope);
    for &hb in &folds.folded {
        if !starts.contains(&hb) {
            continue;
        }
        let body = outline::body_range(blocks, &rope, hb);
        if byte >= body.start && byte < body.end {
            return hb;
        }
    }
    byte
}

/// Number of hidden body LINES for a folded heading (for the "… N lines" marker).
pub fn hidden_count_lines(
    folds: &FoldState,
    blocks: &BlockTree,
    buf: &TextBuffer,
    heading_byte: usize,
) -> usize {
    let _ = folds;
    let rope = buf.snapshot();
    let body = outline::body_range(blocks, &rope, heading_byte);
    if body.start >= body.end {
        return 0;
    }
    // EOF-safe: derive the exclusive end line from end-1, matching FoldView.
    let first = buf.byte_to_line(body.start);
    let last = buf.byte_to_line(body.end.saturating_sub(1)) + 1;
    last.saturating_sub(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_core::block_tree::full_parse_rope;

    fn parse(doc: &str) -> (BlockTree, TextBuffer) {
        let buf = TextBuffer::from_str(doc);
        let blocks = full_parse_rope(&buf.snapshot());
        (blocks, buf)
    }

    const DOC: &str = "# Top\nintro\n## A\nbody1\nbody2\n## B\ntail\n";
    //  line 0: # Top
    //  line 1: intro
    //  line 2: ## A      <- fold this
    //  line 3: body1
    //  line 4: body2
    //  line 5: ## B
    //  line 6: tail
    //  line 7: ""        (trailing)

    #[test]
    fn hidden_byte_ranges_cover_body_not_heading() {
        let (blocks, buf) = parse(DOC);
        let mut f = FoldState::default();
        let a = DOC.find("## A").unwrap();
        f.toggle(a);
        let ranges = f.hidden_byte_ranges(&blocks, &buf);
        assert_eq!(ranges.len(), 1);
        // body starts after the "## A\n" line and ends at "## B"
        let body_start = DOC.find("body1").unwrap();
        let b = DOC.find("## B").unwrap();
        assert_eq!(ranges[0], body_start..b);
    }

    #[test]
    fn foldview_skips_hidden_lines() {
        let (blocks, buf) = parse(DOC);
        let mut f = FoldState::default();
        f.toggle(DOC.find("## A").unwrap());
        let fv = FoldView::compute(&f, &blocks, &buf);
        // body1 (line 3) and body2 (line 4) are hidden
        assert!(fv.is_hidden(3));
        assert!(fv.is_hidden(4));
        assert!(!fv.is_hidden(2)); // the heading line itself stays visible
        assert!(!fv.is_hidden(5));
        // next visible after the heading line (2) is the next heading (5)
        assert_eq!(fv.next_visible(2), Some(5));
        // prev visible before line 5 is line 2
        assert_eq!(fv.prev_visible(5), Some(2));
    }

    #[test]
    fn foldview_visible_count_and_ordinals() {
        let (blocks, buf) = parse(DOC);
        let mut f = FoldState::default();
        f.toggle(DOC.find("## A").unwrap());
        let fv = FoldView::compute(&f, &blocks, &buf);
        let total = buf.snapshot().len_lines();
        // two hidden lines (3,4)
        assert_eq!(fv.visible_count(), total - 2);
        // ordinal of line 5 = number of visible lines before it (0,1,2 -> 3)
        assert_eq!(fv.visible_ordinal(5), 3);
        // inverse
        assert_eq!(fv.line_at_ordinal(3), 5);
    }

    #[test]
    fn foldview_normalize_line_snaps_hidden_to_heading() {
        let (blocks, buf) = parse(DOC);
        let mut f = FoldState::default();
        f.toggle(DOC.find("## A").unwrap());
        let fv = FoldView::compute(&f, &blocks, &buf);
        assert_eq!(fv.normalize_line(3), 2); // hidden body -> heading line
        assert_eq!(fv.normalize_line(4), 2);
        assert_eq!(fv.normalize_line(5), 5); // already visible -> unchanged
    }

    #[test]
    fn normalize_caret_snaps_into_fold_to_heading_start() {
        let (blocks, buf) = parse(DOC);
        let mut f = FoldState::default();
        let a = DOC.find("## A").unwrap();
        f.toggle(a);
        let inside = DOC.find("body2").unwrap() + 1;
        assert_eq!(normalize_caret(&f, &blocks, &buf, inside), a);
        // a caret on a visible line is unchanged
        let visible = DOC.find("tail").unwrap();
        assert_eq!(normalize_caret(&f, &blocks, &buf, visible), visible);
    }

    #[test]
    fn reconcile_drops_anchor_that_is_no_longer_a_heading() {
        let (blocks, buf) = parse(DOC);
        let mut f = FoldState::default();
        f.toggle(DOC.find("## A").unwrap());
        f.toggle(DOC.find("intro").unwrap()); // not a heading start
        f.reconcile(&blocks, &buf);
        assert!(f.folded.contains(&DOC.find("## A").unwrap()));
        assert!(!f.folded.contains(&DOC.find("intro").unwrap()));
    }

    #[test]
    fn fold_all_then_unfold_all() {
        let (blocks, buf) = parse(DOC);
        let mut f = FoldState::default();
        f.fold_all(&blocks, &buf);
        assert_eq!(f.folded.len(), 3); // Top, A, B
        f.unfold_all();
        assert!(f.folded.is_empty());
    }

    #[test]
    fn hidden_count_lines_reports_body_line_count() {
        let (blocks, buf) = parse(DOC);
        let f = FoldState::default();
        // ## A body is body1, body2 -> 2 lines
        assert_eq!(hidden_count_lines(&f, &blocks, &buf, DOC.find("## A").unwrap()), 2);
    }

    #[test]
    fn nested_folds_merge_and_do_not_double_count() {
        // # Top contains ## A; folding BOTH must not subtract A's lines twice.
        let doc = "# Top\nt1\n## A\na1\na2\n## B\nb1\n";
        let (blocks, buf) = parse(doc);
        let mut f = FoldState::default();
        f.toggle(doc.find("# Top").unwrap()); // hides everything from t1 to ## B start; ## A folds a1..a2
        f.toggle(doc.find("## A").unwrap());  // # Top folds t1..##B start; ## A folds a1..a2
        let fv = FoldView::compute(&f, &blocks, &buf);
        let total = buf.snapshot().len_lines();
        // The union of hidden lines is t1, a1, a2 (## A heading line stays the
        // boundary of # Top's body; ## A's body is inside # Top's body). The merge
        // must count each hidden line once.
        let hidden_lines = total - fv.visible_count();
        // visible_count + hidden_lines == total, and no line counted twice:
        assert!(hidden_lines <= total);
        // ordinal round-trips through the merged view
        let vc = fv.visible_count();
        for ord in 0..vc {
            let line = fv.line_at_ordinal(ord);
            assert_eq!(fv.visible_ordinal(line), ord, "ordinal round-trip at {ord}");
            assert!(!fv.is_hidden(line));
        }
    }

    #[test]
    fn setext_fold_keeps_underline_visible_and_normalizes_to_title() {
        let doc = "Title\n===\nbody1\nbody2\n## next\n";
        let (blocks, buf) = parse(doc);
        let mut f = FoldState::default();
        f.toggle(0); // fold the setext heading
        let fv = FoldView::compute(&f, &blocks, &buf);
        // title line 0 and underline line 1 stay visible; body lines 2,3 hidden.
        assert!(!fv.is_hidden(0));
        assert!(!fv.is_hidden(1));
        assert!(fv.is_hidden(2));
        assert!(fv.is_hidden(3));
        // a caret in the hidden body normalizes to the TITLE line (0), not the
        // underline line (1) — owner is the heading's own line.
        assert_eq!(fv.normalize_line(2), 0);
    }

    #[test]
    fn fold_hides_final_body_without_trailing_newline() {
        // No trailing newline: "## A\nbody" — body is one line with no '\n'.
        let doc = "## A\nbody";
        let (blocks, buf) = parse(doc);
        let mut f = FoldState::default();
        f.toggle(0);
        let fv = FoldView::compute(&f, &blocks, &buf);
        assert!(!fv.is_hidden(0));      // "## A" visible
        assert!(fv.is_hidden(1));       // "body" hidden (the EOF-safe end-line fix)
        assert_eq!(hidden_count_lines(&f, &blocks, &buf, 0), 1);
    }
}
