# Footguns (valid-by-construction cache-key fields) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make it a compile error to write `Document.blocks` (via the field) without bumping `blocks_generation`, or change `FoldState.folded` without bumping `epoch` — by privatizing the four fields behind bumping accessors. Route the two production whole-struct `folds` assignments through a bumping mutator. Pure behavior-preserving refactor.

**Architecture:** Shell-only. Privatize `blocks`/`blocks_generation` (editor.rs) + `folded`/`epoch` (fold.rs); add `&`-returning getters + a bumping `set_blocks`; the 8 existing fold mutators stay the sole `folded` write path. Convert every out-of-module read to a getter and every out-of-module write (incl. tests) to `set_blocks`/a mutator. The compiler enumerates the sites — a missed one is a build error.

**Tech Stack:** Rust, `wordcartel` shell.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-02-wordcartel-footguns-valid-by-construction-design.md` (Codex GO ×4 + Fable5 folded).
- **Zero observable behavior change** — no runtime tests added; the existing 900-test suite staying green + workspace clippy clean IS the correctness proof (privacy makes the bypass a compile error).
- Gates: `cargo test -p wordcartel -p wordcartel-core` green; `cargo build`/`test --no-run` warning-free; **`cargo clippy --workspace --all-targets` clean (deny gate LIVE)**; NO `cargo fmt`; house style (em-dash `—`, doc-comment public items).
- `#![forbid(unsafe_code)]` unaffected; no core change.
- **Clippy borrow-form (deny gate):** `&x.blocks` → `x.blocks()` (NOT `&x.blocks()` — `needless_borrow`); an `assert_eq!(…blocks, sentinel)` owned-vs-`&` comparison needs `*…blocks()` or `&sentinel`.
- Trailers on every commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

## Working method (both tasks)

This is a compiler-guided refactor. Per task: (1) add the accessors, (2) drop `pub` from the fields (the build now fails at every out-of-module site), (3) fix each error mechanically — a READ becomes the getter, a WRITE becomes `set_blocks`/a mutator — using the spec's Blast Radius census + write table as the checklist, (4) `cargo test -p wordcartel -p wordcartel-core` green + `cargo clippy --workspace --all-targets` clean, (5) commit. In-module refs (`editor::` for `Document`, `fold::` for `FoldState`, incl. their own `#[cfg(test)]` children) stay direct.

---

### Task 1: Component 1 — privatize `Document.blocks` / `blocks_generation`

**Files:** Modify `wordcartel/src/editor.rs` (struct + accessors), and convert reads/writes in `derive.rs`, `render.rs`, `registry.rs`, `commands.rs`, `nav.rs`, `app.rs`, `save.rs`, `reconcile.rs`, `transform.rs` (+ their test modules).

**Interfaces produced:** `Document::blocks(&self) -> &BlockTree`, `Document::blocks_generation(&self) -> u64`, `Document::set_blocks(&mut self, BlockTree)`.

- [ ] **Step 1: Add the accessors on `Document`** (`editor.rs`, in `impl Document` — add one if none):

```rust
    /// Read the derived block tree (private field — writes go through `set_blocks`).
    #[inline]
    pub fn blocks(&self) -> &wordcartel_core::block_tree::BlockTree { &self.blocks }
    /// The block-tree identity token; changes on every `set_blocks`. Keys the FoldView + layout caches.
    #[inline]
    pub fn blocks_generation(&self) -> u64 { self.blocks_generation }
    /// The ONLY way to write `blocks` — bumps `blocks_generation` so no writer can bypass the
    /// cache-identity token (valid-by-construction). Unconditional bump on each call; a caller
    /// wanting write-on-change guards the CALL (see the reconcile merge), not the bump.
    pub fn set_blocks(&mut self, blocks: wordcartel_core::block_tree::BlockTree) {
        self.blocks = blocks;
        self.blocks_generation = self.blocks_generation.wrapping_add(1);
    }
```

- [ ] **Step 2: Privatize the fields** — in the `Document` struct (editor.rs:54-60) drop `pub` from `blocks: BlockTree` and `blocks_generation: u64` (keep the doc comments). Build now fails at every out-of-module reference.

