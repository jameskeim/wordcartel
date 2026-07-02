# Editing responsiveness: cache the draw-path outline work — design

**Status:** spec-review round 2 folded (Fable5: CRITICAL-1 layout-output invariant + RefCell &self accessor + full nav routing); re-review pending
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

## Shared prerequisite — `blocks_generation` (a true block-tree identity token)

**Spec-review Critical (folded).** `reconcile.blocks_version` does NOT uniquely identify
`document.blocks`. The background reconcile-job merge (`reconcile.rs:64`) replaces
`document.blocks` with the corrected full-parse tree *while keeping the same version number*
(`b.reconcile.blocks_version = version` where `version` is the value the incremental parse
already stored). So a cache keyed on `blocks_version` would serve a stale `FoldView` /
skip relayout after the merge lands — silently defeating the reconcile's convergence
(the corrected styling would not render until the next keystroke). Both F2 and Component 3
need a key that changes across the reconcile-merge boundary.

- Add `blocks_generation: u64` on `Document` (next to `blocks`/`version`), bumped on
  **every** write to `document.blocks`: the parse-phase assignment (`derive.rs:135`) and
  the reconcile-merge replacement (inside the `if b.document.blocks != tree` branch,
  `reconcile.rs:66`). Monotonic; a fresh `Document` starts at 0.
- `blocks_generation` is the tree-identity token for BOTH caches below (replacing
  `blocks_version`). It changes whenever `blocks` changes — including a merge that keeps the
  version — and, because the parse phase reassigns `blocks` on every version change, it also
  changes on every text edit (so it subsumes buffer/text changes for `line_text`).
- **The parse-phase assignment (`derive.rs:135`) stays UNCONDITIONAL** — no "skip the bump
  if the new tree equals the old" optimization. Replacing one character with another can
  yield a byte-identical tree while `line_text` changed; the `line_text`-subsumption argument
  for both caches depends on the generation bumping on every version change regardless.

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
- The sibling-non-overlap invariant is source-supported (`push_child` only appends,
  `block_tree.rs:442`) but currently unasserted. Pin it with the differential property test
  (below) and, if cheap, a `debug_assert` in the tree constructor / a dedicated test — NOT
  inside `role_at` itself (no validation on the per-keystroke hot path).

## Component 2 — F2: one shared, cached `FoldView` + folded reconcile (shell)

Today each of ~5–6 sites recomputes the fold view from scratch every draw:

| Call site | File:line |
|---|---|
| `rebuild_downstream` (FoldView + reconcile) | `derive.rs:166,170` |
| `ensure_visible` → `fold_view()` | `nav.rs:133` (used at :405,:451,:464) |
| scrollbar render (when visible) | `render.rs:596` |
| scrollbar mouse click / drag | `mouse.rs:157,:222` |

`FoldView::compute(folds, blocks, buf)` (`fold.rs:76`) is a **pure function of
`(folds, blocks, buffer)`** — it does NOT read scroll/viewport/selection (confirmed) — so it
is cacheable by `(blocks_generation, fold_epoch)`. `folds.reconcile` (`fold.rs:37`) prunes
fold anchors whose heading no longer exists.

**Complete call-site set (spec-review, folded — the round-1 table undercounted nav).**
The shell computes a fold view at: `rebuild_downstream` (`derive.rs:170`), the scrollbar
render (`render.rs:596`), scrollbar mouse (`mouse.rs:157,:222`), and — the big one — the
free fn `nav::fold_view(editor: &Editor)` (`nav.rs:131`) with **14 callers**: `:325,:329,:378,
:382` (visual_down/up + debug_asserts), `ensure_visible` `:405,:451,:464,:475`, `rows_before_caret`
`:545`, `advance_view_top_one_row`/`scroll_up_one` `:560,:587`, `clamp_caret_into_view` `:616`,
`last_fully_visible_line` `:797`, `offset_at_cell` `:917`. Arrow-key motion and drag-autoscroll
run through these, so they ARE per-keystroke hot paths — all must be routed to get the win.
Several (`nav::fold_view` itself, `last_fully_visible_line`, `offset_at_cell`,
`rows_before_caret`→`screen_pos`) take **`&Editor`**, so the accessor MUST be `&self`
(interior-mutability cache) — a `&mut self` accessor would force a `&Editor`→`&mut Editor`
signature ripple across nav.

