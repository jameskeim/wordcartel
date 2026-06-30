# M4-rest — Panic Isolation for Untrusted/Async Paths — Design

**Status:** Approved (brainstorm + Codex design review complete)
**Date:** 2026-06-29
**Parent:** Hardening campaign workstream **M4** (BUG-1 done; this completes the rest).
**Crate:** `wordcartel` shell.

## Goal

Isolate panics from untrusted/library code (the in-process `repar` transform, and —
foundationally — future plugin calls) and from async work, so a panic surfaces as a
recoverable **error + status** instead of crashing the editor or hanging async state. This
completes the isolation BUG-1 started: BUG-1 made the job worker *survive* a panic, but
left two holes — (a) a panicked job is caught yet **hangs silently** (no status, in-flight/
dirty never clears — `jobs.rs:115-119`, "surfacing the panic is deferred"), and (b) the
main-thread sync transform and the ad-hoc `thread::spawn` async paths (transform/filter/
export) are unguarded, so a panic there crashes the editor or strands `*_in_flight` forever.

## Core principle (from the Codex design review)

**A panic is handled exactly like a failed completion.** Every async path already has a
failure handler that clears its in-flight flag, keeps the buffer correctly dirty, aborts
quit-drain, and sets a status — with the clock it needs. So panic recovery reuses that
existing failure path rather than inventing parallel cleanup. We do NOT catch our own
command handlers (Q1): a panic in our code is a bug to find and fix, not silently swallow.

## Decisions

1. **Targeted untrusted-boundary isolation** (Q1-A). `catch_unwind` only around code that
   legitimately panics on input we don't control (the `repar` transform; future plugins) and
   around the ad-hoc worker threads. Command handlers stay loud.
2. **Unify the panic-recovery story** (Q2-B) — close BUG-1's deferred TODO so a panicked
   *executor* job surfaces, AND make the ad-hoc threads panic-safe.
3. **Per-thread `catch` for the ad-hoc async threads, NOT move-them-onto-the-Executor**
   (revised after Codex review). Moving the async transform onto the Executor fails the
   `JobResult::merge: FnOnce(&mut Editor)` contract because `merge_transform_into` needs a
   `Clock`; it also wouldn't cover filter/export. Instead, each ad-hoc thread wraps its body
   in `catch` and on panic sends its **existing completion `Msg` carrying an error result**,
   reusing the existing handler (which has the clock + clears in-flight).
4. **Generic panic signal routed by kind** for executor jobs (Q3-A): the result channel
   carries a `JobOutcome` enum; a `Panicked` outcome is routed by `kind` to replicate that
   kind's failure cleanup.

## Components

### 1. Shared `catch` helper (`wordcartel/src/panicx.rs`, new)

```rust
//! The untrusted/library/worker panic boundary (M4). Reused by the sync transform, the
//! ad-hoc worker threads, the job Executor, and (later) plugin call-sites.

/// Run `f`, catching a panic and returning a best-effort message instead of unwinding.
pub(crate) fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(panic_message)
}

/// Extract a human-readable string from a panic payload (best-effort).
pub(crate) fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}
```

### 2. Executor: surface panicked jobs (`jobs.rs`) — close BUG-1's deferred TODO

The result channel carries a `JobOutcome` instead of a bare `JobResult`:
```rust
pub enum JobOutcome {
    Done(JobResult),
    Panicked { buffer_id: BufferId, class: ResultClass, version: u64, kind: JobKind, msg: String },
}
```
- **Worker (`ThreadExecutor`):** destructure the `Copy` metadata (`buffer_id`, `class`,
  `version`, `kind`) from `job` into locals **before** the `AssertUnwindSafe(|| (job.run)())`
  consumes `job.run` (do not rely implicitly on edition-2021 disjoint capture — Codex). On
  `Ok(r)` send `JobOutcome::Done(r)`; on `Err(payload)` send
  `JobOutcome::Panicked { …, msg: panic_message(payload) }`, then the wake nudge either way.
- **`InlineExecutor`:** likewise `catch_unwind` (currently bare `(job.run)()`, `jobs.rs:84`)
  so the panic path is deterministically testable. No existing test relies on a panic
  propagating out of inline dispatch (the survival test targets `ThreadExecutor`).
- **`drain() -> Vec<JobOutcome>`** (was `Vec<JobResult>`). Ripples to every drain/apply site
  — enumerate and update: `app.rs:776`, `app.rs:1720`, `save.rs:313`, `swap.rs:567`,
  `file.rs:292`.
- **`is_stale`** applies only to the `Done(JobResult)` arm. A `Panicked` outcome is NOT
  subject to the staleness-discard *before* cleanup: its per-kind cleanup MUST run (to clear
  in-flight state), then the outcome is done. Order: route `Panicked` → cleanup first.

**Apply funnel: route `Panicked` by kind (replicate each kind's FAILURE cleanup).** A
panicked job's editor state is untouched (the worker thread never mutates the `Editor`; only
the merge does, and it never ran), so cleanup is exactly the failure path:

| Kind | Panic cleanup (= its failure path) |
|---|---|
| `Save` | keep buffer **dirty** (don't set `saved_version`), status `"save failed (internal error)"`, **and abort `pending_after_save` / quit-drain** exactly as the failed-save path (`app.rs:119`) so save-and-quit / quit-after-drain are not stranded. |
| `SwapWrite` | **clear `swap_in_flight`** for the buffer (`swap.rs:295`) + status. (Status-only would hang the next swap.) |
| `CoalesceProbe` | test-only; cleanup is a no-op + status. |

### 3. Ad-hoc async threads: `catch` body + error-`Msg` (`transform.rs`, `filter.rs`, `export.rs`)

Each raw `thread::spawn` that runs untrusted work and reports via a completion `Msg` wraps
its body in `catch`; on panic it sends the **existing** completion `Msg` with an error
result, so the existing main-thread handler (which has the clock + clears in-flight) treats
it as a failed completion:

- **Async transform** (`transform.rs:112-118`): on panic send
  `Msg::TransformDone { …, result: Err(TransformError::Panicked(msg)) }`. The existing
  `apply_transform_done` handles `Err` → clears `transform_in_flight` + status. **Confirm**
  `transform_in_flight` is cleared BEFORE the version-staleness check (`app.rs:263`), so a
  stale/closed buffer still un-sticks the flag.
- **Filter** (`filter.rs:322`): on panic send
  `Msg::FilterDone { …, outcome: RunResult::Err(FilterError::Panicked(msg)) }` → existing
  `apply_filter_done` clears `filter_in_flight` + status. (Same in-flight-before-staleness
  rule.)
- **Export** (`export.rs:90`): on panic send `Msg::ExportDone { …, result: Err(<panic>) }`
  → existing handler clears the export in-flight state + status.

New error variants: `TransformError::Panicked(String)`, `FilterError::Panicked(String)`, and
the export error's equivalent (reuse an existing "internal/io" shape if cleaner). To keep the
mapping testable without relying on `repar`/a subprocess actually panicking, factor each
thread body as `match catch(|| work()) { Ok(r) => send(done(r)), Err(msg) => send(err(msg)) }`
and unit-test the `Err(msg) → error-Msg` mapping with a closure that panics.

### 4. Sync transform (main thread) (`transform.rs:122-124`)

Wrap the small-region synchronous `run_transform` in `catch` → on panic produce
`TransformError::Panicked(msg)`, flowing through the existing `apply_transform_result` error
path (status + buffer unchanged — the result is applied only *after* `run_transform`
returns, so the editor is untouched on panic). The sync branch stays synchronous (no async
round-trip for tiny transforms — responsiveness preserved).

## Error handling / recovery contract

No caught panic crashes the editor or corrupts the document. Worker/ad-hoc-thread `run`
bodies do no editor mutation, so a panicked async unit leaves the `Editor` untouched — only
in-flight/dirty/quit-drain/status need resetting, which is exactly each path's existing
failure handling. Our own (non-job, non-library) command-handler panics remain **un-caught
and loud** so they get fixed.

## Out of scope / noted (with rationale)

- **Input and clipboard reader threads** (raw `thread::spawn`, not governed by the executor
  catch). These do NOT fit the "send an error completion Msg" pattern — they're long-lived
  readers, not request/response units, and a dead reader is a different failure mode (the app
  stops receiving input/clipboard = a hang, not a stuck in-flight flag). Deferred to a
  follow-up that needs its own approach (e.g. a supervisor that detects a dead reader and
  restarts it or surfaces it). **Also fix** the inaccurate `term.rs:90` comment that claims
  input/clipboard panics are caught — they are not.
- **Diagnostics** already has a Harper-specific catch (`diagnostics_run.rs:57`); a broad
  diagnostics-thread catch is deferred unless trivial.
- **Our command handlers** stay un-caught (Q1-A).
- **Plugin call-sites** (Effort P) will reuse the `catch` helper + this failure-as-panic
  contract; M4-rest only builds the substrate.

## Testing strategy

- `catch` returns `Err(msg)` for `&str`, `String`, and other payloads (→ `"panic"`).
- **Executor (deterministic via `InlineExecutor`):** a panicking `Save` job →
  `JobOutcome::Panicked` → buffer stays dirty, `saved_version` unset, `pending_after_save`/
  quit-drain aborted, status set. A panicking `SwapWrite` → `swap_in_flight` cleared +
  status. The existing `worker_survives_panicking_job_and_runs_next_job` stays green (adapted
  to `JobOutcome`).
- **Sync transform** panic → `TransformError::Panicked` → status, buffer unchanged.
- **Ad-hoc threads:** the `catch(work) → error-Msg` mapping for transform/filter/export is
  unit-tested with a panicking work closure → asserts the error-`Msg` is produced (and, where
  feasible, that handling it clears the in-flight flag).
- `term.rs` comment corrected.

## New code surface (checklist for the plan)

- `wordcartel/src/panicx.rs` (new): `catch`, `panic_message`; `pub mod panicx;` in `lib.rs`.
- `wordcartel/src/jobs.rs`: `JobOutcome` enum; both executors `catch_unwind` + emit
  `Panicked`; `drain() -> Vec<JobOutcome>`; `is_stale` confined to `Done`.
- Drain/apply ripple: `app.rs` (the funnel — new `Panicked` routing by kind),
  `save.rs:313`, `swap.rs:567`, `file.rs:292`, `app.rs:776`/`1720`.
- `wordcartel/src/transform.rs`: `TransformError::Panicked`; sync `catch`; async thread
  `catch` + `TransformDone(Err)`.
- `wordcartel/src/filter.rs`: `FilterError::Panicked`; thread `catch` + `FilterDone(Err)`.
- `wordcartel/src/export.rs`: export error panic variant; thread `catch` + `ExportDone(Err)`.
- `wordcartel/src/term.rs`: correct the panic-hook comment.
- Tests per the testing strategy.
