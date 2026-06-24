# Wordcartel Effort 4c-1 — Filter Primitive & Pandoc Export — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `filter` primitive — pipe the selection/buffer through an external CLI and route the result back (Filter/Insert/Export) off the keystroke path — plus pandoc export presets and a minimal filter-command minibuffer.

**Architecture:** A synchronous subprocess engine (`subprocess` crate) drives the child with deadlock-safe stdin/stdout, timeout, size cap, and strict UTF-8. Each invocation runs on a **dedicated thread** (not the 4b FIFO worker) owning the child for kill; its result returns as a new `Msg::FilterDone` into the existing unified loop, where the foreground merges it via `by_id_mut(buffer_id).apply` (one undoable edit) with version-discard staleness. A single-line minibuffer (separate from the keypress modal `prompt`) collects typed filter commands. Pandoc export presets reuse the engine.

**Tech Stack:** Rust 2021, `subprocess` crate (new), `ropey` 1.6.1, `crossterm` 0.28, `std::thread`/`mpsc`, `proptest` (dev). Builds on 4b (job substrate, unified loop) + 4r (`Editor` over `Vec<Buffer>`, `Buffer::apply`, `by_id_mut`).

## Global Constraints

(From `docs/superpowers/specs/2026-06-24-wordcartel-04c1-filter-export-design.md`; bind every task.)

- **Responsiveness #1** (§3.9): the foreground never blocks on the subprocess; a filter dispatches, shows `running <cmd> ⏳`, the loop stays live, **Esc cancels**.
- **Foreground owns all `Editor` mutation**: the filter thread NEVER touches `Editor`; it owns only the child (kill) and sends a result. `filter_in_flight` is set/cleared only on the foreground (dispatch / Esc / FilterDone arms).
- **Single mutation channel** (§10.1): a filter's document change happens only via the **target buffer's** `apply` — `editor.by_id_mut(buffer_id)`'s `Buffer::apply` with one `Transaction`, `EditKind::Other` (one undoable edit, no coalescing). **Never `active_mut()`/`Editor::apply()`** for a filter result.
- **Reconcile = discard** (§10.3): a result whose `(buffer_id, version)` no longer matches the live buffer is **discarded**, never applied. The captured `input_range` is byte-valid iff `version` is unchanged (cursor motion doesn't bump `version`); stale results are dropped before any apply → no OOB/panic.
- **Security**: argv-default, no implicit shell; `shell: true` is a **per-preset** opt-in (typed input can never set it). Typed filters are whitespace-split with **no quoting** (documented limitation). Shell presets pass dynamic values as argv, never spliced into `sh -c`.
- **Never lose work** (§15): the buffer is replaced **only** after a successful, fully-collected, UTF-8-valid, under-cap result; the source `.md` is never touched by a filter; export writes **temp-then-rename**; never `unwrap` on a subprocess path.
- **Cancel kill scope**: the shell crate is `#![forbid(unsafe_code)]`, so process-group kill is out. **4c-1 kills the child only** (`terminate()` then `kill()`); group-kill defers to shell presets (4c-2/5).
- **`prompt` xor `minibuffer`**: at most one is `Some`; the minibuffer intercepts only `Msg::Input` keys — `Msg::FilterDone`/`JobDone`/`Tick` always run (do not starve background results — the 4b lesson).
- **Workspace facts:** `cargo test` from repo root; binary `wcartel`; baseline at start = 142 shell + 105 core + 34 oracle + integration, all green. New dep `subprocess`. No test weakened. Swap/file-writing tests use unique temp paths (4b-2 discipline).

### Plumbing corrections (Codex plan-review — apply these throughout)

These cross-cut several tasks; the per-task steps below are written against them.

1. **`msg_tx` reaches every dispatch path via `Ctx`.** `registry::Ctx` gains
   `pub msg_tx: &'a std::sync::mpsc::Sender<crate::app::Msg>`. `reduce` (which now
   takes `msg_tx`) builds every `Ctx` with it; `Registry::dispatch` already passes
   `Ctx` to handlers, so the `filter`/`export_*` handlers can send `Msg::FilterDone`.
   `resolve_prompt` **also gains `msg_tx`** (it re-runs export on `OverwriteExport`).
   All `Ctx { editor, clock, executor }` literals + `reduce(..)` + `resolve_prompt(..)`
   call sites (production + tests) get the new field/arg (mechanical; tests pass a
   throwaway `let (tx,_rx)=std::sync::mpsc::channel()`).
2. **`Msg::FilterDone` must NOT be starved by an open `prompt`/`minibuffer`.** Today
   `reduce`'s prompt-interception block returns early and only re-handles
   `Msg::JobDone`. Add a `Msg::FilterDone` branch **inside** that block (parallel to
   `JobDone`) — and the same inside the new minibuffer block — so a filter result
   merges even while a modal/minibuffer is open.
3. **Cancellation actually kills the child.** Esc setting an atomic flag that
   `run_filter` only checks *after* a blocking read does nothing (the read blocks for
   the full timeout). `run_filter` instead runs a **poll loop**: read with a short
   per-iteration wait (e.g. 50 ms), accumulate stdout, and **each iteration** check
   the `CancelFlag`, the overall `timeout` deadline, and the `max_output` cap —
   `terminate()` (then `kill()`) the child the moment any trips. Esc therefore kills
   within ~one poll interval. (Confirm the `subprocess` Communicator supports
   incremental bounded reads; if not, fall back to a stdin-writer thread + a
   stdout-reader thread + a `wait_timeout` poll loop that `terminate()`s on the flag.
   The **contract** is fixed: Esc/timeout must terminate the child promptly, never
   wait out the full timeout.)
4. **Export carries bytes + explicit paths** (binary formats): see Task 5 — a
   byte-capable export result, an `Editor.pending_export` state for the overwrite
   re-run, and a new `file::save_atomic_bytes`.

---

## File Structure

- `wordcartel/src/filter.rs` *(new)* — `FilterSpec`/`Disposition`/`Input`/`ExportSink`/`FilterOutcome`/`FilterError`; `run_filter` (sync engine); `dispatch_filter` (thread + Msg::FilterDone); `CancelFlag`.
- `wordcartel/src/minibuffer.rs` *(new)* — `Minibuffer { prompt, text, cursor }` + editing methods.
- `wordcartel/src/export.rs` *(new)* — pandoc probe, presets, derived path, temp-then-rename atomic output.
- `wordcartel/src/editor.rs` *(modify)* — `minibuffer: Option<Minibuffer>`, `filter_in_flight: Option<CancelFlag>`.
- `wordcartel/src/app.rs` *(modify)* — `Msg::FilterDone`; `reduce` gains `msg_tx` + the FilterDone arm + minibuffer key routing; `run` passes `msg_tx`; Esc cancellation.
- `wordcartel/src/commands.rs` *(modify)* — expose a public `build_range_replace` helper.
- `wordcartel/src/prompt.rs` *(modify)* — `PromptAction::OverwriteExport` + `Prompt::export_overwrite`.
- `wordcartel/src/registry.rs` *(modify)* — `filter` + `export_*` command-ids.
- `wordcartel/src/render.rs` *(modify)* — paint the minibuffer / `running ⏳`.
- `wordcartel/Cargo.toml` *(modify)* — add `subprocess`.

---

## Task 1: `subprocess` dep + filter data types + public range-replace helper

Foundation: the data model (no execution) and the public helper the filter merge needs (the existing range-replace builder is private in `commands.rs`).

**Files:**
- Modify: `wordcartel/Cargo.toml`
- Create: `wordcartel/src/filter.rs`
- Modify: `wordcartel/src/lib.rs` (`pub mod filter;`), `wordcartel/src/commands.rs`
- Test: `wordcartel/src/filter.rs`, `wordcartel/src/commands.rs`

**Interfaces:**
- Produces:
  - In `filter.rs`: `pub struct FilterSpec { pub argv: Vec<String>, pub shell: bool, pub disposition: Disposition, pub input: Input, pub timeout: std::time::Duration, pub max_output: usize }`; `pub enum Disposition { Filter, Insert, Export(ExportSink) }`; `pub enum ExportSink { Capture { ext: String }, WritesOutput { ext: String } }`; `pub enum Input { SelectionElseBuffer, None, WholeBuffer }`; `pub enum FilterOutcome { Replaced(String), Inserted(String), Exported(std::path::PathBuf), Failed(FilterError) }`; `pub enum FilterError { Spawn(String), NonZero { code: String, stderr: String }, Timeout, Cancelled, TooLarge, NotUtf8, ExportWrite(String) }` (all derive `Clone, Debug`; `FilterError: PartialEq` for tests).
  - In `commands.rs`: `pub fn build_range_replace(from: usize, to: usize, text: &str, doc_len: usize) -> (wordcartel_core::change::ChangeSet, wordcartel_core::block_tree::Edit)` — returns the `ChangeSet` AND the matching `Edit { range: from..to, new_len: text.len() }` for `Buffer::apply`. The existing private `replace_changeset` is reimplemented to call it (or `build_range_replace` wraps it).

- [ ] **Step 1: Add the dependency.** In `wordcartel/Cargo.toml` `[dependencies]`, add:
```toml
subprocess = "0.2"
```

- [ ] **Step 2: Write the failing test** in `wordcartel/src/commands.rs` tests:
```rust
#[test]
fn build_range_replace_yields_changeset_and_matching_edit() {
    use crate::editor::Editor;
    use wordcartel_core::history::{EditKind, Transaction};
    let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
    let doc_len = e.active().document.buffer.len();
    // Replace bytes 1..3 ("bc") with "X".
    let (cs, edit) = build_range_replace(1, 3, "X", doc_len);
    assert_eq!((edit.range.clone(), edit.new_len), (1..3, 1));
    let txn = Transaction::new(cs).with_selection(wordcartel_core::selection::Selection::single(2));
    e.active_mut().apply(txn, edit, EditKind::Other, &TestClock(0));
    assert_eq!(e.active().document.buffer.to_string(), "aXde\n");
}
```
(Use the `TestClock` helper already in the `commands.rs` test module.)

- [ ] **Step 3: Run to verify failure.**
Run: `cargo test -p wordcartel --lib commands::tests::build_range_replace_yields_changeset_and_matching_edit`
Expected: FAIL — `build_range_replace` not found.

- [ ] **Step 4: Add `build_range_replace`** to `commands.rs` (extract from the private `replace_changeset`, lines ~76-101, and return the `Edit` too):
```rust
/// Build a `(ChangeSet, Edit)` replacing byte range `from..to` with `text`.
/// Public so the filter merge (filter.rs) can produce one undoable edit.
pub fn build_range_replace(
    from: usize, to: usize, text: &str, doc_len: usize,
) -> (wordcartel_core::change::ChangeSet, wordcartel_core::block_tree::Edit) {
    let cs = replace_changeset(from, to, text, doc_len); // existing private builder
    let edit = wordcartel_core::block_tree::Edit { range: from..to, new_len: text.len() };
    (cs, edit)
}
```

- [ ] **Step 5: Declare the module + write `filter.rs` data types** (no execution yet). Add `pub mod filter;` to `lib.rs`, and the structs/enums above in `filter.rs`, plus a unit test constructing a `FilterSpec`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filter_spec_constructs() {
        let s = FilterSpec {
            argv: vec!["cat".into()], shell: false,
            disposition: Disposition::Filter, input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10), max_output: 1 << 20,
        };
        assert_eq!(s.argv, vec!["cat".to_string()]);
        assert!(matches!(s.disposition, Disposition::Filter));
    }
}
```

- [ ] **Step 6: Run the full suite + commit.**
Run: `cargo test` — the new tests pass; all 142 prior green; `cargo build --workspace` zero warnings (the unused filter types may warn — add `#[allow(dead_code)]` on the not-yet-used items with a `// wired in Task 2/3` note, removed as they get used).
```bash
git add wordcartel/Cargo.toml Cargo.lock wordcartel/src/filter.rs wordcartel/src/lib.rs wordcartel/src/commands.rs
git commit -m "feat(filter): subprocess dep + FilterSpec data types + public build_range_replace"
```