Design:
- **`fold_epoch` — robust against missed sites.** Add `fold_epoch: u64` on `FoldState`
  (`fold.rs:12`), and make every mutation of the `folded` set go through a `FoldState`
  method that bumps the epoch (only on a real change), so no call site can silently forget
  it. Direct `folds.folded.{insert,remove,retain,=}` pokes must be converted to those
  helpers. The full set of production mutation sites (plan must confirm exhaustive via grep
  — a missed one renders stale folded rows):
  - fold commands: `registry.rs:384` toggle, `:397` fold_all, `:411` unfold_all,
    `:509` unfold_ancestors_of;
  - anchor remap in `Buffer::apply`/`undo`/`redo`: `editor.rs:210,:235,:254`;
  - session restore: `app.rs:467`;
  - **reload / recovery wholesale replace (spec-review, folded): `save.rs:233,:277`**
    (`editor.active_mut().folds = prev_folds`). Note these run AFTER a full `Buffer`
    replacement, so the fresh `Buffer`'s caches are already `None` (miss → recompute →
    safe); still route the assignment through a helper so `fold_epoch` stays consistent.
- **Cache on `Buffer` (interior-mutability, `&self` accessor):**
  `fold_view_cache: RefCell<Option<(u64 /*blocks_generation*/, u64 /*fold_epoch*/, Rc<FoldView>)>>`,
  initialized empty at `Buffer` construction (so any `Buffer` replacement is safe by
  construction). `RefCell` is `Clone`/`Debug`-compatible (`Buffer` derives both) and safe
  (no `unsafe`); `Buffer` is not `Send` (see plan-confirm), so no `Sync` issue.
- **Accessor `Editor::active_fold_view(&self) -> Rc<FoldView>`:** borrow the `RefCell`; if the
  cached key `(active blocks_generation, active fold_epoch)` matches, clone + return the `Rc`;
  on a miss, compute `FoldView::compute` from the CURRENT `(folds, blocks, buffer)`, store
  `(blocks_generation, fold_epoch, Rc::new(view))`, drop the borrow, return the `Rc`. This is
  a **pure** memo (no mutation of editor state) — which is what lets it be `&self` and cover
  the `&Editor` nav call sites.
- **`reconcile` is a SEPARATE, generation-gated step (not on the FoldView miss path).**
  `FoldView::compute` filters `folds.folded` against the CURRENT sections, so a not-yet-pruned
  stale anchor (heading deleted) simply matches nothing — the FoldView is correct with or
  without a prune. So `reconcile` (the prune) is housekeeping, not a correctness input to the
  cache. Run it in `rebuild_downstream` (the `&mut` entry) gated on
  `blocks_generation != last_reconciled_generation` (a new `Buffer` field) → once per tree
  change, not every draw, preserving today's pruning behavior; route its `retain` through the
  epoch-bumping helper. This dissolves the round-1 "reconcile on the miss path / perpetual
  miss" concern entirely.
- Route ALL 18 call sites (the table above) through `active_fold_view(&self)`.

Net: **1 `sections` walk per (tree-or-fold change)**, not 5–6+ per keystroke (incl. the nav
hot paths); the `blocks.clone()` (`derive.rs:164`) and `buffer.clone()` (`:165`) done solely
to feed `reconcile`/`compute` are removed (the reconcile step and the accessor borrow directly).

## Component 3 — double-rebuild collapse via a computed layout key (shell)

`rebuild_downstream`'s visible-line loop (`derive.rs:200–225`) clears and rebuilds
`view.line_layouts` from scratch on every call — twice per keystroke (command + pre-draw)
and on every idle Tick. Its output depends entirely on:
`(blocks_generation, fold_epoch, view.scroll, view.scroll_row, view.area, text_width, active_line, view.mode, theme.heading_level_glyph)`.

Two inputs the first spec draft would have MISSED (spec-review, folded): `blocks_generation`
(not `blocks_version` — the reconcile merge changes the tree at the same version;
`role_at` at `derive.rs:216` reads `document.blocks`, so a merged tree must relayout), and
`theme.heading_level_glyph` — passed to `layout::layout` (`derive.rs:220`), it changes the
heading prefix width (`layout.rs:253`) and flips at runtime on theme apply (`editor.rs:640`).
Omitting either would leave a stale/incorrect layout cache with no on-demand render fallback.
(`line_text` and `role_at` — the buffer/tree inputs — are subsumed by `blocks_generation`,
which bumps on every text edit and every merge.)

