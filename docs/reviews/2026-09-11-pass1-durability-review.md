# Pass 1 — durability and buffer lifecycle review

Revision: `d3aea924988f60702529f6c68a0e332e64d44f1d`.
Disposition: review complete for the scope and scenarios below; defects are not fixed.
Follow-up: R8 and R12 now have [uncommitted fixes and validation](2026-09-11-r8-r12-implementation.md)
on `fix/quit-save-durability`. This report preserves the original baseline findings.
See the [baseline/map](2026-09-11-pass0-baseline-map.md) and
[original findings](2026-09-10-wordcartel-recovery-findings.md).

The review confirms **11 implementation findings, including the original three,
plus one design gap**. Review-local IDs R1–R12 are not backlog IDs. The highest
priority is normal-exit data loss (R12), followed by save destination bookkeeping
(R8) and recovery ownership/discovery (R1, R4, R5).

All findings below have reproduction evidence. The retained tests assert the
observed defective behavior, so PASS means reproduced, not corrected. The final
suite contains 16 tests, including positive controls and variants of shared root
causes. Its [log](evidence/2026-09-11-continuation/quit-paths.log) records the
original source SHA256. [Probe source](evidence/2026-09-11-continuation/probes.rs)
and [runner instructions](evidence/2026-09-11-continuation/README.md) are retained.

## R12 — P1: Quit paths can exit with other unsaved documents

**Sources:** [`registry::builtins`](../../wordcartel/src/registry.rs),
[`save::dispatch_save_and_quit`](../../wordcartel/src/save.rs),
[`jobs_apply::apply_result` and `drive_quit_drain`](../../wordcartel/src/jobs_apply.rs).

Two reachable variants were reproduced:

1. With A and B dirty, run the registered **Save and Quit** command while A is
   active. Only A is saved. `PostSaveAction::Quit` checks A's dirty state and sets
   `editor.quit = true`; B remains dirty, its file contains the old bytes, and no
   prompt asks about B. The fixture has no recovery file for B.
2. Start **Save All** with A dirty and B clean. While A's deferred save is pending,
   switch to B and type. B is absent from the snapshot queue. A's completion drains
   that queue and exits while B is dirty.

**Impact:** unsaved edits can be lost during a normal user-requested exit; no crash
or filesystem fault is necessary. The production normal-exit path persists session
metadata and permanent scratch, not every ordinary dirty document's body.

**Evidence:** `save_and_quit_leaves_another_dirty_document_unsaved` and
`save_all_quit_misses_a_buffer_dirtied_after_queue_snapshot`, using real registry
dispatch/completion handlers and FIFO execution.

**Direction:** make every save-and-exit path honor a session-wide dirty check.
Before final exit, reconcile the current dirty set with saved and explicitly
discarded decisions, rather than treating an empty old queue as proof. This follows
the [multi-buffer quit intent](../superpowers/specs/2026-06-28-wordcartel-06-multi-buffer-workspace-design.md).

## R8 — P1: A save to the old path can mark the new path clean

**Source:** [`save::do_save_to`](../../wordcartel/src/save.rs), especially successful
merge bookkeeping, and `prompts::perform_save_as`.

Queue Save As from A to B at version 1. Before completion, edit to version 2 and
issue ordinary Save. The second dispatch still captures A. In normal FIFO order,
the first merge rekeys the buffer to B; the second writes A and unconditionally
sets `saved_version = 2` and A's fingerprint on the buffer now associated with B.

**Impact:** B holds version 1 while its displayed buffer contains version 2 and
reports clean. A receives the later text unexpectedly. Closing/exiting can discard
the in-memory version without warning because the dirty predicate is false.

**Evidence:** `normal_save_queued_after_save_as_marks_wrong_path_clean` verifies
both files' distinct contents, the final path, and the false clean state. No
out-of-order worker execution was assumed.

**Direction:** make save bookkeeping destination/generation-aware, or serialize
path-changing saves so subsequent saves cannot capture the wrong association.

## R1 — P1: Recovery filenames do not uniquely identify their owners

**Sources:** [`swap::swap_path`, `dispatch_swap_write`, `delete`](../../wordcartel/src/swap.rs)
and successful save cleanup in [`save::do_save_to`](../../wordcartel/src/save.rs).