---

## Task 2: synchronous filter engine `run_filter`

The subprocess core, isolated and synchronous so its correctness is tested directly with real commands — no threading yet.

> **⚠️ VERIFY THE `subprocess` API AT IMPLEMENTATION TIME.** The crate is not cached in this environment, so the exact method names below are written from its documented API and MUST be confirmed against `cargo doc -p subprocess --open` / docs.rs before relying on them. The contract to satisfy is fixed; the spelling may differ:
> - spawn `argv` (or `["sh","-c",joined]` when `spec.shell`) with stdin/stdout/stderr piped;
> - feed all of `stdin`, read stdout, **deadlock-free**, under a **timeout** and a **byte size cap**;
> - distinguish: timeout, size-exceeded, non-zero exit (capture stderr + code), spawn ENOENT;
> - collect stdout as **bytes**, then `String::from_utf8` **strictly** (reject binary);
> - kill the child on cancel/timeout (`terminate()` then `kill()`).
> The expected shape: `Popen::create(&argv, PopenConfig { stdin: Redirection::Pipe, stdout: Redirection::Pipe, stderr: Redirection::Pipe, ..Default::default() })`; `popen.communicate_start(Some(stdin_bytes)).limit_time(timeout).limit_size(max_output).read() -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), CommunicateError>` where `CommunicateError::error.kind() == io::ErrorKind::TimedOut` on timeout and `.capture` holds partial output; `popen.wait()/poll() -> ExitStatus::Exited(u32)`.

