# M4-rest — Panic Isolation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Isolate panics from untrusted/library code (the in-process `repar` transform) and async work so a panic surfaces as a recoverable error + status instead of crashing the editor or hanging `*_in_flight` state — completing the M4 panic-isolation BUG-1 started.

**Architecture:** A shared `catch` helper is the panic boundary. Executor jobs gain a `JobOutcome { Done | Panicked }`; a `Panicked` outcome is routed by kind to that kind's explicit failure cleanup. The ad-hoc async threads (transform/filter/export) wrap their untrusted work in `catch` and on panic send their existing completion `Msg` with an error result, reusing the existing failure handler. Our own command handlers stay un-caught (a panic there is a bug to fix).

**Tech Stack:** Rust (`wordcartel` shell). `std::panic::catch_unwind`. No new deps.

**Spec:** `docs/superpowers/specs/2026-06-29-wordcartel-m4-panic-isolation-design.md` (Codex-reviewed: design pass + 3 spec rounds, GO).

## Global Constraints

- **A panic is treated as a failed completion.** Ad-hoc threads route their panic to the existing error-`Msg` handler; executor jobs perform explicit per-kind cleanup (the failed-save path is NON-uniform across `PostSaveAction`, so it is spelled out, not "reused").
- **Targeted isolation:** `catch` only untrusted/library/worker code — NOT our own command handlers.
- **Gates:** `cargo test -p wordcartel -p wordcartel-core` green; `cargo build` + `cargo test --no-run` warning-free for touched crates; **no new clippy findings on touched lines** (`cargo clippy -p wordcartel --tests`, check your diff — do NOT chase a clean whole-workspace `-D warnings`; pre-existing debt is out of scope). **Do NOT run `cargo fmt`** (hand-formatted dense house style, no rustfmt.toml; match neighbors — `—` em-dashes never `--`, no emoji, single-line blocks where they read well).
- Commit trailers (append to every commit message):
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
  `Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6`

## File Structure

- **Create** `wordcartel/src/panicx.rs` — `catch` + `panic_message` (the one panic boundary).
- **Modify** `wordcartel/src/lib.rs` — `pub mod panicx;`.
- **Modify** `wordcartel/src/jobs.rs` — `JobOutcome` enum; both executors catch; `drain() -> Vec<JobOutcome>`; `is_stale` confined to `Done`.
- **Modify** `wordcartel/src/app.rs` — `apply_outcome` wrapper + `apply_panic` per-kind cleanup; thread the `JobOutcome` type through every drain site (grep-audited).
- **Modify** `wordcartel/src/{transform,filter,export}.rs` — per-path `guarded_*` + new `Panicked` error variants.
- **Modify** `wordcartel/src/term.rs` — correct the stale panic-hook comment.

---

### Task 1: `panicx` — the shared `catch` helper (+ term.rs comment fix)

**Files:**
- Create: `wordcartel/src/panicx.rs`
- Modify: `wordcartel/src/lib.rs` (`pub mod panicx;`), `wordcartel/src/term.rs` (comment)
- Test: `wordcartel/src/panicx.rs` tests

