# F1: bound the synchronous WidenToEnd reparse — design

**Status:** approved design (pre-spec-review)
**Date:** 2026-07-02
**Effort:** F1 (responsiveness follow-up; the second of the two deferred from editing-responsiveness — F8 was assessed and shelved: its "bound layout to visible rows" premise is unsafe because `ColMap` consumers need the whole logical line)

## Context

The incremental block-tree parser (`wordcartel-core/src/block_tree.rs`) has a
`WidenReason::WidenToEnd` path that, on certain edits inside/adjacent to a top-level
container (List / BlockQuote / IndentedCode / Table, or a ref-def/fence change),
reparses **from the enclosing container's start to END-OF-DOCUMENT synchronously on the
per-keystroke path** (block_tree.rs:773 sets `region_old_end = old_src.len()`; the
`parse_region` call at ~:900 then reparses `[container_start, EOF)`). Container structure
(loose/tight, nesting, absorb-to-EOF) is non-local, so the widen exists for correctness.

A measurement (10 MiB list-doc, editing near the top) showed **~33 ms/keystroke** — a
visible typing freeze, O(document). It only bites large *container*-heavy docs (prose is
always `Local`/fast), but there it is severe. The eventual-consistency reconcile machinery
(the debounced `JobKind::Reparse` that converges any `maybe_stale` tree at rest, 150 ms)
already exists — this effort routes the pathological case through it.

## Goals

- Cap the SYNCHRONOUS reparse cost of a widen so a keystroke inside an arbitrarily large
  container stays O(local block) instead of O(container→EOF).
- Zero behavior change for documents whose widen region is ≤ the cap (essentially all real
  markdown) — that path stays byte-identical to today, fully correct with no added latency.
- The pathological (>cap) case trades ~150 ms of transiently-wrong styling *below the edit*
  for killing the freeze, converged by the existing reconcile.

## Non-goals

- No change to the reconcile machinery (`dispatch_reconcile`/`RECONCILE_DEBOUNCE_MS`) — it
  already converges any `maybe_stale` tree.
- No change to the `≤ cap` widen path (`WidenToEnd` stays exactly as today).
- Not F8 (shelved). Not a rework of the incremental parser's correctness elsewhere.

## The tradeoff (accepted)

Editing inside a container that spans **> 1 MiB** shows transiently-wrong styling/structure
below the edit for ~150 ms after you pause, then converges to the correct tree. Everything
≤ 1 MiB is unaffected (fully correct, zero added latency). This is an EXPANSION of the
already-accepted `maybe_stale`/reconcile eventual-consistency class (which the
editing-responsiveness effort established for the `Local`/`WidenToEnd` tail divergences).

## Component 1 — the size-gated widen (`wordcartel-core/src/block_tree.rs`)

- Add a variant to `WidenReason` (block_tree.rs:477-485):
  ```
  /// The widen region exceeded MAX_SYNC_WIDEN_BYTES — reparsed only the local block
  /// (like Local) and left the container-wide effects STALE. maybe_stale → reconcile
  /// converges it to full_parse at rest.
  BoundedStale,
  ```
  `NoOverlapFull` remains the only variant guaranteed byte-equal to `full_parse`. `Local`
  and `WidenToEnd` are "may diverge in the tail" (as today); `BoundedStale` is
  "deliberately diverges beyond the local block."
- Add `pub const MAX_SYNC_WIDEN_BYTES: usize = 1 << 20;` (1 MiB) — a named tunable, mirroring
  `RECONCILE_DEBOUNCE_MS`.
