# M1 — ChangeSet/Selection Valid-by-Construction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an inconsistent `ChangeSet` or `Selection` impossible to construct, and make `ChangeSet::apply` fail fast on a length-mismatch precondition — closing the raw-field corruption hole (D0) without `Result` plumbing.

**Architecture:** Pure `wordcartel-core` change + a small `wordcartel` shell migration. Add a validating `ChangeSet::from_ops` and a `len_after()` accessor; harden `insert`/`delete` (clamp → release-assert); add an `apply` entry length-assert; then privatize the invariant-bearing fields and migrate all construction/read sites onto the constructors. `Selection.{ranges,primary}` privatized the same way; `Range.{anchor,head}` stay public.

**Tech Stack:** Rust, `wordcartel-core` (`#![forbid(unsafe_code)]`, ropey, smartstring/Tendril), `proptest` (dev-dep).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-06-28-wordcartel-m1-changeset-selection-valid-design.md`.
- D0 policy: edits are **valid-by-construction**; an invalid offset/op **fails fast (release `assert!`)**, never silently clamps or corrupts. The `Result`-returning boundary and the op-boundary preflight are **out of scope** (M2/plugin).
- Keep `Range.{anchor, head}` **public**. Do NOT add `Selection::from_ranges`/`ranges()` (no multi-range caller exists — YAGNI).
- `len_after = sum(Retain) + sum(Insert byte len)`; consumption invariant `sum(Retain) + sum(Delete) == len_before`.
- Baseline at branch start: `cargo test -p wordcartel-core` and `cargo test -p wordcartel` green (182 core lib + 42 oracle; 568 shell). Keep them green.
- `#![forbid(unsafe_code)]` in core — no unsafe.
- Commit trailers on EVERY commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

## File Structure

- `wordcartel-core/src/change.rs` — `ChangeSet`: add `from_ops`, `len_after()`; harden `insert`/`delete`; `apply` length-assert; later privatize fields. (MODIFY)
- `wordcartel-core/src/selection.rs` — privatize `Selection.{ranges,primary}`; fix the `len_after` read to use the accessor. (MODIFY)
- `wordcartel/src/commands.rs` — `build_multi_replace`/`build_range_replace` → `from_ops`; raw `Selection` sites → `Selection::range`. (MODIFY)

---

### Task 1: ChangeSet API hardening (additive — fields stay public)

Adds the new/validating API and the fail-fast behavior **without** privatizing yet, so the task compiles standalone (existing raw-construction sites still work). Privatization + migration is Task 2.

**Files:**
- Modify: `wordcartel-core/src/change.rs` (`insert` ~37–49, `delete` ~57–79, `apply` ~83–95; add `from_ops` + `len_after()`; tests mod ~243,256 and the `#[cfg(test)]` block)

**Interfaces:**
- Produces: `ChangeSet::from_ops(ops: Vec<Op>, len_before: usize) -> ChangeSet`; `ChangeSet::len_after(&self) -> usize`. `insert`/`delete` now `assert!` (no clamp). `apply` asserts `buf.len() == len_before`.

- [ ] **Step 1: Write the failing tests**

In `wordcartel-core/src/change.rs` `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn from_ops_computes_len_after_and_accepts_valid() {
    use super::*;
    // doc "abc" (3), delete "b", insert "XY": Retain(1) Delete(1) Insert("XY") Retain(1)
    let ops = vec![Op::Retain(1), Op::Delete(1), Op::Insert(Tendril::from("XY")), Op::Retain(1)];
    let cs = ChangeSet::from_ops(ops, 3);
    assert_eq!(cs.len_before(), 3);
    assert_eq!(cs.len_after(), 4); // retain 2 + insert 2
}

#[test]
#[should_panic(expected = "len_before")]
fn from_ops_rejects_non_summing_ops() {
    use super::*;
    // Retain(1)+Delete(1) = 2, but len_before claimed 5.
    let _ = ChangeSet::from_ops(vec![Op::Retain(1), Op::Delete(1), Op::Retain(1)], 5);
}

#[test]
#[should_panic(expected = "len_before")]
fn apply_rejects_buffer_length_mismatch() {
    use super::*;
    let cs = ChangeSet::insert(0, "x", 3); // built for doc_len 3
    let mut buf = TextBuffer::from_str("ab"); // len 2 ≠ 3
    cs.apply(&mut buf);
}

#[test]
#[should_panic(expected = "doc_len")]
fn insert_panics_past_doc_len() {
    use super::*;
    let _ = ChangeSet::insert(10, "x", 3);
}

#[test]
#[should_panic(expected = "doc_len")]
fn delete_panics_out_of_bounds() {
    use super::*;
    let _ = ChangeSet::delete(2..10, 3);
}

#[test]
fn delete_normalizes_reversed_in_bounds_range() {
    use super::*;
    // reversed but in-bounds: 3..1 on doc_len 5 → deletes [1,3)
    let cs = ChangeSet::delete(3..1, 5);
    assert_eq!(cs.len_before(), 5);
    assert_eq!(cs.len_after(), 3); // deleted 2 bytes
}
```

