# M2 — Adversarial Edit-Submission Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A validated, plugin-ready `submit_transaction` boundary that accepts an untrusted `Transaction`, validates it against the live buffer, and applies it (trusted path) or returns `Err` with **zero mutation** — never panics, never partially edits.

**Architecture:** Core (`wordcartel-core`) gets the buffer-relative validation (`ChangeSet::validate_against`) + a buffer-only snap primitive (`TextBuffer::clamp_to_boundary`) + a proptest proving "validate Ok ⟹ apply never panics." The shell (`wordcartel`) gets the `submit_transaction` boundary (clone-snap-then-apply) + adversarial unit tests. Internal trusted edits are untouched.

**Tech Stack:** Rust, `wordcartel-core` (`#![forbid(unsafe_code)]`, ropey), `proptest` (core dev-dep), `wordcartel` shell.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-06-28-wordcartel-m2-edit-submission-boundary-design.md` (Codex-reviewed ×3, CLEAN).
- Decisions: minimal `submit_transaction(editor, txn, clock) -> Result<(), EditError>`; conservative whole-doc Edit; **hard-reject the mutation (StaleLength, OpBoundary), snap the cursor** (`EditError` = 2 variants). Result-wiring/op-boundary-for-frequent-edits/IO-faults are M3/M4/P.
- `validate_against` op-walk tracks an **OLD-text cursor**: `Retain(n)` advances; `Delete(n)` checks **both** `old_pos` and `old_pos+n` then advances; `Insert(_)` checks `old_pos` only and does NOT advance.
- Selection snap is **fully pre-apply** via a buffer clone (history must record the snapped cursor). Single-range only. `nav::clamp_snap` is **NOT** touched (layout-coupled).
- Baseline at branch start: `cargo test -p wordcartel-core` (192 lib + 42 oracle) and `cargo test -p wordcartel` (568) green. Keep green.
- `#![forbid(unsafe_code)]` in core — no unsafe.
- Commit trailers on EVERY commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

## File Structure

- `wordcartel-core/src/buffer.rs` — `is_char_boundary` → `pub(crate)`; add `clamp_to_boundary`. (MODIFY)
- `wordcartel-core/src/change.rs` — add `EditError` + `ChangeSet::validate_against` + tests + the core proptest. (MODIFY)
- `wordcartel/src/transact.rs` — NEW: `submit_transaction` + re-export `EditError` + adversarial unit tests.
- `wordcartel/src/lib.rs` (or `main.rs`) — declare `mod transact;`. (MODIFY)

---

### Task 1: Core validation + snap primitive + proptest

**Files:**
- Modify: `wordcartel-core/src/buffer.rs` (`is_char_boundary` ~25; add `clamp_to_boundary`)
- Modify: `wordcartel-core/src/change.rs` (add `EditError` + `validate_against`; tests + proptest)

**Interfaces:**
- Produces: `wordcartel_core::change::EditError` (`StaleLength{expected,actual}`, `OpBoundary{pos}`); `ChangeSet::validate_against(&self, buf: &TextBuffer) -> Result<(), EditError>`; `TextBuffer::clamp_to_boundary(&self, off: usize) -> usize`. `TextBuffer::is_char_boundary` becomes `pub(crate)`.

- [ ] **Step 1: Write failing tests**

In `wordcartel-core/src/buffer.rs` tests mod:

```rust
#[test]
fn clamp_to_boundary_clamps_and_floors() {
    let b = TextBuffer::from_str("aé"); // 'a'=1 byte, 'é'=2 bytes → len 3; byte 2 is mid-é
    assert_eq!(b.clamp_to_boundary(99), 3, "clamps past len");
    assert_eq!(b.clamp_to_boundary(2), 1, "floors mid-char byte 2 to boundary 1");
    assert_eq!(b.clamp_to_boundary(0), 0);
    assert_eq!(b.clamp_to_boundary(3), 3);
}
```

In `wordcartel-core/src/change.rs` tests mod:

```rust
#[test]
fn validate_against_ok_for_matching_valid_changeset() {
    use super::*;
    let buf = TextBuffer::from_str("hello"); // len 5
    let cs = ChangeSet::insert(2, "X", 5);    // built for len 5, boundary 2 is valid
    assert!(cs.validate_against(&buf).is_ok());
}

#[test]
fn validate_against_stale_length() {
    use super::*;
    let buf = TextBuffer::from_str("hello"); // len 5
    let cs = ChangeSet::insert(0, "X", 3);    // built for len 3 ≠ 5
    assert_eq!(cs.validate_against(&buf), Err(EditError::StaleLength { expected: 3, actual: 5 }));
}

#[test]
fn validate_against_delete_end_mid_char() {
    use super::*;
    // doc "é" = 2 bytes (0xC3 0xA9); a Delete(1) ends at byte 1 (mid-char).
    let buf = TextBuffer::from_str("é"); // len 2
    let cs = ChangeSet::from_ops(vec![Op::Delete(1), Op::Retain(1)], 2); // sum-valid
    assert_eq!(cs.validate_against(&buf), Err(EditError::OpBoundary { pos: 1 }));
}

#[test]
fn validate_against_insert_mid_char() {
    use super::*;
    let buf = TextBuffer::from_str("é"); // len 2
    // Retain(1) lands old_pos at byte 1 (mid-é), then Insert there.
    let cs = ChangeSet::from_ops(vec![Op::Retain(1), Op::Insert(Tendril::from("x")), Op::Retain(1)], 2);
    assert_eq!(cs.validate_against(&buf), Err(EditError::OpBoundary { pos: 1 }));
}
```