Every unnamed buffer in one process uses `scratch-<pid>.swp`. Named buffers sharing
the same path use the same filename, including across processes. Checkpointing B
overwrites A's recovery bytes without invalidating A's `swapped_version`. Saving A
can remove B's recovery copy while B remains dirty and believes it is checkpointed.

**Evidence:** the original `untitled_checkpoints_collide` was rerun; new variants
`save_as_of_one_unnamed_buffer_deletes_anothers_checkpoint` and
`named_buffers_share_checkpoint_and_saving_one_removes_the_other` verify cleanup
and latches. A separate [two-process probe](evidence/2026-09-11-continuation/sessions.log)
uses the normal library's checkpoint functions in two distinct live helper
processes with shared isolated XDG state: B replaces A's recovery content.
Those helpers exercise persistence directly; they are not two full terminal UIs.

**Direction:** unique document/session recovery ownership throughout naming,
cleanup, Save As, and orphan discovery. A filename-only patch is insufficient:
discovery and migrations must still locate every recoverable document.

## R4 — P1: In-session opening skips recovery and Save deletes the unoffered copy

**Sources:** [`app::run`](../../wordcartel/src/app.rs) has launch recovery assessment;
[`workspace::open_as_new_buffer`](../../wordcartel/src/workspace.rs) and
[`session_restore::open_into_current`](../../wordcartel/src/session_restore.rs)
do not perform equivalent assessment. `file_browser::file_browser_enter` routes opening
to the workspace function.

Seed a valid, divergent swap for a crashed document and open its file while the
editor is already running. Both additive opening and throwaway-buffer replacement
load the disk text with no recovery prompt or staged recovery body. An ordinary
Save then removes that swap, even when the disk write is `Unchanged`.

**Impact:** an existing recoverable copy can be permanently deleted without ever
being offered to the user.

**Evidence:** `opening_during_session_skips_recovery_and_save_deletes_carrier`
tests both opening branches and first verifies that the shared `assess` oracle
would return the lost body. The picker caller was traced in source; the probe
invokes its workspace operation directly.

**Direction:** centralize recovery assessment across all open entry points and
retain ownership of unreviewed carriers until a recovery/discard decision is made.

## R5 — P1: Recovering unnamed work deletes its carrier without checkpointing it

**Sources:** [`prompts::resolve_prompt`, `PromptAction::Recover`](../../wordcartel/src/prompts.rs),
[`save::load_recovered`](../../wordcartel/src/save.rs), and swap timer gating.

Startup stages an orphan body/path. Pressing Recover replaces the buffer and
deletes the orphan file. The replacement is dirty but has `last_edit_at = None`;
the modal path bypasses the ordinary edit-timestamp hook. Pre-render `advance`
does not repair this. Even a minute later there is no swap deadline.

**Impact:** a second abrupt termination before another ordinary edit/save can lose
the work that was just recovered. The only existing recovery carrier has been removed.

**Evidence:** `recovering_unnamed_work_deletes_carrier_without_arming_checkpoint`
uses the actual Recover key path, runs pre-render advance, verifies carrier deletion,
and checks the absence of a future checkpoint. This is deterministic lifecycle
testing, not a physical power-loss experiment.

**Direction:** transfer recovery ownership safely before deleting the old carrier;
arm checkpointing for restored dirty content independently of keyboard timestamps.

## R2 — P1: Inactive dirty buffers have no checkpoint deadline

**Source:** [`timers::swap_deadline` and `on_tick`](../../wordcartel/src/timers.rs).

Only the active buffer participates in checkpoint scheduling. Edit A and switch
to clean B before A's idle delay expires: A can remain uncheckpointed indefinitely.

**Evidence:** original `inactive_dirty_buffer_has_no_checkpoint_deadline` rerun.
The switch path was also inspected; it does not flush the departing buffer.

**Direction:** compute pending deadlines and dispatch writes for all dirty buffers
by identity without changing UI focus.

## R6 — P1: Continuous typing can prevent the first checkpoint indefinitely

**Source:** [`swap::due` and `next_deadline_ms`](../../wordcartel/src/swap.rs).

Before a first successful swap, both the idle and maximum deadlines are based on
the latest edit timestamp. Every new edit therefore moves the nominal maximum
deadline. Typing every second for two minutes produces no checkpoint; pausing
then produces one normally.

