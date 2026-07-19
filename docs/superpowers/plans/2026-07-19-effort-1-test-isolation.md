# Effort ① — implementation plan: `run_subprocess` EPIPE fix + test isolation

**Spec:** `docs/superpowers/specs/2026-07-19-effort-1-test-isolation-design.md` (committed `1837c75`).
**Branch:** `effort-1-test-isolation` off `main`.
**Date:** 2026-07-19.

## Goal

Two independent outcomes, both merged on one branch:

1. **Fix a production bug.** `filter::run_subprocess` treats `BrokenPipe` on stdin as fatal —
   killing the child and discarding already-captured output — so an ordinary early-exiting filter
   (`head -1`, `grep -q`, `sed 3q`) can lose its result. Measured 39% failure under six-way
   parallel load, 0/300 isolated.
2. **Close the test-isolation class.** Stop the test suite writing the developer's real
   `$XDG_STATE_HOME/wordcartel` (including a 64 MiB write and three unrestored `session.toml`
   overwrites), fix the one demonstrated in-process-global flake (`recovery::LAST_GOOD`), and
   move two fixed `/tmp` paths to real tempdirs.

## Architecture

**`run_subprocess` becomes two phases plus an RAII guard.** stdin leaves the `subprocess`
crate's `Communicator` (where an EPIPE is unresumable and fatal) and moves to a dedicated writer
thread where an EPIPE is *ordinary Unix filter semantics* and is ignored. The poll loop then only
drains stdout/stderr (**phase 1**), and a second bounded loop reaps the child under the *same*
cancel/deadline guard (**phase 2**) so timeout and cancel stay enforced from spawn to reap. A
`ReapGuard(Popen)` wrapper guarantees on every exit — including unwind — that `Popen::drop` can
never block. The writer thread is **never joined**.

**Test isolation uses a chokepoint redirect, not per-test seams.** `swap::state_dir()` is the
single resolution point for the state dir; under `#[cfg(test)]` it resolves to a per-process temp
base, so every current and future lib-test caller is redirected at once. `recovery::LAST_GOOD`
gets a serialization gate with a same-thread bypass (a plain lock is self-defeating — see Task 4).

## Tech stack

Rust 2021, `wordcartel` shell crate only (`wordcartel-core` untouched). `subprocess 0.2.15`
(pinned in `Cargo.lock`). `tempfile 3` and `proptest 1` are the only dev-dependencies; **no new
dependency is added by this effort.** Tests are `#[cfg(test)] mod tests` co-located in `src/`.

## Global constraints

- **House style, hand-formatted. NEVER run `cargo fmt`** — the repo has no `rustfmt.toml` and
  `cargo fmt` reflows the whole tree. Match surrounding code by hand; do not reflow lines you did
  not otherwise change. Em-dash `—` in prose comments, never `--`. No emoji.
- **Per-task gates, all must pass before commit:**
  - `cargo test --workspace` green
  - `cargo clippy --workspace --all-targets` clean (workspace `clippy::all = "deny"`)
  - `cargo build` and `cargo test --no-run` warning-free
  - `clippy::too_many_lines` threshold is 100 — split rather than exceed, or carry an item-local
    `#[allow(clippy::too_many_lines)]` with a one-line reason.
- **There is no CI.** Every gate runs only because you run it. Paste the command output in your
  task report; do not write "CI will catch it."
- **Validation is at DEFAULT threading.** This machine has 32 cores. Both target flakes appear
  only at `--test-threads` ≥ 32 and never at 1 or 4. **A fix verified with `--test-threads=1`
  proves nothing.** Never add a thread-count flag to make a test pass.