- [ ] **Step 2: Run, verify they fail**

Run: `cargo test -p wordcartel-core clamp_to_boundary validate_against`
Expected: FAIL — `clamp_to_boundary`/`validate_against`/`EditError` not found.

- [ ] **Step 3: Add `clamp_to_boundary` + make `is_char_boundary` `pub(crate)`**

In `wordcartel-core/src/buffer.rs`, change `fn is_char_boundary` (line ~25) to `pub(crate) fn is_char_boundary`. Add to `impl TextBuffer`:

```rust
    /// Clamp `off` to `[0, len]` and floor it to a char boundary. Pure buffer-local
    /// (no layout). Used to snap an untrusted/plugin-submitted offset to a safe position.
    pub fn clamp_to_boundary(&self, off: usize) -> usize {
        let off = off.min(self.len());
        self.rope.char_to_byte(self.rope.byte_to_char(off))
    }
```

- [ ] **Step 4: Add `EditError` + `validate_against`**

In `wordcartel-core/src/change.rs`, add near the top (after the `Op`/`ChangeSet` defs):

```rust
/// Why an untrusted changeset was rejected against a specific buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditError {
    /// The changeset was built for a different document length than the buffer has.
    StaleLength { expected: usize, actual: usize },
    /// An op's byte boundary lands inside a multibyte char in the buffer.
    OpBoundary { pos: usize },
}
```

In `impl ChangeSet`, add:

```rust
    /// Validate this changeset against `buf` WITHOUT mutating. Length must match
    /// (→ `StaleLength`), and every op's OLD-text byte boundaries must be char
    /// boundaries in `buf` (→ `OpBoundary`) — so a later `apply` cannot panic partway.
    /// Returns the FIRST violation; `Ok(())` means `apply(buf)` is panic-safe.
    pub fn validate_against(&self, buf: &TextBuffer) -> Result<(), EditError> {
        if self.len_before != buf.len() {
            return Err(EditError::StaleLength { expected: self.len_before, actual: buf.len() });
        }
        let mut old_pos: usize = 0;
        for op in &self.ops {
            match op {
                Op::Retain(n) => old_pos += n,
                Op::Delete(n) => {
                    if !buf.is_char_boundary(old_pos) { return Err(EditError::OpBoundary { pos: old_pos }); }
                    let end = old_pos + n;
                    if !buf.is_char_boundary(end) { return Err(EditError::OpBoundary { pos: end }); }
                    old_pos = end;
                }
                Op::Insert(_) => {
                    if !buf.is_char_boundary(old_pos) { return Err(EditError::OpBoundary { pos: old_pos }); }
                    // old_pos unchanged: insert adds to the NEW text, not the OLD.
                }
            }
        }
        Ok(())
    }
```

(`validate_against` is in `change.rs` → same module as the private `ops`; `is_char_boundary` is now `pub(crate)`; `buf.len()` is public. All accessible.)

- [ ] **Step 5: Run, verify pass**

Run: `cargo test -p wordcartel-core clamp_to_boundary validate_against`
Expected: PASS (5 tests).

- [ ] **Step 6: Write the core proptest (the M2 guarantee)**

In `wordcartel-core/src/change.rs` tests mod (proptest is already a dev-dep — see the existing `prop_*` tests in this file for the import pattern):

