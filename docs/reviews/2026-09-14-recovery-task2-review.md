# Recovery Task 2 independent review

Date: 2026-09-14. Scope: design revision 5, plan revision 2 Task 2, actual
`recovery_store.rs`, its codec/tests, `recovery_regressions::store_integration`,
and the added `Fs::canonicalize_existing` RealFs/FaultFs boundary and regression.
Task 1 was reviewed separately. This is a static review; the reviewer ran no cargo
commands and changed no source.

**Spec/plan compliance: NO-GO pending I1 test coverage.**
**Code quality: GO for the inspected production implementation.**
Findings: 0 Critical / 1 Important / 0 Minor. No production correctness defect was
identified in this bounded review; that does not substitute for the missing fault proof.

## I1 — Important: reconstructed-slot and pre-publication failure proofs are missing

Task 2 explicitly requires repeating every ancestor failure with a different slot /
reconstructed store over existing directories, and injecting mkdir failure before
record/slot publication then retrying against partially provisioned roots. The current
`every_ancestor_failure_retries_complete_obligation_on_same_slot` only retries its
same live slot. Neither the other store tests nor the three integration tests exercise
the required discarded-state retry or partial provisioning cases.

These cases protect the prior P1 design finding: existence of directories after an
interrupted allocation cannot count as durability. A same-slot test alone would miss
a future allocator optimization that syncs existing ancestors only when live state
remembers their creation. The production algorithm currently looks conservative, but
the promised regression barrier for this data-preservation invariant is incomplete.

Add parameterized failure-then-fresh-slot tests at every first-checkpoint strict-sync
boundary and at allocation mkdir boundaries. Preserve the failed attempt's files;
drop its slot to model lost process state, then use a fresh slot against the same root.
Assert successful retry journals the entire resolved chain through filesystem root
before returning an Ack, does not adopt/remove the old owner or lock, and leaves any
old checkpoint intact. Include failure while creating an intermediate configured-root
directory before any owner exists. Re-review after the tests pass.

## Confirmed implementation properties

- Slot creation is memory-only; generation reservation, identity comparison and Debug
  do not lock the worker mutex. The Arc retains the lease for cloned queued ownership.
- Exclusive owner-directory allocation retries collisions without adopting tombstones.
  Canonicalization resolves the trusted configured root before creating/validating the
  private protocol hierarchy. The stable lock is created/acquired before publication.
- Captured generations are reserved before work; attempts consume their generation,
  stale/replayed work cannot overwrite later data, and overflow refuses without wrap.
- Checkpoint writes use exclusive temp creation, write/flush/file-sync/close/rename,
  then strict owner sync. Until the first Ack, all resolved ancestors are synced on
  every attempt. Failure and mutex poisoning do not clear that obligation. Later
  checkpoints retain the owner-directory barrier.
- Codec validates magic/version, independent header and body limits, exact lengths,
  UTF-8, owner/generation/path metadata and timestamp range. Exactly MAX_OPEN_BYTES
  remains valid independently of the header. Raw Unix paths roundtrip; foreign-platform
  paths cannot become local destinations. Decode itself grants no deletion authority.
- Unsupported filesystem capabilities propagate failure rather than producing an Ack.
  No destructor performs sync, removal or retry. Canonicalization remains inside Fs.

## Evidence and limits

Inspected existing logs: `task2-green.log` reports 11 passing store tests;
`task2-integration.log` reports 3 passing integration tests. The parent reports clean
Task 2 clippy and a missing-sync mutation that the existing same-slot test detects.
The overwritten initial compilation-failure log is not evidence of original TDD
chronology. No independent runtime, Windows or power-loss verification is claimed.
Discovery, save receipts, handoff and UI lifecycle are later tasks, not implemented
or certified by this Task 2 verdict.