- **Commit trailers, verbatim, on every commit:**
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG
  ```
- **Command-surface contract** (`docs/design/command-surface-contract.md`): **N/A — this effort
  does not touch the command surface.** No command, option, palette entry, menu row, or keybinding
  changes. State this in your task report.
- **Trust `cargo`, not editor diagnostics.** For compile/usage/signature questions on code you are
  editing, trust `cargo build` / `cargo test` / `grep` — not an editor "unused"/"undefined" hint,
  which lags edits.

## Sequencing decision

**Order: EPIPE work (Tasks 1–2) → isolation work (Tasks 3–6) → validation (Task 7).**

The two halves are genuinely independent — the filter tests never call `state_dir()`, and the
isolation work never touches `filter.rs`. EPIPE goes first because it is the user-facing
production bug, and because `filter::tests::run_filter_non_zero_exit_carries_stderr` is one of the
two metrics the Task 7 soak measures; fixing it first means the final soak measures both fixes in
their finished state.

**Within the isolation work, the `state_dir` redirect (Task 3) goes before the `LAST_GOOD` gate
(Task 4)**, and the analysis behind that is worth stating because the plan was asked to work it
out rather than leave it implicit. The redirect *does* change what other tests see:

- Tests that **scan** the state dir (`swap::tests::enumerator_scan_includes_discard_silently_excludes_prompt`,
  `kept_recoverable_count_reports_what_the_sweep_deliberately_spares`,
  `prompts::tests::the_clean_recovery_modal_names_kept_recoverable_files`) currently see ambient
  litter — orphans from prior real sessions and from other tests. After the redirect they see
  only same-process litter. All three already use **containment** assertions (`contains` /
  `!contains`, never exact counts) precisely because of that litter, so they get strictly more
  reliable, not less. The modal test's `KEPT_SHOWN`-elision workaround (it stamps `ts_ms` = now so
  its fixture is not sorted below a dozen ambient orphans) keeps working and matters less.
- Tests that **assert on the returned dir** (`state_dir_is_0700`,
  `swap_path_named_is_deterministic_and_in_state_dir`) stay valid: the redirect changes only the
  *base*, and the same `create_dir_all` + `chmod 0700` body runs on it, so the mode assertion and
  the `starts_with(state_dir())` assertion are both self-consistent.
- No test depends on ambient litter existing.

So the redirect cannot break Task 4's work, but Task 4 (which edits `recovery::record_snapshot`,
called by *every* editing test in the crate) has the broadest blast radius of anything here.
Landing the redirect first means that if the Task 7 soak shows a surprise, the narrower change is
already isolated in its own commit and the broad one is the obvious suspect.

Tasks 5 and 6 are independent of everything and could land anywhere; they are last among the
isolation tasks because they are trivial.

## Task count: 7

---

## Task 1 — `run_subprocess`: writer thread, two-phase loop, `ReapGuard`

### Files
- `wordcartel/src/filter.rs` (modify: module-level consts + two new private items + rewrite of
  `run_subprocess`'s body; add 4 tests to the existing `mod tests`)

### Interfaces

**Consumes** (existing, unchanged — do not alter these signatures):
```rust
pub fn run_subprocess(
    argv: &[String],
    shell: bool,
    stdin: String,
    timeout: std::time::Duration,
    max_output: usize,
    cancel: &CancelFlag,
) -> Result<Vec<u8>, FilterError>
```
Callers that must keep compiling unchanged: `filter::run_filter` (same file) and two sites in
`wordcartel/src/export.rs` (inside `run_export`'s `match &sink` arms).

From `subprocess 0.2.15` (verified against the pinned source — do not assume other APIs exist):
```rust
pub struct Popen { pub stdin: Option<std::fs::File>, /* … */ }
impl Popen {
    pub fn create(argv: &[impl AsRef<OsStr>], config: PopenConfig) -> subprocess::Result<Popen>;
    pub fn communicate_start(&mut self, input_data: Option<Vec<u8>>) -> Communicator;
    pub fn poll(&mut self) -> Option<ExitStatus>;               // guaranteed not to block
    pub fn wait_timeout(&mut self, dur: Duration) -> subprocess::Result<Option<ExitStatus>>;
    pub fn terminate(&mut self) -> std::io::Result<()>;
    pub fn kill(&mut self) -> std::io::Result<()>;
    pub fn detach(&mut self);                                    // sets the flag `Drop` tests
}
impl Communicator {
    pub fn read(&mut self) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), CommunicateError>;
    pub fn limit_size(self, size: usize) -> Communicator;        // takes self BY VALUE
    pub fn limit_time(self, time: Duration) -> Communicator;     // takes self BY VALUE
}
```
Two facts you will rely on: `Communicator` has **no lifetime parameter** and owns the `File`s
`communicate_start` took, so you may use the `Popen` mutably while the `Communicator` is alive;
and `communicate_start(None)` is legal *only* when `Popen.stdin` is already `None` (the crate
asserts `stdin.is_some() ⇒ input_data.is_some()`).

**Produces** (new, private to `filter.rs`; Task 2 refers to these by name):
```rust
const POLL: std::time::Duration        // 50 ms — moved from inside run_subprocess to module level
const REAP_GRACE: std::time::Duration  // 20 ms
struct ReapGuard(subprocess::Popen);
fn spawn_stdin_writer(stdin: Option<std::fs::File>, bytes: Vec<u8>)
    -> Option<std::thread::JoinHandle<()>>;
```

### REQUIREMENT — `ReapGuard` construction ordering (checkable in review)

The `Popen` returned by a successful `Popen::create` **MUST** be moved into `ReapGuard`
immediately — before `stdin.take()`, before `spawn_stdin_writer`, before `communicate_start`,
before any other statement. A bare `Popen` alive across even one panic-capable statement is a
window in which an unwind bypasses the guard and `Popen::drop` can block. Do not "move the guard
construction down near the loop for readability." The reviewer will check that the statement
immediately following the `Ok(c) => c` binding is the `ReapGuard` construction.

### Steps

**1.1 — Write the four failing tests.** Add to the existing `#[cfg(test)] mod tests` in
`wordcartel/src/filter.rs`, after `run_filter_non_zero_exit_carries_stderr`. Add these two helpers
at the top of the module (immediately after `use super::*;`):

```rust
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
```

Then the four tests:

```rust
    /// EPIPE regression (spec §1.1): a child that exits after reading part of stdin is ORDINARY
    /// Unix filter semantics. Its output must survive. Before the fix the communicator's stdin
    /// write raced the child's exit and returned Err(Spawn("Broken pipe (os error 32)")).
    #[test]
    #[cfg(unix)]
    fn early_exiting_child_keeps_its_output() {
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
    #[test]
    #[cfg(unix)]
    fn child_that_never_reads_stdin_still_succeeds() {
        let spec = spec_for(&["sh", "-c", "exit 0"], 10);
        match run_filter(&spec, big_stdin(), &CancelFlag::new()) {
            RunResult::Stdout(s) => assert!(s.is_empty(), "no output expected, got {s:?}"),
            other => panic!("expected empty Stdout, got {other:?}"),
        }
    }

    /// EPIPE regression: bytes the child wrote BEFORE exiting stay readable in the pipe until
    /// EOF, so the drain must collect them rather than discarding them with the EPIPE.
    #[test]
    #[cfg(unix)]
    fn output_buffered_before_child_exit_is_not_lost() {
        let spec = spec_for(&["sh", "-c", "echo out; exit 0"], 10);
        match run_filter(&spec, big_stdin(), &CancelFlag::new()) {
            RunResult::Stdout(s) => assert_eq!(s, "out\n"),
            other => panic!("expected Stdout, got {other:?}"),
        }
    }
```

