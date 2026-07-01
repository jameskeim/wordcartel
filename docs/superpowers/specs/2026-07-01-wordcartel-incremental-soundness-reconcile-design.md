# Deep incremental-soundness via eventual-consistency reconcile — design

**Status:** spec-review clean (Codex READY FOR PLANNING, no open findings)
**Date:** 2026-07-01
**Effort:** incremental-soundness reconcile (M7 residual (a); pre-Effort-P)

## Context

This effort began as "close M7 residual (a)" — the F2 fuzz oracle
(`incremental_equals_full`) keeps finding block-tree divergences on the adversarial
tail (nested containers, loose/tight lists), where the *incremental* tree disagrees
with a *full* reparse. That divergence is cosmetic (wrong styling/conceal/fold/
outline, not data-loss/panic), and the incremental engine is structurally
"whack-a-mole" there: it reasons about **top-level blocks only** and does not model
nested extents or loose↔tight (a non-local effect — one blank line inside a list
changes every item), so localized detection cannot win the tail.

**But the spec review surfaced the real picture, and it reframes the effort.**
`derive::rebuild` runs on **every draw** (`app.rs:2136`, unconditional pre-draw),
and `pre_edit_rope`/`last_edit` are `.take()`n — so they are `Some` only on the one
draw right after an edit. On that edit draw it does an incremental update; on
**every other draw** (cursor move, idle `Tick`, job completion, scroll, resize) it
falls to the `_ =>` arm and calls **`full_parse_rope` — a full O(document) parse**
(`derive.rs:98–106`). Two consequences:

1. **The app is already eventually-consistent by brute force.** A divergence in an
   incremental result is corrected on the very next draw's full parse, so M7
   residual (a) flashes for at most one frame and self-heals. Its real user impact
   today is near-zero.
2. **The actual problem is performance.** A settled buffer pays an O(document) full
   parse on essentially every frame — a serious large-doc responsiveness cost, and
   responsiveness is the project's #1 priority.

So the effort's true shape is: **stop the per-draw full parse (the responsiveness
win) — and, because that removal also removes the accidental per-frame self-heal,
add a debounced async reconcile to preserve eventual consistency (correctness).**
The two are coupled: the perf fix alone would regress soundness; the reconcile
holds the line. This is exactly Option A — we are replacing *expensive* brute-force
eventual-consistency (full parse every frame) with *cheap* eventual-consistency
(incremental per edit + one async full parse per typing-pause).

**Honest tradeoff:** today's code is accidentally *always*-correct (it full-parses
every frame), just at O(document)/frame. This effort trades that down to
*correct-at-rest* in exchange for responsiveness. The incremental parser exists
precisely to keep per-keystroke cost O(region); "just full-parse on every edit
instead" would defeat it (O(document)/keystroke while typing). A keeps both the
per-keystroke path and the non-edit-draw path cheap.

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

- **Eliminate the per-draw full parse (the primary win).** `derive::rebuild` must
  reparse only when there is a pending edit; a non-edit draw refreshes the layout /
  fold view from the *existing* `document.blocks` without re-parsing. This removes
  an O(document)-per-frame cost from settled large buffers.
- Implement the convergence theorem above for the active buffer, so removing the
  per-draw parse does **not** regress correctness (the reconcile restores the
  eventual-consistency the per-frame parse was accidentally providing).
- Run the reconcile on the async job substrate (the **Executor** `JobKind` path) —
  it never blocks the input loop at any document size (M5 permits up to 64 MiB,
  where a synchronous full parse stalls for seconds).
- Compose with the M4 worker panic isolation: a reconcile that hits the upstream
  `pulldown-cmark` panic is caught on the worker and the round is skipped, tree
  unchanged, no crash.
- No change to the incremental per-*edit* path (only the non-edit draw path changes).

## Non-goals

- No new incremental-engine guards; no loose/tight or nested-container modeling;
  no principled "safe local reparse" invariant (all of that is B — a separate
  future effort).
- No "bounded staleness during sustained typing" cap — the theorem is
  correct-*at-rest*. (A fast typist editing a pathological region continuously
  keeps the incremental tree until they pause; that is accepted.)
- Reconcile targets the **active buffer only**. (Post-refactor a switched-to buffer
  reuses its existing tree — its `version` is unchanged if it wasn't edited since —
  and if that tree was left maybe-stale, the reconcile converges it once it is
  active and idle. So background buffers are correct-*at-rest* after a switch, via
  the same flag+reconcile, not full-parsed on switch. The per-buffer stale flag
  persists across switches, so convergence is preserved.)
- No fix to `pulldown-cmark` itself (residual (b) stays its own optional effort).

## Architecture & components

Entirely shell-side plus surfacing one existing core value; `full_parse` is already
pure and its `BlockTree` result is `Send`, so it runs on a worker unchanged.