**Files:**
- Modify: `wordcartel/src/filter.rs`
- Test: `wordcartel/src/filter.rs`

**Interfaces:**
- Consumes: `FilterSpec`, `FilterError` (Task 1).
- Produces:
  - `pub struct CancelFlag(pub std::sync::Arc<std::sync::atomic::AtomicBool>);` with `pub fn new() -> CancelFlag`, `pub fn cancel(&self)`, `pub fn is_cancelled(&self) -> bool`.
  - `pub fn run_filter(spec: &FilterSpec, stdin: String, cancel: &CancelFlag) -> RunResult` where `pub enum RunResult { Stdout(String), Exported, Err(FilterError) }` — synchronous: runs the child, returns collected stdout (Filter/Insert), `Exported` (Export, after the engine wrote the output — but the export *path* logic lives in Task 5; for now `Export` dispositions return `Stdout` for `Capture` or are handled in Task 5), or a `FilterError`. (Disposition-specific output handling — what to DO with the stdout — is the merge's job, Task 3/5; `run_filter` only produces the validated stdout or the error.)

- [ ] **Step 1: Write failing tests** in `filter.rs` (real commands available on any POSIX test box):
```rust
#[test]
fn run_filter_identity_cat() {
    let spec = FilterSpec { argv: vec!["cat".into()], shell: false,
        disposition: Disposition::Filter, input: Input::SelectionElseBuffer,
        timeout: std::time::Duration::from_secs(10), max_output: 1 << 20 };
    let out = run_filter(&spec, "hello\nworld\n".into(), &CancelFlag::new());
    assert!(matches!(out, RunResult::Stdout(ref s) if s == "hello\nworld\n"));
}
#[test]
fn run_filter_transform_tr() {
    let spec = FilterSpec { argv: vec!["tr".into(), "a-z".into(), "A-Z".into()], shell: false,
        disposition: Disposition::Filter, input: Input::SelectionElseBuffer,
        timeout: std::time::Duration::from_secs(10), max_output: 1 << 20 };
    let out = run_filter(&spec, "abc\n".into(), &CancelFlag::new());
    assert!(matches!(out, RunResult::Stdout(ref s) if s == "ABC\n"));
}
#[test]
fn run_filter_non_zero_exit_carries_stderr() {
    let spec = FilterSpec { argv: vec!["sh".into(), "-c".into(), "echo boom >&2; exit 3".into()],
        shell: false, disposition: Disposition::Filter, input: Input::SelectionElseBuffer,
        timeout: std::time::Duration::from_secs(10), max_output: 1 << 20 };
    match run_filter(&spec, "x\n".into(), &CancelFlag::new()) {
        RunResult::Err(FilterError::NonZero { code, stderr }) => {
            assert!(code.contains('3')); assert!(stderr.contains("boom"));
        }
        other => panic!("expected NonZero, got {other:?}"),
    }
}
#[test]
fn run_filter_rejects_oversized() {
    // `yes` floods stdout; a tiny cap must abort with TooLarge.
    let spec = FilterSpec { argv: vec!["yes".into()], shell: false,
        disposition: Disposition::Filter, input: Input::None,
        timeout: std::time::Duration::from_secs(5), max_output: 64 };
    assert!(matches!(run_filter(&spec, String::new(), &CancelFlag::new()), RunResult::Err(FilterError::TooLarge)));
}
#[test]
fn run_filter_rejects_non_utf8() {
    let spec = FilterSpec { argv: vec!["printf".into(), "\\xff".into()], shell: false,
        disposition: Disposition::Filter, input: Input::None,
        timeout: std::time::Duration::from_secs(10), max_output: 1 << 20 };
    assert!(matches!(run_filter(&spec, String::new(), &CancelFlag::new()), RunResult::Err(FilterError::NotUtf8)));
}
#[test]
fn run_filter_missing_binary_is_spawn_error() {
    let spec = FilterSpec { argv: vec!["wcartel-no-such-binary-xyz".into()], shell: false,
        disposition: Disposition::Filter, input: Input::SelectionElseBuffer,
        timeout: std::time::Duration::from_secs(10), max_output: 1 << 20 };
    assert!(matches!(run_filter(&spec, "x".into(), &CancelFlag::new()), RunResult::Err(FilterError::Spawn(_))));
}
```
(`RunResult` must derive `Debug` for the `panic!`/`matches!` messages.)

- [ ] **Step 2: Run to verify failure.**
Run: `cargo test -p wordcartel --lib filter::tests::run_filter`
Expected: FAIL — `run_filter`/`CancelFlag`/`RunResult` not found.

- [ ] **Step 3: Implement `CancelFlag` + `run_filter`** in `filter.rs` against the verified `subprocess` API (skeleton — confirm method spellings per the ⚠️ note):
```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
pub struct CancelFlag(pub Arc<AtomicBool>);
impl CancelFlag {
    pub fn new() -> CancelFlag { CancelFlag(Arc::new(AtomicBool::new(false))) }
    pub fn cancel(&self) { self.0.store(true, Ordering::SeqCst); }
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::SeqCst) }
}

#[derive(Debug)]
pub enum RunResult { Stdout(String), Exported, Err(FilterError) }

pub fn run_filter(spec: &FilterSpec, stdin: String, cancel: &CancelFlag) -> RunResult {
    use subprocess::{Popen, PopenConfig, Redirection};
    let argv: Vec<String> = if spec.shell {
        vec!["sh".into(), "-c".into(), spec.argv.join(" ")]
    } else {
        spec.argv.clone()
    };
    let mut child = match Popen::create(&argv, PopenConfig {
        stdin: Redirection::Pipe, stdout: Redirection::Pipe, stderr: Redirection::Pipe,
        ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => return RunResult::Err(FilterError::Spawn(e.to_string())),
    };
    // POLL LOOP (Codex review): a single blocking communicate would ignore the
    // CancelFlag until it returns, so Esc could not kill a hung child. Instead loop
    // with a SHORT per-iteration wait, accumulating output, and check cancel /
    // overall-deadline / size each pass — terminate() promptly when any trips.
    const POLL: std::time::Duration = std::time::Duration::from_millis(50);
    let deadline = std::time::Instant::now() + spec.timeout;
    let mut comm = child.communicate_start(Some(stdin.into_bytes()));
    let mut out_buf: Vec<u8> = Vec::new();
    let mut err_buf: Vec<u8> = Vec::new();
    loop {
        if cancel.is_cancelled() { let _ = child.terminate(); let _ = child.kill(); return RunResult::Err(FilterError::Cancelled); }
        if std::time::Instant::now() >= deadline { let _ = child.terminate(); let _ = child.kill(); return RunResult::Err(FilterError::Timeout); }
        // Read with a short bound; on each pass append what arrived.
        match comm.limit_time(POLL).read() {     // confirm Communicator reuse + bounded read
            Ok((o, e)) => {
                if let Some(o) = o { out_buf.extend_from_slice(&o); }
                if let Some(e) = e { err_buf.extend_from_slice(&e); }
                break; // EOF on both streams -> child finished its I/O
            }
            Err(ce) => {
                // Append partial capture, then classify.
                let (po, pe) = ce.capture;  // confirm field name; partial output on timeout
                if let Some(o) = po { out_buf.extend_from_slice(&o); }
                if let Some(e) = pe { err_buf.extend_from_slice(&e); }
                if ce.error.kind() == std::io::ErrorKind::TimedOut {
                    // per-iteration POLL timeout: not done yet — loop again (the
                    // OVERALL deadline + cancel are checked at the top).
                } else {
                    let _ = child.terminate(); return RunResult::Err(FilterError::Spawn(ce.error.to_string()));
                }
            }
        }
        if out_buf.len() > spec.max_output { let _ = child.terminate(); let _ = child.kill(); return RunResult::Err(FilterError::TooLarge); }
    }
    let status = child.wait().ok();
    let (out, err) = (Some(out_buf), Some(err_buf));
    let stderr = String::from_utf8_lossy(&err.unwrap_or_default()).into_owned();
    match status {
        Some(subprocess::ExitStatus::Exited(0)) | None => {
            let bytes = out.unwrap_or_default();
            match String::from_utf8(bytes) {
                Ok(s) => RunResult::Stdout(s),
                Err(_) => RunResult::Err(FilterError::NotUtf8),
            }
        }
        Some(s) => RunResult::Err(FilterError::NonZero {
            code: format!("{s:?}"), stderr: truncate(&stderr, 200),
        }),
    }
}

fn truncate(s: &str, n: usize) -> String { s.chars().take(n).collect() }
```
**Notes for the implementer:** (a) confirm whether `limit_size` overflow returns an error or a flag on the `Communicator` — map the overflow case to `FilterError::TooLarge` precisely; the `yes`+tiny-cap test pins the behavior. (b) `read()` may be `read()` or `read_string()`; we want **bytes** (so we can reject non-UTF-8 ourselves), so use the byte form. (c) the `Cancelled` mid-wait check is best-effort; real Esc-kill is wired in Task 3 (the foreground kills via the held child) — for the sync engine the flag check is sufficient for the unit surface.

- [ ] **Step 4: Run tests + full suite.**
Run: `cargo test` — the 6 engine tests pass; all prior green. `cargo build --workspace` zero warnings. (If a CI box lacks `yes`/`printf`, gate those two tests `#[cfg(unix)]` and note it.)

- [ ] **Step 5: Commit.**
```bash
git add wordcartel/src/filter.rs
git commit -m "feat(filter): synchronous subprocess engine (timeout, size cap, strict UTF-8, exit handling)"
```

---

## Task 3: filter dispatch on a dedicated thread + `Msg::FilterDone` + version-discard merge

Wire the engine onto a per-invocation thread; deliver the result via a new `Msg::FilterDone`; merge it on the foreground by `buffer_id` with staleness; enforce one-at-a-time + Esc-cancel.

**Files:**
- Modify: `wordcartel/src/editor.rs` (`filter_in_flight` field), `wordcartel/src/app.rs` (`Msg::FilterDone`, `reduce` gains `msg_tx` + the arm, `run` passes it, Esc handling), `wordcartel/src/filter.rs` (`dispatch_filter`)
- Test: `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `run_filter`/`CancelFlag`/`RunResult`/`FilterSpec`/`Disposition` (T1/T2), `commands::build_range_replace`, `editor::{Editor, BufferId, by_id_mut}`, `Buffer::apply`.
- Produces:
  - `Editor.filter_in_flight: Option<crate::filter::CancelFlag>` (init `None`).
  - `Msg::FilterDone { buffer_id: BufferId, version: u64, range: std::ops::Range<usize>, cursor: usize, disposition: Disposition, outcome: RunResult }`.
  - `reduce(msg, editor, reg, ex, clock, msg_tx: &std::sync::mpsc::Sender<Msg>) -> bool` — **new trailing `msg_tx` param** (all callers updated).
  - `pub fn dispatch_filter(editor: &mut Editor, spec: FilterSpec, msg_tx: std::sync::mpsc::Sender<Msg>)` in `filter.rs` — captures `(buffer_id, version, range, cursor, snapshot)`, sets `filter_in_flight`, spawns the thread (materializes stdin from the snapshot+range, runs `run_filter`, sends `Msg::FilterDone`). Rejects with a status if a filter is already in flight.

- [ ] **Step 1: Write failing tests** in `app.rs` (merge logic via synthesized `Msg::FilterDone`; a throwaway `msg_tx`):
```rust
#[test]
fn filterdone_replaces_range_when_fresh() {
    use crate::editor::Editor;
    use crate::filter::{Disposition, RunResult};
    use crate::jobs::InlineExecutor;
    use crate::registry::Registry;
    let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
    let id = e.active().id; let v = e.active().document.version;
    let (tx, _rx) = std::sync::mpsc::channel();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    let msg = Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
        disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) };
    crate::app::reduce(msg, &mut e, &reg, &ex, &clk, &tx);
    assert_eq!(e.active().document.buffer.to_string(), "aXde\n");
    // one undo step restores the original
    e.active_mut().undo();
    assert_eq!(e.active().document.buffer.to_string(), "abcde\n");
}
#[test]
fn filterdone_discarded_when_version_moved() {
    use crate::editor::Editor;
    use crate::filter::{Disposition, RunResult};
    use crate::jobs::InlineExecutor; use crate::registry::Registry;
    let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
    let id = e.active().id; let stale_v = e.active().document.version;
    e.active_mut().document.version += 1; // simulate an intervening edit
    let (tx, _rx) = std::sync::mpsc::channel();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    crate::app::reduce(Msg::FilterDone { buffer_id: id, version: stale_v, range: 1..3, cursor: 2,
        disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &ex, &clk, &tx);
    assert_eq!(e.active().document.buffer.to_string(), "abcde\n", "stale filter result discarded");
    assert!(e.status.to_lowercase().contains("discarded"));
}
#[test]
fn filterdone_failure_shows_status_keeps_buffer() {
    use crate::editor::Editor;
    use crate::filter::{Disposition, RunResult, FilterError};
    use crate::jobs::InlineExecutor; use crate::registry::Registry;
    let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
    let id = e.active().id; let v = e.active().document.version;
    let (tx, _rx) = std::sync::mpsc::channel();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
        disposition: Disposition::Filter,
        outcome: RunResult::Err(FilterError::NonZero { code: "Exited(3)".into(), stderr: "boom".into() }) },
        &mut e, &reg, &ex, &clk, &tx);
    assert_eq!(e.active().document.buffer.to_string(), "abcde\n");
    assert!(e.status.contains("boom") && e.status.contains('3'));
}
#[test]
fn dispatch_filter_runs_real_command_and_delivers_filterdone() {
    // One live-thread integration test (deterministic: block on the channel).
    use crate::editor::Editor;
    use crate::filter::{dispatch_filter, FilterSpec, Disposition, Input};
    let mut e = Editor::new_from_text("abc\n", None, (80, 24));
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    let spec = FilterSpec { argv: vec!["tr".into(),"a-z".into(),"A-Z".into()], shell: false,
        disposition: Disposition::Filter, input: Input::SelectionElseBuffer,
        timeout: std::time::Duration::from_secs(10), max_output: 1 << 20 };
    dispatch_filter(&mut e, spec, tx);
    let msg = rx.recv().expect("FilterDone must arrive"); // blocks; no timing assert
    match msg { Msg::FilterDone { outcome: crate::filter::RunResult::Stdout(s), .. } => assert_eq!(s, "ABC\n"),
                other => panic!("expected FilterDone Stdout, got {other:?}") }
}
```
(Update the existing `reduce_*` tests to pass a throwaway `&tx` — a mechanical signature change; create `let (tx,_rx)=std::sync::mpsc::channel();` in each.)

- [ ] **Step 2: Run to verify failure.**
Run: `cargo test -p wordcartel --lib app::tests::filterdone app::tests::dispatch_filter_runs_real_command_and_delivers_filterdone`
Expected: FAIL — `Msg::FilterDone`, the new `reduce` arg, `dispatch_filter` missing.

- [ ] **Step 3: Add the `filter_in_flight` field** to `Editor` (`editor.rs`): `pub filter_in_flight: Option<crate::filter::CancelFlag>,` init `None` in `new_from_text`.

- [ ] **Step 4: Add `Msg::FilterDone`** to the `Msg` enum (`app.rs`) with the fields in Interfaces.

- [ ] **Step 5: Thread `msg_tx` through `reduce` + `Ctx` + `resolve_prompt`** (Plumbing corrections #1) — add the trailing `msg_tx: &std::sync::mpsc::Sender<Msg>` param to `reduce`; add `msg_tx` to `registry::Ctx` and build it from `reduce`'s `msg_tx`; add `msg_tx` to `resolve_prompt`. Update `run()` (passes `&msg_tx`) and every `reduce(...)`/`resolve_prompt(...)`/`Ctx{..}` call site (production + tests pass a throwaway `&tx`). **Add the `Msg::FilterDone` handling in TWO places (Plumbing correction #2):** (a) **inside** the `editor.prompt.is_some()` interception block, parallel to its existing `Msg::JobDone` branch (so a filter result merges even while a modal is open), and (b) inside the new minibuffer block (Task 4); plus the normal-match arm for the no-modal case. Factor the merge into a `fn apply_filter_done(editor, buffer_id, version, range, cursor, disposition, outcome, clock)` so all three sites call one function. Its body:
```rust
        Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => {
            editor.filter_in_flight = None;
            let stale = editor.by_id(buffer_id).map(|b| b.document.version) != Some(version);
            match outcome {
                _ if stale => { editor.status = "filter discarded — buffer changed".into(); }
                crate::filter::RunResult::Err(err) => {
                    editor.status = crate::filter::describe_error(&err); // "exit 3: boom" etc.
                }
                crate::filter::RunResult::Stdout(text) => {
                    if let Some(b) = editor.by_id_mut(buffer_id) {
                        let doc_len = b.document.buffer.len();
                        let (from, to, at) = match disposition {
                            crate::filter::Disposition::Filter => (range.start, range.end, range.start),
                            crate::filter::Disposition::Insert => (cursor, cursor, cursor),
                            crate::filter::Disposition::Export(_) => { editor_status_export(&mut editor.status); return !editor.quit; }
                        };
                        let (cs, edit) = crate::commands::build_range_replace(from, to, &text, doc_len);
                        let txn = wordcartel_core::history::Transaction::new(cs)
                            .with_selection(wordcartel_core::selection::Selection::single(at + text.len()));
                        b.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
                        crate::derive::rebuild(editor);
                        crate::nav::ensure_visible(editor);
                        editor.status = "filter applied".into();
                    }
                }
                crate::filter::RunResult::Exported => { editor.status = "exported".into(); }
            }
        }
