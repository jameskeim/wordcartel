# Effort 5c — Keyboard Navigation & Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add prose-grade navigation and selection — word/paragraph/page/document motion, expand/shrink-selection text objects, named marks, and a jump-back ring — atop the existing nav/selection/block-tree substrate.

**Architecture:** A new pure `wordcartel-core/src/textobj.rs` provides word/sentence boundary queries (already-present `unicode-segmentation`); paragraph spans come from the existing `block_tree`. The shell extends `nav.rs` with new motions, adds a `marks.rs`, and threads per-`Buffer` mark/ring/selection-history state through the single `Buffer::apply` mutation channel so marks follow edits exactly as the selection does. New motions reuse the existing `Command::Move { dir, extend }` dispatch so selection-extend variants fall out for free.

**Tech Stack:** Rust, `unicode-segmentation` (already a core dep), `wordcartel-core::{block_tree, change, layout, selection}`, ratatui/crossterm (no new deps).

## Global Constraints

- `#![forbid(unsafe_code)]`; `wordcartel-core` stays IO/thread-free. **No new dependencies** (unicode-segmentation + crossterm already present).
- `cargo build --workspace` must be **zero warnings**; an item unused in production until a later task/effort carries a SCOPED per-item `#[allow(dead_code)]` with a `// wired in <task/effort>` note (never a module-level allow).
- No pre-existing test may be weakened or deleted; `cargo test --workspace` stays green.
- Offsets that become a caret are always **clamped to `0..=len` then grapheme-snapped** via `ColMap::snap_to_stop` (mirrors existing nav).
- Word motion / text objects operate on the **containing leaf-block window**, never the whole document (responsiveness).
- New commands are registered via `Registry::register(id, label, menu, handler)` (5b) so they appear in the palette/menu automatically; new keybindings go in the CUA (and where apt, WordStar) static tables and must be verified free before binding.

## File Structure

| File | Responsibility | Task(s) |
|------|----------------|---------|
| `wordcartel-core/src/textobj.rs` (new) | Pure word/sentence bounds over a `&str` window | 1 |
| `wordcartel-core/src/lib.rs` | `pub mod textobj;` | 1 |
| `wordcartel-core/src/change.rs` | Public `map_pos` (extracted from selection) | 3 |
| `wordcartel-core/src/selection.rs` | `Selection::map`/`Range::map` call `change::map_pos` | 3 |
| `wordcartel/src/nav.rs` | `paragraph_range_at`, word/para/page/doc motions, `offset_at_cell` | 2,5,6,11 |
| `wordcartel/src/marks.rs` (new) | Mark/ring command bodies + helpers | 8,9 |
| `wordcartel/src/editor.rs` | Per-`Buffer` mark/ring/sel_history fields; `Editor.pending_mark`; `Buffer::apply` mapping + sel_history reset | 4 |
| `wordcartel/src/commands.rs` | New `Dir` variants + `Command` variants; `Move`-arm sel_history reset; delete-word; text objects | 5,6,7 |
| `wordcartel/src/registry.rs` | Register the new commands | 5,6,7,8,9 |
| `wordcartel/src/keymap.rs` | CUA/WordStar bindings | 5,6,8,9 |
| `wordcartel/src/app.rs` | `pending_mark` interception block; menu clears it; marks load/save | 8,10 |
| `wordcartel/src/state.rs` | (shape exists) marks save/load wiring | 10 |

**Linear task order (respects dependencies):** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11.

---

## Task 1: Core `textobj.rs` — word & sentence bounds (pure)

**Files:**
- Create: `wordcartel-core/src/textobj.rs`
- Modify: `wordcartel-core/src/lib.rs` (add `pub mod textobj;` after line 14)
- Test: `wordcartel-core/src/textobj.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `unicode_segmentation::UnicodeSegmentation` (`split_word_bound_indices`, `split_sentence_bound_indices`).
- Produces: `pub fn word_bounds(text: &str, pos: usize) -> (usize, usize)`; `pub fn next_word_start(text: &str, pos: usize) -> Option<usize>`; `pub fn prev_word_start(text: &str, pos: usize) -> Option<usize>`; `pub fn sentence_bounds(text: &str, pos: usize) -> (usize, usize)`. All offsets are byte indices into `text`; `pos` is clamped into `0..=text.len()`.

- [ ] **Step 1: Write the failing tests** in `wordcartel-core/src/textobj.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_bounds_inside_word() {
        // "the quick" — pos 5 is inside "quick" (bytes 4..9)
        assert_eq!(word_bounds("the quick", 5), (4, 9));
    }
    #[test]
    fn word_bounds_contraction_is_one_word() {
        // UAX-#29 keeps "don't" together
        assert_eq!(word_bounds("don't stop", 2), (0, 5));
    }
    #[test]
    fn word_bounds_in_whitespace_is_point() {
        // pos 3 is the space between "the" and "x"
        assert_eq!(word_bounds("the x", 3), (3, 3));
    }
    #[test]
    fn word_bounds_multibyte() {
        // "café x" — 'é' is 2 bytes; "café" spans 0..5
        assert_eq!(word_bounds("café x", 2), (0, 5));
    }
    #[test]
    fn next_and_prev_word_start() {
        let t = "alpha beta gamma";
        assert_eq!(next_word_start(t, 0), Some(6));   // start of "beta"
        assert_eq!(next_word_start(t, 6), Some(11));  // start of "gamma"
        assert_eq!(next_word_start(t, 11), None);     // no further word
        assert_eq!(prev_word_start(t, 16), Some(11)); // back to "gamma"
        assert_eq!(prev_word_start(t, 6), Some(0));   // back to "alpha"
        assert_eq!(prev_word_start(t, 0), None);
    }
    #[test]
    fn sentence_bounds_basic() {
        // Two sentences; pos inside the second
        let t = "One two. Three four.";
        assert_eq!(sentence_bounds(t, 12), (9, 20)); // "Three four."
        assert_eq!(sentence_bounds(t, 2), (0, 9));   // "One two. "
    }
    #[test]
    fn empty_window_is_safe() {
        assert_eq!(word_bounds("", 0), (0, 0));
        assert_eq!(next_word_start("", 0), None);
        assert_eq!(prev_word_start("", 0), None);
        assert_eq!(sentence_bounds("", 0), (0, 0));
    }
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel-core --lib textobj::` → FAIL (module/functions missing).

- [ ] **Step 3: Implement `textobj.rs`** (module body, above the tests):

```rust
//! Pure word/sentence boundary queries (UAX-#29). Offsets are byte indices
//! into `text`; `pos` is clamped into `0..=text.len()`. The shell passes the
//! caret's containing leaf-block slice as `text` so work is paragraph-bounded.

use unicode_segmentation::UnicodeSegmentation;

/// A "word" segment is one whose first char is alphanumeric (punctuation and
/// whitespace runs are non-words).
fn is_word(seg: &str) -> bool {
    seg.chars().next().is_some_and(char::is_alphanumeric)
}

/// (from, to) byte range of the word at `pos`. If `pos` sits in a non-word
/// (whitespace/punctuation) run, returns the zero-width point `(pos, pos)`.
pub fn word_bounds(text: &str, pos: usize) -> (usize, usize) {
    let pos = pos.min(text.len());
    for (start, seg) in text.split_word_bound_indices() {
        let end = start + seg.len();
        if pos >= start && pos < end {
            return if is_word(seg) { (start, end) } else { (pos, pos) };
        }
    }
    (pos, pos)
}

/// Start of the next word strictly after `pos`, or `None` if none remain.
pub fn next_word_start(text: &str, pos: usize) -> Option<usize> {
    let pos = pos.min(text.len());
    text.split_word_bound_indices()
        .find(|(start, seg)| *start > pos && is_word(seg))
        .map(|(start, _)| start)
}