- The work ABOVE the gated loop (`derive.rs:162–199`: fold-view fetch + scroll normalization,
  `view.scroll = first_line` at `:197`) stays UNCONDITIONAL — only the visible-line loop
  (`:200–225`) is gated. Capture `LayoutKey.scroll` **post-normalization** (the value the loop
  actually consumes), so the key reflects the true layout input.
- Compute that tuple as a `LayoutKey`; store the last one as `Buffer.layout_key: Option<LayoutKey>`.
- At the top of the layout section: compute the current key; if it equals `layout_key`,
  **skip** the layout loop entirely (`line_layouts` already matches); else run the loop and
  store the new key.
- **CRITICAL fix (spec-review, folded) — the gate must hold the OUTPUT invariant, not just
  the input key.** The "robust by construction" argument assumed the layout loop is the sole
  writer of `line_layouts`. It is NOT: `line_layouts.clear()` also happens EXTERNALLY at the
  **Resize handler** (`app.rs:1735`, clears every buffer) and the reload/recovery replace
  (`save.rs:238,:281`). A spurious same-dimensions `Resize` (tmux pane ops / focus deliver
  unfiltered SIGWINCH) would clear `line_layouts` while leaving `LayoutKey` unchanged → the
  gate skips → **blank editing rows, no render fallback** — the exact hazard the gate claims
  to prevent. Fix: maintain the invariant *"`layout_key == Some(k)` ⟹ `line_layouts` is valid
  for `k`"* at EVERY writer — introduce `Buffer::invalidate_layout()` that does
  `line_layouts.clear()` **and** `layout_key = None`, and route all three external clear sites
  through it (the gated loop's own clear at `derive.rs:208` stays, since it re-stores the key
  in the same pass). The plan must enumerate every `line_layouts` writer and confirm none
  clears it without nulling `layout_key`.
- With that invariant, the gate is genuinely safe: we never skip when either an input changed
  (key differs) OR the output was externally cleared (`layout_key` was nulled). Eliminates the
  redundant second pass *and* idle-Tick rebuilds.

Note the fold-reconcile + `FoldView` (Component 2) is separately cached, so a skipped
layout pass recomputes neither.

**Net per keystroke (all three):** ~1 tree walk + ~1 layout pass with O(log N) `role_at`,
down from ~5–6 walks + 2 deep clones + 2 layout passes.

## Testing

Correctness burden is "same output, faster," so the **existing render/nav/fold/layout suite
staying green is the primary net** (any regression there = a cache-key bug). Added tests:

- **F3 (core):** a differential property test — new `role_at` ≡ the pre-change linear result
  over byte positions. **The trees MUST be produced via the real producers** (random text →
  `full_parse`, and incremental-update chains) — NOT an arbitrary/synthetic `Block` tree
  strategy, which could generate unsorted/overlapping siblings that violate the very invariant
  the binary search rests on and cause spurious failures. Plus a test/`debug_assert` (outside
  the hot path) that children are ordered + non-overlapping at every level.
- **`blocks_generation` (core of the Critical fix):** it bumps on the parse-phase assignment
  AND on a reconcile-merge that replaces `blocks` at the same version — a test that simulates
  the merge (replace `document.blocks` via the merge path) asserts `blocks_generation`
  advanced, and that `active_fold_view` + the layout gate then **recompute** (the corrected
  tree renders). This is the regression guard for the exact bug spec-review caught.
- **F2:** `active_fold_view` returns the **same** `Rc` (`Rc::ptr_eq`) on an unchanged
  `(blocks_generation, fold_epoch)`; recomputes (new `Rc`) on a generation bump and on a fold
  toggle; still prunes stale fold anchors when headings are removed; and the cached
  `FoldView` equals a fresh `FoldView::compute` for the same state.
- **Component 3:** the layout-key gate **skips** when all inputs are unchanged (a second
  `rebuild_downstream` with no state change) and **recomputes** on a change to any of
  blocks_generation / fold_epoch / scroll / area / text_width / active_line / mode /
  **heading_level_glyph**; and `line_layouts` is correct (non-empty, right rows) after a
  change. Include an explicit case: flipping `theme.heading_level_glyph` alone invalidates.
- **CRITICAL-1 regression guard (spec-review, folded):** a **same-dimensions `Resize`**
  (`invalidate_layout` clears `line_layouts` but the `LayoutKey` inputs are unchanged) must
  NOT leave the rows blank — assert that after such a Resize + rebuild, `line_layouts` is
  repopulated (the `layout_key = None` forces the gated pass to run despite the matching key).