**Interfaces:**
- Produces: `pub(crate) fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String>`; `pub(crate) fn panic_message(p: Box<dyn std::any::Any + Send>) -> String`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_returns_ok_for_non_panicking() {
        assert_eq!(catch(|| 1 + 1).unwrap(), 2);
    }

    #[test]
    fn catch_maps_str_panic_to_message() {
        let e = catch(|| panic!("boom")).unwrap_err();
        assert_eq!(e, "boom");
    }

    #[test]
    fn catch_maps_string_panic_to_message() {
        let e = catch(|| panic!("{}", String::from("dynamic"))).unwrap_err();
        assert_eq!(e, "dynamic");
    }

    #[test]
    fn catch_maps_other_payload_to_default() {
        let e = catch(|| std::panic::panic_any(42u32)).unwrap_err();
        assert_eq!(e, "panic");
    }
}
```
(The default panic hook prints "panicked at …" to stderr even when caught — that's expected noise, not a test failure.)

- [ ] **Step 2: Implement `panicx.rs`**

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

- [ ] **Step 3: Wire the module**

`wordcartel/src/lib.rs` — add near the other infra modules (house ordering; do NOT re-sort):
```rust
pub mod panicx;
```

- [ ] **Step 4: Correct the stale term.rs comment**

In `wordcartel/src/term.rs` (~lines 90-93), the doc comment wrongly claims the clipboard/input
reader threads catch their own panics. Replace the inaccurate clause:
```rust
/// the dump + terminal restore.  A non-main-thread panic in the job WORKER is caught by the
/// executor (it surfaces the panic as a failed job, M4); the hook must not touch the terminal
/// off the main thread or it corrupts the live UI.  NOTE: the clipboard helper and input reader
/// threads are NOT yet guarded — a panic there is a separate (deferred) failure mode.
```
(Match the surrounding comment style; keep the "hook must not touch the terminal off-main" point.)

- [ ] **Step 5: Run tests + gates + commit**

`cargo test -p wordcartel --lib panicx` green; `cargo build -p wordcartel 2>&1 | grep -i warning` empty; `cargo clippy -p wordcartel --tests 2>&1 | grep -A3 "panicx.rs"` no findings on new lines.
```bash
git add wordcartel/src/panicx.rs wordcartel/src/lib.rs wordcartel/src/term.rs
git commit -m "feat(m4): panicx catch helper + correct stale panic-hook comment"
```

---

### Task 2: `JobOutcome` + executors catch + drain ripple + per-kind panic cleanup

**Files:**
- Modify: `wordcartel/src/jobs.rs` (`JobOutcome`, both executors, `drain`, `is_stale`, tests), `wordcartel/src/app.rs` (`apply_outcome` + `apply_panic`; thread the type through all drain sites)
- Test: `jobs.rs` + `app.rs`/`save.rs`/`swap.rs` tests

**Interfaces:**
- Consumes: `crate::panicx::{catch, panic_message}` (Task 1).
- Produces: `pub enum JobOutcome { Done(JobResult), Panicked { buffer_id, class, version, kind, msg } }`; `drain() -> Vec<JobOutcome>`; `pub fn apply_outcome(outcome: JobOutcome, editor: &mut Editor)`; `apply_job_outcome(outcome, editor, ex, clock, msg_tx)` (the funnel variant).

- [ ] **Step 1: Add `JobOutcome` + adapt both executors (failing test first)**

Failing test (jobs.rs tests): a panicking job yields a `Panicked` outcome from the inline executor.
```rust
#[test]
fn inline_executor_emits_panicked_outcome() {
    let ex = InlineExecutor::default();
    ex.dispatch(Job {
        buffer_id: BufferId(1), class: ResultClass::BufferLocal, version: 1, kind: JobKind::Save,
        run: Box::new(|| panic!("boom")),
    });
    let out = ex.drain();
    assert_eq!(out.len(), 1);
    assert!(matches!(&out[0], JobOutcome::Panicked { kind: JobKind::Save, msg, .. } if msg == "boom"));
}
```
Implement:
```rust
pub enum JobOutcome {
    Done(JobResult),
    Panicked { buffer_id: crate::editor::BufferId, class: ResultClass, version: u64, kind: JobKind, msg: String },
}
```
`InlineExecutor::dispatch` (jobs.rs:83) — catch and buffer a `JobOutcome`:
```rust
fn dispatch(&self, job: Job) {
    let (buffer_id, class, version, kind) = (job.buffer_id, job.class, job.version, job.kind);
    let outcome = match crate::panicx::catch(job.run) {
        Ok(result) => JobOutcome::Done(result),
        Err(msg) => JobOutcome::Panicked { buffer_id, class, version, kind, msg },
    };
    self.pending.borrow_mut().push(outcome);
}
```
(`pending: RefCell<Vec<JobOutcome>>`; `drain() -> Vec<JobOutcome>`.)

`ThreadExecutor` worker loop (jobs.rs:107-122) — destructure metadata BEFORE consuming `job.run`:
```rust
while let Ok(job) = job_rx.recv() {
    let (buffer_id, class, version, kind) = (job.buffer_id, job.class, job.version, job.kind);
    let outcome = match crate::panicx::catch(job.run) {
        Ok(result) => JobOutcome::Done(result),
        Err(msg) => JobOutcome::Panicked { buffer_id, class, version, kind, msg },
    };
    if result_tx.send(outcome).is_err() { break; }
    let _ = wake.send(());
}
```
`result_tx`/`result_rx` become `mpsc::channel::<JobOutcome>()`; `ThreadExecutor::drain` returns `Vec<JobOutcome>`. The `Executor` trait `drain` signature → `Vec<JobOutcome>`.

- [ ] **Step 2: Confine `is_stale` to `Done`; keep it `JobResult`-typed**

`is_stale(&JobResult, &Editor)` is unchanged in signature — it is only meaningful for a `Done`
result. The new `apply_outcome` calls it only in the `Done` arm. Adapt the `is_stale` unit
tests (`jobs.rs` ~221/292/300) that construct `JobResult` directly — they keep using
`JobResult` (not wrapped), since they test `is_stale` directly.

- [ ] **Step 3: `apply_outcome` + `apply_panic` (failing tests first)**

Failing tests (in `save.rs`/`swap.rs` test modules, mirroring the existing harness):
```rust
// save.rs tests
#[test]
fn panicked_save_keeps_dirty_and_aborts_quit() {
    let p = scratch(); std::fs::write(&p, "old\n").unwrap();
    let mut e = Editor::new_from_text("v1\n", Some(p.clone()), (80, 24));
    e.active_mut().document.saved_version = None; e.active_mut().document.version = 1;
    let id = e.active().id;
    // Arm a save-then-quit, then deliver a Panicked Save outcome for it.
    e.pending_after_save = Some(crate::editor::PendingAfterSave {
        buffer_id: id, version: 1, action: crate::editor::PostSaveAction::Quit });
    crate::app::apply_outcome(
        crate::jobs::JobOutcome::Panicked {
            buffer_id: id, class: crate::jobs::ResultClass::Durability, version: 1,
            kind: crate::jobs::JobKind::Save, msg: "boom".into() },
        &mut e);
    assert!(e.active().document.dirty(), "panicked save keeps the buffer dirty");
    assert!(e.pending_after_save.is_none(), "awaited quit must be cleared");
    assert!(!e.quit, "must NOT quit on a panicked save");
    assert!(e.status.to_lowercase().contains("save"));
    let _ = std::fs::remove_file(&p);
}
```
```rust
// swap.rs tests
#[test]
fn panicked_swap_clears_in_flight() {
    let p = scratch(); std::fs::write(&p, "x\n").unwrap();
    let mut e = Editor::new_from_text("x\n", Some(p.clone()), (80, 24));
    let id = e.active().id;
    e.active_mut().swap_in_flight = true;
    crate::app::apply_outcome(
        crate::jobs::JobOutcome::Panicked {
            buffer_id: id, class: crate::jobs::ResultClass::BufferLocal, version: 1,
            kind: crate::jobs::JobKind::SwapWrite, msg: "boom".into() },
        &mut e);
    assert!(!e.active().swap_in_flight, "panicked swap must clear swap_in_flight");
    let _ = std::fs::remove_file(&p);
}
```
Implement in `app.rs`:
```rust
/// Apply a job outcome: a normal Done routes to the existing apply_result; a Panicked outcome
/// runs that kind's explicit failure cleanup (a panic is a failed completion).
pub fn apply_outcome(outcome: crate::jobs::JobOutcome, editor: &mut Editor) {
    match outcome {
        crate::jobs::JobOutcome::Done(r) => apply_result(r, editor),
        crate::jobs::JobOutcome::Panicked { buffer_id, version, kind, msg, .. } =>
            apply_panic(buffer_id, version, kind, &msg, editor),
    }
}

