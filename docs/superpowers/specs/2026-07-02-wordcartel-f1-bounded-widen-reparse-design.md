# F1: bound the synchronous WidenToEnd reparse — design

**Status:** Fable5 folded + Codex fold-verify corrections (I2 reason=NoOverlapFull, I4 +2 sites); re-verify pending
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
Beyond styling, the stale-but-valid tree is safe for every downstream consumer (Fable
confirmed: no panic/data-loss — save writes the rope TEXT not the tree; spans stay
clamped/ordered); the only other visible effect within the window is transiently
wrong-extent transform reflow and caret snapping off stale sections (same accepted class).
One pre-existing leak F1 enlarges (Fable M4): if a `BoundedStale` edit's reconcile
`full_parse` PANICS, the parse-panic handler (app.rs) clears `maybe_stale` WITHOUT installing
a correct tree, so the deliberately-wrong tail persists silently until the next edit — an
accepted (M4-rest) behavior whose stale footprint is now larger.

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
  `RECONCILE_DEBOUNCE_MS`. NOTE (Fable M6): the exact ms-per-MiB of `parse_region` was not
  rigorously derived from the 33 ms measurement (estimates range ~3–15 ms/MiB); 1 MiB may sit
  near/over a 16 ms frame. Validate the real cost during implementation and lower the const
  (e.g. 512 KiB) if a 1 MiB bounded parse exceeds a frame — it is a one-line tunable.
- **The three-way gate — classify a PREDICTED base region on copies (Codex round 3).**
  IMPORTANT: at today's widen decision (block_tree.rs:767) the region is not yet finalized —
  the Local +1 slack (:802) and gap-edit pullback (:822) run inside the `else` branch, and the
  shared straddle repair (:836) and trailing-gap coverage (:862) run for both paths afterward.
  Several of these mutate `region_old_start` (not just `region_old_end`), and the gap-edit
  pullback applies ONLY on the Local path — so the finalization logic canNOT simply be hoisted
  before the widen decision without changing `WidenToEnd` behavior. Instead the design PREDICTS
  the base region on copies and installs it only for the bounded case:
  ```
  // 1. compute `widen` exactly where it is today (pre-slack values) — unchanged.
  // 2. if widen, predict the base local region on COPIES by applying the Local-path
  //    finalization (slack :802, gap-edit pullback :822) then the shared straddle repair
  //    (:836) + trailing-gap coverage (:862) to `base_old_start`/`base_old_end` copies:
  let base_region_size = base_old_end.saturating_sub(base_old_start);
  let widen_span       = old_src.len().saturating_sub(region_old_start);
  if widen {
      if widen_span <= MAX_SYNC_WIDEN_BYTES {
          // cheap extension → today's ORIGINAL WidenToEnd path, entirely unchanged
          region_old_end = old_src.len(); reason = WidenReason::WidenToEnd;
      } else if base_region_size <= MAX_SYNC_WIDEN_BYTES {
          // Case A: expensive extension, small base → install the copied base bounds, defer
          region_old_start = base_old_start; region_old_end = base_old_end;
          reason = WidenReason::BoundedStale;
      } else {
          // Case B: base itself > cap → today's ORIGINAL WidenToEnd path, unchanged
          region_old_end = old_src.len(); reason = WidenReason::WidenToEnd;
      }
  }
  // (no widen → today's Local branch, unchanged)
  ```
  Only `BoundedStale` installs the copied base bounds; both `WidenToEnd` branches and the
  `Local` branch keep today's exact behavior. Because the copies apply the SAME finalization
  the Local path would (including the :862 trailing-gap, which can reach EOF), `base_region_size`
  is the true Local cost: a base that trailing-gaps to EOF classifies as Case B (falls through
  to `WidenToEnd`); a small base classifies as Case A. The `BoundedStale` splice is then the
  existing `Local` region+splice over the installed bounds — the base region always contains the
  edit, so the edit is parsed correctly; only the container-wide effect beyond it is stale.
- **The second widen trigger (created-container growth, block_tree.rs:915-929)** also extends
  `region_old_end = old_src.len()` — but AFTER the base region was already parsed once
  (block_tree.rs:903). **When the FIRST gate already chose `BoundedStale`, the second trigger
  must NEVER extend** (Fable I1). Otherwise: the second extension can be ≤ cap (e.g. widen_span
  1.6 MiB, base 0.9 MiB ⇒ extension 0.7 MiB), so a naive "extend only if ≤ cap" would parse
  base+extension ≈ 2·cap on top of the first base parse — ~3·cap ≈ 3 MiB synchronous, the very
  freeze F1 exists to kill, and the extension buys nothing durable (the result would be
  `maybe_stale` and reconcile runs anyway). So: if the first gate chose `BoundedStale`, keep the
  base region, full stop. The cap check at the second trigger applies ONLY to Local-path
  arrivals at :915 (base ≤ cap, considering a first-time extension), gated the same three-way
  way. Case B (first gate `WidenToEnd`) is unchanged from today.
