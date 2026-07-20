//! Filter primitive — subprocess execution and async dispatch.
//!
//! Execution (spawning subprocesses, merging output) is wired in Task 2/3.
//! Export types (`ExportSink`, `ExportResult`) live in `export.rs` (Task 5).

/// Specification for a text-filter command.
pub struct FilterSpec {
    pub argv: Vec<String>,
    pub shell: bool,
    pub disposition: Disposition,
    pub input: Input,
    pub timeout: std::time::Duration,
    pub max_output: usize,
}

/// How the filter's output is merged back into the document.
///
/// TEXT-ONLY: `Export` is a separate path built in Task 5 — not here.
#[derive(Clone, Debug)]
pub enum Disposition {
    /// Replace the input range with the filter's stdout.
    Filter,
    /// Insert the filter's stdout after the input range (input is not deleted).
    Insert,
}

/// What bytes are sent to the subprocess's stdin.
#[derive(Clone, Debug)]
pub enum Input {
    /// Send the primary selection if non-empty, otherwise the whole buffer.
    SelectionElseBuffer,
    /// Send nothing (empty stdin).
    None,
    /// Always send the entire buffer.
    WholeBuffer,
}

/// Errors that can occur when running a filter.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterError {
    /// The subprocess could not be spawned.
    Spawn(String),
    /// The subprocess exited with a non-zero status.
    NonZero { code: String, stderr: String },
    /// The subprocess did not finish within the timeout.
    Timeout,
    /// The operation was cancelled.
    Cancelled,
    /// The subprocess output exceeded `max_output` bytes.
    TooLarge,
    /// The subprocess output was not valid UTF-8.
    NotUtf8,
    /// Writing the export file failed (used by export path, Task 5).
    ExportWrite(String),
    /// The filter or export worker thread panicked.
    Panicked(String),
}

