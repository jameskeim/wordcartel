# F4: in-place consume-and-splice (kill the per-keystroke splice allocations) — design

**Status:** Codex round 1 folded (take_blocks via mem::replace+empty_tree; zero-length prose qualified; alloc claim softened); re-review pending
**Date:** 2026-07-03
**Effort:** F4 (the deferred per-keystroke O(#nodes) splice-clone — the allocation floor F1's Case B bottoms out on; recommended resolution "(f)" from the Fable5 F4 analysis)

## Context

The incremental block-tree parser's splice rebuilds the ENTIRE top-level block Vec on every
edit, even a cheap `Local` one. In `wordcartel-core/src/block_tree.rs`,
`incremental_update_instrumented_src`'s splice (block_tree.rs:956-1003):
- deep-clones every "before" block (`result_children.push(b.clone())`, :959);
- deep-clones the freshly-reparsed subtree (`.extend(reparsed.root.children.iter().cloned())`,
  :963) — a **pure waste**: `reparsed` is an owned local, dead after this line;
- `shift_block`-clones every "after" block (:975; `shift_block` :1269 recursively allocates a
  fresh `Vec` per node while rewriting `span` by `delta`);
- grows `result_children` from an empty `Vec` (no `with_capacity`) → reallocations.

**F4 is a constant-factor ALLOCATION problem, not an asymptotic one (Fable5).** Realistic prose
(even a book-length 1 MB doc, ~2–5k mostly-leaf blocks) costs tens of µs here — invisible. The
only shape that genuinely janks is editing ABOVE a large mass of nested container nodes (~150k+
after-nodes), where `shift_block`'s allocation-per-node → ~5–15 ms. (The giant-single-list case
is F1's Case B — parse-dominated `WidenToEnd`, orthogonal; F4's fix does not rescue it.) And the
update function already does ~6 linear scans over `tops` plus downstream full-tree walks
(`heading_starts`, `FoldView` sections) per keystroke — so making the splice *sublinear* buys
nothing observable while those stay O(#nodes). The honest target is the **allocation traffic**,
not big-O.

Approach (f) — **change OWNERSHIP, not the `Block` type**: take `old_tree` by value and edit its
`root.children` in place. This eliminates 100% of the splice's per-keystroke allocations
(before-clones, the :963 double-copy, the `shift_block` allocations, the `result_children`
growth) with **near-zero blast radius** — no `Block` representation change, so `role_at`,
`FoldView`, `nav`, `render`, `top_level()` and the oracle's `==` are all untouched. Worst
pathological case drops from ~5–15 ms of allocator churn to ~0.1–1 ms of span arithmetic.
Rejected alternatives (Rc/persistent-vector/relative-spans/sum-tree) all fail the after-block
span-shift half and/or require re-deriving a fuzz-hardened module — see the F4 analysis.

## Goals

- Eliminate the per-keystroke splice ALLOCATIONS: before-blocks moved (not cloned), reparsed
  moved (not cloned), after-blocks shifted in place (no per-node allocation), no fresh
  `result_children` Vec.
- **Byte-identical behavior** — the produced tree, `WidenReason`, `reparsed_bytes`, and all
  early-return paths are exactly today's. The existing oracle + F2 fuzz suite is the correctness
  proof (they assert the output `== full_parse`).
- Single source of truth: ONE splice implementation (the owned path); the `&`-API delegates.

## Non-goals

- ONLY the splice. NOT the ~6 region scans over `tops`, NOT the downstream `heading_starts` /
  `FoldView` full-tree walks — those keep the update O(#nodes) regardless and are separate
  efforts.
- NOT F1's Case B (a single > MAX_SYNC_WIDEN_BYTES container; parse-dominated, orthogonal).
- NO `Block`/`BlockTree` representation change (no `Rc`, no persistent vector, no relative
  spans); no new dependency; `#![forbid(unsafe_code)]` in core intact.
- No behavior change of any kind — this is a pure allocation-elimination refactor.

## Component 1 — the owned entry + in-place splice (`wordcartel-core/src/block_tree.rs`)

### API shape (A1 — owned is the single source of truth)

- Add `pub fn incremental_update_instrumented_src_owned<S: TextSource>(old_tree: BlockTree,
  old_src: &S, edit: &Edit, new_src: &S) -> UpdateOutcome` holding the ENTIRE current
  region-computation + splice logic (moved out of the existing `&`-entry). The region
  computation borrows `&old_tree` (`old_tree.top_level()` / `tops`) exactly as today; the tree is
  consumed only at the splice.
- Rewrite the existing `pub fn incremental_update_instrumented_src<S>(old_tree: &BlockTree, …)`
  to delegate: `incremental_update_instrumented_src_owned(old_tree.clone(), old_src, edit, new_src)`.
  All the other `&`-entry points (`incremental_update` :498, `incremental_update_instrumented`
  :510, `incremental_update_rope` :525, `incremental_update_src` :535) already funnel through
  `incremental_update_instrumented_src`, so they transparently exercise the owned path. Tests /
  the oracle / the F2 fuzz target keep their `&`-signatures (their clone is irrelevant — not hot)
  and become the free regression net for the owned splice.
- Optionally add `incremental_update_owned(old_tree, …) -> BlockTree` (`.tree` of the owned
  instrumented) if the shell prefers the tree-only form; the shell can also use the instrumented
  owned directly and take `.tree`.

### The in-place splice (replaces block_tree.rs:956-1003)

`tops` is sorted by `span.start` + non-overlapping (straddle repair guarantees it), so the
current per-element classification (before / overlapping-dropped / after) is a CONTIGUOUS 3-way
partition. Reproduce it with two `partition_point`s over `&old_tree.root.children` (borrow before
the move), then edit the moved Vec in place:

```
// borrow phase (before consuming old_tree):
let splice_lo = tops.partition_point(|b| b.span.end <= region_old_start);          // before | overlap boundary (:958)
let splice_hi = tops.partition_point(|b| !(b.span.start >= region_old_end && b.span.end > region_old_end)); // overlap | after (:974)
let reparsed_len = reparsed.root.children.len();
// consume + splice in place:
let mut children = old_tree.root.children;
children.splice(splice_lo..splice_hi, reparsed.root.children);   // before-blocks [0,splice_lo) stay put; overlap dropped; reparsed MOVED in
// shift the after-blocks (now at splice_lo+reparsed_len ..) in place, no allocation:
let after_seam = splice_lo + reparsed_len;
for b in &mut children[after_seam..] { shift_in_place(b, delta); }
```

- `splice_lo` = number of before-blocks (`span.end <= region_old_start`). Monotone: before-blocks
  form the true-prefix.
- `splice_hi` = number of non-after blocks; the after-predicate `span.start >= region_old_end &&
  span.end > region_old_end` is monotone-false-then-true over the sorted blocks. The zero-length
  boundary block (`:974` carve-out: `span.start == span.end == p == region_old_end`) is `false` for
  the after-predicate → dropped in the `[splice_lo, splice_hi)` middle, **when `region_old_start <
  region_old_end`**. (Codex round 1 qualification: if `region_old_start == region_old_end == p`, a
  `p..p` block satisfies `span.end <= region_old_start` → it is a BEFORE block under both the
  current loop AND `splice_lo` — so the predicates still match the current per-element loop exactly;
  only the "always dropped" prose was overbroad.)
- `shift_in_place(b: &mut Block, delta: isize)` mutates `b.span` (`start`/`end` by `delta`, same
  arithmetic as `shift_block` :1270-1272) and recurses over `&mut b.children` — same O(after-nodes)
  time, NO per-node allocation. Replaces `shift_block` (delete it; the splice is the sole caller —
  plan-confirm). (The outer `children` Vec may reallocate ONCE if the reparsed replacement changes
  its length beyond capacity — bounded/amortized, not the per-node O(#nodes) allocation of today.)

### Seam consistency — unchanged, same order

`before_count = splice_lo`; `after_seam = splice_lo + reparsed_len`. The `merge_at` closure
(:989-993) reads `children[i-1]` / `children[i]` at the two seams. The after-blocks are shifted
BEFORE the seam check (as today: `shift_block` at :975 runs before the seam check at :989), so
`paragraph_absorbs_next` sees new-coordinate spans. If a seam fires → `full_parse_src(new_src)`
returning `NoOverlapFull` (drops the mutated `children`) — identical to today. Else wrap:
`BlockTree { root: Block { kind: Document, span: 0..new_src.len(), children } }`.

### Early-return / full-reparse paths

Every path that returns `full_parse_src` (the `NoOverlapFull` no-overlap exits, the front-matter
full reparse, the seam-guard, F1's Case-B `WidenToEnd` which reparses to EOF then splices — same
in-place logic with the whole region) simply drops the owned `old_tree` (or splices as above) and
returns the same outcome as today. The owned tree is consumed by value; no path needs it after
the region computation.

## Component 2 — shell wiring + the perf-proof test (`wordcartel/src/derive.rs`, `editor.rs`)

- Add `Document::take_blocks(&mut self) -> BlockTree` (editor.rs, beside `set_blocks` :90).
  `BlockTree` has NO `Default` (it derives only `Clone/PartialEq/Eq`, block_tree.rs:164), so use
  `std::mem::replace(&mut self.blocks, wordcartel_core::block_tree::empty_tree(self.buffer.len()))`
  — returns the current tree, leaves a valid empty tree spanning the doc (block_tree.rs:333). This
  does NOT bump `blocks_generation` (it's a take, not a semantic write; the subsequent `set_blocks`
  bumps). The placeholder is held only between `take_blocks` and the merge — never read.
- In `derive.rs`'s incremental branch, replace the `&`-call. Today it passes
  `editor.active().document.blocks()` (a borrow) into the `panicx::catch` closure (derive.rs:134;
  `catch` wraps the `FnOnce` in `AssertUnwindSafe`, panicx.rs:27, so moving the owned tree in
  compiles). New: BEFORE the closure, `let old = editor.active_mut().document.take_blocks();` then
  move `old` into the closure calling `incremental_update_instrumented_src_owned(old, &old_rope,
  edit, &new_rope_ref)`; on `Ok` → `set_blocks(outcome.tree)` (as today), on `Err` → the existing
  `apply_parse_result(editor, new_len, Err(msg))` degraded-empty-tree fallback (derive.rs:283).
- **Panic-safety (behavior-identical):** `take_blocks` pulls out a tree that is about to be
  replaced regardless — by `set_blocks(outcome.tree)` on success, or by the empty-tree fallback on
  a parse panic. So a mid-splice panic loses nothing the current code wouldn't (the current code
  also discards the old tree on that path). The document briefly holds an empty tree between
  `take_blocks` and the merge — invisible (single-threaded; no read between).

### The perf-proof test (pointer-identity, allocator-free)

Add a `#[cfg(test)]` test (core, block_tree.rs tests) that pins the zero-copy property: build a
tree with a before-block whose `children` Vec is non-empty; capture that before-block's
`children.as_ptr()`; run an incremental edit whose region is AFTER that block (so it is a
"before" block, untouched by the splice); assert the same before-block's `children.as_ptr()` is
UNCHANGED — proving before-blocks are moved, not deep-cloned. (A deep clone would allocate a new
child Vec → different pointer.) This guards the win against a future refactor silently
reintroducing a clone, without a counting global allocator.

## Testing

- **Correctness — FREE:** the existing `block_tree_oracle.rs` suite (`check`, `assert_all_paths_agree!`,
  the chain macro), the deterministic regressions, and the F2 fuzz target (`incremental_equals_full`
  :1300) all assert `incremental == full_parse` (byte-identical trees). Since the `&`-wrappers now
  delegate to the owned path, this entire harness covers the owned splice with ZERO new correctness
  tests. Any classification/shift/seam drift fails an existing oracle assertion.
- **Perf pin:** the pointer-identity zero-copy test above.
- **Shell:** the existing `derive.rs`/`app.rs` tests + the (new this campaign) e2e journeys stay
  green (the `take_blocks` wiring is behavior-preserving).

## Decomposition (2 tasks)

1. **Core** — `incremental_update_instrumented_src_owned` + the in-place splice (2 partition
   points + `Vec::splice` + `shift_in_place` + seam order) + rewrite the `&`-entry to delegate via
   `old_tree.clone()`. The existing oracle + F2 fuzz suite stays green (byte-identical output) —
   the correctness proof.
2. **Shell wiring + perf pin** — `Document::take_blocks` + the `derive.rs` owned call (panic-safety
   preserved) + the pointer-identity zero-copy test. Full suite green.

## Global constraints

- `#![forbid(unsafe_code)]` in core intact; no new dependency; no `Block`/`BlockTree` repr change.
- `cargo test -p wordcartel-core -p wordcartel` green (the oracle is the correctness net);
  workspace clippy **deny** gate clean; no `cargo fmt`; house style (em-dash `—`).
- Hot path: the splice becomes O(after-nodes) TIME while removing the per-node/deep-clone
  allocations (was O(#nodes) allocations); the outer children Vec may reallocate at most once on a
  net length change (bounded). No new O(document) work; the sibling scans are unchanged (out of scope).

## Plan-confirms (resolve during the implementation plan, against real source)

1. **Partition-point exactness (the load-bearing point).** Confirm the two `partition_point`
   predicates yield the EXACT set the current per-element loop classifies — before = `span.end <=
   region_old_start`; after = `span.start >= region_old_end && span.end > region_old_end`; the
   dropped middle = everything else INCLUDING the zero-length boundary block. Verify the
   after-predicate is monotone over the sorted `tops` (all-false then all-true) so `partition_point`
   is valid, and that `Vec::splice(splice_lo..splice_hi, reparsed)` drops exactly the current
   overlapping+zero-length set.
2. **The `&`-wrapper delegation** — confirm every current `&`-entry point funnels through
   `incremental_update_instrumented_src`, so rewriting just that one to `…_owned(old.clone(), …)`
   preserves all public signatures + routes the oracle/fuzz through the owned path. Confirm
   `BlockTree: Clone`.
3. **`shift_block` sole caller** — grep; confirm the splice is the only caller so replacing it with
   `shift_in_place` (or keeping both) is clean. Confirm `shift_in_place`'s span arithmetic +
   recursion matches `shift_block` exactly (byte-identical shifted tree).
4. **`take_blocks` via `mem::replace` + `empty_tree`** (Codex round 1: `BlockTree` has no `Default`,
   block_tree.rs:164) — confirm `empty_tree(self.buffer.len())` (block_tree.rs:333) is the right
   placeholder + import path, that `take_blocks` returns the real tree + leaves a valid empty one,
   and that it does NOT bump `blocks_generation` (only `set_blocks` does, editor.rs:90).
5. **The exact `derive.rs` incremental branch shape** — the `panicx::catch` closure (derive.rs:134,
   `AssertUnwindSafe`), the `Ok`→`set_blocks` / `Err`→`apply_parse_result` (derive.rs:283) arms —
   confirm moving `take_blocks()`'s owned result into the closure compiles (ownership) and the
   panic-safety argument holds (the taken tree was about to be replaced regardless).
6. **Seam order** — confirm the current code shifts after-blocks (:975) BEFORE the seam check
   (:989), so the in-place version must shift-then-seam-check to preserve `paragraph_absorbs_next`
   reading new-coordinate spans.