- **Seam-guard escape hatch — the freeze ceiling is best-effort, not absolute (Fable I2).**
  Today's `WidenToEnd` has no "after" blocks, so the post-splice seam guard's AFTER-seam
  (`merge_at` over after-blocks, block_tree.rs:970-982) can never fire for it (the before-seam
  is still evaluated — Codex). `BoundedStale` CREATES after-blocks, so for specific adjacencies
  (e.g. the base's last List becomes a Paragraph abutting a shifted `IndentedCode` with no
  blank line → `paragraph_absorbs_next`) the AFTER-seam triggers a synchronous
  `full_parse_src(new_src)` — O(document), i.e. the freeze survives for those rare shapes. This
  is CORRECT (the full parse is right) but not cheaper. Accepted + documented: the cap is a
  freeze ceiling for the common Case-A shapes (fences, list/table downstream-merge), not an
  absolute guarantee for every adjacency. **Reason for the seam-tripped case (Codex — corrected):
  it must be `NoOverlapFull`** — the existing seam-guard `full_parse_src` reason at
  block_tree.rs:975, which `derive.rs:139` treats as NON-stale (so no redundant reconcile over
  an already-correct full parse). NOT `BoundedStale` and NOT `WidenToEnd` — both are marked
  stale by `derive.rs`. Plan-confirm: confirm the seam-guard path already yields `NoOverlapFull`
  for the bounded case (the reason set earlier is overridden when the guard does the full parse);
  if not, ensure the guard overrides it.