**1.2 — Run them and watch them fail.**
```
cargo test -p wordcartel --lib filter::tests:: 2>&1 | tail -40
```
Expect all four to FAIL with `Err(Spawn("Broken pipe (os error 32)"))`. **If any passes, stop and
report** — the scenario is not being created and the rest of this task is unverifiable.

**1.3 — Add the module-level consts and the two new private items.** In `wordcartel/src/filter.rs`,
directly above `run_subprocess`'s doc comment (after the `use std::sync::atomic::…` imports near
`CancelFlag`), add:

```rust
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
```

**1.4 — Rewrite `run_subprocess`'s body.** Replace the whole function (keep the signature). Note
the doc comment is rewritten: the old text claims "We feed stdin during the same loop that drains
stdout", which is now false. Also drop the stale `#[allow(dead_code)] // wired in Task 5` — the
function is `pub` and called by `run_filter` and `export.rs`.

```rust
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

        // Ask for at most (max_output - captured + 1) bytes so limit_size will trip at the right
        // threshold.  CRITICAL: the subprocess crate's `limit_size` counts the COMBINED
        // stdout+stderr bytes of a read() (communicate.rs: `total = outvec.len() + errvec.len()`),
        // so we must budget against the combined captured total — NOT stdout alone.  If we
        // budgeted on stdout only, a child that floods stderr would never trip the cap here,
        // `read()` would return Ok via the size_limit break with a small stdout, and we would
        // break to the reap phase while the child is still blocked writing stderr to a full pipe
        // we stopped draining — deadlocking forever.  The +1 lets us see one byte past the cap so
        // we can distinguish "exactly max_output captured" from "more pending".
        let captured = out_buf.len() + err_buf.len();
        let remaining_cap = max_output.saturating_sub(captured) + 1;

        // limit_time/limit_size take self by value and return a new Communicator, so we rebind
        // rather than chain inline (which would move comm out of the loop variable).
        comm = comm.limit_time(iter_time).limit_size(remaining_cap);
        match comm.read() {
            Ok((o, e)) => {
                // Ok means EITHER both streams hit EOF, OR the combined size_limit was reached
                // mid-stream (the crate breaks its read loop on `total >= size_limit` and returns
                // Ok — it does NOT signal EOF).  The combined-overflow check below distinguishes
                // them: if we are over the cap it was a size_limit break (kill + TooLarge — do NOT
                // reap a child that may still be writing), otherwise it is a genuine EOF.
                if let Some(o) = o {
                    out_buf.extend_from_slice(&o);
                }
                if let Some(e) = e {
                    err_buf.extend_from_slice(&e);
                }
                if out_buf.len() + err_buf.len() > max_output {
                    let _ = guard.0.terminate();
                    let _ = guard.0.kill();
                    return Err(FilterError::TooLarge);
                }
                break; // genuine EOF (both streams closed, under cap) — go reap.
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

                if out_buf.len() + err_buf.len() > max_output {
                    let _ = guard.0.terminate();
                    let _ = guard.0.kill();
                    return Err(FilterError::TooLarge);
                }

                if ce.error.kind() == std::io::ErrorKind::TimedOut {
                    continue; // per-iteration timeout — loop to re-check cancel/deadline.
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
```

**1.5 — Run the new tests green.**
```
cargo test -p wordcartel --lib filter::tests:: 2>&1 | tail -20
```
All four new tests plus every pre-existing `filter::tests::*` must pass. `run_filter_rejects_oversized`
and `run_filter_large_stderr_does_not_deadlock` must be **green without modification** — they are
the combined-size-cap regression guards and this change must not touch their behaviour. If either
fails, the cap reasoning has been broken; stop and report.

**1.6 — Targeted repeat at default threading.** The old flake was load-sensitive, so a single pass
proves little:
```
cargo build -p wordcartel --tests
BIN=$(ls -t target/debug/deps/wordcartel-* | grep -v '\.d$' | head -1)
for i in $(seq 1 30); do $BIN filter:: 2>&1 | grep -E "^test result" ; done | sort | uniq -c
```
Expect 30 identical `ok` lines, zero failures. Paste the `uniq -c` output in your report.

**1.7 — Full gates, then commit.**
```
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build -p wordcartel && cargo test --workspace --no-run
```
Commit message: `fix(filter): drain-then-reap so an early-exiting child keeps its output`, with a
body naming the EPIPE mechanism, and the two trailers verbatim.

### Report must state
- The 1.2 output proving all four tests failed first.
- The 1.6 `uniq -c` output.
- That `ReapGuard` is constructed on the statement immediately after `Ok(c) => c`.
- Command-surface contract: N/A.

---

## Task 2 — hang-regression tests (T5, T6, T7) + the fd-0 probe

### Files
- `wordcartel/src/filter.rs` (modify: add 3 tests to `mod tests`)

### Interfaces

**Consumes** (all created by Task 1 — do not redefine, do not rename):
```rust
fn spec_for(argv: &[&str], timeout_secs: u64) -> FilterSpec   // in filter.rs mod tests
fn big_stdin() -> String                                       // in filter.rs mod tests
```
Plus existing `run_filter`, `RunResult`, `FilterError`, `CancelFlag` from the same module.

