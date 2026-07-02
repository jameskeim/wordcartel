# F1: bound the synchronous WidenToEnd reparse — design

**Status:** approved design, rescoped to Case A after Codex spec-review round 1 (re-review pending)
**Date:** 2026-07-02
**Effort:** F1 (responsiveness follow-up; the second of the two deferred from editing-responsiveness — F8 was assessed and shelved: its "bound layout to visible rows" premise is unsafe because `ColMap` consumers need the whole logical line)

## Context

The incremental block-tree parser (`wordcartel-core/src/block_tree.rs`) has a
`WidenReason::WidenToEnd` path that, on certain edits inside/adjacent to a top-level
container (List / BlockQuote / IndentedCode / Table, or a ref-def/fence change),
reparses **from the reparse region's start to END-OF-DOCUMENT synchronously on the
per-keystroke path** (block_tree.rs:773 sets `region_old_end = old_src.len()`; the
`parse_region` call at ~:903 then reparses `[region_start, EOF)`). Container structure
(loose/tight, nesting, absorb-to-EOF) is non-local, so the widen exists for correctness.

A measurement showed **~33 ms/keystroke** on a large container-doc — a visible typing
freeze, O(document). The eventual-consistency reconcile machinery (the debounced
`JobKind::Reparse` that converges any `maybe_stale` tree at rest, 150 ms) already exists —
this effort routes the bounded case through it.

## Two worst cases — and which one F1 targets (rescoped, Codex round 1)

The reparse region is initialized from the **enclosing top-level block's span**
(block_tree.rs:573), line-snapped and group-walked (block_tree.rs:584/594/611), *before*
the widen decision. So there are two distinct pathological shapes:

- **Case A — a *small* container whose widen speculatively reparses the tail to EOF.** The
  base local region (the enclosing block + slack) is small; the *widen extension* to EOF is
  the expensive part (e.g. a 5-line list near the top of a 2 MiB doc; or typing an opening
  ` ``` ` fence — `needs_widen_to_end` fires and the whole tail becomes a code candidate).
  **This is what F1 bounds:** don't extend to EOF; reparse only the small base region and
  let reconcile converge the container-wide effect. Realistic and common in large structured
  documents.
- **Case B — a single *huge* top-level container** (a doc-spanning 10 MiB list). Here the
  enclosing block already *is* the whole document, so the base local region is megabytes
  *before* the widen decision. Bounding the extension saves nothing. **F1 does NOT bound
  Case B** (see Non-goals) — cheaply reparsing a window *inside* one giant block needs
  synthetic-coverage machinery the block-splice model lacks, and it is additionally floored
  by the deferred F4 cost (even a `Local` edit deep-clones all ~N blocks). Case B stays on
  today's `WidenToEnd` path (correct, still slow) — a documented limitation, deferred.

## Goals

- Cap the SYNCHRONOUS reparse cost when the expense is the **widen extension** (Case A): a
  keystroke stays O(base local block) instead of O(region-start→EOF).
- Zero behavior change for documents whose widen extension is ≤ the cap (essentially all
  real markdown) — that path stays byte-identical to today.
- The bounded (Case A) case trades ~150 ms of transiently-wrong styling *below the edit* for
  killing the freeze, converged by the existing reconcile.

## Non-goals

- **Case B is out of scope** — a single top-level container larger than the cap is not
  bounded (needs window+synthetic-coverage machinery + is F4-floor-limited; a future effort).
  When the base local region itself exceeds the cap, F1 falls through to today's exact
  `WidenToEnd` behavior (no `BoundedStale`, no redundant reconcile).
- No change to the reconcile machinery (`dispatch_reconcile`/`RECONCILE_DEBOUNCE_MS`).
- No change to the `≤ cap` widen path (`WidenToEnd` stays exactly as today).
- Not F8 (shelved).

## The tradeoff (accepted)

Editing a small container/fence whose correct reparse would speculatively extend > 1 MiB to
EOF shows transiently-wrong styling below the edit for ~150 ms, then converges. Everything
whose widen extension is ≤ 1 MiB is unaffected (fully correct, zero added latency). This is
an EXPANSION of the already-accepted `maybe_stale`/reconcile eventual-consistency class.

## Component 1 — the size-gated widen (`wordcartel-core/src/block_tree.rs`)

- Add a variant to `WidenReason` (block_tree.rs:477-485):
  ```
  /// The widen EXTENSION to EOF would exceed MAX_SYNC_WIDEN_BYTES while the base local
  /// region is small (Case A) — reparsed only the base region and left the container-wide
  /// effect STALE. maybe_stale → reconcile converges it to full_parse at rest.
  BoundedStale,
  ```
  `NoOverlapFull` remains the only variant guaranteed byte-equal to `full_parse`. `Local`
  and `WidenToEnd` are "may diverge in the tail" (as today); `BoundedStale` is "deliberately
  diverges beyond the base local region."
- Add `pub const MAX_SYNC_WIDEN_BYTES: usize = 1 << 20;` (1 MiB) — a named tunable, mirroring
  `RECONCILE_DEBOUNCE_MS`.
- **The three-way gate (the widen decision, block_tree.rs:767-774).** Today:
  ```
  let widen = absorptive_in_region || slack_is_absorptive || downstream_container_merge
      || needs_widen_to_end(...);
  if widen { region_old_end = old_src.len(); reason = WidenReason::WidenToEnd; }
  else { /* +1 slack sets region_old_end */ reason = WidenReason::Local; }
  ```
  At the gate, `region_old_end` is the **base local region** end (enclosing block + slack),
  before the widen extension. Restructure to bound only when it *helps* — i.e. the extension
  is expensive AND the base region is small:
  ```
  // base_region_size = the fallback (Local) reparse cost; widen_span = the WidenToEnd cost.
  let base_region_size = region_old_end.saturating_sub(region_old_start);
  let widen_span       = old_src.len().saturating_sub(region_old_start);
  if widen {
      if widen_span <= MAX_SYNC_WIDEN_BYTES {
          // extension is cheap → widen fully, exactly as today
          region_old_end = old_src.len();
          reason = WidenReason::WidenToEnd;
      } else if base_region_size <= MAX_SYNC_WIDEN_BYTES {
          // Case A: expensive extension, small base → bound to the base region, defer
          // (leave region_old_end at the base; do NOT extend to EOF)
          reason = WidenReason::BoundedStale;
      } else {
          // Case B: the base local region itself exceeds the cap → F1 can't help;
          // do the full widen as today (correct; avoids a redundant reconcile pass)
          region_old_end = old_src.len();
          reason = WidenReason::WidenToEnd;
      }
  } else {
      reason = WidenReason::Local;
  }
  ```
  The `BoundedStale` branch reuses the existing `Local` region+splice machinery unchanged —
  the only difference from `Local` is the reason tag. The base local region always contains
  the edit, so the edit is parsed correctly; only the container-wide effect beyond it is stale.
- **The second widen trigger (created-container growth, block_tree.rs:~924)** also extends
  `region_old_end = old_src.len()` — but AFTER the base region was already parsed once
  (block_tree.rs:903). Apply the same gate: if the extension would exceed the cap while the
  base region was small, do NOT extend to EOF (skip the expensive SECOND parse) and tag
  `BoundedStale`; the cheap first (base-region) parse already ran. If the base was itself >
  cap (Case B), extend as today.
- **`reparsed_bytes`** for a `BoundedStale` outcome reflects the BASE region size (≤ cap in
  Case A), so the freeze-ceiling property is observable in the outcome.
- The resulting `BoundedStale` tree is a **valid, full-coverage splice** (before-blocks +
  base-region reparse + verbatim-shifted after-blocks — ordered, non-overlapping, covers
  `[0, EOF)`, so `role_at`/fold/render stay sound) that is semantically stale beyond the base
  region.

## Component 2 — oracle carve-out + wiring

### Oracle (`wordcartel-core/tests/block_tree_oracle.rs`)

`incremental ≡ full` is asserted UNCONDITIONALLY today (`check()` :88-99;
`assert_all_paths_agree!` :24-44; `assert_chain_paths_agree!` :53-79). A `BoundedStale` tree
is not equal to `full_parse` by design. The single-edit macros currently call the
NON-instrumented `incremental_update` / `incremental_update_rope` (block_tree_oracle.rs:31),
so they cannot see the reason — **convert both single-edit and chain helpers to the
instrumented variants** (`incremental_update_instrumented_src` / the rope equivalent, both
public) to read `outcome.reason`. Then:
- **`check()`** — assert `outcome.tree == full` only when `reason != BoundedStale`.
- **`assert_all_paths_agree!`** — skip the two `== full` assertions for `BoundedStale`, but
  KEEP the str-path-vs-rope-path agreement (`rope_inc == str_inc`) — the bounding logic is
  source-type-independent, so the same input must give the same bounded output on both.
- **`assert_chain_paths_agree!`** — when a step yields `BoundedStale`, RESET the
  carried-forward trees to `full_parse(new_text)` before the next iteration: a stale tree fed
  to the next `incremental_update` sees wrong block spans and the divergence CASCADES,
  corrupting every subsequent step. `if reason == BoundedStale { str_tree = full_parse(new_text) }
  else { str_tree = new_str_tree }` (and likewise the rope tree).
- Note: the proptest generators only produce small (KB) docs → never reach the cap → never
  emit `BoundedStale`, so the existing proptests stay green regardless. The carve-out is
  contract-correctness + future-proofing, exercised by the deterministic test below.

### Wiring (`wordcartel/src/derive.rs`)

- Add `BoundedStale` to the `matches!(outcome.reason, WidenReason::Local | WidenReason::WidenToEnd)`
  (derive.rs:139-143) that sets `stale = true`. `BoundedStale` → `maybe_stale = true` → arms
  the reconcile exactly like `WidenToEnd`. No other change.

### Reconcile — NO change

`dispatch_reconcile` already runs `full_parse_rope` on the debounce and installs the correct
tree, clearing `maybe_stale`. A `BoundedStale` tree converges through the identical path; the
convergence theorem already covers it.

## Testing

- **Existing oracle + proptest suite stays green** (small docs never hit the cap → still
  `WidenToEnd`/`Local` → still `== full`).
- **Pinned deterministic Case-A regression (the heart):** build a large doc whose base local
  region for the edit is SMALL but whose widen extension reaches EOF > 1 MiB — e.g. a small
  paragraph/list near the top of ~1.5 MiB of paragraphs, then insert an opening ` ``` ` fence
  (fires `needs_widen_to_end`; the tail becomes a code candidate). Assert:
  (a) `outcome.reason == BoundedStale`;
  (b) `outcome.reparsed_bytes <= MAX_SYNC_WIDEN_BYTES` (freeze ceiling is real — the BASE
      region, not EOF);
  (c) the tree is structurally valid AND full-coverage (children ordered + non-overlapping at
      every level, spans cover `[0, new_len)` — reuse the F3 invariant helper);
  (d) it `!=` `full_parse(new_text)` (genuinely bounded/stale, not accidentally correct);
  (e) `full_parse(new_text)` (what reconcile computes) is the correct convergence target.
- **"Small extension still widens fully":** a small-doc edit that triggers the widen path with
  a ≤ cap extension → `reason == WidenToEnd` (NOT `BoundedStale`) and `tree == full_parse` —
  proving ZERO behavior change for the common path.
- **"Case B falls through to WidenToEnd":** an edit inside a single container whose BASE
  region already exceeds the cap → `reason == WidenToEnd` (NOT `BoundedStale`) — documents
  that F1 does not perturb (or redundantly reconcile) the single-huge-container case.
- **Convergence (shell, optional):** after a `BoundedStale` edit the reconcile debounce +
  merge yields `full_parse` at rest (the reconcile-effort tests already cover general
  `maybe_stale` convergence; `BoundedStale` is just another source).

## Decomposition (2 tasks)

1. **Core** — `WidenReason::BoundedStale` + `MAX_SYNC_WIDEN_BYTES` + the three-way size gate
   (both widen trigger sites) + `reparsed_bytes` accounting + the `derive.rs` `maybe_stale`
   one-liner. Existing tests stay green (small docs never hit the cap).
2. **Oracle carve-out + tests** — convert the macros to the instrumented variants; the
   `check`/`assert_all_paths_agree!`/chain-reset carve-outs + the pinned Case-A regression
   (a-e) + the small-extension-still-`WidenToEnd` and Case-B-fall-through tests.

## Global constraints

- `#![forbid(unsafe_code)]` in core unchanged; core-only for Components 1-2 (+ the 1-line
  `derive.rs` wiring in the shell).
- `cargo test -p wordcartel-core -p wordcartel` green; workspace clippy **deny** gate clean;
  no `cargo fmt`; house style (em-dash `—`).
- Hot path: the widen decision adds two `usize` comparisons; the Case-A path is O(base local
  block) — strictly cheaper than today. No new O(document) work; Case B unchanged.

## Plan-confirms (resolve during the implementation plan, against real source)

1. **Region-construction ordering (Codex round 1).** Confirm that at the widen decision
   (block_tree.rs:767) `region_old_end` is genuinely the BASE local region end (enclosing
   block + slack), and identify whether the local branch's own EOF extensions
   (block_tree.rs:802 "no later block", :862 trailing-gap) fire BEFORE or AFTER the gate — if
   before, `base_region_size` must be measured to include/exclude them consistently so the
   Case-A/Case-B classification and `reparsed_bytes ≤ cap` hold.
2. The `BoundedStale` branch reuses the EXISTING `Local` +1-slack region/splice unchanged
   (only the reason tag differs) — confirm the base region always contains the edit and the
   resulting splice is structurally valid and full-coverage.
3. The second widen trigger (created-container growth, block_tree.rs:~924) — confirm its exact
   shape and that gating it skips only the EXPENSIVE second (EOF) parse (the cheap base-region
   parse at :903 already ran), tagging `BoundedStale` only in Case A.
4. `reparsed_bytes` is set to the BASE region size on the `BoundedStale` path (so test (b)
   holds), not left at an EOF-relative value.
5. Any OTHER consumer of `WidenReason` beyond derive.rs's `maybe_stale` `matches!` and the
   oracle (grep). Codex round 1 confirmed no exhaustive `match` sites exist in core/shell, so
   adding a variant won't break the build — but list every reason-consuming site the new
   variant must be reasoned about.
6. The oracle's instrumented variants (`incremental_update_instrumented_src` + the rope
   equivalent) are public and return the reason — confirm both single-edit and chain macros
   can convert to them for the carve-out.
