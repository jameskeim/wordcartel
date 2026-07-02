# Editing responsiveness: cache the draw-path outline work — design

**Status:** approved design (pre-spec-review)
**Date:** 2026-07-02
**Effort:** editing-responsiveness (pre-Effort-P; the safe, always-on per-keystroke wins)

## Context

A Fable 5 hot-path analysis + a measurement pass identified where per-keystroke work
scales badly. `derive::rebuild` (`wordcartel/src/derive.rs`) runs on every draw; its
parse phase is already version-memoized (only reparses when `document.version !=
reconcile.blocks_version`), but its DOWNSTREAM phase, `rebuild_downstream`, runs
unconditionally every draw and does O(document-structure) work regardless of viewport
size — violating the "per-keystroke work stays O(visible)+O(edited)" priority even when
the parse is skipped.

This effort closes the three **safe, always-on** costs — pure optimizations (identical
output, fewer cycles) that speed up every keystroke on every document:

- **F2 — redundant outline/fold walks.** `rebuild_downstream` deep-clones the block
  tree and runs ~5–6 full tree walks per keystroke (`FoldView::compute` → `outline::sections`,
  `folds.reconcile` → `outline::heading_starts`), each allocating a `String` per heading,
  even with zero folds — repeated across `rebuild_downstream`, `ensure_visible`, the
  scrollbar render, and mouse handlers.
- **F3 — `role_at` linear scan.** `block_tree::role_at` recursively scans all top-level
  blocks, called once per visible line — O(visible × N_blocks) per draw.
- **Double-rebuild.** Command handlers call `derive::rebuild`, then the run loop calls it
  again pre-draw (`app.rs:~2161`), so `rebuild_downstream`'s layout pass runs twice per
  keystroke — and pointlessly on idle Ticks.

**Explicitly deferred to their own efforts:** **F1** (bound the synchronous `WidenToEnd`
reparse + let the reconcile converge) — a *behavior change* (transient wrong styling
below the edit) that only bites large container-heavy documents; and **F8** (per-line
layout memoization + bounding layout to visible rows) — a constant-factor + rare-trap
refinement in the grapheme layer.

## Goals

- Eliminate the redundant per-draw outline/fold tree walks and the block-tree deep clone,
  so `rebuild_downstream`'s non-layout work runs at most once per (tree-or-fold change),
  not 5–6× per keystroke.
- Make `role_at` O(log N) instead of O(N_blocks).
- Run the visible-line layout pass at most once per actual change (collapse the
  double-rebuild; skip on idle Ticks) — robustly, with zero screen-blank risk.
- **No observable behavior change** — identical rendered output; the existing render/nav/
  fold/layout suite is the primary correctness net.

## Non-goals

- No change to the incremental parser or the reconcile machinery (F1 is a separate effort).
- No change to the per-grapheme layout internals or the O(logical-line) layout extent
  (F8 is a separate effort).
- No new O(document) work on any path; the hot path must stay O(visible)+O(edited).

## Component 1 — F3: `role_at` binary search (core)