**Produces:** three `#[test]` fns, no production code.

### Why these tests exist (read before writing them)

Task 1's four tests all use children that **exit**. Every one of them passes against an
implementation that reaps with a blocking `wait()` or that joins the writer thread. These three
cover the paths where a hang is silent and permanent. They are guard tests for a design that is
already correct, so each carries an explicit **FAIL-VERIFY** step: make the named one-line
regression, watch the test hang and blow its harness bound, then revert.

### REQUIREMENT — re-run the fd-0 probe before trusting T7

T7's shell shape is load-bearing and **platform-dependent**. The plausible-looking variant
`sleep 600 & exit 0` does **not** work: under bash a non-interactive background job takes fd 0
from `/dev/null`, so nothing holds the filter's stdin, the writer gets EPIPE, and a restored
`join()` returns normally — i.e. the test silently passes against a broken implementation. That
exact defect was caught mid-review; do not re-introduce it by reasoning about shell semantics.
**The probe is the acceptance criterion.** Run this on the implementing machine and paste the
output into your report:

```sh
cd "$(mktemp -d)"
probe() {                            # $1 = label, $2 = sh -c script
  F="$PWD/F$1"; mkfifo "$F"
  sleep 15 > "$F" & HOLDER=$!
  sleep 0.2
  sh -c "$2" < "$F" >/dev/null 2>&1 & SHPID=$!
  sleep 0.6
  echo "--- $1"
  if kill -0 $SHPID 2>/dev/null; then echo "    direct child pid=$SHPID STILL ALIVE"
  else wait $SHPID 2>/dev/null; echo "    direct child pid=$SHPID EXITED rc=$?"; fi
  found=0
  for p in /proc/[0-9]*; do
    pid=${p#/proc/}; [ "$pid" = "$HOLDER" ] && continue
    if [ "$(readlink $p/fd/0 2>/dev/null)" = "$F" ]; then
      found=1
      echo "    HOLDS STDIN: pid=$pid cmd=$(tr '\0' ' ' < $p/cmdline 2>/dev/null)"
      echo "        fd1=$(readlink $p/fd/1 2>/dev/null)  fd2=$(readlink $p/fd/2 2>/dev/null)"
    fi
  done
  [ $found = 0 ] && echo "    NOBODY holds the stdin object"
  pkill -x sleep 2>/dev/null; sleep 0.3; true
}
probe T5    'exec >/dev/null 2>/dev/null; sleep 600'
probe T7bad 'exec >/dev/null 2>/dev/null; sleep 600 & exit 0'
probe T7    'exec 3<&0; exec >/dev/null 2>/dev/null; sleep 30 <&3 & exit 0'
pkill -x sleep 2>/dev/null; true
```

Required outcomes — **if any differs, stop and report rather than adjusting the assertions**:
- `T5` — direct child **STILL ALIVE**, holds stdin, `fd1`/`fd2` both `/dev/null`.
- `T7bad` — direct child **EXITED rc=0**, **NOBODY holds the stdin object** (this is the trap,
  confirming the probe can tell the shapes apart).
- `T7` — direct child **EXITED rc=0**, and a **different pid** (the grandchild) holds stdin with
  `fd1`/`fd2` both `/dev/null`.

If `/bin/sh` is not bash on the implementing machine, or the holder differs, adjust the argv until
the probe shows the intended holder and report what you changed and why.

### Steps

**2.1 — Write the three tests.** Append to `mod tests` in `wordcartel/src/filter.rs`:

```rust
    /// Timeout must stay enforced after stdout/stderr hit EOF. Moving stdin to a writer thread
    /// also moved it out of the drain loop's protection, so a child that closes its outputs and
    /// keeps running would block a plain `wait()` forever with nothing watching the deadline.
    /// Phase 2 is what prevents that. Runs on a worker thread so a regression times out the
    /// harness instead of wedging the whole suite.
    ///
    /// FAIL-VERIFY: replace the phase-2 loop with `guard.0.wait().unwrap_or(ExitStatus::Undetermined)`,
    /// watch this blow its 15s bound (the child sleeps 600s), then revert.
    #[test]
    #[cfg(unix)]
    fn timeout_fires_when_a_child_closes_its_outputs_and_keeps_running() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
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
        std::thread::spawn(move || {
            let spec = spec_for(&["sh", "-c", "exec >/dev/null 2>/dev/null; sleep 600"], 60);
            let _ = tx.send(run_filter(&spec, big_stdin(), &worker_flag));
        });
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
    #[test]
    #[cfg(unix)]
    fn success_returns_promptly_when_a_descendant_inherits_stdin() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
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
```

**2.2 — Run them green.** `cargo test -p wordcartel --lib filter::tests:: 2>&1 | tail -20`

**2.3 — FAIL-VERIFY each of the three**, one at a time, using the regression named in each test's
doc comment. For each: make the edit, run only that test, confirm it fails on its harness bound,
revert, confirm green again. Paste the three failure lines in your report. **This is the only
evidence that these tests can fail at all** — three previous review rounds turned on tests that
looked right and asserted nothing.

**2.4 — Gates and commit.** Full gates as Task 1.7. Commit:
`test(filter): hang regressions for phase-2 reap and the never-joined writer`.

### Report must state
- The full probe output from the REQUIREMENT block.
- The three FAIL-VERIFY failure lines.

---

## Task 3 — redirect `swap::state_dir()` in test builds