- **The gate (the widen decision, block_tree.rs:767-774).** Today:
  ```
  let widen = absorptive_in_region || slack_is_absorptive || downstream_container_merge
      || needs_widen_to_end(...);
  if widen { region_old_end = old_src.len(); reason = WidenReason::WidenToEnd; }
  else { /* +1 slack sets region_old_end */ reason = WidenReason::Local; }
  ```
  Restructure so the widen only fires synchronously when its span is within the cap; else
  fall through to the same local (+1-slack) region the non-widen branch computes, tagged
  `BoundedStale`:
  ```
  // Cost of the would-be widen ≈ container-start → EOF, measured in OLD coords.
  let widen_span = old_src.len().saturating_sub(region_old_start);
  if widen && widen_span <= MAX_SYNC_WIDEN_BYTES {
      region_old_end = old_src.len();
      reason = WidenReason::WidenToEnd;
  } else {
      // the existing +1-slack local region logic sets region_old_end
      reason = if widen { WidenReason::BoundedStale } else { WidenReason::Local };
  }
  ```
  So the `> cap` path reuses the existing `Local` region+splice machinery unchanged — the
  ONLY difference from `Local` is the reason tag. The local region always contains the edit
  (it's the enclosing block + slack), so the edit itself is parsed correctly; only the
  container-wide effects beyond it are stale.
- **The second widen trigger (created-container growth, block_tree.rs:~924)** also extends
  `region_old_end = old_src.len()`. Apply the same gate: if the grown span would exceed the
  cap, do NOT extend to EOF — keep the already-reparsed local region and set
  `reason = WidenReason::BoundedStale`.
- **`reparsed_bytes`** for a `BoundedStale` outcome reflects the LOCAL region size (≤ cap),
  not EOF — so the "freeze ceiling is bounded" property is observable in the outcome.
- The resulting `BoundedStale` tree is a **valid splice** (local reparse + verbatim-shifted
  "after" blocks — ordered, non-overlapping at every level, so `role_at`/fold/render stay
  sound) that is semantically stale beyond the local block.

## Component 2 — oracle carve-out + wiring

### Oracle (`wordcartel-core/tests/block_tree_oracle.rs`)

`incremental ≡ full` is asserted UNCONDITIONALLY today (`check()` :88-99;
`assert_all_paths_agree!` :24-44; `assert_chain_paths_agree!` :53-79). A `BoundedStale`
tree is not equal to `full_parse` by design, so:
- **`check()`** — assert `outcome.tree == full` only when `outcome.reason != BoundedStale`.
- **`assert_all_paths_agree!`** — skip the two `== full` assertions for `BoundedStale`, but
  KEEP the str-path-vs-rope-path agreement assertion (`rope_inc == str_inc`) — the same input
  must still produce the same bounded output on both source types.
- **`assert_chain_paths_agree!`** — when a step yields `BoundedStale`, RESET the carried-forward
  trees to `full_parse(new_text)` before the next iteration (via the instrumented variant that
  returns the reason): a stale tree fed to the next `incremental_update` sees wrong block spans
  and the divergence CASCADES, corrupting every subsequent step. `if reason == BoundedStale {
  str_tree = full_parse(new_text); } else { str_tree = new_str_tree; }` (and likewise the rope
  tree).
- Note: the proptest generators only produce small (KB) docs, so they never reach the 1 MiB
  cap → never actually emit `BoundedStale` → the existing proptests stay green regardless. The
  carve-out is contract-correctness + future-proofing (a future larger generator, or the
  deterministic test below, exercises it).

### Wiring (`wordcartel/src/derive.rs`)

- Add `BoundedStale` to the `matches!(outcome.reason, WidenReason::Local | WidenReason::WidenToEnd)`
  (derive.rs:139-143) that sets `stale = true`. `BoundedStale` → `maybe_stale = true` → arms
  the reconcile exactly like `WidenToEnd` does. No other change.

### Reconcile — NO change

`dispatch_reconcile` already runs `full_parse_rope` on the debounce and installs the correct
tree, clearing `maybe_stale`. A `BoundedStale` tree converges through the identical path. The
convergence theorem (no edits for `RECONCILE_DEBOUNCE_MS` ⇒ `blocks == full_parse`) already
covers it.

## Testing

- **Existing oracle + proptest suite stays green** (small docs never hit the cap → still
  `WidenToEnd`/`Local` → still `== full`).
- **Pinned deterministic regression (the heart):** build a > 1 MiB doc-spanning list
  (e.g. ~1.5 MiB of `- item\n`), edit near the TOP, and assert:
  (a) `outcome.reason == BoundedStale`;
  (b) `outcome.reparsed_bytes <= MAX_SYNC_WIDEN_BYTES` (the freeze ceiling is real);
  (c) the tree is structurally valid (children ordered + non-overlapping at every level —
      reuse the F3 invariant helper);
  (d) it `!=` `full_parse(new_text)` (genuinely bounded/stale, not accidentally correct);
  (e) `full_parse(new_text)` (what reconcile computes) is the correct convergence target and a
      subsequent full reparse installs it.
- **"Small container still widens fully":** a small-container edit that triggers the widen
  path → `reason == WidenToEnd` (NOT `BoundedStale`) and `tree == full_parse` — proving ZERO
  behavior change for normal docs (the cap doesn't perturb the common path).
- **Convergence (shell, optional):** after a `BoundedStale` edit the reconcile debounce +
  merge yields `full_parse` at rest (the reconcile-effort tests already cover the general
  `maybe_stale` convergence; `BoundedStale` is just another source).

## Decomposition (2 tasks)

1. **Core** — `WidenReason::BoundedStale` + `MAX_SYNC_WIDEN_BYTES` + the size gate (both widen
   trigger sites) + `reparsed_bytes` accounting + the `derive.rs` `maybe_stale` one-liner.
   Existing tests stay green (small docs never hit the cap).
2. **Oracle carve-out + tests** — the `check`/`assert_all_paths_agree!`/chain-reset carve-outs
   + the pinned `BoundedStale` regression (a-e) + the small-container-still-`WidenToEnd` test.

## Global constraints

- `#![forbid(unsafe_code)]` in core unchanged; core-only for Components 1-2 (+ the 1-line
  `derive.rs` wiring in the shell).
- `cargo test -p wordcartel-core -p wordcartel` green; workspace clippy **deny** gate clean;
  no `cargo fmt`; house style (em-dash `—`).
- Hot path: the widen decision adds one `usize` comparison; the `> cap` path is O(local block)
  — strictly cheaper than today. No new O(document) work.

## Plan-confirms (resolve during the implementation plan, against real source)

1. The exact widen-region-size expression for the gate — is `old_src.len() - region_old_start`
   the right measure of the would-be widen cost at the point of the decision (region_old_start
   already line-snapped/group-walked), and does it match `reparsed_bytes`' definition?
2. The `> cap` path reuses the EXISTING `Local` +1-slack region/splice unchanged (only the
   reason tag differs) — confirm the local region always contains the edit (so the edit is
   parsed correctly) and the resulting splice is structurally valid.
3. The second widen trigger (created-container growth, block_tree.rs:~924) — confirm its exact
   current shape and that the same size gate applies (don't extend to EOF when > cap; tag
   `BoundedStale`).
4. `reparsed_bytes` is set to the LOCAL region size on the `BoundedStale` path (so test (b)
   holds), not left at an EOF-relative value.
5. Any OTHER consumer of `WidenReason` beyond derive.rs's `maybe_stale` `matches!` and the
   oracle (grep) that a new variant would need to handle (exhaustive `match`es on `WidenReason`).
6. The oracle's instrumented-variant availability for the chain-reset (`incremental_update_instrumented`
   returns the reason) — confirm the chain macro can read it per step.