```rust
proptest::proptest! {
    /// The load-bearing guarantee: if `validate_against` returns Ok, then `apply`
    /// NEVER panics and yields exactly `len_after`. (If Err, nothing is applied.)
    #[test]
    fn validated_changeset_applies_without_panic(
        doc in proptest::collection::vec(
            proptest::sample::select(vec!['a', 'é', '中', '🙂', '\n']), 0..24usize
        ).prop_map(|cs| cs.into_iter().collect::<String>()),
        claimed_len in 0usize..28,
        is_delete in proptest::bool::ANY,
        p1 in 0usize..28,
        p2 in 0usize..28,
        text in proptest::string::string_regex("[aé中]{0,4}").unwrap(),
    ) {
        use super::*;
        let buf = TextBuffer::from_str(&doc);
        // Build a SUM-VALID changeset (valid-by-construction via M1's insert/delete)
        // for `claimed_len` — which may NOT match the buffer (→ StaleLength) and whose
        // positions may land mid-char in the buffer (→ OpBoundary). insert/delete assert
        // their offset ≤ claimed_len, so keep positions in range of claimed_len.
        let cs = if is_delete {
            let a = p1 % (claimed_len + 1);
            let b = p2 % (claimed_len + 1);
            ChangeSet::delete(a.min(b)..a.max(b), claimed_len)
        } else {
            let at = p1 % (claimed_len + 1);
            ChangeSet::insert(at, &text, claimed_len)
        };
        match cs.validate_against(&buf) {
            Ok(()) => {
                let mut b = buf.clone();
                cs.apply(&mut b);                       // must NOT panic
                proptest::prop_assert_eq!(b.len(), cs.len_after());
            }
            Err(_) => { /* rejected — buffer never touched */ }
        }
    }
}
```

- [ ] **Step 7: Run the proptest + the full core suite**

Run: `cargo test -p wordcartel-core validated_changeset_applies_without_panic && cargo test -p wordcartel-core`
Expected: PASS — the proptest green at its default case count; all 192 lib + 42 oracle still green.

- [ ] **Step 8: Commit**

```bash
git add wordcartel-core/src/buffer.rs wordcartel-core/src/change.rs
git commit -m "feat(m2): ChangeSet::validate_against + TextBuffer::clamp_to_boundary + proptest

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

### Task 2: The `submit_transaction` boundary + adversarial harness

**Files:**
- Create: `wordcartel/src/transact.rs`
- Modify: `wordcartel/src/lib.rs` (or `main.rs` — wherever modules are declared; grep `mod scratch;`) to add `mod transact;`

**Interfaces:**
- Consumes: `ChangeSet::validate_against`, `TextBuffer::clamp_to_boundary`, `EditError` (Task 1); `editor.apply`, `Transaction::new`/`with_selection`, `Selection::range`, `block_tree::Edit`, `ChangeSet::len_before()`/`len_after()` (M1).
- Produces: `wordcartel::transact::submit_transaction(editor: &mut Editor, txn: Transaction, clock: &dyn Clock) -> Result<(), EditError>`; re-exported `EditError`.

- [ ] **Step 1: Write the adversarial unit tests**

Create `wordcartel/src/transact.rs` with the tests first:

```rust
//! M2: the untrusted edit-submission boundary (Effort P's `apply(Transaction)` seam).
//! Validates an untrusted Transaction against the live buffer; on Err: zero mutation.
//! On Ok: snaps the cursor, derives a conservative whole-doc Edit, applies via the
//! trusted `editor.apply`. Never panics, never partially edits.

use crate::editor::Editor;
pub use wordcartel_core::change::EditError;
use wordcartel_core::history::{Clock, EditKind, Transaction};
use wordcartel_core::selection::Selection;

