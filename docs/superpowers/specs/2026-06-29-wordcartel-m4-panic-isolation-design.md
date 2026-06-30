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

**A panic is treated as a failed completion.** For the ad-hoc async threads
(transform/filter/export), this is literal: the panic sends the path's existing error-`Msg`,
routing to the existing failure handler (which has the clock + clears the in-flight flag). For
executor jobs (Save/SwapWrite), the `Panicked` arm performs the kind's failure cleanup
**explicitly** — NOT by "reusing the failure path," because the real failed-save path is
**non-uniform** across `PostSaveAction` (e.g. the `Quit` variant leaves `pending_after_save`
armed on failure), so the panic cleanup is spelled out field-by-field below to guarantee a
panicked save never quits/strands. We do NOT catch our own command handlers (Q1): a panic in
our code is a bug to find and fix, not silently swallow.

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
- **`drain() -> Vec<JobOutcome>`** (was `Vec<JobResult>`). This ripples to **dozens** of
  call sites across production AND test code (far more than can be reliably hand-listed). The
  plan MUST mandate a **grep-based audit, not a sample list**: the implementer runs
  `rg '\.drain\(|apply_result|apply_job_result|is_stale'` across `wordcartel/src/` and updates
  EVERY hit before the type change compiles. (Per Codex, the real set includes — non-
  exhaustively — production `app.rs:776`/`1720` + the `apply_job_result` early-returns and
  `Msg::JobDone` handling scattered through `reduce` at app.rs:1039/1052/1056/1089/1102/1113/
  1188/1202/1211/1254/1266/1274/1346/1380/1399/1447/1473/1484/1509/1530/1562/1598/1673; tests
  in save.rs/swap.rs/file.rs/jobs.rs and app.rs's test module. Treat this list as a
  starting point, NOT the complete set — the grep is authoritative.) The cleanest way to
  contain the blast radius: keep `apply_result(JobResult, &mut Editor)` as-is and add a thin
  `apply_outcome(JobOutcome, &mut Editor)` that matches `Done` → existing `apply_result` /
  `Panicked` → the new per-kind cleanup, so most call sites change only the drained type, not
  their body. The plan should evaluate that wrapper to minimize churn.
- **`is_stale`** applies only to the `Done(JobResult)` arm (production caller `apply_result`
  takes a `JobResult`, app.rs:107-108 — confining it to `Done` is sufficient). A `Panicked`
  outcome is NOT subject to staleness-discard *before* cleanup: its per-kind cleanup MUST run
  (to clear in-flight state), then the outcome is done. Order: route `Panicked` → cleanup
  first, unconditionally.

**Apply funnel: route `Panicked` by kind (replicate each kind's FAILURE cleanup).** A
panicked job's editor state is untouched (the worker thread never mutates the `Editor`; only
the merge does, and it never ran), so cleanup is exactly the failure path:

| Kind | Panic cleanup (explicit — the failed-save path is NOT uniform across `PostSaveAction`, so spell it out) |
|---|---|
| `Save` | Keep the buffer **dirty** — do NOT set `saved_version`. A panicked save must NOT quit / lose data, so **explicitly reset the quit state** (the failed-save path leaves `pending_after_save` *armed* for the `Quit` variant — app.rs:129-131 — which we must NOT replicate): set `editor.pending_after_save = None` (editor.rs:297), and `editor.quit_drain = None` (editor.rs:355) + `editor.quit_drain_advance = false` (editor.rs:358) to abort any in-progress quit-drain. Status `"save failed (internal error)"`. (Armed by `dispatch_save_then`, save.rs:159/170.) |
| `SwapWrite` | **clear `swap_in_flight`** (the buffer-local field, editor.rs:105) for the buffer — mirroring the success merge's clear at `swap.rs:299`. Without it the Tick swap-cadence guard (app.rs:1688-1692) blocks all future swaps for that buffer. + status. |
| `CoalesceProbe` | test-only; cleanup is a no-op + status. |

### 3. Ad-hoc async threads: `catch` body + error-`Msg` (`transform.rs`, `filter.rs`, `export.rs`)

Each raw `thread::spawn` that runs untrusted work and reports via a completion `Msg` wraps
its body in `catch`; on panic it sends the **existing** completion `Msg` with an error
result, so the existing main-thread handler (which has the clock + clears in-flight) treats
it as a failed completion:

- **Async transform** (spawn at `transform.rs:112-118`): on panic send
  `Msg::TransformDone { …, result: Err(TransformError::Panicked(msg)) }` (`result` is
  `Result<String, TransformError>`, app.rs:48-53). The existing `apply_transform_done`
  clears `transform_in_flight` (editor.rs:305) + status, and already does so BEFORE the
  version-staleness check (app.rs:272-273) — so a stale/closed buffer still un-sticks the
  flag. ✓
- **Filter** (spawn at `filter.rs:346`): on panic send
  `Msg::FilterDone { …, outcome: RunResult::Err(FilterError::Panicked(msg)) }` (`RunResult`
  has an `Err(FilterError)` arm, filter.rs:92-96) → existing `apply_filter_done` clears
  `filter_in_flight` (editor.rs:304) before staleness (app.rs:230-231) + status. ✓
