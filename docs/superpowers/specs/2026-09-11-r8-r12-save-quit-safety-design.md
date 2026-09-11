# R8 / R12 — save completion and quit safety

Status: Save All policy approved by the user; implemented and locally validated,
with independent external reviews pending. See the
[implementation report](../../reviews/2026-09-11-r8-r12-implementation.md).
Branch: `fix/quit-save-durability`.
Baseline: `d3aea924988f60702529f6c68a0e332e64d44f1d`.
Findings: [R8 and R12](../../reviews/2026-09-11-pass1-durability-review.md).

## Problem and scope

Save and Quit currently saves the active document and can exit with another dirty
document. Save All snapshots a queue and can miss a document edited after the queue
was formed. Separately, a normal save queued behind Save As can write the old path
and mark the buffer at its new path clean.

This effort fixes those quit and completion boundaries. Recovery-file identity,
checkpoint cadence, in-session recovery, and external-writer conflict policy remain
separate review findings. No automatic migration or deletion of recovery files is
introduced here.

## Section A — Save and Quit policy (approved)

**Decision: save all unsaved ordinary documents, then quit.** The registered
command and the Save and Quit prompt action must enter the same shared flow.
Unnamed documents use the existing Save As picker. Cancel, write failure, unresolved
external conflict, or timeout must leave the editor running and must not strand a
quit drain. Permanent scratch retains its existing separate persistence policy.

The user selected “Save all documents” over saving the active document and asking
about the remainder. There is no further policy question pending for this effort.

The active document's existing explicit-save behavior should be preserved: when
Save and Quit is invoked from a clean unnamed document, the existing filename picker
should not silently disappear as a side effect of filtering the queue to dirty IDs.
The implementation plan must spell out this first-save step separately from the
remaining dirty-document drain.

## Section B — Quit decisions apply to live versions

The current `QuitDrain` stores only a queue and mode. Extend its owned state with
explicit discard decisions keyed by `(BufferId, document.version)`. ReviewDiscard
records the version actually reviewed before removing that queue entry.

When the queue drains, inspect the current buffer set. Any ordinary dirty buffer
that lacks an explicit discard decision for its current version must be processed
in the selected mode. Newly opened dirty documents, formerly clean documents edited
during a save, and edits to an already processed document must all be included.

A discard for an unchanged version remains honored; otherwise a naive final rescan
would repeatedly ask about a document the user already chose to discard. Editing
after discard invalidates that decision. Closed/replaced buffer IDs are not reused
as authorization for another buffer. Cancel discards the entire flow's decisions.

All normal save-driven quit paths use this session-wide decision. The unreachable
legacy Quit action and prompt are removed by the approved cleanup; explicit discard
remains available through Review Each. A failed save never authorizes discard.

Before implementation, audit dispatch reporting: `dispatch_save_reporting` currently
returns only whether a picker opened. A started quit flow needs an unambiguous result
for queued save, picker, conflict, and rejection so it can cancel cleanly when no job
was dispatched. Avoid inferring successful dispatch from the absence of a prompt.

## Section C — Save completion is destination-aware

For an ordinary save result, require the captured chosen destination to still
match the buffer's `Document.path` before updating `saved_version`, `stored_fp`,
checkpoint bookkeeping, or recovery cleanup for that buffer.

If a prior Save As completion changed the path, the old-target write may still
finish (durability work remains FIFO). Its completion must accurately name the
destination written, preserve the current buffer's saved-state metadata, and avoid
deleting that buffer's recovery file. Report a warning that the current document
was not saved by this completion. Successful disk-write plugin notifications still
describe the write that actually occurred.

Save As results retain their intentional rekey behavior in FIFO order. Do not
discard all version-stale saves: saving an older snapshot of the same destination
still records a real successful write and leaves later edits dirty. Do not mark a
buffer dirty unconditionally on a path mismatch either: its existing metadata may
already correctly describe a separate successful save to its current destination.

This guard addresses association changes observed by the editor. It does not claim
to solve external symlink retargeting or the external-process race in R7.

