# Valid-by-construction cache-key fields (footguns) — design

**Status:** spec-review round 1 folded (honest blast radius incl. sibling-test reads + test WRITE sites); re-review pending
**Date:** 2026-07-02
**Effort:** footguns (valid-by-construction; the first of two — F1 is a later, separate effort)

## Context

The editing-responsiveness effort (merged @ 84f999c) introduced two cache-identity
tokens that MUST be bumped on every write of their paired data, or a downstream cache
serves a stale value and the screen renders stale/wrong:

- `Document.blocks_generation` — bumped on every `document.blocks` write; keys the
  `FoldView` cache and the `LayoutKey` gate.
- `FoldState.epoch` — bumped on every `folded`-set change; keys the `FoldView` cache.

Today both the data field and its token are `pub` (`editor.rs:54-60`, `fold.rs:13-15`),
so a FUTURE direct mutation (`document.blocks = …` / `folds.folded.insert(…)`) would
silently bypass the bump → a stale-render bug. Every CURRENT production writer routes
correctly, but nothing enforces it — the compiler permits the footgun. A Fable 5
implementation review flagged this (Minors M1/M2), recommending the house
"valid-by-construction, private fields + accessors" convention, especially **before
Effort P exposes editor state to Lua plugins** (where a plugin writing a field directly
would become a stale-render vector).

This effort makes the bypass a **compile error** by privatizing the four fields behind
bumping accessors. It is a **pure, behavior-preserving refactor** — no runtime behavior
changes; the existing 900-test render/nav/fold/layout suite is the correctness net.

## Goals

- Make it impossible (compile error) to write `Document.blocks` without bumping
  `blocks_generation`, or to change `FoldState.folded` without bumping `epoch`.
- Zero observable behavior change; no new O(document) work; no logic change.

## Non-goals

- Do NOT privatize other `Document`/`FoldState` fields (`buffer`, `selection`, `history`,
  `version`, `path`, …) — scoped strictly to the two cache-key footguns Fable flagged.
- No new runtime tests (behavior-preserving → suite-green + clippy-clean is the proof,
  mirroring the clippy-debt effort). Add doc comments only.
- Not F1 (the bounded WidenToEnd reparse) — a separate later effort.

## Component 1 — `Document.blocks` / `blocks_generation` (editor.rs)

`Document` (editor.rs:51-68) derives only `Debug, Clone` (private fields break no derive),
and is constructed via a struct literal at EXACTLY ONE site — `Buffer::from_text`
(editor.rs:151-161), which is in the same crate/module and can see private fields (no new
constructor needed).

- Drop `pub` from `blocks: BlockTree` and `blocks_generation: u64` (→ private to the
  `editor` module).
- Add read accessors on `Document`:
  ```
  #[inline] pub fn blocks(&self) -> &wordcartel_core::block_tree::BlockTree { &self.blocks }
  #[inline] pub fn blocks_generation(&self) -> u64 { self.blocks_generation }
  ```
- Add the SOLE write path:
  ```
  /// The only way to write `blocks` — bumps `blocks_generation` so no writer can
  /// bypass the cache-identity token (valid-by-construction). Unconditional bump on
  /// each call; a caller that only wants to write-on-change guards the CALL (see
  /// the reconcile merge), not the bump.
  pub fn set_blocks(&mut self, blocks: wordcartel_core::block_tree::BlockTree) {
      self.blocks = blocks;
      self.blocks_generation = self.blocks_generation.wrapping_add(1);
  }
  ```
- **Writers → `set_blocks`:**
  - `derive.rs:157` (parse phase) currently does `editor.active_mut().document.blocks = new_blocks;` + a separate `blocks_generation = next_gen;`. Replace BOTH lines with
    `editor.active_mut().document.set_blocks(new_blocks);` (drop the now-redundant
    `next_gen` local and the explicit generation write — `set_blocks` bumps). Unconditional, matching today.
  - `reconcile.rs:66` merge: currently inside `if b.document.blocks != tree { b.document.blocks = tree; b.document.blocks_generation = …+1; }`. Keep the `!= tree` guard (now
    `if b.document.blocks() != &tree`), and inside it call `b.document.set_blocks(tree);`
    (drop the explicit generation bump). Preserves "bump only on real change" exactly.
- **Reads (~25 out-of-module `.document.blocks` sites → `.document.blocks()`):**
  `derive.rs:135,193,261`, `render.rs:285,1035`, `registry.rs:378,386,396,400,476,503,525`,
  `commands.rs:170,386`, `nav.rs:63,146,725,736,754,851,881`, `app.rs:468,1648,1661`,
  `save.rs:244,287`, `reconcile.rs:65` (the `!= tree` compare). In-module `editor.rs:463,608`
  stay direct (same module sees private fields).
- **`blocks_generation` reads (~4 → `.blocks_generation()`):** `derive.rs:156,189,233`.
  `editor.rs:454` is in-module → stays direct.

## Component 2 — `FoldState.folded` / `epoch` (fold.rs)

`FoldState` (fold.rs:12-16) derives `Debug, Clone, Default`; the derived `Default` needs no
struct literal, and there are ZERO `FoldState { … }` literals anywhere (all via
`FoldState::default()`), so privatization is transparent to construction.

- Drop `pub` from `folded: BTreeSet<usize>` and `epoch: u64` (→ private to the `fold`
  module).
- The **8 existing bumping mutators** (`toggle`/`fold_all`/`unfold_all`/`reconcile_to`/
  `remove`/`replace_folded`/`clamp`/`remap`, fold.rs:23-79) remain the only write path —
  they already bump `epoch` and stay valid with a private field. Add a doc line noting they
  are the sole write path.