```
(Add `crate::filter::describe_error(&FilterError) -> String` mapping each variant to a user message: `NonZero{code,stderr}` → `format!("{code}: {stderr}")`, `Timeout` → "filter timed out", `Cancelled` → "filter cancelled", `TooLarge` → "filter output too large", `NotUtf8` → "filter produced non-text output", `Spawn(m)` → `format!("cannot run filter: {m}")`, `ExportWrite(m)` → m. Resolve the borrow-split as in 4r's save merge: compute the status in a local, set `editor.status` after the `b` borrow ends; the snippet above is illustrative — make it compile by extracting `text`/status assembly so `b` and `editor.status`/`derive::rebuild(editor)` don't overlap-borrow. `derive::rebuild`/`ensure_visible` take `&mut Editor`, so call them after the `b` borrow ends.)

- [ ] **Step 6: Implement `dispatch_filter`** in `filter.rs`:
```rust
pub fn dispatch_filter(editor: &mut crate::editor::Editor, spec: FilterSpec, msg_tx: std::sync::mpsc::Sender<crate::app::Msg>) {
    if editor.filter_in_flight.is_some() {
        editor.status = "a filter is already running".into();
        return;
    }
    let b = editor.active();
    let buffer_id = b.id;
    let version = b.document.version;
    let sel = b.document.selection.primary();
    let (range, cursor) = match spec.input {
        Input::SelectionElseBuffer if !sel.is_empty() => (sel.from()..sel.to(), sel.from()),
        Input::SelectionElseBuffer | Input::WholeBuffer => (0..b.document.buffer.len(), sel.head),
        Input::None => (sel.head..sel.head, sel.head),
    };
    let snapshot = b.document.buffer.snapshot();      // O(1)
    let cancel = CancelFlag::new();
    editor.filter_in_flight = Some(cancel.clone());
    editor.status = format!("running {} \u{23F3}", spec.argv.first().cloned().unwrap_or_default());
    let disposition = spec.disposition.clone();
    let range_c = range.clone();
    std::thread::spawn(move || {
        // materialize stdin on the thread (off the keystroke path)
        let stdin = match spec.input {
            Input::None => String::new(),
            _ => snapshot.byte_slice(range_c.clone()).to_string(),
        };
        let outcome = run_filter(&spec, stdin, &cancel);
        let _ = msg_tx.send(crate::app::Msg::FilterDone {
            buffer_id, version, range: range_c, cursor, disposition, outcome,
        });
    });
}
```
(Confirm `ropey::Rope::byte_slice(range)` exists / use the project's slice helper; `Disposition`/`Input` need `Clone`.)

- [ ] **Step 7: Esc-cancel wiring** — in `reduce`'s key handling (foreground), when `filter_in_flight.is_some()` and the key is Esc, call `editor.filter_in_flight.take().unwrap().cancel();` + status `"cancelling…"`. (The thread observes the flag / the in-flight result then arrives as Cancelled or stale and is handled by the FilterDone arm.) Place this Esc check so it doesn't conflict with prompt/minibuffer Esc (a filter can run with no modal open).

- [ ] **Step 8: Run tests + full suite (run the thread test a few times).**
Run: `cargo test` then `for i in 1 2 3; do cargo test -p wordcartel --lib app::tests::dispatch_filter_runs_real_command_and_delivers_filterdone; done`
Expected: all pass; the merge tests + the live-thread test stable; `cargo build --workspace` zero warnings.

- [ ] **Step 9: Commit.**
```bash
git add wordcartel/src/editor.rs wordcartel/src/app.rs wordcartel/src/filter.rs
git commit -m "feat(filter): dedicated-thread dispatch + Msg::FilterDone + version-discard merge via by_id_mut + Esc-cancel"
```

---

## Task 4: Minibuffer + the `filter` command

The single-line input mode + the command that opens it + submit → `dispatch_filter`.

**Files:**
- Create: `wordcartel/src/minibuffer.rs`
- Modify: `wordcartel/src/editor.rs` (`minibuffer` field + the open invariant), `wordcartel/src/app.rs` (reduce minibuffer routing + submit), `wordcartel/src/registry.rs` (`filter` command-id), `wordcartel/src/render.rs` (paint), `wordcartel/src/lib.rs`
- Test: `wordcartel/src/app.rs`, `wordcartel/src/minibuffer.rs`

**Interfaces:**
- Consumes: `dispatch_filter`, `FilterSpec`/`Disposition`/`Input`.
- Produces:
  - `pub struct Minibuffer { pub prompt: String, pub text: String, pub cursor: usize }` + `pub fn insert(&mut self, c: char)`, `backspace`, `left`, `right`.
  - `Editor.minibuffer: Option<Minibuffer>` (init `None`) + `Editor::open_minibuffer(&mut self, prompt: &str)` asserting `self.prompt.is_none()`.
  - registry `filter` command-id (opens the minibuffer with prompt `> `).

- [ ] **Step 1: Write failing tests** (minibuffer editing + reduce routing + submit dispatches):
```rust
// minibuffer.rs
#[test]
fn minibuffer_edits_text() {
    let mut m = Minibuffer { prompt: "> ".into(), text: String::new(), cursor: 0 };
    for c in "abc".chars() { m.insert(c); }
    assert_eq!((m.text.as_str(), m.cursor), ("abc", 3));
    m.left(); m.backspace();
    assert_eq!((m.text.as_str(), m.cursor), ("ac", 1));
}
// app.rs
#[test]
fn minibuffer_routing_and_submit_dispatches_filter() {
    use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("abc\n", None, (80, 24));
    e.open_minibuffer("> ");
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    let key = |c: char| Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press, state: KeyEventState::NONE });
    for c in "cat".chars() { crate::app::reduce(Msg::Input(key(c)), &mut e, &reg, &ex, &clk, &tx); }
    assert_eq!(e.minibuffer.as_ref().unwrap().text, "cat");
    // Enter submits -> dispatch_filter -> a FilterDone arrives, minibuffer cleared
    let enter = Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press, state: KeyEventState::NONE });
    crate::app::reduce(Msg::Input(enter), &mut e, &reg, &ex, &clk, &tx);
    assert!(e.minibuffer.is_none(), "submit clears the minibuffer");
    match rx.recv().unwrap() { Msg::FilterDone { outcome: crate::filter::RunResult::Stdout(s), .. } => assert_eq!(s, "abc\n"),
                               o => panic!("expected FilterDone, got {o:?}") }
}
#[test]
fn minibuffer_does_not_starve_filterdone() {
    use crate::editor::Editor; use crate::filter::{Disposition, RunResult};
    use crate::jobs::InlineExecutor; use crate::registry::Registry;
    let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
    let id = e.active().id; let v = e.active().document.version;
    e.open_minibuffer("> ");
    let (tx, _rx) = std::sync::mpsc::channel(); let reg = Registry::builtins();
    let ex = InlineExecutor::default(); let clk = TestClock(0);
    crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
        disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &ex, &clk, &tx);
    assert_eq!(e.active().document.buffer.to_string(), "aXde\n", "FilterDone applies even under an open minibuffer");
}
```

- [ ] **Step 2: Run to verify failure.**
Run: `cargo test -p wordcartel --lib minibuffer:: app::tests::minibuffer`
Expected: FAIL — `Minibuffer`/`open_minibuffer`/routing missing.

- [ ] **Step 3: Write `minibuffer.rs`** (struct + methods, char-cursor over `text`; for simplicity treat `cursor` as a char index and operate on a `Vec<char>` or byte-safe ops — keep ASCII-simple, document that multibyte caret in the minibuffer is fine since `text` is small):
```rust
pub struct Minibuffer { pub prompt: String, pub text: String, pub cursor: usize }
impl Minibuffer {
    pub fn insert(&mut self, c: char) { self.text.insert(self.cursor, c); self.cursor += c.len_utf8(); }
    pub fn backspace(&mut self) {
        if self.cursor == 0 { return; }
        let prev = self.text[..self.cursor].chars().next_back().map(char::len_utf8).unwrap_or(0);
        self.cursor -= prev; self.text.replace_range(self.cursor..self.cursor + prev, "");
    }
    pub fn left(&mut self) { if self.cursor > 0 { let p = self.text[..self.cursor].chars().next_back().unwrap().len_utf8(); self.cursor -= p; } }
    pub fn right(&mut self) { if self.cursor < self.text.len() { let n = self.text[self.cursor..].chars().next().unwrap().len_utf8(); self.cursor += n; } }
}
```
Declare `pub mod minibuffer;` in `lib.rs`.

- [ ] **Step 4: Add the `Editor` field + open helper** (`editor.rs`): `pub minibuffer: Option<crate::minibuffer::Minibuffer>,` init `None`; and `pub fn open_minibuffer(&mut self, prompt: &str) { debug_assert!(self.prompt.is_none(), "prompt xor minibuffer"); self.minibuffer = Some(crate::minibuffer::Minibuffer { prompt: prompt.into(), text: String::new(), cursor: 0 }); }`.

- [ ] **Step 5: Minibuffer key routing in `reduce`** — after the prompt-interception block, add a minibuffer block that intercepts ONLY `Msg::Input` keys (FilterDone/JobDone/Tick fall through to their arms):
```rust
    if editor.minibuffer.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                match k.code {
                    crossterm::event::KeyCode::Char(c) if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => editor.minibuffer.as_mut().unwrap().insert(c),
                    crossterm::event::KeyCode::Backspace => editor.minibuffer.as_mut().unwrap().backspace(),
                    crossterm::event::KeyCode::Left => editor.minibuffer.as_mut().unwrap().left(),
                    crossterm::event::KeyCode::Right => editor.minibuffer.as_mut().unwrap().right(),
                    crossterm::event::KeyCode::Esc => { editor.minibuffer = None; }
                    crossterm::event::KeyCode::Enter => {
                        let line = editor.minibuffer.take().unwrap().text;
                        submit_filter_line(editor, &line, msg_tx);
                    }
                    _ => {}
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        // non-key (FilterDone/JobDone/Tick/Resize) falls through to the normal match below
    }
