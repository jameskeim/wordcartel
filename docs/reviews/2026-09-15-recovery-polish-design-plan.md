# Recovery polish — scoped design and implementation plan

The user accepted folding Fable round-2 M-A (cancelled handoff acknowledgement) and
M-D (busy-row clarity) into this effort. M-B reclamation and M-C delayed offers remain
separate follow-ups. Branch remains fix/recovery-safety; no commit/merge/push authorized.
The prior 45-file snapshot and both GO reports remain historical evidence.

## Design

1. `recovery_flow/import.rs::finish` already retrieves the request exactly once and
   validates the live Buffer's slot identity. Apply a successful result Ack (or the
   worker Progress's durable Ack retained through a later cleanup failure) even if
   the opening batch was cancelled, provided generation and captured path still
   match. Record the captured version in swapped_version: later edits remain dirty
   and require their own checkpoint. Clear prior protection failure only for a valid
   Ack. Preserve the existing request cleanup and retry bookkeeping.
2. Cancellation still revokes source-retirement authorization before its CAS, clears
   remaining selected candidates, prevents prepared installation, and suppresses
   late errors/status and quit effects. Latching an Ack grants no deletion authority.
   Closed/replaced slots and path/generation mismatches receive no Ack. An error with
   no durable Ack cannot mark the document protected. Do not change worker IO.
3. In `recovery_picker.rs`, metadata-less busy rows get a plain label such as
   "Recovery files in use" and an explanation that an editor is using these recovery
   files, so they cannot currently be reviewed. Preserve known association/provenance
   filenames when available. Keep source paths inspectable, but do not lead with the
   internal checkpoint filename or raw IO error. Never infer current-session ownership,
   a clean document, or a missing checkpoint from a busy lease.
4. Busy rows remain unselectable and manual-review-only. Preserve geometry, bounded
   drawing, tiny-terminal safety, command registration, scan filtering and dismissal.
   No new IO, locks, timers, metadata fields or cleanup rules are needed.

Command-surface conformance: existing Review Recovery Files command/menu/palette route
is unchanged; no new command, option, key binding or hint is introduced.

## Implementation sequence and evidence

1. Add failing behavioral tests before production edits: cancelled installed handoff
   latches a real durable successor while preserving the source; later edits keep
   the captured swapped_version; close/reload/replacement/path/generation mismatches
   cannot latch; failed checkpoint without Ack remains unprotected. Reuse bounded
   released-lease fixtures; verify remaining batch cancellation/late-quit protection.
2. Add rendered busy-row regression and selection checks, including metadata-less
   and known-path cases, preserving existing tiny-terminal and mouse geometry tests.
3. Independent reviewer verifies this amendment against actual source before production
   edits. Implement narrow branches in finish and picker rendering helpers, then run
   focused import/picker/recovery tests and independent spec/code review.
4. Run workspace tests, build, compile-only tests, workspace/all-target clippy,
   git diff --check, and mandatory advisory terminal smoke; preserve gate logs.
5. Capture a fresh source manifest and reviewed diff. Both Codex and Fable re-review
   the revised implementation before declaring this addendum complete. Preserve old
   findings/reports; mark only M-A/M-D resolved after their evidence and gates pass.

No rustfmt/cargo fmt. Evidence: docs/reviews/evidence/2026-09-15-recovery-polish/.
Windows durability and OS IO shutdown limitations remain unchanged.