- [ ] **Step 2: Run, verify they fail**

Run: `cargo test -p wordcartel-core from_ops apply_rejects_buffer insert_panics delete_panics delete_normalizes`
Expected: FAIL — `from_ops`/`len_after` not found; the panic tests don't panic yet (old `insert`/`delete` clamp, old `apply` has no assert).

- [ ] **Step 3: Add `from_ops` + `len_after()`**

In `impl ChangeSet` (change.rs), add:

```rust
    /// Build a ChangeSet from raw ops over a document of length `len_before`.
    /// Computes `len_after` from the ops; release-asserts the consumption invariant
    /// `sum(Retain)+sum(Delete) == len_before`. (UTF-8 op-boundary correctness is NOT
    /// checked here — there is no document; it stays enforced by TextBuffer's asserts in
    /// `apply`.) Trusted-caller constructor; the future plugin path validates upstream.
    pub fn from_ops(ops: Vec<Op>, len_before: usize) -> ChangeSet {
        let (mut retain, mut delete, mut insert) = (0usize, 0usize, 0usize);
        for op in &ops {
            match op {
                Op::Retain(n) => retain += n,
                Op::Delete(n) => delete += n,
                Op::Insert(s) => insert += s.len(),
            }
        }
        assert!(
            retain + delete == len_before,
            "from_ops: retain+delete {} != len_before {}",
            retain + delete, len_before
        );
        ChangeSet { ops, len_before, len_after: retain + insert }
    }

    /// Document length after this changeset applies.
    pub fn len_after(&self) -> usize { self.len_after }
```

- [ ] **Step 4: Harden `insert`/`delete` (clamp → assert) and `apply` (length-assert)**

Replace `insert` (change.rs ~37–49) — drop the `debug_assert`+clamp, use a release `assert!`:

```rust
    pub fn insert(at: BytePos, text: &str, doc_len: usize) -> ChangeSet {
        assert!(at <= doc_len, "insert at {} past doc_len {}", at, doc_len);
        let mut ops = Vec::new();
        if at > 0 { ops.push(Op::Retain(at)); }
        ops.push(Op::Insert(Tendril::from(text)));
        if at < doc_len { ops.push(Op::Retain(doc_len - at)); }
        ChangeSet { ops, len_before: doc_len, len_after: doc_len + text.len() }
    }
```

Replace `delete` (change.rs ~57–79) — normalize a reversed range (interpretation, not corruption), then release-`assert!` out-of-bounds (no clamp):

```rust
    pub fn delete(range: Range<BytePos>, doc_len: usize) -> ChangeSet {
        // Normalize a reversed range (head..anchor); then assert in-bounds.
        let start = range.start.min(range.end);
        let end = range.start.max(range.end);
        assert!(end <= doc_len, "delete end {} past doc_len {}", end, doc_len);
        let del_len = end - start;
        let mut ops = Vec::new();
        if start > 0 { ops.push(Op::Retain(start)); }
        ops.push(Op::Delete(del_len));
        if end < doc_len { ops.push(Op::Retain(doc_len - end)); }
        ChangeSet { ops, len_before: doc_len, len_after: doc_len - del_len }
    }
```

Add the length-assert as the **first statement** of `apply` (change.rs ~83):

```rust
    pub fn apply(&self, buf: &mut TextBuffer) {
        assert!(
            buf.len() == self.len_before,
            "apply: buf.len() {} != len_before {}",
            buf.len(), self.len_before
        );
        let mut pos: BytePos = 0;
        for op in &self.ops {
            // ... unchanged ...
        }
    }
```

- [ ] **Step 5: Convert the two old clamp tests to `#[should_panic]`**

The existing release-only tests at change.rs ~243 and ~256 assert that `insert`/`delete` CLAMP an out-of-range offset. That behavior is gone. Find them (grep the test names around those lines — they assert a clamped result) and convert each to the new contract: an out-of-range offset now panics. Replace each old test body with a `#[should_panic(expected = "doc_len")]` test that calls the same out-of-range `insert`/`delete` and expects the panic. (Do NOT delete the coverage — re-point it to the new fail-fast behavior.)