```
Add `fn submit_filter_line(editor, line: &str, msg_tx: &Sender<Msg>)`: build `FilterSpec { argv: line.split_whitespace().map(String::from).collect(), shell: false, disposition: Filter, input: SelectionElseBuffer, timeout: 10s, max_output: 1<<20 }`; if argv empty → status + return; else `crate::filter::dispatch_filter(editor, spec, msg_tx.clone())`.

- [ ] **Step 6: Register the `filter` command + bind a key** (Codex review — it must be reachable; there is no palette yet). In `registry.rs` `builtins()`: `map.insert(CommandId("filter"), |c| { c.editor.open_minibuffer("> "); CommandResult::Handled });` (confirm the `Ctx`/handler shape). In `input.rs` `key_to_command_id`, map a concrete chord to `"filter"` — use **`Ctrl+E`** (mnemonic: "execute/external"; verify it's unbound in the current keymap; if taken, pick another free Ctrl key and note it). Add a keymap test asserting `Ctrl+E → Id(CommandId("filter"))`, mirroring the existing `keymap_*` tests.

- [ ] **Step 7: Render the minibuffer** (`render.rs`) — when `editor.minibuffer.is_some()`, paint `<prompt><text>` on the status row with the caret at `prompt.len()+cursor` (overrides the normal status line, like the modal prompt rendering); else unchanged. Add a render test mirroring the existing `renders_active_prompt_on_status_row`.

- [ ] **Step 8: Run tests + full suite + commit.**
Run: `cargo test` (+ 3× parallel shell-lib); `cargo build --workspace` zero warnings.
```bash
git add wordcartel/src/minibuffer.rs wordcartel/src/editor.rs wordcartel/src/app.rs wordcartel/src/registry.rs wordcartel/src/render.rs wordcartel/src/lib.rs
git commit -m "feat(minibuffer): single-line filter-command input + filter command + render"
```

---

## Task 5: Pandoc export

Export presets reusing the engine: startup probe + graceful disable, derived output path, scratch refusal, dedicated overwrite-confirm action, temp-then-rename atomic output.

**Files:**
- Create: `wordcartel/src/export.rs`
- Modify: `wordcartel/src/file.rs` (`save_atomic_bytes`), `wordcartel/src/editor.rs` (`pending_export` field), `wordcartel/src/prompt.rs` (`OverwriteExport` + `export_overwrite`), `wordcartel/src/app.rs` (`resolve_prompt` gains `msg_tx` + the `OverwriteExport` arm), `wordcartel/src/registry.rs` (`export_*` ids), `wordcartel/src/lib.rs`
- Test: `wordcartel/src/file.rs`, `wordcartel/src/export.rs`, `wordcartel/src/app.rs`

**Interfaces (revised per Codex review — bytes-capable export + explicit state):**
- Consumes: `FilterSpec`/`Disposition::Export`/`ExportSink`, `prompt::{Prompt, PromptAction}`, `Ctx.msg_tx` (Plumbing #1).
- Produces:
  - `file::save_atomic_bytes(path: &Path, bytes: &[u8]) -> Result<(), SaveError>` — **byte-capable** atomic write (same-dir O_EXCL temp `0600`, write, fsync, rename, dir-fsync) for **binary** export output (`file::save_atomic` is UTF-8-text-only and cannot be reused).
  - `pub fn probe_pandoc() -> bool` (spawn `pandoc --version`; `ENOENT` → false; cached on the workspace, set at `run()` startup).
  - `pub fn derived_export_path(source: &Path, ext: &str) -> PathBuf` (`set_extension`).
  - `Editor.pending_export: Option<PendingExport>` where `PendingExport { ext: String, target: PathBuf }` — set when the overwrite prompt is raised; read by `resolve_prompt`'s `OverwriteExport` arm to re-run.
  - `pub fn run_export(editor: &mut Editor, ext: &str, msg_tx: &Sender<Msg>)` — refuse scratch (`save the file first`); pandoc-absent → status; derive `target`; if `target` exists → set `pending_export` + raise `Prompt::export_overwrite(&target)`; else `do_export(editor, ext, &target, msg_tx)`.
  - `fn do_export(editor, ext, target, msg_tx)` — dispatch the pandoc run on the filter thread; **the export result carries bytes + paths**: extend `Msg::FilterDone`'s `outcome`/disposition path, OR (cleaner) add a dedicated `Msg::ExportDone { buffer_id, target: PathBuf, result: Result<Vec<u8> | (), FilterError> }`. For `Capture`, the thread returns the child's **stdout bytes**; the foreground writes them with `save_atomic_bytes(&target, &bytes)`. For `WritesOutput`, pandoc writes a **temp** path (`-o <target>.tmp-<pid>`); on child success the foreground `rename`s tmp → target. Either sink leaves an existing `target` untouched on any failure. **`RunResult` gains an `ExportBytes(Vec<u8>)` variant** (or use the dedicated `ExportDone` Msg) — do not force binary through `String`.
  - `PromptAction::OverwriteExport` + `Prompt::export_overwrite(path)` (distinct from save `Overwrite`).

- [ ] **Step 1: Write failing tests** (path derivation; scratch refusal; capture-write-rename using a stub "pandoc" = `cat`/a script; pandoc-absent disable; overwrite raises the new action):
```rust
#[test]
fn derived_export_path_swaps_extension_beside_source() {
    let p = derived_export_path(std::path::Path::new("/a/b/notes.md"), "html");
    assert_eq!(p, std::path::Path::new("/a/b/notes.html"));
}
#[test]
fn export_refuses_scratch_buffer() {
    use crate::editor::Editor;
    let mut e = Editor::new_from_text("x\n", None, (80, 24)); // scratch (no path)
    let (tx, _rx) = std::sync::mpsc::channel();
    run_export(&mut e, "html", &tx);
    assert!(e.status.to_lowercase().contains("save the file first"));
}
#[test]
fn export_overwrite_action_is_distinct_from_save_overwrite() {
    use crate::prompt::{Prompt, PromptAction};
    let p = Prompt::export_overwrite(std::path::Path::new("/a/notes.html"));
    assert_eq!(p.action_for('o'), Some(PromptAction::OverwriteExport));
    assert_ne!(PromptAction::OverwriteExport, PromptAction::Overwrite);
}
```
(A full capture-write-rename test uses a stub pandoc: build a `FilterSpec` whose argv is `["cat"]` with `Disposition::Export(Capture{ext:"txt"})`, run the export against a named temp buffer, assert the output file exists beside the source with the buffer content, then clean up. Place it with unique temp paths.)

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib export:: app::tests::export` → FAIL (module/action missing).