- [ ] **Step 3: Convert the two production WRITERS to `set_blocks`.**
  - `derive.rs:157` parse phase — replace the two lines
    `editor.active_mut().document.blocks = new_blocks;` and `…document.blocks_generation = next_gen;`
    with `editor.active_mut().document.set_blocks(new_blocks);` and DELETE the now-unused
    `let next_gen = …;` local (Step-1's bump replaces it). Unconditional — matches today.
  - `reconcile.rs:66` merge — keep the guard, now `if b.document.blocks() != &tree { … }`, and
    inside it replace `b.document.blocks = tree; b.document.blocks_generation = …+1;` with a single
    `b.document.set_blocks(tree);`. Bump-on-change stays byte-identical.

- [ ] **Step 4: Convert the production READS to `.blocks()` / `.blocks_generation()`.** Uniform mechanical swap at every out-of-module read the compiler flags — the census (spec Blast Radius + Component 1): `derive.rs:135,193,261` (+ `156,189,233` for `blocks_generation`), `render.rs:285,1035`, `registry.rs:378,386,396,400,476,503,525`, `commands.rs:170,386`, `nav.rs:63,146,725,736,754,851,881`, `app.rs:468,1648,1661`, `save.rs:244,287`, `reconcile.rs:65`, and **`transform.rs:90`** (`snap_to_blocks(&doc.blocks, …)` → `snap_to_blocks(doc.blocks(), …)` — the alias-binding site a `.document.` grep misses). Apply the clippy borrow-form rule.

- [ ] **Step 5: Convert the test WRITE sites** (sibling-module `#[cfg(test)]`; `editor::tests` stays direct):
  - `derive.rs:327`, `reconcile.rs:170` (`…document.blocks = X;` where `X` doesn't borrow the editor) → `…document.set_blocks(X);` in place.
  - `reconcile.rs:112` (`…document.blocks = block_tree::empty_tree(e.active().document.buffer.len());`) — the RHS borrows the editor, so a direct `set_blocks(RHS)` FAILS (the `&mut` from `active_mut()` conflicts with the `active()` in the arg; today's assignment only compiles because assignment evaluates RHS before the LHS place). **Two-statement split:** `let t = block_tree::empty_tree(e.active().document.buffer.len()); e.active_mut().document.set_blocks(t);`.
  - **Value-compare asserts** (`assert_eq!`/`!=` of `document.blocks` vs an owned `BlockTree`): `derive.rs:332,351`, `reconcile.rs:116,122,178` → compare `*doc.blocks()` (deref) or `&expected` (the owned-vs-`&` fix from the borrow-form constraint).
  - `derive.rs:768-769` (the layout-gate rerun test's `document.blocks_generation = …wrapping_add(1)`) → `let t = e.active().document.blocks().clone(); e.active_mut().document.set_blocks(t);` (bumps generation with UNCHANGED blocks — the sub-case's intent).
  - Convert the sibling-test READS too (compiler-flagged; e.g. `render.rs:1274`, `commands.rs:1386/1406/1426`, `nav.rs:1203-1428`, `transform.rs:220`, `mouse.rs:334`, `save.rs:674/681`, `app.rs:4751-4785`, and the `derive.rs`/`reconcile.rs` test reads). None of these tests assert a specific post-write `blocks_generation` (confirmed in spec review), so no expected-value adjustments — but WATCH for one and adjust if the compiler-fixed site had such an assertion.

- [ ] **Step 6: Verify grep completeness** — `git grep -n '\.blocks\b'` / `'\.blocks_generation\b'` across the crate; confirm every remaining bare-field hit is in-module (`editor.rs` incl. `editor::tests`) or is the accessor definitions themselves. Bare-field grep (not `.document.`) catches alias-binding sites. Filter comment/doc hits (e.g. jobs.rs, reconcile.rs:3, derive.rs:178) when checking remaining bare-field matches.

- [ ] **Step 7: Run + gates + commit** — `cargo test -p wordcartel -p wordcartel-core` green; `cargo clippy --workspace --all-targets` clean.
```bash
git add -A
git commit -m "refactor(editor): privatize Document.blocks/blocks_generation behind set_blocks accessor"   # + trailers
```

---

### Task 2: Component 2 — privatize `FoldState.folded` / `epoch`

**Files:** Modify `wordcartel/src/fold.rs` (struct + accessors), and convert reads/writes in `registry.rs`, `render.rs`, `app.rs`, `editor.rs`, `derive.rs`, `save.rs`, `marks.rs` (test `folded` reads at `:173/:221`) (+ test modules).

**Interfaces produced:** `FoldState::folded(&self) -> &BTreeSet<usize>`, `FoldState::epoch(&self) -> u64`. (The 8 bumping mutators already exist.)

- [ ] **Step 1: Add the accessors on `FoldState`** (`fold.rs`, in `impl FoldState`):

```rust
    /// Read the folded-anchor set (private field — all mutations go through the bumping
    /// mutators: toggle/fold_all/unfold_all/reconcile_to/remove/replace_folded/clamp/remap).
    #[inline]
    pub fn folded(&self) -> &std::collections::BTreeSet<usize> { &self.folded }
    /// The fold-identity token; changes on every folded-set mutation. Keys the FoldView cache.
    #[inline]
    pub fn epoch(&self) -> u64 { self.epoch }
```
Also add a one-line doc note on the mutator block (fold.rs:23-79) that they are the sole write path for `folded`.

- [ ] **Step 2: Privatize the fields** — in `FoldState` (fold.rs:13-15) drop `pub` from `folded: BTreeSet<usize>` and `epoch: u64`. Build fails at out-of-module references.

- [ ] **Step 3: Convert the two production whole-struct `folds =` assignments** (the residual-vector fix). In `reload_from_disk` (`save.rs:230/233`) and `load_recovered` (`save.rs:274/277`):
  - change the capture `let prev_folds = editor.active().folds.clone();` → `let prev_folded = editor.active().folds.folded().clone();`
  - change the restore `editor.active_mut().folds = prev_folds;` → `editor.active_mut().folds.replace_folded(prev_folded);`
  Behavior-equivalent (same folded set restored onto the just-installed fresh-cache buffer; `replace_folded` bumps `epoch`). The immutable `active()` borrow ends at the `.clone()`, before `active_mut()`.

- [ ] **Step 4: Convert the production READS to `.folded()` / `.epoch()`** at the out-of-module sites: `folded` → `registry.rs:505`, `render.rs:1034`, `app.rs:2277`; `epoch` → `editor.rs:454` (the `active_fold_view` key tuple), `derive.rs:234` (the `LayoutKey.fold_epoch`). In-module `fold.rs:129,249` stay direct.

- [ ] **Step 5: Convert the test WRITE + reads** (sibling-module `#[cfg(test)]`; `fold::tests` stays direct):
  - `app.rs:4748` (`folds.folded.insert(hb)`) → `folds.toggle(hb)` (inserts, since the anchor is absent on a fresh buffer). The extra `epoch` bump is inert-but-correct: the test does call `derive::rebuild` (which reads the epoch-keyed `active_fold_view` cache), but the cache is empty at that point, so a bump can only force a recompute — never a stale hit. (The old raw `insert`-without-bump was itself an instance of the very bug class this effort closes — it "worked" only because the cache was `None`; the conversion is strictly safer.)
  - Convert the sibling-test `folded`/`epoch` READS the compiler flags → the getters.

- [ ] **Step 6: Verify grep completeness** — `git grep -n '\.folded\b'` / `'\.epoch\b'` (filter to `FoldState`); confirm remaining bare-field hits are in-module (`fold.rs` incl. `fold::tests`) or the accessor defs (filter comment/doc hits). Confirm NO production whole-struct `folds =` / `document =` remains that bypasses a bump.

- [ ] **Step 7: Run + gates + commit** — full `cargo test -p wordcartel -p wordcartel-core` green; `cargo clippy --workspace --all-targets` clean.
```bash
git add -A
git commit -m "refactor(fold): privatize FoldState.folded/epoch; route save.rs whole-struct folds writes through replace_folded"   # + trailers
```

---

## Self-Review

**Spec coverage:** Component 1 (privatize + `blocks()`/`blocks_generation()`/`set_blocks()`, 2 writers, ~28 reads incl. transform.rs:90, 4 test writes) → Task 1 ✓; Component 2 (privatize + `folded()`/`epoch()`, reads, app.rs:4748 test write, the two `save.rs` whole-struct → `replace_folded` residual-vector fix) → Task 2 ✓; the grep-recipe (bare field patterns) → Steps 6 ✓; no new tests (suite-green proof) → Global Constraints ✓.

**Placeholder scan:** the bulk read conversions are intentionally "convert every compiler-flagged site mechanically" (a uniform `.field` → `.field()` swap) rather than an exhaustive per-line transcription — appropriate for a compiler-guided refactor; every NON-obvious conversion (writers, `set_blocks`, the `derive.rs:768` migration, the `save.rs` `replace_folded`, `transform.rs:90`, the borrow-form) is given explicitly.

**Type consistency:** `blocks()`/`folded()` return `&`; `blocks_generation()`/`epoch()` return `u64` (Copy); `set_blocks(BlockTree)` + `replace_folded(BTreeSet<usize>)` match their call sites; `Buffer` derives (Debug, Clone) + `Document`/`FoldState` derives unaffected by private fields.

**Ordering:** Task 1 (Document) and Task 2 (FoldState) are independent — either order compiles; Document first for the larger blast radius.