fn apply_panic(buffer_id: crate::editor::BufferId, version: u64, kind: crate::jobs::JobKind, msg: &str, editor: &mut Editor) {
    use crate::jobs::JobKind;
    match kind {
        JobKind::Save => {
            // The merge never ran, so saved_version is untouched (buffer stays dirty). A panicked
            // save must NOT quit/strand: clear any awaited-quit state explicitly (the failed-save
            // Quit path leaves pending_after_save armed — we must not).
            let awaited = editor.pending_after_save.as_ref()
                .map(|p| p.buffer_id == buffer_id && p.version == version).unwrap_or(false);
            if awaited {
                editor.pending_after_save = None;
                editor.quit_drain = None;
                editor.quit_drain_advance = false;
            }
            editor.status = format!("save failed (internal error: {msg})");
        }
        JobKind::SwapWrite => {
            if let Some(b) = editor.by_id_mut(buffer_id) { b.swap_in_flight = false; }
            editor.status = format!("swap failed (internal error: {msg})");
        }
        #[cfg(test)]
        JobKind::CoalesceProbe => { editor.status = format!("job failed (internal error: {msg})"); }
    }
}
```
And the funnel variant (mirrors `apply_job_result`):
```rust
pub fn apply_job_outcome(outcome: crate::jobs::JobOutcome, editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock, msg_tx: &std::sync::mpsc::Sender<Msg>) {
    apply_outcome(outcome, editor);
    if editor.quit_drain_advance {
        editor.quit_drain_advance = false;
        drive_quit_drain(editor, ex, clock, msg_tx);
    }
}
```

- [ ] **Step 4: Grep-audit + thread the `JobOutcome` type through EVERY drain/apply site**

Run `rg '\.drain\(|apply_result|apply_job_result|is_stale' wordcartel/src/` and update EVERY hit
(production AND tests). The list is large and scattered — the grep is authoritative. Mechanically:
- Production `reduce`/loop sites that did `for r in ex.drain() { apply_job_result(r, …) }` →
  `for o in ex.drain() { apply_job_outcome(o, …) }`. Sites that called `apply_result(r, editor)`
  on a drained item → `apply_outcome(o, editor)`.
- Tests that did `for r in ex.drain() { crate::app::apply_result(r, &mut e) }` →
  `… apply_outcome(o, &mut e)`. Tests that construct a `JobResult` and pass it to `apply_result`
  directly → wrap as `JobOutcome::Done(r)` and call `apply_outcome`, OR keep calling
  `apply_result` directly (still public, still `JobResult`-typed) — whichever is the smaller diff.
- `is_stale` direct-call tests keep `JobResult`.

After the edits, `cargo build -p wordcartel` must compile (the type change forces every site).

- [ ] **Step 5: Adapt the worker-survival test**

`worker_survives_panicking_job_and_runs_next_job` (jobs.rs ~242): the first job panics; assert it
now yields a `JobOutcome::Panicked` AND the second (normal) job yields `JobOutcome::Done`. Keep
the "worker survives" guarantee.

- [ ] **Step 6: Run + gates + commit**

`cargo test -p wordcartel -p wordcartel-core` green; warning-free build/test-compile; clippy clean on touched lines.
```bash
git add wordcartel/src/jobs.rs wordcartel/src/app.rs wordcartel/src/save.rs wordcartel/src/swap.rs
git commit -m "feat(m4): JobOutcome + executor panic-surfacing + per-kind panic cleanup"
```

---

### Task 3: Transform panic isolation (sync + async)

**Files:**
- Modify: `wordcartel/src/transform.rs` (`TransformError::Panicked`; `guarded_transform`; sync + async paths)
- Test: `transform.rs` tests

**Interfaces:**
- Consumes: `crate::panicx::catch`.
- Produces: `TransformError::Panicked(String)`.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn guarded_transform_maps_panic_to_error() {
    let r = guarded_transform(|| panic!("kaboom"));
    assert!(matches!(r, Err(TransformError::Panicked(ref m)) if m == "kaboom"));
    let ok = guarded_transform(|| Ok("hi".to_string()));
    assert_eq!(ok.unwrap(), "hi");
}
```