Implementation refinement from the queued-save tests: matching only buffer ID and
version is insufficient because two separate saves can share both. Every dispatched
save returns a unique immutable `SaveRequest`. Completion state belongs to the waiting
action and is set only by that request's merge. Panics preserve the same identity. This keeps an earlier Save As from satisfying
a close/quit waiting on a later ordinary save of the same version. The worker and
result ordering remain FIFO. Synthetic legacy-result tests use an explicit absent
request; both production constructors always bind a real request.

## Regression evidence prepared

The test-only module
[`durability_regressions.rs`](../../../wordcartel/src/durability_regressions.rs)
contains three requirements driven by a deferred FIFO executor and real edit
transactions:

1. An old-path completion leaves the current path dirty when it lacks the newest
   edit and preserves that path's saved version/fingerprint.
2. Save and Quit cannot exit while another unreviewed document is dirty.
3. Save All picks up a document dirtied after the quit flow began, saves it, then exits.

All three fail on the baseline for the expected behavioral assertions, not compilation
errors. [Red test log](../../reviews/evidence/2026-09-11-r8-r12-fix/red-tests.log).
This log records the pre-implementation red phase. Subsequent validation is recorded
in the implementation report; the regression module now also covers edge cases.

## Implementation and validation sequence after approval

1. Independently review this design against the actual caller/state transitions,
   then write and review the implementation plan as required by `CLAUDE.md`.
2. Implement the destination-aware merge and add controls for same-path stale
   versions, sequential Save As operations, closed buffers, plugin notifications,
   and preservation of the new path's checkpoint.
3. Implement shared quit orchestration, version-specific discard decisions, and a
   final dirty-set reconciliation. Cover unchanged discard, edit-after-discard,
   newly created/closed buffers, unnamed Save As, cancellation, conflict, failure,
   timeout, and attempted overlapping quit flows.
4. Update existing tests whose single-buffer action shape changes, while preserving
   their data-loss protections. Run targeted tests, then workspace tests, build,
   test compilation, and workspace clippy. Do not run rustfmt.
5. Run the isolated PTY smoke suite and quote its exact advisory summary. Obtain
   the project's independent final reviews before any merge. Do not commit, push,
   or merge without the user's authorization.

## Command-surface conformance

Keep the existing `save_and_quit` command ID and route keybindings, menu, palette,
and plugin dispatch through the registry's shared handler. No new option, command,
binding, or setting is introduced. Keep the existing command-surface invariant
tests and hint behavior intact; this changes command execution semantics, not its
reachability or registration.

## Review boundary

`CLAUDE.md`'s development pipeline requires an approved design before implementation:
"Get section-by-section approval → an approved design." The user approved Section
A's policy after the design was presented; Sections B and C specify the safety
guarantees supporting it. Independent design/plan/final reviews have not occurred
and are not being claimed by these notes. No merge is authorized by this document.


## Approved pre-merge follow-ups

The user authorized proceeding through the pre-merge checklist and explicitly
approved blocking new manual Save As operations while quitting. Amend the initial
retention choice: remove the unreachable legacy Quit action/prompt chain, preserving
equivalent coverage of the live flow. Request IDs are immutable; completion belongs
to the pending action. Save panics carry request identity through both executors.
Destination cancellations share the quit cleanup helper. Preserve the existing
safe distinction: ordinary Quit supersedes a pending close; Save and Quit refuses
to replace an awaited close-save.

Manual Save As is refused during quit at opening and submission; quit-owned
filename requests remain allowed. Already-dispatched saves must finish and merge
before exit. Keep the conservative warning/cancellation for old-destination saves.

The new waiting phase also covers ordinary Quit with a clean workspace and queued
saves. If edits arrive during that wait, ask for review rather than saving them
implicitly. Waiting for existing merges has the same five-second timeout as an
explicit save/quit. A confirmed quit-summary choice can restart a prior quit attempt
whose picker it replaced; old dispatched writes still complete under their own IDs.

Independent Codex review found that reduce's quit decision preceded the plugin pump.
Exit is now provisional until a shared post-callback `finish_iteration` barrier runs
in both the real loop and the end-to-end harness. Keep discard decisions until that
barrier; rescan any callback edits or new saves, and honor cancellation. Only after
this check may rendering/session persistence lead to the loop's final break. Manual
Save As stays blocked even when the provisional quit flag is already set.