- **`reparsed_bytes`** is set from `new_region.len()` at block_tree.rs:930, AFTER all base
  growth. Because `BoundedStale` installs the PREDICTED base bounds (which already include the
  slack/straddle/trailing-gap finalization) and classifies against their size, `reparsed_bytes`
  equals that base size (≤ cap in Case A by the gate's check) — the freeze-ceiling property is
  observable. The prediction must include the trailing-gap coverage so a base that would reach
  EOF classifies as Case B (WidenToEnd), never as a `BoundedStale` that then reparses to EOF.
- The resulting `BoundedStale` tree is a **valid splice** (before-blocks + base-region reparse
  + verbatim-shifted after-blocks) whose ROOT spans `[0, new_len)` with child spans
  ordered/non-overlapping at every level (gaps between top-level blocks allowed — children do
  not tile), so `role_at`/fold/render stay sound; it is semantically stale beyond the base
  region.

## Component 2 — oracle carve-out + wiring

### Oracle (`wordcartel-core/tests/block_tree_oracle.rs`)

`incremental ≡ full` is asserted UNCONDITIONALLY today (`check()` :88-99;
`assert_all_paths_agree!` :24-44; `assert_chain_paths_agree!` :53-79). A `BoundedStale` tree
is not equal to `full_parse` by design. The single-edit macros currently call the
NON-instrumented `incremental_update` / `incremental_update_rope` (block_tree_oracle.rs:31),
so they cannot see the reason — **convert both single-edit and chain helpers to the
instrumented variant** `incremental_update_instrumented_src` (public at block_tree.rs:545;
its generic signature covers BOTH string and rope sources — no separate rope-instrumented
variant exists or is needed, Codex round 2) to read `outcome.reason`. Then:
- **`check()`** — assert `outcome.tree == full` only when `reason != BoundedStale`.
- **`assert_all_paths_agree!`** — skip the two `== full` assertions for `BoundedStale`, but
  KEEP the str-path-vs-rope-path agreement (`rope_inc == str_inc`) — the bounding logic is
  source-type-independent, so the same input must give the same bounded output on both.
- **`assert_chain_paths_agree!`** — when a step yields `BoundedStale`, RESET the
  carried-forward trees to `full_parse(new_text)` before the next iteration: a stale tree fed
  to the next `incremental_update` sees wrong block spans and the divergence CASCADES,
  corrupting every subsequent step. `if reason == BoundedStale { str_tree = full_parse(new_text) }
  else { str_tree = new_str_tree }` (and likewise the rope tree). When converting, also assert
  `str_reason == rope_reason` (Fable M3) — else the skip logic silently trusts one path's reason.
- **Complete the reason-blind `== full` inventory (Fable I4).** Beyond `check()`/the two
  proptest macros, the SAME carve-out (or a stated exemption) is needed at every other
  unconditional `incremental == full` site the new variant can reach:
  - **`incremental_equals_full` (block_tree.rs:1280-1284) — the cargo-fuzz F2 oracle**
    (`fuzz/fuzz_targets/block_tree.rs:10`). CRITICAL to carve out: a future fuzz run with
    `max_len ≥ cap` would emit `BoundedStale` and produce spurious assertion failures
    indistinguishable from the real F2 divergences the campaign is still chasing — a
    false-positive trap. Skip the `== full` when `reason == BoundedStale`.
  - `assert_all_paths_agree_det` (block_tree_oracle.rs:803-825) and the chained regression body
    in `regression_inline_link_end_corrupts_list_nesting` (:779-794).
  - the additional unconditional `== full` sites at block_tree_oracle.rs:714 and :731 (Codex —
    small fixed docs, low risk, but the inventory should be complete).
  - the in-module `check` (block_tree.rs:1325).
- Note: the proptest/fuzz generators only produce small (KB) docs → never reach the cap →
  never emit `BoundedStale` today, so the existing suites stay green regardless. The carve-out
  is contract-correctness + future-proofing (a larger generator/fuzz corpus, or the
  deterministic tests below, exercise it).

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
  (c) the tree is structurally valid — the ROOT spans `[0, new_len)` and child spans are
      ordered + non-overlapping at every level (gaps between top-level blocks are allowed;
      children do NOT tile the document — Codex round 2). NOTE (Fable M1): the F3 invariant
      helper (`assert_ordered_nonoverlapping`/`check_tree`, block_tree.rs:1829-1842) is
      `#[cfg(test)]`-private to the lib and unreachable from the external oracle test crate,
      and `check_tree`'s per-byte `role_at` differential is quadratic on a 1.5 MiB doc — so
      the test needs its own lightweight ordered/non-overlapping walk (a public helper, or a
      local recursion), NOT the private/quadratic helper;
  (d) it `!=` `full_parse(new_text)` (genuinely bounded/stale, not accidentally correct);
  (e) `full_parse(new_text)` (what reconcile computes) is the correct convergence target.
- **"Small extension still widens fully":** a small-doc edit that triggers the widen path with
  a ≤ cap extension → `reason == WidenToEnd` (NOT `BoundedStale`) and `tree == full_parse` —
  proving ZERO behavior change for the common path.
- **"Case B falls through to WidenToEnd":** an edit inside a single container whose BASE
  region already exceeds the cap → `reason == WidenToEnd` (NOT `BoundedStale`) — documents
  that F1 does not perturb (or redundantly reconcile) the single-huge-container case.
- **Successive BoundedStale edits without reset (Fable I3 — the production pattern):**
  production feeds a `BoundedStale` tree into the NEXT `incremental_update` (derive.rs:126-137
  — one keystroke per rebuild; typing = several BoundedStale edits per 150 ms window), and
  UNLIKE the oracle it does NOT reset to `full_parse` between keystrokes. Add a deterministic
  test that chains TWO consecutive `BoundedStale` edits WITHOUT a reset and asserts: no panic,
  the tree stays ordered/non-overlapping with root `[0, new_len)`, and a `full_parse` of the
  final text is the convergence target. (Fable's trace says this is panic-free and text-lossless
  — this test pins it, since the oracle's chain-reset erases exactly this coverage.)
- **Convergence (shell, optional):** after a `BoundedStale` edit the reconcile debounce +
  merge yields `full_parse` at rest (the reconcile-effort tests already cover general
  `maybe_stale` convergence; `BoundedStale` is just another source).

## Decomposition (2 tasks)

1. **Core** — `WidenReason::BoundedStale` + `MAX_SYNC_WIDEN_BYTES` + the three-way size gate
   with copy-prediction (first trigger) + the second-trigger "never extend after a first-gate
   BoundedStale" rule (I1) + confirm the seam-tripped case yields `NoOverlapFull` (I2) +
   `reparsed_bytes` accounting + the `derive.rs` `maybe_stale` one-liner. Existing tests stay green.
2. **Oracle carve-out + tests** — convert the macros to `incremental_update_instrumented_src`
   (assert `str_reason == rope_reason`); the FULL carve-out inventory (I4:
   `check`/`assert_all_paths_agree!`/chain-reset + `incremental_equals_full` fuzz oracle +
   `assert_all_paths_agree_det` + in-module `check`); + the pinned Case-A regression (a-e), the
   successive-BoundedStale-no-reset test (I3), and the small-extension-still-`WidenToEnd` and
   Case-B-fall-through tests. Provide a non-quadratic, oracle-crate-reachable validity helper (M1).

## Global constraints

- `#![forbid(unsafe_code)]` in core unchanged; core-only for Components 1-2 (+ the 1-line
  `derive.rs` wiring in the shell).
- `cargo test -p wordcartel-core -p wordcartel` green; workspace clippy **deny** gate clean;
  no `cargo fmt`; house style (em-dash `—`).
- Hot path: the widen decision adds the copy-prediction + two `usize` comparisons; the Case-A
  path is O(base local block) — cheaper than today for the common shapes. Case B is unchanged.
  EXCEPTION (Fable I2): a `BoundedStale` splice whose adjacency trips the seam-consistency
  guard (block_tree.rs:970-982) still does one `full_parse_src` (O(document)) — correct, rare,
  documented; so "no new O(document) work" holds for the common Case-A shapes but is not an
  absolute guarantee.

## Plan-confirms (resolve during the implementation plan, against real source)

1. **Predict the base region on copies; install only for `BoundedStale` (Codex round 3).** The
   region-finalization logic is NOT side-effect-free: the Local +1 slack (:802) and gap-edit
   pullback (:822) run only on the Local path and the pullback mutates `region_old_start`; the
   shared straddle repair (:836) and trailing-gap coverage (:862) mutate both bounds. So do NOT
   hoist them. The plan must: (1) compute `widen` where it is today from the pre-slack values;
   (2) if `widen`, compute a PREDICTED base region on copies `base_old_start`/`base_old_end` by
   applying the Local slack + gap-edit pullback, then the shared straddle repair + trailing-gap
   coverage, to those copies; (3) classify against the copies; (4) install the copied bounds
   ONLY on the `BoundedStale` branch, leaving both `WidenToEnd` branches and the `Local` branch
   byte-identical to today. Codex round 4 confirmed this logic reads only `tops`/`old_src`/
   `slack_pos`/`have_overlap` and mutates only region-bound locals (feasible read-only). **MANDATE
   the shared-helper form, not replication (Fable M2/drift hazard):** extract the straddle-repair
   + trailing-gap (and the slack/pullback) into ONE helper the real path and the prediction both
   call — a replicated copy would silently drift if that logic later changes. The prediction must
   replicate the `!have_overlap` guard on the gap pullback; the `fm_floor` clamp (:892) is NOT
   applied to the copies (conservative-safe — it can only misclassify Case A→B, never B→A).
2. The `BoundedStale` branch reuses the EXISTING `Local` +1-slack region/splice unchanged
   (only the reason tag differs) — confirm the base region always contains the edit and the
   resulting splice is structurally valid and full-coverage.
3. The second widen trigger (created-container growth, block_tree.rs:~924) — confirm its exact
   shape and that gating it skips only the EXPENSIVE second (EOF) parse (the cheap base-region
   parse at :903 already ran), tagging `BoundedStale` only in Case A.
4. `reparsed_bytes` is set to the BASE region size on the `BoundedStale` path (so test (b)
   holds), not left at an EOF-relative value.
5. TWO consumer greps (Fable M5): (a) every `WidenReason` consumer (derive.rs `maybe_stale`
   `matches!` + all the oracle/fuzz `== full` sites in I4) — Codex round 1 confirmed no
   exhaustive `match` sites, so the build won't break; (b) every TREE-SPAN consumer that reads
   `document.blocks` within the 150 ms window and could see a stale span — `role_at`, fold
   `FoldView::compute`/`normalize_caret`, render, nav, and notably `transform::snap_to_blocks`/
   `region_for_transform` (transform.rs:62-92, which turns stale spans into text-edit
   boundaries — transient wrong-extent reflow, accepted class). Confirm none can panic or lose
   data on a valid-but-stale tree.
6. The instrumented variant `incremental_update_instrumented_src` (public, block_tree.rs:545) is
   generic over string AND rope sources — a SINGLE function, no separate rope variant — and
   returns the reason; confirm both single-edit and chain macros can convert to it.
7. Doc-comment drift to update (Fable M5): the `WidenReason`/reason wording at derive.rs:100-101
   and `ReconcileStore` (reconcile.rs:18-19) once `BoundedStale` exists.
