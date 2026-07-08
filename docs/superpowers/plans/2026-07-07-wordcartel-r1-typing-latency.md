# R1 Typing-Latency Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the two per-keystroke whole-block-tree walks in `derive::rebuild_downstream` for the common no-folds case (behavior-identical), lock the invariant in with a regression guard, and fix the T5 first-frame startup-staleness bug.

**Architecture:** Guard both O(document) walks on the existing `FoldState::is_empty()` predicate — a trivial-view early-return in `FoldView::compute` (fixes the fold-view walk for every caller) and a `!folds.is_empty()` gate on the reconcile block in `derive.rs` (skips the heading-starts walk). Both skip only work that is empty by construction, so output is unchanged. Two `#[cfg(test)]` walk counters on the expensive paths prove the walks are skipped. A `LayoutKey`-gated `derive::rebuild` after `ensure_visible` at the startup site fixes the off-screen-caret first frame.

**Tech Stack:** Rust, `wordcartel` shell crate (ratatui 0.30). No `wordcartel-core` change, no new crates.

## Global Constraints

- **Shell-only:** touch only `wordcartel/src/{fold.rs, derive.rs, app.rs}`. `outline::heading_starts`/`sections` live in `wordcartel-core/src/outline.rs` and are NOT modified — only their shell callers are guarded.
- **Behavior-identical:** the no-folds fast path must not change any rendered output. The guards skip only provably-empty work.
- **House style:** snake_case, 4-space indent, hand-wrapped ~100 cols, em-dash `—` in prose comments (never `--`), no emoji. Do NOT run `cargo fmt`. Match neighbors by hand.
- **No `.unwrap()`** on fallible/external paths; exhaustive matches (no catch-all `_` absorbing a new variant).
- **Merge GATEs:** `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean; build + `--no-run` warning-free for touched crates. The `#[cfg(test)]` instrumentation must add ZERO cost to a non-test/release build.
- **Command-surface contract:** N/A — this effort touches no commands, options, palette, menu, or keybindings.
- **PTY smoke:** mandatory-run / advisory-pass — quote the one-line summary in the final report.

---

## File structure

- `wordcartel/src/fold.rs` — `FoldView::compute` `is_empty()` early-return (walk-2 fix) + a `#[cfg(test)]` `SECTIONS_WALKS` counter on the expensive path.
- `wordcartel/src/derive.rs` — `rebuild_downstream` reconcile guard `!folds.is_empty()` (walk-1 fix) + a `#[cfg(test)]` `HEADING_STARTS_WALKS` counter inside the guarded block.
- `wordcartel/src/app.rs` — the T5 `derive::rebuild` after `ensure_visible` at the startup/session-resume site.

Task order: 1 (fold-view walk, most-called) → 2 (reconcile walk) → 3 (T5). Each is independently testable via its own counter (Tasks 1-2) or a layout-freshness assertion (Task 3).

---

### Task 1: No-folds fast path — `FoldView::compute` trivial early-return (walk 2)

**Files:**
- Modify: `wordcartel/src/fold.rs` — `FoldView::compute` (~fold.rs:133); add a `#[cfg(test)]` `SECTIONS_WALKS` thread-local near the other test instrumentation.
- Test: `wordcartel/src/fold.rs` `#[cfg(test)] mod tests`.

**Interfaces:**
- Consumes: `FoldState::is_empty()` (fold.rs:19); `FoldView { hidden: Vec<HiddenRun>, total: usize }` (fold.rs:127); `FoldView::compute(&FoldState, &BlockTree, &TextBuffer) -> FoldView` (fold.rs:133); `outline::sections` (the walk being guarded).
- Produces: `pub(crate) static SECTIONS_WALKS` `#[cfg(test)]` counter (read by later tests); `FoldView::compute` now O(1) when `folds.is_empty()`.

- [ ] **Step 1: Add the counter AND instrument the CURRENT expensive path** (so the Red test genuinely
fails on today's code). Add the `#[cfg(test)]` thread-local near the top of `fold.rs` (mirror `LAYOUT_RUNS`
at derive.rs:38-40), and increment it immediately BEFORE the existing `outline::sections(...)` call
(fold.rs:138) — today `compute` always reaches this, so it increments on every call:

```rust
#[cfg(test)]
thread_local! {
    /// Counts full `outline::sections` walks in `FoldView::compute` — the expensive
    /// path. A no-folds keystroke must not increment this (R1 invariant guard).
    pub static SECTIONS_WALKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
```
```rust
// …in FoldView::compute, immediately before the existing `let mut runs = outline::sections(...)`:
#[cfg(test)]
SECTIONS_WALKS.with(|c| c.set(c.get() + 1));
```

- [ ] **Step 2: Write the tests** in `fold.rs`'s own `#[cfg(test)] mod tests` (private `FoldView` fields
force the literal to live here). Real APIs confirmed by the plan gate: `block_tree::full_parse(&str)`,
`FoldState::toggle(heading_byte)`:

```rust
// Build a (BlockTree, TextBuffer) from markdown source for fold tests.
fn doc(src: &str) -> (wordcartel_core::block_tree::BlockTree, wordcartel_core::buffer::TextBuffer) {
    let buf = wordcartel_core::buffer::TextBuffer::from_str(src);
    let tree = wordcartel_core::block_tree::full_parse(src);
    (tree, buf)
}

#[test]
fn foldview_compute_skips_sections_walk_when_no_folds() {
    let (blocks, buf) = doc("# H1\n\npara one\n\n## H2\n\nbody two\n");
    let folds = FoldState::default(); // empty
    SECTIONS_WALKS.with(|c| c.set(0));
    let v = FoldView::compute(&folds, &blocks, &buf);
    assert_eq!(SECTIONS_WALKS.with(|c| c.get()), 0, "no sections walk when no folds");
    // Behavior-identical to a computed-empty view: nothing hidden, total = line count.
    assert_eq!(v, FoldView { hidden: Vec::new(), total: buf.snapshot().len_lines() });
}

#[test]
fn foldview_compute_runs_sections_walk_when_folds_active() {
    let (blocks, buf) = doc("# H1\n\npara one\n\n## H2\n\nbody two\n");
    let mut folds = FoldState::default();
    folds.toggle(0); // fold the H1 at byte 0 (FoldState::toggle, fold.rs:34)
    SECTIONS_WALKS.with(|c| c.set(0));
    let _ = FoldView::compute(&folds, &blocks, &buf);
    assert!(SECTIONS_WALKS.with(|c| c.get()) >= 1, "folds active → the walk DOES run");
}
```

- [ ] **Step 3: Run to verify the skip test FAILS (Red)**

Run: `cargo test -p wordcartel --lib foldview_compute 2>&1 | tail -15`
Expected: `foldview_compute_skips_sections_walk_when_no_folds` FAILS (counter is 1, not 0 — today
`compute` always walks); `foldview_compute_runs_sections_walk_when_folds_active` PASSES.

- [ ] **Step 4: Implement the early-return** in `FoldView::compute` (fold.rs:133), ABOVE the counter +
`sections()` call, reusing the existing `let rope`/`let total` bindings (do not duplicate them):

```rust
pub fn compute(folds: &FoldState, blocks: &BlockTree, buf: &TextBuffer) -> FoldView {
    let rope = buf.snapshot();
    let total = rope.len_lines();
    // R1: no folds → nothing hidden. Skip the O(document) sections() walk entirely;
    // the full path would filter every section out anyway (empty `folded`), so this is
    // behavior-identical. Fixes every caller (derive + nav) at once.
    if folds.is_empty() {
        return FoldView { hidden: Vec::new(), total };
    }
    #[cfg(test)]
    SECTIONS_WALKS.with(|c| c.set(c.get() + 1)); // now behind the guard: only the expensive path counts
    // …existing sections()/filter/merge body unchanged…
}
```

- [ ] **Step 5: Run both tests (Green)**

Run: `cargo test -p wordcartel --lib foldview_compute 2>&1 | tail -15`
Expected: PASS — no-folds early-returns before the counter (0 walks); folds-active reaches it (≥1).

