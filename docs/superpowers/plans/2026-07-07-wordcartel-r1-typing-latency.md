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

- [ ] **Step 1: Add the `#[cfg(test)]` counter** near the top of `fold.rs` (mirror the `LAYOUT_RUNS` pattern in derive.rs:38-40):

```rust
#[cfg(test)]
thread_local! {
    /// Counts full `outline::sections` walks in `FoldView::compute` — the expensive
    /// path. A no-folds keystroke must not increment this (R1 invariant guard).
    pub static SECTIONS_WALKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
```

- [ ] **Step 2: Write the failing tests** in `fold.rs` tests module (confirm the doc-build helper + `FoldState` fold-mutator name against the real source — see the anchor note after the code):

```rust
// Build a (BlockTree, TextBuffer) from markdown source for fold tests.
// Confirm the real parse entry point (block_tree::full_parse_src / parse) + TextBuffer ctor.
fn doc(src: &str) -> (wordcartel_core::block_tree::BlockTree, wordcartel_core::buffer::TextBuffer) {
    let buf = wordcartel_core::buffer::TextBuffer::from_str(src);
    let tree = wordcartel_core::block_tree::full_parse_src(src);
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
    folds.fold(0); // fold the H1 at byte 0 — confirm the real mutator name (fold/toggle/insert)
    SECTIONS_WALKS.with(|c| c.set(0));
    let _ = FoldView::compute(&folds, &blocks, &buf);
    assert!(SECTIONS_WALKS.with(|c| c.get()) >= 1, "folds active → the walk DOES run");
}
```

> **Anchor confirmations (do these first, adjust the snippet to the real names):** (a) the parse entry
> point — grep `pub fn full_parse_src` / `pub fn parse` in `wordcartel-core/src/block_tree.rs`; (b)
> `TextBuffer::from_str` and `.snapshot().len_lines()`; (c) the `FoldState` mutator that folds a heading
> by byte offset (grep `pub fn fold` / `toggle` in fold.rs); (d) that `FoldView` derives `PartialEq`
> (it does — fold.rs:126 `#[derive(... PartialEq, Eq)]`) so the `assert_eq!` on the whole struct compiles.

- [ ] **Step 3: Run to verify the skip test FAILS**

Run: `cargo test -p wordcartel --lib foldview_compute_skips 2>&1 | tail -15`
Expected: FAIL — today `compute` always runs `sections()`, so `SECTIONS_WALKS` is ≥1, not 0.

- [ ] **Step 4: Implement the early-return** in `FoldView::compute` (fold.rs:133), before the `sections()` walk:

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
    SECTIONS_WALKS.with(|c| c.set(c.get() + 1));
    // …existing sections()/filter/merge body unchanged…
}
```

(Keep the rest of the function exactly as-is; only the early-return + the counter line are added, and the
existing `let rope`/`let total` bindings are reused — do not duplicate them.)

- [ ] **Step 5: Run both tests**

Run: `cargo test -p wordcartel --lib foldview_compute 2>&1 | tail -15`
Expected: PASS — skip test now 0 walks; positive control ≥1.

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

- [ ] **Step 1: Add the `#[cfg(test)]` counter** near `LAYOUT_RUNS` in derive.rs:

```rust
#[cfg(test)]
thread_local! {
    /// Counts `outline::heading_starts` reconcile walks in `rebuild_downstream`. A
    /// no-folds keystroke must not increment this (R1 invariant guard).
    pub static HEADING_STARTS_WALKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
```

- [ ] **Step 2: Write the failing tests** in derive.rs tests module (confirm `BlockTree: Clone` + the `set_blocks`/selection paths — see anchor note):

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
    e.active_mut().folds.fold(0); // fold H1 — confirm the mutator name (same as Task 1)
    let tree = e.active().document.blocks().clone();
    e.active_mut().document.set_blocks(tree);
    HEADING_STARTS_WALKS.with(|c| c.set(0));
    crate::derive::rebuild_downstream(&mut e);
    assert!(HEADING_STARTS_WALKS.with(|c| c.get()) >= 1, "folds active → the walk DOES run");
}
```

> **Anchor confirmations:** (a) `Document::blocks()` returns `&BlockTree` and `BlockTree` derives
> `Clone` (grep `#[derive` on `BlockTree` in block_tree.rs); (b) `e.active_mut().document.set_blocks(..)`
> is the real path that bumps `blocks_generation` (editor.rs:91); (c) `e.active_mut().folds` is the
> `FoldState` on the active buffer, with the same fold mutator as Task 1; (d) `rebuild_downstream` is
> `pub(crate)`, callable from a derive.rs test.

