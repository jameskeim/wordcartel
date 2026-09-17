# Recovery Fable production corrections — 2026-09-15

Original Fable report/probes and source snapshot are preserved. These are working-tree
corrections requiring revised independent review and coordinator gates.

## I1: foreground quit timeout

The five-second deadline now exists only while quit is in progress and covers concrete
handoffs/association requests. An ordinary checkpoint blocking a queued association is
included; an ordinary background checkpoint alone is excluded. Preparations do not block
exit and are cancelled by the existing quit boundary, without a timeout.

Timeout detaches exit blockers while preserving eventual result routing and successful
Ack latching. It cancels handoff Pending retirement CAS only during that foreground quit,
clears remaining selected preparations, and shows the exact quit-cancelled warning once.
Detached late errors cannot affect a subsequent quit. Matching successful late ordinary,
association, and handoff checkpoints may still update protection metadata. Explicit close
or Discard cancellation keeps its existing no-latch semantics. A cancelled/failed handoff
still supplies the bounded automatic retry floor implemented by the coordinator; successful
or already durable checkpoints clear that floor without inventing metadata acknowledgements.

Tests cover a six-second background checkpoint with no warning/retry, a slow background
handoff that still retires safely, detached association success/error, cancelled preparation
noninstallation, and existing quit/Discard cases. Original Fable P1 is the independently
recorded failing behavior; these focused regression tests were added with the correction.

## I2 and M6: strict Save As cleanup, user approved

`AssociationPolicy` explicitly distinguishes ordinary Save from accepted Save As and
carries the document path captured with the save's slot/content snapshot. The strict
receipt accepts the committed association, or for Save As only, no association or that
captured previous association. It still checks slot owner, generation ceiling, edit
version, exact body, same-handle saved fingerprint/bytes/file sync, and parent sync.

`CleanupOutcome::RetainedByRule` separates expected newer/divergent/association retention
from IO faults and corrupted identity. Save completion reports expected retention as Info;
IO, identity mismatch, interrupted cleanup and post-unlink uncertainty remain Warning.
`cleanup_saved` preserves the ordinary policy; `cleanup_saved_with_policy` is the explicit
Save As-capable worker entry point. The additional argument has a documented item-local
clippy rationale, with no global lint weakening.

Tests cover ordinary clean A-to-B Save As and actual recovered first Save As retiring the
owned successor without warnings or another checkpoint; named/pathless receipt eligibility;
foreign owner, newer generation/version, divergent body and unrelated association refusal;
and strict sync/stat/unlink failures retaining the source with warnings. The earlier
contract check is marked superseded by the user's 2026-09-15 approval, and spec/plan record
both I2 and the coordinator's I4 retry-delay approval.

## I3 and M1: dismissible errors, retained contextual work

Unavailable tokenless rows have per-session dismissal identity from source path, displayed
timestamp and failure reason. Closing or leaving such a row unselected suppresses repeated
automatic offers; manual review always shows it. Root-level scan errors are shown once
automatically per distinct error and remain available through manual review.

Closing/replacing a picker invalidates its manual scans by exact request identity but
retains unrelated contextual queues, requests and offers. Manual results are prioritized
over deferred automatic offers, preventing retained work from stranding a Loading picker.
Tests exercise corrupt entry across two contextual opens, queued work surviving dismissal,
manual review afterward, and manual scan completing while an automatic result is deferred.

## M3, M4, M5: shared helpers, bounded scan allocation, readability

- Canonical missing-suffix normalization now has one home in `fsx`; flow reexports it and
  discovery uses the same implementation. Owner validation has one home in store codec.
- Legacy/v2 readers pass borrowed bodies to a consumer. Scan returns only Candidate metadata;
  preparation alone owns the full body. `swap::parse_borrowed` shares existing legacy header
  parsing, while the old owning `parse` remains compatible for legacy callers.
- Hand-wrapped touched flow/picker functions and expanded dense request structures without
  invoking a formatter. Existing test wrappers and parent regression-module declarations
  from the coordinator's C1 correction are preserved.
- Migrated the existing legacy repeated-selection test to expect F1's synchronous protected
  document focus instead of an unnecessary prepare job.

## Validation

Evidence directory: `docs/reviews/evidence/2026-09-14-recovery-implementation/`.

- `fable-flow-final.log`: 35 flow tests pass on final source, including retry integration.
- `fable-picker-final.log`: 12 picker tests pass on final source.
- `fable-recovery-green.log`: 158 recovery tests pass, one ignored child entry point; this
  preceded only final hand-wrapping and the cancelled-handoff retry-floor edge correction.
- `fable-cleanup-green.log`: 14 cleanup tests pass.
- `fable-save-as-green.log`: six Save As tests pass; `fable-first-save-as-green.log`: actual
  recovered first Save As test passes.
- `fable-discovery-green.log`: 12 discovery tests pass; `fable-legacy-green.log`: 37 legacy
  swap tests pass through the shared borrowed parser.
- `fable-fixes-initial.log` preserves initial failures: three additional process-inheritance
  test acquisition races fixed by coordinator I5 work, plus the obsolete legacy focus-job
  test assumption migrated here. Intermediate compile/test output is not final-gate proof.

Coordinator owns C1/I4/I5/I6, whole-workspace/clippy/smoke gates, revised source manifest,
and independent Fable/Codex rereview. No commit, merge or snapshot rewrite performed here.