// ---------------------------------------------------------------------------
// CancelFlag — wired in Task 3/5
// ---------------------------------------------------------------------------

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cheap-to-clone flag that lets an async caller cancel a running subprocess.
///
/// The poll loop in `run_subprocess` checks this every ~50 ms and kills the
/// child immediately when it is set, so Esc-key cancellation latency is bounded.
#[derive(Clone, Debug)]
pub struct CancelFlag(pub Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> CancelFlag {
        CancelFlag(Arc::new(AtomicBool::new(false)))
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

impl Default for CancelFlag {
    fn default() -> Self {
        CancelFlag::new()
    }
}

// ---------------------------------------------------------------------------
// RunResult — wired in Task 3/5
// ---------------------------------------------------------------------------

/// Result of running a text filter.
///
/// No `Exported` variant: export is a separate path built in Task 5, which
/// calls `run_subprocess` directly for raw bytes.
#[derive(Debug)]
pub enum RunResult {
    /// The filter produced valid UTF-8 stdout.
    Stdout(String),
    /// The filter failed.
    Err(FilterError),
}

// ---------------------------------------------------------------------------
// run_subprocess — bytes core (wired in Task 5 for binary export)
// ---------------------------------------------------------------------------

/// Per-iteration poll window. Short enough that cancel latency stays inside the budget; long
/// enough not to burn CPU on well-behaved fast commands. Module-level (not inside
/// `run_subprocess`) so `REAP_GRACE` can sit beside it and `ReapGuard` can reach it.
const POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Grace for the bounded reap in `ReapGuard::drop`. Small on purpose: reaping a process the
/// kernel already destroyed with an uncatchable SIGKILL normally takes well under a millisecond,
/// and this sits inside the cancel budget (≤ POLL to notice + ≤ REAP_GRACE to reap ≈ 70 ms
/// requested — a target under normal scheduling, not a wall-clock guarantee).
const REAP_GRACE: std::time::Duration = std::time::Duration::from_millis(20);

/// Owns the child for the whole of `run_subprocess` and guarantees, on EVERY exit — normal
/// return, early return, or unwind — that dropping the inner `Popen` cannot block.
///
/// `Popen::drop` calls `self.wait()` (unbounded) whenever `detached == false` AND
/// `child_state == Running`. This makes at least one of those false first. Two crate facts make
/// the guard necessary rather than decorative: `kill()` leaves `child_state` as `Running`
/// whatever it returns, and `waitpid` promotes to `Finished` only on a successful reap or
/// `ECHILD` — so "we killed it" alone does not stop `Drop` from blocking.
struct ReapGuard(subprocess::Popen);

impl Drop for ReapGuard {
    fn drop(&mut self) {
        // Already reaped (the normal success path) — `Popen::drop` will not wait.
        if self.0.poll().is_some() { return; }
        let _ = self.0.terminate();
        let _ = self.0.kill();
        // Bounded reap: no zombie in the normal case. If the reap is not CONFIRMED — kill failed,
        // EINTR, uninterruptible sleep — detach so `Popen::drop` returns at once. Never `wait()`.
        if !matches!(self.0.wait_timeout(REAP_GRACE), Ok(Some(_))) {
            self.0.detach();
        }
    }
}

/// Feed the child's stdin from a dedicated thread.
///
/// An `Err` here — EPIPE included — means the child stopped reading, which is ORDINARY Unix
/// filter semantics, not a failure: it is the entire bug this split exists to fix. The `File`
/// drops when the closure ends, delivering EOF to a child still reading. Returns `None` (having
/// closed stdin at once) when there is nothing to send.
///
/// The returned handle is NEVER joined — see `run_subprocess`. Joining would block on every
/// process holding the pipe's read end, including descendants we never spawned and cannot see.
// fs-chokepoint-allow: (a) the child's own stdin PIPE handle, not a path opened by us
fn spawn_stdin_writer(stdin: Option<std::fs::File>, bytes: Vec<u8>)
    -> Option<std::thread::JoinHandle<()>>
{
    let f = stdin?;
    if bytes.is_empty() {
        drop(f); // empty input: close stdin immediately so the child sees EOF
        return None;
    }
    Some(std::thread::spawn(move || {
        use std::io::Write;
        let mut f = f;
        let _ = f.write_all(&bytes);
    }))
}

/// Spawn a subprocess, feed `stdin`, collect stdout bytes, enforce `timeout` and `max_output`,
/// respect `cancel`.
///
/// ## Two phases, one guard
///
/// stdin does NOT go through the `Communicator`. The `subprocess` crate propagates an EPIPE from
/// its internal stdin write as a fatal error and cannot resume afterwards (it neither clears its
/// stdin handle nor drains the pending input), so an early-exiting child — `head -1`, `grep -q` —
/// used to kill the run and discard output we had already captured. Instead a writer thread owns
/// stdin, where EPIPE is ordinary filter semantics and is ignored.
///
/// * **Phase 1 (drain):** poll stdout/stderr with a per-iteration `limit_time`, checking `cancel`
///   and the deadline every iteration. Ends at genuine EOF, or bails on the size cap.
/// * **Phase 2 (reap):** the same cancel/deadline guard around a bounded `wait_timeout`. This
///   phase exists because moving stdin out also moved it out of phase 1's protection: a child
///   that closes its outputs and keeps running would otherwise block a plain `wait()` forever
///   with nothing watching the deadline.
///
/// `ReapGuard` ensures `Popen::drop` can never block on any exit path, unwind included.
///
/// ## Size-cap behaviour (unchanged — see the CRITICAL note in the loop)
///
/// `limit_size(n)` makes `read()` return `Ok` once `n` COMBINED stdout+stderr bytes accumulate;
/// it does not signal EOF. We detect the cap by checking the combined length after each read.
// A flat, cohesive two-phase drain/reap loop: both phases share `deadline`, `guard` and the same
// cancel/deadline preamble, so splitting them would separate state that must be read together.
#[allow(clippy::too_many_lines)]
pub fn run_subprocess(
    argv: &[String],
    shell: bool,
    stdin: String,
    timeout: std::time::Duration,
    max_output: usize,
    cancel: &CancelFlag,
) -> Result<Vec<u8>, FilterError> {
    use subprocess::{ExitStatus, Popen, PopenConfig, Redirection};

    // Build the real argv: either direct exec or sh -c.
    let real_argv: Vec<String> = if shell {
        vec!["sh".into(), "-c".into(), argv.join(" ")]
    } else {
        argv.to_vec()
    };

    // Spawn with all three streams piped.
    let child = match Popen::create(
        &real_argv,
        PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    ) {
        Ok(c) => c,
        Err(e) => return Err(FilterError::Spawn(e.to_string())),
    };
    // ORDERING CONSTRAINT: wrap IMMEDIATELY. Nothing panic-capable may run while the `Popen` is
    // bare, or an unwind bypasses the guard and `Popen::drop` can block. Do not move this down.
    let mut guard = ReapGuard(child);

    let deadline = std::time::Instant::now() + timeout;

    // Take stdin OUT of the Popen and hand it to the writer thread; `communicate_start(None)` is
    // then legal (the crate asserts stdin.is_some() => input_data.is_some()) and the communicator
    // never touches stdin at all. The handle is deliberately never joined.
    let _writer = spawn_stdin_writer(guard.0.stdin.take(), stdin.into_bytes());

    // DROP-ORDER CONSTRAINT: `comm` must stay declared AFTER `guard` (as it is here — it is
    // built FROM `guard.0`), so that Rust's reverse-declaration-order drop closes our
    // stdout/stderr read ends BEFORE `ReapGuard::drop` attempts its bounded reap. That is what
    // lets the bounded reap succeed against a child blocked writing to a pipe we stopped
    // draining. Breaking this order does not hang — it silently degrades the reap into a
    // `detach()` fallback and leaks a zombie, which nothing in the suite would catch.
    let mut comm = guard.0.communicate_start(None);
    let mut out_buf: Vec<u8> = Vec::new();
    let mut err_buf: Vec<u8> = Vec::new();

    // ---- Phase 1: drain stdout/stderr under the cancel/deadline guard. ----
    loop {
        if cancel.is_cancelled() {
            let _ = guard.0.terminate();
            let _ = guard.0.kill();
            return Err(FilterError::Cancelled);
        }
        if std::time::Instant::now() >= deadline {
            let _ = guard.0.terminate();
            let _ = guard.0.kill();
            return Err(FilterError::Timeout);
        }

        // Remaining budget for this iteration (cap to POLL).
        let iter_time = POLL.min(deadline.saturating_duration_since(std::time::Instant::now()));

        // Ask for at most (max_output - captured + 1) bytes so limit_size will
        // trip at the right threshold.  CRITICAL: the subprocess crate's
        // `limit_size` counts the COMBINED stdout+stderr bytes of a read()
        // (communicate.rs: `total = outvec.len() + errvec.len()`), so we must
        // budget against the combined captured total — NOT stdout alone.  If we
        // budgeted on stdout only, a child that floods stderr would never trip
        // the cap here, `read()` would return Ok via the size_limit break with a
        // small stdout, and we would break to the reap phase while the child is
        // still blocked writing stderr to a full pipe we stopped draining —
        // deadlocking forever.  The +1 lets us see one byte past the cap so we
        // can distinguish "exactly max_output captured" from "more pending".
        let captured = out_buf.len() + err_buf.len();
        let remaining_cap = max_output.saturating_sub(captured) + 1;

        // Reassign comm with per-iteration limits; limit_time/limit_size take
        // self by value and return a new Communicator with the fields set, so
        // we must rebind rather than chain inline (which would move comm out of
        // the loop variable).
        comm = comm.limit_time(iter_time).limit_size(remaining_cap);
        match comm.read() {
            Ok((o, e)) => {
                // Ok means EITHER both streams hit EOF, OR the combined
                // size_limit was reached mid-stream (the crate breaks its read
                // loop on `total >= size_limit` and returns Ok — it does NOT
                // signal EOF).  The combined-overflow check below distinguishes
                // them: if we are over the cap it was a size_limit break (kill +
                // TooLarge — do NOT reap a child that may still be writing),
                // otherwise it is a genuine EOF and reaping is safe.
                if let Some(o) = o {
                    out_buf.extend_from_slice(&o);
                }
                if let Some(e) = e {
                    err_buf.extend_from_slice(&e);
                }
                // Combined size check after accumulating this batch.
                if out_buf.len() + err_buf.len() > max_output {
                    let _ = guard.0.terminate();
                    let _ = guard.0.kill();
                    return Err(FilterError::TooLarge);
                }
                // Genuine EOF (both streams closed, under cap) — go reap.
                break;
            }
            Err(ce) => {
                // Accumulate partial data regardless of error kind.
                let (po, pe) = ce.capture;
                if let Some(o) = po {
                    out_buf.extend_from_slice(&o);
                }
                if let Some(e) = pe {
                    err_buf.extend_from_slice(&e);
                }

                // Combined size cap — limit_size hit, or we accumulated too much.
                if out_buf.len() + err_buf.len() > max_output {
                    let _ = guard.0.terminate();
                    let _ = guard.0.kill();
                    return Err(FilterError::TooLarge);
                }

                if ce.error.kind() == std::io::ErrorKind::TimedOut {
                    // Per-iteration timeout expired — loop to check cancel/deadline.
                    continue;
                } else {
                    // A genuine stdout/stderr READ failure. Unreachable for stdin errors now:
                    // no stdin write happens inside the communicator any more.
                    let _ = guard.0.terminate();
                    let _ = guard.0.kill();
                    return Err(FilterError::Spawn(ce.error.to_string()));
                }
            }
        }
    }

    // ---- Phase 2: reap under the SAME guard. Never a blocking `wait()`. ----
    let status = loop {
        if cancel.is_cancelled() {
            let _ = guard.0.terminate();
            let _ = guard.0.kill();
            return Err(FilterError::Cancelled);
        }
        if std::time::Instant::now() >= deadline {
            let _ = guard.0.terminate();
            let _ = guard.0.kill();
            return Err(FilterError::Timeout);
        }
        let slice = POLL.min(deadline.saturating_duration_since(std::time::Instant::now()));
        match guard.0.wait_timeout(slice) {
            Ok(Some(st)) => break st,
            Ok(None) => continue,
            // Preserves today's `unwrap_or(Undetermined)` fallback. NOTE: this arm leaves
            // `child_state == Running` (the crate sets Finished only on success or ECHILD) —
            // `ReapGuard`, not this arm, is what stops `Popen::drop` blocking here.
            Err(_) => break ExitStatus::Undetermined,
        }
    };

    let stderr_str = String::from_utf8_lossy(&err_buf).into_owned();

    match status {
        ExitStatus::Exited(0) => Ok(out_buf),
        ExitStatus::Exited(code) => Err(FilterError::NonZero {
            code: code.to_string(),
            stderr: truncate(&stderr_str, 200),
        }),
        other => Err(FilterError::NonZero {
            code: format!("{other:?}"),
            stderr: truncate(&stderr_str, 200),
        }),
    }
}

// ---------------------------------------------------------------------------
// run_filter — text wrapper (wired in Task 3)
// ---------------------------------------------------------------------------

/// Run a text filter: spawn, feed stdin, collect stdout as UTF-8.
///
/// Thin wrapper around `run_subprocess` that converts the raw bytes to a
/// `String`, returning `Err(NotUtf8)` for non-UTF-8 output.
/// `Exported` is NOT a variant here — export is Task 5's separate path.
pub fn run_filter(spec: &FilterSpec, stdin: String, cancel: &CancelFlag) -> RunResult {
    match run_subprocess(
        &spec.argv,
        spec.shell,
        stdin,
        spec.timeout,
        spec.max_output,
        cancel,
    ) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => RunResult::Stdout(s),
            Err(_) => RunResult::Err(FilterError::NotUtf8),
        },
        Err(e) => RunResult::Err(e),
    }
}