#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_core::change::{ChangeSet, Op, Tendril};
    struct C(u64);
    impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }

    fn ed(s: &str) -> Editor { Editor::new_from_text(s, None, (40, 10)) }

    #[test]
    fn valid_transaction_applies() {
        let mut e = ed("hello\n"); // len 6
        let cs = ChangeSet::insert(0, "X", 6);
        let r = submit_transaction(&mut e, Transaction::new(cs), &C(0));
        assert!(r.is_ok());
        assert_eq!(e.active().document.buffer.to_string(), "Xhello\n");
    }

    #[test]
    fn stale_length_rejected_no_mutation() {
        let mut e = ed("hello\n"); // len 6
        let before = e.active().document.buffer.to_string();
        let cs = ChangeSet::insert(0, "X", 3); // built for len 3 ≠ 6
        let r = submit_transaction(&mut e, Transaction::new(cs), &C(0));
        assert!(matches!(r, Err(EditError::StaleLength { .. })));
        assert_eq!(e.active().document.buffer.to_string(), before, "buffer unchanged");
    }

    #[test]
    fn op_boundary_rejected_no_mutation_no_panic() {
        let mut e = ed("é\n"); // 'é'=2 bytes + '\n' → len 3
        let before = e.active().document.buffer.to_string();
        // Delete(1) ends at byte 1 (mid-é); Retain(2) covers "\n"+... sum = 1+2 = 3.
        let cs = ChangeSet::from_ops(vec![Op::Delete(1), Op::Retain(2)], 3);
        let r = submit_transaction(&mut e, Transaction::new(cs), &C(0));
        assert!(matches!(r, Err(EditError::OpBoundary { .. })));
        assert_eq!(e.active().document.buffer.to_string(), before, "buffer unchanged");
    }

    #[test]
    fn out_of_bounds_selection_snaps_not_rejects() {
        let mut e = ed("hi\n"); // len 3
        let cs = ChangeSet::insert(0, "X", 3); // → len_after 4 ("Xhi\n")
        let txn = Transaction::new(cs).with_selection(Selection::range(999, 999));
        let r = submit_transaction(&mut e, txn, &C(0));
        assert!(r.is_ok());
        assert_eq!(e.active().document.buffer.to_string(), "Xhi\n");
        let head = e.active().document.selection.primary().head;
        assert!(head <= 4, "cursor snapped into [0, len_after]; got {head}");
    }
}
```

- [ ] **Step 2: Declare the module + run, verify failure**

Add `mod transact;` to the module list (grep `mod scratch;` to find it; likely `wordcartel/src/main.rs` or `lib.rs`).
Run: `cargo test -p wordcartel transact::`
Expected: FAIL — `submit_transaction` not defined.

- [ ] **Step 3: Implement `submit_transaction`**

Add to `wordcartel/src/transact.rs` (above the tests mod):

```rust
/// Untrusted edit-submission boundary. See module docs.
pub fn submit_transaction(
    editor: &mut Editor,
    txn: Transaction,
    clock: &dyn Clock,
) -> Result<(), EditError> {
    // 1. Validate against the LIVE buffer — no mutation. Early-return on Err.
    txn.changes.validate_against(&editor.active().document.buffer)?;

    let len_before = txn.changes.len_before();
    let len_after = txn.changes.len_after();

    // 2. Snap the (single-range) selection against a CLONE, pre-apply, so history records
    //    the snapped cursor (redo-safe). Cursor positions snap — never a reject.
    let snapped_sel: Option<Selection> = txn.selection.as_ref().map(|sel| {
        let mut clone = editor.active().document.buffer.clone();
        txn.changes.apply(&mut clone); // validated → cannot panic; gives post-edit text
        let r = sel.primary();
        Selection::range(clone.clamp_to_boundary(r.anchor), clone.clamp_to_boundary(r.head))
    });

    // 3. Conservative whole-doc reparse Edit.
    let edit = wordcartel_core::block_tree::Edit { range: 0..len_before, new_len: len_after };

    // 4. Build the final transaction (original changes + snapped selection) and apply
    //    once via the trusted path — the only live mutation.
    let mut final_txn = Transaction::new(txn.changes);
    if let Some(sel) = snapped_sel { final_txn = final_txn.with_selection(sel); }
    editor.apply(final_txn, edit, EditKind::Other, clock);
    Ok(())
}
```

NOTE on borrows: step 1's `validate_against` borrow and step 2's `clone()`/`apply` borrows of `editor.active()` are immutable and end before step 4's `editor.apply` (mut). `len_before`/`len_after` are read before `txn.changes` is moved into `Transaction::new`. The `txn.selection.as_ref()` borrow ends when `snapped_sel` is produced (owned). If the borrow checker complains about `txn` being partially moved, bind `let changes = txn.changes;` / `let selection = txn.selection;` up front and operate on the locals.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p wordcartel transact::`
Expected: PASS (4 tests).

- [ ] **Step 5: Run the full shell + core suites**

Run: `cargo test -p wordcartel-core && cargo test -p wordcartel`
Expected: all green — `submit_transaction` is additive; no existing test affected (internal edit paths and `editor.apply` are unchanged).

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/transact.rs wordcartel/src/main.rs wordcartel/src/lib.rs
git commit -m "feat(m2): submit_transaction untrusted edit boundary + adversarial tests

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

## Self-Review

**Spec coverage:**
- `is_char_boundary` → `pub(crate)`; `clamp_to_boundary` → Task 1. ✓
- `EditError` + `validate_against` (OLD-cursor walk, Delete both endpoints) → Task 1. ✓
- Core proptest (validate Ok ⟹ apply never panics + len_after) → Task 1. ✓
- `submit_transaction` (validate → clone-snap pre-apply → conservative Edit → editor.apply) → Task 2. ✓
- Single-range selection via `primary()` + `Selection::range`; `nav::clamp_snap` untouched → Task 2. ✓
- Adversarial unit tests (valid / StaleLength / OpBoundary no-mutation no-panic / selection snaps) → Task 2. ✓
- Out of scope (Result wiring, IO faults, merge-panic, tight Edit) → not built. ✓

**Type consistency:** `validate_against(&self, buf: &TextBuffer) -> Result<(), EditError>`, `clamp_to_boundary(&self, off: usize) -> usize`, `submit_transaction(editor, txn, clock) -> Result<(), EditError>`, `EditError::{StaleLength{expected,actual}, OpBoundary{pos}}` — consistent across tasks.

**Placeholder scan:** no TBD/TODO; the `mod transact;` location is grep-resolved (one line), and the borrow-checker fallback (bind locals) is a concrete instruction, not a placeholder.