/// Start of the word before `pos`, or `None` if at/before the first word.
pub fn prev_word_start(text: &str, pos: usize) -> Option<usize> {
    let pos = pos.min(text.len());
    text.split_word_bound_indices()
        .filter(|(start, seg)| *start < pos && is_word(seg))
        .next_back()
        .map(|(start, _)| start)
}

/// (from, to) byte range of the sentence containing `pos`, scoped to `text`.
pub fn sentence_bounds(text: &str, pos: usize) -> (usize, usize) {
    let pos = pos.min(text.len());
    for (start, seg) in text.split_sentence_bound_indices() {
        let end = start + seg.len();
        if pos >= start && pos < end {
            return (start, end);
        }
    }
    // pos == text.len(): fall to the last sentence if any.
    text.split_sentence_bound_indices()
        .next_back()
        .map(|(s, seg)| (s, s + seg.len()))
        .unwrap_or((pos, pos))
}
```

- [ ] **Step 4: Run tests + build.** `cargo test -p wordcartel-core --lib textobj::` → PASS; `cargo build -p wordcartel-core` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel-core/src/textobj.rs wordcartel-core/src/lib.rs
git commit -m "feat(core): textobj word/sentence bounds (UAX-29) — pure block-window queries"
```

---

## Task 2: `nav::paragraph_range_at` — leaf-block recursion + gap fallback

**Files:**
- Modify: `wordcartel/src/nav.rs` (add the helper + tests)
- Test: `wordcartel/src/nav.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `wordcartel_core::block_tree::{BlockTree, Block}` (`top_level()`, `Block.span: Range<usize>`, `Block.children: Vec<Block>`); `editor.active().document.blocks`; `editor.active().document.buffer` (`len()`, `byte_to_line`, `line_to_byte`).
- Produces: `pub fn paragraph_range_at(blocks: &BlockTree, buf: &TextBuffer, pos: usize) -> (usize, usize)` — total over the document.

- [ ] **Step 1: Write the failing tests** in `nav.rs` tests module:

```rust
#[test]
fn paragraph_range_selects_leaf_block_not_container() {
    // A list: paragraph_range at a list item must select the ITEM span,
    // not the whole list container.
    let mut e = Editor::new_from_text("- one\n- two\n\nAfter\n", None, (80, 24));
    derive::rebuild(&mut e);
    let buf = &e.active().document.buffer;
    let blocks = &e.active().document.blocks;
    // pos inside "two" (second list item)
    let pos = 8;
    let (from, to) = super::paragraph_range_at(blocks, buf, pos);
    let slice = buf.slice(from..to);
    assert!(slice.contains("two") && !slice.contains("one"),
        "expected the 'two' item span, got {slice:?}");
}

