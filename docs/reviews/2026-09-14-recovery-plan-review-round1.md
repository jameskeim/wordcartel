# Recovery safety — independent implementation-plan review, round 1

Date: 2026-09-14. Reviewed the proposed plan against design revision 4 and actual
source at `9e409c0`, including its final harness/caller-census additions. Read-only
review; no cargo, source/plan edits, or delegation.

**Spec compliance: NOT READY. Plan quality: NOT READY. Gate: NO-GO.**

Findings: **0 Critical, 3 Important, 1 Minor.** The plan correctly carries most
of the accepted architecture, including combined worker handoff, strict same-handle
save receipts, copy-only legacy import, request identity, bootstrap, real input and
plugin coverage, overlay cancellation, and preserved quit behavior. The cancellation
and deferred-executor reference snippets are consistent with current signatures.

## Important P1 — Provisioning sync obligations must survive failure and retry

Tasks 1–2 record newly created directories/parents and require their strict sync
before acknowledgement, but do not specify retaining those obligations after a
failed first checkpoint. `RecoverySlot` publishes its lease once and remains live
after that failure. A subsequent retry sees existing directories and can therefore
skip the "newly provisioned" ancestor list, strictly syncing only the owner.

Example: create recovery-v2 and owner, write checkpoint, then fail the sync linking
recovery-v2 into its parent. Retry with the same slot finds those directories
already present. A success receipt without resyncing the failed ancestor can retire
the source even though the replacement's hierarchy was never acknowledged durable.
Existence after an IO error is not equivalent to persistence of its directory entry.
Current `swap::state_dir`/`Fs` contain no retained provisioning-debt mechanism to
provide this implicitly.

**Correction:** specify either a retained provisioning obligation in the slot/store
that is cleared only after the complete required sequence succeeds, or a conservative
strict-sync chain to the trusted existing ancestor on every relevant checkpoint.
Cover failure after rename, before/at each ancestor sync, retry with the SAME live
slot, and retry through a newly reconstructed store after process restart. The
regression must verify the required ancestor sync happens again before any Ack or
source retirement, not only that initial failure withheld an Ack.

## Important P2 — Successful queue acceptance is not observable through the actual Executor API

Task 4 says to set a matching request latch "only ... after successful queue
acceptance" and introduces `DispatchOutcome`, but the actual `jobs::Executor::dispatch`
returns `()`. `ThreadExecutor::dispatch` ignores `tx.send(job)` failure and also
silently does nothing if its sender is absent. The plan does not change that API
or provide an alternative acknowledgement path. The proposed test executor also
returns `()` by contract.

As written, flow cannot distinguish accepted work from a dropped job. It can leave
an ordinary checkpoint request/latch stranded forever; the quit-only timeout does
not restore normal protection scheduling. A dropped handoff job also needs explicit
request termination and source-lease release rather than fictitious queued state.

**Correction:** define a concrete acceptance/error API or an equally concrete
terminal-failure outcome delivery mechanism. Enumerate the required migrations:
InlineExecutor, ThreadExecutor, BenchExecutor, the existing durability DeferredExecutor,
CountingSwapExecutor, DrainSpy, and the new DeferredRecoveryExecutor, plus affected
dispatch callers. Specify synchronous rejection status and latch/request cleanup,
including initial combined handoff rejection after Buffer installation. Add a
rejecting executor regression proving no stranded protection state or retained
source lock. Do not describe an ignored `send` result as successful acceptance.

## Important P3 — The artifact explicitly substitutes algorithms for the project's complete-code plan gate

The plan calls itself an "algorithm and interface plan" and provides complete code
only for HandoffProgress and a test executor. The actual IO, checkpoint/handoff,
receipt cleanup, request lifecycle, wrapper integration, and behavioral regression
bodies are left for the implementer. This is useful architectural task decomposition,
but differs from `CLAUDE.md` pipeline step 3: "Task-by-task, COMPLETE code, TDD steps,
grounded in real signatures." There is no explicit user override of that requirement
in the reviewed task.

This matters to the gate rather than just document length: omitted bodies prevent
static verification of precise control flow, resource transfer, error rollback,
and test invocation—the kinds of mismatch P1/P2 already expose. Lists of assertions
and cargo filters do not supply executable red/green regressions.

**Correction:** provide complete task packets, linked from this overview if useful,
with concrete implementation and behavioral-test code for each bounded task, and
review those packets against actual interfaces before execution. Preserve the
overview as a dependency/coverage map. If a smaller just-in-time plan workflow is
deliberately authorized instead, state that explicit governing exception; do not
declare this artifact compliant with a complete-code gate by relabeling interfaces.

## Minor P4 — Unix ownership validation has no selected safe effective-UID source

The proposed `Fs::validate_private_dir` requires Unix owner/private-mode validation.
`MetadataExt::uid` can read the directory owner safely, but the plan does not name
how it obtains the effective process UID for comparison. Current crate dependencies
do not expose such an API; libc is proposed only for constants, and `libc::geteuid`
requires an unsafe call prohibited by `wordcartel/src/lib.rs`. The local lockfile
contains safe-wrapper crates transitively, which does not make them direct imports.

**Correction:** choose and document a safe effective-UID provider (including any
target dependency/feature), or a concrete alternate ownership-validation mechanism
consistent with the trusted-root contract. Do not use environment USER/UID strings
as security authority or introduce an unsafe block. Pin a wrong-owner rejection test
through the injected seam, with platform/runtime scope stated honestly.

## Re-review

Resolve P1/P2 mechanism gaps, P3 gate-completeness discrepancy, and P4 feasibility
detail, then re-review the exact revised plan/task packets. No requested correction
requires changing approved D1–D6. Existing passing spec review remains valid; this
NO-GO applies to implementation readiness of this plan.