- **Export** (spawn at `export.rs:103`): on panic send
  `Msg::ExportDone { …, result: Err(FilterError::Panicked(msg)) }` (`result` is
  `Result<ExportResult, FilterError>`, app.rs:37-40; export already uses `FilterError` for
  failures, export.rs:115). The existing `apply_export_done` `Err` arm (app.rs:329-330)
  surfaces an error **status**. **Note:** export has NO in-flight flag (`pending_export`,
  editor.rs:307, is overwrite-confirmation, not a dispatch guard) — so there is nothing to
  un-stick; the panic simply surfaces an error status instead of silently killing the thread.

New error variants: `TransformError::Panicked(String)` and a **single shared
`FilterError::Panicked(String)`** (used by both the filter and export paths). Adding
`FilterError::Panicked` requires a new arm in `filter::describe_error` (and any other
exhaustive `FilterError` match) so the status renders, e.g. `"internal error: {msg}"`. To keep the
mapping testable without relying on `repar`/a subprocess actually panicking, factor each
thread body as `match catch(|| work()) { Ok(r) => send(done(r)), Err(msg) => send(err(msg)) }`
and unit-test the `Err(msg) → error-Msg` mapping with a closure that panics. All three closures
already capture the metadata the error-`Msg` needs (transform: buffer_id/version/range/kind,
transform.rs:105-117; filter: buffer_id/version/range/cursor/disposition, filter.rs:331-352;
export: buffer_id/target/overwrite_confirmed, export.rs:98-105).

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
  catch): the input thread (`app.rs:1935-1938`) and the clipboard worker
  (`clipboard.rs:63-85`) are both unguarded. These do NOT fit the "send an error completion
  Msg" pattern — they're long-lived readers, not request/response units, and a dead reader is
  a different failure mode (the app stops receiving input/clipboard = a hang, not a stuck
  in-flight flag). Deferred to a follow-up that needs its own approach (a supervisor that
  detects a dead reader and restarts/surfaces it). **Also fix** the inaccurate `term.rs:90-93`
  comment that claims input/clipboard reader panics are caught by each thread's own
  `catch_unwind` — neither thread has one.
- The **diagnostics warmup thread** (`app.rs:1891-1898`, fire-and-forget) is unguarded but
  deferred; diagnostics already wraps Harper panics (`diagnostics_run.rs:57-61`), and the wake
  relay (app.rs:1922-1926) does no untrusted work.
- **Diagnostics** already has a Harper-specific catch (`diagnostics_run.rs:57`); a broad
  diagnostics-thread catch is deferred unless trivial.
- **Our command handlers** stay un-caught (Q1-A).
- **Plugin call-sites** (Effort P) will reuse the `catch` helper + this failure-as-panic
  contract; M4-rest only builds the substrate.

## Testing strategy

- `catch` returns `Err(msg)` for `&str`, `String`, and other payloads (→ `"panic"`).
- **Executor (deterministic via `InlineExecutor`):** a panicking `Save` job →
  `JobOutcome::Panicked` → buffer stays dirty (`saved_version` unset), and
  `pending_after_save == None` + `quit_drain == None` + `quit_drain_advance == false` (the
  quit is aborted, not stranded), status set. A panicking `SwapWrite` → `swap_in_flight`
  cleared + status. The existing `worker_survives_panicking_job_and_runs_next_job` stays green
  (adapted to `JobOutcome`).
- **Sync transform** panic → `TransformError::Panicked` → status, buffer unchanged.
- **Ad-hoc threads:** the `catch(work) → error-Msg` mapping for transform/filter/export is
  unit-tested with a panicking work closure → asserts the error-`Msg` is produced. Where the
  handler has an in-flight flag (transform/filter), assert it's cleared; for export, assert an
  error status (no flag exists).
- `term.rs` comment corrected.

## New code surface (checklist for the plan)

- `wordcartel/src/panicx.rs` (new): `catch`, `panic_message`; `pub mod panicx;` in `lib.rs`.
- `wordcartel/src/jobs.rs`: `JobOutcome` enum; both executors `catch_unwind` + emit
  `Panicked` (destructure `Copy` metadata before consuming `job.run`); `drain() -> Vec<JobOutcome>`;
  `is_stale` confined to `Done`; adapt `is_stale` unit tests (jobs.rs:221/292/300).
- Drain/apply ripple — **grep is authoritative** (`rg '\.drain\(|apply_result|apply_job_result|is_stale'`);
  starting-point sites only: production `app.rs:776`/`1720` + the `apply_job_result`/`Msg::JobDone`
  sites scattered through `reduce`; the new `Panicked` routing-by-kind in the funnel (consider
  the `apply_outcome` wrapper); test drains in `save.rs`/`swap.rs`/`file.rs`/`jobs.rs`/app.rs tests.
- `wordcartel/src/transform.rs`: `TransformError::Panicked`; sync `catch` → that error; async
  thread (spawn ~112-118) `catch` + `TransformDone(Err)`.
- `wordcartel/src/filter.rs`: `FilterError::Panicked` (shared with export); thread (spawn 346)
  `catch` + `FilterDone(Err(FilterError::Panicked))`.
- `wordcartel/src/export.rs`: thread (spawn 103) `catch` + `ExportDone(Err(FilterError::Panicked))`
  — status only (no in-flight flag exists).
- Per-kind `Panicked` cleanup fields: `pending_after_save`/`quit_drain`/`quit_drain_advance`
  (Save), `swap_in_flight` (SwapWrite).
- `wordcartel/src/term.rs`: correct the panic-hook comment (input app.rs:1935-1938 + clipboard
  clipboard.rs:63-85 are NOT caught).
- Tests per the testing strategy.
