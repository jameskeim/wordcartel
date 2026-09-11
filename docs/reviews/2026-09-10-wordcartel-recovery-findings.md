# Wordcartel recovery review — 2026-09-10

Reviewed revision: `d3aea924988f60702529f6c68a0e332e64d44f1d` (`main`).

This was a focused correctness review of saving, recovery, background jobs, and
related editing and plugin boundaries, not an exhaustive audit. Three recovery
defects were reproduced. No implementation fixes were made. These notes record
review evidence and proposed directions; they are not an approved implementation
plan or a replacement for the project's backlog tracker.

## 1. P1 — Untitled buffers share a recovery file

**Source:** [`swap::swap_path`](../../wordcartel/src/swap.rs), especially the
`None => format!("scratch-{}.swp", std::process::id())` branch;
`swap::dispatch_swap_write`; [`workspace::new_empty_buffer`](../../wordcartel/src/workspace.rs).

Every unnamed buffer in a process resolves to the same swap filename. The
workspace permits multiple unnamed documents, but swap naming does not include
their identities.

**Reproduction:**

1. Create an unnamed document A with unsaved content and checkpoint it.
2. Create an unnamed document B with different content and checkpoint it.
3. Read the shared swap: it contains B's content.
4. Inspect A: its `swapped_version` still says its current version is checkpointed.

**Impact:** A's recovery content has been overwritten, and its checkpoint latch
suppresses rewriting it until another edit or state change. A crash or power loss
can lose A's unsaved work. Source inspection also shows that Save As cleanup uses
the old unnamed path, so saving one unnamed buffer can delete the shared recovery
file used by another; that cleanup scenario was not separately exercised by a probe.

**Proposed direction:** give each live document/session a unique recovery key and
carry it consistently through checkpoint writes, cleanup, Save As, and orphan
discovery. Audit multiple buffers opening the same named path and multiple running
sessions too: the current named-path key also lacks buffer/session identity.

**Regression coverage to add:** two unnamed documents retain distinct recoverable
bodies; saving or closing one cannot remove the other's checkpoint; recovery can
discover all orphan unnamed documents.

## 2. P1 — Inactive dirty buffers stop receiving recovery checkpoints

**Source:** [`timers::swap_deadline` and `timers::on_tick`](../../wordcartel/src/timers.rs);
[`Editor::switch_to_index`](../../wordcartel/src/editor.rs).

Both deadline calculation and checkpoint dispatch inspect only `editor.active()`.
Switching buffers does not checkpoint the buffer being left.

**Reproduction:**

1. Edit document A.
2. Switch to a clean document B before A's two-second idle debounce expires.
3. Advance the clock beyond the nominal 30-second maximum interval.
4. The swap subsystem reports no deadline, and A remains uncheckpointed.

**Impact:** A's edits can remain without recovery protection indefinitely while
the user works elsewhere. A crash or power loss can lose those edits. If A had an
older checkpoint, recovery can still omit its newer edits.

**Proposed direction:** calculate the earliest pending checkpoint across all
buffers, then dispatch due work by buffer identity without changing UI focus.
Keep in-flight and version bookkeeping per buffer.

**Regression coverage to add:** edit A, immediately switch to B, and verify A is
checkpointed within the intended cadence while B stays active. Include multiple
pending buffers and edits to a non-active buffer.

## 3. P2 — Failed checkpoint writes immediately retry

**Source:** [`swap::dispatch_swap_write`](../../wordcartel/src/swap.rs), its completion
closure, and [`timers::swap_deadline`](../../wordcartel/src/timers.rs).

An unsuccessful write clears `swap_in_flight` but does not advance a retry deadline
or otherwise delay the next attempt. The original edit deadline remains overdue,
so the next completion/wake cycle can immediately dispatch another write.

**Reproduction:**

1. Make a dirty document's checkpoint due.
2. Inject a filesystem create failure using `FaultFs::new(FaultAt::Create)`.
3. Dispatch and apply the failed result.
4. At the same clock time, the deadline is still immediately due.
5. Repeat: each tick dispatches another failing write and reports `swap write failed`.

The probe confirmed three successive attempts without advancing the clock.

**Impact:** persistent disk-full or permission errors cause repeated filesystem
operations, worker wakeups, and error reporting while the editor is otherwise idle.
Recovery remains unavailable throughout the failure.

**Proposed direction:** track failed attempts separately from successful checkpoint
versions and use bounded retry backoff. Keep unsaved work pending, retain a visible
error, and allow recovery once the filesystem becomes writable.

**Regression coverage to add:** repeated failures do not schedule immediate retries;
later retries occur on the chosen schedule; success after a failure checkpoints the
latest content and clears retry state.

## Validation

Existing library suites were run before adding reproduction probes:

```text
cargo test --workspace --lib --quiet
wordcartel:      2005 passed; 0 failed; 1 ignored
wordcartel-core:  308 passed; 0 failed
wordcartel-nlp:    48 passed; 0 failed
Total:          2361 passed; 0 failed; 1 ignored
```

Three temporary unit probes were then run with:

```text
cargo test -p wordcartel --lib review_probes -- --test-threads=1
swap::review_probes::untitled_checkpoints_collide                 ... ok
swap::review_probes::inactive_dirty_buffer_has_no_checkpoint_deadline ... ok
swap::review_probes::failed_checkpoint_immediately_rearms         ... ok
```

These probes asserted the defective behavior, so their passing confirms the
reproductions; it does not mean the defects are fixed. Temporary source additions
were removed, and the tracked source diff was empty after the review. Integration
suites, clippy, and PTY smoke tests were not run for this focused review.

## Architectural observations

Useful foundations include the separate editing core, centralized transaction
validation, atomic filesystem writes, explicit background-result routing, and a
fault-injectable filesystem seam. The findings expose gaps in the interaction
between multi-buffer state and recovery scheduling/identity, plus failure-path
retry behavior. Existing passing tests did not cover those combinations.