### 0. `derive::rebuild` refactor (the perf fix + the reconcile's precondition)
Today `rebuild` (`derive.rs:82`) reparses on every draw: incremental when a pending
edit is present (`pre_edit_rope`/`last_edit` both `Some`), else `full_parse_rope`
(`derive.rs:98–106`), and it is called unconditionally pre-draw (`app.rs:2136`).
Refactor it into two phases, gated by **version memoization** (a per-buffer
`blocks_version` = the document version the current `blocks` were built for):
- **Parse phase (only when `document.version != blocks_version` — i.e. the text
  actually changed since the tree was built):**
  - if a pending incremental edit is present (`pre_edit_rope`/`last_edit` both
    `Some`) → incremental update; set the stale flag from its `WidenReason` (§1).
    This is normal keystrokes AND filter/transform/paste — all route through
    `Buffer::apply` (`editor.rs:189`), which sets the incremental state (`app.rs:306`
    filter, `transform.rs:166`, `app.rs:770` paste);
  - otherwise (a text change with NO incremental info — undo/redo, which bump the
    version and explicitly clear `pre_edit_rope`/`last_edit` at `editor.rs:222`/`:241`)
    → **full parse** (correct, and NOT a per-keystroke hot path); clear the stale flag;
  - assign `document.blocks`; set `blocks_version = document.version`.
- **Downstream phase (always):** given the *current* `document.blocks`, refresh the
  fold view + outline + layout cache. Runs every draw; does NOT reparse.

So a non-edit draw (`version == blocks_version`: cursor move, `Tick`, jobdone,
scroll, resize) does the downstream phase only — O(visible), no `full_parse`. The
version gate is what preserves undo/redo/filter correctness (those change the
version, so they still reparse) while removing the redundant per-frame parse.
This is both the responsiveness win and the precondition that makes the async
reconcile meaningful (without it, the per-draw full parse would already keep the
tree correct, at O(document)/frame, and the reconcile would be pointless). The
downstream phase is a reusable function the reconcile merge (§4) also calls after
it swaps in a fresh tree. Behavior change: a non-edit draw now renders from the
*existing* (possibly-diverged) tree rather than a freshly-full-parsed one —
converged by the reconcile at rest. (Plan-confirm the exact fold/outline/layout
seam in `rebuild`.)

### 1. `blocks_maybe_stale` flag (per `Document`)
A `bool` on `Document`. Set **`true`** in `derive::rebuild` when an incremental
update takes a can-diverge path (`WidenReason::Local` or `WidenReason::WidenToEnd`).
Set **`false`** whenever a *full* parse establishes the tree: the sync
`WidenReason::NoOverlapFull` path, initial load / undo / redo (which already
`full_parse`), and a reconcile-job merge. Meaning of `false`: `document.blocks ==
full_parse(text)` as of the last write, and no edit since.

To read the `WidenReason`, `derive::rebuild` must switch from the plain
`incremental_update_rope` (which discards instrumentation via
`incremental_update_src(...).tree`, `block_tree.rs:521`) to
**`incremental_update_instrumented_src`** (`block_tree.rs:540`, returns
`UpdateOutcome { tree, reason, reparsed_bytes }`) — there is no rope-specific
instrumented wrapper, so it takes the rope→str conversion the plain wrapper does.
Confirmed sound (Codex spec review): `NoOverlapFull` is the ONLY reason whose result
is guaranteed byte-equal to `full_parse` (every `NoOverlapFull` return calls
`full_parse_src(new_src)`); `WidenToEnd` preserves "before" blocks verbatim and
`Local` is a splice — both must be treated as maybe-stale. So "stale unless
`NoOverlapFull`" has no hole.

### 2. Reconcile deadline (main loop) — self-armed `reconcile_due_at`
Scheduling is keyed off a per-buffer **`reconcile_due_at`**, NOT `last_edit_at`
(which the reduce loop updates only for the *active* buffer, `app.rs:1775`, and so
misses text that lands in *inactive* buffers — scratch append `scratch.rs:21`,
inactive transform merge `transform.rs:176`, inactive paste `app.rs:776`). Instead,
**whenever §0's parse phase produces a maybe-stale tree** (an incremental update on
the now-active buffer — including the parse that runs the first time a
stale-from-inactive-mutation buffer is drawn after a switch, `workspace.rs:39`), set
`reconcile_due_at = now + RECONCILE_DEBOUNCE_MS`. Each subsequent such rebuild pushes
it back (re-debounce). The main loop dispatches the reconcile when `now >=
reconcile_due_at` **and** `blocks_maybe_stale` **and** no reconcile is already in
flight for the buffer. This mirrors the diagnostics `DiagStore.recheck_due_at` /
`in_flight_version` pattern exactly, and — because it is armed by the *parse phase*
rather than by an active-buffer edit event — it correctly covers staleness whenever
it is first observed, including at switch time. `RECONCILE_DEBOUNCE_MS` is a tunable
const (~150 ms), aligned with `swap::T_IDLE_MS` / the diagnostics debounce; the
deadline folds into the existing `next_deadline` computation in `run` (`app.rs:2089`).

### 3. Reconcile job — the **Executor** `JobKind::Reparse` path (NOT diagnostics)
Correction from the spec review: the diagnostics job does NOT use the Executor — it
spawns its own thread and sends `Msg::DiagnosticsDone` directly. We take the
**Executor** path instead (it gives worker panic isolation + version-discard for
free) and reuse only the diagnostics *debounce/in-flight shape*.
- New `JobKind::Reparse`, `ResultClass::BufferLocal`, **coalescible** in `is_stale`
  (`jobs.rs:66`) so a superseded reconcile is dropped.