- [ ] **Step 6: Run, verify pass**

Run: `cargo test -p wordcartel-core`
Expected: PASS — the 6 new tests + the 2 converted clamp tests green; all other core tests (including the change.rs proptests at ~320,387 and the oracle) still green. (Fields are still public, so nothing else broke.)

- [ ] **Step 7: Commit**

```bash
git add wordcartel-core/src/change.rs
git commit -m "feat(m1): ChangeSet::from_ops + len_after + fail-fast insert/delete/apply

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

### Task 2: Privatize `ChangeSet` fields + migrate all sites

Flips the three fields to private (closing the raw-construction hole) and migrates every construction/read site onto the constructors/accessor. Depends on Task 1 (`from_ops`, `len_after()` exist).

**Files:**
- Modify: `wordcartel-core/src/change.rs` (field decls ~16–21; same-module raw-construction tests ~307,430,437)
- Modify: `wordcartel-core/src/selection.rs` (`len_after` read ~135)
- Modify: `wordcartel/src/commands.rs` (`build_range_replace` ~125; `build_multi_replace` ~157)

**Interfaces:**
- Consumes: `ChangeSet::from_ops`, `ChangeSet::len_after()` (Task 1).
- Produces: `ChangeSet` with private `ops`/`len_before`/`len_after`; raw construction no longer possible outside `change.rs`'s own constructors.

- [ ] **Step 1: Privatize the fields**

In `change.rs` (~16–21), remove `pub` from the three fields:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSet {
    ops: Vec<Op>,
    len_before: usize,
    len_after: usize,
}
```

- [ ] **Step 2: Run the build to enumerate every break**

