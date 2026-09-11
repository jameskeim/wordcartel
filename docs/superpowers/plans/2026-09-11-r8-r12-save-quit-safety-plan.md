# R8 / R12 implementation and verification plan

Design: [save/quit safety](../specs/2026-09-11-r8-r12-save-quit-safety-design.md).
Branch: `fix/quit-save-durability`. No commit, push, or merge is authorized.

The user approved Save and Quit saving all unsaved ordinary documents. Keep the
active document's explicit Save/Save As first step; then process all remaining dirty
documents, including those created or edited while saving.

## 1. Regression requirements

Add `durability_regressions.rs` as a test-only module. Its deferred executor queues
real save jobs and delivers them in production FIFO order. Use real transactions
for edits. First verify the original three safety assertions fail on the baseline:
old-path completion cannot clean a new path; Save and Quit cannot lose another
dirty buffer; Save All must include newly dirty buffers. Retain the red log.

## 2. Destination-aware completion

In `save::do_save_to`, ordinary-save completion checks the captured `chosen_path`
against the live `Document.path`. A mismatch leaves saved version/fingerprint,
checkpoint bookkeeping, recovery files, and session migration state untouched.
Finish the save status with a warning naming the destination actually written.
Still emit the successful disk-write event for that destination.

Cancel a matching awaited post-save action on a write error or destination mismatch.
Do not infer success from an existing `saved_version`: a previously clean version
can have a failed new save attempt. Keep successful stale-version saves to the same
destination and FIFO Save As rekeys working as before.

Bind the action to a unique `SaveRequest` returned by dispatch, not just the buffer
ID/version. `finish_save_action` marks only that exact request complete. The generic
post-save handler additionally requires its completion flag before acting. This
refinement was required by a same-version Save As / ordinary-save / close regression.

## 3. Quit orchestration

Put session-wide orchestration in `quit.rs`; keep dispatch hubs delegating.

- `QuitDrain::new` initializes queue/mode plus a map of discarded versions and the
  optional `(BufferId, version)` currently displayed for review.
- `quit::save_and_quit` explicitly saves the active buffer with
  `ContinueQuitDrain`; `quit::start` starts summary-prompt Save All/Review Each.
  Reject overlapping quit/close-save flows with the existing busy warning.
- `quit::refill` recomputes the outstanding dirty set when the queue empties,
  excluding only unchanged versions explicitly discarded in this flow.
- Review actions operate on the displayed identity/version, not arbitrary current
  focus. A stale Save decision reopens the review; a stale Discard authorizes only
  its old version. Cancellation clears the flow's decisions and awaited quit.
- `jobs_apply::drive_quit_drain` quits only after refill finds no outstanding work;
  its completion handler pops only the result's own front queue entry.
- Remove the now-unreachable legacy `PostSaveAction::Quit` chain as authorized in
  the pre-merge checklist; preserve safety tests on the live quit flow.
- Replace save dispatch's picker-only boolean with a private enum distinguishing
  queued work, picker, conflict, and rejection. Abort a quit when no save was queued
  or picker opened; preserve the conflict/error UI.

## 4. Edge cases and controls

Cover version-bound discard; edits after discard; focus changes under a review;
stale review Save; unnamed Save As cancellation; conflicting files; save failures;
previously clean versions with failed save attempts; cancellation and timeout with
late results; repeated quit commands; same-path stale saves; sequential Save As;
checkpoint preservation and truthful plugin events on old-path completion.

Update old single-buffer tests to expect the shared drain and drive completions
through `apply_job_outcome`, which performs the production drain continuation.
Preserve their underlying no-data-loss assertions.

## 5. Validation and review

Run targeted regressions, full workspace tests, workspace build and test compilation,
and `cargo clippy --workspace --all-targets`. Run `scripts/smoke/run.sh` with isolated
state/config/cache and retain the exact advisory summary. Do not run rustfmt.
Review the final diff for identity/version mistakes, cancellation leaks, accidental
recovery cleanup, and command-surface drift. Record validation and any limitations.

Independent external reviews required by the project workflow are not claimed as
completed by local diff inspection. Leave the result uncommitted for those reviews.

## Command-surface conformance

Keep `save_and_quit` registered under the existing ID; keys, palette, menu, and
plugin dispatch reach the same handler. Introduce no new commands or user-settable
options. Existing palette/menu completeness and hint-resolution tests remain gates.