**Evidence:** `uninterrupted_typing_never_creates_first_checkpoint`. The existing
`app::tests::continuous_editing_checkpoints_but_stays_bounded` explicitly inserts
a three-second pause first, so it covers only the already-checkpointed branch.

**Direction:** retain the first outstanding unsaved-edit time, or an equivalent
stable maximum deadline. The [crash-safety spec](../superpowers/specs/2026-06-23-wordcartel-04b-async-crash-safety.md)
explicitly promises a forced write during an uninterrupted burst.

## R9 — P1: Palette edits bypass recovery timestamping

**Sources:** [`app::reduce_dispatch` and `advance`](../../wordcartel/src/app.rs),
the palette interceptor, and swap deadline gating.

Open a saved file and use the palette's Delete Line command as the first edit.
The buffer changes and becomes dirty, but no edit timestamp is recorded. A minute
of idle produces no checkpoint. Typing an ordinary character then allows it.

**Evidence:** `palette_edit_has_no_checkpoint_until_an_unrelated_normal_edit`
exercises palette Enter, reduce, advance, and ticks. The existing
`palette_dispatched_edit_skips_version_hook` test explicitly locks in the missing
timestamp as a preserved dispatch asymmetry.

**Direction/design conflict:** recovery scheduling must follow committed changes,
not the dispatch branch that happened to produce them. Preserve any necessary
overlay ordering, but revisit the test/contract that demands no timestamp. The
same architectural question applies to plugin/non-active edits; those variants
were not separately reproduced here.

## R3 — P2: Failed checkpoint writes immediately retry

**Sources:** [`swap::dispatch_swap_write`](../../wordcartel/src/swap.rs) completion
and [`timers::swap_deadline`](../../wordcartel/src/timers.rs).

A failed write clears `swap_in_flight` without changing the overdue deadline.
Persistent disk/permission failure therefore schedules another attempt immediately
on every completion cycle.

**Evidence:** original `failed_checkpoint_immediately_rearms` rerun with three
consecutive injected create failures at the same virtual time.

**Direction:** explicit retry state and bounded backoff, while preserving dirty
state and visible failure information.

## R10 — P2: Header overhead makes an accepted-size document's swap unreadable

**Sources:** [`swap::serialize`, `read_swap_capped`, `assess`](../../wordcartel/src/swap.rs),
[`file::open`](../../wordcartel/src/file.rs), and `limits::MAX_OPEN_BYTES`.

The document reader accepts 64 MiB of text. The swap writer adds a header, but
the recovery reader applies the same 64 MiB cap to the entire serialized file.
A valid swap with a 64 MiB body is silently treated as absent (`OpenNormally`).

**Evidence:** `header_makes_max_size_valid_document_unrecoverable` verifies actual
file-open acceptance, valid serialized format/body, and failed recovery discovery.

**Direction:** define a recovery format limit accounting for header overhead and
the actual permitted in-memory document size; do not conflate over-cap recovery
data with an absent carrier.

## R11 — P2: A modal prompt suppresses checkpoint ticks but leaves zero-timeout wakes

**Sources:** [`prompts::intercept`](../../wordcartel/src/prompts.rs),
[`timers::next_wake`](../../wordcartel/src/timers.rs), and the `app::run` receive loop.

Type, then leave a modal prompt open. Its interceptor consumes/ignores ticks.
Once the swap is due, the deadline remains at or before the current time, so
`recv_timeout` repeatedly receives a zero duration. No checkpoint runs until the
prompt closes. Other expired subsystem deadlines can make the wake happen even earlier.

**Evidence:** `modal_discards_due_swap_ticks_and_leaves_immediate_wakeup` verifies
the pending swap, overdue loop wake, absent carrier, and a successful checkpoint
after cancellation. CPU consumption is inferred from the zero-timeout receive loop;
no CPU percentage was measured.

**Direction:** process safe background timer work while modal, or coherently suspend
both work and its deadlines. Recovery protection should not depend on prompt dismissal.

## R7 — P1 design gap: Queued saves overwrite intervening external edits

**Sources:** [`save::dispatch_save_reporting` and `do_save_to`](../../wordcartel/src/save.rs).

The external-modification check happens before queueing. If another process writes
the file before the worker executes, the worker overwrites that version without a
second comparison and reports success.

