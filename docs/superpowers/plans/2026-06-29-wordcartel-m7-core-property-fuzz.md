# M7 — Core Property Tests + Fuzz Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Property-test the core's data-loss-critical operations (T1–T4) and stand up a coverage-guided cargo-fuzz crate (F0/F1/F2) for the apply pipeline + block_tree — proving no-panic + model-equivalence. The last minimal-viable hardening pillar before Effort P.

**Architecture:** A proptest-free `#[cfg(any(test, fuzzing))]` `test_support` module holds the shared `String` byte-mirror model + edit vocabulary. T1–T4 are proptests (in `cargo test`, 2048 cases) building on it. F0/F1/F2 are a separate `wordcartel-core/fuzz/` cargo-fuzz crate (manual campaign) that imports `test_support` under `--cfg fuzzing`.

**Tech Stack:** Rust. `proptest` (existing dev-dep); `cargo-fuzz 0.13.2` + nightly + `libfuzzer-sys`/`arbitrary` (the new fuzz crate). Both installed.

**Spec:** `docs/superpowers/specs/2026-06-29-wordcartel-m7-core-property-fuzz-design.md` (Codex-GO'd ×2).

## Global Constraints

- **Reference model = a `String` byte-mirror** of `TextBuffer` (`model.replace_range(at..at+del, ins)`). NOT Vec<char>.
- **`test_support` is `#[cfg(any(test, fuzzing))]` and proptest-FREE** — `proptest` is a dev-dep, absent under `cfg(fuzzing)`. proptest STRATEGIES stay `#[cfg(test)]` in the test modules.
- **`snap` already exists** as a local test helper in `change.rs` — consolidate it into `test_support` (single canonical copy) rather than duplicating. Likewise, EXTEND existing proptests (e.g. `prop_apply_then_invert_is_identity`, change.rs:544) — do not duplicate them.
- **`apply_edit` stays unconditionally `pub`** (an integration test `tests/block_tree_oracle.rs` imports it). Only `incremental_equals_full` is added (`cfg(any(test, fuzzing))`).
- **The fuzz crate is EXCLUDED from the main workspace** (cargo-fuzz does this) so `cargo test -p wordcartel-core -p wordcartel` is unaffected. `cfg(fuzzing)` is false in normal builds, so `test_support`/`incremental_equals_full` are compiled only under `cfg(test)` (normal `cargo build` excludes them — no dead-code).
- **Gates (T1–T4 + lift):** `cargo test -p wordcartel-core` green; `cargo build -p wordcartel-core` + `cargo test --no-run` warning-free; no new clippy on touched lines. **Do NOT run `cargo fmt`** (house style: em-dashes, dense). **Fuzz gate (Task 5):** `cargo +nightly fuzz build` clean + a BOUNDED `cargo +nightly fuzz run <target> -- -max_total_time=30 -max_len=4096 -rss_limit_mb=2048` per target with zero crashes.
- A **property failure** (shrunk counterexample) or a **fuzz crash** is a finding: if a real core bug → fix it + pin the minimized input as a normal-suite regression test; if the property/generator over-claims → fix the test. Never weaken a property to hide a real bug.
- Commit trailers (append to every commit):
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
  `Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6`

## File Structure

- **Create** `wordcartel-core/src/test_support.rs` — model + `EditOp` + `snap` + `UNICODE_PALETTE` (cfg-gated, proptest-free).
- **Modify** `wordcartel-core/src/lib.rs` — `#[cfg(any(test, fuzzing))] pub mod test_support;`.
- **Modify** `wordcartel-core/src/buffer.rs` (T1), `change.rs` (T2+T3), `history.rs` (T4) — proptests + strategies (`cfg(test)`).
- **Modify** `wordcartel-core/src/block_tree.rs` — `incremental_equals_full` (cfg-gated) + re-express the unit oracle.
- **Create** `wordcartel-core/fuzz/` — the cargo-fuzz crate (F0) with `apply_pipeline` (F1) + `block_tree` (F2) targets.

---

### Task 1: `test_support` module + T1 (TextBuffer model oracle)

**Files:**
- Create: `wordcartel-core/src/test_support.rs`
- Modify: `wordcartel-core/src/lib.rs` (`pub mod test_support;` cfg-gated), `wordcartel-core/src/buffer.rs` (T1 proptests + strategies), `wordcartel-core/src/change.rs` (point its local `snap` at `test_support::snap`)
- Test: buffer.rs `#[cfg(test)] mod`

**Interfaces:**
- Produces: `test_support::{model_apply(&mut String, usize, usize, &str), EditOp{at,del,ins}, snap(&str, usize) -> usize, UNICODE_PALETTE: &[&str]}`.

- [ ] **Step 1: Create `test_support.rs`**

```rust
//! Shared reference model + edit vocabulary for the M7 property tests AND fuzz targets.
//! NO proptest here — it is a dev-dependency, absent under cfg(fuzzing).

/// A byte-faithful reference for TextBuffer: the same byte splice a ChangeSet performs.
pub fn model_apply(model: &mut String, at: usize, del: usize, ins: &str) {
    model.replace_range(at..at + del, ins);
}

/// One generated edit. Positions are BYTE offsets; callers snap to boundaries before applying.
#[derive(Clone, Debug)]
pub struct EditOp {
    pub at: usize,
    pub del: usize,
    pub ins: String,
}

/// Snap a byte offset DOWN to the nearest char boundary of `s` (and clamp into `0..=s.len()`).
pub fn snap(s: &str, off: usize) -> usize {
    let off = off.min(s.len());
    (0..=off).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0)
}

/// Unicode-biased palette for generated strings (ASCII + multibyte + combining + ZWJ + emoji).
pub const UNICODE_PALETTE: &[&str] = &["a", "Z", " ", "\n", "é", "中", "🙂", "\u{0301}", "\u{200d}"];
```

- [ ] **Step 2: Wire the module**

`wordcartel-core/src/lib.rs`:
```rust
#[cfg(any(test, fuzzing))]
pub mod test_support;
```

- [ ] **Step 3: Consolidate the existing `snap`**

`change.rs` defines a local `fn snap` used by its proptests. Replace its definition with a re-use:
delete the local `fn snap` and change its call sites to `crate::test_support::snap(...)` (same
signature/semantics). Run `cargo test -p wordcartel-core --lib change` to confirm the existing
change.rs proptests still pass against the consolidated `snap`.

- [ ] **Step 4: T1 strategies + proptests (buffer.rs)**

Add `#[cfg(test)]` strategies (in buffer.rs's test mod or a shared `cfg(test)` helper):
```rust
use proptest::prelude::*;
use crate::test_support::{UNICODE_PALETTE, model_apply, snap};

/// A unicode-biased string: a sequence of palette pieces.
fn prop_unicode_string() -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::sample::select(UNICODE_PALETTE), 0..40)
        .prop_map(|parts| parts.concat())
}
```
The model-oracle property (insert/delete/slice in lockstep with the `String` model):
```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn t1_textbuffer_matches_string_model(
        text in prop_unicode_string(),
        // a sequence of (op-choice, position, del-len, insert) tuples
        ops in proptest::collection::vec(
            (0u8..3, 0usize..60, 0usize..20, prop_unicode_string()), 0..12),
    ) {
        let mut buf = TextBuffer::from_str(&text);
        let mut model = text.clone();
        for (which, p, dl, ins) in ops {
            let at = snap(&model, p.min(model.len()));
            match which {
                0 => { buf.insert(at, &ins); model_apply(&mut model, at, 0, &ins); }
                1 => {
                    let end = snap(&model, (at + dl).min(model.len()));
                    buf.delete(at..end); model_apply(&mut model, at, end - at, "");
                }
                _ => {
                    let end = snap(&model, (at + dl).min(model.len()));
                    prop_assert_eq!(buf.slice(at..end), model[at..end].to_string());
                }
            }
            prop_assert_eq!(buf.len(), model.len());
            prop_assert_eq!(buf.slice(0..buf.len()), model.clone());
        }
    }
}
```
Char-boundary rejection (TextBuffer release-asserts; there is NO `try_*`/`Result` path):
```rust
#[test]
fn t1_insert_at_non_char_boundary_panics_no_corruption() {
    // "é" is 2 bytes; offset 1 is mid-char.
    let panicked = std::panic::catch_unwind(|| {
        let mut buf = TextBuffer::from_str("é");
        buf.insert(1, "x"); // off-boundary → release-assert
    });
    assert!(panicked.is_err(), "off-boundary insert must be refused (panic), never UB");
}
```
(Add the delete + slice off-boundary analogues. Wrap the buffer in the closure so a poisoned
state can't leak — `catch_unwind` over an owned buffer.)

- [ ] **Step 5: Run + gates + commit**

`cargo test -p wordcartel-core --lib buffer change` green; warning-free build/test-compile; clippy clean on new lines.
```bash
git add wordcartel-core/src/test_support.rs wordcartel-core/src/lib.rs wordcartel-core/src/buffer.rs wordcartel-core/src/change.rs
git commit -m "test(m7): test_support model module + T1 TextBuffer model oracle"
```

---

### Task 2: T2 (apply==splice + invert) + T3 (map_pos) — change.rs proptests

**Files:**
- Modify: `wordcartel-core/src/change.rs` (extend/add proptests)
- Test: change.rs `#[cfg(test)] mod`

**Interfaces:**
- Consumes: `test_support::{model_apply, snap, UNICODE_PALETTE}`; `prop_unicode_string` (lift it to a shared `cfg(test)` location or re-declare in change.rs's test mod).

- [ ] **Step 1: T2 — apply == naive String splice (failing-or-passing property)**

EXTEND change.rs proptests (the existing `prop_apply_then_invert_is_identity`, change.rs:544,
already covers invert round-trip with `.{0,40}` — keep it; ADD the apply==splice + unicode +
edge cases). New property:
```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn t2_apply_equals_string_splice(
        text in prop_unicode_string(),
        p in 0usize..60, dl in 0usize..20, ins in prop_unicode_string(),
    ) {
        let len = text.len();
        let at = snap(&text, p.min(len));
        let end = snap(&text, (at + dl).min(len));
        // build the real changeset (insert when del==0, else a delete+insert via from_ops or two ops)
        let cs = if end == at {
            ChangeSet::insert(at, &ins, len)
        } else {
            // delete [at,end) then insert — use the real constructors / from_ops
            // (a delete followed by an insert at the same point); see change.rs for the
            // exact multi-op build (Retain(at), Delete(end-at), Insert(ins), Retain(len-end)).
            ChangeSet::from_ops(/* ops as above */ , len)
        };
        let original = TextBuffer::from_str(&text);
        let inv = cs.invert(&original);                 // INVERT FROM ORIGINAL, before apply
        let mut buf = original.clone();                 // (TextBuffer: Clone? else from_str(&text))
        cs.apply(&mut buf);
        let mut model = text.clone();
        model_apply(&mut model, at, end - at, &ins);
        prop_assert_eq!(buf.slice(0..buf.len()), model);     // apply == naive splice
        inv.apply(&mut buf);
        prop_assert_eq!(buf.slice(0..buf.len()), text);      // invert round-trips
    }
}
```
Cover `doc_len == 0` (empty text), edits at `0` and at `len` (the strategy's ranges already reach
these; add explicit unit cases if a boundary needs pinning). NOTE the **invert ordering**:
`cs.invert(&original)` is computed BEFORE `cs.apply` (invert needs the original's deleted bytes).

- [ ] **Step 2: T3 — map_pos on-boundary + monotonic + bias**

Positions are drawn from the OLD doc's char boundaries (map_pos is pure byte arithmetic):
```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn t3_map_pos_boundary_monotonic_bias(
        text in prop_unicode_string(),
        at in 0usize..60, ins in prop_unicode_string(),
        // two query positions as *fractions* into the doc, snapped to boundaries
        q1 in 0usize..60, q2 in 0usize..60,
    ) {
        let len = text.len();
        let at = snap(&text, at.min(len));
        let cs = ChangeSet::insert(at, &ins, len);
        let new_text = { let mut m = text.clone(); model_apply(&mut m, at, 0, &ins); m };
        let (p1, p2) = (snap(&text, q1.min(len)).min(snap(&text, q2.min(len))),
                        snap(&text, q1.min(len)).max(snap(&text, q2.min(len))));
        // on-boundary output
        prop_assert!(new_text.is_char_boundary(map_pos(p1, &cs)));
        prop_assert!(new_text.is_char_boundary(map_pos_before(p1, &cs)));
        // monotonic
        prop_assert!(map_pos(p1, &cs) <= map_pos(p2, &cs));
        prop_assert!(map_pos_before(p1, &cs) <= map_pos_before(p2, &cs));
        // bias at the insertion point: before stays left, after lands right
        prop_assert!(map_pos_before(at, &cs) <= map_pos(at, &cs));
    }
}
```
(Pin the exact bias against the impl: `map_pos(at)` should equal `at + ins.len()` and
`map_pos_before(at)` should equal `at` for a pure insert at `at` — see change.rs unit test at
~:426 which asserts `map_pos(2)==4`, `map_pos_before(2)==2` for inserting "XY" at byte 2.)

- [ ] **Step 3: Run + gates + commit**

`cargo test -p wordcartel-core --lib change` green (existing + new). Then:
```bash
git add wordcartel-core/src/change.rs
git commit -m "test(m7): T2 apply==splice+invert and T3 map_pos boundary/monotonic/bias"
```

---

### Task 3: T4 (History undo/redo) — history.rs proptests

**Files:**
- Modify: `wordcartel-core/src/history.rs` (proptests)
- Test: history.rs `#[cfg(test)] mod`

**Interfaces:**
- Consumes: `History::{commit, undo, redo}`, `Transaction::new`, `Selection::{single, primary}`, `test_support::{snap, model_apply, UNICODE_PALETTE}`.

- [ ] **Step 1: T4 properties**

```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn t4_undo_redo_round_trips_exact(
        text in prop_unicode_string(),
        edits in proptest::collection::vec((0usize..60, prop_unicode_string()), 1..8),
    ) {
        let mut buf = TextBuffer::from_str(&text);
        let mut hist = History::default();
        let mut sel = Selection::single(0);
        // apply a sequence of insert commits, snapshotting state before each undo test
        let pre_states: Vec<String> = Vec::new(); let _ = &pre_states;
        for (p, ins) in &edits {
            let at = snap(&buf.slice(0..buf.len()), (*p).min(buf.len()));
            let cs = ChangeSet::insert(at, ins, buf.len());
            sel = hist.commit(Transaction::new(cs), &mut buf, sel.clone());
            // selection valid: in-bounds + on char boundary
            prop_assert!(sel.primary().head <= buf.len());
            prop_assert!(buf.slice(0..buf.len()).is_char_boundary(sel.primary().head));
        }
        let after_all = buf.slice(0..buf.len());
        // undo then redo returns the exact state
        if hist.undo(&mut buf).is_some() {
            hist.redo(&mut buf);
            prop_assert_eq!(buf.slice(0..buf.len()), after_all);
        }
        // full undo yields the original
        while hist.undo(&mut buf).is_some() {}
        prop_assert_eq!(buf.slice(0..buf.len()), text);
    }
}
```
Add a **coalescing-loses-nothing** property using `commit_coalescing` over a run of `Type` edits
with a `FakeClock` (mirror the existing history.rs tests' clock): a coalesced run, fully undone,
yields the exact original doc. (Do NOT assert "version increases" — version is shell-scoped, not
on core `History`; spec out-of-scope.)

- [ ] **Step 2: Run + gates + commit**

`cargo test -p wordcartel-core --lib history` green. Then:
```bash
git add wordcartel-core/src/history.rs
git commit -m "test(m7): T4 History undo/redo round-trip + coalescing-loses-nothing"
```

---

### Task 4: F2 core oracle lift (`incremental_equals_full`)

**Files:**
- Modify: `wordcartel-core/src/block_tree.rs` (add the helper; re-express the unit oracle test)
- Test: block_tree.rs `#[cfg(test)] mod`

**Interfaces:**
- Consumes: `apply_edit` (already `pub`), `full_parse`, `incremental_update`, `BlockTree: PartialEq`.
- Produces: `#[cfg(any(test, fuzzing))] pub fn incremental_equals_full(&str, Range<usize>, &str) -> bool`.

- [ ] **Step 1: Add the helper**

```rust
/// Property oracle (M7 F2): an incremental block-tree update over `[range)`→`repl` must yield the
/// SAME tree as a full reparse of the resulting text. `cfg(any(test, fuzzing))` so the fuzz crate
/// (built with --cfg fuzzing) can call it; the cfg(test) unit oracle uses it too.
#[cfg(any(test, fuzzing))]
pub fn incremental_equals_full(old: &str, range: std::ops::Range<usize>, repl: &str) -> bool {
    let (new, edit) = apply_edit(old, range, repl);
    incremental_update(&full_parse(old), old, &edit, &new) == full_parse(&new)
}
```
(Do NOT touch `apply_edit` — it stays unconditionally `pub`; the integration test imports it.)

- [ ] **Step 2: Re-express the existing UNIT oracle test via the helper**

The `#[cfg(test)]` test `rope_incremental_matches_full_and_str` (block_tree.rs ~1380): keep its
rope-vs-str check, and replace its str-vs-full `assert_eq!(str_tree, full_parse(&new))` with
`assert!(incremental_equals_full(old, 9..9, "X"))`. (The integration test `tests/block_tree_oracle.rs`
is untouched.)

- [ ] **Step 3: Run + gates + commit**

`cargo test -p wordcartel-core --lib block_tree` green; `cargo build -p wordcartel-core` warning-free
(the cfg-gated helper is compiled only under test/fuzzing — confirm no unused warning under plain build).
```bash
git add wordcartel-core/src/block_tree.rs
git commit -m "test(m7): lift incremental_equals_full oracle into core (cfg test/fuzzing)"
```

---

### Task 5: Fuzz crate — F0 scaffold + F1 (apply pipeline) + F2 (block_tree)

**Files:**
- Create: `wordcartel-core/fuzz/Cargo.toml`, `wordcartel-core/fuzz/fuzz_targets/apply_pipeline.rs`, `wordcartel-core/fuzz/fuzz_targets/block_tree.rs`, `wordcartel-core/fuzz/corpus/*` (seeds)
- Test: bounded `cargo +nightly fuzz run` per target

**Interfaces:**
- Consumes (under `--cfg fuzzing`): `wordcartel_core::test_support::{model_apply, snap}`, `TextBuffer`, `ChangeSet`, `incremental_equals_full`.

REQUIREMENT: this task needs **nightly + cargo-fuzz** (both installed: cargo-fuzz 0.13.2,
nightly-x86_64). If the environment cannot run `cargo +nightly fuzz`, report BLOCKED.

- [ ] **Step 1: Scaffold the fuzz crate (F0)**

From `wordcartel-core/`: `cargo +nightly fuzz init` (creates `fuzz/` excluded from the main
workspace). `fuzz/Cargo.toml` deps: `libfuzzer-sys`, `arbitrary = { version = "1", features = ["derive"] }`,
`wordcartel-core = { path = ".." }`. Confirm `cargo test -p wordcartel-core -p wordcartel` from the
repo root is UNAFFECTED (the fuzz crate is not in the main workspace).

- [ ] **Step 2: F1 — apply_pipeline target**

`fuzz/fuzz_targets/apply_pipeline.rs`:
```rust
#![no_main]
use libfuzzer_sys::fuzz_target;
use arbitrary::Arbitrary;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::change::ChangeSet;
use wordcartel_core::test_support::{model_apply, snap};

#[derive(Arbitrary, Debug)]
struct FuzzInput { doc: String, ops: Vec<FuzzOp> }
#[derive(Arbitrary, Debug)]
struct FuzzOp { at: usize, del: usize, ins: String }

fuzz_target!(|input: FuzzInput| {
    let mut buf = TextBuffer::from_str(&input.doc);
    let mut model = input.doc.clone();
    for op in input.ops {
        let len = model.len();
        let at = snap(&model, op.at % (len + 1));
        let end = snap(&model, (at + (op.del % (len - at + 1))).min(len));
        let cs = if end == at { ChangeSet::insert(at, &op.ins, len) }
                 else { /* delete [at,end) — build via the real constructor / from_ops */ ChangeSet::delete(at..end, len) };
        // (a combined delete+insert can be two sequential ops; keep it simple: delete then insert)
        cs.apply(&mut buf);
        model_apply(&mut model, at, end - at, if end == at { &op.ins } else { "" });
        assert_eq!(buf.slice(0..buf.len()), model, "apply pipeline diverged from the model");
    }
});
```
(Adjust the op semantics to mirror the model EXACTLY — if a single FuzzOp does delete-then-insert,
do both in the model too. The point: every applied edit keeps `buf == model`, and nothing panics
on a snapped/clamped — i.e. VALID — edit.)

- [ ] **Step 3: F2 — block_tree target**

`fuzz/fuzz_targets/block_tree.rs`:
```rust
#![no_main]
use libfuzzer_sys::fuzz_target;
use wordcartel_core::test_support::snap;
use wordcartel_core::block_tree::incremental_equals_full;

fuzz_target!(|input: (String, usize, usize, String)| {
    let (old, s, e, repl) = input;
    let start = snap(&old, s % (old.len() + 1));
    let end = snap(&old, (start + (e % (old.len() - start + 1))).min(old.len())); // snap BOTH endpoints
    assert!(incremental_equals_full(&old, start..end, &repl),
            "incremental block-tree update diverged from a full reparse");
});
```

- [ ] **Step 4: Seed corpus + bounded smoke campaign**

Seed `fuzz/corpus/apply_pipeline/` and `fuzz/corpus/block_tree/` with a few small files (palette
strings + a couple of real markdown docs: headings, list, fenced code, table, ref-def). Then run a
BOUNDED campaign per target and confirm zero crashes:
```bash
cargo +nightly fuzz run apply_pipeline -- -max_total_time=30 -max_len=4096 -rss_limit_mb=2048
cargo +nightly fuzz run block_tree     -- -max_total_time=30 -max_len=4096 -rss_limit_mb=2048
```
If a target crashes: `cargo +nightly fuzz tmin <target> <crash-file>` to minimize, then add the
minimized input as a **pinned `#[cfg(test)]` regression test** in the relevant core module
(buffer/change or block_tree) and FIX the underlying bug before proceeding. Report the exec counts
+ "0 crashes" (or the crash + fix).

- [ ] **Step 5: Gates + commit**

`cargo test -p wordcartel-core -p wordcartel` (main workspace) still green + unaffected;
`cargo +nightly fuzz build` clean; both bounded runs report 0 crashes.
```bash
git add wordcartel-core/fuzz
git commit -m "test(m7): cargo-fuzz crate — F1 apply pipeline + F2 block_tree incremental≡full"
```

---

## Self-Review

**Spec coverage:** test_support model module (Task 1) ✔; T1 TextBuffer oracle + rejection (Task 1) ✔; T2 apply==splice+invert (Task 2) ✔; T3 map_pos (Task 2) ✔; T4 undo/redo+coalescing, version-monotonic correctly DROPPED (Task 3) ✔; F2 core oracle lift, apply_edit left alone (Task 4) ✔; F0 scaffold + F1 + F2 targets with fuzz-local DTO + bounded campaign (Task 5) ✔. Out-of-scope (F3–F6, C2b, C3, standing CI) untouched ✔.

**Type consistency:** `test_support::{model_apply, EditOp, snap, UNICODE_PALETTE}` referenced consistently; `incremental_equals_full(&str, Range, &str) -> bool` defined (Task 4) before its fuzz use (Task 5); the fuzz DTO is fuzz-local (orphan-rule-clean); invert computed from the original before apply (T2); map_pos positions from char boundaries (T3).

**Placeholder scan:** the multi-op ChangeSet build in T2/F1 (delete+insert) says "build via the real constructor / from_ops" — the implementer must mirror the model splice EXACTLY (the dominant correctness point); `TextBuffer: Clone?` is flagged (use `from_str(&text)` if not Clone). The exact map_pos bias is pinned against the existing unit test. Everything else is concrete.

**Ordering:** Task 1 (test_support) first — Tasks 2/3/5 import it. Task 4 (the lift) before Task 5 (the fuzz target uses it). Each task leaves the main workspace compiling + green; the fuzz crate (Task 5) is separate + bounded.

## Execution Handoff

Two execution options:
1. **Subagent-Driven (recommended)** — fresh subagent per task, two-stage review between tasks.
2. **Inline Execution** — batch with checkpoints.

Which approach?