- [ ] **Step 2: Add the variant + the guard helper**

`TransformError` (transform.rs:29): add `Panicked(String)`:
```rust
pub enum TransformError { Repar(String), OutputTooLarge { limit: usize }, Panicked(String) }
```
Extend the `Display` impl with `TransformError::Panicked(m) => write!(f, "transform failed (internal error: {m})"),`.
Add the testable guard (takes the work closure):
```rust
/// Run a transform body, mapping a panic in untrusted (`repar`) code to a recoverable error.
fn guarded_transform(work: impl FnOnce() -> Result<String, TransformError>) -> Result<String, TransformError> {
    match crate::panicx::catch(work) {
        Ok(r) => r,
        Err(msg) => Err(TransformError::Panicked(msg)),
    }
}
```

- [ ] **Step 3: Route both transform paths through it**

- Async (transform.rs ~113-114):
```rust
let input = snapshot.byte_slice(range_c.clone()).to_string();
let result = guarded_transform(|| run_transform(kind, &input, DEFAULT_REFLOW_WIDTH));
let _ = msg_tx.send(crate::app::Msg::TransformDone { buffer_id, version, range: range_c, kind, result });
```
- Sync (transform.rs ~122-124):
```rust
let input = editor.active().document.buffer.slice(range.clone()).to_string();
let result = guarded_transform(|| run_transform(kind, &input, DEFAULT_REFLOW_WIDTH));
apply_transform_result(editor, kind, range, result, clock);
```
The existing `apply_transform_done`/`apply_transform_result` already handle `Err` → clear
`transform_in_flight` + status, so `Panicked` flows through unchanged.

- [ ] **Step 4: Run + gates + commit**

`cargo test -p wordcartel --lib transform` green. Then:
```bash
git add wordcartel/src/transform.rs
git commit -m "feat(m4): isolate transform panics (sync + async) via guarded_transform"
```

---

### Task 4: Filter + export panic isolation

**Files:**
- Modify: `wordcartel/src/filter.rs` (`FilterError::Panicked`; `describe_error` arm; `guarded_filter`; thread), `wordcartel/src/export.rs` (`guarded_export`; thread)
- Test: `filter.rs` + `export.rs` tests

**Interfaces:**
- Consumes: `crate::panicx::catch`.
- Produces: `FilterError::Panicked(String)` (shared by filter + export).