pub fn describe_error(err: &FilterError) -> String {
    match err {
        FilterError::NonZero { code, stderr } => format!("{code}: {stderr}"),
        FilterError::Timeout => "filter timed out".into(),
        FilterError::Cancelled => "filter cancelled".into(),
        FilterError::TooLarge => "filter output too large".into(),
        FilterError::NotUtf8 => "filter produced non-text output".into(),
        FilterError::Spawn(m) => format!("cannot run filter: {m}"),
        FilterError::ExportWrite(m) => m.clone(),
        FilterError::Panicked(m) => format!("internal error: {m}"),
    }
}

fn guarded_filter(work: impl FnOnce() -> RunResult) -> RunResult {
    match crate::panicx::catch(work) {
        Ok(o) => o,
        Err(msg) => RunResult::Err(FilterError::Panicked(msg)),
    }
}

pub fn dispatch_filter(
    editor: &mut crate::editor::Editor,
    spec: FilterSpec,
    msg_tx: std::sync::mpsc::Sender<crate::app::Msg>,
) {
    if editor.active().read_only { editor.reject_read_only(); return; } // A17 T8: no work scheduled, no epilogue.
    if editor.filter_in_flight.is_some() {
        editor.set_status_full(crate::status::StatusKind::Warning, "a filter is already running",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
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
    let snapshot = b.document.buffer.snapshot();
    let cancel = CancelFlag::new();
    editor.filter_in_flight = Some(cancel.clone());
    // Self-replacing Progress on the static Filter topic (`filter_in_flight` guarantees one at a
    // time). `apply_filter_done` collapses this start with its terminal completion (§4.2).
    editor.set_progress(crate::status::StatusTopic::Filter,
        format!("running {} ...", spec.argv.first().cloned().unwrap_or_default()));
    let disposition = spec.disposition.clone();
    let range_c = range.clone();
    std::thread::spawn(move || {
        let stdin = match spec.input {
            Input::None => String::new(),
            _ => snapshot.byte_slice(range_c.clone()).to_string(),
        };
        let outcome = guarded_filter(|| run_filter(&spec, stdin, &cancel));
        let _ = msg_tx.send(crate::app::Msg::FilterDone {
            buffer_id,
            version,
            range: range_c,
            cursor,
            disposition,
            outcome,
        });
    });
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A filter spec for the EPIPE regression tests: direct exec (no shell wrapper), the real
    /// 64 MiB cap, and a caller-chosen timeout.
    fn spec_for(argv: &[&str], timeout_secs: u64) -> FilterSpec {
        FilterSpec {
            argv: argv.iter().map(|s| (*s).to_string()).collect(),
            shell: false,
            disposition: Disposition::Filter,
            input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(timeout_secs),
            max_output: crate::limits::MAX_FILTER_OUTPUT,
        }
    }

    /// 1 MiB — far past the 64 KiB default Linux pipe buffer, so a child that stops reading
    /// leaves our writer genuinely blocked mid-write. This is what makes the EPIPE deterministic
    /// instead of a race we lose 39% of the time under load.
    fn big_stdin() -> String {
        "x".repeat(1024 * 1024)
    }

    /// Test-only gate against the fd-inheritance race filed as **H30** — see that item before
    /// touching this. `subprocess 0.2.15` creates its pipes with a bare `libc::pipe()` and only
    /// sets `FD_CLOEXEC` afterwards, in a separate `fcntl` (`popen::set_inheritable`). A
    /// `fork()+exec` on another thread inside that window inherits the pipes of every filter
    /// running concurrently. If the inheriting child then outlives the victim's timeout it holds
    /// the victim's stdout/stderr WRITE ends open, the victim's drain loop never sees EOF, and
    /// the victim fails with `Timeout` — observed as 3 failures in 10 suite runs, a *different*
    /// victim each time, with one long-lived child seen holding four foreign pipe fds.
    ///
    /// Deliberately NOT a blanket serialization of the filter suite. A short-lived child makes
    /// the leak invisible — it returns the fds in milliseconds, long before any victim's 10s
    /// timeout — which is why this suite was green for as long as every child exited promptly.
    /// So ordinary spawning tests take this gate SHARED and still run concurrently exactly as
    /// they did before; only the three tests that keep a child alive for 30–600s, longer than
    /// any victim's timeout, take it EXCLUSIVE.
    ///
    /// **Remove this gate and both helpers when H30 is fixed at its root.** The fix cannot live
    /// in `run_subprocess`: the leak is *into the other spawn sites* (`harper_ls`, `export`, the
    /// clipboard helpers), so it needs a process-global spawn lock spanning all of them, or a
    /// patched `subprocess`.
    ///
    /// Checked scope, so a future reader does not have to redo it: `export.rs` (:211, :227) also
    /// calls `run_subprocess` and is victim-class, but no lib test drives that path today (the
    /// export tests exercise `pandoc_argv` construction, not a real spawn). The other spawn
    /// sites — `clipboard.rs`, `harper_ls.rs`, `export::probe_pandoc` — use
    /// `std::process::Command`, which creates its pipes via `pipe2(O_CLOEXEC)` atomically and so
    /// cannot inherit a foreign fd across this race; the `harper_ls` integration tests are
    /// separate binaries with their own fd tables besides. Nothing outside this module's own
    /// tests is currently exposed.
    static SPAWN_GATE: std::sync::RwLock<()> = std::sync::RwLock::new(());

    /// Shared arm of [`SPAWN_GATE`], for a test whose child exits promptly. Poison-tolerant: one
    /// panicking test must not cascade into unrelated failures across the whole suite.
    fn spawn_gate_shared() -> std::sync::RwLockReadGuard<'static, ()> {
        SPAWN_GATE.read().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Exclusive arm of [`SPAWN_GATE`], for a test that keeps a child alive past another test's
    /// timeout. Must be held across the `run_filter` call, because `fork()` is the moment the
    /// inheriting happens: with no other filter's pipes open at that instant, the long-lived
    /// child inherits nothing foreign and the rest of its life is harmless.
    fn spawn_gate_exclusive() -> std::sync::RwLockWriteGuard<'static, ()> {
        SPAWN_GATE.write().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // A17 T8 — driven through the real submit path (builds the spec, then calls `dispatch_filter`,
    // whose read-only entry guard fires before scheduling) — avoids constructing a private FilterSpec.
    #[test]
    fn dispatch_filter_on_read_only_is_rejected() {
        let mut e = crate::editor::Editor::new_from_text("hello\n", None, (40, 6));
        e.active_mut().read_only = true;
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::submit_filter_line(&mut e, "cat", &tx);
        assert!(e.filter_in_flight.is_none(), "no filter scheduled on a read-only buffer");
        assert_eq!(e.status_text(), "buffer is read-only");
    }

    /// A17 T5 (F4 Warning table): the "a filter is already running" blocked-action refusal
    /// is a recoverable Sticky Warning.
    #[test]
    fn dispatch_filter_already_running_is_a_sticky_warning() {
        let mut e = crate::editor::Editor::new_from_text("hello\n", None, (80, 24));
        e.filter_in_flight = Some(CancelFlag::new());
        let spec = FilterSpec {
            argv: vec!["cat".into()],
            shell: false,
            disposition: Disposition::Filter,
            input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10),
            max_output: 1 << 20,
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        dispatch_filter(&mut e, spec, tx);
        assert_eq!(e.status_text(), "a filter is already running");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    #[test]
    fn filter_spec_constructs() {
        let s = FilterSpec {
            argv: vec!["cat".into()],
            shell: false,
            disposition: Disposition::Filter,
            input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10),
            max_output: 1 << 20,
        };
        assert_eq!(s.argv, vec!["cat".to_string()]);
        assert!(matches!(s.disposition, Disposition::Filter));
    }

    #[test]
    fn run_filter_identity_cat() {
        let _gate = spawn_gate_shared();
        let spec = FilterSpec {
            argv: vec!["cat".into()],
            shell: false,
            disposition: Disposition::Filter,
            input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10),
            max_output: 1 << 20,
        };
        let out = run_filter(&spec, "hello\nworld\n".into(), &CancelFlag::new());
        assert!(matches!(out, RunResult::Stdout(ref s) if s == "hello\nworld\n"));
    }

    #[test]
    fn run_filter_transform_tr() {
        let _gate = spawn_gate_shared();
        let spec = FilterSpec {
            argv: vec!["tr".into(), "a-z".into(), "A-Z".into()],
            shell: false,
            disposition: Disposition::Filter,
            input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10),
            max_output: 1 << 20,
        };
        let out = run_filter(&spec, "abc\n".into(), &CancelFlag::new());
        assert!(matches!(out, RunResult::Stdout(ref s) if s == "ABC\n"));
    }

    #[test]
    fn run_filter_non_zero_exit_carries_stderr() {
        let _gate = spawn_gate_shared();
        let spec = FilterSpec {
            argv: vec![
                "sh".into(),
                "-c".into(),
                "echo boom >&2; exit 3".into(),
            ],
            shell: false,
            disposition: Disposition::Filter,
            input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10),
            max_output: 1 << 20,
        };
        match run_filter(&spec, "x\n".into(), &CancelFlag::new()) {
            RunResult::Err(FilterError::NonZero { code, stderr }) => {
                assert!(code.contains('3'));
                assert!(stderr.contains("boom"));
            }
            other => panic!("expected NonZero, got {other:?}"),
        }
    }

    /// EPIPE regression (spec §1.1): a child that exits after reading part of stdin is ORDINARY
    /// Unix filter semantics. Its output must survive. Before the fix the communicator's stdin
    /// write raced the child's exit and returned Err(Spawn("Broken pipe (os error 32)")).
    #[test]
    #[cfg(unix)]
    fn early_exiting_child_keeps_its_output() {
        let _gate = spawn_gate_shared();
        let spec = spec_for(&["head", "-c", "5"], 10);
        match run_filter(&spec, big_stdin(), &CancelFlag::new()) {
            RunResult::Stdout(s) => assert_eq!(s, "xxxxx",
                "an early-exiting filter's captured output must survive EPIPE on stdin"),
            other => panic!("expected Stdout, got {other:?}"),
        }
    }

    /// EPIPE regression: the child's REAL exit status and stderr must survive, not be replaced by
    /// a Spawn error. This is the hardened form of `run_filter_non_zero_exit_carries_stderr`,
    /// which lost this race ~39% of the time under six-way parallel load.
    #[test]
    #[cfg(unix)]
    fn early_exiting_child_reports_its_real_nonzero_status() {
        let _gate = spawn_gate_shared();
        let spec = spec_for(&["sh", "-c", "head -c 4 >/dev/null; echo boom >&2; exit 3"], 10);
        match run_filter(&spec, big_stdin(), &CancelFlag::new()) {
            RunResult::Err(FilterError::NonZero { code, stderr }) => {
                assert!(code.contains('3'), "the child's real exit code, not a Spawn error: {code}");
                assert!(stderr.contains("boom"), "stderr survives the EPIPE: {stderr}");
            }
            other => panic!("expected NonZero, got {other:?}"),
        }
    }

    /// EPIPE regression: a child that never reads stdin at all still succeeds.
    ///
    /// `s.is_empty()` alone is satisfiable vacuously by any implementation that returns empty
    /// output for the WRONG reason, so we also pin the property this test is really about: the
    /// call must return promptly, not merely succeed after riding out (a portion of) the 10s
    /// timeout budget.
    #[test]
    #[cfg(unix)]
    fn child_that_never_reads_stdin_still_succeeds() {
        let _gate = spawn_gate_shared();
        let spec = spec_for(&["sh", "-c", "exit 0"], 10);
        let start = std::time::Instant::now();
        match run_filter(&spec, big_stdin(), &CancelFlag::new()) {
            RunResult::Stdout(s) => assert!(s.is_empty(), "no output expected, got {s:?}"),
            other => panic!("expected empty Stdout, got {other:?}"),
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(2),
            "a child that never reads stdin must return promptly ({:?} elapsed), not merely \
             succeed after riding out the timeout", start.elapsed());
    }

    /// EPIPE regression: bytes the child wrote BEFORE exiting stay readable in the pipe until
    /// EOF, so the drain must collect them rather than discarding them with the EPIPE.
    #[test]
    #[cfg(unix)]
    fn output_buffered_before_child_exit_is_not_lost() {
        let _gate = spawn_gate_shared();
        let spec = spec_for(&["sh", "-c", "echo out; exit 0"], 10);
        match run_filter(&spec, big_stdin(), &CancelFlag::new()) {
            RunResult::Stdout(s) => assert_eq!(s, "out\n"),
            other => panic!("expected Stdout, got {other:?}"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn run_filter_rejects_oversized() {
        let _gate = spawn_gate_shared();
        // `yes` floods stdout; a tiny cap must abort with TooLarge.
        let spec = FilterSpec {
            argv: vec!["yes".into()],
            shell: false,
            disposition: Disposition::Filter,
            input: Input::None,
            timeout: std::time::Duration::from_secs(5),
            max_output: 64,
        };
        assert!(matches!(
            run_filter(&spec, String::new(), &CancelFlag::new()),
            RunResult::Err(FilterError::TooLarge)
        ));
    }

    #[test]
    #[cfg(unix)]
    fn run_filter_large_stderr_does_not_deadlock() {
        // Regression (Codex pre-merge gate): a child that floods STDERR (filling
        // the OS pipe buffer, ~64 KiB) with little/no stdout used to hang the
        // engine forever.  The crate's size_limit counts COMBINED stdout+stderr
        // and `read()` returns Ok on the size_limit break (not EOF); the old
        // code budgeted/checked the cap on stdout alone, so the stderr flood
        // never tripped TooLarge — it broke to `child.wait()` while the child was
        // still blocked writing stderr to a pipe we'd stopped draining.  The fix
        // accounts for combined captured bytes, so the flood trips TooLarge and
        // kills the child.  We run on a worker thread and assert it RETURNS
        // (a regression would time out here rather than wedge the whole suite).
        //
        // Takes the gate SHARED, like every other spawning test — H30's fd-inheritance race
        // needs a foreign child to outlive a victim's timeout, and this one is killed as soon as
        // combined output crosses `max_output`, well inside any victim's window.
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _gate = spawn_gate_shared();
            let spec = FilterSpec {
                // ~200 KiB to stderr, far past the 64 KiB pipe buffer, tiny stdout.
                argv: vec![
                    "sh".into(),
                    "-c".into(),
                    "head -c 200000 /dev/zero | tr '\\0' 'x' 1>&2".into(),
                ],
                shell: false,
                disposition: Disposition::Filter,
                input: Input::None,
                timeout: std::time::Duration::from_secs(10),
                max_output: 64,
            };
            let out = run_filter(&spec, String::new(), &CancelFlag::new());
            let _ = tx.send(out);
        });
        let out = rx
            .recv_timeout(std::time::Duration::from_secs(8))
            .expect("run_filter must return promptly (no deadlock) on a large-stderr child");
        assert!(
            matches!(out, RunResult::Err(FilterError::TooLarge)),
            "large stderr beyond the cap should be TooLarge, got {out:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn filter_output_above_old_1mib_cap_succeeds_under_new_cap() {
        let _gate = spawn_gate_shared();
        // Emit ~2 MiB through `cat`; with MAX_FILTER_OUTPUT (64 MiB) this must NOT hit the cap.
        let input = "x".repeat(2 * 1024 * 1024);
        let expected_len = input.len();
        let spec = FilterSpec {
            argv: vec!["cat".into()],
            shell: false,
            disposition: Disposition::Filter,
            input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10),
            max_output: crate::limits::MAX_FILTER_OUTPUT,
        };
        match run_filter(&spec, input, &CancelFlag::new()) {
            RunResult::Stdout(ref s) => assert_eq!(s.len(), expected_len),
            other => panic!("2 MiB output must succeed under the 64 MiB cap, got {other:?}"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn run_filter_rejects_non_utf8() {
        let _gate = spawn_gate_shared();
        let spec = FilterSpec {
            argv: vec!["printf".into(), "\\xff".into()],
            shell: false,
            disposition: Disposition::Filter,
            input: Input::None,
            timeout: std::time::Duration::from_secs(10),
            max_output: 1 << 20,
        };
        assert!(matches!(
            run_filter(&spec, String::new(), &CancelFlag::new()),
            RunResult::Err(FilterError::NotUtf8)
        ));
    }

    #[test]
    fn run_filter_missing_binary_is_spawn_error() {
        let _gate = spawn_gate_shared();
        let spec = FilterSpec {
            argv: vec!["wcartel-no-such-binary-xyz".into()],
            shell: false,
            disposition: Disposition::Filter,
            input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10),
            max_output: 1 << 20,
        };
        assert!(matches!(
            run_filter(&spec, "x".into(), &CancelFlag::new()),
            RunResult::Err(FilterError::Spawn(_))
        ));
    }

    #[test]
    #[cfg(unix)]
    fn shell_pipeline_survives_quoted_whitespace() {
        let _gate = spawn_gate_shared();
        // shell: true — a single verbatim-line argv element, run through `sh -c`.
        // A plain pipeline works:
        let pipeline = FilterSpec {
            argv: vec!["tr a-z A-Z".into()],
            shell: true,
            disposition: Disposition::Filter,
            input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10),
            max_output: crate::limits::MAX_FILTER_OUTPUT,
        };
        let out = run_filter(&pipeline, "ab\n".into(), &CancelFlag::new());
        assert!(matches!(out, RunResult::Stdout(ref s) if s == "AB\n"), "{out:?}");

        // A quoted double space inside the program must survive verbatim — splitting
        // the line on whitespace and rejoining would collapse it and break the sed
        // program (the whole point of running through sh -c with a single argv elem).
        let quoted = FilterSpec {
            argv: vec!["sed 's/a  b/c/'".into()],
            shell: true,
            disposition: Disposition::Filter,
            input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10),
            max_output: crate::limits::MAX_FILTER_OUTPUT,
        };
        let out = run_filter(&quoted, "a  b\n".into(), &CancelFlag::new());
        assert!(matches!(out, RunResult::Stdout(ref s) if s == "c\n"), "{out:?}");
    }

    /// Important-1 (task-1 review): `ReapGuard::drop`'s terminate → kill → bounded-reap path was
    /// entirely untested — mutation-proven by replacing the whole `Drop` body with a bare
    /// `detach()` and observing every filter test, EPIPE regressions included, still pass. That
    /// blind spot exists BECAUSE every early-return branch in `run_subprocess` (cancel, timeout,
    /// too-large) already calls its own `terminate()`/`kill()` before returning, so driving the
    /// guard only through the public API can never isolate its own kill/reap logic — the
    /// explicit calls mask the difference. This test drives `ReapGuard` directly against a child
    /// that ignores SIGTERM, so `terminate()` alone cannot touch it and the guard's `kill()` +
    /// bounded-reap path is the ONLY thing capable of ending the child.
    ///
    /// `sh`'s `trap` behaviour is a platform fact, not a language guarantee, so we verify it
    /// (rather than assume it) with a probe child before relying on it for the real assertion.
    #[test]
    #[cfg(target_os = "linux")]
    fn reap_guard_drop_kills_and_reaps_a_sigterm_ignoring_child() {
        // Shared, not exclusive: this test spawns, but neither child is a long-lived holder.
        // The real child (under `ReapGuard`, below) is killed and reaped by the code under
        // test. The probe child is killed and reaped in the ordinary case, but has an explicit
        // fallback — if kill() plus a 500ms wait_timeout hasn't reaped it, `probe.detach()`
        // (below) leaves it unreaped rather than block this test on a possibly-stuck reap. That
        // pathological case is vanishingly unlikely, and even then it does not threaten a
        // concurrent victim: kill() is SIGKILL against our own live child, so it cannot
        // meaningfully fail — the process is dead either way — and a failed reap only leaves a
        // ZOMBIE, whose fds were already closed at death. A zombie can't hold any victim's pipe
        // ends open, which — not timeout arithmetic (a live 30 s process would in fact outlast a
        // 10 s victim timeout) — is why SHARED remains correct.
        let _gate = spawn_gate_shared();
        use subprocess::{Popen, PopenConfig, Redirection};

        fn spawn_sigterm_ignoring_child() -> Popen {
            Popen::create(
                &["sh", "-c", "trap '' TERM; exec sleep 30"],
                PopenConfig {
                    stdin: Redirection::None,
                    stdout: Redirection::None,
                    stderr: Redirection::None,
                    ..Default::default()
                },
            )
            .expect("spawn a SIGTERM-ignoring probe/test child")
        }

        // ---- Platform-fact probe: confirm SIGTERM alone cannot end this child on THIS machine.
        let mut probe = spawn_sigterm_ignoring_child();
        std::thread::sleep(std::time::Duration::from_millis(100)); // let the trap install + exec
        let _ = probe.terminate();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let probe_survived_term = probe.poll().is_none();
        // Clean up the probe unconditionally, regardless of what the assert below does.
        let _ = probe.kill();
        let _ = probe.wait_timeout(std::time::Duration::from_millis(500));
        if probe.poll().is_none() {
            probe.detach();
        }
        assert!(probe_survived_term,
            "probe child exited after SIGTERM alone — this machine's `sh` does not preserve an \
             ignored SIGTERM across `exec`, so this test's premise does not hold here");

        // ---- The real assertion: ReapGuard's OWN kill()+bounded-reap path, not the loop's.
        let child = spawn_sigterm_ignoring_child();
        let pid = child.pid().expect("a freshly spawned child has a pid");
        std::thread::sleep(std::time::Duration::from_millis(100));

        let start = std::time::Instant::now();
        { let _guard = ReapGuard(child); } // drop here — nothing else in this test signals it
        let elapsed = start.elapsed();

        assert!(elapsed < std::time::Duration::from_secs(2),
            "ReapGuard::drop must return promptly even against a SIGTERM-ignoring child (took \
             {elapsed:?}) — a regression toward an unbounded wait() would hang here for ~30s");

        // Give the kernel a brief moment to finish the teardown the drop initiated, then confirm
        // the child is actually GONE (killed and reaped) — not merely detached-and-still-alive,
        // which a bare `detach()` would also leave passing the wall-clock assertion above.
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!crate::swap::pid_is_live(pid),
            "child pid {pid} is still alive after ReapGuard::drop — a bare `detach()` never \
             sends kill() at all, so the child would keep running its full 30s sleep, undetected \
             by every other test in this suite");
    }

    #[test]
    fn guarded_filter_maps_panic_to_runresult_err() {
        let r = guarded_filter(|| panic!("flt"));
        assert!(matches!(r, RunResult::Err(FilterError::Panicked(ref m)) if m == "flt"));
    }

    #[test]
    fn describe_error_renders_panicked() {
        assert!(describe_error(&FilterError::Panicked("x".into())).to_lowercase().contains("internal"));
    }

    /// Timeout must stay enforced after stdout/stderr hit EOF. Moving stdin to a writer thread
    /// also moved it out of the drain loop's protection, so a child that closes its outputs and
    /// keeps running would block a plain `wait()` forever with nothing watching the deadline.
    /// Phase 2 is what prevents that. Runs on a worker thread so a regression times out the
    /// harness instead of wedging the whole suite.
    ///
    /// FAIL-VERIFY: replace the phase-2 loop with `guard.0.wait().unwrap_or(ExitStatus::Undetermined)`,
    /// watch this blow its 15s bound (the child sleeps 600s), then revert.
    ///
    /// The 15s bound below funds gate acquisition AND execution together — unlike T6
    /// (`cancel_is_honoured_after_the_child_closes_its_outputs`), which budgets them separately
    /// via a `ready_tx` handshake. Deliberate, not an oversight: this test has no second signal
    /// racing the spawn (T6's problem), so slow acquisition can only delay it, never let it pass
    /// via the wrong code path, and headroom is enormous today (the whole gated suite is ~1.9s).
    #[test]
    #[cfg(unix)]
    fn timeout_fires_when_a_child_closes_its_outputs_and_keeps_running() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _gate = spawn_gate_exclusive();  // H30 — see SPAWN_GATE
            let spec = spec_for(&["sh", "-c", "exec >/dev/null 2>/dev/null; sleep 600"], 1);
            let _ = tx.send(run_filter(&spec, big_stdin(), &CancelFlag::new()));
        });
        let out = rx.recv_timeout(std::time::Duration::from_secs(15))
            .expect("run_filter must return at its deadline, not block on a child that closed its streams");
        assert!(matches!(out, RunResult::Err(FilterError::Timeout)), "expected Timeout, got {out:?}");
    }

    /// Esc must still work on a child that has stopped talking but not died — the cancel check
    /// has to survive into phase 2, not stop at stdout/stderr EOF.
    ///
    /// FAIL-VERIFY: delete the `cancel.is_cancelled()` arm from the phase-2 loop, watch this fall
    /// back to the 60s timeout and blow its 15s bound, then revert.
    #[test]
    #[cfg(unix)]
    fn cancel_is_honoured_after_the_child_closes_its_outputs() {
        use std::sync::mpsc;
        let cancel = CancelFlag::new();
        let worker_flag = cancel.clone();
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _gate = spawn_gate_exclusive();  // H30 — see SPAWN_GATE
            let _ = ready_tx.send(());
            let spec = spec_for(&["sh", "-c", "exec >/dev/null 2>/dev/null; sleep 600"], 60);
            let _ = tx.send(run_filter(&spec, big_stdin(), &worker_flag));
        });
        // Start the 200 ms clock only once the worker actually HOLDS the gate. Otherwise a slow
        // acquisition could let `cancel()` fire before `run_filter` even spawns, and the test
        // would pass via the PHASE-1 cancel check without ever exercising phase 2 — the thing it
        // names. (The gate is the only reason this handshake is needed; it goes with the gate.)
        ready_rx.recv_timeout(std::time::Duration::from_secs(15))
            .expect("worker must acquire the spawn gate");
        std::thread::sleep(std::time::Duration::from_millis(200));
        cancel.cancel();
        let out = rx.recv_timeout(std::time::Duration::from_secs(15))
            .expect("cancel must be honoured after stdout/stderr EOF, not wait for the timeout");
        assert!(matches!(out, RunResult::Err(FilterError::Cancelled)), "expected Cancelled, got {out:?}");
    }

    /// The SUCCESS path must not block when a DESCENDANT inherits stdin. Reaping the direct child
    /// says nothing about its children: here the shell exits 0 while a backgrounded `sleep` holds
    /// the stdin pipe open, so the writer stays blocked in `write_all`. Joining that writer would
    /// hang here — after every timeout and cancel check has already stopped.
    ///
    /// The `exec 3<&0` … `<&3` shape is REQUIRED and verified by probe (see the task's probe
    /// step): without the explicit `<&3`, bash hands a background job /dev/null for fd 0, nothing
    /// holds the pipe, and this test silently passes against a broken implementation.
    /// `sleep 30` (not 600) so a soak run does not strand long-lived processes.
    ///
    /// FAIL-VERIFY: add `let _ = _writer.map(|w| w.join());` before the terminal `match`, watch
    /// this blow its 20s bound, then revert.
    ///
    /// Like T5, the 20s bound below funds gate acquisition AND execution together rather than a
    /// separate budget as in T6 — deliberate for the same reason: no second signal races this
    /// test's spawn, so acquisition latency can only delay it, never reorder which code path it
    /// takes, and today's headroom is enormous (the whole gated suite is ~1.9s).
    #[test]
    #[cfg(unix)]
    fn success_returns_promptly_when_a_descendant_inherits_stdin() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Acquired BEFORE the clock starts, so waiting for the gate can never be charged to
            // the `elapsed < 5s` assertion below.
            let _gate = spawn_gate_exclusive();  // H30 — see SPAWN_GATE
            let spec = spec_for(
                &["sh", "-c", "exec 3<&0; exec >/dev/null 2>/dev/null; sleep 30 <&3 & exit 0"],
                10,
            );
            let started = std::time::Instant::now();
            let out = run_filter(&spec, big_stdin(), &CancelFlag::new());
            let _ = tx.send((out, started.elapsed()));
        });
        let (out, elapsed) = rx.recv_timeout(std::time::Duration::from_secs(20))
            .expect("must not block on a descendant holding stdin open");
        match out {
            RunResult::Stdout(ref s) => assert!(s.is_empty(),
                "the shell's outputs went to /dev/null, so stdout is empty; got {s:?}"),
            other => panic!("expected empty Stdout, got {other:?}"),
        }
        // Load-bearing: without this a future implementation that stalls to the 10s deadline and
        // THEN returns a success would pass the assertion above. Generous against loaded runs.
        assert!(elapsed < std::time::Duration::from_secs(5),
            "must return on the child's exit, not stall to the deadline; took {elapsed:?}");
    }
}
