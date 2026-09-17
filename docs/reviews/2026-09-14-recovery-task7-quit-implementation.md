# Task 7 quit and timeout integration

Implemented on `fix/recovery-safety`, uncommitted; coordinator retains whole-effort ownership.

## APIs and behavior

- `recovery_flow::has_pending_work` is now a concrete exit blocker query: dispatched handoffs (including cancelled requests until completion/timeout), association checkpoint requests, and association intents. Scans, preparations, ordinary checkpoints, and persistent protection-error flags do not block exit.
- `imports_busy` preserves the prior UI/controller gating meaning independently of exit readiness.
- `pending_deadline` registers the earliest outstanding checkpoint/import deadline at start + 5000 ms in the timer table. `timeout_tick` runs from the existing pre-recv seam, acts at the exact deadline, and detaches each expired request once while retaining worker routing and leases. It cancels pending handoff CAS and clears association intents only for detached slots. A later completion cannot cancel a subsequent quit or install an expired preparation.
- Timeout cancels an active foreground quit with exact sticky Warning `Recovery IO is still pending; quit cancelled`. Outside quit it reports `Recovery IO is still pending` without claiming a quit existed. Subsequent timeout ticks are idle for detached requests.
- Existing worker IO and executor join remain unchanged. This bounds foreground state only, never OS IO or shutdown duration.
- The shared after-callback hook cancels pending preparations/remaining batch whenever quitting; already dispatched handoffs continue. Quit empty-queue readiness waits for recovery, and normal outcome wrappers wake a waiting quit. The final existing dirty rescan and discard-version map remain authoritative.
- Explicit review Discard now cancels the matching import/checkpoint ownership before re-driving quit. Its dispatched handoff still blocks until terminal outcome or timeout.
- Migrated jobs_apply obsolete SwapWrite panic handling/test to an actual registered recovery request, including sticky error and exact request latch cleanup. Added `RecoveryRequestId::for_test` for the coordinator's synthetic durability routing tests.

## Evidence

All files under `docs/reviews/evidence/2026-09-14-recovery-implementation/`:

- `task7-quit-initial.log`: original five focused tests pass.
- `task7-quit-timeout-mutation.log`: intentionally removing the recovery timeout call from `timers::pre_recv` makes three timeout tests fail; two non-timeout tests pass. Mutation restored immediately. This is postimplementation sensitivity evidence, not preimplementation TDD.
- `task7-flow-green.log`: 25 flow tests pass after restoration.
- `task7-timers-green.log`: nine timer tests pass.
- `task7-jobs-green.log`: 28 jobs-application tests pass.
- `task7-quit-green.log`: six focused tests pass, including a later scope-isolation test. Two subsequent status wording refinements await coordinator-wide final verification.

Focused coverage: handoff wait through merge; exact five-second timer/no-spin/detachment; cancelled preparation never installed; late success never reinstates quit; Discard preserves source and waits then respects new edits; association success/panic/timeout; ordinary checkpoint exclusion; timed-out blocked association intent removal without losing another buffer's intent.

Full process crash, Lua journeys, all-workspace gates, and independent reviews remain coordinator-owned. Source/retirement fault matrices and cancellation CAS tests from Task 5 remain applicable; this implementation adds foreground integration rather than replacing them.

## Independent-review correction: cancelled filename continuation

Reviewer found that `quit::cancel` cleared the drain and picker owner but left `pending_save_as = ContinueQuitDrain`. A recovery timeout or failure while the quit-owned Save As picker was open therefore left quit permanently in progress. Cancellation now clears only that quit continuation; nonquit CloseBuffer continuation is preserved. Existing picker or overwrite confirmation consistently becomes an ordinary manual Save As, with no later quit action attached.

Added two regressions before the fix: timeout/panic/failure during an actual opened quit-owned filename picker, and actual overwrite submission after cancellation. Both fail in `task7-quit-picker-red.log`; all eight focused quit tests pass after the fix in `task7-quit-picker-green.log`. The picker test also proves manual Save As is available again and CloseBuffer continuation remains intact.
