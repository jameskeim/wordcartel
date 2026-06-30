# M7 — Core Property Tests + Fuzz — Design

**Status:** Approved (brainstorm complete)
**Date:** 2026-06-29
**Parent:** Hardening campaign workstream **M7** (the minimal T1–T4 + F1/F2 subset)
(`docs/superpowers/plans/2026-06-28-wordcartel-hardening-fuzz-proptest-plan.md`).
**Crate:** `wordcartel-core` (pure functional core) + a new `wordcartel-core/fuzz/` cargo-fuzz crate.

## Goal

Property-test the core's data-loss-critical operations and stand up a coverage-guided fuzz
crate for the apply pipeline + block_tree, proving **no-panic** (per D0/M1: invalid input is a
typed/asserted refusal, never UB) and **model-equivalence** (the buffer behaves like a simple
reference model). This is the last minimal-viable hardening pillar before Effort P. M5's
resource caps make the fuzz campaign safe (bounded memory/time); M1/M2 made the edit primitives
valid-by-construction, which these tests now exercise at scale.

## Background

- `proptest = "1"` is already a `wordcartel-core` dev-dep (T5/T6 from M1 use it). cargo-fuzz
  0.13.2 + a nightly toolchain are both installed.
- Target APIs (all in `wordcartel-core`): `TextBuffer::{len, insert(at,&str), delete(Range),
  slice(Range), from_str}` (buffer.rs); `ChangeSet::{insert(at,&str,doc_len),
  delete(Range,doc_len), from_ops(ops,len_before), apply(&mut buf), invert(&buf), len_before,
  len_after}` + free `map_pos(pos,&cs)` / `map_pos_before(pos,&cs)` (change.rs); `History::{commit,
  commit_coalescing, undo, redo}` (history.rs); `block_tree::{full_parse(&str)->BlockTree,
  incremental_update(&old_tree, old_text, &Edit, new_text)->BlockTree}` (`BlockTree: PartialEq`)
  and the test helper `apply_edit(old, Range, repl) -> (String, Edit)` (block_tree.rs).
- `BytePos = usize`. All edit positions are **byte** offsets.

## Decisions (from brainstorm)

1. **cargo-fuzz crate for F1/F2 + proptests for T1–T4** (coverage-guided fuzzing for the #1
   data-loss paths; deterministic in-suite proptests for the rest). The fuzz crate runs as a
   **manual `cargo fuzz run` campaign**, NOT in normal `cargo test`; the always-on protection is
   the T1–T4 proptests. A fuzz crash → minimize → **pin as a normal-suite regression test**.
2. **Reference model = a `String` byte-mirror of `TextBuffer`** (NOT the plan's literal
   "Vec<char>"). `ChangeSet` ops are byte Retain/Delete/Insert, so a `String` byte-splice is the
   faithful, conversion-free oracle. Char-boundary REJECTION is tested separately (T1), not via
   the model.
3. **Lift the block_tree incremental-vs-full check into core** behind `cfg(any(test, fuzzing))`
   so the fuzz crate can call it (today it lives inline in a `#[test]`).

## Components

### 1. Shared test/fuzz support: model + generators

**CRITICAL split (compile correctness):** `proptest` is a `dev-dependency` — it is NOT available
under `cfg(fuzzing)` (the fuzz crate builds core as a normal dependency with `--cfg fuzzing`, no
dev-deps). So the shared module must contain **NO proptest** — only the proptest-free model +
vocabulary. The proptest *strategies* (which use `proptest::`) stay `#[cfg(test)]`.

Shared module `wordcartel-core/src/test_support.rs`, **`#[cfg(any(test, fuzzing))]`**, proptest-free,
public so the fuzz crate can import it:
```rust
//! Shared reference model + edit vocabulary for the M7 property tests AND fuzz targets.
//! NO proptest here (it is a dev-dep, absent under cfg(fuzzing)).

/// A byte-faithful reference for TextBuffer: the same byte splice a ChangeSet performs.
pub fn model_apply(model: &mut String, at: usize, del: usize, ins: &str) {
    model.replace_range(at..at + del, ins);
}

/// One generated edit. Positions are BYTE offsets; callers snap to boundaries before applying.
#[derive(Clone, Debug)]
pub struct EditOp { pub at: usize, pub del: usize, pub ins: String }

/// Snap a byte offset DOWN to the nearest char boundary of `s` (valid-edit generators use this).
pub fn snap(s: &str, off: usize) -> usize {
    let off = off.min(s.len());
    (0..=off).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0)
}

/// Unicode-biased palette for generated strings (ASCII + multibyte + combining + ZWJ + emoji).
pub const UNICODE_PALETTE: &[&str] = &["a", "Z", " ", "\n", "é", "中", "🙂", "\u{0301}", "\u{200d}"];
```

- **Proptest strategies** — `#[cfg(test)]` only (they use `proptest::Strategy`), in the test
  modules: a `prop_unicode_string()` drawing from `UNICODE_PALETTE`, and a `prop_edit(doc)` that
  snaps `at`/`at+del` to char boundaries (valid-edit properties) — plus an unsnapped variant for
  T1's rejection. They build on `test_support` for the model + palette.
