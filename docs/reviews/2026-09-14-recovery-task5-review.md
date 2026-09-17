# Recovery Task 5 independent review

Reviewed against design revision 5 and implementation plan revision 2.

**Spec/plan compliance: GO. Code quality: GO.**

Final findings: **0 Critical / 0 Important / 0 Minor**. The coordinating agent's
expanded matrix is complete and green, and the final additions were inspected.

## Source review

- Prepared recovery owns the exact existing source lease. Accepted installation
  creates a separate pathless dirty Buffer and independent successor slot, preserving
  the disk Buffer. Metadata and initial body are captured before the recovered Open
  event can enter the plugin callback pump.
- The initial checkpoint and predecessor retirement execute in one FIFO worker
  closure. Strict successor checkpoint completion precedes the retirement CAS;
  only its winning transition permits exact v2 source removal and strict source
  directory sync. Legacy sources and retry predecessors are retained.
- Acknowledgement survives cleanup failure through shared progress. Panic routing
  can distinguish failure before protection from uncertain cleanup after protection.
  Captured leases drop on completion, rejection, or unwind.
- Cancellation suppresses stale foreground effects, and BufferId plus slot identity
  protects replacement documents. Quit rejects uninstalled preparations without
  revoking already-dispatched handoff transactions. Remaining readiness, timeout,
  and real plugin exit-barrier integration belong to Task 7.
- Save suggestions remain separate from document.path. Manual and quit-owned Save
  As use the existing destination validation and overwrite flow. Recovered labels
  visibly distinguish pathless recovered documents and escape provenance for display.

## Review follow-ups addressed during this round

- A cancelled installed import whose worker failed could have neither Ack nor failure
  flag. The former retry predicate treated missing failure as protection and skipped
  a requested retry. It now requires a clean document or acknowledged current content;
  unprotected current content can be retried without deleting its predecessor.
- Preparation and handoff now use distinct checked request identities. This is
  defensive routing hardening for late synthetic preparation outcomes, not a claim
  that the production executor was observed to duplicate job outcomes.
- Installation allocation/dispatch failures retain the installed Buffer's failure
  bookkeeping and single recovered Open event.

The earlier concern about repeating a successfully retired original source was not
classified as a defect: the design explicitly requires reread/revalidation and
specifies visible failure when the selected source has disappeared.

## Validation evidence

The parent-run [integration matrix](evidence/2026-09-14-recovery-implementation/task5-integration-green.log)
reports 13 passed, zero failed. Its maintained tests cover checkpoint create/write/
flush/file-sync/rename/directory-sync failures with surviving source bytes and
released source locks; rejection before and after installation; cancelled-live
retry of current content; distinct preparation/handoff identity; initial metadata
and predecessor capture; cleanup failure and panic before/after unlink; sequential
batch progress after the first checkpoint fails; actual first Save through registry,
picker Enter and overwrite confirmation; and unavailable original-parent fallback.

The real ThreadExecutor test stops inside source removal, after authorization,
reads the successor bytes, cancels the batch, releases the finite barrier, and reads
the same surviving successor after source retirement. Both barrier waits have a
ten-second timeout and the owned executor joins; this does not simulate power loss
or demonstrate bounded shutdown under arbitrarily blocked OS IO.

The [handoff follow-up log](evidence/2026-09-14-recovery-implementation/task5-review-followup-green.log)
includes the owned import/progress cases for source preservation, cancellation before
authorization, quit refusal before installation, and immediate initial protection.
The [Save As log](evidence/2026-09-14-recovery-implementation/task5-save-as-green.log)
reports both manual/quit-owned suggestion and unnamed fallback tests passing.

No cargo commands, production source edits, or delegation were performed by this
reviewer. The execution evidence above is from the coordinating agent, independently
checked against the maintained test bodies rather than rerun by this reviewer.
Task 6 selector/command/open-path integration and Task 7 timeout/process/crash gates
are outside this Task 5 verdict and are not certified here.
