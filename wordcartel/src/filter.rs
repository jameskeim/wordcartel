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

/// Spawn a subprocess, feed `stdin`, collect stdout bytes, enforce `timeout`
/// and `max_output`, respect `cancel`.
///
/// ## API path chosen: poll-loop with per-iteration `limit_time`
///
/// The `subprocess` crate's `Communicator` supports `limit_time(d).read()`
/// which returns `Err(CommunicateError { error: TimedOut, capture: partial })`
/// on a per-call deadline.  We call this in a tight loop (~50 ms per iteration)
/// rather than a single blocking `communicate()`, so the `CancelFlag` is
/// checked every ~50 ms and can kill the child promptly on Esc.
///
/// Size-cap behaviour: `limit_size(n)` makes `read()` return Ok once `n` bytes
/// have accumulated (it does NOT return an error).  We detect the cap by
/// checking `out_buf.len() >= max_output` after each successful read.
///
/// ## Deadlock safety
///
/// `Communicator` on Unix uses `poll(2)` to multiplex stdin-write and
/// stdout/stderr-read within a single thread, so it never deadlocks regardless
/// of how large the input or output is.  We feed stdin during the same loop
/// that drains stdout, matching the Python `subprocess.communicate()` model.
#[allow(dead_code)] // wired in Task 5
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
    let mut child = match Popen::create(
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

    // Per-iteration poll window.  Short enough that cancel latency < 100 ms;
    // long enough to not burn CPU on well-behaved fast commands.
    const POLL: std::time::Duration = std::time::Duration::from_millis(50);

    let deadline = std::time::Instant::now() + timeout;
    let stdin_bytes = stdin.into_bytes();
    let stdin_opt: Option<Vec<u8>> = if stdin_bytes.is_empty() {
        // subprocess panics if input_data is None but stdin was redirected to Pipe.
        // Passing Some(vec![]) closes stdin immediately after opening.
        Some(vec![])
    } else {
        Some(stdin_bytes)
    };

    // Communicator takes ownership of the stdin bytes on first call; on timeout
    // it returns partial data and we resume by calling read() again.
    let mut comm = child.communicate_start(stdin_opt);
    let mut out_buf: Vec<u8> = Vec::new();
    let mut err_buf: Vec<u8> = Vec::new();

    loop {
        // Check cancel first — Esc must kill the child within one POLL interval.
        if cancel.is_cancelled() {
            let _ = child.terminate();
            let _ = child.kill();
            return Err(FilterError::Cancelled);
        }
        // Check overall deadline.
        if std::time::Instant::now() >= deadline {
            let _ = child.terminate();
            let _ = child.kill();
            return Err(FilterError::Timeout);
        }

        // Remaining budget for this iteration (cap to POLL).
        let iter_time = POLL.min(deadline.saturating_duration_since(std::time::Instant::now()));

        // Ask for at most (max_output - out_buf.len() + 1) bytes so limit_size
        // will trip at the right threshold.  The +1 ensures we see one byte past
        // the cap so we can distinguish "exactly max_output bytes of output" from
        // "limit hit with more pending".  We treat > max_output as TooLarge.
        let remaining_cap = max_output.saturating_sub(out_buf.len()) + 1;

        // Reassign comm with per-iteration limits; limit_time/limit_size take
        // self by value and return a new Communicator with the fields set, so
        // we must rebind rather than chain inline (which would move comm out of
        // the loop variable).
        comm = comm.limit_time(iter_time).limit_size(remaining_cap);
        match comm.read() {
            Ok((o, e)) => {
                // EOF on both streams — child has closed stdout and stderr.
                if let Some(o) = o {
                    out_buf.extend_from_slice(&o);
                }
                if let Some(e) = e {
                    err_buf.extend_from_slice(&e);
                }
                // Size check after accumulating this batch.
                if out_buf.len() > max_output {
                    let _ = child.terminate();
                    let _ = child.kill();
                    return Err(FilterError::TooLarge);
                }
                // Streams are closed; fall through to wait.
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

                // Size cap — limit_size hit, or we accumulated too much.
                if out_buf.len() > max_output {
                    let _ = child.terminate();
                    let _ = child.kill();
                    return Err(FilterError::TooLarge);
                }

                if ce.error.kind() == std::io::ErrorKind::TimedOut {
                    // Per-iteration timeout expired — loop to check cancel/deadline.
                    continue;
                } else {
                    // Unexpected I/O error.
                    let _ = child.terminate();
                    let _ = child.kill();
                    return Err(FilterError::Spawn(ce.error.to_string()));
                }
            }
        }
    }

    // Wait for the child to exit (it has already closed its streams).
    let status = child.wait().unwrap_or(ExitStatus::Undetermined);
    let stderr_str =
        String::from_utf8_lossy(&err_buf).into_owned();

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
    }
}

pub fn dispatch_filter(
    editor: &mut crate::editor::Editor,
    spec: FilterSpec,
    msg_tx: std::sync::mpsc::Sender<crate::app::Msg>,
) {
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
    let snapshot = b.document.buffer.snapshot();
    let cancel = CancelFlag::new();
    editor.filter_in_flight = Some(cancel.clone());
    editor.status = format!("running {} ...", spec.argv.first().cloned().unwrap_or_default());
    let disposition = spec.disposition.clone();
    let range_c = range.clone();
    std::thread::spawn(move || {
        let stdin = match spec.input {
            Input::None => String::new(),
            _ => snapshot.byte_slice(range_c.clone()).to_string(),
        };
        let outcome = run_filter(&spec, stdin, &cancel);
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

    #[test]
    #[cfg(unix)]
    fn run_filter_rejects_oversized() {
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
    fn run_filter_rejects_non_utf8() {
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
}