#[test]
fn paragraph_range_gap_falls_back_to_blank_delimited_run() {
    // "A\n\nB\n" — pos on the blank line (offset 2) has no block span;
    // fallback returns an empty/whitespace range (no panic), and a pos in
    // paragraph "B" returns the B line range.
    let mut e = Editor::new_from_text("A\n\nB\n", None, (80, 24));
    derive::rebuild(&mut e);
    let buf = &e.active().document.buffer;
    let blocks = &e.active().document.blocks;
    let (bf, bt) = super::paragraph_range_at(blocks, buf, 3); // inside "B"
    assert_eq!(buf.slice(bf..bt).trim(), "B");
    // gap: must not panic and must yield a valid (from<=to<=len) range
    let (gf, gt) = super::paragraph_range_at(blocks, buf, 2);
    assert!(gf <= gt && gt <= buf.len());
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib nav::tests::paragraph_range` → FAIL.

- [ ] **Step 3: Implement** in `nav.rs` (near the top-level helpers):

```rust
use wordcartel_core::block_tree::{Block, BlockTree};
use wordcartel_core::buffer::TextBuffer;

/// Deepest block whose span contains `pos`, searching children first so a
/// list item / blockquote paragraph wins over its container.
fn deepest_block_at(block: &Block, pos: usize) -> Option<&Block> {
    if !(pos >= block.span.start && pos < block.span.end) {
        return None;
    }
    for child in &block.children {
        if let Some(b) = deepest_block_at(child, pos) {
            return Some(b);
        }
    }
    Some(block)
}

/// The (from, to) paragraph span at `pos`. Total over the document: a leaf
/// block if `pos` is inside one, else the blank-line-delimited run around
/// `pos` (the gap fallback).
pub fn paragraph_range_at(blocks: &BlockTree, buf: &TextBuffer, pos: usize) -> (usize, usize) {
    let pos = pos.min(buf.len());
    for top in blocks.top_level() {
        if let Some(b) = deepest_block_at(top, pos) {
            return (b.span.start, b.span.end);
        }
    }
    // Gap fallback: expand to the maximal run of non-blank logical lines.
    let total = total_logical_lines(buf);
    if total == 0 {
        return (0, 0);
    }
    let line = buf.byte_to_line(pos.min(buf.len().saturating_sub(1)));
    let is_blank = |l: usize| derive::line_text(buf, l).trim().is_empty();
    if is_blank(line) {
        let s = derive::line_start(buf, line);
        return (s, s); // empty range on a blank line
    }
    let mut top_line = line;
    while top_line > 0 && !is_blank(top_line - 1) {
        top_line -= 1;
    }
    let mut bot_line = line;
    while bot_line + 1 < total && !is_blank(bot_line + 1) {
        bot_line += 1;
    }
    let from = derive::line_start(buf, top_line);
    let to = derive::line_start(buf, bot_line) + derive::line_text(buf, bot_line).len();
    (from, to)
}
```

(`total_logical_lines`, `line_start`, `line_text` are existing `derive` helpers used elsewhere in `nav.rs`; reuse them.)

- [ ] **Step 4: Run tests + build.** `cargo test -p wordcartel --lib nav::tests::paragraph_range` → PASS; `cargo build -p wordcartel` → zero warnings (the fn is consumed by Tasks 6/7; if Task 2 lands alone it is briefly unused → add `#[allow(dead_code)] // wired in Task 6/7` and remove it in Task 6).

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/nav.rs
git commit -m "feat(nav): paragraph_range_at — leaf-block recursion + blank-line gap fallback"
```

---

## Task 3: Extract `change::map_pos` (shared offset mapper)

**Files:**
- Modify: `wordcartel-core/src/change.rs` (add public `map_pos`)
- Modify: `wordcartel-core/src/selection.rs:46` (remove private `map_pos`; `Range::map`/`Selection::map` call `change::map_pos`)
- Test: `wordcartel-core/src/change.rs`

**Interfaces:**
- Produces: `pub fn map_pos(pos: usize, cs: &ChangeSet) -> usize` in `change.rs`.
- Consumes: `ChangeSet { ops: Vec<Op> }`, `Op::{Retain(usize), Insert(String), Delete(usize)}` (existing).

- [ ] **Step 1: Write the failing test** in `change.rs`:

```rust
#[test]
fn map_pos_shifts_after_insert_and_clamps_in_delete() {
    use crate::buffer::TextBuffer;
    let buf = TextBuffer::from_str("abcdef");
    // insert "XY" at offset 2 → positions >= 2 shift by 2
    let cs = ChangeSet::insert(2, "XY", buf.len());
    assert_eq!(map_pos(0, &cs), 0);
    assert_eq!(map_pos(2, &cs), 4); // bias After
    assert_eq!(map_pos(5, &cs), 7);
    // delete 2..4 → a position inside the deletion clamps to its start
    let cs2 = ChangeSet::delete(2..4, buf.len());
    assert_eq!(map_pos(3, &cs2), 2);
    assert_eq!(map_pos(5, &cs2), 3);
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel-core --lib change::tests::map_pos` → FAIL (`map_pos` not found in `change`).

- [ ] **Step 3: Move the function.** Cut the existing private `fn map_pos(pos: BytePos, cs: &ChangeSet) -> BytePos` body from `selection.rs:46` and paste it into `change.rs` as:

```rust
/// Map one byte position through a ChangeSet (insertion bias = After).
/// Shared by `Selection` mapping and 5c marks/ring mapping.
pub fn map_pos(pos: usize, cs: &ChangeSet) -> usize {
    let mut old = 0usize;
    let mut new = 0usize;
    for op in &cs.ops {
        match op {
            Op::Retain(n) => {
                if pos < old + n { return new + (pos - old); }
                old += n; new += n;
            }
            Op::Insert(s) => { new += s.len(); }
            Op::Delete(n) => {
                if pos < old + n { return new; }
                old += n;
            }
        }
    }
    new + pos.saturating_sub(old)
}
```

In `selection.rs`, replace the two call sites (`Range::map` at ~line 34 and `Selection::map` at ~line 87) so they call `crate::change::map_pos(self.anchor, cs)` / `crate::change::map_pos(self.head, cs)` etc., and delete the now-unused private `map_pos`.

- [ ] **Step 4: Run tests + build.** `cargo test -p wordcartel-core --lib change:: selection::` → PASS (the existing selection-map tests must stay green); `cargo build --workspace` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel-core/src/change.rs wordcartel-core/src/selection.rs
git commit -m "refactor(core): extract change::map_pos; Selection::map reuses it (DRY for 5c marks)"
```

---

## Task 4: Per-`Buffer` mark/ring/sel_history state + `Buffer::apply` mapping & reset

**Files:**
- Modify: `wordcartel/src/editor.rs` (Buffer fields + init at `new_from_text` ~line 156; `Buffer::apply` at line 77; `Editor.pending_mark` field + init)
- Test: `wordcartel/src/editor.rs`

**Interfaces:**
- Consumes: `change::map_pos` (Task 3); `Transaction.changes: ChangeSet` (public field, `ChangeSet: Clone`).
- Produces: on `Buffer` — `pub marks: std::collections::BTreeMap<char, usize>`, `pub jump_ring: Vec<usize>`, `pub ring_cursor: usize`, `pub sel_history: Vec<Selection>`. On `Editor` — `pub pending_mark: Option<MarkPending>` with `pub enum MarkPending { Set, Jump }`.

- [ ] **Step 1: Write the failing tests** in `editor.rs` tests:

```rust
#[test]
fn marks_follow_edits_above_them() {
    use wordcartel_core::change::ChangeSet;
    use wordcartel_core::history::Transaction;
    let clk = TestClock(0);
    let mut e = Editor::new_from_text("abcdef", None, (80, 24));
    e.active_mut().marks.insert('a', 4); // mark at 'e'
    // insert "XY" at offset 1 → mark should shift 4 → 6
    let cs = ChangeSet::insert(1, "XY", e.active().document.buffer.len());
    e.apply(Transaction::new(cs), Edit { range: 1..1, new_len: 2 }, EditKind::Type, &clk);
    assert_eq!(e.active().marks.get(&'a'), Some(&6));
}

#[test]
fn apply_clears_sel_history() {
    use wordcartel_core::change::ChangeSet;
    use wordcartel_core::history::Transaction;
    use wordcartel_core::selection::Selection;
    let clk = TestClock(0);
    let mut e = Editor::new_from_text("abcdef", None, (80, 24));
    e.active_mut().sel_history.push(Selection::single(0));
    let cs = ChangeSet::insert(1, "X", e.active().document.buffer.len());
    e.apply(Transaction::new(cs), Edit { range: 1..1, new_len: 1 }, EditKind::Type, &clk);
    assert!(e.active().sel_history.is_empty(), "edit must reset the expand ladder");
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib editor::tests::marks_follow editor::tests::apply_clears` → FAIL.

- [ ] **Step 3: Implement.**
  - Add to the `Buffer` struct (editor.rs ~line 60), and initialize them in **both** buffer-construction sites (`new_from_text` ~line 156 and any `alloc`/`open` path):

```rust
    pub marks: std::collections::BTreeMap<char, usize>,
    pub jump_ring: Vec<usize>,
    pub ring_cursor: usize,
    pub sel_history: Vec<wordcartel_core::selection::Selection>,
```

  init: `marks: Default::default(), jump_ring: Vec::new(), ring_cursor: 0, sel_history: Vec::new(),`.

  - Add the enum + `Editor` field:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkPending { Set, Jump }
```

   On `Editor` (line ~110, beside `prompt`): `pub pending_mark: Option<MarkPending>,` and init `pending_mark: None,` in `new_from_text`.

  - Rewrite `Buffer::apply` (line 77) to capture the ChangeSet, then map marks/ring and clear `sel_history`:

```rust
    pub fn apply(&mut self, txn: Transaction, edit: wordcartel_core::block_tree::Edit, kind: EditKind, clock: &dyn Clock) {
        let cs = txn.changes.clone();                    // capture BEFORE commit consumes txn
        let old_rope = self.document.buffer.snapshot();
        let before = self.document.selection.clone();
        self.document.selection = self.document.history.commit_coalescing(txn, &mut self.document.buffer, before, clock, kind);
        self.document.version += 1;
        self.pre_edit_rope = Some(old_rope);
        self.last_edit = Some(edit);
        // 5c: marks & ring follow the text; the expand ladder resets on any edit.
        for v in self.marks.values_mut() {
            *v = wordcartel_core::change::map_pos(*v, &cs);
        }
        for v in self.jump_ring.iter_mut() {
            *v = wordcartel_core::change::map_pos(*v, &cs);
        }
        self.sel_history.clear();
        crate::recovery::record_snapshot(self.document.path.as_deref(), self.document.buffer.snapshot());
    }
```

- [ ] **Step 4: Run tests + build.** `cargo test -p wordcartel --lib editor::` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings. (`MarkPending`/`pending_mark`/`marks`/`jump_ring`/`ring_cursor`/`sel_history` are consumed by Tasks 5–10; if any is briefly unused, add a scoped `#[allow(dead_code)] // wired in Task N` and remove it in that task.)

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/editor.rs
git commit -m "feat(editor): per-Buffer marks/ring/sel_history; map them in Buffer::apply; Editor.pending_mark"
```

---

## Task 5: Word navigation + word-delete commands

**Files:**
- Modify: `wordcartel/src/nav.rs` (`move_word_left`/`move_word_right`); `wordcartel/src/commands.rs` (`Dir::WordLeft/WordRight`; `Command::DeleteWord{back}`; `Move`-arm sel_history reset); `wordcartel/src/registry.rs`; `wordcartel/src/keymap.rs`
- Test: `wordcartel/src/commands.rs`

**Interfaces:**
- Consumes: `textobj::{next_word_start, prev_word_start}` (Task 1); `Buffer.sel_history` (Task 4); existing `nav::head`, `derive::{line_start, line_text, total_logical_lines}`.
- Produces: `nav::move_word_left/right(editor) -> usize`; commands `move_word_left/right`, `select_word_left/right`, `delete_word_back/forward`.

- [ ] **Step 1: Write the failing tests** in `commands.rs` tests:

```rust
#[test]
fn move_word_right_crosses_into_next_word_and_block() {
    let mut e = Editor::new_from_text("alpha beta\n\ngamma\n", None, (80, 24));
    set_caret(&mut e, 0); derive::rebuild(&mut e);
    run(Command::Move { dir: Dir::WordRight, extend: false }, &mut e, &TestClock(0));
    assert_eq!(nav::head(&e), 6); // start of "beta"
    run(Command::Move { dir: Dir::WordRight, extend: false }, &mut e, &TestClock(0));
    assert_eq!(nav::head(&e), 12); // start of "gamma" (across the blank-line gap)
}

#[test]
fn select_word_left_extends_selection() {
    let mut e = Editor::new_from_text("alpha beta", None, (80, 24));
    set_caret(&mut e, 10); derive::rebuild(&mut e); // end of "beta"
    run(Command::Move { dir: Dir::WordLeft, extend: true }, &mut e, &TestClock(0));
    let r = e.active().document.selection.primary();
    assert_eq!((r.from(), r.to()), (6, 10)); // "beta" selected
}

#[test]
fn delete_word_back_is_one_undo_step() {
    let mut e = Editor::new_from_text("alpha beta", None, (80, 24));
    set_caret(&mut e, 10); derive::rebuild(&mut e);
    run(Command::DeleteWord { back: true }, &mut e, &TestClock(0));
    assert_eq!(e.active().document.buffer.to_string(), "alpha ");
    e.undo();
    assert_eq!(e.active().document.buffer.to_string(), "alpha beta");
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib commands::tests::move_word commands::tests::select_word commands::tests::delete_word` → FAIL.

- [ ] **Step 3: Implement.**
  - `nav.rs` — word motions with cross-block stitching (window = caret's leaf block via `paragraph_range_at`):

```rust
/// Move to the start of the next word, crossing block boundaries.
pub fn move_word_right(editor: &mut Editor) -> usize {
    let buf = &editor.active().document.buffer;
    let blocks = &editor.active().document.blocks;
    let h = head(editor);
    let (wstart, wend) = paragraph_range_at(blocks, buf, h);
    let window = buf.slice(wstart..wend);
    let rel = h.saturating_sub(wstart);
    let new = match wordcartel_core::textobj::next_word_start(&window, rel) {
        Some(r) => wstart + r,
        None => {
            // No further word in this block → first word of the next block, else doc end.
            let next_para = paragraph_range_at(blocks, buf, wend.min(buf.len().saturating_sub(0)));
            let ntext = buf.slice(next_para.0..next_para.1);
            match wordcartel_core::textobj::next_word_start(&ntext, 0)
                .or_else(|| (!ntext.is_empty()).then_some(0)) {
                Some(r) if next_para.0 + r > h => next_para.0 + r,
                _ => buf.len(),
            }
        }
    };
    editor.active_mut().desired_col = None;
    new
}

/// Move to the start of the previous word, crossing block boundaries.
pub fn move_word_left(editor: &mut Editor) -> usize {
    let buf = &editor.active().document.buffer;
    let blocks = &editor.active().document.blocks;
    let h = head(editor);
    let (wstart, wend) = paragraph_range_at(blocks, buf, h);
    let window = buf.slice(wstart..wend);
    let rel = h.saturating_sub(wstart);
    let new = match wordcartel_core::textobj::prev_word_start(&window, rel) {
        Some(r) => wstart + r,
        None if wstart > 0 => {
            let prev_para = paragraph_range_at(blocks, buf, wstart - 1);
            let ptext = buf.slice(prev_para.0..prev_para.1);
            let rel_end = ptext.len();
            wordcartel_core::textobj::prev_word_start(&ptext, rel_end)
                .map(|r| prev_para.0 + r)
                .unwrap_or(prev_para.0)
        }
        None => 0,
    };
    editor.active_mut().desired_col = None;
    new
}
```

  - `commands.rs` — extend `Dir` with `WordLeft, WordRight` and add the two arms in the `Move` match (line 257):

```rust
                Dir::WordLeft  => nav::move_word_left(editor),
                Dir::WordRight => nav::move_word_right(editor),
```

   and add `editor.active_mut().sel_history.clear();` as the **first line** of the `Command::Move` arm (line 255), so every motion resets the expand ladder (Task 7).

  - `commands.rs` — add `Command::DeleteWord { back: bool }` and its arm (model on the existing `Backspace`/`DeleteForward` arms at lines 188–208 — compute the range via the nav word fns, go through `editor.apply`):

```rust
        Command::DeleteWord { back } => {
            let h = nav::head(editor);
            let target = if back { nav::move_word_left(editor) } else { nav::move_word_right(editor) };
            let (from, to) = if back { (target, h) } else { (h, target) };
            if from == to { return CommandResult::Noop; }
            let doc_len = editor.active().document.buffer.len();
            let cs = ChangeSet::delete(from..to, doc_len);
            let edit = Edit { range: from..to, new_len: 0 };
            let txn = Transaction::new(cs).with_selection(Selection::single(from));
            editor.apply(txn, edit, EditKind::Type, clock);
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }
```

  - `registry.rs` — register (after the existing motion block ~line 86):

```rust
        r.register("move_word_left",  "Move Word Left",  None, |c| run(c, Command::Move { dir: Dir::WordLeft,  extend: false }));
        r.register("move_word_right", "Move Word Right", None, |c| run(c, Command::Move { dir: Dir::WordRight, extend: false }));
        r.register("select_word_left",  "Select Word Left",  None, |c| run(c, Command::Move { dir: Dir::WordLeft,  extend: true }));
        r.register("select_word_right", "Select Word Right", None, |c| run(c, Command::Move { dir: Dir::WordRight, extend: true }));
        r.register("delete_word_back",    "Delete Word Left",  Some(MenuCategory::Edit), |c| run(c, Command::DeleteWord { back: true }));
        r.register("delete_word_forward", "Delete Word Right", Some(MenuCategory::Edit), |c| run(c, Command::DeleteWord { back: false }));
```

  - `keymap.rs` — add to the CUA table (verified free): `("ctrl-left","move_word_left")`, `("ctrl-right","move_word_right")`, `("ctrl-shift-left","select_word_left")`, `("ctrl-shift-right","select_word_right")`, `("ctrl-backspace","delete_word_back")`, `("ctrl-del","delete_word_forward")`.

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib commands::` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/nav.rs wordcartel/src/commands.rs wordcartel/src/registry.rs wordcartel/src/keymap.rs
git commit -m "feat(nav): word motion + select-by-word + word-delete; reset expand ladder on Move"
```

---

## Task 6: Paragraph, page & document navigation commands

**Files:**
- Modify: `wordcartel/src/nav.rs` (`move_paragraph_up/down`, `move_page_up/down`, `move_doc_start/end`); `wordcartel/src/commands.rs` (`Dir` variants + arms); `wordcartel/src/registry.rs`; `wordcartel/src/keymap.rs`
- Test: `wordcartel/src/nav.rs`, `wordcartel/src/commands.rs`

**Interfaces:**
- Consumes: `paragraph_range_at` (Task 2); existing `caret_line`, `ensure_visible`, layout/area helpers; `Buffer.view.area`.
- Produces: `nav::move_paragraph_up/down`, `move_page_up/down`, `move_doc_start/end` (each `-> usize`); commands `move_paragraph_up/down`, `move_page_up/down`, `move_doc_start/end`.

- [ ] **Step 1: Write the failing tests** in `commands.rs` tests:

```rust
#[test]
fn paragraph_down_jumps_to_next_block_start() {
    let mut e = Editor::new_from_text("Para one.\n\nPara two.\n\nThree.\n", None, (80, 24));
    set_caret(&mut e, 0); derive::rebuild(&mut e);
    run(Command::Move { dir: Dir::ParagraphDown, extend: false }, &mut e, &TestClock(0));
    let h = nav::head(&e);
    assert_eq!(e.active().document.buffer.slice(h..h+8), "Para two");
}

#[test]
fn doc_start_and_end() {
    let mut e = Editor::new_from_text("aaa\nbbb\nccc\n", None, (80, 24));
    set_caret(&mut e, 5); derive::rebuild(&mut e);
    run(Command::Move { dir: Dir::DocEnd, extend: false }, &mut e, &TestClock(0));
    assert_eq!(nav::head(&e), e.active().document.buffer.len());
    run(Command::Move { dir: Dir::DocStart, extend: false }, &mut e, &TestClock(0));
    assert_eq!(nav::head(&e), 0);
}

#[test]
fn page_down_moves_down_about_a_page() {
    let text: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let mut e = Editor::new_from_text(&text, None, (80, 10)); // ~9 content rows
    set_caret(&mut e, 0); derive::rebuild(&mut e);
    run(Command::Move { dir: Dir::PageDown, extend: false }, &mut e, &TestClock(0));
    assert!(nav::caret_line(&e) >= 7 && nav::caret_line(&e) <= 9,
        "page-down should advance ~one viewport, got line {}", nav::caret_line(&e));
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib commands::tests::paragraph_down commands::tests::doc_start commands::tests::page_down` → FAIL.

- [ ] **Step 3: Implement.**
  - `nav.rs`:

```rust
pub fn move_paragraph_down(editor: &mut Editor) -> usize {
    let buf = &editor.active().document.buffer;
    let blocks = &editor.active().document.blocks;
    let h = head(editor);
    let (_from, to) = paragraph_range_at(blocks, buf, h);
    let next = paragraph_range_at(blocks, buf, to.min(buf.len()));
    editor.active_mut().desired_col = None;
    if next.0 > h { next.0 } else { buf.len() }
}

pub fn move_paragraph_up(editor: &mut Editor) -> usize {
    let buf = &editor.active().document.buffer;
    let blocks = &editor.active().document.blocks;
    let h = head(editor);
    let (from, _to) = paragraph_range_at(blocks, buf, h);
    editor.active_mut().desired_col = None;
    if from < h { from } else if from == 0 { 0 } else { paragraph_range_at(blocks, buf, from - 1).0 }
}

pub fn move_doc_start(editor: &mut Editor) -> usize { editor.active_mut().desired_col = None; 0 }
pub fn move_doc_end(editor: &mut Editor) -> usize {
    let len = editor.active().document.buffer.len();
    editor.active_mut().desired_col = None; len
}

/// Move the caret down by (editing-area height − 1) visual rows, preserving
/// desired_col (like move_down). Implemented by repeated move_down so wrapped
/// lines are handled by the existing layout-aware logic.
pub fn move_page_down(editor: &mut Editor) -> usize {
    let h = (editor.active().view.area.1 as usize).saturating_sub(1).max(1) - 1;
    let mut off = head(editor);
    for _ in 0..h.max(1) {
        let next = move_down(editor); // preserves desired_col across the run
        if next == off { break; }
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(next);
        off = next;
    }
    off
}
pub fn move_page_up(editor: &mut Editor) -> usize {
    let h = (editor.active().view.area.1 as usize).saturating_sub(1).max(1) - 1;
    let mut off = head(editor);
    for _ in 0..h.max(1) {
        let next = move_up(editor);
        if next == off { break; }
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(next);
        off = next;
    }
    off
}
```

  (Note: `move_page_*` step the real selection internally because `move_up/down` read the live caret; the `Move` arm then sets the final selection from the returned offset. This preserves `desired_col` across the page jump, matching arrow behavior.)

  - `commands.rs` — extend `Dir` with `ParagraphUp, ParagraphDown, PageUp, PageDown, DocStart, DocEnd` and add the match arms:

```rust
                Dir::ParagraphUp   => nav::move_paragraph_up(editor),
                Dir::ParagraphDown => nav::move_paragraph_down(editor),
                Dir::PageUp        => nav::move_page_up(editor),
                Dir::PageDown      => nav::move_page_down(editor),
                Dir::DocStart      => nav::move_doc_start(editor),
                Dir::DocEnd        => nav::move_doc_end(editor),
```

  - `registry.rs`:

```rust
        r.register("move_paragraph_up",   "Move Paragraph Up",   None, |c| run(c, Command::Move { dir: Dir::ParagraphUp,   extend: false }));
        r.register("move_paragraph_down", "Move Paragraph Down", None, |c| run(c, Command::Move { dir: Dir::ParagraphDown, extend: false }));
        r.register("move_page_up",   "Move Page Up",   None, |c| run(c, Command::Move { dir: Dir::PageUp,   extend: false }));
        r.register("move_page_down", "Move Page Down", None, |c| run(c, Command::Move { dir: Dir::PageDown, extend: false }));
        r.register("move_doc_start", "Move to Start",  None, |c| run(c, Command::Move { dir: Dir::DocStart, extend: false }));
        r.register("move_doc_end",   "Move to End",    None, |c| run(c, Command::Move { dir: Dir::DocEnd,   extend: false }));
```

  - `keymap.rs` — CUA (verified free): `("ctrl-up","move_paragraph_up")`, `("ctrl-down","move_paragraph_down")`, `("pageup","move_page_up")`, `("pagedown","move_page_down")`, `("ctrl-home","move_doc_start")`, `("ctrl-end","move_doc_end")`.
  - If Task 2 added a temporary `#[allow(dead_code)]` on `paragraph_range_at`, remove it now (it is live here).

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/nav.rs wordcartel/src/commands.rs wordcartel/src/registry.rs wordcartel/src/keymap.rs
git commit -m "feat(nav): paragraph/page/document motion commands + bindings"
```

---

## Task 7: Text objects — select word/sentence/paragraph + expand/shrink

**Files:**
- Modify: `wordcartel/src/commands.rs` (new `Command` variants + arms); `wordcartel/src/registry.rs`; `wordcartel/src/keymap.rs`
- Test: `wordcartel/src/commands.rs`

**Interfaces:**
- Consumes: `textobj::{word_bounds, sentence_bounds}` (Task 1); `nav::paragraph_range_at` (Task 2); `Buffer.sel_history` (Task 4).
- Produces: commands `select_word`, `select_sentence`, `select_paragraph`, `expand_selection`, `shrink_selection`; helper `commands::scope_range(editor, scope) -> (usize, usize)`.

- [ ] **Step 1: Write the failing tests** in `commands.rs` tests:

```rust
#[test]
fn select_paragraph_selects_block() {
    let mut e = Editor::new_from_text("One two.\n\nThree four.\n", None, (80, 24));
    set_caret(&mut e, 12); derive::rebuild(&mut e); // inside "Three four."
    run(Command::SelectScope(Scope::Paragraph), &mut e, &TestClock(0));
    let r = e.active().document.selection.primary();
    assert_eq!(e.active().document.buffer.slice(r.from()..r.to()).trim(), "Three four.");
}

#[test]
fn expand_then_shrink_round_trips() {
    let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
    set_caret(&mut e, 1); derive::rebuild(&mut e); // inside "One"
    run(Command::ExpandSelection, &mut e, &TestClock(0)); // → word "One"
    let w = e.active().document.selection.primary();
    assert_eq!(e.active().document.buffer.slice(w.from()..w.to()), "One");
    run(Command::ExpandSelection, &mut e, &TestClock(0)); // → sentence
    let s = e.active().document.selection.primary();
    assert!(e.active().document.buffer.slice(s.from()..s.to()).starts_with("One two."));
    run(Command::ShrinkSelection, &mut e, &TestClock(0)); // back to word
    let w2 = e.active().document.selection.primary();
    assert_eq!((w2.from(), w2.to()), (w.from(), w.to()));
}

#[test]
fn a_motion_resets_the_expand_ladder() {
    let mut e = Editor::new_from_text("One two.\n", None, (80, 24));
    set_caret(&mut e, 1); derive::rebuild(&mut e);
    run(Command::ExpandSelection, &mut e, &TestClock(0));
    run(Command::Move { dir: Dir::Right, extend: false }, &mut e, &TestClock(0)); // resets
    assert!(e.active().sel_history.is_empty());
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib commands::tests::select_paragraph commands::tests::expand_then commands::tests::a_motion_resets` → FAIL.

- [ ] **Step 3: Implement.**
  - `commands.rs` — add the scope enum + variants:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope { Word, Sentence, Paragraph, Document }
// in `enum Command`:
    SelectScope(Scope),
    ExpandSelection,
    ShrinkSelection,
```

   helper + arms (the `scope_range` computes a scope's range at the caret; expand walks to the next-larger scope strictly containing the current selection):

```rust
fn scope_range(editor: &Editor, scope: Scope) -> (usize, usize) {
    let buf = &editor.active().document.buffer;
    let blocks = &editor.active().document.blocks;
    let h = nav::head(editor);
    match scope {
        Scope::Word => {
            let (ps, pe) = nav::paragraph_range_at(blocks, buf, h);
            let win = buf.slice(ps..pe);
            let (wf, wt) = wordcartel_core::textobj::word_bounds(&win, h - ps);
            if wf == wt {
                // in whitespace → nearest word (next within block, else prev)
                match wordcartel_core::textobj::next_word_start(&win, h - ps)
                    .or_else(|| wordcartel_core::textobj::prev_word_start(&win, h - ps)) {
                    Some(r) => { let (a, b) = wordcartel_core::textobj::word_bounds(&win, r); (ps + a, ps + b) }
                    None => (h, h),
                }
            } else { (ps + wf, ps + wt) }
        }
        Scope::Sentence => {
            let (ps, pe) = nav::paragraph_range_at(blocks, buf, h);
            let win = buf.slice(ps..pe);
            let (sf, st) = wordcartel_core::textobj::sentence_bounds(&win, h - ps);
            (ps + sf, ps + st)
        }
        Scope::Paragraph => nav::paragraph_range_at(blocks, buf, h),
        Scope::Document => (0, buf.len()),
    }
}

fn set_selection_range(editor: &mut Editor, from: usize, to: usize) {
    editor.active_mut().document.selection = Selection::range(from, to);
    derive::rebuild(editor);
    nav::ensure_visible(editor);
}
```

```rust
        Command::SelectScope(scope) => {
            editor.active_mut().sel_history.clear();
            let (from, to) = scope_range(editor, scope);
            set_selection_range(editor, from, to);
            CommandResult::Handled
        }
        Command::ExpandSelection => {
            let cur = editor.active().document.selection.primary();
            let (cf, ct) = (cur.from(), cur.to());
            // smallest scope strictly larger than the current selection
            let order = [Scope::Word, Scope::Sentence, Scope::Paragraph, Scope::Document];
            let mut next: Option<(usize, usize)> = None;
            for sc in order {
                let (f, t) = scope_range(editor, sc);
                if f <= cf && t >= ct && (f < cf || t > ct) { next = Some((f, t)); break; }
            }
            if let Some((f, t)) = next {
                editor.active_mut().sel_history.push(editor.active().document.selection.clone());
                set_selection_range(editor, f, t);
                CommandResult::Handled
            } else { CommandResult::Noop }
        }
        Command::ShrinkSelection => {
            if let Some(prev) = editor.active_mut().sel_history.pop() {
                editor.active_mut().document.selection = prev;
                derive::rebuild(editor);
                nav::ensure_visible(editor);
                CommandResult::Handled
            } else { CommandResult::Noop }
        }
```

  - `registry.rs`:

```rust
        r.register("select_word",      "Select Word",      None, |c| run(c, Command::SelectScope(Scope::Word)));
        r.register("select_sentence",  "Select Sentence",  None, |c| run(c, Command::SelectScope(Scope::Sentence)));
        r.register("select_paragraph", "Select Paragraph", None, |c| run(c, Command::SelectScope(Scope::Paragraph)));
        r.register("expand_selection", "Expand Selection", None, |c| run(c, Command::ExpandSelection));
        r.register("shrink_selection", "Shrink Selection", None, |c| run(c, Command::ShrinkSelection));
```

  - `keymap.rs` — CUA (verified free): `("ctrl-w","expand_selection")`, `("ctrl-shift-w","shrink_selection")`. (`select_word/sentence/paragraph` are palette-only by default.)

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib commands::` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/commands.rs wordcartel/src/registry.rs wordcartel/src/keymap.rs
git commit -m "feat(commands): expand/shrink + select word/sentence/paragraph text objects"
```

---

## Task 8: Named marks — set/jump commands + pending-char capture

**Files:**
- Create: `wordcartel/src/marks.rs`; Modify: `wordcartel/src/lib.rs` (`pub mod marks;`); `wordcartel/src/app.rs` (interception block + menu clear); `wordcartel/src/registry.rs`; `wordcartel/src/keymap.rs`
- Test: `wordcartel/src/marks.rs`, `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `Editor.pending_mark`, `MarkPending::{Set, Jump}`, `Buffer.marks` (Task 4); `Buffer.jump_ring`/`ring_cursor` via `marks::record_jump` (defined here, used by Task 9 too); existing reduce structure + overlay openers.
- Produces: `marks::set_mark(editor)` (arms capture), `marks::jump_to_mark(editor)` (arms capture), `marks::resolve_pending(editor, ch)` (applies the captured char), `marks::record_jump(buf, pre: usize)`.

- [ ] **Step 1: Write the failing tests** in `marks.rs` + `app.rs`:

`marks.rs`:
```rust
#[cfg(test)]
mod tests {
    use crate::editor::{Editor, MarkPending};
    #[test]
    fn set_then_jump_mark_round_trips() {
        let mut e = Editor::new_from_text("line0\nline1\nline2\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(6); // line1
        super::set_mark(&mut e);
        assert_eq!(e.pending_mark, Some(MarkPending::Set));
        super::resolve_pending(&mut e, 'a');
        assert_eq!(e.active().marks.get(&'a'), Some(&6));
        assert_eq!(e.pending_mark, None);
        // move away, then jump back to 'a'
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        super::jump_to_mark(&mut e);
        super::resolve_pending(&mut e, 'a');
        assert_eq!(e.active().document.selection.primary().head, 6);
    }
}
```

`app.rs`:
```rust
#[test]
fn pending_mark_consumes_one_key_then_clears() {
    use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("abc\n", None, (80, 24));
    e.pending_mark = Some(crate::editor::MarkPending::Set);
    let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
    let (tx, _rx) = std::sync::mpsc::channel();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    let press = |c, m| Event::Key(KeyEvent { code: c, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    crate::app::reduce(Msg::Input(press(KeyCode::Char('q'), KeyModifiers::NONE)), &mut e, &reg, &km, &ex, &clk, &tx);
    assert_eq!(e.pending_mark, None, "capture consumed the key");
    assert_eq!(e.active().marks.get(&'q'), Some(&0));
    assert_eq!(e.active().document.buffer.to_string(), "abc\n", "captured key did NOT type into the doc");
}

#[test]
fn esc_cancels_pending_mark() {
    use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("abc\n", None, (80, 24));
    e.pending_mark = Some(crate::editor::MarkPending::Set);
    let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
    let (tx, _rx) = std::sync::mpsc::channel();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    let esc = Event::Key(KeyEvent { code: KeyCode::Esc, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    crate::app::reduce(Msg::Input(esc), &mut e, &reg, &km, &ex, &clk, &tx);
    assert_eq!(e.pending_mark, None);
    assert!(e.active().marks.is_empty());
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib marks:: app::tests::pending_mark app::tests::esc_cancels` → FAIL.

- [ ] **Step 3: Implement.**
  - `marks.rs`:

```rust
//! Named marks + jump-ring command bodies (5c).
use crate::editor::{Buffer, Editor, MarkPending};
use crate::nav;

pub fn set_mark(editor: &mut Editor)  { editor.active_mut().sel_history.clear(); editor.pending_mark = Some(MarkPending::Set); editor.status = "set mark:".into(); }
pub fn jump_to_mark(editor: &mut Editor) { editor.pending_mark = Some(MarkPending::Jump); editor.status = "jump to mark:".into(); }

/// Push `pre` onto the ring as a deliberate jump origin (Task 9 fills in the
/// back/forward navigation; this is the shared push).
pub fn record_jump(buf: &mut Buffer, pre: usize) {
    const CAP: usize = 64;
    if buf.ring_cursor < buf.jump_ring.len() {
        buf.jump_ring.truncate(buf.ring_cursor); // drop stale forward tail
    }
    if buf.jump_ring.last() != Some(&pre) {
        buf.jump_ring.push(pre);
        if buf.jump_ring.len() > CAP { buf.jump_ring.remove(0); }
    }
    buf.ring_cursor = buf.jump_ring.len();
}

/// Apply the captured mark char for the pending operation.
pub fn resolve_pending(editor: &mut Editor, ch: char) {
    match editor.pending_mark.take() {
        Some(MarkPending::Set) => {
            let at = nav::head(editor);
            editor.active_mut().marks.insert(ch, at);
            editor.status = format!("mark {ch} set");
        }
        Some(MarkPending::Jump) => {
            editor.active_mut().sel_history.clear();
            if let Some(&raw) = editor.active().marks.get(&ch) {
                let pre = nav::head(editor);
                record_jump(editor.active_mut(), pre);
                let off = nav::clamp_snap(editor, raw); // Task 11 helper; see note
                editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
                crate::derive::rebuild(editor);
                nav::ensure_visible(editor);
                editor.status = format!("jumped to mark {ch}");
            } else {
                editor.status = format!("no mark {ch}");
            }
        }
        None => {}
    }
}
```

   **Note on `nav::clamp_snap`:** add a small public helper in `nav.rs` now (it is also used by Tasks 9–10): `pub fn clamp_snap(editor: &Editor, off: usize) -> usize` that clamps to `0..=len`, finds the caret line, gets/derives its `ColMap`, and returns `line_start + map.snap_to_stop(off - line_start)`. Model it on the snapping already done in `screen_pos` (nav.rs:88).

  - `app.rs` — add the **interception block at the very top of `reduce`** (above the menu/palette blocks), key-only, non-key falls through:

```rust
    if editor.pending_mark.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                match k.code {
                    KeyCode::Esc => { editor.pending_mark = None; editor.status.clear(); }
                    KeyCode::Char(c) => crate::marks::resolve_pending(editor, c),
                    _ => { editor.pending_mark = None; } // non-name key cancels
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        // non-key message: fall through to normal handling
    }
```

   and in the `menu` command body (registry.rs, where it already clears prompt/minibuffer/palette/pending_keys) add `c.editor.pending_mark = None;`. Also add `self.pending_mark = None;` is NOT needed in `open_prompt`/`open_minibuffer`/`open_palette` directly — instead add it there too for symmetry (one line each) so every modal opener clears it.

  - `registry.rs`:

```rust
        r.register("set_mark",     "Set Mark…",     None, |c| { crate::marks::set_mark(c.editor); CommandResult::Handled });
        r.register("jump_to_mark", "Jump to Mark…", None, |c| { crate::marks::jump_to_mark(c.editor); CommandResult::Handled });
```

  - `keymap.rs` — CUA: `("ctrl-k m","set_mark")`, `("ctrl-k j","jump_to_mark")` (free in CUA; in WORDSTAR pick trailing keys that don't shadow the existing `ctrl-k ctrl-*` family — e.g. `("ctrl-k m","set_mark")` is fine since WordStar's are `ctrl-k ctrl-s/q/c/v`).

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/marks.rs wordcartel/src/lib.rs wordcartel/src/app.rs wordcartel/src/registry.rs wordcartel/src/keymap.rs
git commit -m "feat(marks): named marks via pending-char capture; set/jump commands; record_jump"
```

---

## Task 9: Jump-back ring — back/forward commands

**Files:**
- Modify: `wordcartel/src/marks.rs` (`jump_back`/`jump_forward`); `wordcartel/src/commands.rs` (`Command::DocStart/DocEnd` push the ring); `wordcartel/src/registry.rs`; `wordcartel/src/keymap.rs`
- Test: `wordcartel/src/marks.rs`

**Interfaces:**
- Consumes: `Buffer.jump_ring`, `ring_cursor` (Task 4); `marks::record_jump`, `nav::clamp_snap` (Task 8).
- Produces: `marks::jump_back(editor)`, `marks::jump_forward(editor)`; commands `jump_back`, `jump_forward`. `move_doc_start/end` now call `record_jump`.

- [ ] **Step 1: Write the failing test** in `marks.rs`:

```rust
#[test]
fn jump_back_and_forward_walk_the_ring() {
    let mut e = Editor::new_from_text("0123456789\n", None, (80, 24));
    // simulate two deliberate jumps from 0 → 5 → 9
    e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
    super::record_jump(e.active_mut(), 0);
    e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5);
    super::record_jump(e.active_mut(), 5);
    e.active_mut().document.selection = wordcartel_core::selection::Selection::single(9);
    // back → 5, back → 0
    super::jump_back(&mut e);
    assert_eq!(e.active().document.selection.primary().head, 5);
    super::jump_back(&mut e);
    assert_eq!(e.active().document.selection.primary().head, 0);
    // forward → 5
    super::jump_forward(&mut e);
    assert_eq!(e.active().document.selection.primary().head, 5);
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib marks::tests::jump_back_and_forward` → FAIL.

- [ ] **Step 3: Implement** in `marks.rs`:

```rust
pub fn jump_back(editor: &mut Editor) {
    editor.active_mut().sel_history.clear();
    let here = nav::head(editor);
    let buf = editor.active_mut();
    if buf.ring_cursor == buf.jump_ring.len() {
        // parked at the live caret — record it as the forward anchor
        if buf.jump_ring.last() != Some(&here) { buf.jump_ring.push(here); }
    }
    if buf.ring_cursor == 0 { editor.status = "ring: at oldest".into(); return; }
    let buf = editor.active_mut();
    buf.ring_cursor -= 1;
    let raw = buf.jump_ring[buf.ring_cursor];
    let off = nav::clamp_snap(editor, raw);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
    crate::derive::rebuild(editor);
    nav::ensure_visible(editor);
}

pub fn jump_forward(editor: &mut Editor) {
    editor.active_mut().sel_history.clear();
    let buf = editor.active_mut();
    if buf.ring_cursor + 1 >= buf.jump_ring.len() { editor.status = "ring: at newest".into(); return; }
    buf.ring_cursor += 1;
    let raw = buf.jump_ring[buf.ring_cursor];
    let off = nav::clamp_snap(editor, raw);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
    crate::derive::rebuild(editor);
    nav::ensure_visible(editor);
}
```

  - `commands.rs` — in the `Move` arm, BEFORE computing `new_head` for `DocStart`/`DocEnd`, push the ring (these are the deliberate jumps): special-case at the top of the arm —

```rust
            if matches!(dir, Dir::DocStart | Dir::DocEnd) {
                let pre = nav::head(editor);
                crate::marks::record_jump(editor.active_mut(), pre);
            }
```

  - `registry.rs`:

```rust
        r.register("jump_back",    "Jump Back",    None, |c| { crate::marks::jump_back(c.editor); CommandResult::Handled });
        r.register("jump_forward", "Jump Forward", None, |c| { crate::marks::jump_forward(c.editor); CommandResult::Handled });
```

  - `keymap.rs` — CUA: `("alt-left","jump_back")`, `("alt-right","jump_forward")`.

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib marks::` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/marks.rs wordcartel/src/commands.rs wordcartel/src/registry.rs wordcartel/src/keymap.rs
git commit -m "feat(marks): jump-back ring (alt-left/right); doc-extent jumps push the ring"
```

---

## Task 10: Marks persistence — load + save (staleness-gated)

**Files:**
- Modify: `wordcartel/src/app.rs` (`apply_resume` loads marks; the persist path writes marks); `wordcartel/src/state.rs` (helpers if needed)
- Test: `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `Buffer.marks` (Task 4); `state.rs` `StateEntry.marks: BTreeMap<String, usize>` + the existing mtime/size staleness guard; `nav::clamp_snap` (Task 8).
- Produces: marks survive a save→reopen round-trip when the staleness guard accepts the entry.

- [ ] **Step 1: Write the failing test** in `app.rs`:

```rust
#[test]
fn marks_round_trip_through_state_entry() {
    // Build a StateEntry with a mark, run apply_resume, assert Buffer.marks populated.
    use std::collections::BTreeMap;
    let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
    let mut marks = BTreeMap::new();
    marks.insert("a".to_string(), 6usize);
    let entry = crate::state::StateEntry {
        cursor: 0, scroll: 0, marks, mtime: 0, size: 0, seq: 1,
    };
    // staleness guard accepts when mtime/size match (here both 0 vs a fresh buffer's 0/len);
    // call the resume applier with a guard-pass and assert marks loaded + clamped.
    crate::app::apply_resume_for_test(&mut e, &entry, /*guard_ok=*/true);
    assert_eq!(e.active().marks.get(&'a'), Some(&6));
}
```

(If `apply_resume` is not directly callable, add a thin `#[cfg(test)] pub fn apply_resume_for_test(...)` shim mirroring the production path, as 5b did for menu routing.)

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib app::tests::marks_round_trip` → FAIL.

- [ ] **Step 3: Implement.**
  - In the production resume path (`apply_resume`, app.rs ~line 237) where it currently applies only cursor/scroll under the staleness guard, also populate marks (string→char, clamped+snapped):

```rust
        for (k, &raw) in &entry.marks {
            if let Some(ch) = k.chars().next() {
                let off = nav::clamp_snap(editor, raw);
                editor.active_mut().marks.insert(ch, off);
            }
        }
```

  - In the persist path (where `StateEntry { … marks: BTreeMap::new() … }` is built, app.rs ~line 1058), replace the empty map with the live marks:

```rust
        marks: editor.active().marks.iter().map(|(c, &o)| (c.to_string(), o)).collect(),
```

  - Keep the existing mtime/size staleness gate unchanged — marks load only when cursor/scroll do.

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib app::` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/app.rs wordcartel/src/state.rs
git commit -m "feat(marks): persist + restore named marks via the session store (staleness-gated)"
```

---

## Task 11: `nav::offset_at_cell` — cell→offset reverse map (for 5c-m)

**Files:**
- Modify: `wordcartel/src/nav.rs` (`offset_at_cell`)
- Test: `wordcartel/src/nav.rs`

**Interfaces:**
- Consumes: `ColMap::visual_to_source` (layout.rs:105), `snap_to_stop` (layout.rs:163); the existing visible-line walk helpers (`rows_before_caret`/`rows_of_line` pattern, nav.rs ~line 397).
- Produces: `pub fn offset_at_cell(editor: &Editor, col: u16, row: u16) -> Option<usize>` — inverse of `screen_pos`. Scoped `#[allow(dead_code)] // wired in 5c-m`.

- [ ] **Step 1: Write the failing test** in `nav.rs`:

```rust
#[test]
fn offset_at_cell_inverts_screen_pos() {
    let mut e = Editor::new_from_text("abc\ndef\n", None, (80, 24));
    set_caret(&mut e, 5); // 'e' on line 1, col 1
    derive::rebuild(&mut e);
    let (col, row) = screen_pos(&e).unwrap();
    assert_eq!(super::offset_at_cell(&e, col, row), Some(5));
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib nav::tests::offset_at_cell` → FAIL.

- [ ] **Step 3: Implement** in `nav.rs` (walk visible logical lines from `(scroll, scroll_row)` accumulating visual rows to find the line+visual-row under `row`, then `visual_to_source`):

```rust
/// Inverse of `screen_pos`: the document byte offset under screen cell
/// `(col, row)` in the editing area, or `None` if `row` is past content.
#[allow(dead_code)] // wired in 5c-m (mouse)
pub fn offset_at_cell(editor: &Editor, col: u16, row: u16) -> Option<usize> {
    let target = row as usize;
    let scroll = editor.active().view.scroll;
    let scroll_row = editor.active().view.scroll_row;
    let total = derive::total_logical_lines(&editor.active().document.buffer);
    let mut acc = 0usize; // visible rows consumed
    let mut line = scroll;
    while line < total {
        let rows = rows_of_line(editor, line);
        let first_vrow = if line == scroll { scroll_row } else { 0 };
        for vrow in first_vrow..rows {
            if acc == target {
                let map = get_or_layout(editor, line);
                let in_off = map.visual_to_source(vrow, col as usize);
                let snapped = map.snap_to_stop(in_off);
                return Some(derive::line_start(&editor.active().document.buffer, line) + snapped);
            }
            acc += 1;
        }
        line += 1;
    }
    None
}
```

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib nav::` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/nav.rs
git commit -m "feat(nav): offset_at_cell reverse map (inverse of screen_pos) for the 5c-m mouse follow-up"
```

---

## Self-Review

**Spec coverage:** §2 substrate (T1 word/sentence, T2 paragraph) ✅; §4 textobj API + block-window (T1/T2) ✅; §5 word/para/page/doc nav + word-delete (T5/T6) ✅; §6 expand/shrink + select-scope + ladder reset (T7, reset in T4 apply + T5 Move-arm) ✅; §7.1 named marks + pending-char capture (T8) ✅; §7.2 jump-ring state machine (T9) ✅; §7.3 persistence load+save staleness-gated (T10) ✅; §8 change::map_pos extraction (T3) + cs-capture-before-commit mapping + clamp/snap (T4 + `clamp_snap` in T8) ✅; §9 pending_mark interception + menu clear (T8) ✅; §10 offset_at_cell scoped dead_code (T11) ✅; §11 edge cases covered by the per-task tests ✅; §12 testing (core oracle T1/T3, shell nav/marks/app) ✅; §13 task-ordering prereqs honored (T3 before T4; T4 before T5–T10; T2 before T6/T7) ✅.

**Placeholder scan:** none — every code step is concrete. The `clamp_snap` helper is fully specified (modeled on `screen_pos`); the `apply_resume_for_test` shim follows the 5b precedent.

**Type consistency:** `Scope`/`Dir`/`Command` variants, `MarkPending`, `Buffer.{marks,jump_ring,ring_cursor,sel_history}`, `Editor.pending_mark`, `change::map_pos`, `nav::{paragraph_range_at, clamp_snap, offset_at_cell, move_word_*, move_paragraph_*, move_page_*, move_doc_*}`, and `marks::{set_mark, jump_to_mark, resolve_pending, record_jump, jump_back, jump_forward}` are used identically across the tasks that define and consume them.