- **Fuzz `arbitrary`** — the fuzz crate CANNOT impl the foreign trait `arbitrary::Arbitrary` for
  core's foreign `EditOp` (orphan rule). So the fuzz crate defines its OWN local input DTO and
  derives `Arbitrary` on it, then converts to the shared model:
  ```rust
  #[derive(arbitrary::Arbitrary, Debug)]
  struct FuzzInput { doc: String, ops: Vec<FuzzOp> }
  #[derive(arbitrary::Arbitrary, Debug)]
  struct FuzzOp { at: usize, del: usize, ins: String }
  ```
  The target maps each `FuzzOp` to a real edit using `wordcartel_core::test_support::{snap, model_apply}`
  (clamp+snap `at`/`del` to char boundaries of the live doc, build a `ChangeSet`). Both halves share
  `test_support`'s `model_apply`/`snap` so the proptest and fuzz oracles are identical — `EditOp`
  itself needs no `Arbitrary` impl.

### 2. T1 — TextBuffer model oracle (buffer.rs proptests)

A sequence of insert/delete/slice ops applied in lockstep to a `TextBuffer` and a `String` model:
- After each op, `buf.len() == model.len()` and `buf.slice(0..buf.len()) == model`.
- `slice(r)` over an in-bounds, boundary-aligned `r` equals `model[r]`.
- **Char-boundary rejection:** `TextBuffer::insert`/`delete`/`slice` guard char boundaries with
  release `assert!` and return `()`/`String` — there is NO `try_*`/`Result` path on `TextBuffer`
  (the typed-refusal path is `ChangeSet::validate_against`, M2, a different layer). So assert the
  off-boundary edit **panics** via `std::panic::catch_unwind` (the release-assert), and that it
  never produces a corrupted buffer (UB-free). `slice()` likewise asserts (does not clamp) on a
  bad range.
- Corpus: strings built from `UNICODE_PALETTE` (é/中/🙂/ZWJ/combining).

### 3. T2 — ChangeSet apply==splice + invert (change.rs proptests)

For an arbitrary unicode doc and a boundary-aligned `EditOp`:
- Build `cs` via the real constructor (`ChangeSet::insert`/`delete`, or `from_ops`); `apply(cs)`
  to a `TextBuffer` seeded with the doc; assert the result equals the naive `String` splice
  `model_apply(doc, at, del, ins)`.
- **Invert ordering (important):** `ChangeSet::invert` needs the ORIGINAL buffer to recover deleted
  text, so compute `let inv = cs.invert(&original)` **before** `cs.apply(&mut buf)`; then
  `inv.apply(&mut buf)` round-trips `buf` back to the original doc. (Do NOT compute `invert` from
  the post-edit buffer — it would lose the deleted text.)
- Cover: multi-op changesets, full-unicode payloads, `doc_len == 0`, edits at `0` and at `len`.

### 4. T3 — map_pos / map_pos_before (change.rs proptests)

For an arbitrary doc + changeset and input positions **drawn from the old doc's char boundaries**
(`map_pos`/`map_pos_before` are pure byte arithmetic with no document access — an off-boundary
INPUT cannot be expected to yield an on-boundary output, so the strategy must generate valid
`BytePos` from real char indices of the generated string):
- **On-boundary:** `map_pos(p, cs)` is a valid char boundary of the post-edit doc.
- **Monotonic:** `p1 <= p2 ⟹ map_pos(p1,cs) <= map_pos(p2,cs)` (same for `map_pos_before`).
- **Before/after bias:** at an insertion point, `map_pos_before` stays left of the insert and
  `map_pos` lands right of it (the documented bias) — pin the exact semantics against the impl.

### 5. T4 — History undo/redo (history.rs proptests)

For an arbitrary sequence of commits over an arbitrary doc:
- **Undo→redo exact:** after `commit`s, `undo()` then `redo()` returns the buffer (and selection)
  to the exact pre-undo state.
- **Selection valid:** every `before`/`after` selection an undo/redo restores is in-bounds and
  on a char boundary.
- **Coalescing loses nothing:** a coalesced run of `Type` edits, fully undone, yields the exact
  original doc (coalescing changes granularity, not content).