- [ ] **Step 6: Clippy + commit**

Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -3` → clean.
```bash
git add wordcartel/src/fold.rs
git commit -m "perf(r1): FoldView::compute skips the sections walk when no folds"
```

---

### Task 2: No-folds fast path — reconcile guard (walk 1)

**Files:**
- Modify: `wordcartel/src/derive.rs` — the reconcile block in `rebuild_downstream` (~derive.rs:237-247); add a `#[cfg(test)]` `HEADING_STARTS_WALKS` counter near `LAYOUT_RUNS` (derive.rs:38-40).
- Test: `wordcartel/src/derive.rs` `#[cfg(test)] mod tests`.

**Interfaces:**
- Consumes: `FoldState::is_empty()`; `Document::set_blocks` (bumps `blocks_generation`, editor.rs:91-94); `derive::rebuild_downstream` (pub(crate)); `outline::heading_starts` (the walk being guarded).
- Produces: `pub(crate) static HEADING_STARTS_WALKS` `#[cfg(test)]` counter; the reconcile walk now skipped when no folds.

- [ ] **Step 1: Add the counter AND instrument the CURRENT gate** (so the Red test fails on today's
code). Add the `#[cfg(test)]` thread-local near `LAYOUT_RUNS` in derive.rs, and increment it INSIDE the
existing generation gate, immediately before the `heading_starts` call (derive.rs:241) — today the gate
opens on every generation change, so it increments every keystroke:

```rust
#[cfg(test)]
thread_local! {
    /// Counts `outline::heading_starts` reconcile walks in `rebuild_downstream`. A
    /// no-folds keystroke must not increment this (R1 invariant guard).
    pub static HEADING_STARTS_WALKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
```
```rust
// …inside the existing `if last_reconciled_generation != Some(gen) {` block, before heading_starts:
#[cfg(test)]
HEADING_STARTS_WALKS.with(|c| c.set(c.get() + 1));
```

- [ ] **Step 2: Write the tests** in derive.rs's own `#[cfg(test)] mod tests` (real APIs confirmed by
the plan gate: `BlockTree: Clone`, `Document::set_blocks`, `FoldState::toggle`, `rebuild_downstream` is
`pub(crate)`):

```rust
#[test]
fn no_folds_downstream_skips_heading_starts_walk() {
    use crate::editor::Editor;
    let mut e = Editor::new_from_text("# H1\n\npara\n\n## H2\n\nbody\n", None, (80, 24));
    crate::derive::rebuild(&mut e); // settle: last_reconciled_generation == current gen
    // Simulate a keystroke's tree bump WITHOUT reparsing: re-set the same blocks so
    // blocks_generation advances (set_blocks bumps unconditionally), reopening the gate.
    let tree = e.active().document.blocks().clone();
    e.active_mut().document.set_blocks(tree);
    HEADING_STARTS_WALKS.with(|c| c.set(0));
    crate::derive::rebuild_downstream(&mut e);
    assert_eq!(HEADING_STARTS_WALKS.with(|c| c.get()), 0, "no folds → no reconcile walk");
}

#[test]
fn folds_active_downstream_runs_heading_starts_walk() {
    use crate::editor::Editor;
    let mut e = Editor::new_from_text("# H1\n\npara\n\n## H2\n\nbody\n", None, (80, 24));
    crate::derive::rebuild(&mut e);
    e.active_mut().folds.toggle(0); // fold H1 (FoldState::toggle, fold.rs:34)
    let tree = e.active().document.blocks().clone();
    e.active_mut().document.set_blocks(tree);
    HEADING_STARTS_WALKS.with(|c| c.set(0));
    crate::derive::rebuild_downstream(&mut e);
    assert!(HEADING_STARTS_WALKS.with(|c| c.get()) >= 1, "folds active → the walk DOES run");
}
```

- [ ] **Step 3: Run to verify the skip test FAILS (Red)**

Run: `cargo test -p wordcartel --lib downstream 2>&1 | tail -15`
Expected: `no_folds_downstream_skips_heading_starts_walk` FAILS (counter is 1 — today the gate opens on
the generation change and walks); `folds_active_downstream_runs_heading_starts_walk` PASSES.