Run: `cargo build -p wordcartel-core && cargo build -p wordcartel`
Expected: COMPILE ERRORS at every external construction/read site. The compiler is the worklist. Expected sites (per the spec's Codex grep): `wordcartel/src/commands.rs:125,157` (raw construct), `wordcartel-core/src/selection.rs:135` (reads `len_after`), and same-module test constructions in `change.rs` (~307,430,437). Fix exactly the sites the compiler reports.

- [ ] **Step 3: Migrate `build_range_replace` and `build_multi_replace`**

In `wordcartel/src/commands.rs`, replace the raw `ChangeSet { ops, len_before: doc_len, len_after }` literals (commands.rs:125 in `build_range_replace`, commands.rs:157 in `build_multi_replace`) with `ChangeSet::from_ops(ops, doc_len)`. `from_ops` recomputes `len_after`, so **delete the now-dead local `len_after` computation** in each function (in `build_multi_replace`, the `len_after` accumulator becomes unused — remove it; keep the `Edit`/`new_len` computation, which is separate). Read both functions in full first; preserve everything except the changeset construction.

- [ ] **Step 4: Migrate the `len_after` read in selection.rs**

At `selection.rs:135`, change the direct field read `cs.len_after` (or `.len_after`) to the accessor `cs.len_after()`. (Different module → must use the accessor.) Read the surrounding function to apply it correctly.

- [ ] **Step 5: Migrate same-module raw-construction tests**

In `change.rs` tests (the sites the compiler flags around ~307,430,437), replace raw `ChangeSet { ops, len_before, len_after }` with `ChangeSet::from_ops(ops, len_before)` (or `insert`/`delete` where that's what the test means). If a test deliberately built an *inconsistent* changeset to exercise some path, re-express it through `from_ops` (which will assert) or drop that specific construction — note it in the report.

- [ ] **Step 6: Add `ops()`/`len_before()` accessors ONLY if the compiler still demands them**

If Step 2 flagged any external reader of `ops` or `len_before` beyond what Steps 3–5 fixed, add `pub fn ops(&self) -> &[Op] { &self.ops }` and/or `pub fn len_before(&self) -> usize { self.len_before }` and use them. If no such reader exists, add nothing (YAGNI).

- [ ] **Step 7: Run both suites**

Run: `cargo build -p wordcartel-core && cargo build -p wordcartel && cargo test -p wordcartel-core && cargo test -p wordcartel`
Expected: clean build, all green. This is T6 complete: raw construction is now impossible outside `change.rs`, and `build_multi_replace`/`build_range_replace` produce the same `(ChangeSet, Edit)` as before (regression covered by `build_range_replace_yields_changeset_and_matching_edit` at commands.rs:1261 and the existing apply/undo tests).

- [ ] **Step 8: Commit**

```bash
git add wordcartel-core/src/change.rs wordcartel-core/src/selection.rs wordcartel/src/commands.rs
git commit -m "feat(m1): privatize ChangeSet fields; migrate builders to from_ops

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

### Task 3: Privatize `Selection` fields + migrate raw sites

Closes the `Selection { ranges, primary }` hole (the `primary >= ranges.len()` / empty-ranges release panic). Independent of Tasks 1–2.

**Files:**
- Modify: `wordcartel-core/src/selection.rs` (field decls ~13–16; tests)
- Modify: `wordcartel/src/commands.rs` (raw `Selection` sites: live ~398; tests ~952,1112,1142,1163,1184,1247)

**Interfaces:**
- Produces: `Selection` with private `ranges`/`primary`; constructed only via `single`/`range`. `Range.{anchor,head}` stay public.

- [ ] **Step 1: Write the T5 invariant tests**

In `selection.rs` tests, add:

```rust
#[test]
fn constructors_keep_primary_in_bounds_and_nonempty() {
    use super::*;
    for s in [Selection::single(7), Selection::range(2, 5)] {
        assert!(!s.is_empty_ranges(), "ranges non-empty"); // see Step 3 helper note
        // primary() must never panic and must return the sole range:
        let p = s.primary();
        let _ = (p.from(), p.to());
    }
}
```

NOTE: if `Selection` has no `is_empty_ranges`/`len` accessor, assert non-emptiness via `s.primary()` not panicking plus a `ranges` length check available in-module (the test is in `selection.rs`, same module → it can read the private `ranges` field directly: `assert!(!s.ranges.is_empty()); assert!(s.primary < s.ranges.len());`). Prefer the direct private-field assertion since the test is same-module.

- [ ] **Step 2: Run, verify it fails or is meaningless until privatized**

Run: `cargo test -p wordcartel-core constructors_keep_primary`
Expected: compiles and PASSES against current code (the invariant already holds for `single`/`range`). This test is the *guard* that privatization preserves — it should stay green through the task. (If you wrote it using private fields, it already works; the value is that it pins the invariant.)

- [ ] **Step 3: Privatize the fields**

In `selection.rs` (~13–16):

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    ranges: SmallVec<[Range; 1]>,
    primary: usize,
}
```

(`Range` is unchanged — `anchor`/`head` stay `pub`.)

- [ ] **Step 4: Build to enumerate breaks; migrate raw `Selection` sites**

Run: `cargo build -p wordcartel-core && cargo build -p wordcartel`
Expected: compile errors at raw `Selection { ranges, primary }` sites — the live site at `commands.rs:398` and the test sites at commands.rs:952,1112,1142,1163,1184,1247 (per the spec's Codex grep), plus any the compiler flags. Replace each with `Selection::range(anchor, head)` (they are all single-range, `primary: 0`). Read each site to pass the right `anchor`/`head`.

- [ ] **Step 5: Run both suites**

Run: `cargo build -p wordcartel-core && cargo build -p wordcartel && cargo test -p wordcartel-core && cargo test -p wordcartel`
Expected: clean build, all green. Raw `Selection` construction is now impossible outside `selection.rs`; `primary()`'s `debug_assert` can no longer be reached with a bad invariant.

- [ ] **Step 6: Commit**

```bash
git add wordcartel-core/src/selection.rs wordcartel/src/commands.rs
git commit -m "feat(m1): privatize Selection ranges/primary; migrate raw sites to range()

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

## Self-Review

**Spec coverage:**
- Privatize `ChangeSet.{ops,len_before,len_after}` → Task 2. ✓
- `from_ops` (validating) + `len_after()` accessor → Task 1. ✓
- `insert`/`delete` clamp → release-assert (decision 4) + 2 clamp tests → should_panic → Task 1. ✓
- `apply` length-assert → Task 1. ✓
- Privatize `Selection.{ranges,primary}`; keep `Range` public; no `from_ranges`/`ranges()` → Task 3. ✓
- Migrate `build_multi_replace`/`build_range_replace` → `from_ops` → Task 2. ✓
- Migrate raw `Selection` sites → `range` → Task 3. ✓
- T5 (Selection invariants) → Task 3; T6 (ChangeSet validity) → Tasks 1–2. ✓
- Out of scope (Result boundary, op-boundary preflight) → not built. ✓

**Type consistency:** `from_ops(ops: Vec<Op>, len_before: usize) -> ChangeSet`, `len_after(&self) -> usize`, `apply` asserts on `self.len_before`, `Selection::range(anchor, head)` — used consistently across tasks.

**Placeholder scan:** no TBD/TODO; the only compiler-driven items (which exact accessor/sites) are intentional — privatization breaks are enumerated by `cargo build` (Task 2 Step 2, Task 3 Step 4), which is the correct mechanical worklist for a field-privatization migration.
