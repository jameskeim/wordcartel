# Recovery safety — independent implementation-plan review, round 2

Date: 2026-09-14. Reviewed the full revised plan, including its final revision-5
authority-link correction, against the GO specification revision 5 and actual
source at `9e409c0`. Checked the proposed Executor replacement against existing
fields/implementors, worker/result/harness seams, retry obligations, and prior
grounding. No cargo, implementation edits, or delegation.

**Spec compliance: PASS. Plan quality under the documented format decision: PASS.
Technical plan gate: GO.**

Open findings: **0 Critical, 0 Important, 0 Minor.**

## Previous findings

- **P1 resolved:** first-checkpoint full-chain strict-sync obligations remain until
  complete acknowledgement. Same-slot, different-slot, failed-unpublished-allocation,
  and reconstructed-store retries are explicitly covered. Existing directories do
  not discharge the obligation, and tests must prove repeat ancestor sync before Ack
  or retirement.
- **P2 resolved:** required `try_dispatch` reports real acceptance; the compatibility
  default preserves old `dispatch` behavior. All existing implementors are named for
  migration. Recovery inserts provisional identity before immediate execution and
  terminalizes exact rejection synchronously, including initial handoff rejection
  after separate Buffer installation. Unsent jobs release their captured guards.
- **P3 disposition recorded:** the coordinating agent's explicitly linked scoped
  plan-format decision governs this algorithm/interface artifact. This review does
  not claim that the original plan literally satisfied CLAUDE.md's complete-code
  wording or that the user authored that exception. Technical completeness, per-task
  code review, regressions, and final gates remain required.
- **P4 resolved:** the explicit Unix rustix dependency supplies safe effective-UID
  validation. Wrong-owner rejection and real current-owner checks are specified
  without unsafe code or environment-derived authority.

## Readiness and evidence limits

The plan maps D1–D6/R1/R4/R5 to bounded tasks, concrete interfaces and ordered
algorithms, meaningful behavioral red/green requirements, real-process and injected
fault coverage, every live opening/cleanup migration family, exact result-wrapper
and callback sequencing, command/overlay conformance, and final independent gates.
No remaining technical contract requires an implementer to invent a safety policy.

The reference cancellation primitive was separately compiled/executed by the parent;
its recorded log reports two passing CAS-order tests. This reviewer only inspected
that evidence. The proposed Executor/IO/domain APIs have **not** been compiled
against an implementation by this review; successful static source matching is not
a build result. Each task must supply real implementation, retained behavioral
coverage, and its required independent compliance/quality review before advancing.

GO authorizes progression through the established workflow, not commit/merge/push,
nor a claim of implemented recovery safety. Unsupported platform durability,
copy-only legacy limits, trusted-root assumptions, and the existing blocking worker
join remain explicitly documented boundaries.
