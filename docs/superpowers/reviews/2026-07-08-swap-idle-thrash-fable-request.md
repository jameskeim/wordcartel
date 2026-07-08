# Deferred Fable review request — swap idle-thrash fix (SSD wear / durability)

**Status:** PENDING — deferred to conserve Fable credits (was at 94% on 2026-07-08; resets ~11:00 pm on the 11th).
**Already gated:** Codex pre-merge returned **GO** after 4 rounds (found 3 follow-on issues, all fixed). This is the *second, executable* lens, NOT a re-do of Codex's static pass.
**Merged:** yes — `--no-ff` to `main` as `0a34a15` (already shipped; a Fable finding here would be fix-forward on a new branch).

## How to dispatch (when credits are back)
Whole-branch Fable review on the most-capable model, pointed at the range below:
> `Agent subagent_type:"general-purpose" model:"fable"` — "Whole-branch pre-merge-style review of the swap idle-thrash effort. Read this brief (`docs/superpowers/reviews/2026-07-08-swap-idle-thrash-fable-request.md`) and the diff `git diff 13a3a72..0a34a15`. Compile and RUN scratch probes against the real branch to verify the runtime claims below, then delete them. Report Critical/Important/Minor + a GO/NO-GO."

## Range to review
- Effort range: `git diff 13a3a72..0a34a15` (base `13a3a72` → merge `0a34a15`).
- Commits: `84c7571` (core fix + guardrails), `baf30c1` (save-side latch clear), `fd09de3` (path-aware merge), `0a34a15` (merge). Files: `wordcartel/src/{swap.rs, editor.rs, app.rs, save.rs}`.

## The problem (as reported)
Laptop left overnight with `wcartel` open on a small *edited* doc → fans/heat; `btop` showed high CPU. It "ramped over time" when left running; not noticed during active use.

## Root cause + fix
The crash-recovery **swap file was rewritten continuously (~10+/sec) while dirty+idle** — an SSD-wear / no-idle-heat pathology (~0% userspace CPU, constant `fsync`s; the "100% CPU spin" hypothesis was refuted by measurement). Cause: the swap scheduler was **level-triggered off `Buffer.last_edit_at`** (set every edit, never cleared), so `swap::due` stayed true forever and `next_deadline_ms` returned a permanently past-due wake-up; the main loop (`app.rs` `run()`, `recv_timeout(min_deadline-now)`) re-dispatched a `SwapWrite` on every wake. Fix (edge-trigger, mirror reconcile/diagnostics version-discard): `Buffer.swapped_version` latch + `swap::pending(dirty, version, swapped_version)`, gating BOTH the loop wake-up deadline (`app.rs:~1611`) and the Tick dispatch (`~1177`). Empirical: 40 writes/4s → 1; saved-idle 8.8%→0% CPU.

## The 3 follow-on issues Codex found (all fixed — verify they hold at runtime)
1. **Save-side latch clear** (`save.rs`): save DELETES the swap file, so `swapped_version` must be cleared on every successful save, else a stale latch suppresses future swaps.
2. **SaveAs-in-flight stale-path race** (`swap.rs` merge): a `SwapWrite` dispatched under the OLD path merging after a rekey would relatch at the wrong path → the merge is now **path-aware** (latch only if written path == buffer's current `swap_path`). Composes with #1 under BOTH merge orderings.
3. **Orphan-deletion removed**: an earlier fix deleted the stale swap on mismatch — unsafe because the workspace allows the same path in multiple buffers (could delete a co-open buffer's live swap). Now intentionally left in place.

## Charge for Fable — the executable checks Codex could not do
Codex is static reading; my regression tests simulate merge ordering with the *synchronous* `InlineExecutor`. Fable should COMPILE + RUN probes against the real branch to verify at runtime:
1. **Threaded merge ordering:** drive the REAL threaded `Executor` (`jobs.rs`) through the SaveAs-in-flight race under BOTH orderings (swap-merge-before-save and save-before-swap). Confirm `swapped_version` ends `None` and the new path recheckpoints — not just in the InlineExecutor simulation.
2. **Durability windows:** confirm no state where unsaved content ends up with NO recovery swap that the OLD code would have written. Exercise: buffer switch (merge routes via `by_id_mut`), undo/redo version bumps, multi-buffer same-path, recovery-on-launch (`assess`/`RecoveryDecision`), and a genuinely-in-flight save.
3. **The pre-existing nuance (NOT introduced here, but confirm not widened):** continuous typing with zero >2 s pauses defers the first checkpoint (max-cap None-branch measures from the latest edit).
4. **Bound sanity:** the guardrail bounds (`≤1` idle, `≥2 ∧ ≤12` continuous) reflect real cadence, not artifacts of the test harness.

## Guardrails already in place (merge-gated, red-checked)
`swap::settled_buffer_is_not_pending_so_the_loop_can_block`, `app::idle_buffer_does_not_thrash_the_swap_file` (red=299), `app::continuous_editing_checkpoints_but_stays_bounded`, `app::save_clears_the_swap_latch_so_later_edits_recheckpoint`, `swap::stale_path_swap_does_not_relatch_after_rekey`. Counting uses a `CountingSwapExecutor` (dispatches, not `ex.drain()` — `reduce` drains internally at `app.rs:~1218`).

## Related
Memory: `wordcartel-swap-idle-thrash`. Engineering-health triage: `docs/engineering-health.md`.