- [ ] **Step 4: Implement the guard** — add `!folds.is_empty() &&` to the existing gate condition
(derive.rs:238). This automatically brings the whole block — the counter, the pre-existing
`#[cfg(test)] bench_hs_t0` span, `heading_starts`, `reconcile_to`, and the `last_reconciled_generation`
set — under the stricter guard (no separate move needed; they already sit inside this `if`):

```rust
let gen = editor.active().document.blocks_generation();
if !editor.active().folds.is_empty()
    && editor.active().last_reconciled_generation != Some(gen)
{
    // …unchanged block body: bench span + HEADING_STARTS_WALKS counter + heading_starts +
    //    reconcile_to + set last_reconciled_generation…
}
```

- [ ] **Step 5: Run both tests + confirm no existing test broke (Green)**

Run: `cargo test -p wordcartel --lib downstream 2>&1 | tail -15` → both PASS.
Run: `cargo test -p wordcartel --lib 2>&1 | tail -5` → full shell suite green (behavior-identical).

- [ ] **Step 6: Clippy + commit**

Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -3` → clean.
```bash
git add wordcartel/src/derive.rs
git commit -m "perf(r1): skip the heading-starts reconcile walk when no folds"
```

---

### Task 3: T5 — first-frame startup rebuild (via a shared, guarded helper)

**Files:**
- Modify: `wordcartel/src/app.rs` — extract a `pub(crate) fn first_frame_settle(&mut Editor)` (`ensure_visible` + `derive::rebuild`), and route the startup/session-resume site (~app.rs:1589) through it. The shared helper is what makes the test a real regression guard for the app site.
- Test: `wordcartel/src/app.rs` `#[cfg(test)] mod tests` (calls the same helper).

**Interfaces:**
- Consumes: `nav::ensure_visible` (mutates `view.scroll`/`scroll_row`, returns `()`); `derive::rebuild` (`LayoutKey`-gated; `derive` already imported at app.rs:7); `Buffer.layout_key` (pub, editor.rs:161) / `LayoutKey.scroll` (pub, derive.rs:13); `Selection::single(BytePos)` (selection.rs:43).
- Produces: `pub(crate) fn first_frame_settle` — the pre-first-frame invariant, shared by `run` and the test.

- [ ] **Step 1: Write the test** in app.rs's `#[cfg(test)] mod tests` (it calls the not-yet-existing helper, so it won't compile — that's the Red):

```rust
#[test]
fn first_frame_settle_refreshes_layout_for_offscreen_caret() {
    use crate::editor::Editor;
    // A doc taller than the 10-row viewport; caret near the end, scroll pinned at top.
    let src = "line\n".repeat(200);
    let mut e = Editor::new_from_text(&src, None, (80, 10));
    let caret = e.active().document.buffer.len().saturating_sub(1);
    e.active_mut().document.selection = wordcartel_core::selection::Selection::single(
        wordcartel_core::selection::BytePos(caret)); // confirm the BytePos path/ctor
    e.active_mut().view.scroll = 0;
    e.active_mut().view.scroll_row = 0;
    crate::derive::rebuild(&mut e); // builds layout for scroll = 0
    crate::app::first_frame_settle(&mut e); // ensure_visible + rebuild — the T5 unit under test
    let scroll_after = e.active().view.scroll;
    assert!(scroll_after > 0, "precondition: ensure_visible moved the viewport");
    assert_eq!(
        e.active().layout_key.as_ref().map(|k| k.scroll),
        Some(scroll_after),
        "layout cache must be rebuilt for the post-ensure_visible scroll (T5)"
    );
}
```

> **Anchor confirmations:** (a) the `BytePos` path/constructor — grep `pub struct BytePos` /
> `Selection::single` in `wordcartel-core/src/selection.rs` and use the real form (`BytePos(caret)` or a
> `BytePos::new`/`from`); (b) `Buffer.layout_key` + `LayoutKey.scroll` are pub (confirmed) — if the
> `LayoutKey` type itself is not nameable from app.rs, assert instead on
> `e.active().view.line_layouts.contains_key(&scroll_after)` (the `BTreeMap<usize, _>` visible-range
> cache covers the new top line).

- [ ] **Step 2: Create the helper WITHOUT the rebuild (reproduce the bug) and route startup through it.**
Add to app.rs and replace the bare `crate::nav::ensure_visible(&mut editor);` at the startup site
(~app.rs:1589) with `first_frame_settle(&mut editor);`:

```rust
/// Prepare the editor for the FIRST frame drawn OUTSIDE the reduce loop (startup /
/// session-resume): pin the caret's viewport, then rebuild so the layout cache matches
/// the possibly-moved scroll. The reduce loop's `advance` does this per keystroke; the
/// one-off startup draw must call this or the first frame can render a stale range (T5).
pub(crate) fn first_frame_settle(editor: &mut Editor) {
    crate::nav::ensure_visible(editor);
    // (rebuild added in Step 4)
}
```

- [ ] **Step 3: Run to verify the test FAILS (Red)**

Run: `cargo test -p wordcartel --lib first_frame_settle_refreshes 2>&1 | tail -15`
Expected: FAIL — `ensure_visible` moved the scroll but no rebuild ran, so `layout_key.scroll` is still 0,
not `scroll_after`.

- [ ] **Step 4: Add the rebuild to the helper**

```rust
pub(crate) fn first_frame_settle(editor: &mut Editor) {
    crate::nav::ensure_visible(editor);
    derive::rebuild(editor); // T5: refresh the layout cache for the (possibly moved) scroll.
                             // LayoutKey-gated → a cheap no-op when scroll did not move.
}
```

- [ ] **Step 5: Run the test + full suite (Green)**

Run: `cargo test -p wordcartel --lib first_frame_settle_refreshes 2>&1 | tail -10` → PASS.
Run: `cargo test -p wordcartel-core -p wordcartel 2>&1 | tail -5` → all green.

Because `run`'s startup site now calls the same `first_frame_settle`, deleting the rebuild from the helper
would fail this test — the app site is genuinely guarded.

- [ ] **Step 6: Clippy + smoke + commit**

Run: `cargo clippy --workspace --all-targets 2>&1 | tail -3` → clean.
Run: `bash scripts/smoke/run.sh 2>&1 | tail -3` → quote the one-line summary (mandatory-run / advisory-pass).
```bash
git add wordcartel/src/app.rs
git commit -m "fix(r1): first_frame_settle rebuilds after ensure_visible at startup (T5)"
```

---

## Self-review notes (author)

- **Spec coverage:** Component 1a → Task 2; 1b → Task 1; Component 2 (regression guard: counters + no-folds-zero + folds-active positive control) → Tasks 1 & 2 (per-walk counters + both positive controls); Component 3 (T5) → Task 3. Testing §4 → the per-task tests; the latency-evidence bench re-run is not a task (advisory).
- **Type consistency:** `SECTIONS_WALKS` (fold.rs), `HEADING_STARTS_WALKS` (derive.rs), `FoldView { hidden: Vec::new(), total }`, `!folds.is_empty()`, `derive::rebuild` after `ensure_visible` — used consistently.
- **Anchors resolved by the Codex plan gate (round 1):** parse fn `block_tree::full_parse(&str)`
  (block_tree.rs:325); `FoldState::toggle(heading_byte)` (fold.rs:34, NOT `fold`); `TextBuffer::from_str`
  (buffer.rs:14) + `snapshot().len_lines()`; `BlockTree: Clone` (block_tree.rs:164); `Document::set_blocks`
  (editor.rs:91); `FoldView` `PartialEq`/`Eq`, fields private → literal only inside `fold.rs` tests
  (fold.rs:126); `LayoutKey.scroll` pub (derive.rs:13) + `Buffer.layout_key` pub (editor.rs:161);
  `Selection::single(BytePos)` (selection.rs:43) — the only still-flagged anchor is the exact `BytePos`
  constructor path (Task 3 Step 1). TDD Red is now genuine: each counter is instrumented on the CURRENT
  expensive path in Step 1, so the skip test fails before the guard lands.
- **Behavior-identical:** Tasks 1-2 change no output — the guards skip only empty-by-construction work; the full shell suite staying green (Task 2 Step 5) is the regression proof. Task 3 is `LayoutKey`-gated (no-op when scroll unchanged).
- **Out of scope (per spec §6):** reconcile-debounce retiming, structural-generation, input coalescing, the incremental-soundness divergences — none appear as tasks.