- The job snapshots the active buffer's rope (O(1) ropey clone) + `version`; on the
  worker runs `full_parse_rope(&rope)` → owned `BlockTree`. `ropey::Rope` and
  `BlockTree` are `Send`; `full_parse_rope` is pure.
- Result flows via the existing `Msg::JobDone(JobOutcome)` path (no new `Msg`).
- **Panic-isolated:** the Executor worker wraps `job.run` in `panicx::catch`
  (`jobs.rs:121`); a pulldown panic → `JobOutcome::Panicked` → **a `Reparse` arm in
  `apply_panic`** (`app.rs:198`) that clears the reconcile in-flight state and
  leaves `document.blocks` unchanged. (Rust exhaustiveness forces both the
  `is_stale` and `apply_panic` `Reparse` arms — the plan must add them.)

### 4. The merge — `JobResult::merge` (main thread, version-checked)
The reconcile's `JobResult.merge` runs on the main thread:
- If the active buffer moved on (its `version` ≠ the job's) or was closed →
  **discard**; clear in-flight (a fresh reconcile re-schedules next idle).
- Else **diff** the fresh full tree against `document.blocks`:
  - **Equal** (incremental was already correct — the common case): clear
    `blocks_maybe_stale` + in-flight; **no tree change, no forced redraw**.
  - **Different** (a real divergence caught): replace `document.blocks`, call the §0
    downstream phase (fold + outline + layout) on the new tree **without
    re-parsing**, clear `blocks_maybe_stale` + in-flight; the next draw renders it.

**`merge` contract note:** `JobResult::merge` is documented as touching "only
non-document bookkeeping" (`jobs.rs:43`). `document.blocks` is a **derived cache**
(regenerable from the text), not authoritative document content (text / version /
selection / history are untouched) — so replacing it is within the merge contract.
The plan should widen that doc comment to say "derived caches included."

## Error handling summary

| Situation | Response |
|---|---|
| Reconcile job panics (pulldown residual) | worker `catch` → job panicked → skip round, tree unchanged, no crash |
| Buffer version advanced / buffer closed before merge | discard; re-schedule next idle |
| Fresh tree == current tree | clear flag, no redraw |
| Fresh tree != current tree | replace + refresh downstream + redraw + clear flag |

## Testing

- **`rebuild` refactor (the perf fix):** a non-edit `rebuild` (no pending
  `pre_edit_rope`/`last_edit`) must NOT reparse — assert it leaves `document.blocks`
  identical to a chosen (possibly-diverged) tree and does not call `full_parse`
  (e.g. seed `document.blocks` with a sentinel tree that differs from
  `full_parse(text)`, run a non-edit `rebuild`, assert the sentinel survives). An
  edit `rebuild` still produces the incremental result. The downstream phase yields
  the same layout/fold output for a given tree as before.
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

1. The exact `derive::rebuild` (`derive.rs:82`) two-phase split: the boundary
   between the parse phase and the fold/outline/layout downstream phase
   (`derive.rs:115` fold reconcile, `:131` layout), extracted as a reusable
   downstream function the reconcile merge also calls. Confirm nothing else in
   `rebuild` depends on a fresh full parse on non-edit draws.
2. The `incremental_update_instrumented_src` (`block_tree.rs:540`) rope→str call
   shape to replace `incremental_update_rope` in the parse phase (surfacing
   `WidenReason`).
3. The Executor path to mirror: `JobKind` + `ResultClass` + `is_stale` (`jobs.rs:66`)
   coalescing, `JobResult`/`JobResult::merge` (`jobs.rs:43`), `JobOutcome::Panicked`
   + `apply_panic` (`app.rs:198`) — the new `Reparse` arms in `is_stale` and
   `apply_panic`; plus how a reconcile job is submitted to the Executor. Reuse the
   diagnostics *debounce/in-flight* shape (`DiagStore.recheck_due_at`/
   `in_flight_version`, `diagnostics_run.rs`) only for the deadline logic.
4. `RECONCILE_DEBOUNCE_MS` value + where it lives, aligned with `swap::T_IDLE_MS` /
   the diagnostics debounce; and folding the reconcile deadline into `next_deadline`
   (`app.rs` ~2089).
5. Where the new state lives (`Document` vs `Buffer`, `editor.rs:51`/`:95`):
   `blocks_maybe_stale`, `blocks_version` (the memoization key for §0),
   `reconcile_due_at`, and the reconcile `in_flight_version` — alongside existing
   `version`, `blocks`, `last_edit_at`. Confirm nothing already relied on the
   per-frame full parse for correctness after undo/redo (the version gate must cover
   every text-changing path), and that filter/transform/paste indeed set the
   incremental state via `Buffer::apply` (so they take the incremental branch).
6. Widen the `JobResult::merge` doc comment (`jobs.rs:43`) to note derived document
   caches (e.g. `document.blocks`) are within scope.