- **Call-count assertions (the proof-of-speedup, zero-dependency, no timing):**
  - `FoldView` reuse: after simulating a keystroke through both the command rebuild and the
    pre-draw rebuild, assert the two `active_fold_view` results are the same `Rc`
    (`Rc::ptr_eq`) — i.e. ≤1 compute per keystroke.
  - Layout-pass count: a `#[cfg(test)]` counter (e.g. `Buffer.layout_runs: u64` behind
    `cfg(test)`, or a test-local atomic incremented where the loop runs) asserts the layout
    loop runs **0** times on a no-change rebuild / idle Tick and **1** time per keystroke.
    **Use a mid-screen (non-scrolling) keystroke** — if the keystroke scrolls, `ensure_visible`
    legitimately changes `scroll`, so the pre-draw pass re-runs (key changed) and 2 is correct.
    The honest invariant is "≤1 layout pass per actual input change," not a raw "always 1."

## Decomposition (3 tasks, ascending risk)

1. **F3** — `role_at` binary search + the differential property test (core only). Independent;
   lands first.
2. **F2** — `blocks_generation` (the shared token: field + bump at both `document.blocks`
   writers) + `fold_epoch` on `FoldState` (with epoch-bumping mutation helpers) + the
   `Rc<FoldView>` cache + `active_fold_view` + route all call sites through it + fold
   `reconcile` into the miss path + tests (incl. the `Rc::ptr_eq` reuse assertion and the
   merge-bumps-generation regression guard).
3. **Component 3** — the computed `LayoutKey` gate (keyed on `blocks_generation` +
   `heading_level_glyph` among the full input set) on the layout pass + tests (incl. the
   layout-run counter and the heading_level_glyph-flip invalidation case). Builds on F2's
   `blocks_generation` + cache being present.

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
3. The exact, exhaustive set of `folded`-mutation sites that must bump `fold_epoch` — grep
   for EVERY production write to `folds.folded` / assignment of `folds`; confirm the table
   above is complete (incl. `save.rs:233,:277`) and that all pokes route through the
   epoch-bumping `FoldState` helpers. (Reconcile is now a separate generation-gated step, not
   on the FoldView miss path — so no perpetual-miss concern; confirm that restructure.)
4. The COMPLETE `nav::fold_view` call-site set (the 14 listed + the 4 non-nav sites) is routed
   through `active_fold_view(&self)`; confirm the `&self` accessor compiles at the `&Editor`
   sites (`nav::fold_view`, `last_fully_visible_line`, `offset_at_cell`, `rows_before_caret`→
   `screen_pos`) without a `&mut` ripple, and the `FoldView` methods used
   (`normalize_line`, `next_visible`, `prev_visible`, `is_hidden`, `line_at_ordinal`,
   `visible_ordinal`, `visible_count`) work through an `Rc` deref.
5. The precise `LayoutKey` field list matches every input the layout loop + `layout::layout`
   read — confirm the complete set is `blocks_generation`, `fold_epoch`, `scroll`,
   `scroll_row`, `area`, `text_width`, `active_line`, `mode`, `heading_level_glyph`, and
   nothing else the loop touches (other `theme`/`view`/`config` field feeding
   `line_text`/`role_at`/`layout::layout`; confirm `text_left`/gutter, tab width, wrap config
   are subsumed by `text_width` and don't affect `line_layouts` independently).
6. `blocks_generation` is bumped at EVERY `document.blocks` writer — confirm exactly two in
   production: the parse-phase assignment (`derive.rs:135`) and the reconcile-merge
   replacement (`reconcile.rs:66`, inside the `!= tree` branch) — and no third writer exists.
7. EVERY `view.line_layouts` writer is enumerated (the gated loop `derive.rs:208`, Resize
   `app.rs:1735`, reload/recovery `save.rs:238,:281`) and every EXTERNAL clear routes through
   `Buffer::invalidate_layout()` (clear + `layout_key = None`) — the CRITICAL-1 invariant.
8. All fold-view CONSUMERS rebuild before consuming (no path reads `active_fold_view` between
   an edit and the next `rebuild`, which would return a pre-edit cached view) — confirm the
   command/mouse/theme paths rebuild first (M3), so the cache introduces no transient
   divergence from today's recompute-each-time behavior.