### Files
- `wordcartel/src/swap.rs` (modify: `state_dir` body; add 1 test to `mod tests`)

### Interfaces

**Consumes / Produces** — the signature is unchanged and MUST stay unchanged; ~20 call sites
depend on it:
```rust
pub fn state_dir() -> std::io::Result<std::path::PathBuf>
```

Current body, for reference (replace only the `let base = …` binding):
```rust
pub fn state_dir() -> io::Result<PathBuf> {
    let base = dirs::state_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no state dir"))?;
    let dir = base.join("wordcartel");
    // fs-chokepoint-allow: (b) directory provisioning — the seam's own state dir
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // fs-chokepoint-allow: (b) directory provisioning — chmod the newly-created state dir
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}
```

### Steps

**3.1 — Write the failing guard test.** Add to `#[cfg(test)] mod tests` in `wordcartel/src/swap.rs`,
next to `state_dir_is_0700`:

```rust
    /// Effort ① D5: in test builds `state_dir()` must never resolve the developer's real XDG
    /// state dir. This is the structural half of "a test never touches the user's real files" —
    /// it closes the whole class at the chokepoint, so a test that forgets a seam damages
    /// nothing ambient. A compiled assertion, not a textual scanner: there is nothing to evade.
    ///
    /// Boundary (deliberate, documented on `state_dir`): `cfg(test)` covers this lib test binary
    /// and the in-source `e2e` module, NOT `wordcartel/tests/*` integration binaries, which link
    /// the library without it. No current integration test calls `state_dir`.
    #[test]
    fn state_dir_in_test_builds_is_redirected_off_the_real_xdg_dir() {
        let d = state_dir().expect("state dir resolves in test builds");
        assert!(d.starts_with(std::env::temp_dir()),
            "test builds must resolve under the temp dir, never the real XDG state dir; got {d:?}");
        assert!(d.ends_with("wordcartel"),
            "the wordcartel component is still appended, so path shapes match production: {d:?}");
    }
```

**3.2 — Run it and watch it fail.**
```
cargo test -p wordcartel --lib swap::tests::state_dir_in_test_builds 2>&1 | tail -20
```
Expect FAIL: the returned path is under `$HOME/.local/state` or `$XDG_STATE_HOME`.

**3.3 — Implement the redirect.** Replace only the `let base = …` binding in `state_dir`, and
extend the doc comment. The rest of the body — `create_dir_all`, the `cfg(unix)` chmod, and both
`fs-chokepoint-allow: (b)` markers — stays exactly as it is:

```rust
/// `$XDG_STATE_HOME/wordcartel`, created 0700 on Unix. Falls back to
/// `~/.local/state/wordcartel` when `dirs::state_dir()` is None.
///
/// **In test builds this resolves to a per-process temp dir instead** (Effort ①, D5): every
/// caller — production code reached from a test included — is redirected at this single
/// chokepoint, so no test can write the developer's real session/swap files by forgetting a
/// seam. The provisioning body below is identical in both builds, so assertions about the
/// returned directory (mode 0700, path shape) stay meaningful.
///
/// Boundary: `cfg(test)` applies to the lib test binary and the in-source `e2e` module only.
/// Integration binaries under `wordcartel/tests/` link the library WITHOUT it and would reach
/// the real directory — none does today. The PTY smoke suite drives the real binary against the
/// real directory deliberately; that is where real-state-dir behaviour is proven end-to-end.
pub fn state_dir() -> io::Result<PathBuf> {
    #[cfg(test)]
    let base = std::env::temp_dir().join(format!("wcartel-test-state-{}", std::process::id()));
    #[cfg(not(test))]
    let base = dirs::state_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no state dir"))?;
    let dir = base.join("wordcartel");
    // fs-chokepoint-allow: (b) directory provisioning — the seam's own state dir
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // fs-chokepoint-allow: (b) directory provisioning — chmod the newly-created state dir
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}
```

**3.4 — Run green, and verify the blast radius.** The redirect changes what every state-dir test
sees, so run the affected modules explicitly:
```
cargo test -p wordcartel --lib swap::   2>&1 | tail -20
cargo test -p wordcartel --lib recovery:: 2>&1 | tail -20
cargo test -p wordcartel --lib prompts::  2>&1 | tail -20
cargo test -p wordcartel --lib session_restore:: 2>&1 | tail -20
cargo test -p wordcartel --lib e2e::      2>&1 | tail -20
```
All must pass. Expected, and fine: `state_dir_is_0700` still passes (we create and chmod the
redirected dir ourselves); the enumerator/kept/modal tests still pass because they assert
containment, not exact counts. **If a test fails because it expected ambient litter, stop and
report — do not weaken its assertion.**

**3.5 — Prove the real directory is untouched by a full run.**
```
ls -la --time-style=full-iso ~/.local/state/wordcartel 2>/dev/null | head -5   # before
cargo test --workspace >/dev/null 2>&1
ls -la --time-style=full-iso ~/.local/state/wordcartel 2>/dev/null | head -5   # after
```
Directory mtime and contents must be unchanged across the run. Paste both listings. (If the
directory does not exist, that is a pass — say so.)

**3.6 — Gates and commit.** Full gates as Task 1.7. Commit:
`test(swap): redirect state_dir off the real XDG dir in test builds`.

### Report must state
- The 3.2 failure output.
- The 3.5 before/after listings.
- Confirmation that no existing test's assertion was weakened.

---

## Task 4 — serialize `recovery::LAST_GOOD` for the one asserting test

### Files
- `wordcartel/src/recovery.rs` (modify: add gate items; modify `record_snapshot`)
- `wordcartel/src/editor.rs` (modify: `undo_and_redo_refresh_the_recovery_snapshot` + its comment)

### Interfaces

**Consumes** (existing, unchanged):
```rust
pub static LAST_GOOD: std::sync::Mutex<Option<(Option<std::path::PathBuf>, ropey::Rope)>>;
pub fn record_snapshot(path: Option<&std::path::Path>, rope: ropey::Rope);   // signature unchanged
```
Called unconditionally from `Buffer::apply`, `Buffer::undo`, `Buffer::redo` in `editor.rs`, i.e.
from nearly every editing test in the crate.

**Produces** (new, `#[cfg(test)]`-only, in `recovery.rs`):
```rust
#[cfg(test)] pub(crate) struct SnapshotGate(Option<std::sync::MutexGuard<'static, ()>>);
#[cfg(test)] impl SnapshotGate { pub(crate) fn acquire() -> SnapshotGate; }
```

### Why a plain lock does not work (read before implementing)

The obvious fix — have the asserting test hold `LAST_GOOD` across its edits — is **self-defeating**.
`record_snapshot` writes through `LAST_GOOD.try_lock()` and **silently skips on contention** (a
deliberate property: the panic hook must never deadlock). A test holding that mutex would suppress
its *own* snapshots and then assert on a stale value. Hence a *separate* gate with a same-thread
bypass: other threads block before touching `LAST_GOOD`; the holding thread passes straight
through. Lock order is always gate → `LAST_GOOD`, so there is no cycle.

### Steps

**4.1 — Add the gate to `wordcartel/src/recovery.rs`.** Insert after the `LAST_GOOD` declaration:

```rust
/// Effort ①: serializes `record_snapshot` against the one test that ASSERTS on `LAST_GOOD`.
///
/// Not the `LAST_GOOD` mutex itself: `record_snapshot` writes via `try_lock` and SKIPS on
/// contention (so the panic hook can never deadlock), so a test holding `LAST_GOOD` would
/// suppress its own snapshots. This gate is taken BEFORE `LAST_GOOD` on every path — lock order
/// is gate → LAST_GOOD, never the reverse.
#[cfg(test)]
pub(crate) static SNAPSHOT_GATE: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    /// True while THIS thread holds `SNAPSHOT_GATE`, so its own `record_snapshot` calls bypass
    /// the gate instead of self-deadlocking on a non-reentrant mutex.
    static GATE_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// RAII handle: while alive, every OTHER thread blocks in `record_snapshot` before touching
/// `LAST_GOOD`, and this thread's own snapshots still land normally.
#[cfg(test)]
pub(crate) struct SnapshotGate(Option<std::sync::MutexGuard<'static, ()>>);

#[cfg(test)]
impl SnapshotGate {
    pub(crate) fn acquire() -> SnapshotGate {
        // Poisoning is neutralized deliberately: a panicking gate holder must not cascade into
        // every editing test's `apply`.
        let g = SNAPSHOT_GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        GATE_HELD.with(|c| c.set(true));
        SnapshotGate(Some(g))
    }
}

#[cfg(test)]
impl Drop for SnapshotGate {
    fn drop(&mut self) {
        GATE_HELD.with(|c| c.set(false));
        self.0 = None; // release the gate AFTER clearing the flag
    }
}
```

**4.2 — Route `record_snapshot` through the gate.** Replace the function:

```rust
/// Record the post-edit snapshot (O(1) rope clone). Called from `Editor::apply`.
///
/// In test builds this first passes through `SNAPSHOT_GATE` (unless this thread holds it), so a
/// test asserting on `LAST_GOOD` can serialize every other thread's writes. Production builds
/// compile to exactly the `try_lock` write below.
pub fn record_snapshot(path: Option<&Path>, rope: ropey::Rope) {
    #[cfg(test)]
    let _serial = if GATE_HELD.with(std::cell::Cell::get) {
        None
    } else {
        Some(SNAPSHOT_GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    };
    if let Ok(mut g) = LAST_GOOD.try_lock() {
        *g = Some((path.map(Path::to_path_buf), rope));
    }
}
```

**4.3 — Rewrite the test and its false comment** in `wordcartel/src/editor.rs`. The existing
comment describes a serialization strategy ("taking it, seeding a sentinel, dropping the guard")
that is not in the body and would not work. Replace the whole test:

```rust
    /// Effort ①: this test ASSERTS the value of the process-global `LAST_GOOD`, which every other
    /// editing test writes via `apply`/`undo`/`redo`. Without serialization an unrelated test's
    /// snapshot lands between this test's write and its read (measured: 3/60 runs at default
    /// `--test-threads`, 0/300 isolated — captured values were other tests' buffer text).
    ///
    /// `SnapshotGate` blocks OTHER threads inside `record_snapshot` before they touch `LAST_GOOD`,
    /// while this thread bypasses the gate so its own snapshots still land. Holding `LAST_GOOD`
    /// itself would be self-defeating: `record_snapshot` writes via `try_lock` and SKIPS on
    /// contention, so this test's own snapshots would be dropped and it would assert on a stale
    /// value.
    #[test]
    fn undo_and_redo_refresh_the_recovery_snapshot() {
        let _gate = crate::recovery::SnapshotGate::acquire();
        struct C; impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { 0 } }
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "X".into())], 4);
        e.active_mut().apply(Transaction::new(cs), edit, EditKind::Other, &C); // LAST_GOOD = "Xabc\n"
        e.active_mut().undo();
        let after_undo = crate::recovery::LAST_GOOD.lock().unwrap()
            .as_ref().map(|(_, r)| r.to_string());
        assert_eq!(after_undo.as_deref(), Some("abc\n"), "undo refreshes the recovery snapshot");
        e.active_mut().redo();
        let after_redo = crate::recovery::LAST_GOOD.lock().unwrap()
            .as_ref().map(|(_, r)| r.to_string());
        assert_eq!(after_redo.as_deref(), Some("Xabc\n"), "redo refreshes the recovery snapshot");
    }
```

**4.4 — Soak the specific flake at DEFAULT threading.** A single green run proves nothing: the
baseline failure rate is ~3/60 whole-binary runs.
```
cargo build -p wordcartel --tests
BIN=$(ls -t target/debug/deps/wordcartel-* | grep -v '\.d$' | head -1)
fails=0
for i in $(seq 1 60); do
  $BIN >/tmp/wc-lastgood-$i.log 2>&1 || fails=$((fails+1))
done
echo "failed runs: $fails"
grep -l "undo_and_redo_refresh_the_recovery_snapshot" /tmp/wc-lastgood-*.log | wc -l
```
Both numbers must be **0**. Note `$BIN` with no arguments runs the whole lib binary at default
threads — that is the point; do not add `--test-threads`.

**4.5 — Gates and commit.** Full gates as Task 1.7. Commit:
`test(recovery): serialize LAST_GOOD for the one test that asserts it`.

### Report must state
- The 4.4 counts (both zero) and that no `--test-threads` flag was used.
- Confirmation that `record_snapshot`'s production behaviour is unchanged (the `cfg(test)` block
  compiles out).

---

## Task 5 — move two fixed `/tmp` paths to real tempdirs

### Files
- `wordcartel/src/search_ui.rs` (modify: 1 test)
- `wordcartel/src/diagnostics_run.rs` (modify: 1 test)

### Interfaces

**Consumes:** `tempfile` (already a dev-dependency of `wordcartel`; no `Cargo.toml` change).
```rust
tempfile::tempdir() -> std::io::Result<tempfile::TempDir>
impl TempDir { pub fn path(&self) -> &std::path::Path }   // dir is removed when TempDir drops
```
**Produces:** none.

**Note:** the `TempDir` binding must stay alive for the whole test — it deletes its directory on
drop. Do not write `tempfile::tempdir().unwrap().path().to_path_buf()`.

### Steps

**5.1 — `search_ui.rs`.** In `diag_apply_selected_add_dict_writes_file_once_and_nudges_reload`,
replace these three lines:
```rust
        let dir = format!("/tmp/wordcartel_adddict_{}", std::process::id());
        let _ = std::fs::remove_dir_all(&dir);
        let dict_path = std::path::PathBuf::from(&dir).join("dictionary.txt");
```
with:
```rust
        let dir = tempfile::tempdir().expect("tempdir");
        let dict_path = dir.path().join("dictionary.txt");
```
and delete the trailing cleanup line at the end of that test:
```rust
        let _ = std::fs::remove_dir_all(&dir);
```
(`TempDir` cleans up on drop. Everything between is unchanged.)

**5.2 — `diagnostics_run.rs`.** In `append_word_to_dict_creates_parent_dir`, replace:
```rust
        let temp_dir = format!("/tmp/wordcartel_test_{}", std::process::id());
        let dict_path = std::path::PathBuf::from(&temp_dir)
            .join("subdir")
            .join("nested")
            .join("dictionary.txt");

        // Clean up before test
        let _ = std::fs::remove_dir_all(&temp_dir);
```
with:
```rust
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let dict_path = temp_dir.path()
            .join("subdir")
            .join("nested")
            .join("dictionary.txt");
```
and delete the trailing:
```rust
        // Clean up after test
        let _ = std::fs::remove_dir_all(&temp_dir);
```
The `append_word_to_dict(...)` call and all three assertions are unchanged — the test still proves
parent directories are created, now inside a private dir.

**5.3 — Run green.**
```
cargo test -p wordcartel --lib search_ui::tests::diag_apply_selected_add_dict 2>&1 | tail -10
cargo test -p wordcartel --lib diagnostics_run::tests::append_word_to_dict 2>&1 | tail -10
```

**5.4 — Confirm no fixed path remains.**
```
grep -rn '/tmp/wordcartel_' wordcartel/src/ || echo "clean"
```
Must print `clean`.

**5.5 — Gates and commit.** Full gates as Task 1.7. Commit:
`test: use real tempdirs instead of fixed /tmp paths in two tests`.

---

## Task 6 — correct one false test comment

### Files
- `wordcartel/src/session_restore.rs` (modify: one comment line)

### Interfaces
None. Comment-only change; no behaviour, no signature.

### Context
`file_browser_enter_on_file_opens_it_when_clean` says it simulates Enter, but calls
`open_into_current` directly and never touches picker input. This is the fifth instance of a
false-invariant comment found in this codebase's tests (C5 fixed four). The **comment** is wrong,
not the test — the underlying "is that warning reachable" question is deferred H28 work and is
explicitly **not** in scope here. Do not change the test body or add pumping.

### Steps

**6.1 — Replace this line** in `wordcartel/src/session_restore.rs`:
```rust
        // select "note.md" and simulate Enter via the browser's open path:
```
with:
```rust
        // NOT a simulated Enter: this calls the clean-path open handler DIRECTLY, bypassing the
        // picker's intercept, selection and highlight entirely. It covers `open_into_current`'s
        // clean-buffer behaviour, nothing about Enter dispatch.
```
Leave the trailing `// the clean-path the Enter handler takes` on the `open_into_current` line as
it is — that part is accurate.

**6.2 — Run green.**
```
cargo test -p wordcartel --lib session_restore::tests::file_browser_enter_on_file 2>&1 | tail -10
```

**6.3 — Gates and commit.** Full gates as Task 1.7. Commit:
`docs(test): correct a comment that claimed to simulate Enter`.

---

## Task 7 — soak validation, full gates, smoke

### Files
None modified. This task produces a report only. If it finds a failure, **stop and report** — do
not fix in this task.

### Steps

**7.1 — Full workspace gates.**
```
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build -p wordcartel && cargo test --workspace --no-run
```
All green/clean/warning-free. Paste the `test result:` lines and the clippy summary.

**7.2 — Repro-basis soak, 60× at DEFAULT threading.** This is the exact condition that produced
the baseline failures (3/60 for the editor test, 4/60 for the filter test):
```
cargo build -p wordcartel --tests
BIN=$(ls -t target/debug/deps/wordcartel-* | grep -v '\.d$' | head -1)
fails=0
for i in $(seq 1 60); do
  $BIN >/tmp/wc-soak-$i.log 2>&1 || fails=$((fails+1))
done
echo "failed runs: $fails / 60"
grep -h "^test .* FAILED\|panicked at" /tmp/wc-soak-*.log | sort | uniq -c
```
Required: `failed runs: 0 / 60`, and the grep empty. **No `--test-threads` flag anywhere.**

**7.3 — Contention soak, 6 rounds × 6 concurrent processes.** This is the condition under which
the filter test failed 14/36 (~39%):
```
for r in $(seq 1 6); do
  for p in $(seq 1 6); do $BIN >/tmp/wc-cont-$r-$p.log 2>&1 & done
  wait
done
grep -h "^test .* FAILED\|panicked at" /tmp/wc-cont-*.log | sort | uniq -c
grep -l "FAILED" /tmp/wc-cont-*.log | wc -l
```
Required: zero failures across all 36 runs. Specifically confirm none of these appear:
`filter::tests::run_filter_non_zero_exit_carries_stderr`,
`editor::tests::undo_and_redo_refresh_the_recovery_snapshot`,
`prompts::tests::the_clean_recovery_modal_names_kept_recoverable_files`,
`config::tests::clipboard_provider_unknown_warns_and_defaults_auto`.
(The last two were observed as co-occurring failures in the same contention window during
mapping; they are not this effort's targets, but their absence is worth recording, and their
presence is worth reporting.)

**7.4 — Real state dir untouched.** After all of the above:
```
ls -la --time-style=full-iso ~/.local/state/wordcartel 2>/dev/null || echo "(does not exist — pass)"
```
Nothing may have been created or modified by the test runs.

**7.5 — PTY smoke suite (mandatory-run, advisory-pass).**
```
scripts/smoke/run.sh
```
Quote the one-line summary **verbatim** (e.g. `smoke: 9/9 PASS`, or `smoke: FAIL s3 — advisory`,
or `smoke: SKIP — …`). A red result does **not** block the merge but MUST be surfaced explicitly
as an advisory finding. Note the smoke suite drives the real `wcartel` binary against the **real**
state dir — that is deliberate and correct, and is where real-state-dir behaviour is proven.

**7.6 — Report.** No commit unless a `.history` file changed. Summarize: gate results, both soak
results with counts, the state-dir listing, the verbatim smoke line, and any advisory finding.

---

## Notes for the reviewer

Check these specifically; each is a defect this effort's review rounds actually caught:

1. **`ReapGuard` is constructed immediately after `Popen::create`** — before `stdin.take()`,
   before the writer spawn, before `communicate_start`. Anything else leaves an unwind window.
2. **The writer thread is never joined** on any path. A `join()` anywhere in `run_subprocess` is a
   Critical, not a cleanup improvement.
3. **No bare `child.wait()`** anywhere in `run_subprocess`. The only `wait`-family calls are
   `wait_timeout` and `poll`.
4. **T7's argv contains `exec 3<&0` and `<&3`.** Without them the test creates no scenario and
   passes against a broken implementation. The task report must contain probe output.
5. **The combined-size-cap comment block survives verbatim** and
   `run_filter_large_stderr_does_not_deadlock` / `run_filter_rejects_oversized` pass unmodified.
6. **No `--test-threads` flag** was added to any test invocation to make something pass.
7. **No existing assertion was weakened** to accommodate the `state_dir` redirect.

## History

- 2026-07-19 (rev 1) — Initial plan, written against spec rev 8 (committed `1837c75`). Seven
  tasks. Sequencing: EPIPE (1–2) before isolation (3–6) before validation (7), with the
  redirect-before-`LAST_GOOD` ordering justified by blast radius in the Sequencing section.
  Implementation choices made here rather than deferred to implementers: `REAP_GRACE = 20 ms` and
  `POLL` both lifted to module level (`ReapGuard` needs to reach them); `spawn_stdin_writer`
  returns `Option<JoinHandle>` and closes stdin itself on empty input; T7 uses `sleep 30` rather
  than the spec's `sleep 600` so soak runs do not strand long-lived grandchildren; Task 5 keeps
  the `TempDir` binding alive rather than extracting a `PathBuf`.
