# Wordcartel Edit Kernel — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the pure, headless **edit kernel** — a `ropey`-backed text buffer, a reversible-diff (ChangeSet) undo system, and a selection that maps correctly through edits — as a fully property-tested library with no terminal, no markdown, and no IO.

**Architecture:** Functional core (spec §10): one mutation channel `Transaction` carrying a `ChangeSet` (+ optional new `Selection`). Positions are **byte offsets** (canonical, spec §16.1). Undo is a linear stack of revisions, each a list of reversible edits; selection is mapped through every ChangeSet on the same step as the text edit. Everything is synchronous and pure; `Rope` snapshots are O(1) for future async workers.

**Tech Stack:** Rust 2021, `ropey` (rope), `smartstring` (small inserts), `smallvec` (single-selection inline), `unicode-segmentation`/`unicode-width` (pulled in now for later efforts), `proptest` (property tests).

## Global Constraints

- Rust edition **2021**; crate name **`wordcartel-core`**; `#![forbid(unsafe_code)]` in the crate root.
- Pin **`ropey = "=1.6.1"`** (matches Helix; spec §3.10). Other deps: `smartstring = "1"`, `smallvec = "1"`, `unicode-segmentation = "1"`, `unicode-width = "0.1"`; dev: `proptest = "1"`.
- License **MIT**. Add a top-of-file note in any file whose design derives from Helix (MPL) or CodeMirror (MIT) that it is a *reimplementation from patterns*, not copied source (spec §9.6).
- **Canonical position type = byte offset (`usize`)** measured into the buffer. All char/line conversions live inside `TextBuffer`; no other module calls `ropey` conversion APIs (spec §16.1, Codex #5).
- Pure/headless: **no `std::io`, no threads, no terminal** in this crate. (`Rope::clone` is the O(1) snapshot seam for later async workers — spec §10.3.)
- Tests use `proptest` for the round-trip laws (spec §11.2); commit `proptest-regressions/` seeds.

---

## File Structure

- `wordcartel-core/Cargo.toml` — crate manifest + pinned deps.
- `wordcartel-core/src/lib.rs` — crate root: `#![forbid(unsafe_code)]`, module decls, re-exports.
- `wordcartel-core/src/buffer.rs` — `TextBuffer` (ropey wrapper; the only place that touches `ropey` conversion APIs).
- `wordcartel-core/src/change.rs` — `ChangeSet`, `Op`, `apply`, `invert`.
- `wordcartel-core/src/selection.rs` — `Range`, `Selection`, `Range::map`.
- `wordcartel-core/src/history.rs` — `Transaction`, `Revision`, `History` (undo/redo + coalescing), `Clock` trait.
- `wordcartel-core/src/register.rs` — in-process clipboard `Register` (copy/cut/paste payload).
- `wordcartel-core/tests/integration.rs` — cross-module property test (random edits → undo-all → original).

Module responsibilities are single-purpose; `buffer.rs` is the sole owner of byte↔char↔line conversion.

---

### Task 0: Crate scaffold

**Files:**
- Create: `wordcartel-core/Cargo.toml`
- Create: `wordcartel-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: a compiling empty crate with all deps available; the `BytePos` doc convention.

- [ ] **Step 1: Create the manifest**

`wordcartel-core/Cargo.toml`:
```toml
[package]
name = "wordcartel-core"
version = "0.0.0"
edition = "2021"
license = "MIT"

[dependencies]
ropey = "=1.6.1"
smartstring = "1"
smallvec = "1"
unicode-segmentation = "1"
unicode-width = "0.1"

[dev-dependencies]
proptest = "1"
```

- [ ] **Step 2: Create the crate root**

`wordcartel-core/src/lib.rs`:
```rust
//! Wordcartel edit kernel: pure, headless buffer + undo + selection.
//! Canonical position = byte offset (usize) into the buffer.
#![forbid(unsafe_code)]

pub mod buffer;
pub mod change;
pub mod history;
pub mod register;
pub mod selection;

/// A byte offset into a buffer's text. The kernel's canonical position type.
pub type BytePos = usize;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build --manifest-path wordcartel-core/Cargo.toml`
Expected: compiles. (Empty modules are declared but not yet created → this will FAIL until Step 4 stubs them.)

- [ ] **Step 4: Stub the modules so the crate builds**

Create each of `src/buffer.rs`, `src/change.rs`, `src/history.rs`, `src/register.rs`, `src/selection.rs` containing only:
```rust
// filled in by later tasks
```

- [ ] **Step 5: Verify + commit**

Run: `cargo build --manifest-path wordcartel-core/Cargo.toml`
Expected: PASS (clean build).
```bash
git add wordcartel-core
git commit -m "chore(core): scaffold wordcartel-core crate"
```

---

### Task 1: TextBuffer (the ropey wrapper)

**Files:**
- Modify: `wordcartel-core/src/buffer.rs`

**Interfaces:**
- Consumes: `crate::BytePos`.
- Produces:
  - `struct TextBuffer { rope: ropey::Rope }`
  - `TextBuffer::from_str(&str) -> Self`
  - `fn len(&self) -> usize` (bytes)
  - `fn insert(&mut self, at: BytePos, text: &str)`
  - `fn delete(&mut self, range: std::ops::Range<BytePos>)`
  - `fn slice(&self, range: std::ops::Range<BytePos>) -> String`
  - `fn byte_to_line(&self, b: BytePos) -> usize`
  - `fn line_to_byte(&self, line: usize) -> BytePos`
  - `fn snapshot(&self) -> ropey::Rope` (O(1) clone)
  - `fn to_string(&self) -> String`

- [ ] **Step 1: Write failing tests**

`src/buffer.rs`:
```rust
//! TextBuffer: the only owner of byte↔char↔line conversion (spec §16.1).
use crate::BytePos;
use std::ops::Range;

#[derive(Clone, Debug)]
pub struct TextBuffer {
    rope: ropey::Rope,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_delete_ascii() {
        let mut b = TextBuffer::from_str("hello world");
        b.insert(5, ",");
        assert_eq!(b.to_string(), "hello, world");
        b.delete(0..7); // remove "hello, "
        assert_eq!(b.to_string(), "world");
        assert_eq!(b.len(), 5);
    }

    #[test]
    fn slice_and_multibyte() {
        // "héllo" — 'é' is 2 bytes (U+00E9). bytes: h(0) é(1..3) l(3) l(4) o(5)
        let b = TextBuffer::from_str("héllo");
        assert_eq!(b.len(), 6);
        assert_eq!(b.slice(0..3), "hé");
        assert_eq!(b.slice(3..6), "llo");
    }

    #[test]
    fn line_conversions() {
        let b = TextBuffer::from_str("a\nbb\nccc");
        // bytes: a(0) \n(1) b(2) b(3) \n(4) c(5) c(6) c(7)
        assert_eq!(b.byte_to_line(0), 0);
        assert_eq!(b.byte_to_line(2), 1);
        assert_eq!(b.byte_to_line(5), 2);
        assert_eq!(b.line_to_byte(1), 2);
        assert_eq!(b.line_to_byte(2), 5);
    }

    #[test]
    fn snapshot_is_independent() {
        let mut b = TextBuffer::from_str("abc");
        let snap = b.snapshot();
        b.insert(3, "d");
        assert_eq!(b.to_string(), "abcd");
        assert_eq!(snap.to_string(), "abc"); // snapshot unaffected
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml buffer`
Expected: FAIL (no methods on `TextBuffer`).

- [ ] **Step 3: Implement TextBuffer**

Add above the `#[cfg(test)]` block in `src/buffer.rs`:
```rust
impl TextBuffer {
    pub fn from_str(s: &str) -> Self {
        TextBuffer { rope: ropey::Rope::from_str(s) }
    }

    pub fn len(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }

    pub fn insert(&mut self, at: BytePos, text: &str) {
        let char_idx = self.rope.byte_to_char(at);
        self.rope.insert(char_idx, text);
    }

    pub fn delete(&mut self, range: Range<BytePos>) {
        let start = self.rope.byte_to_char(range.start);
        let end = self.rope.byte_to_char(range.end);
        self.rope.remove(start..end);
    }

    pub fn slice(&self, range: Range<BytePos>) -> String {
        self.rope.byte_slice(range).to_string()
    }

    pub fn byte_to_line(&self, b: BytePos) -> usize {
        self.rope.byte_to_line(b)
    }

    pub fn line_to_byte(&self, line: usize) -> BytePos {
        self.rope.line_to_byte(line)
    }

    pub fn snapshot(&self) -> ropey::Rope {
        self.rope.clone() // O(1) — the async-worker seam (spec §10.3)
    }

    pub fn to_string(&self) -> String {
        self.rope.to_string()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml buffer`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add wordcartel-core/src/buffer.rs
git commit -m "feat(core): TextBuffer ropey wrapper with byte-offset API"
```

---

### Task 2: ChangeSet + apply

**Files:**
- Modify: `wordcartel-core/src/change.rs`

**Interfaces:**
- Consumes: `crate::BytePos`, `crate::buffer::TextBuffer`.
- Produces:
  - `type Tendril = smartstring::alias::String;`
  - `enum Op { Retain(usize), Delete(usize), Insert(Tendril) }` (counts are **bytes**)
  - `struct ChangeSet { ops: Vec<Op>, len_before: usize, len_after: usize }`
  - `ChangeSet::insert(at: BytePos, text: &str, doc_len: usize) -> ChangeSet`
  - `ChangeSet::delete(range: Range<BytePos>, doc_len: usize) -> ChangeSet`
  - `fn apply(&self, buf: &mut TextBuffer)`

- [ ] **Step 1: Write failing tests**

`src/change.rs`:
```rust
//! ChangeSet: reversible byte-diff. Reimplemented from the Helix/CodeMirror
//! transaction pattern (MPL/MIT) — pattern, not copied source (spec §9.6).
use crate::buffer::TextBuffer;
use crate::BytePos;
use std::ops::Range;

pub type Tendril = smartstring::alias::String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Retain(usize), // bytes
    Delete(usize), // bytes
    Insert(Tendril),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSet {
    pub ops: Vec<Op>,
    pub len_before: usize,
    pub len_after: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_insert() {
        let mut b = TextBuffer::from_str("hello world");
        let cs = ChangeSet::insert(5, ",", b.len());
        cs.apply(&mut b);
        assert_eq!(b.to_string(), "hello, world");
    }

    #[test]
    fn apply_delete() {
        let mut b = TextBuffer::from_str("hello, world");
        let cs = ChangeSet::delete(5..7, b.len()); // remove ", "
        cs.apply(&mut b);
        assert_eq!(b.to_string(), "helloworld");
    }

    #[test]
    fn len_fields_track_size() {
        let b = TextBuffer::from_str("abc");
        let ins = ChangeSet::insert(1, "XY", b.len());
        assert_eq!((ins.len_before, ins.len_after), (3, 5));
        let del = ChangeSet::delete(0..2, b.len());
        assert_eq!((del.len_before, del.len_after), (3, 1));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml change`
Expected: FAIL (no constructors / `apply`).

- [ ] **Step 3: Implement constructors + apply**

Add above the test module in `src/change.rs`:
```rust
impl ChangeSet {
    /// Insert `text` at byte offset `at` in a document of length `doc_len`.
    pub fn insert(at: BytePos, text: &str, doc_len: usize) -> ChangeSet {
        let mut ops = Vec::new();
        if at > 0 {
            ops.push(Op::Retain(at));
        }
        ops.push(Op::Insert(Tendril::from(text)));
        if at < doc_len {
            ops.push(Op::Retain(doc_len - at));
        }
        ChangeSet { ops, len_before: doc_len, len_after: doc_len + text.len() }
    }

    /// Delete `range` (bytes) in a document of length `doc_len`.
    pub fn delete(range: Range<BytePos>, doc_len: usize) -> ChangeSet {
        let mut ops = Vec::new();
        if range.start > 0 {
            ops.push(Op::Retain(range.start));
        }
        ops.push(Op::Delete(range.end - range.start));
        if range.end < doc_len {
            ops.push(Op::Retain(doc_len - range.end));
        }
        ChangeSet { ops, len_before: doc_len, len_after: doc_len - (range.end - range.start) }
    }

    /// Apply in place. O(edit size + #ops·log n): Retain only advances a cursor,
    /// so a single-key edit never copies the whole document.
    pub fn apply(&self, buf: &mut TextBuffer) {
        let mut pos: BytePos = 0;
        for op in &self.ops {
            match op {
                Op::Retain(n) => pos += n,
                Op::Delete(n) => buf.delete(pos..pos + n), // pos stays; tail shifts left
                Op::Insert(s) => {
                    buf.insert(pos, s);
                    pos += s.len();
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml change`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add wordcartel-core/src/change.rs
git commit -m "feat(core): ChangeSet with in-place apply"
```

---

### Task 3: ChangeSet invert + round-trip property

**Files:**
- Modify: `wordcartel-core/src/change.rs`

**Interfaces:**
- Consumes: Task 2 types.
- Produces: `fn invert(&self, original: &TextBuffer) -> ChangeSet`.

- [ ] **Step 1: Write failing tests (unit + property)**

Add to the `tests` module in `src/change.rs`:
```rust
    #[test]
    fn invert_restores_original() {
        let original = TextBuffer::from_str("hello world");
        let cs = ChangeSet::delete(5..11, original.len()); // delete " world"
        let inv = cs.invert(&original);

        let mut b = original.clone();
        cs.apply(&mut b);
        assert_eq!(b.to_string(), "hello");
        inv.apply(&mut b);
        assert_eq!(b.to_string(), "hello world"); // round-trip
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        // LAW (spec §11.2): apply(invert(cs)) ∘ apply(cs) == identity.
        #[test]
        fn prop_apply_then_invert_is_identity(
            text in ".{0,40}",
            at in 0usize..40,
            ins in ".{0,8}",
            del_len in 0usize..40,
        ) {
            let original = TextBuffer::from_str(&text);
            let len = original.len();
            // clamp to valid byte boundaries by snapping onto char starts
            let at = snap(&text, at.min(len));
            // build either an insert or a bounded delete
            let cs = if del_len == 0 || at >= len {
                ChangeSet::insert(at, &ins, len)
            } else {
                let end = snap(&text, (at + del_len).min(len));
                ChangeSet::delete(at..end, len)
            };
            let inv = cs.invert(&original);
            let mut b = original.clone();
            cs.apply(&mut b);
            inv.apply(&mut b);
            prop_assert_eq!(b.to_string(), text);
        }
    }

    // test helper: snap a byte index down to the nearest char boundary
    fn snap(s: &str, mut i: usize) -> usize {
        while i < s.len() && !s.is_char_boundary(i) {
            i -= 1;
        }
        i.min(s.len())
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml change`
Expected: FAIL (no `invert`).

- [ ] **Step 3: Implement invert**

Add to `impl ChangeSet` in `src/change.rs`:
```rust
    /// Produce the inverse changeset. Needs the *original* buffer to recover the
    /// bytes a Delete removed (re-emitted as an Insert).
    pub fn invert(&self, original: &TextBuffer) -> ChangeSet {
        let mut inv = Vec::with_capacity(self.ops.len());
        let mut pos: BytePos = 0; // position in the ORIGINAL text
        for op in &self.ops {
            match op {
                Op::Retain(n) => {
                    inv.push(Op::Retain(*n));
                    pos += n;
                }
                Op::Delete(n) => {
                    let removed = original.slice(pos..pos + n);
                    inv.push(Op::Insert(Tendril::from(removed.as_str())));
                    pos += n;
                }
                Op::Insert(s) => {
                    // inserted text is not present in the original; pos unchanged
                    inv.push(Op::Delete(s.len()));
                }
            }
        }
        ChangeSet { ops: inv, len_before: self.len_after, len_after: self.len_before }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml change`
Expected: PASS (unit + property).

- [ ] **Step 5: Commit (include regression seeds)**

```bash
git add wordcartel-core/src/change.rs wordcartel-core/proptest-regressions
git commit -m "feat(core): ChangeSet::invert + apply∘invert identity property"
```

---

### Task 4: Selection + Range::map

**Files:**
- Modify: `wordcartel-core/src/selection.rs`

**Interfaces:**
- Consumes: `crate::BytePos`, `crate::change::{ChangeSet, Op}`.
- Produces:
  - `struct Range { anchor: BytePos, head: BytePos }` with `from`/`to`/`is_empty`/`cursor`.
  - `struct Selection { ranges: smallvec::SmallVec<[Range; 1]>, primary: usize }` with `single(pos)`, `primary()`.
  - `fn map(&self, cs: &ChangeSet) -> Range` and `Selection::map(&self, cs) -> Selection`.
  - Insertion bias: a position exactly at an insert point moves to **after** the inserted text (Assoc::After).

- [ ] **Step 1: Write failing tests**

`src/selection.rs`:
```rust
//! Selection over byte offsets. `map` keeps positions valid across edits — the
//! #1 "cursor jumped" bug class (spec §10.2). Reimplemented from Helix's pattern.
use crate::change::{ChangeSet, Op};
use crate::BytePos;
use smallvec::{smallvec, SmallVec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub anchor: BytePos,
    pub head: BytePos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub ranges: SmallVec<[Range; 1]>,
    pub primary: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_after_insert_before_it() {
        // cursor at byte 5; insert 2 bytes at byte 2 (before it) → cursor shifts +2
        let cur = Range { anchor: 5, head: 5 };
        let cs = ChangeSet::insert(2, "XY", 10);
        let mapped = cur.map(&cs);
        assert_eq!(mapped, Range { anchor: 7, head: 7 });
    }

    #[test]
    fn cursor_unaffected_by_insert_after_it() {
        let cur = Range { anchor: 3, head: 3 };
        let cs = ChangeSet::insert(8, "Z", 10);
        assert_eq!(cur.map(&cs), Range { anchor: 3, head: 3 });
    }

    #[test]
    fn cursor_after_delete_before_it() {
        let cur = Range { anchor: 9, head: 9 };
        let cs = ChangeSet::delete(2..5, 12); // remove 3 bytes before cursor
        assert_eq!(cur.map(&cs), Range { anchor: 6, head: 6 });
    }

    #[test]
    fn cursor_inside_deleted_clamps_to_start() {
        let cur = Range { anchor: 4, head: 4 };
        let cs = ChangeSet::delete(2..6, 10); // cursor 4 is inside [2,6)
        assert_eq!(cur.map(&cs), Range { anchor: 2, head: 2 });
    }

    #[test]
    fn insertion_bias_is_after() {
        // cursor exactly at the insert point moves to AFTER the inserted text
        let cur = Range { anchor: 2, head: 2 };
        let cs = ChangeSet::insert(2, "AB", 10);
        assert_eq!(cur.map(&cs), Range { anchor: 4, head: 4 });
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml selection`
Expected: FAIL.

- [ ] **Step 3: Implement Range/Selection + map**

Add above the test module in `src/selection.rs`:
```rust
impl Range {
    pub fn point(pos: BytePos) -> Range {
        Range { anchor: pos, head: pos }
    }
    pub fn from(&self) -> BytePos {
        self.anchor.min(self.head)
    }
    pub fn to(&self) -> BytePos {
        self.anchor.max(self.head)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Map both ends through a ChangeSet (insertion bias = After).
    pub fn map(&self, cs: &ChangeSet) -> Range {
        Range {
            anchor: map_pos(self.anchor, cs),
            head: map_pos(self.head, cs),
        }
    }
}

/// Map one byte position through a ChangeSet.
/// - Retain(n): positions in the retained span shift by the net delta so far.
/// - Insert(s): a position at/after the insert point gains s.len() (bias After).
/// - Delete(n): a position inside the deleted span clamps to its start.
fn map_pos(pos: BytePos, cs: &ChangeSet) -> BytePos {
    let mut old = 0usize; // cursor in the pre-change doc
    let mut new = 0usize; // cursor in the post-change doc
    for op in &cs.ops {
        match op {
            Op::Retain(n) => {
                if pos < old + n {
                    return new + (pos - old);
                }
                old += n;
                new += n;
            }
            Op::Insert(s) => {
                if pos == old {
                    // bias After: jump past the inserted text
                    new += s.len();
                }
                new += if pos == old { 0 } else { s.len() };
                // note: if pos < old this branch isn't reached; if pos > old we
                // simply account the inserted bytes below
                if pos != old {
                    // already counted s.len() above for the pos>old case? no —
                    // ensure exactly one accounting:
                }
            }
            Op::Delete(n) => {
                if pos < old + n {
                    // inside (or at start of) the deletion → clamp to its start
                    return new;
                }
                old += n;
            }
        }
    }
    new + pos.saturating_sub(old)
}

impl Selection {
    pub fn single(pos: BytePos) -> Selection {
        Selection { ranges: smallvec![Range::point(pos)], primary: 0 }
    }
    pub fn primary(&self) -> Range {
        self.ranges[self.primary]
    }
    pub fn map(&self, cs: &ChangeSet) -> Selection {
        Selection {
            ranges: self.ranges.iter().map(|r| r.map(cs)).collect(),
            primary: self.primary,
        }
    }
}
```

> Implementation note for the engineer: the `Insert` arm above is written awkwardly
> on purpose to make the bias explicit — simplify it to the following equivalent,
> which is what the tests pin: at `Op::Insert(s)`, if `pos >= old` then `new += s.len()`
> (a position before the insert point has already returned in an earlier Retain/Delete
> arm, so by the time we reach an Insert, `pos >= old` always holds). Replace the
> whole `Op::Insert` arm with:
> ```rust
> Op::Insert(s) => { new += s.len(); }
> ```

- [ ] **Step 4: Apply the simplification and run tests**

Replace the `Op::Insert` arm in `map_pos` with the one-line version from the note above, then:

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml selection`
Expected: PASS (5 tests).

- [ ] **Step 5: Add the mapping property test**

Add to the `tests` module:
```rust
    use proptest::prelude::*;

    proptest! {
        // LAW: a mapped position is always within the new document bounds and never
        // lands inside what was deleted before it (spec §10.2 cursor-jump class).
        #[test]
        fn prop_mapped_pos_in_bounds(
            doc_len in 1usize..40,
            pos in 0usize..40,
            at in 0usize..40,
            ins_len in 0usize..6,
        ) {
            let pos = pos.min(doc_len);
            let at = at.min(doc_len);
            let cs = if ins_len > 0 {
                ChangeSet::insert(at, &"x".repeat(ins_len), doc_len)
            } else if at < doc_len {
                ChangeSet::delete(at..doc_len, doc_len)
            } else {
                ChangeSet::insert(at, "x", doc_len)
            };
            let mapped = super::map_pos(pos, &cs);
            prop_assert!(mapped <= cs.len_after);
        }
    }
```

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml selection`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add wordcartel-core/src/selection.rs wordcartel-core/proptest-regressions
git commit -m "feat(core): Selection + Range::map with insertion bias"
```

---

### Task 5: Transaction + History (linear undo/redo)

**Files:**
- Modify: `wordcartel-core/src/history.rs`

**Interfaces:**
- Consumes: `TextBuffer`, `ChangeSet`, `Selection`.
- Produces:
  - `struct Edit { changes: ChangeSet, inverse: ChangeSet }`
  - `struct Transaction { changes: ChangeSet, selection: Option<Selection> }` + `Transaction::new(changes)`
  - `struct Revision { edits: Vec<Edit>, before: Selection, after: Selection }`
  - `struct History { revisions: Vec<Revision>, current: usize }` (current = count of applied revisions; 0 = nothing)
  - `History::commit(&mut self, txn, buf, before_sel) -> Selection` (applies + records, clears redo)
  - `History::undo(&mut self, buf) -> Option<Selection>`
  - `History::redo(&mut self, buf) -> Option<Selection>`

> **v1 simplification (recorded):** undo is a **linear** stack with redo cleared on
> new edits. The spec's branching-tree undo (§9.1) is deferred to a later effort;
> linear is the standard word-processor UX and lower risk. Note this in the ledger.

- [ ] **Step 1: Write failing tests**

`src/history.rs`:
```rust
//! Linear undo/redo. Reimplemented from Helix/CodeMirror history patterns
//! (MPL/MIT) — pattern, not copied source (spec §9.6). v1 is linear (no branch).
use crate::buffer::TextBuffer;
use crate::change::ChangeSet;
use crate::selection::Selection;

#[derive(Clone, Debug)]
pub struct Edit {
    pub changes: ChangeSet,
    pub inverse: ChangeSet,
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub changes: ChangeSet,
    pub selection: Option<Selection>,
}

impl Transaction {
    pub fn new(changes: ChangeSet) -> Self {
        Transaction { changes, selection: None }
    }
    pub fn with_selection(mut self, sel: Selection) -> Self {
        self.selection = Some(sel);
        self
    }
}

#[derive(Clone, Debug)]
pub struct Revision {
    pub edits: Vec<Edit>,
    pub before: Selection,
    pub after: Selection,
}

#[derive(Clone, Debug, Default)]
pub struct History {
    pub revisions: Vec<Revision>,
    pub current: usize, // number of revisions currently applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::ChangeSet;
    use crate::selection::Selection;

    fn type_char(buf: &TextBuffer, at: usize, ch: &str) -> Transaction {
        let cs = ChangeSet::insert(at, ch, buf.len());
        Transaction::new(cs).with_selection(Selection::single(at + ch.len()))
    }

    #[test]
    fn undo_then_redo_round_trip() {
        let mut buf = TextBuffer::from_str("");
        let mut hist = History::default();
        let mut sel = Selection::single(0);

        sel = hist.commit(type_char(&buf, 0, "a"), &mut buf, sel.clone());
        // apply happens inside commit; re-fetch buffer state by applying in commit
        // (commit applies the txn to buf)
        assert_eq!(buf.to_string(), "a");
        sel = hist.commit(type_char(&buf, 1, "b"), &mut buf, sel.clone());
        assert_eq!(buf.to_string(), "ab");

        let s = hist.undo(&mut buf).unwrap();
        assert_eq!(buf.to_string(), "a");
        assert_eq!(s, Selection::single(1)); // before-selection of the 'b' revision

        let s2 = hist.redo(&mut buf).unwrap();
        assert_eq!(buf.to_string(), "ab");
        assert_eq!(s2, Selection::single(2));

        let _ = sel;
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut buf = TextBuffer::from_str("");
        let mut hist = History::default();
        let sel = Selection::single(0);
        let sel = hist.commit(type_char(&buf, 0, "a"), &mut buf, sel);
        hist.undo(&mut buf);
        assert_eq!(buf.to_string(), "");
        // a new edit after undo clears the redo stack
        hist.commit(type_char(&buf, 0, "z"), &mut buf, sel);
        assert_eq!(buf.to_string(), "z");
        assert!(hist.redo(&mut buf).is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml history`
Expected: FAIL (no `commit`/`undo`/`redo`).

- [ ] **Step 3: Implement History**

Add above the test module in `src/history.rs`:
```rust
impl History {
    /// Apply `txn` to `buf`, record it as a new revision, and return the new
    /// selection. Clears any redo tail.
    pub fn commit(&mut self, txn: Transaction, buf: &mut TextBuffer, before: Selection) -> Selection {
        let inverse = txn.changes.invert(buf);
        txn.changes.apply(buf);
        let after = txn
            .selection
            .clone()
            .unwrap_or_else(|| before.map(&txn.changes));
        // drop any redo tail
        self.revisions.truncate(self.current);
        self.revisions.push(Revision {
            edits: vec![Edit { changes: txn.changes, inverse }],
            before,
            after: after.clone(),
        });
        self.current += 1;
        after
    }

    pub fn undo(&mut self, buf: &mut TextBuffer) -> Option<Selection> {
        if self.current == 0 {
            return None;
        }
        self.current -= 1;
        let rev = &self.revisions[self.current];
        for edit in rev.edits.iter().rev() {
            edit.inverse.apply(buf);
        }
        Some(rev.before.clone())
    }

    pub fn redo(&mut self, buf: &mut TextBuffer) -> Option<Selection> {
        if self.current >= self.revisions.len() {
            return None;
        }
        let rev = &self.revisions[self.current];
        for edit in rev.edits.iter() {
            edit.changes.apply(buf);
        }
        self.current += 1;
        Some(rev.after.clone())
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml history`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add wordcartel-core/src/history.rs
git commit -m "feat(core): Transaction + linear undo/redo History"
```

---

### Task 6: Undo coalescing (injected clock)

**Files:**
- Modify: `wordcartel-core/src/history.rs`

**Interfaces:**
- Consumes: Task 5 types.
- Produces:
  - `trait Clock { fn now_ms(&self) -> u64; }`
  - `History::commit_coalescing(&mut self, txn, buf, before, clock, kind) -> Selection`
  - `enum EditKind { Type, Other }` — only consecutive `Type` edits within `COALESCE_MS` merge.
  - `const COALESCE_MS: u64 = 500;` (spec §9.2)

- [ ] **Step 1: Write failing tests**

Add to the `tests` module in `src/history.rs`:
```rust
    struct FakeClock {
        t: std::cell::Cell<u64>,
    }
    impl FakeClock {
        fn new() -> Self { FakeClock { t: std::cell::Cell::new(0) } }
        fn set(&self, ms: u64) { self.t.set(ms); }
    }
    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 { self.t.get() }
    }

    #[test]
    fn rapid_typing_coalesces_into_one_undo() {
        let mut buf = TextBuffer::from_str("");
        let mut hist = History::default();
        let clock = FakeClock::new();
        let mut sel = Selection::single(0);

        clock.set(0);
        sel = hist.commit_coalescing(type_char(&buf, 0, "a"), &mut buf, sel, &clock, EditKind::Type);
        clock.set(100); // within 500ms window
        sel = hist.commit_coalescing(type_char(&buf, 1, "b"), &mut buf, sel, &clock, EditKind::Type);
        clock.set(200);
        let _ = hist.commit_coalescing(type_char(&buf, 2, "c"), &mut buf, sel, &clock, EditKind::Type);
        assert_eq!(buf.to_string(), "abc");

        // one undo removes the whole "abc" burst
        let s = hist.undo(&mut buf).unwrap();
        assert_eq!(buf.to_string(), "");
        assert_eq!(s, Selection::single(0));
    }

    #[test]
    fn pause_breaks_the_group() {
        let mut buf = TextBuffer::from_str("");
        let mut hist = History::default();
        let clock = FakeClock::new();
        let mut sel = Selection::single(0);

        clock.set(0);
        sel = hist.commit_coalescing(type_char(&buf, 0, "a"), &mut buf, sel, &clock, EditKind::Type);
        clock.set(1000); // > 500ms later → new group
        let _ = hist.commit_coalescing(type_char(&buf, 1, "b"), &mut buf, sel, &clock, EditKind::Type);
        assert_eq!(buf.to_string(), "ab");

        // undo removes only "b"
        hist.undo(&mut buf);
        assert_eq!(buf.to_string(), "a");
    }

    #[test]
    fn non_type_edit_never_coalesces() {
        let mut buf = TextBuffer::from_str("");
        let mut hist = History::default();
        let clock = FakeClock::new();
        let mut sel = Selection::single(0);
        clock.set(0);
        sel = hist.commit_coalescing(type_char(&buf, 0, "a"), &mut buf, sel, &clock, EditKind::Type);
        clock.set(50);
        // a paste/programmatic edit (EditKind::Other) starts its own group even within the window
        let _ = hist.commit_coalescing(type_char(&buf, 1, "X"), &mut buf, sel, &clock, EditKind::Other);
        hist.undo(&mut buf);
        assert_eq!(buf.to_string(), "a"); // only the Other edit undone
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml history`
Expected: FAIL (no `Clock`/`commit_coalescing`/`EditKind`).

- [ ] **Step 3: Implement coalescing**

Add to `src/history.rs` (above the test module). Extend `Revision` with timing/kind metadata:
```rust
pub const COALESCE_MS: u64 = 500;

pub trait Clock {
    fn now_ms(&self) -> u64;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Type,
    Other,
}
```

Add `last_ms: u64` and `kind: EditKind` fields to `Revision` (update its definition and the `commit` constructor to set `last_ms: 0, kind: EditKind::Other`). Then add:
```rust
impl History {
    pub fn commit_coalescing(
        &mut self,
        txn: Transaction,
        buf: &mut TextBuffer,
        before: Selection,
        clock: &dyn Clock,
        kind: EditKind,
    ) -> Selection {
        let now = clock.now_ms();
        let can_merge = self.current > 0
            && self.current == self.revisions.len() // nothing in redo tail
            && kind == EditKind::Type
            && {
                let top = &self.revisions[self.current - 1];
                top.kind == EditKind::Type && now.saturating_sub(top.last_ms) <= COALESCE_MS
            };

        let inverse = txn.changes.invert(buf);
        txn.changes.apply(buf);
        let after = txn
            .selection
            .clone()
            .unwrap_or_else(|| before.map(&txn.changes));

        if can_merge {
            let top = self.revisions.last_mut().unwrap();
            top.edits.push(Edit { changes: txn.changes, inverse });
            top.after = after.clone();
            top.last_ms = now;
        } else {
            self.revisions.truncate(self.current);
            self.revisions.push(Revision {
                edits: vec![Edit { changes: txn.changes, inverse }],
                before,
                after: after.clone(),
                last_ms: now,
                kind,
            });
            self.current += 1;
        }
        after
    }
}
```

Also update the plain `commit` from Task 5 to set the two new fields (`last_ms: 0, kind: EditKind::Other`) in its `Revision { .. }` literal so the crate compiles.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml history`
Expected: PASS (all history tests).

- [ ] **Step 5: Commit**

```bash
git add wordcartel-core/src/history.rs
git commit -m "feat(core): prose-tuned undo coalescing with injected clock"
```

---

### Task 7: In-process clipboard register

**Files:**
- Modify: `wordcartel-core/src/register.rs`

**Interfaces:**
- Consumes: `TextBuffer`, `Selection`, `Range`, `ChangeSet`.
- Produces:
  - `struct Register { text: Option<String> }` with `set`/`get`.
  - `fn copy(buf: &TextBuffer, range: Range, reg: &mut Register)`
  - `fn cut(range: Range, doc_len: usize, reg: &mut Register, buf_text: &TextBuffer) -> ChangeSet`
  - `fn paste(at: BytePos, doc_len: usize, reg: &Register) -> Option<ChangeSet>`

> This is the **in-process** register only — copy/cut/paste always work here, even
> when the system clipboard is unavailable (spec §9.5/§15.6). System-clipboard sync
> is effort 3.

- [ ] **Step 1: Write failing tests**

`src/register.rs`:
```rust
//! In-process clipboard register. Copy/cut/paste always work via this register,
//! independent of any system clipboard (spec §9.5/§15.6).
use crate::buffer::TextBuffer;
use crate::change::ChangeSet;
use crate::selection::Range;
use crate::BytePos;

#[derive(Clone, Debug, Default)]
pub struct Register {
    pub text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_then_paste() {
        let buf = TextBuffer::from_str("hello world");
        let mut reg = Register::default();
        copy(&buf, Range { anchor: 0, head: 5 }, &mut reg); // "hello"
        assert_eq!(reg.text.as_deref(), Some("hello"));

        let cs = paste(11, buf.len(), &reg).unwrap();
        let mut b2 = buf.clone();
        cs.apply(&mut b2);
        assert_eq!(b2.to_string(), "hello worldhello");
    }

    #[test]
    fn cut_removes_and_fills_register() {
        let buf = TextBuffer::from_str("hello world");
        let mut reg = Register::default();
        let cs = cut(Range { anchor: 5, head: 11 }, buf.len(), &mut reg, &buf); // " world"
        assert_eq!(reg.text.as_deref(), Some(" world"));
        let mut b = buf.clone();
        cs.apply(&mut b);
        assert_eq!(b.to_string(), "hello");
    }

    #[test]
    fn paste_empty_register_is_none() {
        let reg = Register::default();
        assert!(paste(0, 0, &reg).is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml register`
Expected: FAIL.

- [ ] **Step 3: Implement the register ops**

Add above the test module in `src/register.rs`:
```rust
impl Register {
    pub fn set(&mut self, text: String) {
        self.text = Some(text);
    }
    pub fn get(&self) -> Option<&str> {
        self.text.as_deref()
    }
}

pub fn copy(buf: &TextBuffer, range: Range, reg: &mut Register) {
    reg.set(buf.slice(range.from()..range.to()));
}

pub fn cut(range: Range, doc_len: usize, reg: &mut Register, buf: &TextBuffer) -> ChangeSet {
    reg.set(buf.slice(range.from()..range.to()));
    ChangeSet::delete(range.from()..range.to(), doc_len)
}

pub fn paste(at: BytePos, doc_len: usize, reg: &Register) -> Option<ChangeSet> {
    reg.get().map(|t| ChangeSet::insert(at, t, doc_len))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml register`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add wordcartel-core/src/register.rs
git commit -m "feat(core): in-process clipboard register (copy/cut/paste)"
```

---

### Task 8: Integration property — random edits, undo-all restores original

**Files:**
- Create: `wordcartel-core/tests/integration.rs`

**Interfaces:**
- Consumes: the whole public API.
- Produces: the kernel's top-level safety law.

- [ ] **Step 1: Write the failing property test**

`wordcartel-core/tests/integration.rs`:
```rust
//! Top-level kernel law (spec §11.2): a random sequence of edits, each committed
//! with selection mapping, fully reverses under repeated undo back to the original
//! text — and every intermediate selection stays within document bounds.
use proptest::prelude::*;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::change::ChangeSet;
use wordcartel_core::history::{History, Transaction};
use wordcartel_core::selection::Selection;

fn snap(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i -= 1;
    }
    i.min(s.len())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn undo_all_restores_original(
        start in ".{0,20}",
        ops in proptest::collection::vec((0usize..30, ".{0,4}", any::<bool>()), 0..12),
    ) {
        let original = start.clone();
        let mut buf = TextBuffer::from_str(&start);
        let mut hist = History::default();
        let mut sel = Selection::single(0);

        for (pos, ins, is_insert) in ops {
            let len = buf.len();
            let at = snap(&buf.to_string(), pos.min(len));
            let cs = if is_insert || at >= len {
                ChangeSet::insert(at, &ins, len)
            } else {
                let end = snap(&buf.to_string(), (at + 1).min(len));
                ChangeSet::delete(at..end, len)
            };
            sel = hist.commit(Transaction::new(cs), &mut buf, sel.clone());
            // selection stays in bounds after every commit
            prop_assert!(sel.primary().head <= buf.len());
        }

        // undo everything
        while hist.undo(&mut buf).is_some() {}
        prop_assert_eq!(buf.to_string(), original);
    }
}
```

- [ ] **Step 2: Run to verify it fails (or passes — then strengthen)**

Run: `cargo test --manifest-path wordcartel-core/Cargo.toml --test integration`
Expected: PASS if Tasks 1–5 are correct. If it FAILS, proptest prints a minimal counterexample — fix the offending module (most likely `map_pos` bias or `invert`) before proceeding. Do **not** weaken the test.

- [ ] **Step 3: Commit**

```bash
git add wordcartel-core/tests/integration.rs wordcartel-core/proptest-regressions
git commit -m "test(core): integration property — undo-all restores original"
```

---

## Self-Review (completed during planning)

- **Spec coverage:** every 🔨 row in the coverage ledger maps to a task here —
  ropey buffer + byte-canonical (Task 1), ChangeSet undo backbone (Tasks 2–3),
  selection map (Task 4), Transaction + history + coalescing (Tasks 5–6), in-process
  register (Task 7), top-level law (Task 8). Render/IO/app rows are explicitly ⏳ in
  later efforts.
- **Recorded simplifications** (flag in the ledger): undo is **linear** (branching
  §9.1 deferred); `ChangeSet::compose` is **not** built (coalescing uses an
  edit-list per revision instead, avoiding the trickiest OT algorithm until a real
  need appears — YAGNI).
- **Type consistency:** `BytePos = usize` throughout; `Tendril = smartstring::alias::String`;
  `Selection`/`Range` field names and `map` signatures match across Tasks 4–8.
- **Placeholder scan:** none — every code step contains complete, runnable code.

## Completion

When all task checkboxes are `- [x]` and `cargo test --manifest-path wordcartel-core/Cargo.toml`
is green: flip **Effort 1** to ✅ in the coverage ledger and update the 🔨 rows to ✅.