- [ ] **Step 3: Add `file::save_atomic_bytes`** (`file.rs`) — a byte-capable sibling of `save_atomic` (same-dir O_EXCL `0600` temp → write bytes → `flush`/`sync_all` → `rename` → dir-`fsync`; `TempGuard` cleanup; no UTF-8/skip-unchanged checks — export output is binary). Test: write a byte buffer (incl. a `0xFF` byte) to a unique temp path, read it back, assert equal; assert no `.tmp` litter remains (mirror `no_temp_litter_after_save`).

- [ ] **Step 4: Add the overwrite action + state + resolver plumbing.** In `prompt.rs`: `PromptAction::OverwriteExport` + `Prompt::export_overwrite(path)` (`[O]verwrite · [C]ancel`, message names the path; distinct from save `Overwrite`). In `editor.rs`: `pub pending_export: Option<crate::export::PendingExport>` (init `None`). In `app.rs`: `resolve_prompt` gains `msg_tx` (Plumbing #1); its `OverwriteExport` arm reads `editor.pending_export.take()` and calls `export::do_export(editor, &pe.ext, &pe.target, msg_tx)`; `Cancel` on the export prompt clears `pending_export`.

- [ ] **Step 5: Write `export.rs`** — `probe_pandoc` (spawn `pandoc --version`; `ENOENT` → false; cached on the workspace, set at `run()` startup); `derived_export_path` (`set_extension`); `run_export` (scratch refusal; pandoc-absent status; derive `target`; if exists → `editor.pending_export = Some(PendingExport{ext,target})` + raise `Prompt::export_overwrite(&target)`; else `do_export`); `do_export` (dispatch the pandoc run on the filter thread; the thread returns the child's stdout **bytes** for `Capture` or signals success for `WritesOutput`; on the foreground, `Capture` → `file::save_atomic_bytes(&target, &bytes)`, `WritesOutput` → pandoc wrote `<target>.tmp-<pid>` which the foreground `rename`s to `target`; either leaves an existing `target` untouched on failure). Confirm the exact pandoc argv/flags per `ext` in an impl note. Declare `pub mod export;` + the `PendingExport { ext: String, target: PathBuf }` struct. **The export result delivery uses a dedicated `Msg::ExportDone { buffer_id, target, result }`** (or the `RunResult::ExportBytes` variant per the Interfaces) — do NOT route binary through the `Stdout(String)` path.

- [ ] **Step 6: Register `export_html`/`export_docx`/`export_pdf` command-ids** (`registry.rs`) calling `export::run_export(c.editor, ext, c.msg_tx)` (Ctx now carries `msg_tx`). If `probe_pandoc()` was false at startup, the handler reports `pandoc not found — install it to export` (graceful disable; editing unaffected). Add a `Msg::ExportDone` arm in `reduce` (also inside the prompt block) writing/renaming + status, mirroring `FilterDone`.

- [ ] **Step 7: Run tests + full suite + commit.**
Run: `cargo test`; `cargo build --workspace` zero warnings.
```bash
git add wordcartel/src/export.rs wordcartel/src/file.rs wordcartel/src/editor.rs wordcartel/src/prompt.rs wordcartel/src/app.rs wordcartel/src/registry.rs wordcartel/src/lib.rs
git commit -m "feat(export): pandoc presets (probe, derived path, save_atomic_bytes temp-rename, OverwriteExport + pending_export)"
```

---

## Self-Review (4c-1)

**Spec coverage:** §4.2 FilterSpec model (T1); §4.3 engine + dispatch + version-discard merge via `by_id_mut` + Esc-cancel (T2/T3); §5 minibuffer + invariant + reducer order (T4); §6 pandoc export incl. `OverwriteExport` + temp-rename + scratch refusal + probe/disable (T5); §3 security (argv-default typed filters, no shell — T2/T4; no-quoting documented); §7 error handling (T2 errors → T3 `describe_error` status). ✅

**Codex plan-review fixes (applied):** (1) cancellation is a **poll loop** that terminates the child within ~one `POLL` interval on Esc/timeout (not a flag checked after a blocking read); (2) `Msg::FilterDone`/`ExportDone` are handled **inside** the prompt + minibuffer interception blocks (factored into one `apply_filter_done`), so a result never gets starved by an open modal; (3) `registry::Ctx` gains `msg_tx` (and `resolve_prompt` too) so the `export_*` handlers + the overwrite re-run can dispatch; (4) `Editor.pending_export` carries the overwrite re-run state (payload-free `PromptAction` can't); (5) export is **byte-capable** (a dedicated `Msg::ExportDone`/`ExportBytes`, not `String`) with `file::save_atomic_bytes` (temp→rename) since `save_atomic` is UTF-8-only; (6) the `filter` command is **bound to a real key** (Ctrl+E) since there's no palette yet.

**Known external-API caveat:** Task 2's `subprocess` calls are written against the documented API but **must be verified** at implementation time (the crate isn't cached) — the ⚠️ note + the deterministic real-command tests are the safety net; the contract (timeout/size/UTF-8-reject/exit/kill) is fixed. Codex confirmed `ropey::Rope::byte_slice(range)` exists (the `dispatch_filter` snapshot slice is sound).

**Borrow-split reminders:** the FilterDone merge and `run_export` must assemble status/text in locals so the `by_id_mut(b)` borrow ends before `editor.status`/`derive::rebuild(editor)` (the 4r save-merge pattern). The plan's snippets are illustrative on this point — make them compile with that discipline.

**Type consistency:** `FilterSpec`/`Disposition`/`Input`/`ExportSink`/`FilterOutcome`/`RunResult`/`FilterError`/`CancelFlag`, `Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome }`, `reduce(.., msg_tx)`, `dispatch_filter`, `build_range_replace`, `Minibuffer`, `open_minibuffer`, `PromptAction::OverwriteExport` — used consistently across tasks. `reduce`'s new `msg_tx` param ripples to `run()` + all reduce tests (called out in T3).

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-24-wordcartel-04c1-filter-export.md`. (Siblings 4c-2 repar transforms + 4c-3 clipboard get their own spec→plan cycles.)