- Add read accessors on `FoldState`:
  ```
  #[inline] pub fn folded(&self) -> &std::collections::BTreeSet<usize> { &self.folded }
  #[inline] pub fn epoch(&self) -> u64 { self.epoch }
  ```
- **`folded` reads:** in-module `fold.rs:129` (`FoldView::compute`) and `fold.rs:249`
  (`normalize_caret`) stay direct (same module). The 3 out-of-module reads →
  `.folds.folded()`: `registry.rs:505`, `render.rs:1034`, `app.rs:2277`.
- **`epoch` reads (both out-of-module → `.epoch()`):** `editor.rs:454` (the `active_fold_view`
  cache-key tuple) and `derive.rs:234` (the `LayoutKey.fold_epoch`).

## Blast radius — production AND tests (spec-review, folded)

The privatization forces the accessor on EVERY out-of-module reference, and sibling-module
`#[cfg(test)]` blocks are out-of-module too — so the real touch-count is roughly DOUBLE the
production figure. This is compiler-guided (each unconverted site is a build error), but the
spec accounts for it so the plan/implementer expect it:

- **`.document.blocks` reads:** ~27 production + ~25 sibling-test (e.g. `render.rs:1274`,
  `commands.rs:1386/1406/1426`, `nav.rs:1203-1428`, `derive.rs`-tests, `reconcile.rs`-tests,
  `transform.rs:220`, `mouse.rs:334`, `save.rs:674/681`, `app.rs:4751-4785`) → `.blocks()`.
- **`folds.folded` reads:** 3 production + ~18 sibling-test → `.folded()`.
- **`blocks_generation`/`epoch` reads:** the production sites above + any sibling-test hits.

**Direct WRITE sites in tests (must route through the API — the point of the effort):**
- `document.blocks = …` in tests — `derive.rs:327`, `reconcile.rs:112`, `reconcile.rs:170` →
  `document.set_blocks(…)`.
- `folds.folded.insert(…)` in a test — `app.rs:4748` → an existing mutator (`toggle(hb)` to
  fold one anchor, or `replace_folded(set)` for an exact set).

Converting test writes through `set_blocks`/mutators is a FEATURE, not a workaround: it makes
the valid-by-construction guarantee hold end-to-end, tests included. Watch for any test that
asserts a SPECIFIC `blocks_generation`/`epoch` value after such a write — `set_blocks` bumps
the generation, so such an assertion may need its expected value adjusted (a real behavior of
the accessor, not a bug).

## Testing

Behavior-preserving refactor → **the entire existing suite staying green (INCLUDING sibling-test
compilation, which now routes through the accessors) + workspace clippy clean is the correctness
proof.** Privacy makes a bump-bypassing write a COMPILE error — the
type system is the test. No new runtime tests. (Same rationale the clippy-debt effort used:
suite-green + clippy-0 is sufficient for a mechanical, behavior-preserving change.)

## Decomposition (2 tasks)

1. **Component 1 — `Document`:** privatize `blocks`/`blocks_generation`; add
   `blocks()`/`blocks_generation()`/`set_blocks()`; convert the 2 writers + ~29 out-of-module
   reads. Suite green + clippy clean.
2. **Component 2 — `FoldState`:** privatize `folded`/`epoch`; add `folded()`/`epoch()`;
   convert the 3 `folded` + 2 `epoch` out-of-module reads. Suite green + clippy clean.

## Global constraints

- Shell-only (`wordcartel`); no `wordcartel-core` change; `#![forbid(unsafe_code)]` unaffected.
- Workspace clippy **deny** gate stays clean; no `cargo fmt`; house style (em-dash `—`,
  doc-comment public items).
- No behavior change; no new O(document) work; the hot path is untouched (accessors are
  `#[inline]` and return `&`/`Copy`).

## Plan-confirms (resolve during the implementation plan, against real source)

1. No name collision: `Document` has no existing `blocks`/`blocks_generation`/`set_blocks`
   method, and `FoldState` no existing `folded`/`epoch` method (grep). If `Editor` also has a
   `blocks(...)`-like method, no conflict (different type).
2. Re-grep EXHAUSTIVELY for every out-of-module reference (production AND sibling-`#[cfg(test)]`)
   to `.document.blocks`, `document.blocks_generation`, `.folds.folded`, `.folds.epoch` — both
   READS (→ accessor) and WRITES (→ `set_blocks`/mutator). In-module refs (incl. the defining
   module's own `#[cfg(test)]` children — `editor::tests`, `fold::tests`) stay direct. The Blast
   Radius section lists the known sites; the plan must confirm none are missed (a miss is a
   build error, but the plan should be complete). Handle the direct test-write sites
   (`derive.rs:327`, `reconcile.rs:112/170` → `set_blocks`; `app.rs:4748` → a fold mutator).
3. Any test that asserts a specific `blocks_generation`/`epoch` value AFTER a converted write:
   `set_blocks` bumps the generation, so adjust the expected value if needed (accessor
   behavior, not a regression). Confirm none silently changes an assertion's meaning.
4. The `reconcile.rs` merge keeps its `!= tree` guard (now `blocks() != &tree`) so the
   bump-on-change behavior is byte-identical to today; `derive.rs` parse-phase call is
   unconditional (matches the current unconditional bump).
5. `active_fold_view` (editor.rs:454) reads `document.blocks_generation` (in-module → direct)
   but `folds.epoch` (out-of-module → `.epoch()`) — confirm the mixed access compiles.