**File:** `wordcartel-core/src/block_tree.rs`. `role_at` (:188) calls `collect_role` (:231),
which recursively descends from `root` and, at each block, **linearly** iterates
`block.children` checking `span.contains(byte)`. Top-level children (`root.children`) and
the children at every nesting level are stored in **document order and non-overlapping**
(enforced by the parser's sequential `push_child`). So at each level, at most one child
contains `byte`, and it can be found by `partition_point` on `span.start` instead of a
linear scan.

- Replace the linear `for child in &block.children` scan in `collect_role` with a
  `partition_point`-based lookup of the single child whose span contains `byte`, then
  descend into it. The role-precedence accumulation (`role_precedence` min over containing
  blocks) is unchanged — only the child-selection loop changes.
- Complexity: O(depth · log(children)) instead of O(N_blocks). Same `BlockRole` result.

## Component 2 — F2: one shared, cached `FoldView` + folded reconcile (shell)

Today each of ~5–6 sites recomputes the fold view from scratch every draw:

| Call site | File:line |
|---|---|
| `rebuild_downstream` (FoldView + reconcile) | `derive.rs:166,170` |
| `ensure_visible` → `fold_view()` | `nav.rs:133` (used at :405,:451,:464) |
| scrollbar render (when visible) | `render.rs:596` |
| scrollbar mouse click / drag | `mouse.rs:157,:222` |

`FoldView::compute(folds, blocks, buf)` (`fold.rs:76`) is a **pure function of
`(folds, blocks, buffer)`** — it does NOT read scroll/viewport — and `blocks_version`
(`reconcile.blocks_version`) exactly identifies the current `blocks` (including after a
background reconcile-job merge). `folds.reconcile` (`fold.rs:37`) prunes fold anchors whose
heading no longer exists — it only mutates `folded` when the tree structure changed.

Design:
- Add `fold_epoch: u64` (on `FoldState`, `fold.rs:12`), bumped whenever the `folded` set
  actually changes. Mutation sites to cover: the fold commands (`registry.rs:384` toggle,
  `:397` fold_all, `:411` unfold_all, `:509` unfold_ancestors_of), the anchor remap in
  `Buffer::apply`/`undo`/`redo` (`editor.rs:210,:235,:254`), session restore (`app.rs:467`),
  and `reconcile`'s prune (`fold.rs:39`) — the last only when `retain` removes an element.
  (A helper that bumps the epoch only on a real change keeps this honest.)
- Add a cache on `Buffer`: `fold_view_cache: Option<(u64 /*blocks_version*/, u64 /*fold_epoch*/, Rc<FoldView>)>`.
- Add an accessor `Editor::active_fold_view(&mut self) -> Rc<FoldView>`: if the cache key
  `(active blocks_version, active fold_epoch)` matches, return the `Rc` clone; on a miss,
  run `folds.reconcile` (which may prune → may bump the epoch), then `FoldView::compute`,
  store `(blocks_version, fold_epoch, Rc::new(view))`, and return the `Rc`.
- Route ALL the call sites above through `active_fold_view`. `folds.reconcile` is folded
  into the miss path — so it stops running every draw and runs only when the tree changed.
- `Rc<FoldView>` avoids borrow-checker friction (the cache holds the `Rc`; callers clone it
  cheaply and can then mutate `editor`).

Net: **1 `sections`/`heading_starts` walk per (tree-or-fold change)**, not 5–6 per keystroke;
the `blocks.clone()` (`derive.rs:164`) and `buffer.clone()` (`:165`) done solely to feed
`reconcile`/`compute` are removed (the accessor borrows directly).

## Component 3 — double-rebuild collapse via a computed layout key (shell)

`rebuild_downstream`'s visible-line loop (`derive.rs:200–225`) clears and rebuilds
`view.line_layouts` from scratch on every call — twice per keystroke (command + pre-draw)
and on every idle Tick. Its output depends entirely on:
`(blocks_version, fold_epoch, view.scroll, view.scroll_row, view.area, text_width, active_line, view.mode)`.

- Compute that tuple as a `LayoutKey`; store the last one as `Buffer.layout_key: Option<LayoutKey>`.
- At the top of the layout section: compute the current key; if it equals `layout_key`,
  **skip** the layout loop entirely (`line_layouts` already matches); else run the loop and
  store the new key.
- Robust **by construction**: the key captures every input to `line_layouts`, so we never
  skip when anything changed — the "stale layout cache blanks the editing rows" hazard
  (render has no on-demand fallback) cannot occur. Eliminates the redundant second pass
  *and* idle-Tick rebuilds.

Note the fold-reconcile + `FoldView` (Component 2) is separately cached, so a skipped
layout pass recomputes neither.

**Net per keystroke (all three):** ~1 tree walk + ~1 layout pass with O(log N) `role_at`,
down from ~5–6 walks + 2 deep clones + 2 layout passes.

## Testing

Correctness burden is "same output, faster," so the **existing render/nav/fold/layout suite
staying green is the primary net** (any regression there = a cache-key bug). Added tests:

- **F3 (core):** a differential property test — new `role_at` ≡ the pre-change linear result
  over randomized trees × byte positions; plus a test/`debug_assert` that children are
  ordered + non-overlapping at every level (the invariant it rests on).
- **F2:** `active_fold_view` returns the **same** `Rc` (`Rc::ptr_eq`) on an unchanged
  `(blocks_version, fold_epoch)`; recomputes (new `Rc`) on a version bump and on a fold
  toggle; still prunes stale fold anchors when headings are removed; and the cached
  `FoldView` equals a fresh `FoldView::compute` for the same state.
- **Component 3:** the layout-key gate **skips** when all inputs are unchanged (a second
  `rebuild_downstream` with no state change) and **recomputes** on a change to any of
  version / fold_epoch / scroll / area / text_width / active_line / mode; and `line_layouts`
  is correct (non-empty, right rows) after a change.
- **Call-count assertions (the proof-of-speedup, zero-dependency, no timing):**
  - `FoldView` reuse: after simulating a keystroke through both the command rebuild and the
    pre-draw rebuild, assert the two `active_fold_view` results are the same `Rc`
    (`Rc::ptr_eq`) — i.e. ≤1 compute per keystroke.
  - Layout-pass count: a `#[cfg(test)]` counter (e.g. `Buffer.layout_runs: u64` behind
    `cfg(test)`, or a test-local atomic incremented where the loop runs) asserts the layout
    loop runs **0** times on a no-change rebuild / idle Tick and **1** time per keystroke,
    not 2.

## Decomposition (3 tasks, ascending risk)

1. **F3** — `role_at` binary search + the differential property test (core only). Independent;
   lands first.
2. **F2** — `fold_epoch` + the `Rc<FoldView>` cache + `active_fold_view` + route all call
   sites through it + fold `reconcile` into the miss path + tests (incl. the `Rc::ptr_eq`
   reuse assertion).
3. **Component 3** — the computed `LayoutKey` gate on the layout pass + tests (incl. the
   layout-run counter). Builds on F2's cache being present.

## Global constraints

- Shell + one core fn (`role_at`); `#![forbid(unsafe_code)]` in core unchanged.
- Workspace clippy **deny** gate stays clean; no `cargo fmt`; house style (em-dash `—`).
- Hot path stays O(visible)+O(edited); no task introduces O(document) work.

## Plan-confirms (resolve during the implementation plan, against real source)

1. Children are ordered + non-overlapping at **every** nesting level (not just top-level) —
   confirm against the parser's `push_child`, so the `partition_point` descent in
   `collect_role` is correct at depth.
2. `Buffer` is main-thread-only (the reconcile/save jobs capture rope snapshots + versions,
   not the `Buffer` itself), so `Rc<FoldView>` on `Buffer` is sound. If any path requires
   `Buffer: Send`, fall back to an owned `FoldView` + clone (or `Arc`).
3. The exact, exhaustive set of `folded`-mutation sites that must bump `fold_epoch` (the
   table above) — confirm none are missed, and that `reconcile`'s prune bumps only on a real
   change (else the F2 cache never hits, since `reconcile` runs on the miss path).
4. `FoldView` is cheap to hold behind `Rc` and its methods used by `ensure_visible`
   (`normalize_line`, `next_visible`, `prev_visible`, `line_at_ordinal`, `visible_ordinal`,
   `visible_count`) work unchanged through an `Rc` deref.
5. The precise `LayoutKey` field list matches every input the layout loop reads (confirm
   `text_width`/`area`/`scroll_row`/`mode`/`active_line` are the complete set; nothing else
   the loop reads is omitted).