- [ ] **Step 1: Failing tests**

```rust
// filter.rs tests
#[test]
fn guarded_filter_maps_panic_to_runresult_err() {
    let r = guarded_filter(|| panic!("flt"));
    assert!(matches!(r, RunResult::Err(FilterError::Panicked(ref m)) if m == "flt"));
}
#[test]
fn describe_error_renders_panicked() {
    assert!(describe_error(&FilterError::Panicked("x".into())).to_lowercase().contains("internal"));
}
```
```rust
// export.rs tests
#[test]
fn guarded_export_maps_panic_to_err() {
    let r = guarded_export(|| panic!("exp"));
    assert!(matches!(r, Err(crate::filter::FilterError::Panicked(ref m)) if m == "exp"));
}
```

- [ ] **Step 2: Add `FilterError::Panicked` + `describe_error` arm**

`FilterError` (filter.rs:40): add `Panicked(String)`. In `describe_error` (filter.rs:310) add:
```rust
FilterError::Panicked(m) => format!("internal error: {m}"),
```

- [ ] **Step 3: `guarded_filter` + route the filter thread**

```rust
fn guarded_filter(work: impl FnOnce() -> RunResult) -> RunResult {
    match crate::panicx::catch(work) {
        Ok(o) => o,
        Err(msg) => RunResult::Err(FilterError::Panicked(msg)),
    }
}
```
Filter thread (filter.rs ~350-351):
```rust
let outcome = guarded_filter(|| run_filter(&spec, stdin, &cancel));
let _ = msg_tx.send(crate::app::Msg::FilterDone { buffer_id, version, range: range_c, cursor, disposition, outcome });
```
The existing `apply_filter_done` handles `RunResult::Err` → clears `filter_in_flight` + status.

- [ ] **Step 4: `guarded_export` + route the export thread**

In `export.rs`:
```rust
fn guarded_export(work: impl FnOnce() -> Result<ExportResult, crate::filter::FilterError>)
    -> Result<ExportResult, crate::filter::FilterError> {
    match crate::panicx::catch(work) {
        Ok(r) => r,
        Err(msg) => Err(crate::filter::FilterError::Panicked(msg)),
    }
}
```
Export thread (export.rs ~104-105):
```rust
let result = guarded_export(|| run_pandoc(sink, &stdin, &target));
let _ = msg_tx.send(crate::app::Msg::ExportDone { buffer_id, target, result, overwrite_confirmed });
```
The existing `apply_export_done` `Err` arm surfaces an error status (export has NO in-flight flag
— nothing to un-stick; the win is not silently killing the thread).

- [ ] **Step 5: Run + gates + commit**

`cargo test -p wordcartel --lib filter export` green; full `cargo test -p wordcartel -p wordcartel-core` green.
```bash
git add wordcartel/src/filter.rs wordcartel/src/export.rs
git commit -m "feat(m4): isolate filter + export panics via shared FilterError::Panicked"
```

---

## Self-Review

**Spec coverage:** `catch` helper (Task 1) ✔; term.rs comment fix (Task 1) ✔; `JobOutcome` + both executors catch + drain ripple (grep-audited) + `apply_outcome`/`apply_panic` per-kind cleanup (Task 2) ✔; transform sync+async (Task 3) ✔; filter + export with shared `FilterError::Panicked` + `describe_error` arm (Task 4) ✔. Input/clipboard reader threads explicitly deferred (spec out-of-scope) ✔.

**Type consistency:** `JobOutcome { Done | Panicked }` and `drain() -> Vec<JobOutcome>` are consistent across Task 2; `apply_outcome`/`apply_job_outcome` referenced consistently; `TransformError::Panicked(String)` / `FilterError::Panicked(String)` defined before use; the per-kind cleanup names match real fields (`pending_after_save`, `quit_drain`, `quit_drain_advance`, `swap_in_flight`).

**Placeholder scan:** the grep-audit (Task 2 Step 4) is a deliberate "run this and update every hit" instruction (the site set is too large + scattered to hand-list, per Codex); every other step has concrete code. The exact line anchors in transform/filter/export (~113, ~350, ~104) may have shifted by a few lines — the implementer locates the real spawn/send.

**Ordering:** Task 1 (`catch`) first — everything imports it. Tasks 2/3/4 each depend on Task 1 and are independent of each other; each leaves the crate compiling + green.

## Execution Handoff

Two execution options:
1. **Subagent-Driven (recommended)** — fresh subagent per task, two-stage review between tasks.
2. **Inline Execution** — batch with checkpoints.

Which approach?