- [ ] **Step 3: Run to verify the skip test FAILS**

Run: `cargo test -p wordcartel --lib no_folds_downstream_skips 2>&1 | tail -15`
Expected: FAIL — today the gate opens on the generation change and calls `heading_starts`, so the counter is ≥1.

- [ ] **Step 4: Implement the guard** — add the counter to the reconcile block and gate it on `!folds.is_empty()` (derive.rs:237-247):

```rust
{
    let gen = editor.active().document.blocks_generation();
    if !editor.active().folds.is_empty()
        && editor.active().last_reconciled_generation != Some(gen)
    {
        #[cfg(test)]
        HEADING_STARTS_WALKS.with(|c| c.set(c.get() + 1));
        let starts = {
            let b = editor.active();
            wordcartel_core::outline::heading_starts(b.document.blocks(), &b.document.buffer.snapshot())
        };
        editor.active_mut().folds.reconcile_to(&starts);
        editor.active_mut().last_reconciled_generation = Some(gen);
    }
}
```

(Remove the pre-existing `#[cfg(test)] bench_hs_t0` span timing OR leave it — but it must sit INSIDE the
`if` now so it only records on the expensive path, consistent with the counter. Keep whichever the bench
relies on; if kept, move it inside the guarded block.)

- [ ] **Step 5: Run both tests + confirm no existing test broke**

Run: `cargo test -p wordcartel --lib downstream 2>&1 | tail -15` → both PASS.
Run: `cargo test -p wordcartel --lib 2>&1 | tail -5` → full shell suite green (behavior-identical).

- [ ] **Step 6: Clippy + commit**

Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -3` → clean.
```bash
git add wordcartel/src/derive.rs
git commit -m "perf(r1): skip the heading-starts reconcile walk when no folds"
```

---

### Task 3: T5 — first-frame startup rebuild

**Files:**
- Modify: `wordcartel/src/app.rs` — the startup/session-resume site (~app.rs:1589, after `nav::ensure_visible`, before the first `draw`).
- Test: `wordcartel/src/derive.rs` or `nav.rs` tests module (a layout-freshness assertion that reproduces the sequence).

**Interfaces:**
- Consumes: `nav::ensure_visible` (mutates `view.scroll`/`scroll_row`, returns `()`); `derive::rebuild` (`LayoutKey`-gated); `editor.active().layout_key` / `view.scroll`.
- Produces: correct first-frame layout when the caret is off-screen on open.

- [ ] **Step 1: Write the failing test** — reproduce the off-screen-caret sequence and assert the layout
cache is fresh after a post-`ensure_visible` rebuild (confirm the `LayoutKey.scroll` accessor + `Selection`
path against real source):

```rust
#[test]
fn startup_rebuild_after_ensure_visible_refreshes_layout_for_offscreen_caret() {
    use crate::editor::Editor;
    // A doc taller than the 10-row viewport; caret near the end, scroll pinned at top.
    let src = "line\n".repeat(200);
    let mut e = Editor::new_from_text(&src, None, (80, 10));
    let caret = e.active().document.buffer.len().saturating_sub(1);
    e.active_mut().document.selection = wordcartel_core::selection::Selection::single(caret);
    e.active_mut().view.scroll = 0;
    e.active_mut().view.scroll_row = 0;
    crate::derive::rebuild(&mut e);              // builds layout for scroll = 0
    crate::nav::ensure_visible(&mut e);          // scrolls down to the off-screen caret
    let scroll_after = e.active().view.scroll;
    assert!(scroll_after > 0, "precondition: ensure_visible moved the viewport");
    // The T5 fix: rebuild after ensure_visible so the first frame's layout matches the new scroll.
    crate::derive::rebuild(&mut e);
    assert_eq!(
        e.active().layout_key.as_ref().map(|k| k.scroll),
        Some(scroll_after),
        "layout cache must be rebuilt for the post-ensure_visible scroll (T5)"
    );
}
```

> **Anchor confirmations:** (a) `LayoutKey` has a `scroll` field (derive.rs:249-259 — it does) and
> `editor.layout_key: Option<LayoutKey>` is accessible from the test; (b) `Selection::single(usize)`
> and `document.buffer.len()`; (c) if `layout_key` is private, assert instead on
> `e.active().view.line_layouts.contains_key(&scroll_after)` (the visible-range cache covers the new
> top line).

- [ ] **Step 2: Run to verify it FAILS**

Run: `cargo test -p wordcartel --lib startup_rebuild_after_ensure_visible 2>&1 | tail -15`
Expected: FAIL if the assertion is written to check state BEFORE the final rebuild; since the test itself
includes the fix's rebuild call, instead verify the BUG first by temporarily removing the final
`crate::derive::rebuild(&mut e);` line and confirming the layout_key scroll is stale (0, not `scroll_after`).
Then restore it. (This proves the assertion discriminates.)

- [ ] **Step 3: Implement the fix in app.rs** — add the rebuild at the startup site (after `ensure_visible`, ~app.rs:1589):

```rust
    crate::nav::ensure_visible(&mut editor);
    // T5: ensure_visible may have scrolled to an off-screen caret AFTER the earlier rebuild;
    // refresh the layout cache so the very first frame is correct. LayoutKey-gated → a cheap
    // no-op when scroll did not move.
    derive::rebuild(&mut editor);
    guard.terminal().draw(|f| render::render(f, &mut editor))?;
