# Wordcartel Effort 5g — Outline & Folding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Structure navigation (outline picker + heading motions) and section-by-heading folding, driven by the block-tree heading hierarchy.

**Architecture:** A pure `wordcartel-core::outline` module extracts headings from the block tree. Folding's keystone is a single fold-aware visible-line API (`fold::FoldView`) that every line-space consumer in the shell routes through — `derive::rebuild` omits folded body lines from the layout cache, and the nav on-demand-layout paths, mouse hit-testing, scrollbar, page/doc-end motion, and caret normalization are all re-expressed over visible lines. Folds are byte-anchored on `Buffer`, remapped through edits (Before-biased), reconciled against the block tree, and persisted per-file.

**Tech Stack:** Rust, ropey 1.6.1, pulldown-cmark 0.13, ratatui 0.30, nucleo-matcher (existing palette stack).

## Global Constraints

- `wordcartel-core` is IO/thread-free and `#![forbid(unsafe_code)]` — `outline` and the `map_pos` change are pure, no IO, no threads.
- Byte offsets come ONLY from rope/block APIs, never synthesized — `TextBuffer::slice` asserts char boundaries and panics in all profiles on a non-boundary.
- Responsiveness is the #1 priority: no new thread/channel/async; fold computation is O(folds) per frame and makes `rebuild` lay out *fewer* lines, never more.
- The caret is NEVER inside a hidden range and `view.scroll` is ALWAYS a visible line — a single enforced invariant (`fold::normalize_caret` after every motion/jump; `FoldView::normalize_line` on every scroll write).
- Overlays are independent `Option` fields on `Editor` (no single slot); the new `outline` overlay clears every other overlay in both directions and is bound to `buffer_id`.
- Overlay reduce branches intercept KEY input only and let NON-key messages fall through (the 5e/5f starvation lesson) — verified by a no-starvation test.
- `StateEntry.folds` MUST be `#[serde(default)]` — `load_in` returns an empty session on any TOML parse failure, so a non-defaulted new field would wipe every saved session on upgrade.
- Fold anchors use a Before-biased position map, NOT the default `change::map_pos` (which is After-biased) — a heading-start anchor must survive an insertion at its own offset.
- Section folding only (no block/list/code folding); toggle / all / none only (no fold-to-level-N); inline marker only (no gutter column); overlay picker only (no side panel).

---

## File Structure

**wordcartel-core (new / modified):**
- `src/outline.rs` (NEW) — `Heading`, `headings()`, `section_range()`, `heading_starts()`. Pure.
- `src/change.rs` (MODIFY) — add `map_pos_before` (Before-biased sibling of `map_pos`).
- `src/lib.rs` (MODIFY) — `pub mod outline;`.

**wordcartel (new / modified):**
- `src/fold.rs` (NEW) — `FoldState` (per-Buffer) + `FoldView` (the per-frame visible-line API).
- `src/outline_overlay.rs` (NEW) — `OutlineOverlay` (fuzzy heading picker state).
- `src/editor.rs` (MODIFY) — `Buffer.folds`, `Editor.outline`, `open_outline`, anchor remap in `apply`, undo/redo reconcile.
- `src/derive.rs` (MODIFY) — fold-skip in the `rebuild` walk + scroll normalization.
- `src/nav.rs` (MODIFY) — fold-aware motion, on-demand layout refusal, ensure_visible/scroll, page, doc-end, mouse hit-test, caret normalize.
- `src/render.rs` (MODIFY) — fold marker on folded heading rows; scrollbar over visible-line count.
- `src/mouse.rs` (MODIFY) — scrollbar drag over visible-line count.
- `src/registry.rs` (MODIFY) — fold + heading-motion commands; diagnostics auto-unfold.
- `src/app.rs` (MODIFY) — outline overlay interception; search auto-unfold; persist/resume folds.
- `src/state.rs` (MODIFY) — `StateEntry.folds`.
- `src/save.rs` (MODIFY) — reconcile folds + normalize caret on reload/recovery.
- `src/marks.rs` (REUSE) — `record_jump` for jump-ring pushes (no change).
- `src/input.rs` / `src/keymap.rs` (MODIFY) — key binds.

---

## Task 1: Core — Before-biased `map_pos`

**Files:**
- Modify: `wordcartel-core/src/change.rs` (after `map_pos`, ~line 142)
- Test: `wordcartel-core/src/change.rs` (`#[cfg(test)]` module, near the existing `map_pos_*` tests ~line 232)

**Interfaces:**
- Consumes: existing `ChangeSet { ops: Vec<Op> }`, `Op::{Retain(usize), Delete(usize), Insert(Tendril)}`.
- Produces: `pub fn map_pos_before(pos: usize, cs: &ChangeSet) -> usize` — like `map_pos` but a position exactly at an insertion point stays BEFORE the inserted text.

- [ ] **Step 1: Write the failing test**

In the `#[cfg(test)]` module of `change.rs`:

```rust
#[test]
fn map_pos_before_keeps_anchor_before_insertion() {
    use crate::buffer::TextBuffer;
    let buf = TextBuffer::from_str("abcdef");
    // insert "XY" at offset 2
    let cs = ChangeSet::insert(2, "XY", buf.len());
    // After-biased map_pos moves 2 -> 4; the Before variant keeps it at 2.
    assert_eq!(map_pos(2, &cs), 4);
    assert_eq!(map_pos_before(2, &cs), 2);
    // positions strictly after the insertion still shift by the insert length
    assert_eq!(map_pos_before(3, &cs), 5);
    // positions strictly before are unchanged
    assert_eq!(map_pos_before(1, &cs), 1);
    // insertion at offset 0 keeps a byte-0 anchor at 0
    let cs0 = ChangeSet::insert(0, "Z", buf.len());
    assert_eq!(map_pos_before(0, &cs0), 0);
    // deletion behaves identically to map_pos (clamp to deletion start)
    let csd = ChangeSet::delete(2..4, buf.len());
    assert_eq!(map_pos_before(3, &csd), 2);
    assert_eq!(map_pos_before(5, &csd), 3);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel-core map_pos_before_keeps_anchor_before_insertion`
Expected: FAIL — `cannot find function map_pos_before`.

- [ ] **Step 3: Implement `map_pos_before`**

Add directly below `map_pos`. The ONLY difference from `map_pos` is an explicit Before guard on the `Insert` arm: when the position sits exactly at the current boundary (`pos == old`), return `new` BEFORE adding the insert length, so an insertion at the anchor stays in front of it. (A leading `Insert` at offset 0 has no preceding `Retain` to catch `pos == 0`, which is exactly the case the naive `<=`-on-Retain version got wrong — it mapped `0 → 1`.)

```rust
/// Map one byte position through a ChangeSet with insertion bias = Before.
/// A position sitting exactly at an insertion point stays BEFORE the inserted
/// text (the opposite of `map_pos`). Used for fold anchors at heading starts:
/// inserting text at the heading's first byte must not push the anchor into the
/// body. Deletion behaviour matches `map_pos` (a position inside a deletion
/// clamps to the deletion start).
pub fn map_pos_before(pos: usize, cs: &ChangeSet) -> usize {
    let mut old = 0usize;
    let mut new = 0usize;
    for op in &cs.ops {
        match op {
            Op::Retain(n) => {
                if pos < old + n { return new + (pos - old); }
                old += n; new += n;
            }
            Op::Insert(s) => {
                // Before bias: a position exactly at the insertion point stays
                // in front of the inserted text. (map_pos omits this guard, so a
                // boundary position is carried past the insert = After bias.)
                if pos == old { return new; }
                new += s.len();
            }
            Op::Delete(n) => {
                if pos < old + n { return new; }
                old += n;
            }
        }
    }
    new + pos.saturating_sub(old)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p wordcartel-core map_pos_before_keeps_anchor_before_insertion`
Expected: PASS.

- [ ] **Step 5: Run the existing change tests to confirm no regression**

Run: `cargo test -p wordcartel-core change::`
Expected: PASS (all existing `map_pos` tests unchanged).

- [ ] **Step 6: Commit**

```bash
git add wordcartel-core/src/change.rs
git commit -m "feat(core): map_pos_before (Before-biased position map for fold anchors)"
```

---

## Task 2: Core — `outline` module

**Files:**
- Create: `wordcartel-core/src/outline.rs`
- Modify: `wordcartel-core/src/lib.rs` (add `pub mod outline;` to the export list ~line 18)
- Test: `wordcartel-core/src/outline.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `block_tree::{BlockTree, Block, BlockKind::Heading(u8)}`, `BlockTree::top_level() -> &[Block]`, `Block { kind, span: Range<usize>, children: Vec<Block> }`; `&ropey::Rope` for slicing.
- Produces:
  - `pub struct Heading { pub level: u8, pub byte: usize, pub text: String }`
  - `pub fn headings(blocks: &BlockTree, rope: &Rope) -> Vec<Heading>` (document order, recurses into containers)
  - `pub fn section_range(blocks: &BlockTree, rope: &Rope, heading_byte: usize) -> std::ops::Range<usize>`
  - `pub fn heading_starts(blocks: &BlockTree, rope: &Rope) -> std::collections::BTreeSet<usize>`

- [ ] **Step 1: Write the failing tests**

Create `wordcartel-core/src/outline.rs` with the test module first:

```rust
//! Pure heading extraction over the block tree. No IO, no threads.
use crate::block_tree::{Block, BlockKind, BlockTree};
use ropey::Rope;
use std::collections::BTreeSet;
use std::ops::Range;