**Evidence:** `queued_save_overwrites_external_edit_after_foreground_check` uses a
deferred FIFO executor and a real disk rewrite between dispatch and execution.

**Design distinction:** the [4b spec, §4.3](../superpowers/specs/2026-06-23-wordcartel-04b-async-crash-safety.md)
explicitly puts the check on the foreground and instructs the worker to write.
This behavior follows that mechanism; it is not a claim that the implementation
deviated from it. The resulting data-loss window needs a design decision.

**Direction:** consider a worker-side expected-fingerprint check and a conflict
result delivered to the foreground. Handle prior queued saves by the same editor
without false conflicts. A second check narrows the window; it does not by itself
provide atomic compare-and-replace against arbitrary external writers.

## Failure and recovery matrix

| Scenario | Disk document | Recovery / buffer outcome | Evidence |
| --- | --- | --- | --- |
| Create, partial write, chmod, flush, file-sync, rename failure | Original preserved | Dirty stays set, existing swap preserved, temp removed | `save_failure_matrix_preserves_dirty_state_and_original_before_rename` |
| Directory-sync failure after rename | New bytes visible; crash durability uncertain | Dirty stays set, existing swap preserved, error reported | Same matrix, `SyncDir` case |
| Two unnamed buffers checkpoint | No saved document | Later checkpoint replaces earlier; both may claim checkpointed | R1 |
| Two live processes checkpoint one named document | Original untouched by checkpoint | Later process replaces earlier's carrier | Cross-process helper |
| Save As one unnamed buffer while another has a swap | Saved target correct | Other buffer's carrier deleted | R1 variant |
| Open crashed document while already running, then Save | Original loaded/saved | Divergent carrier never offered, then deleted | R4 |
| Recover unnamed body, then wait | No named document | Old carrier removed; dirty body has no checkpoint deadline | R5 |
| Switch before debounce / type without first pause / palette-only edit | Original remains | No new checkpoint in the tested interval | R2 / R6 / R9 |
| Save As B followed by queued normal save A | B old, A newer | B's buffer falsely clean | R8 |
| Save and Quit with another dirty document | Only active file updated | Exit with other body unsaved | R12 |
| Save All then dirty a formerly clean document | Only queued file updated | Exit misses newly dirty body | R12 variant |
| Named symlink Save As | Resolved target receives bytes; link remains | Chosen buffer path and written fingerprint checked | Existing `save_as_onto_a_symlink_splits_chosen_and_resolved_correctly`, passing baseline |
| Controlled input-reader loss | User files not implicitly overwritten | One dump per dirty buffer, distinct unnamed dump names | Existing `recovery::tests::dump_all_dirty_*`, passing baseline |
| Real named-file kill/relaunch | Smoke fixture | Existing S8 checkpoint/recovery journey passes | `smoke: 9/9 PASS` |

The smoke suite's named-file recovery success does not cover in-session open,
unnamed re-recovery, multiple live owners, or the new queue interleavings. These
distinctions explain why the baseline remains green.

## Remediation order and review limits

1. Correct session-wide quit safety and destination-aware save completion (R12, R8).
2. Establish unique recovery ownership and safe discovery/transfer (R1, R4, R5).
3. Centralize checkpoint eligibility and stable deadlines across buffers and edit
   entry points (R2, R6, R9), with failure/modal behavior handled together (R3, R11).
4. Align recovery size limits (R10) and decide the external-write conflict policy (R7).

These are proposed directions, not approved patches. Keep the reproduced cases as
regression requirements; rewrite defect-asserting tests to require correct behavior
when implementing fixes. The existing dispatch-asymmetry tests and foreground-only
conflict design need explicit review rather than silent changes.

This pass screened session persistence, path resolution, scratch operations, buffer
construction/replacement, opening, saving, recovery, and quit/close boundaries.
It did not exhaustively test every OS/filesystem, physical power loss, hard-link or
symlink retarget races, non-UTF-8 path handling, corrupt recovery files, state-directory
provisioning failures, all plugin edit entry points, or every provider/overlay.
Those are coverage limits and follow-ups, not implicit passes. Resource/worker
shutdown and deeper protocol behavior remain assigned to later review passes.

No implementation changes, commits, or backlog edits were made. Pass 0's ledger
records scoped inspection rather than claiming every source module is reviewed.