```

- [ ] **Step 4: Run the test + full suite**

Run: `cargo test -p wordcartel --lib startup_rebuild_after_ensure_visible 2>&1 | tail -10` → PASS.
Run: `cargo test -p wordcartel-core -p wordcartel 2>&1 | tail -5` → all green.

- [ ] **Step 5: Clippy + smoke + commit**

Run: `cargo clippy --workspace --all-targets 2>&1 | tail -3` → clean.
Run: `bash scripts/smoke/run.sh 2>&1 | tail -3` → quote the one-line summary (mandatory-run / advisory-pass).
```bash
git add wordcartel/src/app.rs
git commit -m "fix(r1): rebuild after ensure_visible at startup (T5 first-frame staleness)"
```

---

## Self-review notes (author)

- **Spec coverage:** Component 1a → Task 2; 1b → Task 1; Component 2 (regression guard: counters + no-folds-zero + folds-active positive control) → Tasks 1 & 2 (per-walk counters + both positive controls); Component 3 (T5) → Task 3. Testing §4 → the per-task tests; the latency-evidence bench re-run is not a task (advisory).
- **Type consistency:** `SECTIONS_WALKS` (fold.rs), `HEADING_STARTS_WALKS` (derive.rs), `FoldView { hidden: Vec::new(), total }`, `!folds.is_empty()`, `derive::rebuild` after `ensure_visible` — used consistently.
- **Anchors the implementer confirms against real source (flagged in-task):** the `block_tree` parse entry point (`full_parse_src`/`parse`) + `TextBuffer::from_str`/`snapshot().len_lines()`; the `FoldState` fold mutator name; `BlockTree: Clone`; `Document::set_blocks` path; `LayoutKey.scroll` visibility (fallback to `line_layouts.contains_key`); `Selection::single`. Each is called out where used.
- **Behavior-identical:** Tasks 1-2 change no output — the guards skip only empty-by-construction work; the full shell suite staying green (Task 2 Step 5) is the regression proof. Task 3 is `LayoutKey`-gated (no-op when scroll unchanged).
- **Out of scope (per spec §6):** reconcile-debounce retiming, structural-generation, input coalescing, the incremental-soundness divergences — none appear as tasks.