// (implementation added in Step 3)

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
    fn section_range_stops_at_same_or_higher_level() {
        let doc = "# Top\n\na\n\n## A\n\nb\n\n### A.1\n\nc\n\n## B\n\nd\n";
        let t = full_parse(doc);
        let r = &rope(doc);
        // ## A folds through A.1 but stops at ## B
        let a = doc.find("## A").unwrap();
        let b = doc.find("## B").unwrap();
        assert_eq!(section_range(&t, r, a), a..b);
        // # Top folds the entire rest of the document
        let top = doc.find("# Top").unwrap();
        assert_eq!(section_range(&t, r, top), top..doc.len());
        // ### A.1 stops at ## B (next heading with level <= 3)
        let a1 = doc.find("### A.1").unwrap();
        assert_eq!(section_range(&t, r, a1), a1..b);
    }

    #[test]
    fn section_range_last_heading_runs_to_eof() {
        let doc = "## only\n\ntail body\n";
        let t = full_parse(doc);
        let only = doc.find("## only").unwrap();
        assert_eq!(section_range(&t, &rope(doc), only), only..doc.len());
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
        // setext heading occupies TWO lines: "Title" + "===".
        let doc = "Title\n===\nbody1\nbody2\n## next\n";
        let t = full_parse(doc);
        let r = rope(doc);
        let h = 0usize; // setext heading starts at byte 0
        // body must start at "body1", NOT at the "===" underline line.
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel-core outline::`
Expected: FAIL — `headings`/`section_range`/`heading_starts` not defined.

- [ ] **Step 3: Implement the module body**

Insert above the `#[cfg(test)]` module:

```rust
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

/// Document-order list reused by `section_range` and `heading_starts`.
fn ordered(blocks: &BlockTree, rope: &Rope) -> Vec<Heading> {
    headings(blocks, rope)
}

/// The full section range of the heading whose start is `heading_byte`: from
/// that heading to the start of the next heading with `level <= this level`, or
/// end of document. Returns the FULL section (heading line + body). Callers hide
/// the body (heading-line-end .. range.end) and keep the heading visible.
/// If `heading_byte` is not a heading start, returns an empty range at that byte.
pub fn section_range(blocks: &BlockTree, rope: &Rope, heading_byte: usize) -> Range<usize> {
    let hs = ordered(blocks, rope);
    let Some(idx) = hs.iter().position(|h| h.byte == heading_byte) else {
        return heading_byte..heading_byte;
    };
    let level = hs[idx].level;
    let end = hs[idx + 1..]
        .iter()
        .find(|h| h.level <= level)
        .map(|h| h.byte)
        .unwrap_or_else(|| rope.len_bytes());
    heading_byte..end
}

/// The canonical set of heading-start byte offsets. `FoldState::reconcile`
/// validates anchors against THIS set (not `block_tree::role_at`, which only
/// classifies a byte's role and cannot prove a byte is a heading *start*).
pub fn heading_starts(blocks: &BlockTree, rope: &Rope) -> BTreeSet<usize> {
    ordered(blocks, rope).into_iter().map(|h| h.byte).collect()
}

/// The foldable BODY range of the heading at `heading_byte`: from the start of
/// the first line AFTER the heading's own line(s) to the section end. The
/// heading's line(s) stay visible — for a setext heading this correctly leaves
/// BOTH the title and the underline visible (body starts after `heading.end`),
/// fixing the off-by-one that `byte_to_line(heading_byte) + 1` would cause.
/// Returns an empty range (`start == end`) when the section has no body.
/// The single source of body-start math for `FoldState`/`normalize_caret`/
/// `unfold_ancestors_of`.
pub fn body_range(blocks: &BlockTree, rope: &Rope, heading_byte: usize) -> Range<usize> {
    let hs = ordered(blocks, rope);
    let Some(h) = hs.iter().find(|h| h.byte == heading_byte) else {
        return heading_byte..heading_byte;
    };
    let section_end = section_range(blocks, rope, heading_byte).end;
    // First line strictly after the heading's last own line.
    let heading_last_line = rope.byte_to_line(h.end.saturating_sub(1).max(h.byte));
    let body_start = rope.line_to_byte(heading_last_line + 1).min(section_end);
    body_start..section_end
}
```

Then add `pub mod outline;` to `wordcartel-core/src/lib.rs` (alphabetically, after `pub mod md_parse;`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p wordcartel-core outline::`
Expected: PASS (all 7 tests).

- [ ] **Step 5: Commit**

```bash
git add wordcartel-core/src/outline.rs wordcartel-core/src/lib.rs
git commit -m "feat(core): outline module (headings/section_range/heading_starts, ATX+setext)"
```

---

## Task 3: Shell — `fold.rs` (FoldState + FoldView visible-line API)

**Files:**
- Create: `wordcartel/src/fold.rs`
- Modify: `wordcartel/src/lib.rs` (add `pub mod fold;`)
- Modify: `wordcartel/src/editor.rs` (add `pub folds: crate::fold::FoldState` to `Buffer` ~line 78; initialise it everywhere a `Buffer` literal is built — `Default`/`new_from_text` paths)
- Test: `wordcartel/src/fold.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `wordcartel_core::outline::{headings, section_range, heading_starts}`, `wordcartel_core::block_tree::BlockTree`, `wordcartel_core::buffer::TextBuffer` (`byte_to_line`, `line_to_byte`, `len`), ropey `len_lines`.
- Produces:
  - `pub struct FoldState { pub folded: BTreeSet<usize> }` with `toggle/fold_all/unfold_all/reconcile/hidden_byte_ranges/is_empty`.
  - `pub struct FoldView { hidden: Vec<Range<usize>>, total: usize }` (LINE space) with `compute/is_hidden/next_visible/prev_visible/visible_count/visible_ordinal/line_at_ordinal/normalize_line`.
  - `pub fn normalize_caret(folds: &FoldState, blocks: &BlockTree, buf: &TextBuffer, byte: usize) -> usize` (BYTE space).
  - `pub fn hidden_count_lines(folds: &FoldState, blocks: &BlockTree, buf: &TextBuffer, heading_byte: usize) -> usize` (for the "… N lines" marker).

- [ ] **Step 1: Write the failing tests**

Create `wordcartel/src/fold.rs`:

```rust
//! Section-by-heading folding. `FoldState` holds the folded heading anchors
//! (byte offsets) on a Buffer; `FoldView` is the per-frame visible-line API
//! every line-space consumer (derive/render/nav/mouse) routes through.
use std::collections::BTreeSet;
use std::ops::Range;
use wordcartel_core::block_tree::BlockTree;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::outline;

// (implementation added in Step 3)

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
        f.toggle(doc.find("# Top").unwrap()); // hides everything from t1 to ## B's section? No:
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
        // underline line (1).
        assert_eq!(fv.normalize_line(2), 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel fold::`
Expected: FAIL — types/functions not defined.

- [ ] **Step 3: Implement the module body**

Insert above the test module:

```rust
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

    /// Hidden body ranges in BYTES. For each folded heading still present, hide
    /// its foldable body (`outline::body_range`), keeping the heading line(s)
    /// visible. Anchors that aren't heading starts are skipped. The body math
    /// lives in `outline::body_range` so ATX/setext are handled in one place.
    pub fn hidden_byte_ranges(&self, blocks: &BlockTree, buf: &TextBuffer) -> Vec<Range<usize>> {
        let rope = buf.snapshot();
        let starts = outline::heading_starts(blocks, &rope);
        let mut out = Vec::new();
        for &hb in &self.folded {
            if !starts.contains(&hb) {
                continue;
            }
            let body = outline::body_range(blocks, &rope, hb);
            if body.start < body.end {
                out.push(body);
            }
        }
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
        let total = buf.snapshot().len_lines();
        // (body line range, owner heading line) per folded heading.
        let mut runs: Vec<HiddenRun> = folds
            .hidden_byte_ranges(blocks, buf)
            .into_iter()
            .filter_map(|r| {
                let first = buf.byte_to_line(r.start);
                let last = buf.byte_to_line(r.end); // exclusive boundary line
                if first < last {
                    // owner = the heading line just above the body's first line.
                    Some(HiddenRun { lines: first..last, owner: first.saturating_sub(1) })
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
    buf.byte_to_line(body.end).saturating_sub(buf.byte_to_line(body.start))
}
```

Add `pub mod fold;` to `wordcartel/src/lib.rs`. Add `pub folds: crate::fold::FoldState` to the `Buffer` struct in `editor.rs` and initialise it to `crate::fold::FoldState::default()` in every `Buffer { .. }` construction site (search for `Buffer {` — the `new_from_text` path and any test builders). The `..new_buf` spread sites in `save.rs` already copy it.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p wordcartel fold::`
Expected: PASS (all `fold::` unit tests).

- [ ] **Step 5: Build the whole shell to confirm Buffer initialisation is complete**

Run: `cargo build -p wordcartel`
Expected: builds with no "missing field `folds`" errors.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/fold.rs wordcartel/src/lib.rs wordcartel/src/editor.rs
git commit -m "feat(5g): fold.rs — FoldState + FoldView visible-line API; Buffer.folds"
```

---

## Task 4: Anchor remap on edit + undo/redo reconcile

**Files:**
- Modify: `wordcartel/src/editor.rs` — `Buffer::apply` (~lines 84-101), `Buffer::undo`/`Buffer::redo` (~lines 102-113)
- Test: `wordcartel/src/editor.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `wordcartel_core::change::map_pos_before` (Task 1), `crate::fold::FoldState`, `crate::derive`/`block_tree` for reconcile after undo/redo.
- Produces: folds that survive edits (Before-biased remap) and undo/redo (full reconcile).

- [ ] **Step 1: Write the failing tests**

In the `editor.rs` test module. **The helpers `crate::edit::insert_text`,
`crate::edit::delete_range`, and `crate::editor::test_clock` do NOT exist** —
build the edit directly the way the existing marks-remap test does. Before
writing these, open `editor.rs` and copy the EXACT construction from the closest
existing `Buffer::apply` test (the marks/jump_ring remap test): it builds a
`ChangeSet` (`ChangeSet::insert(at, text, len_before)` / `ChangeSet::delete(range, len_before)`),
wraps it in a `Transaction`, supplies a `block_tree::Edit { range, new_len }`
descriptor, an `EditKind`, and a local `TestClock`. Use those real types:

```rust
// helper mirrored from the existing apply tests in this module
fn apply_insert(buf: &mut Buffer, at: usize, text: &str) {
    let len_before = buf.document.buffer.len();
    let cs = wordcartel_core::change::ChangeSet::insert(at, text, len_before);
    let txn = wordcartel_core::history::Transaction::new(cs); // match the real ctor
    let edit = wordcartel_core::block_tree::Edit { range: at..at, new_len: text.len() };
    buf.apply(txn, edit, EditKind::Insert, &TestClock::default());
}
fn apply_delete(buf: &mut Buffer, range: std::ops::Range<usize>) {
    let len_before = buf.document.buffer.len();
    let cs = wordcartel_core::change::ChangeSet::delete(range.clone(), len_before);
    let txn = wordcartel_core::history::Transaction::new(cs);
    let edit = wordcartel_core::block_tree::Edit { range: range.clone(), new_len: 0 };
    buf.apply(txn, edit, EditKind::Delete, &TestClock::default());
}

#[test]
fn fold_anchor_survives_insertion_above_it() {
    let mut ed = Editor::new_from_text("# A\n\nbody\n\n## B\n\nb2\n", None, (80, 24));
    let buf = ed.active_mut();
    let b_off = "# A\n\nbody\n\n".len(); // start of "## B"
    buf.folds.toggle(b_off);
    apply_insert(buf, 0, "X\n"); // insert above the fold
    // anchor shifts by 2 and still lands on "## B".
    assert!(buf.folds.folded.contains(&(b_off + 2)));
}

#[test]
fn fold_anchor_at_heading_start_uses_before_bias() {
    let mut ed = Editor::new_from_text("## H\nbody\n", None, (80, 24));
    let buf = ed.active_mut();
    buf.folds.toggle(0); // fold the heading at byte 0
    apply_insert(buf, 0, "Z");
    // Before-biased: the anchor stays at 0 (text is now "Z## H"), it is NOT
    // pushed to 1. (Whether 0 is still a heading start is decided later by
    // reconcile in rebuild — Task 5; here we only assert the remap bias.)
    assert!(buf.folds.folded.contains(&0));
    assert!(!buf.folds.folded.contains(&1));
}

#[test]
fn undo_does_not_panic_and_clamps_fold_anchors() {
    let mut ed = Editor::new_from_text("## H\nbody\n", None, (80, 24));
    let buf = ed.active_mut();
    buf.folds.toggle(0);
    apply_delete(buf, 0.."## H\n".len()); // delete the heading line
    buf.undo();
    // Step-4 clamp guarantees no anchor points past EOF (no panic on later slice).
    // The definitive "deleted-heading fold is dropped" check lives in Task 5
    // (after rebuild's reconcile) — see `undo_then_rebuild_drops_dead_fold`.
    let len = buf.document.buffer.len();
    assert!(buf.folds.folded.iter().all(|&b| b <= len));
}
```

> NOTE: the exact `Transaction`/`Edit`/`TestClock` constructors above are
> best-effort from the gathered API shape. Before running, open the nearest
> existing `Buffer::apply` test in `editor.rs` and match its construction
> EXACTLY (ctor names, field names). The assertions are the contract; only the
> three setup lines may need adapting.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel fold_anchor_survives_insertion_above_it fold_anchor_at_heading_start_uses_before_bias undo_does_not_panic_and_clamps_fold_anchors`
Expected: FAIL — folds not remapped (anchor unchanged / panic).

- [ ] **Step 3: Remap folds in `Buffer::apply`**

In `Buffer::apply`, beside the existing marks/jump_ring remap loops (~lines 94-98), add a fold remap using the BEFORE-biased map, then reconcile:

```rust
        // 5c: marks & ring follow the text; the expand ladder resets on any edit.
        for v in self.marks.values_mut() {
            *v = wordcartel_core::change::map_pos(*v, &cs);
        }
        for v in self.jump_ring.iter_mut() {
            *v = wordcartel_core::change::map_pos(*v, &cs);
        }
        // 5g: fold anchors are heading STARTS — use Before bias so an insertion
        // at the heading's first byte does not push the anchor into the body.
        let remapped: std::collections::BTreeSet<usize> = self
            .folds
            .folded
            .iter()
            .map(|&b| wordcartel_core::change::map_pos_before(b, &cs))
            .collect();
        self.folds.folded = remapped;
```

`reconcile` against the fresh block tree happens in `derive::rebuild` (Task 5) which runs after every edit, so no per-apply reconcile is needed here — but `apply` does not have the new block tree yet. Defer reconcile to rebuild.

- [ ] **Step 4: Reconcile folds on undo/redo**

`undo`/`redo` replace content wholesale and clear `last_edit`/`pre_edit_rope` (forcing a full reparse in `rebuild`). Folds cannot be remapped through a ChangeSet here (there isn't one), so mark them for reconcile by leaving the anchors and letting `rebuild`'s reconcile drop the invalid ones. To guarantee no stale anchor points past EOF before rebuild runs, clamp in undo/redo:

```rust
    pub fn undo(&mut self) -> bool {
        match self.document.history.undo(&mut self.document.buffer) {
            Some(sel) => {
                self.document.selection = sel;
                self.document.version += 1;
                self.last_edit = None;
                self.pre_edit_rope = None;
                self.sel_history.clear();
                // 5g: drop fold anchors now past EOF; rebuild reconciles the rest.
                let len = self.document.buffer.len();
                self.folds.folded.retain(|&b| b <= len);
                true
            }
            None => false,
        }
    }
```

Apply the identical `retain` line to `redo`.

- [ ] **Step 5: Ensure `rebuild` reconciles folds against the fresh tree (forward ref to Task 5)**

Task 5 adds `editor.active_mut().folds.reconcile(&blocks, buf)` right after the new block tree is assigned in `rebuild`. Note this dependency in the commit message; the undo/redo test's final assertion (anchors ≤ len) passes from Step 4 alone.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p wordcartel fold_anchor_survives_insertion_above_it fold_anchor_at_heading_start_uses_before_bias undo_does_not_panic_and_clamps_fold_anchors`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/editor.rs
git commit -m "feat(5g): remap fold anchors (Before-biased) on edit; clamp on undo/redo"
```

---

## Task 5: derive::rebuild — fold-skip + scroll normalize + reconcile

**Files:**
- Modify: `wordcartel/src/derive.rs` — `rebuild` (~lines 82-157)
- Test: `wordcartel/src/derive.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `crate::fold::FoldView`, `FoldState::reconcile`.
- Produces: a `line_layouts` cache that contains the folded heading line but NOT its body lines; `view.scroll` normalized to a visible line before the walk.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn rebuild_omits_folded_body_lines_from_cache() {
    let doc = "# Top\nintro\n## A\nbody1\nbody2\n## B\ntail\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
    let a = doc.find("## A").unwrap();
    ed.active_mut().folds.toggle(a);
    crate::derive::rebuild(&mut ed);
    let keys: Vec<usize> = ed.active().view.line_layouts.keys().copied().collect();
    // line 2 (## A) present; lines 3,4 (body1,body2) absent; line 5 (## B) present.
    assert!(keys.contains(&2));
    assert!(!keys.contains(&3));
    assert!(!keys.contains(&4));
    assert!(keys.contains(&5));
}

#[test]
fn rebuild_normalizes_scroll_that_a_fold_swallowed() {
    let doc = "# Top\nintro\n## A\nbody1\nbody2\n## B\ntail\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 4));
    ed.active_mut().view.scroll = 3; // park scroll on body1
    ed.active_mut().folds.toggle(doc.find("## A").unwrap());
    crate::derive::rebuild(&mut ed);
    // scroll must have snapped to the heading line (2), never a hidden line.
    assert_eq!(ed.active().view.scroll, 2);
}

#[test]
fn rebuild_reconciles_dead_fold_anchor() {
    // The definitive reconcile check relocated from Task 4: after an edit that
    // deletes a folded heading, rebuild's reconcile must DROP the anchor (the
    // Task 4 EOF-clamp alone would leave a stale non-heading anchor).
    use wordcartel_core::change::ChangeSet;
    let doc = "## H\nbody\n## K\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
    ed.active_mut().folds.toggle(0); // fold ## H
    // delete "## H\n" so byte 0 is no longer a heading start
    let len = ed.active().document.buffer.len();
    let cs = ChangeSet::delete(0.."## H\n".len(), len);
    let txn = wordcartel_core::history::Transaction::new(cs);
    let edit = wordcartel_core::block_tree::Edit { range: 0.."## H\n".len(), new_len: 0 };
    ed.active_mut().apply(txn, edit, crate::editor::EditKind::Delete, &crate::editor::TestClock::default());
    crate::derive::rebuild(&mut ed);
    // byte 0 is now "body" — not a heading start — so the fold is gone.
    assert!(!ed.active().folds.folded.contains(&0));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel rebuild_omits_folded_body_lines_from_cache rebuild_normalizes_scroll_that_a_fold_swallowed rebuild_reconciles_dead_fold_anchor`
Expected: FAIL — body lines present / scroll unchanged / dead anchor retained.

- [ ] **Step 3: Reconcile folds + build a FoldView + normalize scroll**

In `rebuild`, immediately after `editor.active_mut().document.blocks = new_blocks;`:

```rust
    editor.active_mut().document.blocks = new_blocks;
    // last_edit and pre_edit_rope were already cleared by .take() above.

    // 5g: reconcile fold anchors against the fresh tree (drops anchors no longer
    // at a heading start, e.g. after an edit/undo deleted the heading).
    {
        let b = editor.active_mut();
        let blocks = b.document.blocks.clone();
        let buf = b.document.buffer.clone();
        b.folds.reconcile(&blocks, &buf);
    }
    let fold_view = {
        let b = editor.active();
        crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer)
    };
```

(If cloning the block tree/buffer per frame is measurable, the implementer may instead compute `FoldView` first against immutable borrows and reconcile via a split borrow; the snapshot is an O(1) ropey clone and `blocks` clone is O(blocks) — acceptable for v1 and consistent with the existing per-frame parse.)

- [ ] **Step 4: Normalize the first visible line and skip folded bodies in the walk**

Replace the `first_line` computation and the `while l < total_lines` walk so the top is normalized and the loop advances past hidden bodies:

```rust
        let first_line = {
            let raw = b.view.scroll.min(total_lines.saturating_sub(1));
            fold_view.normalize_line(raw)
        };
```

…and persist the normalized scroll back so consumers agree:

```rust
    editor.active_mut().view.scroll = first_line;
```

(Place this write after the snapshot block, before the walk.)

In the walk, advance `l` past folded bodies:

```rust
    let mut l = first_line;
    while l < total_lines && visual_rows_accumulated < overscan_budget {
        // (existing layout of line `l` into line_layouts)
        let (rows, map) = layout::layout(&text, role, is_active_effective, vp_width);
        visual_rows_accumulated += rows.len();
        editor.active_mut().view.line_layouts.insert(l, (rows, map));
        // 5g: jump past any folded body that follows this line.
        l = fold_view.next_visible(l).unwrap_or(total_lines);
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p wordcartel rebuild_omits_folded_body_lines_from_cache rebuild_normalizes_scroll_that_a_fold_swallowed`
Expected: PASS.

- [ ] **Step 6: Run the full derive + render test set for no regression**

Run: `cargo test -p wordcartel derive:: render::`
Expected: PASS (no-fold path unchanged — `next_visible(l)` returns `l+1` when nothing is hidden).

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/derive.rs
git commit -m "feat(5g): rebuild omits folded bodies + normalizes scroll + reconciles folds"
```

---

## Task 6: nav — fold-aware caret motion + on-demand-layout refusal + caret normalize

**Files:**
- Modify: `wordcartel/src/nav.rs` — `move_up`/`move_down` (~284-368), `move_doc_end` (~650-656), add the private `fold_view` helper
- Modify: `wordcartel/src/commands.rs` — `Command::Move` arm (~line 357): central `normalize_caret` on the committed head
- Test: `wordcartel/src/nav.rs` + `wordcartel/src/commands.rs` (`#[cfg(test)]` modules)

**Interfaces:**
- Consumes: `crate::fold::{FoldView, normalize_caret}`.
- Produces: vertical motion that treats a folded heading as a single stop; on-demand layout only ever called on a visible line; `move_doc_end` and ALL `Command::Move` motions land on a visible line.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn move_down_skips_folded_body() {
    let doc = "# Top\nintro\n## A\nbody1\nbody2\n## B\ntail\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
    ed.active_mut().folds.toggle(doc.find("## A").unwrap());
    crate::derive::rebuild(&mut ed);
    // put caret on "## A" (line 2)
    let a = doc.find("## A").unwrap();
    ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(a);
    let next = crate::nav::move_down(&mut ed);
    // one row down from the folded heading lands on "## B" (line 5), not body1.
    let b = doc.find("## B").unwrap();
    assert_eq!(ed.active().document.buffer.byte_to_line(next), ed.active().document.buffer.byte_to_line(b));
}

#[test]
fn move_doc_end_lands_outside_a_fold() {
    let doc = "# Top\nintro\n## tail\nx\ny\nz\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
    ed.active_mut().folds.toggle(doc.find("## tail").unwrap());
    crate::derive::rebuild(&mut ed);
    let end = crate::nav::move_doc_end(&mut ed);
    // end must not be inside the hidden body; it snaps to the "## tail" heading.
    let h = doc.find("## tail").unwrap();
    assert_eq!(end, h);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel move_down_skips_folded_body move_doc_end_lands_outside_a_fold`
Expected: FAIL.

- [ ] **Step 3: Reroute cross-line motion through visible lines**

`get_or_layout`/`layout_line_on_demand`/`layout_line_active` return `ColMap` (NOT `Option<ColMap>`) — do NOT change their signatures. The correct fix is upstream: the cross-line branches of `move_up`/`move_down` must never ASK for a hidden line in the first place. Replace the bare `l + 1` / `l - 1` targets with `FoldView::next_visible(l)` / `prev_visible(l)`, so `layout_line_active` is only ever called on a visible line. Add a `debug_assert!(!fold_view(editor).is_hidden(nl))` at each call as a cheap invariant tripwire (debug builds only).

In `move_down`'s cross-line branch, replace the bare `l + 1` target with the next VISIBLE line; in `move_up`, the previous visible line:

```rust
    // move_down cross-line branch:
    None => {
        let fv = fold_view(editor); // helper: FoldView::compute over the active buffer
        match fv.next_visible(l) {
            None => h, // already on the last visible line — no-op
            Some(nl) => {
                let next_map = layout_line_active(editor, nl);
                let next_ls = derive::line_start(&editor.active().document.buffer, nl);
                let c = layout::enter_from_top(&next_map, desired);
                next_ls + c.offset
            }
        }
    }
```

```rust
    // move_up cross-line branch:
    None => {
        if l == 0 { return h; }
        let fv = fold_view(editor);
        match fv.prev_visible(l) {
            None => h,
            Some(pl) => {
                let prev_map = layout_line_active(editor, pl);
                let prev_ls = derive::line_start(&editor.active().document.buffer, pl);
                let c = layout::enter_from_bottom(&prev_map, desired);
                prev_ls + c.offset
            }
        }
    }
```

Add the private helper near the top of `nav.rs`:

```rust
fn fold_view(editor: &Editor) -> crate::fold::FoldView {
    let b = editor.active();
    crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer)
}
```

- [ ] **Step 4: Normalize `move_doc_end`**

```rust
pub fn move_doc_end(editor: &mut Editor) -> usize {
    let len = editor.active().document.buffer.len();
    editor.active_mut().desired_col = None;
    let b = editor.active();
    crate::fold::normalize_caret(&b.folds, &b.document.blocks, &b.document.buffer, len)
}
```

- [ ] **Step 4b: Central caret normalization in `Command::Move` (the invariant)**

Vertical motion and `move_doc_end` are now fold-safe, but horizontal, word, and
paragraph motions (`commands.rs`, the `Command::Move` arm ~line 357) compute raw
byte targets and write `Selection::single(new_head)` with no fold awareness — a
left/right/word/paragraph move could still land the caret inside a hidden body.
Add ONE normalization at the single point where `Command::Move` commits the new
head, which enforces the "caret always visible" invariant for EVERY motion at
once:

```rust
    // commands.rs, Command::Move arm — after computing new_head, before setting selection:
    let new_head = {
        let b = editor.active();
        crate::fold::normalize_caret(&b.folds, &b.document.blocks, &b.document.buffer, new_head)
    };
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(new_head);
```

Add a test:

```rust
#[test]
fn horizontal_move_into_fold_normalizes_to_heading() {
    let doc = "## A\nbody1\nbody2\n## B\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
    ed.active_mut().folds.toggle(doc.find("## A").unwrap());
    crate::derive::rebuild(&mut ed);
    // caret at end of "## A" line; move_right would cross into hidden "body1".
    let a_end = doc.find("## A").unwrap() + "## A".len();
    ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(a_end);
    crate::commands::dispatch_move_right(&mut ed); // use the real Command::Move path/helper
    let head = ed.active().document.selection.primary().head;
    let fv = { let b = ed.active(); crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer) };
    assert!(!fv.is_hidden(ed.active().document.buffer.byte_to_line(head)));
}
```

> Use the real `Command::Move` dispatch path the codebase exposes for tests
> (mirror an existing `commands.rs` move test). The assertion is the contract.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p wordcartel move_down_skips_folded_body move_doc_end_lands_outside_a_fold horizontal_move_into_fold_normalizes_to_heading`
Expected: PASS.

- [ ] **Step 6: Run the nav test set for no regression**

Run: `cargo test -p wordcartel nav::`
Expected: PASS (no-fold path unchanged — `next_visible`/`prev_visible` are `l±1` when nothing is hidden).

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/nav.rs wordcartel/src/commands.rs
git commit -m "feat(5g): fold-aware caret motion + central Command::Move normalize + doc-end"
```

---

## Task 7: nav — ensure_visible / scroll / page over visible lines

**Files:**
- Modify: `wordcartel/src/nav.rs` — `ensure_visible` (~378-443), `rows_before_caret` (~491-506), `advance_view_top_one_row`/`scroll_down_one`/`scroll_up_one` (~509-540), `page_step`/`move_page_up`/`move_page_down` (~665-687)
- Test: `wordcartel/src/nav.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `crate::fold::FoldView`.
- Produces: `view.scroll` is always a visible line; `ensure_visible` never pins to a hidden line; scroll-by-one and paging step over visible lines.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn ensure_visible_never_pins_scroll_to_hidden_line() {
    let doc = "# Top\nintro\n## A\nb1\nb2\nb3\n## B\ntail\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 3));
    ed.active_mut().folds.toggle(doc.find("## A").unwrap());
    crate::derive::rebuild(&mut ed);
    // caret on tail
    let tail = doc.find("tail").unwrap();
    ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(tail);
    crate::nav::ensure_visible(&mut ed);
    let fv = {
        let b = ed.active();
        crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer)
    };
    assert!(!fv.is_hidden(ed.active().view.scroll));
}

#[test]
fn scroll_down_one_steps_over_hidden_lines() {
    let doc = "# Top\nintro\n## A\nb1\nb2\n## B\ntail\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
    ed.active_mut().folds.toggle(doc.find("## A").unwrap());
    crate::derive::rebuild(&mut ed);
    ed.active_mut().view.scroll = 2;    // ## A
    ed.active_mut().view.scroll_row = 0;
    crate::nav::scroll_down_one(&mut ed);
    // next visible after line 2 is line 5 (## B), not hidden body lines 3/4.
    assert_eq!(ed.active().view.scroll, 5);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel ensure_visible_never_pins_scroll_to_hidden_line scroll_down_one_steps_over_hidden_lines`
Expected: FAIL.

- [ ] **Step 3: Make `rows_before_caret` walk visible lines**

The `(scroll + 1)..caret_line` loop must accumulate rows of VISIBLE lines only:

```rust
    let fv = fold_view(editor);
    let mut rows_before = rows_of_line(editor, scroll).saturating_sub(scroll_row);
    let mut li = fv.next_visible(scroll);
    while let Some(line_idx) = li {
        if line_idx >= caret_line { break; }
        rows_before += rows_of_line(editor, line_idx);
        li = fv.next_visible(line_idx);
    }
    Some(rows_before + caret_vrow)
```

- [ ] **Step 4: Make scroll-by-one cross to the next/prev VISIBLE line**

In `advance_view_top_one_row`, when crossing logical lines, use `next_visible`:

```rust
fn advance_view_top_one_row(editor: &mut Editor, max_scroll: usize) {
    let rows = rows_of_line(editor, editor.active().view.scroll);
    editor.active_mut().view.scroll_row += 1;
    if editor.active().view.scroll_row >= rows {
        let fv = fold_view(editor);
        match fv.next_visible(editor.active().view.scroll) {
            Some(nl) if editor.active().view.scroll < max_scroll => {
                editor.active_mut().view.scroll = nl;
                editor.active_mut().view.scroll_row = 0;
            }
            _ => {
                editor.active_mut().view.scroll_row = rows.saturating_sub(1);
            }
        }
    }
}
```

In `scroll_up_one`, cross to `prev_visible`:

```rust
    } else if scroll > 0 {
        let fv = fold_view(editor);
        if let Some(prev) = fv.prev_visible(scroll) {
            let rows = rows_of_line(editor, prev);
            let v = &mut editor.active_mut().view;
            v.scroll = prev;
            v.scroll_row = rows.saturating_sub(1);
        }
    }
```

- [ ] **Step 5: Normalize scroll at the end of `ensure_visible`**

After the existing clamps and the caret-above-scroll early returns, guard every scroll write through the FoldView. Add at the start of `ensure_visible` (non-typewriter path) a normalization of the current scroll, and after the `if l < scroll` branch sets `scroll = l`, snap it:

```rust
    // 5g: scroll must be a visible line at all times.
    {
        let fv = fold_view(editor);
        let s = editor.active().view.scroll;
        let ns = fv.normalize_line(s);
        if ns != s { editor.active_mut().view.scroll = ns; editor.active_mut().view.scroll_row = 0; }
    }
```

And in the caret-above branch, set `scroll = fv.normalize_line(l)` instead of raw `l`. (Caret `l` is already visible because Task 6 normalizes the caret, so `normalize_line(l) == l`; the call is defensive.)

- [ ] **Step 5b: Make the typewriter branch fold-aware**

The typewriter branch of `ensure_visible` (~nav.rs:379-402) returns BEFORE the
Step-5 normalization and walks dense logical lines (`for li in 0..l` and the
`'outer: for li in 0..total` solver), so folded body rows still affect centering
and the resulting `(scroll, scroll_row)` can be a hidden line. Build one
`FoldView` at the top of the branch and route both walks through visible lines:

```rust
    if editor.view_opts.typewriter {
        let edit_height = (editor.active().view.area.1 as usize).saturating_sub(1);
        if edit_height == 0 { return; }
        let fv = fold_view(editor);
        let anchor = editor.view_opts.typewriter_anchor.clamp(0.0, 1.0);
        let anchor_row = ((edit_height as f32 * anchor).round() as usize).min(edit_height - 1);
        let l = caret_line(editor);
        let cvr = caret_visual_row(editor, l);
        let text_width = text_geometry(editor).text_width as usize;
        // caret absolute visual row: sum rows of VISIBLE lines before `l` only.
        let mut caret_abs = cvr;
        let mut li = if l == 0 { None } else { fv.prev_visible(l) };
        // walk visible predecessors of `l`
        let mut cursor = l;
        while let Some(p) = (if cursor == 0 { None } else { fv.prev_visible(cursor) }) {
            caret_abs += typewriter_rows_of_line(editor, p, text_width);
            cursor = p;
        }
        let _ = li;
        let target_top = caret_abs.saturating_sub(anchor_row);
        // convert target_top -> (scroll, scroll_row) walking VISIBLE lines.
        let mut acc = 0usize; let mut scroll = 0usize; let mut scroll_row = 0usize;
        let mut vline = Some(0usize);
        while let Some(li2) = vline {
            let rows = rows_of_line(editor, li2);
            if acc + rows > target_top { scroll = li2; scroll_row = target_top - acc; break; }
            acc += rows; scroll = li2; scroll_row = rows.saturating_sub(1);
            vline = fv.next_visible(li2);
        }
        editor.active_mut().view.scroll = fv.normalize_line(scroll);
        editor.active_mut().view.scroll_row = scroll_row;
        return;
    }
```

> The implementer should simplify the predecessor walk to one loop (the draft
> above keeps two cursors for clarity); the contract is: only visible lines
> contribute rows, and the final `scroll` is `normalize_line`-clean.

Add a test:

```rust
#[test]
fn typewriter_scroll_is_visible_under_folds() {
    let doc = "# Top\nintro\n## A\nb1\nb2\nb3\n## B\nt1\nt2\nt3\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 6));
    ed.active_mut().view_opts.typewriter = true;
    ed.active_mut().folds.toggle(doc.find("## A").unwrap());
    crate::derive::rebuild(&mut ed);
    let t2 = doc.find("t2").unwrap();
    ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(t2);
    crate::nav::ensure_visible(&mut ed);
    let fv = { let b = ed.active(); crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer) };
    assert!(!fv.is_hidden(ed.active().view.scroll));
}
```

- [ ] **Step 6: Paging composes for free**

`move_page_up`/`move_page_down` call `move_up`/`move_down` (Task 6, now fold-aware), so they already skip hidden lines. No change needed beyond confirming with a test:

```rust
#[test]
fn page_down_skips_folded_section() {
    let doc = "# Top\nintro\n## A\nb1\nb2\nb3\nb4\n## B\ntail\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 4));
    ed.active_mut().folds.toggle(doc.find("## A").unwrap());
    crate::derive::rebuild(&mut ed);
    let top = doc.find("# Top").unwrap();
    ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(top);
    let landed = crate::nav::move_page_down(&mut ed);
    let fv = { let b = ed.active(); crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer) };
    assert!(!fv.is_hidden(ed.active().document.buffer.byte_to_line(landed)));
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p wordcartel ensure_visible_never_pins_scroll_to_hidden_line scroll_down_one_steps_over_hidden_lines page_down_skips_folded_section typewriter_scroll_is_visible_under_folds`
Expected: PASS.

- [ ] **Step 8: Run the nav test set for no regression**

Run: `cargo test -p wordcartel nav::`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add wordcartel/src/nav.rs
git commit -m "feat(5g): fold-aware ensure_visible/scroll/page (visible-line accounting)"
```

---

## Task 8: mouse + render scrollbar — visible-line count; hit-test over visible lines

**Files:**
- Modify: `wordcartel/src/nav.rs` — `offset_at_cell` (~749-777)
- Modify: `wordcartel/src/render.rs` — scrollbar (~382-392)
- Modify: `wordcartel/src/mouse.rs` — scrollbar click + drag (~138-151, 202-216)
- Test: `wordcartel/src/nav.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `crate::fold::FoldView`.
- Produces: clicks resolve to visible lines only; scrollbar ratio/drag use visible-line count and map to visible lines.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn offset_at_cell_never_returns_a_hidden_line() {
    let doc = "# Top\nintro\n## A\nb1\nb2\n## B\ntail\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
    ed.active_mut().folds.toggle(doc.find("## A").unwrap());
    crate::derive::rebuild(&mut ed);
    // The render shows: line0,line1,line2(## A),line5(## B),line6(tail)...
    // Row 3 in the editing area is "## B" — clicking it must resolve into line 5,
    // never a hidden body line.
    let off = crate::nav::offset_at_cell(&ed, 0, 3).unwrap();
    let fv = { let b = ed.active(); crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer) };
    assert!(!fv.is_hidden(ed.active().document.buffer.byte_to_line(off)));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel offset_at_cell_never_returns_a_hidden_line`
Expected: FAIL (the `line += 1` walk visits hidden lines).

- [ ] **Step 3: Make `offset_at_cell` walk visible lines**

Replace the inner `line += 1` advance with `FoldView::next_visible`:

```rust
pub fn offset_at_cell(editor: &Editor, col: u16, row: u16) -> Option<usize> {
    let text_left = text_geometry(editor).text_left;
    let col = col.saturating_sub(text_left);
    let target = row as usize;
    let scroll = editor.active().view.scroll;
    let scroll_row = editor.active().view.scroll_row;
    let total = derive::total_logical_lines(&editor.active().document.buffer);
    let fv = fold_view(editor);
    let mut acc = 0usize;
    let mut line = Some(scroll);
    while let Some(l) = line {
        if l >= total { break; }
        let rows = rows_of_line(editor, l);
        let first_vrow = if l == scroll { scroll_row } else { 0 };
        for vrow in first_vrow..rows {
            if acc == target {
                let map = get_or_layout(editor, l);
                let in_off = map.visual_to_source(vrow, col as usize);
                let snapped = map.snap_to_stop(in_off);
                return Some(derive::line_start(&editor.active().document.buffer, l) + snapped);
            }
            acc += 1;
        }
        line = fv.next_visible(l);
    }
    None
}
```

- [ ] **Step 4: Scrollbar ratio over visible-line count (render.rs)**

```rust
    if editor.mouse.scrollbar_visible {
        let fv = crate::fold::FoldView::compute(
            &editor.active().folds,
            &editor.active().document.blocks,
            &editor.active().document.buffer,
        );
        let total = fv.visible_count();
        let scroll_pos = fv.visible_ordinal(editor.active().view.scroll);
        let sb_area = Rect::new(area.x, edit_top, w, edit_height);
        let mut sb_state = ScrollbarState::new(total).position(scroll_pos);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            sb_area,
            &mut sb_state,
        );
    }
```

- [ ] **Step 5: Scrollbar click + drag map to visible lines (mouse.rs)**

In BOTH the click (`CellHit::Scrollbar`) and `Drag(Left)` blocks, replace the `total_logical_lines` / `max_scroll` math with visible-line mapping:

```rust
                let fv = crate::fold::FoldView::compute(
                    &editor.active().folds,
                    &editor.active().document.blocks,
                    &editor.active().document.buffer,
                );
                let vis = fv.visible_count();
                let max_ord = vis.saturating_sub(1);
                let erow_in_track = ev.row.saturating_sub(menu_rows) as usize;
                let new_ord = if edit_height > 0 {
                    ((erow_in_track * max_ord) / edit_height).min(max_ord)
                } else {
                    0
                };
                editor.active_mut().view.scroll = fv.line_at_ordinal(new_ord);
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p wordcartel offset_at_cell_never_returns_a_hidden_line`
Expected: PASS.

- [ ] **Step 7: Run mouse + render + nav tests for no regression**

Run: `cargo test -p wordcartel mouse:: render:: nav::`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add wordcartel/src/nav.rs wordcartel/src/render.rs wordcartel/src/mouse.rs
git commit -m "feat(5g): mouse hit-test + scrollbar over visible-line count"
```

---

## Task 9: render — fold marker (▸ / … N lines)

**Files:**
- Modify: `wordcartel/src/render.rs` — the main row loop (~214-376), at the point a folded heading row is painted
- Test: `wordcartel/src/render.rs` (`#[cfg(test)]` module, mirror an existing render snapshot/string test)

**Interfaces:**
- Consumes: `crate::fold::hidden_count_lines`, `Buffer.folds`, the existing per-row span builder.
- Produces: a folded heading row rendered with a leading `▸ ` and a trailing dim `… N lines`.

- [ ] **Step 1: Write the failing test**

Mirror the codebase's existing render-to-string helper (find a test that renders into a `TestBackend`/buffer and asserts on cell contents). Assert:

```rust
#[test]
fn folded_heading_row_shows_marker_and_count() {
    let doc = "## A\nb1\nb2\n## B\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (40, 10));
    ed.active_mut().folds.toggle(doc.find("## A").unwrap());
    crate::derive::rebuild(&mut ed);
    let text = crate::render::render_to_string(&mut ed); // use the project's helper
    assert!(text.contains("▸"), "expected fold marker, got:\n{text}");
    assert!(text.contains("… 2 lines"), "expected hidden-line count, got:\n{text}");
    // the body lines are not painted
    assert!(!text.contains("b1"));
    assert!(!text.contains("b2"));
}
```

> If no `render_to_string` helper exists, add a minimal one in the test module that walks a `ratatui::buffer::Buffer` after `render::draw`, OR assert on `ed.active().view.line_layouts` keys + a new `fold_marker_for(editor, line)` pure helper (preferred — keeps the test off the TUI backend). Choose the pure-helper route:

```rust
#[test]
fn fold_marker_helper_reports_marker_for_folded_heading() {
    let doc = "## A\nb1\nb2\n## B\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (40, 10));
    ed.active_mut().folds.toggle(doc.find("## A").unwrap());
    crate::derive::rebuild(&mut ed);
    let a_line = 0usize; // "## A"
    assert_eq!(crate::render::fold_marker_for(&ed, a_line), Some(2)); // 2 hidden lines
    assert_eq!(crate::render::fold_marker_for(&ed, 3), None);          // "## B" not folded
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel fold_marker_helper_reports_marker_for_folded_heading`
Expected: FAIL — `fold_marker_for` not defined.

- [ ] **Step 3: Add the pure marker helper + render the marker**

Add to `render.rs`:

```rust
/// If logical line `l` is the heading line of a folded section, return the hidden
/// body line count; otherwise None. Pure — drives both the marker glyph and tests.
pub fn fold_marker_for(editor: &crate::editor::Editor, l: usize) -> Option<usize> {
    let b = editor.active();
    let buf = &b.document.buffer;
    // The folded anchor whose heading line is `l`.
    let hb = b.folds.folded.iter().copied().find(|&hb| buf.byte_to_line(hb) == l)?;
    Some(crate::fold::hidden_count_lines(&b.folds, &b.document.blocks, buf, hb))
}
```

In the row loop, when `row_index == 0` for a logical line `l`, prepend the marker span and append the count span if `fold_marker_for(editor, l)` is `Some(n)`:

```rust
    // 5g: fold marker on the heading's first visual row.
    if row_index == skip_rows {
        if let Some(n) = fold_marker_for(editor, l) {
            spans.insert(0, Span::styled("▸ ", RStyle::default().fg(Color::DarkGray)));
            spans.push(Span::styled(
                format!("  … {n} lines"),
                RStyle::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
            ));
        }
    }
```

(Insert this just before `let line_widget = Line::from(spans);`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p wordcartel fold_marker_helper_reports_marker_for_folded_heading`
Expected: PASS.

- [ ] **Step 5: Run render tests for no regression**

Run: `cargo test -p wordcartel render::`
Expected: PASS (no-fold rows unchanged — `fold_marker_for` returns None).

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/render.rs
git commit -m "feat(5g): render fold marker (▸ + dim '… N lines') on folded heading rows"
```

---

## Task 10: Commands — fold toggle/all/none + heading motions (B)

**Files:**
- Modify: `wordcartel/src/registry.rs` — register `fold_toggle`, `fold_all`, `unfold_all`, `heading_next`, `heading_prev`, `heading_parent`
- Modify: `wordcartel/src/marks.rs` — reuse `record_jump` (no change; called from registry)
- Test: `wordcartel/src/registry.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `wordcartel_core::outline::{headings, section_range}`, `crate::fold::normalize_caret`, `crate::marks::record_jump`, `Ctx { editor, .. }`, `CommandResult::Handled`.
- Produces: registered command ids `fold_toggle`/`fold_all`/`unfold_all`/`heading_next`/`heading_prev`/`heading_parent`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn fold_toggle_folds_caret_section_and_moves_caret_to_heading() {
    let doc = "# Top\nintro\n## A\nbody1\nbody2\n## B\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
    crate::derive::rebuild(&mut ed);
    // caret inside ## A's body
    let inside = doc.find("body2").unwrap();
    ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(inside);
    dispatch_id(&mut ed, "fold_toggle"); // test helper that builds Ctx and dispatches
    let a = doc.find("## A").unwrap();
    assert!(ed.active().folds.folded.contains(&a));
    // caret moved out of the now-hidden body, onto the heading
    assert_eq!(ed.active().document.selection.primary().head, a);
}

#[test]
fn heading_next_prev_parent_navigate_and_push_ring() {
    let doc = "# Top\nintro\n## A\nbody\n### A1\nx\n## B\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
    crate::derive::rebuild(&mut ed);
    let top = doc.find("# Top").unwrap();
    ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(top);
    dispatch_id(&mut ed, "heading_next");
    assert_eq!(ed.active().document.selection.primary().head, doc.find("## A").unwrap());
    // ring got the origin pushed
    assert!(ed.active().jump_ring.contains(&top));
    // parent of ### A1 is ## A
    let a1 = doc.find("### A1").unwrap();
    ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(a1);
    dispatch_id(&mut ed, "heading_parent");
    assert_eq!(ed.active().document.selection.primary().head, doc.find("## A").unwrap());
}
```

> Use/extend the registry test helper that dispatches a command id (mirror an existing `registry.rs` test that builds a `Ctx`).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel fold_toggle_folds_caret_section heading_next_prev_parent`
Expected: FAIL — unknown commands.

- [ ] **Step 3: Register the fold commands**

In the registry build function, mirroring the `find`/`replace` pattern:

```rust
    r.register("fold_toggle", "Fold/Unfold Section", Some(MenuCategory::View), |c| {
        let caret = c.editor.active().document.selection.primary().head;
        let (blocks, buf) = {
            let b = c.editor.active();
            (b.document.blocks.clone(), b.document.buffer.clone())
        };
        let rope = buf.snapshot();
        // The heading whose section encloses the caret: the nearest heading start
        // at or before the caret.
        let hs = wordcartel_core::outline::headings(&blocks, &rope);
        if let Some(h) = hs.iter().rev().find(|h| h.byte <= caret) {
            c.editor.active_mut().folds.toggle(h.byte);
            // caret can't sit in a now-hidden body: normalize it.
            let b = c.editor.active();
            let nc = crate::fold::normalize_caret(&b.folds, &b.document.blocks, &b.document.buffer, caret);
            c.editor.active_mut().document.selection =
                wordcartel_core::selection::Selection::single(nc);
            crate::derive::rebuild(c.editor);
            crate::nav::ensure_visible(c.editor);
        } else {
            c.editor.status = "no heading at cursor".into();
        }
        CommandResult::Handled
    });
    r.register("fold_all", "Fold All Sections", Some(MenuCategory::View), |c| {
        let (blocks, buf) = { let b = c.editor.active(); (b.document.blocks.clone(), b.document.buffer.clone()) };
        c.editor.active_mut().folds.fold_all(&blocks, &buf);
        let caret = c.editor.active().document.selection.primary().head;
        let b = c.editor.active();
        let nc = crate::fold::normalize_caret(&b.folds, &b.document.blocks, &b.document.buffer, caret);
        c.editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(nc);
        crate::derive::rebuild(c.editor);
        crate::nav::ensure_visible(c.editor);
        CommandResult::Handled
    });
    r.register("unfold_all", "Unfold All Sections", Some(MenuCategory::View), |c| {
        c.editor.active_mut().folds.unfold_all();
        crate::derive::rebuild(c.editor);
        crate::nav::ensure_visible(c.editor);
        CommandResult::Handled
    });
```

- [ ] **Step 4: Register the heading motions (B)**

```rust
    r.register("heading_next", "Next Heading", None, |c| { heading_jump(c, Dirn::Next); CommandResult::Handled });
    r.register("heading_prev", "Previous Heading", None, |c| { heading_jump(c, Dirn::Prev); CommandResult::Handled });
    r.register("heading_parent", "Parent Heading", None, |c| { heading_jump(c, Dirn::Parent); CommandResult::Handled });
```

Add the shared helper (private fn in `registry.rs`):

```rust
enum Dirn { Next, Prev, Parent }

fn heading_jump(c: &mut Ctx, dir: Dirn) {
    let caret = c.editor.active().document.selection.primary().head;
    let (blocks, buf) = { let b = c.editor.active(); (b.document.blocks.clone(), b.document.buffer.clone()) };
    let rope = buf.snapshot();
    let hs = wordcartel_core::outline::headings(&blocks, &rope);
    let target = match dir {
        Dirn::Next => hs.iter().find(|h| h.byte > caret).map(|h| h.byte),
        Dirn::Prev => hs.iter().rev().find(|h| h.byte < caret).map(|h| h.byte),
        Dirn::Parent => {
            // current section's heading, then nearest preceding heading of lower level
            let cur = hs.iter().rev().find(|h| h.byte <= caret);
            match cur {
                Some(cur) => hs.iter().rev().find(|h| h.byte < cur.byte && h.level < cur.level).map(|h| h.byte),
                None => None,
            }
        }
    };
    if let Some(t) = target {
        crate::marks::record_jump(c.editor.active_mut(), caret); // push origin
        // auto-unfold an ancestor that hides the target, then move + reveal.
        unfold_ancestors_of(c.editor, t);
        c.editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(t);
        crate::derive::rebuild(c.editor);
        crate::nav::ensure_visible(c.editor);
    } else {
        c.editor.status = "no heading".into();
    }
}

/// Unfold every folded heading whose BODY contains `byte` (so a jump target
/// hidden inside a folded ancestor becomes visible). Body math comes from
/// `outline::body_range` (ATX/setext correct, single source).
pub(crate) fn unfold_ancestors_of(editor: &mut crate::editor::Editor, byte: usize) {
    let (blocks, buf) = { let b = editor.active(); (b.document.blocks.clone(), b.document.buffer.clone()) };
    let rope = buf.snapshot();
    let anchors: Vec<usize> = editor.active().folds.folded.iter().copied().collect();
    for hb in anchors {
        let body = wordcartel_core::outline::body_range(&blocks, &rope, hb);
        if byte >= body.start && byte < body.end {
            editor.active_mut().folds.folded.remove(&hb);
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p wordcartel fold_toggle_folds_caret_section heading_next_prev_parent`
Expected: PASS.

- [ ] **Step 6: Run the registry test set for no regression**

Run: `cargo test -p wordcartel registry::`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/registry.rs
git commit -m "feat(5g): fold + heading-motion commands (caret-out-of-fold, jump-ring, auto-unfold)"
```

---

## Task 11: Outline picker (A) — overlay + interception + jump

**Files:**
- Create: `wordcartel/src/outline_overlay.rs`
- Modify: `wordcartel/src/lib.rs` (add `pub mod outline_overlay;`)
- Modify: `wordcartel/src/editor.rs` (add `pub outline: Option<crate::outline_overlay::OutlineOverlay>`; add `open_outline`; clear `self.outline = None` in every OTHER opener)
- Modify: `wordcartel/src/registry.rs` (register `outline` command)
- Modify: `wordcartel/src/app.rs` (outline interception block beside palette/search/diag; render overlay)
- Modify: `wordcartel/src/render.rs` (paint the outline overlay — mirror palette paint)
- Test: `wordcartel/src/outline_overlay.rs` + `wordcartel/src/app.rs` test modules

**Interfaces:**
- Consumes: `wordcartel_core::outline::headings`, the existing `nucleo`/palette fuzzy helper, `registry::unfold_ancestors_of`, `marks::record_jump`.
- Produces: `OutlineOverlay { buffer_id, query, cursor, rows: Vec<OutlineRow>, selected }`; `Editor::open_outline`; command `outline`.

- [ ] **Step 1: Write the failing tests**

```rust
// outline_overlay.rs
#[test]
fn overlay_lists_headings_indented_and_filters() {
    let doc = "# Top\n## Alpha\n## Beta\n### Beta1\n";
    let buf = wordcartel_core::buffer::TextBuffer::from_str(doc);
    let blocks = wordcartel_core::block_tree::full_parse_rope(&buf.snapshot());
    let mut ov = OutlineOverlay::open(crate::editor::BufferId(7), &blocks, &buf.snapshot());
    assert_eq!(ov.rows.len(), 4);
    assert_eq!(ov.rows[0].indent, 0); // level 1
    assert_eq!(ov.rows[3].indent, 2); // level 3 (### Beta1)
    ov.set_query("beta", &blocks, &buf.snapshot());
    assert!(ov.rows.iter().all(|r| r.text.to_lowercase().contains("beta")));
}
```

```rust
// app.rs
#[test]
fn outline_overlay_does_not_starve_background_messages() {
    // mirror search_does_not_starve_filterdone: open outline, feed a non-key Msg,
    // assert it is handled (e.g. a FilterDone applies) while the overlay stays open.
}

#[test]
fn outline_jump_auto_unfolds_ancestor_and_moves_caret() {
    let doc = "# Top\nintro\n## A\nbody\n### A1\nx\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
    ed.active_mut().folds.toggle(doc.find("## A").unwrap()); // hide A's body incl. A1
    crate::derive::rebuild(&mut ed);
    let a1 = doc.find("### A1").unwrap();
    // simulate selecting A1 in the overlay and pressing Enter:
    crate::app::outline_jump_to(&mut ed, a1);
    assert_eq!(ed.active().document.selection.primary().head, a1);
    // ancestor ## A was unfolded so A1 is visible
    assert!(!ed.active().folds.folded.contains(&doc.find("## A").unwrap()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel overlay_lists_headings_indented_and_filters outline_jump_auto_unfolds_ancestor`
Expected: FAIL — types/functions not defined.

- [ ] **Step 3: Implement `outline_overlay.rs`**

```rust
//! Fuzzy heading picker overlay. XOR with the other overlays; bound to buffer_id.
use wordcartel_core::block_tree::BlockTree;
use ropey::Rope;

#[derive(Debug, Clone)]
pub struct OutlineRow {
    pub byte: usize,
    pub indent: usize, // level - 1
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct OutlineOverlay {
    pub buffer_id: crate::editor::BufferId,
    pub query: String,
    pub cursor: usize,
    pub rows: Vec<OutlineRow>,
    pub selected: usize,
    all: Vec<OutlineRow>, // unfiltered, document order
}

impl OutlineOverlay {
    pub fn open(buffer_id: crate::editor::BufferId, blocks: &BlockTree, rope: &Rope) -> OutlineOverlay {
        let all: Vec<OutlineRow> = wordcartel_core::outline::headings(blocks, rope)
            .into_iter()
            .map(|h| OutlineRow { byte: h.byte, indent: (h.level as usize).saturating_sub(1), text: h.text })
            .collect();
        OutlineOverlay { buffer_id, query: String::new(), cursor: 0, rows: all.clone(), selected: 0, all }
    }

    pub fn set_query(&mut self, q: &str, _blocks: &BlockTree, _rope: &Rope) {
        self.query = q.to_string();
        if q.is_empty() {
            self.rows = self.all.clone();
        } else {
            // reuse the project's fuzzy matcher (same as palette::rebuild_rows).
            self.rows = crate::palette::fuzzy_filter(&self.all, q, |r| &r.text);
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }
}
```

> **`crate::palette::fuzzy_filter` does NOT exist** — the nucleo matching is currently embedded inside `palette::rebuild_rows`. Before this step compiles, factor the nucleo call out of `rebuild_rows` into a reusable generic helper and have `rebuild_rows` call it, then call it here too (DRY). Add this as Step 3a:

- [ ] **Step 3a: Factor a reusable fuzzy helper out of `palette::rebuild_rows`**

In `palette.rs`, extract the nucleo matching into:

```rust
/// Fuzzy-rank `items` against `query` by each item's key string, best-first.
/// Returns the matching items (cloned). Shared by the palette and the outline overlay.
pub fn fuzzy_filter<T: Clone>(items: &[T], query: &str, key: impl Fn(&T) -> &str) -> Vec<T> {
    // move the existing nucleo Matcher + Pattern construction from rebuild_rows here
    // ...
}
```

Refactor `rebuild_rows` to call `fuzzy_filter`, run `cargo test -p wordcartel palette::` to confirm no behaviour change, then proceed.

- [ ] **Step 4: Wire the overlay field + opener (editor.rs)**

Add `pub outline: Option<crate::outline_overlay::OutlineOverlay>` to `Editor`. Add to EVERY existing opener (`open_minibuffer`, `open_prompt`, `open_palette`, `open_search`, `open_diag`, and the menu opener) the line `self.outline = None;`. Add:

```rust
    pub fn open_outline(&mut self) {
        self.prompt = None; self.minibuffer = None; self.palette = None; self.menu = None;
        self.search = None; self.diag = None;
        self.pending_keys.clear(); self.pending_mark = None;
        let bid = self.active().id;
        let blocks = self.active().document.blocks.clone();
        let rope = self.active().document.buffer.snapshot();
        self.outline = Some(crate::outline_overlay::OutlineOverlay::open(bid, &blocks, &rope));
    }
```

- [ ] **Step 5: Register the `outline` command (registry.rs)**

```rust
    r.register("outline", "Outline…", Some(MenuCategory::View), |c| {
        c.editor.open_outline();
        CommandResult::Handled
    });
```

- [ ] **Step 6: Interception block + jump (app.rs)**

Add `outline_jump_to` and the interception block (mirror the search block exactly — intercept key only, non-key falls through, bound to buffer_id):

```rust
pub fn outline_jump_to(editor: &mut Editor, byte: usize) {
    let origin = editor.active().document.selection.primary().head;
    crate::marks::record_jump(editor.active_mut(), origin);
    crate::registry::unfold_ancestors_of(editor, byte);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(byte);
    editor.outline = None;
    derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}
```

Interception (place beside the diag block ~after line 980):

```rust
    if editor.outline.is_some() {
        // close if the active buffer changed (stale buffer_id guard)
        if editor.outline.as_ref().map(|o| o.buffer_id) != Some(editor.active().id) {
            editor.outline = None;
        }
    }
    if editor.outline.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::KeyCode;
                match k.code {
                    KeyCode::Esc => { editor.outline = None; }
                    KeyCode::Up => { if let Some(o) = editor.outline.as_mut() { o.selected = o.selected.saturating_sub(1); } }
                    KeyCode::Down => { if let Some(o) = editor.outline.as_mut() { let max = o.rows.len().saturating_sub(1); o.selected = (o.selected + 1).min(max); } }
                    KeyCode::Enter => {
                        let target = editor.outline.as_ref().and_then(|o| o.rows.get(o.selected)).map(|r| r.byte);
                        if let Some(t) = target { outline_jump_to(editor, t); }
                    }
                    KeyCode::Backspace => {
                        if let Some(o) = editor.outline.as_mut() {
                            o.query.pop();
                            let q = o.query.clone();
                            let (blocks, rope) = { let b = editor.active(); (b.document.blocks.clone(), b.document.buffer.snapshot()) };
                            editor.outline.as_mut().unwrap().set_query(&q, &blocks, &rope);
                        }
                    }
                    KeyCode::Char(c) if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        if let Some(o) = editor.outline.as_mut() { o.query.push(c); }
                        let q = editor.outline.as_ref().unwrap().query.clone();
                        let (blocks, rope) = { let b = editor.active(); (b.document.blocks.clone(), b.document.buffer.snapshot()) };
                        editor.outline.as_mut().unwrap().set_query(&q, &blocks, &rope);
                    }
                    _ => {}
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        // Non-key messages fall through to the normal handlers below.
    }
```

- [ ] **Step 7: Paint the overlay (render.rs)**

Mirror the palette paint: if `editor.outline.is_some()`, draw a centered rectangle listing `rows` (indent each by `indent * 2` spaces), highlight `selected`, show the `query` on the top line. Reuse the palette rectangle helper.

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p wordcartel overlay_lists_headings_indented_and_filters outline_jump_auto_unfolds_ancestor outline_overlay_does_not_starve_background_messages`
Expected: PASS.

- [ ] **Step 9: Run the app + render test set for no regression**

Run: `cargo test -p wordcartel app:: render:: outline_overlay::`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add wordcartel/src/outline_overlay.rs wordcartel/src/lib.rs wordcartel/src/editor.rs wordcartel/src/registry.rs wordcartel/src/app.rs wordcartel/src/render.rs
git commit -m "feat(5g): outline picker (A) — overlay, interception, auto-unfold jump"
```

---

## Task 12: Fold composition — search + diagnostics auto-unfold

**Files:**
- Modify: `wordcartel/src/app.rs` — `search_sync`/`search_step`/`search_pin` (~464-481, 577-582)
- Modify: `wordcartel/src/registry.rs` — `diag_next`/`diag_prev` (~239-276), quick-fix caret set
- Test: `wordcartel/src/app.rs` + `wordcartel/src/registry.rs` test modules

**Interfaces:**
- Consumes: `registry::unfold_ancestors_of`.
- Produces: a search/replace match or a diagnostic jump that lands inside a fold auto-unfolds the ancestor chain before moving the caret.

- [ ] **Step 1: Write the failing tests**

```rust
// app.rs
#[test]
fn search_hit_inside_fold_auto_unfolds() {
    let doc = "# Top\nintro\n## A\nneedle here\nmore\n## B\n";
    let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
    ed.active_mut().folds.toggle(doc.find("## A").unwrap());
    crate::derive::rebuild(&mut ed);
    // open search for "needle" and step to it
    crate::app::open_search_and_find(&mut ed, "needle"); // test helper: open + sync
    let pos = doc.find("needle").unwrap();
    assert_eq!(ed.active().document.selection.primary().head, pos);
    assert!(!ed.active().folds.folded.contains(&doc.find("## A").unwrap()));
}
```

```rust
// registry.rs
#[test]
fn diag_next_into_fold_auto_unfolds() {
    // build a buffer with a diagnostic inside a folded section, dispatch diag_next,
    // assert caret on the diagnostic and the ancestor unfolded.
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel search_hit_inside_fold_auto_unfolds`
Expected: FAIL — caret in hidden body / fold still set.

- [ ] **Step 3: Auto-unfold on search caret moves**

In `search_sync`, `search_step`, and `search_pin`, before setting the selection + `ensure_visible`, unfold ancestors of the match start:

```rust
    if let Some(m) = editor.search.as_ref().and_then(|s| s.current()) {
        crate::registry::unfold_ancestors_of(editor, m.start);
        editor.active_mut().document.selection =
            wordcartel_core::selection::Selection::range(m.start, m.end);
        derive::rebuild(editor);
        crate::nav::ensure_visible(editor);
    }
```

Apply the same one-line insertion (`unfold_ancestors_of(editor, m.start);`) in all three functions.

- [ ] **Step 4: Auto-unfold on diagnostic jumps**

In `diag_next`/`diag_prev`, after computing `target` and before setting the selection:

```rust
    crate::registry::unfold_ancestors_of(c.editor, target);
    c.editor.active_mut().document.selection =
        wordcartel_core::selection::Selection::single(target);
    crate::derive::rebuild(c.editor);
    crate::nav::ensure_visible(c.editor);
```

**Quick-fix apply (app.rs ~637):** the quick-fix apply path computes `new_cursor` after applying a suggestion edit and writes the selection directly. Add the unfold there too, before the selection write:

```rust
    crate::registry::unfold_ancestors_of(editor, new_cursor);
    editor.active_mut().document.selection =
        wordcartel_core::selection::Selection::single(new_cursor);
```

(The edit may have changed headings; `unfold_ancestors_of` runs against the post-edit tree via `rebuild`, so call it after the edit's `rebuild`/derive has refreshed the block tree, or rely on the next `rebuild` to reconcile — confirm ordering against the real apply path.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p wordcartel search_hit_inside_fold_auto_unfolds diag_next_into_fold_auto_unfolds`
Expected: PASS.

- [ ] **Step 6: Run app + registry tests for no regression**

Run: `cargo test -p wordcartel app:: registry::`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/app.rs wordcartel/src/registry.rs
git commit -m "feat(5g): search + diagnostic caret jumps auto-unfold folded ancestors"
```

---

## Task 13: Persistence — StateEntry.folds (serde-default migration)

**Files:**
- Modify: `wordcartel/src/state.rs` — `StateEntry` (~14-26)
- Modify: `wordcartel/src/app.rs` — persist (~1480-1489) + resume (~1319-1327)
- Test: `wordcartel/src/state.rs` + `wordcartel/src/app.rs` test modules

**Interfaces:**
- Consumes: `Buffer.folds`, `FoldState::reconcile`, existing `apply_resume`.
- Produces: `StateEntry.folds: Vec<usize>` (`#[serde(default)]`), persisted and restored + reconciled.

- [ ] **Step 1: Write the failing tests**

```rust
// state.rs
#[test]
fn old_session_toml_without_folds_loads_with_empty_folds() {
    // an entry serialized before `folds` existed must deserialize, not wipe.
    let toml = r#"
[entries."/tmp/x.md"]
cursor = 3
scroll = 0
mtime = 1
size = 2
seq = 1
"#;
    let s: SessionState = toml::from_str(toml).expect("must deserialize without folds");
    assert!(s.entries["/tmp/x.md"].folds.is_empty());
}

#[test]
fn folds_round_trip_through_toml() {
    let mut s = SessionState::default();
    s.entries.insert("/tmp/x.md".into(), StateEntry { cursor: 0, scroll: 0, marks: Default::default(), mtime: 1, size: 2, seq: 1, folds: vec![10, 42] });
    let out = toml::to_string(&s).unwrap();
    let back: SessionState = toml::from_str(&out).unwrap();
    assert_eq!(back.entries["/tmp/x.md"].folds, vec![10, 42]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel old_session_toml_without_folds_loads_with_empty_folds folds_round_trip_through_toml`
Expected: FAIL — no `folds` field.

- [ ] **Step 3: Add the field with serde default**

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateEntry {
    pub cursor: usize,
    pub scroll: usize,
    pub marks: BTreeMap<String, usize>,
    pub mtime: i64,
    pub size: u64,
    pub seq: u64,
    /// 5g: folded heading byte-offsets. Defaulted so pre-5g session.toml loads.
    #[serde(default)]
    pub folds: Vec<usize>,
}
```

- [ ] **Step 3b: Update every `StateEntry { .. }` literal**

Adding a field breaks every struct-literal construction of `StateEntry`. Grep for them and fix each:

Run: `rg -n "StateEntry \{" wordcartel/src`

The production builder (`app.rs` persist, Step 4) gets `folds: ...` explicitly. Every OTHER literal — in `state.rs` tests and `app.rs` tests — append `folds: vec![]` (or end the literal with `..Default::default()` if it already omits optional fields). Build to confirm none are missed:

Run: `cargo build -p wordcartel --tests`
Expected: no "missing field `folds`" errors.

- [ ] **Step 4: Persist folds (app.rs)**

In the persist block, add `folds` from the active buffer:

```rust
    let entry = crate::state::StateEntry {
        cursor,
        scroll,
        marks: editor.active().marks.iter().map(|(c, &o)| (c.to_string(), o)).collect(),
        mtime,
        size,
        seq,
        folds: editor.active().folds.folded.iter().copied().collect(),
    };
```

- [ ] **Step 5: Restore + reconcile folds on resume (app.rs)**

In the resume block, after `apply_resume` accepts identity and restores cursor/scroll/marks, restore + reconcile folds:

```rust
        if let Some((cur, scroll)) = apply_resume(entry, identity, doc_len) {
            // ... existing selection/scroll/marks restore ...
            editor.active_mut().folds.folded = entry.folds.iter().copied().collect();
            let (blocks, buf) = { let b = editor.active(); (b.document.blocks.clone(), b.document.buffer.clone()) };
            editor.active_mut().folds.reconcile(&blocks, &buf);
        }
```

(`rebuild` later in startup re-normalizes scroll against the restored folds.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p wordcartel old_session_toml_without_folds_loads_with_empty_folds folds_round_trip_through_toml`
Expected: PASS.

- [ ] **Step 7: Run state + app tests for no regression**

Run: `cargo test -p wordcartel state:: app::`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add wordcartel/src/state.rs wordcartel/src/app.rs
git commit -m "feat(5g): persist + restore folds (serde-default migration, reconcile on resume)"
```

---

## Task 14: Reload/recovery — reconcile folds + normalize caret

**Files:**
- Modify: `wordcartel/src/save.rs` — `reload_from_disk` (~122-156), `load_recovered` (~160-184)
- Test: `wordcartel/src/save.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: `FoldState::reconcile`, `fold::normalize_caret`.
- Produces: after a reload/recovery, surviving fold anchors are kept, dropped if their heading vanished, and the caret is visible.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn reload_reconciles_folds_against_new_content() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("d.md");
    std::fs::write(&path, "## A\nbody\n## B\nx\n").unwrap();
    let mut ed = crate::editor::Editor::new_from_text("## A\nbody\n## B\nx\n", Some(path.clone()), (80, 24));
    crate::derive::rebuild(&mut ed);
    ed.active_mut().folds.toggle("## A\nbody\n".len()); // fold ## B
    // rewrite the file so ## B is gone
    std::fs::write(&path, "## A\nbody only\n").unwrap();
    let b_anchor = "## A\nbody\n".len(); // the ## B offset we folded
    crate::save::reload_from_disk(&mut ed);
    // STRONG assertion: the exact stale ## B anchor is gone, and the surviving
    // fold set equals exactly the post-reconcile heading-start set it should be.
    assert!(!ed.active().folds.folded.contains(&b_anchor), "stale ## B fold must be dropped");
    let starts: std::collections::BTreeSet<usize> = {
        let b = ed.active();
        wordcartel_core::outline::heading_starts(&b.document.blocks, &b.document.buffer.snapshot())
    };
    assert!(ed.active().folds.folded.iter().all(|b| starts.contains(b)),
        "every surviving fold must be a real heading start in the new content");
    // caret is visible (normalize is a no-op because it's already out of folds)
    let head = ed.active().document.selection.primary().head;
    let b = ed.active();
    assert_eq!(crate::fold::normalize_caret(&b.folds, &b.document.blocks, &b.document.buffer, head), head);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel reload_reconciles_folds_against_new_content`
Expected: FAIL (or panic) — folds not reconciled against new content.

- [ ] **Step 3: Reconcile + normalize in both buffer-replacement paths**

In `reload_from_disk` and `load_recovered`, the `*editor.active_mut() = Buffer { id, ..new_buf };` spread carries `new_buf.folds` (default empty). To PRESERVE the user's folds across a reload of the same file, copy the pre-reload folds forward then reconcile. Capture before replacement:

```rust
    let prev_folds = editor.active().folds.clone();
    // ... existing replacement ...
    *editor.active_mut() = crate::editor::Buffer { id, ..new_buf };
    // 5g: carry folds across the reload and reconcile against the new tree.
    editor.active_mut().folds = prev_folds;
    editor.active_mut().view.line_layouts.clear();
    crate::derive::rebuild(editor); // reconciles folds + normalizes scroll
    // normalize the caret out of any fold the new content created/changed.
    let head = editor.active().document.selection.primary().head;
    let nc = {
        let b = editor.active();
        crate::fold::normalize_caret(&b.folds, &b.document.blocks, &b.document.buffer, head)
    };
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(nc);
    crate::nav::ensure_visible(editor);
```

(`rebuild` already calls `folds.reconcile`, so no separate reconcile call is needed; the forward-copy + rebuild is sufficient.)

For `load_recovered`, recovered content is unsaved/edited — same forward-copy + reconcile applies.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p wordcartel reload_reconciles_folds_against_new_content`
Expected: PASS.

- [ ] **Step 5: Run save tests for no regression**

Run: `cargo test -p wordcartel save::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/save.rs
git commit -m "feat(5g): reconcile folds + normalize caret on reload/recovery"
```

---

## Task 15: Key bindings + palette reachability

**Files:**
- Modify: `wordcartel/src/input.rs` — `key_to_command_id` (test mirror, ~92-136)
- Modify: `wordcartel/src/keymap.rs` — the `CUA` table (~222-293)
- Test: `wordcartel/src/keymap.rs` + `wordcartel/src/input.rs` test modules

**Interfaces:**
- Consumes: the registered command ids from Tasks 10-11.
- Produces: collision-free CUA binds, mirrored in `input.rs` and `keymap.rs`; all seven commands palette-reachable (they already are — registered with labels).

**Bind choices (confirmed free against the current CUA inventory — Ctrl+E/F/R, F3/F8, Alt+Left/Right are taken; these are not):**

| Command | Chord |
|---------|-------|
| `outline` | `Alt+O` |
| `heading_prev` | `Alt+Up` |
| `heading_next` | `Alt+Down` |
| `heading_parent` | `Alt+Shift+Up` |
| `fold_toggle` | `Alt+Z` |
| `fold_all` | `Alt+Shift+Z` |
| `unfold_all` | `Alt+Shift+X` |

- [ ] **Step 1: Write the failing test**

```rust
// keymap.rs — NOTE: `build_cua_keymap`/`parse_chord_seq` do NOT exist. The real
// API is `build_keymap(&KeymapConfig, &Registry)` and `parse_seq(&str)`. Confirm
// the exact names against an existing keymap test before running.
#[test]
fn fold_and_outline_binds_resolve_and_do_not_collide() {
    let km = build_keymap(&KeymapConfig::default(), &Registry::builtins());
    let seq = |s: &str| parse_seq(s).unwrap();
    assert_eq!(km.resolve(&seq("alt-o")), Resolution::Command(CommandId("outline")));
    assert_eq!(km.resolve(&seq("alt-up")), Resolution::Command(CommandId("heading_prev")));
    assert_eq!(km.resolve(&seq("alt-down")), Resolution::Command(CommandId("heading_next")));
    assert_eq!(km.resolve(&seq("alt-shift-up")), Resolution::Command(CommandId("heading_parent")));
    assert_eq!(km.resolve(&seq("alt-z")), Resolution::Command(CommandId("fold_toggle")));
    assert_eq!(km.resolve(&seq("alt-shift-z")), Resolution::Command(CommandId("fold_all")));
    assert_eq!(km.resolve(&seq("alt-shift-x")), Resolution::Command(CommandId("unfold_all")));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel fold_and_outline_binds_resolve_and_do_not_collide`
Expected: FAIL — unbound (`Resolution::None`).

- [ ] **Step 3: Add the binds to the CUA table (keymap.rs)**

Append to `CUA`:

```rust
    // Outline & folding (Effort 5g)
    ("alt-o",        "outline"),
    ("alt-up",       "heading_prev"),
    ("alt-down",     "heading_next"),
    ("alt-shift-up", "heading_parent"),
    ("alt-z",        "fold_toggle"),
    ("alt-shift-z",  "fold_all"),
    ("alt-shift-x",  "unfold_all"),
```

- [ ] **Step 4: Mirror in the input.rs test map**

Add to `key_to_command_id` (keeping the test mirror in sync, as the module comment requires):

```rust
        KeyCode::Char('o') if alt && !shift => id("outline"),
        KeyCode::Up   if alt && shift       => id("heading_parent"),
        KeyCode::Up   if alt                => id("heading_prev"),
        KeyCode::Down if alt                => id("heading_next"),
        KeyCode::Char('z') if alt && !shift => id("fold_toggle"),
        KeyCode::Char('z') if alt && shift  => id("fold_all"),
        KeyCode::Char('x') if alt && shift  => id("unfold_all"),
```

**CRITICAL ordering:** the existing function has bare `KeyCode::Up => ...` / `KeyCode::Down => ...` arms (the non-shift move/select arrows). A `match` evaluates top-down, and a bare `KeyCode::Up =>` with NO guard matches FIRST and shadows any later `KeyCode::Up if alt` arm. Therefore the new `KeyCode::Up/Down if alt ...` arms MUST be placed physically ABOVE the existing bare arrow arms (and above the generic `KeyCode::Char(c) if !ctrl && !alt` insert arm). Insert them immediately after the existing F8/diag arms, before the `KeyCode::Left/Right/Up/Down` block.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p wordcartel fold_and_outline_binds_resolve_and_do_not_collide`
Expected: PASS.

- [ ] **Step 6: Run the keymap + input test set for no regression (collision guard)**

Run: `cargo test -p wordcartel keymap:: input::`
Expected: PASS — confirms the new binds don't collide with existing ones and the input/keymap mirrors stay consistent (the existing mirror-consistency test, if present, must still pass).

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/input.rs wordcartel/src/keymap.rs
git commit -m "feat(5g): bind outline/heading/fold commands (Alt+O, Alt+arrows, Alt+Z family)"
```

---

## Final Verification

- [ ] **Run the full workspace test suite**

Run: `cargo test`
Expected: PASS — all core + shell tests green (131+ core, 335+ shell, plus the new 5g tests).

- [ ] **Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Manual smoke (optional):** open a multi-heading markdown file, `Alt+O` to outline-jump, `Alt+Z` to fold a section (caret snaps to heading, `▸ … N lines` shows), `Alt+Down`/`Alt+Up` to walk headings, fold a section then search for text inside it (auto-unfolds), reopen the file (folds restored), edit above a fold (fold stays put).

---

## Self-Review Notes (coverage vs spec)

- §3.1 core outline → Task 2; §3.2 FoldState/FoldView → Task 3; anchor remap (Before-biased) + undo/redo → Tasks 1, 4.
- §4.0 visible-line API → Task 3; §4.1 rebuild fold-skip + scroll normalize → Task 5; §4.2 ten consumers → Tasks 5 (rebuild/scroll), 6 (caret motion, on-demand layout, doc-end), 7 (ensure_visible/scroll/page), 8 (mouse/scrollbar).
- §5.1 picker A → Task 11; §5.2 motions B → Task 10; §5.3 search/diag/focus composition → Task 12 (focus-dim composes for free because the caret is normalized out of folds — asserted via the no-hidden-caret invariant; no separate focus code needed).
- §6 fold marker → Task 9; keys → Task 15.
- §7 persistence (serde-default) → Task 13; reload/recovery → Task 14.
- §10 tests: every listed test maps to a task's test block.
