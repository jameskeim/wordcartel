# Recovery Task 4 independent review

Date: 2026-09-14. Reviewed against design revision 5 and plan revision 2.

**Spec/plan compliance: NO-GO pending M1. Code quality: NO-GO pending M1.**

Findings: 0 Critical, 0 Important, 1 Minor.

## M1 — Save-and-close hides the recovery cleanup warning

`wordcartel/src/jobs_apply.rs:63` replaces the successful save completion's status
with unconditional Info `saved — closed`. When `cleanup_saved` retains a checkpoint
because its strict receipt fails (or cleanup panics), `merge_save` correctly emits
a Warning containing the reason, but the same foreground outcome immediately
overwrites it on the pending CloseBuffer path. Users never see the required
retained/uncertain cleanup feedback. The file save and conservative checkpoint
preservation themselves remain safe; this is a feedback defect, not demonstrated
data loss.

Preserve the cleanup warning and its severity while also reporting the successful
close. Exercise the production `apply_job_outcome` wrapper with a pending
save-and-close action and injected cleanup failure; assert the buffer closes,
saved bytes remain correct, the checkpoint survives, and warning feedback survives.

## Inspected guarantees

- Normal checkpoints now capture independent slots, checked generations, and exact
  request IDs. Provisional entries precede dispatch; explicit rejection terminalizes
  the request without publishing a stuck latch. Panic transport retains identity.
- Completion routes through originating BufferId and slot identity. Replacement
  and closure cancel that instance while queued captures keep its owner lease alive.
- The save refactor similarly rejects replacement-instance acknowledgement and
  preserves the prior old-path Save As safety checks.
- Cleanup requires eligible owned metadata, exact body coverage, generation/version
  bounds, normalized association, a matching target fingerprint, same-handle target
  compare and sync, and strict parent sync before unlink. Unchanged uses the same
  receipt path. No foreign or legacy record is authorized by ordinary save cleanup.
- Nested cleanup panic handling preserves successful user-save semantics.
- Association work captures current dirty content and both production completion
  wrappers run follow-ups before quit re-drive. The callback boundary is unconditional
  before the existing quit barrier. Harness save completion paths use these wrappers.
- Inspected cleanup fault tests, request/replacement/panic tests, save integration
  regressions, and source lease lifetime tests. No cargo commands were run in this
  independent review; execution results are the parent's supplied evidence.

This is a Task 4 gate only. Task 5 import/handoff, Task 6 selection/discovery UI and
Task 7 recovery exit blockers remain explicitly unimplemented and are not certified
by this report. Transitional prepared-source consumer warnings are not an additional
finding for this task boundary.
