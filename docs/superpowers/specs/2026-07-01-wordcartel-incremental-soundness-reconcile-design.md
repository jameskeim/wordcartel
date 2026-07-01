# Deep incremental-soundness via eventual-consistency reconcile — design

**Status:** approved design (pre-spec-review)
**Date:** 2026-07-01
**Effort:** incremental-soundness reconcile (M7 residual (a); pre-Effort-P)

## Context

M7's F2 differential fuzz oracle (`incremental_equals_full`) keeps finding
block-tree divergences in the tail: on adversarial edits, the *incremental* block
tree (`incremental_update`) disagrees with a *full* reparse (`full_parse`). The
impact is cosmetic — wrong block roles → wrong styling / heading-conceal / folding
/ outline — **not** data-loss and **not** a panic.

The incremental engine (`block_tree.rs`, ~775 lines of engine + hazard guards)
reasons about **top-level blocks only**: every widen/fallback decision inspects
`old_tree.top_level()`. It does not model nested-container extents or list
loose/tight, and loose↔tight is a **non-local** effect (one blank line inside a
list changes every item's children). The architecture is therefore structurally
"whack-a-mole": compute a region → parse locally → run O(1)/O(region) guards to
detect "could this be wrong?" → fall back. Each of ~12 guards was added reactively
(the 7 M7 bugs). There is no soundness *theorem* — only sound-by-case
construction — and the nested/loose tail is where localized detection cannot win.

### The strategic decision (A over B)

Two ways to close the tail were considered:
- **B — perfect soundness:** model loose/tight + nested-container extents, add the
  missing widen triggers, ideally behind a *proven* "safe local reparse"
  invariant. Delivers the strong theorem (tree *always* equals `full_parse`) but is
  an open-ended modeling investment, adds complexity, and pushes more edits onto
  O(document) fallbacks (a responsiveness cost) — for correctness on adversarial
  inputs a user rarely types.
- **A — eventual consistency (CHOSEN):** keep the incremental engine as the fast,
  common-case-correct live-frame path, and add a **debounced background full
  reparse that reconciles the tree when typing pauses**. Delivers a *weaker but
  real* theorem — **correct-at-rest** — cheaply, on the existing job substrate,
  and composes as a safety net under any future B work.

This effort implements **A**. It does not attempt B (explicit non-goal).

## The guarantee (the deliverable)

**Convergence theorem:** *in the absence of edits to the active buffer for at least
`RECONCILE_DEBOUNCE_MS`, `document.blocks` converges to `full_parse(text)`.*

Precisely: the reconcile job computes `full_parse` of the buffer at version `V`; its
result is merged **iff** the buffer is still at `V` at merge time (version-discard).
So once editing stops, the next idle deadline dispatches a reconcile that lands and
makes the tree exactly the full parse. During active typing the tree remains the
incremental approximation (correct in the common cases via the existing guards,
possibly diverged on the adversarial tail) — self-healing within a debounce
interval of the last keystroke. This is **not** the strong always-correct theorem
(that is B); it is a convergence/eventual-consistency theorem.

A structural bonus: each reconcile **re-bases the incremental engine on a
known-correct tree**, so divergence cannot accumulate across edits.

## Goals

- Implement the convergence theorem above for the active buffer.
- Reuse the existing async job substrate (modeled on the diagnostics job) — the
  reconcile never blocks the input loop, at any document size (M5 permits up to
  64 MiB, where a synchronous full parse would stall for seconds).
- Compose with the M4 worker panic isolation: a reconcile that hits the upstream
  `pulldown-cmark` panic is caught on the worker and the round is skipped, tree
  unchanged, no crash.
- No behavior change to the incremental live-frame path.

## Non-goals

- No new incremental-engine guards; no loose/tight or nested-container modeling;
  no principled "safe local reparse" invariant (all of that is B — a separate
  future effort).
- No "bounded staleness during sustained typing" cap — the theorem is
  correct-*at-rest*. (A fast typist editing a pathological region continuously
  keeps the incremental tree until they pause; that is accepted.)
- Reconcile targets the **active buffer only** (background buffers full-parse on
  activation already, so they are correct-on-switch).
- No fix to `pulldown-cmark` itself (residual (b) stays its own optional effort).

## Architecture & components

Entirely shell-side plus surfacing one existing core value; `full_parse` is already
pure and its `BlockTree` result is `Send`, so it runs on a worker unchanged.

### 1. `blocks_maybe_stale` flag (per `Document`)
A `bool` on `Document`. Set **`true`** in `derive::rebuild` when an incremental
update takes a can-diverge path (`WidenReason::Local` or `WidenReason::WidenToEnd`).
Set **`false`** whenever a *full* parse establishes the tree: the sync
`WidenReason::NoOverlapFull` path, initial load / undo / redo (which already
`full_parse`), and a reconcile-job merge. Meaning of `false`: `document.blocks ==
full_parse(text)` as of the last write, and no edit since.

To read the `WidenReason`, `derive::rebuild` uses the **instrumented** incremental
entry (`incremental_update_instrumented_*`, which returns `UpdateOutcome { tree,
reason, reparsed_bytes }`) instead of the plain wrapper. (Plan-confirm the exact
rope-variant name.)

### 2. Reconcile deadline (main loop)
Mirrors the diagnostics deadline. When the active buffer has been idle (no edit)
for `RECONCILE_DEBOUNCE_MS` **and** `blocks_maybe_stale` is set **and** no reconcile
is already in flight for it, dispatch the reconcile job. `RECONCILE_DEBOUNCE_MS` is
a tunable const (~150 ms — long enough not to fire mid-burst, short enough to feel
like an instant self-heal); the plan aligns it with the existing debounce consts
(swap `T_IDLE_MS`, the diagnostics debounce). The deadline folds into the existing
`next_deadline` computation in `run`.

### 3. Reconcile job (new `JobKind`, diagnostics-shaped)
Snapshots the active buffer's rope (O(1) ropey clone) + version; on a worker runs
`full_parse_rope(&rope)` and returns the `BlockTree`. Properties:
- **Coalescible + version-discarded** (only the latest reconcile per buffer matters).
- **Panic-isolated:** it runs inside the worker's `panicx::catch` (M4/BUG-1). A
  pulldown panic → the job surfaces as panicked → reconcile skipped this round,
  tree unchanged, no crash. Best-effort by design.

### 4. The merge (main thread, version-checked)
On the reconcile result:
- If the active buffer moved on (version ≠ job version) or the buffer was closed →
  **discard** (a fresh reconcile re-schedules on the next idle deadline).
- Else **diff** the fresh full tree against `document.blocks`:
  - **Equal** (incremental was already correct — the common case): clear
    `blocks_maybe_stale`; **no redraw**.
  - **Different** (a real divergence caught): replace `document.blocks`, re-run the
    downstream-of-tree consumers (fold reconcile + outline + layout) **without
    re-parsing** (§ integration seam), clear `blocks_maybe_stale`, and redraw.

## Integration seam (the one real subtlety)

The merge must refresh folds/outline/layout from the new tree **without triggering
a redundant full parse** — otherwise the per-iteration `derive::rebuild` would
re-`full_parse` on the following loop turn. The design factors the "given a
`BlockTree`, refresh the fold view + outline + layout cache" portion of
`derive::rebuild` into a reusable function that the reconcile merge calls after
assigning `document.blocks`. Plan-confirm: the exact `derive::rebuild` structure,
how it already avoids re-parsing on non-edit iterations, and the clean seam to
extract.

## Error handling summary

| Situation | Response |
|---|---|
| Reconcile job panics (pulldown residual) | worker `catch` → job panicked → skip round, tree unchanged, no crash |
| Buffer version advanced / buffer closed before merge | discard; re-schedule next idle |
| Fresh tree == current tree | clear flag, no redraw |
| Fresh tree != current tree | replace + refresh downstream + redraw + clear flag |

## Testing

- **Convergence (the theorem):** take an input where the incremental tree diverges
  from `full_parse` (reuse a fuzzer-found / constructed case); assert the divergence
  exists (incremental tree ≠ full tree), simulate the reconcile merge, assert
  `document.blocks == full_parse(text)`.
- **Flag transitions:** `blocks_maybe_stale` set after a `Local`/`WidenToEnd`
  update; cleared after a full parse / undo / redo / reconcile merge.
- **Version-discard:** a reconcile whose version is stale is dropped and does not
  clobber the newer tree.
- **Merge-diff no-op:** when the incremental result already equals `full_parse`, the
  merge changes nothing and issues no redraw.
- **Panic isolation:** a reconcile job that panics leaves the tree unchanged (reuse
  the M4 panicked-job test pattern).
- **Deadline:** unit-test the reconcile deadline/debounce computation (given
  `last_edit_at`, `blocks_maybe_stale`, now) like the diagnostics deadline tests.
- No new tests are needed in `wordcartel-core` (the incremental engine is unchanged).

## Plan-confirms (resolve during the implementation plan, against real source)

1. The instrumented incremental rope entry that returns `UpdateOutcome`/`WidenReason`
   (so `derive::rebuild` can set `blocks_maybe_stale`), and confirm `NoOverlapFull`
   is the only reason that guarantees tree == `full_parse`.
2. The exact `derive::rebuild` structure — how it avoids re-parsing on non-edit
   loop iterations, and the seam to extract the "downstream-of-tree" refresh
   (fold/outline/layout) that the merge reuses.
3. The diagnostics job as the template: `JobKind`, `ResultClass`/coalescing,
   version-discard, `next_deadline` folding, the `Msg::*Done` + `apply_*_done`
   merge path — to mirror for reconcile.
4. `RECONCILE_DEBOUNCE_MS` value + where it lives, aligned with `swap::T_IDLE_MS` /
   the diagnostics debounce.
5. Where `blocks_maybe_stale` and the "reconcile in flight" state live on
   `Document`/`Editor`, and the `Document` fields (`version`, `blocks`, `last_edit_at`).
6. That `full_parse_rope` is pure/`Send`-safe to run on a worker with an owned rope
   snapshot, and `BlockTree` is `Send`.