(NOT in core scope: "version strictly increases" — `version` lives on the SHELL `Document`
(`wordcartel/src/editor.rs`), not on core `History` (which has only `revisions`/`current`/
`bytes`/`last_evicted`). The monotonic increment happens in shell `Editor::apply`/`undo`/`redo`.
A core proptest cannot assert it; it would be a shell integration test — out of M7's core scope.)

### 6. F0 — the fuzz crate (`wordcartel-core/fuzz/`)

`cargo fuzz init`-style layout: `fuzz/Cargo.toml` (deps: `libfuzzer-sys`, `arbitrary` with derive,
`wordcartel-core` with the `fuzzing` cfg), `fuzz/fuzz_targets/{apply_pipeline.rs, block_tree.rs}`.
The `fuzzing` cfg is set via `RUSTFLAGS=--cfg fuzzing` (cargo-fuzz does this) so the
`#[cfg(any(test, fuzzing))]` core support compiles into the fuzz build. Seed `fuzz/corpus/*` from
the unicode palette + a few real markdown docs.

### 7. F1 — apply-pipeline fuzz target (the #1 data-loss target)

`fuzz_target!(|input: FuzzInput|)` (the fuzz-local DTO above):
- Start with `doc = input.doc` (valid UTF-8 from `arbitrary`); seed a `TextBuffer` and a `String`
  model.
- For each `FuzzOp`: clamp `at`/`del` into range and **snap to char boundaries** via
  `test_support::snap` (so we fuzz the VALID-edit pipeline; off-boundary refusal is T1's job),
  build a real `ChangeSet`, `apply` it, and `model_apply` the same splice.
- Assert after every op: `buf` content == `model` (no data loss/corruption). The target never
  panics on valid input (a panic = a fuzz finding). T2 covers the single-op property in-suite;
  F1 is the multi-op, coverage-guided sweep.

### 8. F2 — block_tree incremental ≡ full (oracle lift + fuzz target)

Lift the incremental-vs-full check into core. **`apply_edit` is ALREADY unconditionally `pub`**
(block_tree.rs ~1100) and is imported by the integration test `tests/block_tree_oracle.rs` — do
NOT re-gate it (a `cfg(test)` gate would make it invisible to that integration test, which builds
the lib as an external crate). Add only the new helper, gated `cfg(any(test, fuzzing))`:
```rust
#[cfg(any(test, fuzzing))]
pub fn incremental_equals_full(old: &str, range: std::ops::Range<usize>, repl: &str) -> bool {
    let (new, edit) = apply_edit(old, range, repl); // already pub — unchanged
    incremental_update(&full_parse(old), old, &edit, &new) == full_parse(&new)
}
```
`BlockTree` derives `PartialEq, Eq` (block_tree.rs:164), so `==` is valid. The existing UNIT test
`rope_incremental_matches_full_and_str` (block_tree.rs ~1380, a `#[cfg(test)]` test — it CAN see
the `cfg(any(test,fuzzing))` helper) re-expresses its assertion via `incremental_equals_full`
(keeping its rope-vs-str check too). The integration test in `tests/block_tree_oracle.rs` is
untouched (it uses the still-`pub` `apply_edit`/`full_parse`/`incremental_update` directly).
`fuzz_targets/block_tree.rs`: `fuzz_target!(|input: (String, usize, usize, String)|)` →
derive a valid `old`, clamp+snap `range` to char boundaries, call `incremental_equals_full(old,
range, repl)`, and assert it returns `true` (a `false`/panic = a fuzz finding — an incremental
parse that diverges from a full reparse is a real correctness bug).

## Definition of done (the M7 bar)

- **T1–T4** (and T6 from M1) green at the project's elevated case count (`ProptestConfig::with_cases`
  at the 2048+ standard the codebase already uses for load-bearing properties).
- **F1/F2** run a **manual `cargo fuzz run` campaign** (a bounded local session per target) with
  **zero new crashes**. Any crash → `cargo fuzz tmin` to minimize → add the minimized input as a
  pinned **normal-suite regression test** (so the bug can never silently return), then fix.
- No new data-loss / panic found that isn't fixed + pinned.

## Out of scope (comprehensive-later)

- **F3–F6** (ChangeSet-construction fuzz, `search` fuzz, etc.), **C2b** locality property,
  **C3** pathological corpus, and **standing fuzz CI** (~60 s/PR + nightly cron). M7 is the
  minimal T1–T4 + F1/F2 subset only.
- Reusing the F1/F2 generators for perf benchmarks (separate perf effort).

## New code surface (checklist for the plan)

- `wordcartel-core/src/test_support.rs` (new, `#[cfg(any(test, fuzzing))]`, **proptest-free**):
  `model_apply`, `EditOp`, `snap`, `UNICODE_PALETTE`; `#[cfg(any(test, fuzzing))] pub mod test_support;`
  in `lib.rs`. The proptest STRATEGIES live `#[cfg(test)]` in the test modules (proptest is a
  dev-dep, absent under `cfg(fuzzing)`).
- `wordcartel-core/src/buffer.rs`: T1 proptests.
- `wordcartel-core/src/change.rs`: T2 + T3 proptests.
- `wordcartel-core/src/history.rs`: T4 proptests.
- `wordcartel-core/src/block_tree.rs`: add `incremental_equals_full` behind `#[cfg(any(test, fuzzing))]`
  (leave `apply_edit` unconditionally `pub` — an integration test imports it); re-express the
  existing UNIT oracle test via the new helper.
- `wordcartel-core/fuzz/` (new crate): `Cargo.toml`, `fuzz_targets/apply_pipeline.rs` (F1),
  `fuzz_targets/block_tree.rs` (F2), seed `corpus/`.
- Any minimized fuzz-crash repro → a pinned regression test in the relevant core module.
