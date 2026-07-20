# Effort ① — `run_subprocess` EPIPE fix + test-isolation class: design spec

**Date:** 2026-07-19. **Author:** Fable (independent grounding pass against `main`, clean tree).
**Inputs:** `scratchpad/effort-1-gates/decisions.md` (human-ratified D1–D5),
`scratchpad/effort-1-gates/map-{flakes,testinfra,h26,h27,h28}.md` (facts-only maps), plus this
author's own source verification of every claim below. Claims are anchored on **symbol names**,
never line numbers.

**Scope (D1):** two things, deliberately not the five backlog items as filed:
1. Fix the `filter::run_subprocess` EPIPE bug — a **production** bug in the shipped `!` filter
   feature (D2), measured at ~39% test failure under six-way parallel load, 0/300 isolated.
2. Fix the test-isolation class (D3/D4/D5): tests that damage the developer's real
   `$XDG_STATE_HOME/wordcartel`, plus the one demonstrated in-process-global flake
   (`recovery::LAST_GOOD`), plus the two fixed `/tmp` paths.

**Decision conformance is summarized in §8.** One decision (D3's "a lock" for `LAST_GOOD`)
required a mechanism refinement forced by verified code facts; it is a design detail within the
ratified choice, not a reversal — see §5.3.

---

## 0. Standing constraints (restated so no reviewer has to infer them)

- **There is no CI.** No workflows, no hooks, no gate scripts. Every gate below runs only because
  a human or agent follows `CLAUDE.md`. Nothing in this spec claims automated enforcement.
- **Merge gates:** `cargo test --workspace` green; `cargo clippy --workspace --all-targets` clean
  (workspace `clippy::all = "deny"`); `cargo build` / `cargo test --no-run` warning-free for
  touched crates; `clippy::too_many_lines` threshold 100 (`clippy.toml`); module budgets
  (`wordcartel/tests/module_budgets.rs` — `filter.rs`, `swap.rs`, `recovery.rs` are not budgeted
  hubs, but new functions must stay under the line threshold or be split).
- **PTY smoke suite:** mandatory-run, advisory-pass. The pre-merge report quotes
  `scripts/smoke/run.sh`'s one-line summary verbatim.
- **Validation at default threading.** This machine has 32 cores; both measured flakes appear only
  at `--test-threads` ≥ 32 and never at 1 or 4 (map-flakes §1e). A fix "verified" in isolation
  proves nothing; §7 defines the required protocol.
- **Formatting:** the tree is hand-formatted; no `cargo fmt`. Match neighbors by hand.
- `wordcartel-core` is `#![forbid(unsafe_code)]`; this effort touches only the `wordcartel` shell
  crate and uses no `unsafe` anywhere.

**Command-surface contract (`docs/design/command-surface-contract.md`):
N/A — this effort does not touch the command surface.** No command, user-settable option,
palette entry, menu row, or keybinding hint is added, removed, or changed. The `run_subprocess`
fix alters the internal subprocess engine behind the existing `!` filter and export commands
without changing their registry entries, arguments, or bindings; the isolation work touches only
tests and `#[cfg(test)]`-gated code plus one internal function body (`swap::state_dir`).

---

## 1. Part 1 — the `run_subprocess` EPIPE bug

### 1.1 Verified root cause

`filter::run_subprocess` (in `wordcartel/src/filter.rs`) spawns via `subprocess::Popen::create`
with all three streams `Redirection::Pipe`, then calls `child.communicate_start(stdin_opt)` and
polls `comm.limit_time(iter_time).limit_size(remaining_cap).read()` in a ~50 ms loop. The
`Err(ce)` arm accumulates `ce.capture` partial data, then:

- `ce.error.kind() == ErrorKind::TimedOut` → `continue` (per-iteration poll timeout, by design);
- **any other kind** → `child.terminate()`, `child.kill()`, `return Err(FilterError::Spawn(...))`
  — discarding `out_buf`/`err_buf` already captured.

A `BrokenPipe` (EPIPE, os error 32) from writing the child's stdin lands in that second arm. So a
filter child that exits before consuming all of stdin — `head -1`, `grep -q`, `sed 3q`, or simply
`sh -c "echo boom >&2; exit 3"` losing the race — is killed and reported as
`Spawn("Broken pipe (os error 32)")`, with its real exit status and captured output thrown away.
All 18 captured failures of `filter::tests::run_filter_non_zero_exit_carries_stderr` in
map-flakes hit exactly this arm (verbatim `expected NonZero, got Err(Spawn("Broken pipe (os
error 32)"))`), at 0/300 isolated → ~7% at default threads → 14/36 (~39%) under six-way
contention. This is user-facing: any `!` filter whose command legitimately stops reading early
can lose its output in real use, not just in tests.

### 1.2 Why an in-loop retry cannot fix it (verified against the vendored crate, `subprocess 0.2.15`)

Facts from `~/.cargo/registry/src/*/subprocess-0.2.15/src/{communicate.rs,popen.rs}`:

1. `Popen::communicate_start` **takes** `self.stdin/stdout/stderr` out of the `Popen`
   (`self.stdin.take()`, …) and moves them into the `Communicator`'s private `RawCommunicator`.
   There is no accessor to reach or drop them afterwards.
2. In the Unix `RawCommunicator::read` loop, a ready stdin does
   `self.stdin.as_ref().unwrap().write(chunk)?` — the `?` propagates EPIPE **without clearing
   `self.stdin` or draining `input_data`**. The write happens *before* the stdout/stderr reads in
   the same iteration.
3. `poll(2)` on a pipe write-end whose read end is closed reports `POLLERR` (and on Linux
   typically `POLLOUT` too). The crate's readiness test is `test(POLLOUT | POLLHUP)`.

Consequences: after one EPIPE, calling `comm.read()` again either (a) sees stdin "ready" again,
re-attempts the write first, and EPIPEs again before ever reading — an EPIPE livelock that burns
the whole `timeout` budget draining nothing — or (b) if the kernel reported only `POLLERR`, stdin
is never "ready", remains `Some`, and once stdout/stderr hit EOF the loop's all-`None`
termination check never fires, so every `read()` returns `TimedOut` until our deadline —
`FilterError::Timeout` with output stranded. Either kernel behavior, treating `BrokenPipe` like
`TimedOut` and looping is wrong. The communicator's stdin handling cannot be resumed or bypassed
after EPIPE; the fix must keep stdin out of the communicator entirely.

### 1.3 Design: dedicated stdin-writer thread; the communicator only drains

Restructure `run_subprocess` (signature unchanged; callers `filter::run_filter` and the two
`export.rs` sites are untouched):

1. After `Popen::create`, **take the stdin handle out of the `Popen`** (`child.stdin.take()` —
   `Popen.stdin` is a `pub Option<File>` field) *before* calling `communicate_start`.
2. If the input bytes are empty, drop the handle immediately (child sees EOF at once). This
   replaces the current `Some(vec![])` workaround and its comment — with stdin already `None` in
   the `Popen`, `communicate_start(None)` satisfies the crate's assertion (`stdin.is_some() ⇒
   input_data.is_some()`) and the communicator never touches stdin at all.
3. Otherwise spawn a small **writer thread** (helper `spawn_stdin_writer`, a new private fn in
   `filter.rs`, keeping `run_subprocess` under the `too_many_lines` threshold) that does
   `f.write_all(&stdin_bytes)` and ignores the result: an `Err` here — EPIPE included — means the
   child stopped reading, which is **ordinary Unix filter semantics**, not a failure. The `File`
   drops at thread end, delivering EOF to a child still reading. (Rust's runtime ignores
   `SIGPIPE`, so the write returns `EPIPE` rather than killing the process — same assumption the
   `subprocess` crate itself relies on.)
4. Call `child.communicate_start(None)`. The poll loop is otherwise **unchanged**: same `POLL`
   cadence, same cancel and deadline checks, same `remaining_cap` budgeting, same
   combined-size `TooLarge` checks in both `Ok` and `Err` arms, same
   `TimedOut → continue`. The non-`TimedOut` error arm still kills and returns
   `FilterError::Spawn` — but it is now reachable only by genuine stdout/stderr *read* failures,
   because no stdin write ever happens inside the communicator. (Keeping `Spawn` for that arm is
   a deliberate non-change; re-taxonomizing `FilterError` is out of scope.)
5. **Drain, then REAP UNDER THE SAME GUARD — never a blocking `wait()`.** This is the point the
   first draft of this spec got wrong (Codex gate round 1, Critical + Important; see History).
   Moving stdin out of the communicator also moves it out of the poll loop's protection: once
   stdout/stderr hit EOF the drain loop breaks, and a blocking `child.wait()` there is
   **unbounded** for a child that closes its outputs and keeps running without reading stdin
   (`sh -c "exec >/dev/null 2>/dev/null; sleep 600"`). Writer blocked in `write_all` on a full
   pipe, main thread blocked in `wait()`, nobody watching the deadline — the same deadlock shape
   the existing in-loop comment warns about, with stdin substituted for stderr. So the genuine-
   EOF `break` does **not** go to `wait()`. It enters a second bounded loop, **phase 2 (reap)**,
   which repeats the drain loop's own guard structure:

   ```
   loop {
       if cancel.is_cancelled() { terminate; kill; return Err(Cancelled) }
       if Instant::now() >= deadline { terminate; kill; return Err(Timeout) }
       let slice = POLL.min(deadline.saturating_duration_since(Instant::now()));
       match child.wait_timeout(slice) {            // bounded by construction
           Ok(Some(status)) => break status,        // child reaped → report (writer NEVER joined)
           Ok(None) => continue,                    // still alive → re-check cancel/deadline
           // Preserves today's `unwrap_or(Undetermined)` fallback. NOTE: this arm leaves
           // `child_state == Running` (the crate only sets Finished on success or ECHILD) —
           // the ReapGuard in §1.3.2, not this arm, is what stops `Popen::drop` blocking here.
           Err(_) => break ExitStatus::Undetermined,
       }
   }
   ```

   `Popen::wait_timeout` is documented in the pinned 0.2.15 as blocking "roughly no longer than
   `dur`" (Unix: `waitpid(..., WNOHANG)` in an adaptive-sleep loop), and `Popen::poll` is
   "guaranteed not to block" — either is bounded; `wait_timeout(slice)` is preferred because it
   sleeps rather than spins, satisfying the resource law. **Cancel latency and the timeout
   deadline are therefore enforced continuously from spawn to reap, in both phases**, exactly as
   the single-phase loop enforced them today. Note phase 2 costs nothing in the common case: a
   normal child is already dead by EOF, so the first `wait_timeout` returns `Some` immediately.
   Terminal `ExitStatus` handling (`Exited(0)` → `Ok(out_buf)`, `Exited(code)`/`other` →
   `NonZero`) is unchanged.
6. **The writer thread is NEVER joined — on any path. `run_subprocess` drops the
   `JoinHandle`.** Rev 2 of this spec joined after a "confirmed reap"; Codex gate round 2 showed
   that is still unbounded, and it is right (see History). Reaping the *direct* child proves
   nothing about **descendants**. The trigger — **use this exact shape, verified by probe in
   §1.4.1; the plausible-looking `sleep 600 & exit 0` variant does NOT work, see below**:

   ```sh
   sh -c 'exec 3<&0; exec >/dev/null 2>/dev/null; sleep 600 <&3 & exit 0'
   ```

   The shell saves stdin on fd 3, redirects its own outputs away, backgrounds a `sleep` whose
   fd 0 is explicitly the *inherited pipe*, and exits 0. Phase 1 sees EOF; phase 2 reaps the
   shell immediately; the join then blocks forever because the grandchild holds the pipe's read
   end open and never reads — an unbounded hang on the **success** path, after every timeout and
   cancel check has stopped. (Writing this without the explicit `<&3` is the trap rev 3 fell
   into: under bash a background job's fd 0 comes from `/dev/null`, so nothing holds the pipe,
   the writer gets EPIPE, and a restored `join()` returns normally — i.e. the check silently
   passes against a broken implementation.) Verified in the pinned
   0.2.15 and POSIX: `PopenConfig::default()` sets `setpgid: false`, the pipe's child end becomes
   the child's fd 0 via `dup2`, Unix pipe fds are inherited across `fork`/`exec` unless made
   `CLOEXEC`, and `waitpid(pid, WNOHANG)` speaks only for `pid`.

   **Applying the invariant this spec named, to itself:** "no blocking call whose bound depends
   on a process cooperating." The join is exactly such a call — its bound depends on *every*
   process holding the stdin read end, which is strictly weaker than anything `Popen` can
   guarantee. So the join does not survive. It also has no purpose: at the point it stood, the
   drain loop had already read stdout/stderr to EOF and phase 2 had the exit status, so the
   writer's completion is not an input to any result. Removing it costs nothing.

   **The detached-writer envelope, stated honestly.** A writer left blocked in `write_all` holds
   one `File` and the input `Vec` (≤ the filtered range, already resident during the run) until
   every process holding the read end closes it, or until the editor exits. It never blocks
   `run_subprocess`'s return. Accumulating N such threads requires N deliberate runs of a filter
   whose child leaves a descendant holding stdin open; memory is reclaimed at process exit. This
   is a bounded-memory cost traded for the removal of an unbounded wait — the project's law
   ranks a silent UI wait strictly worse, and the current shipped code already holds the same
   bytes for the same duration inside the `Communicator`'s `input_data`. Documented in-function.

**What happens to output still buffered in the child's stdout pipe when EPIPE hits on stdin:**
under this design the drain loop *never observes* the EPIPE — it is confined to the writer
thread. POSIX pipe semantics guarantee bytes the child wrote before exiting stay readable in the
pipe until EOF, so the loop drains everything the child produced, then **phase 2 reaps** and
reports the child's **real** exit status: `Exited(0)` → `Ok(out_buf)`, `Exited(n)` → `NonZero`
with the real code and captured stderr. Nothing is discarded.

**The combined-size cap reasoning is preserved untouched.** The `CRITICAL` comment block in the
loop (limit_size counts COMBINED stdout+stderr; budget `max_output − captured + 1`; on a
size-limit break kill-don't-wait because the child may still be blocked writing) encodes the
prior stderr-flood deadlock fix, and nothing in this change interacts with it: the crate's
`total = outvec.len() + errvec.len()` never counted stdin, so removing stdin from the
communicator cannot alter the cap arithmetic, and the `run_filter_large_stderr_does_not_deadlock`
regression test (whose child takes `Input::None` anyway) must remain green unmodified. The
existing comments stay; the surrounding doc comments ("API path chosen", "Deadlock safety") are
**rewritten** to describe the writer-thread split — the old text claims "We feed stdin during the
same loop that drains stdout", which becomes false. The stale `#[allow(dead_code)] // wired in
Task 5` on `run_subprocess` is removed (it is `pub` and called by `run_filter` and `export.rs`).

### 1.3.1 Enumeration: EVERY blocking operation, its bound, and what the bound rests on

Two consecutive gate rounds found this spec relocating a hang rather than removing it, each time
in a call the previous revision had not enumerated. So the enumeration is now exhaustive and is
part of the spec, not a review artifact. **Rule: if a bound depends on a process choosing to do
something, it is not a bound.**

| # | blocking operation | bound | the bound rests on | verdict |
|---|---|---|---|---|
| 1 | `Popen::create` | exec completes or reports errno | the crate's `CLOEXEC` error pipe closes automatically at a successful `exec` — the child has not yet run user code | kernel-guaranteed; unchanged from today |
| 2 | `comm.read()` per iteration (phase 1) | `limit_time(iter_time)` ≤ `POLL` | `poll(2)`'s timeout argument; `do_read` only reads fds `poll` reported ready | kernel-guaranteed; unchanged from today |
| 3 | `child.wait_timeout(slice)` per iteration (phase 2) | `slice` ≤ `POLL` | `waitpid(WNOHANG)` + adaptive sleep — never waits on a live child | kernel-guaranteed |
| 4 | `child.terminate()` / `child.kill()` | none needed | signal syscalls, do not block | n/a |
| 5 | **`Popen::drop` → `self.wait()`** | **made bounded by construction — see below** | after this change: nothing, because `Drop` never waits | rev 3 claimed "a failed `kill()` means `ESRCH`, i.e. already gone." **That claim was wrong** (gate round 3): `kill()` → `os_kill()` → `send_signal(SIGKILL)` returns whatever `posix::kill` returns and **leaves `child_state` as `Running`** — the crate neither enforces nor inspects the errno. Fixed structurally, not by argument (§1.3.2). |
| 6 | ~~writer-thread join~~ | — | *would* rest on every process holding the stdin read end — including descendants we never spawned and cannot see | **NOT A BOUND → removed (§1.3.6)** |

**Scope of this table, stated precisely** (gate round 3, Minor 1): it enumerates every blocking
operation **on `run_subprocess`'s own return path** — i.e. everything that can delay the value
reaching the caller. Exactly one blocking operation in this design is deliberately left
unbounded, and it is *off* that path: the detached writer thread's own `write_all`, which may
block until every holder of the stdin read end closes it (§1.3.6). It is listed here rather than
omitted, because "we enumerated everything" was the claim that failed twice. Its bound is
**absent by design**; what makes that acceptable is that no caller ever waits on it.

Everything on the return path is bounded by the kernel or by our own construction; nothing is
bounded by a child, a grandchild, or a pipe peer choosing to cooperate.

**Re-walk: every way control leaves `run_subprocess`, and whether `Popen::drop` can block**
(gate round 4 asked for this to be exhaustive rather than limited to the arms a review named —
each of the last four rounds found a defect in a path the previous revision had not listed):

| # | exit | `child_state` at exit | `Drop` blocks? |
|---|---|---|---|
| 1 | `Popen::create` fails → `Spawn` | no `Popen` exists | n/a |
| 2 | phase 1, cancel → `Cancelled` | Running (kill sent) | **no** — guard reaps or detaches |
| 3 | phase 1, deadline → `Timeout` | Running (kill sent) | **no** — guard |
| 4 | phase 1 `Ok` arm, combined cap → `TooLarge` | Running (kill sent) | **no** — guard |
| 5 | phase 1 `Err` arm, combined cap → `TooLarge` | Running (kill sent) | **no** — guard |
| 6 | phase 1 `Err` arm, non-`TimedOut` → `Spawn` | Running (kill sent) | **no** — guard |
| 7 | phase 2, cancel → `Cancelled` | Running (kill sent) | **no** — guard |
| 8 | phase 2, deadline → `Timeout` | Running (kill sent) | **no** — guard |
| 9 | phase 2, `Ok(Some(status))` → `Ok`/`NonZero` | **Finished** | **no** — guard returns at `poll()` |
| 10 | **phase 2, `Err(_)` → `Ok`/`NonZero` with `Undetermined`** | **Running** — the round-4 finding | **no** — guard kills, bounded-reaps, detaches |
| 11 | **unwind (panic between spawn and return)** | any | **no** — `ReapGuard::drop` runs during unwind; this is precisely what rev 4's per-return-statement spelling could not cover, and why the guard is RAII rather than an epilogue call. **Conditional on the §1.3.2 ordering constraint** (guard constructed before any panic-capable work) |

Row 11 holds **because of RAII — not because unwind is unreachable.** Rev 5 also argued "no panic
source after spawn"; that argument was overstated and is withdrawn (gate round 5, Minor). It is
not the reason the row is safe, and treating it as a second line of defence would be false
comfort: the row is safe iff the `Popen` is inside `ReapGuard` before any panic-capable statement
runs, which is the **ordering constraint stated in §1.3.2** and is the thing to verify. (For
context only, not as a bound: `filter::guarded_filter` catches a panic above this frame into
`FilterError::Panicked`, so an unwind through here surfaces as a normal typed error rather than
killing the worker.) Rows 2–8 keep their inline `terminate()`/`kill()` for prompt stopping; the
guard is what makes the *bound* true.

### 1.3.2 Bounding `Popen::drop` (rev 4 — replaces an argument with a mechanism)

`Popen::drop` closes `self.stdin`, then calls `self.wait()` whenever `detached == false` and
`child_state == Running`; `wait()` blocks in `os_wait()` until `Finished`. Two verified facts
matter:

- The crate's own deadlock fallback in `Drop` (`self.stdin = None`) is **already inert in shipped
  code**, because `communicate_start` takes stdin out of the `Popen` first — confirmed by the
  gate. But today the `Communicator` still owns that handle and is dropped *before* `child`
  (reverse declaration order), so today a child blocked *reading* stdin does receive EOF at
  scope exit. In this design the **writer thread** owns the handle instead, so that incidental
  EOF no longer happens at return. Left alone, that would make `Drop`'s wait strictly *more*
  reachable than today — a regression this spec must not ship.
- `kill()` leaves `child_state` `Running` regardless of outcome, so "we killed it" does not by
  itself prevent `Drop` from waiting.

**Mechanism — ONE chokepoint, not a per-path fix.** Rev 4 spelled the reap-then-detach out at
each early return, and gate round 4 found the path that spelling missed: phase 2's
`Err(_) => break ExitStatus::Undetermined`. Verified at the source — `Popen::waitpid` sets
`Finished` **only** on a successful reap or `ECHILD`; every other errno (e.g. `EINTR`) returns
`Err(e)` with `child_state` still `Running`, so that break left `Drop` free to block. A
per-path fix would have been the fourth consecutive round of patching the arm a reviewer named.
Instead the guarantee becomes structural, via an RAII guard that owns the child:

```rust
/// Owns the child for the whole of `run_subprocess` and guarantees, on EVERY exit — normal
/// return, early return, or unwind — that dropping the inner `Popen` cannot block.
/// `Popen::drop` waits whenever `detached == false` and `child_state == Running`; this makes
/// at least one of those false before that drop runs.
struct ReapGuard(Popen);

impl Drop for ReapGuard {
    fn drop(&mut self) {
        if self.0.poll().is_some() { return; }  // already Finished → Popen::drop will not wait
        let _ = self.0.terminate();
        let _ = self.0.kill();
        // Bounded reap: no zombie in the normal case. If it is not CONFIRMED reaped — kill
        // failed with anything, EINTR, uninterruptible sleep — detach, so `Popen::drop`
        // returns at once. Never `wait()`.
        if !matches!(self.0.wait_timeout(REAP_GRACE), Ok(Some(_))) { self.0.detach(); }
    }
}
```

`Popen::poll` is a zero-duration `wait_timeout` ("guaranteed not to block") that also promotes
`child_state` to `Finished` if the child has died; `Popen::detach` is `pub` and sets exactly the
`detached` flag `Drop` tests.

**Implementation ordering constraint (required, not incidental — gate round 5, Minor).** The
`Popen` returned by `Popen::create` MUST be moved into `ReapGuard` **immediately**, before
`communicate_start`, before `spawn_stdin_writer`, and before any other work:

```rust
let mut guard = ReapGuard(child);   // FIRST — nothing panic-capable may run before this
let writer = spawn_stdin_writer(guard.0.stdin.take(), stdin_bytes);
let mut comm = guard.0.communicate_start(None);
```

Row 11 of §1.3.1 (unwind) holds **because of RAII, not because unwind is unreachable** — a bare
`Popen` alive across even one panic-capable statement is a window where an unwind bypasses the
guard entirely. This is an ordering an implementer could plausibly get wrong (e.g. building the
communicator first "to keep the guard construction near the loop"), so it is stated as a
constraint the reviewer checks, not left as an assumption.

### 1.3.3 `REAP_GRACE` and the cancel-latency contract — ONE number, one place

Gate round 5 found a real contract conflict: rev 5's `REAP_GRACE` ≈ 200 ms is spent inside
`ReapGuard::drop` on the cancel path, which contradicted the "< ~100 ms" cancel-latency claim
elsewhere in this spec. Resolving it needs two facts about what a writer actually experiences,
verified in `input.rs` and `jobs_apply.rs`:

1. **Esc acknowledgment is immediate and NOT gated on this function.** `input::handle_key`'s Esc
   arm runs on the main thread: `editor.filter_in_flight.take().unwrap().cancel()` plus
   `set_status(Info, "cancelling…")`. The flag is cleared and the cancel delivered on the very
   next loop turn, whatever the worker thread is doing. (The *painting* of `"cancelling…"` is not
   guaranteed: `set_status` goes through status-slot arbitration, so under a stricter
   `messages_min_kind` or a more severe current occupant this Info message can be history-only
   and never visible. The cancellation itself is unaffected.)
2. **The terminal status IS gated on this function returning.** `jobs_apply`'s `FilterDone` arm
   is what calls `finish_topic(...)` to replace "cancelling…" with the final message, and it runs
   only when the worker's `Msg::FilterDone` lands — i.e. after `run_subprocess` returns.

So the latency at stake is not "does Esc respond" (it always does, immediately) but "how long
does `cancelling…` linger before resolving" — a visible transient, and the thing the ~100 ms
budget should govern.

**Decision: option (a) — shrink `REAP_GRACE` to 20 ms, applied uniformly on every path.**

- Rejected **(b) skip the grace on cancel only.** It buys no latency that (a) does not already
  buy, and it makes Esc-during-filter — a routine gesture — routinely create an unreaped child,
  because `poll()` immediately after `kill()` usually has not observed the reap yet. Trading a
  common path's hygiene for nothing is the wrong side of "the editor owns the terminal, so
  orphaned children are not free." It also adds a second contract to keep consistent, which is
  how the last four rounds' defects happened.
- Rejected **(c) keep 200 ms and document ~250 ms.** The stated law is that cancel takes effect
  within ~100 ms; a 20 ms grace satisfies it at no real cost, so documenting a number above the
  law to preserve an arbitrary constant is backwards.
- **(a) keeps one number in one place** and holds the budget with margin. 20 ms is generous for
  the case it exists to serve: reaping a process the kernel has already destroyed with an
  uncatchable SIGKILL typically completes in well under a millisecond.

**Requested-duration arithmetic, cancel path (checkable by a reader).** These are the durations
the design *requests*, not elapsed wall-clock guarantees — see the caveat below, which is part of
the contract, not a footnote:

| step | requested |
|---|---|
| notice the flag — checked at the top of both loops; an iteration is capped by `limit_time(iter_time)` (phase 1) or `wait_timeout(slice)` (phase 2), both ≤ `POLL` | ≤ 50 ms |
| inline `terminate()` + `kill()` — signal syscalls | ~0 |
| `ReapGuard::drop`: `poll()` (nonblocking) + `wait_timeout(REAP_GRACE)` | ≤ 20 ms |
| **total requested** | **~70 ms** (target, under normal scheduling) |

**This is a target, not a hard bound, and the spec does not claim otherwise.** Every step above is
a sleep or a syscall on a preemptive OS: `wait_timeout` → `os_wait_timeout` loops
`waitpid(WNOHANG)` + `thread::sleep(min(delay, remaining))`, and `thread::sleep` guarantees a
*minimum*, not a maximum — it can overshoot arbitrarily under scheduler pressure. Concretely: a
cancel set just after a phase-1 `comm.read()` begins costs nearly the full 50 ms to notice, and a
`ReapGuard` that requests 20 ms may not be rescheduled for 60 ms under load, so visible
completion can exceed 70 ms and can cross the ~100 ms budget. No arrangement of these primitives
yields a wall-clock guarantee, so the honest contract is: **~70 ms under normal scheduling,
degrading with system load; bounded in requested durations on every path; never unbounded.**
That last clause is the property this effort actually guarantees, and it is the one that matters —
the failure mode being engineered out is an *indefinite* hang, not a slow millisecond.

**Scope of this scoping.** The above governs **production contracts**: no production claim in this
spec asserts a hard wall-clock bound. It deliberately does **not** govern **test** assertions,
which necessarily use concrete thresholds — T5's `recv_timeout(15s)`, T7's `recv_timeout(20s)` and
its `elapsed < 5s`, all chosen generously against loaded, `--test-threads`-saturated soak runs.
That is not an inconsistency: detecting an *indefinite* hang requires some finite threshold to
cross, so "did not hang forever" is not assertable without one. Softening those numbers into
requested-duration language would make the tests unable to fail on the exact defect they exist to
catch — the vacuous-test failure mode this spec already hit once in §1.4.1.

Typical case is ~50 ms, dominated by the poll cadence, since the SIGKILLed child reaps in
microseconds. The same reasoning and the same caveat apply to the `Timeout` path's overshoot past
its deadline. **`REAP_GRACE = 20 ms` is defined once, beside `POLL`, and is the only grace
constant in the function.**

Residual, stated so it is not mistaken for a defect later: when a SIGKILLed child is not reapable
within 20 ms — uninterruptible (`D`-state) sleep, or severe scheduling delay such as the 6×6
contention soak in §7 — the guard detaches and leaves **one unreaped child per affected run**.
`filter_in_flight` bounds one *active* filter, but `apply_filter_done` clears it once the worker
returns, so repeated cancel/timeout runs that hit the 20 ms miss branch each leave one behind —
the same accumulation shape already acknowledged for detached writer threads in §1.3.6, and
likewise proportional to deliberate runs and reclaimed at process exit. That is the deliberate
trade: a bounded, invisible resource cost in a rare branch, versus a routinely slower cancel.

The body holds the child as `ReapGuard` and works through `&mut guard.0`. The semantic
`terminate()`/`kill()` at the cancel/timeout sites **stay** — they stop the child promptly, which
is a user-visible property, not just hygiene; the guard is the safety net beneath every exit,
including the ones no reviewer has named. Net effect: in the normal case the child is reaped
within `REAP_GRACE` (no zombie, matching today's hygiene); otherwise it is detached and `Drop`
returns immediately, leaving one unreaped child per affected run (§1.3.3). **`Popen::drop`
cannot block on any path** — and unlike rev 4, that is now a mechanical property of a single
`Drop` impl rather than a claim about a set of return statements someone has to keep complete.

The residual cost of the pathological branch is one unreaped child until the editor exits —
accepted for the same reason as the detached writer: a bounded, invisible resource cost in a
practically-unreachable branch beats any chance of a blocked return.

**Resulting timeout/cancel coverage, per exit path:**

| exit path | phase | what bounds it |
|---|---|---|
| `Cancelled` | 1 or 2 | flag checked at the top of both loops, every ≤ `POLL`; plus `ReapGuard`'s ≤ `REAP_GRACE` before return — ~70 ms requested, a target not a wall-clock bound (§1.3.3) |
| `Timeout` | 1 or 2 | `deadline` checked at the top of both loops, every ≤ `POLL`; phase-1 iterations bounded by row 2, phase-2 by row 3 |
| `TooLarge` | 1 | combined-size check in both the `Ok` and `Err` arms (unchanged) |
| `Spawn` (read error) | 1 | non-`TimedOut` stdout/stderr read failure; kill + return (unchanged) |
| `Ok` / `NonZero` | 2 | child reaped by `wait_timeout`; **no join**, so a descendant holding stdin cannot delay the return |

The child that closes stdout/stderr and sleeps past its timeout exits via the `Timeout` row from
phase 2. The child that closes stdout/stderr, backgrounds a grandchild holding stdin, and exits 0
returns **promptly** via the `Ok` row — matching today's shipped behavior, which the gate
correctly notes is already bounded here (its poll loop keeps hitting `limit_time` and returns at
the deadline). **Cancel latency: see §1.3.3 for the single authoritative figure and its
arithmetic** — ~70 ms requested for `run_subprocess` to return after the flag is set, a target
under normal scheduling rather than a wall-clock bound (Esc acknowledgment itself is immediate and
not gated on this function at all). This spec states that budget in exactly one place; do not
restate it inline.

**Concurrency/resource cost, corrected** (gate round 3, Minor 2 — rev 3 claimed writers were
"bounded to one at a time by `filter_in_flight`" while also admitting N can accumulate; the
claim was the wrong half). `filter_in_flight` bounds **in-flight filters**, not **live writer
threads**: it is cleared when the `FilterDone` message is applied, and the prompt-success path
(T7's shape) returns *while its writer is still blocked*, so the next filter may start with the
previous writer still alive. The accurate statement: one short-lived writer thread per
filter/export run, normally finishing well before its filter completes; in the pathological
descendant case a writer outlives its run, and N such writers require N deliberate runs of a
filter whose child leaves a descendant holding stdin open. Each holds one `File` and its input
`Vec`; all are reclaimed at process exit. This is the §1.3.6 envelope, and it is the only
resource claim this spec makes about writers.

### 1.4 Behavioural tests (real children — not mocked errors)

Six defects last effort were "built, unit-tested against a struct, never exercised on the real
path." These tests all drive real children through the real `run_filter` → `run_subprocess`
path. The key trick that converts the 39%-under-contention race into a **deterministic** test:
give the child stdin much larger than the kernel pipe buffer (Linux default 64 KiB — use 1 MiB)
and a command that exits after reading little or nothing. The writer is then *guaranteed* to be
mid-write (or pre-write) when the child exits, so EPIPE always occurs. Against the **old** code
these tests fail 100% deterministically (the communicator's first post-exit write EPIPEs →
`Spawn`); against the new code they pass. That is the fix's primary evidence.

All `#[cfg(unix)]`, matching the file's existing convention:

- **T1 — early exit, output kept:** `argv ["head","-c","5"]`, 1 MiB stdin →
  `RunResult::Stdout` of exactly the first 5 bytes.
- **T2 — early exit, nonzero + stderr kept (the hardened flaky scenario):**
  `argv ["sh","-c","head -c 4 >/dev/null; echo boom >&2; exit 3"]`, 1 MiB stdin →
  `FilterError::NonZero` with `code` containing `3` and `stderr` containing `boom`.
- **T3 — zero consumption:** `argv ["sh","-c","exit 0"]`, 1 MiB stdin → `Ok` with empty stdout.
- **T4 — pipe-buffered output survives child exit:** `argv ["sh","-c","echo out; exit 0"]`,
  1 MiB stdin → `Stdout("out\n")`.

**T5 and T6 exist because T1–T4 cannot catch the Critical.** All four use children that *exit*,
so every one of them passes against an implementation that reaps with a blocking `wait()` — the
hang needs a child that closes stdout/stderr and stays alive. Both new tests run `run_filter` on
a worker thread and assert the result arrives via `rx.recv_timeout`, following the existing
`run_filter_large_stderr_does_not_deadlock` pattern, so a regression **times out the harness
instead of wedging the whole suite**:

- **T5 — timeout survives a child that closes its outputs and keeps running (the Critical
  regression):** `argv ["sh","-c","exec >/dev/null 2>/dev/null; sleep 600"]`, **1 MiB stdin**
  (so the writer thread is genuinely blocked on a full pipe, not incidentally finished),
  `timeout: Duration::from_secs(1)`; harness `recv_timeout(15s)` must yield
  `RunResult::Err(FilterError::Timeout)`. Against the first draft's design this blocks for 600 s
  and the harness fails; against the phase-2 reap loop it returns in ~1 s. This is the test the
  gate found missing.
- **T6 — cancel survives the same child:** identical argv and 1 MiB stdin, `timeout` 60 s, with
  a second thread calling `cancel.cancel()` after ~200 ms; harness `recv_timeout(15s)` must
  yield `FilterError::Cancelled`, proving the CancelFlag is still honoured after stdout/stderr
  EOF (Esc must work on a child that has stopped talking but not died).

**T7 covers a path T5/T6 structurally cannot** (gate round 2): they exercise
*child-still-alive*, where the deadline eventually rescues any implementation. The descendant
case is *direct-child-reaped, grandchild holds stdin*, and it lands on the **success** path —
where no deadline check remains, so a hang there is silent and permanent. Its assertion shape is
therefore different from T5/T6's: not "the right error arrives", but "**a success arrives, and
arrives promptly**".

- **T7 — success path returns promptly when a descendant inherits stdin (the round-2 Critical
  regression):** `argv ["sh","-c","exec 3<&0; exec >/dev/null 2>/dev/null; sleep 30 <&3 & exit 0"]`
  (the shipped test uses `sleep 30`, not the 600 shown in this rev's design and in §1.4.1's probe
  — deliberate soak hygiene: the grandchild outlives the test by design, and 600 s across a 60×
  soak would strand sixty ten-minute processes; the shorter sleep proves the same mechanism. See
  History), **1 MiB stdin**, `timeout: Duration::from_secs(10)`. On a worker thread with harness
  `recv_timeout(20s)`, assert **both**:
  1. the result is `RunResult::Stdout(s)` with `s.is_empty()` — the shell's outputs went to
     `/dev/null` and it exited 0, so success is the correct verdict; **and**
  2. the measured elapsed time is well under the 10 s timeout (assert `< 5s`, generous against
     loaded CI-less soak runs).

  Assertion 2 is load-bearing and must not be dropped as "flaky-looking": without it a future
  implementation that stalls until the deadline and *then* returns a success would pass
  assertion 1. Against rev 2's join-after-reap design this test blocks ~30 s (the shipped
  descendant lifetime) and the harness fails; against the no-join design it returns in
  milliseconds.
  FAIL-VERIFY for the plan: re-add `let _ = writer.join();` before the terminal `match`, watch
  T7 hang and fail its harness bound, then revert.

#### 1.4.1 How each scenario was VERIFIED to actually occur (not reasoned about)

Rev 3's T7 used `sleep 600 & exit 0` and was **hollow**: on this machine `/bin/sh` is bash, and a
non-interactive background job gets fd 0 from `/dev/null`, so the backgrounded `sleep` never held
the filter's stdin at all — the shell exited, the read end closed, the writer got EPIPE, and a
restored `join()` would have returned normally. **A test written specifically to catch the
round-2 Critical would have passed against the broken design** — the same class as the six
"built, unit-tested, never exercised on the real path" defects from the last effort: a test whose
name describes a scenario it does not create. So every scenario below is verified by observing
the actual process tree, and the probe is recorded here to be re-run.

Probe method: a fifo stands in for the filter's stdin pipe; a background holder keeps its write
end open; the scenario runs with `< F`; then, for each live process, `readlink /proc/<pid>/fd/0`
is compared against the fifo path (and fd 1 / fd 2 read to confirm our stdout/stderr write ends
were released, which is what makes phase 1 see EOF).

```sh
mkfifo F; sleep 15 > F &                 # holder keeps the write end open
sh -c '<SCENARIO>' < F >/dev/null 2>&1 & # scenario, backgrounded so the probe can inspect it
sleep 0.6
for p in /proc/[0-9]*; do [ "$(readlink $p/fd/0 2>/dev/null)" = "$PWD/F" ] && \
  echo "$p holds stdin: $(tr '\0' ' ' < $p/cmdline)"; done
```

Observed on this machine (2026-07-19), verbatim conclusions:

| scenario | direct child | who holds stdin | fd1/fd2 | verdict |
|---|---|---|---|---|
| `sh -c 'sleep 30 & p=$!; readlink /proc/$p/fd/0'` (the gate's probe) | — | — | — | prints `/dev/null` — background jobs do **not** inherit stdin under bash |
| **T5/T6** `exec >/dev/null 2>/dev/null; sleep 600` | **STILL ALIVE** | the direct child itself (bash `exec`s the `sleep`, so pid is unchanged) | `/dev/null`, `/dev/null` | **valid** — child alive, our stdout/stderr write ends released (phase 1 sees EOF), stdin held and never read (writer blocks) |
| **rev 3's T7** `exec >/dev/null 2>/dev/null; sleep 600 & exit 0` | EXITED rc=0 | **NOBODY** | — | **INVALID — discarded**; confirms the gate |
| **T7 (this rev)** `exec 3<&0; exec >/dev/null 2>/dev/null; sleep 600 <&3 &  exit 0` | EXITED rc=0 | a **different** pid (the grandchild), holding the fifo on fd 0 | `/dev/null`, `/dev/null` | **valid** — direct child reaped on the success path while a descendant keeps stdin open, exactly the join-hang trigger |

So T5/T6's shape survives scrutiny for the reason T7's did not: the `sleep` there is the
foreground `exec`-replaced child, not a background job, so it keeps the inherited stdin. The
writer blocks in all three valid cases because 1 MiB far exceeds the 64 KiB default Linux pipe
buffer — the stdin bytes cannot be absorbed and forgotten.

The plan must re-run this probe on the implementing machine before relying on T5–T7, since the
`/bin/sh` → bash link and the background-job fd-0 behaviour are both platform facts, not
guarantees. If `/bin/sh` is dash or the probe shows a different holder, the argv must be adjusted
until the probe shows the intended holder — **the probe, not the shell reasoning, is the
acceptance criterion.**
- **Existing suite unchanged:** `run_filter_identity_cat`, `run_filter_transform_tr`,
  `run_filter_non_zero_exit_carries_stderr` (kept as-is; it must now pass deterministically —
  its stdin is 2 bytes, but even a lost race is handled), `run_filter_rejects_oversized`,
  `run_filter_large_stderr_does_not_deadlock`, `filter_output_above_old_1mib_cap_succeeds…`,
  `run_filter_rejects_non_utf8`, `run_filter_missing_binary_is_spawn_error` (spawn-failure
  `Spawn` path untouched), `shell_pipeline_survives_quoted_whitespace`.

FAIL-VERIFY note for the plan: restore in-communicator stdin feeding, watch T1–T4 fail
deterministically, revert.

---

## 2. Part 2 — the test-isolation class

### 2.1 Verified census: tests that touch the developer's real state dir

`swap::state_dir()` is the **single** resolution point for
`$XDG_STATE_HOME/wordcartel` (fallback `~/.local/state/wordcartel`): the only production caller
of `dirs::state_dir()` in the crate is `swap::state_dir` (verified by grep; `config.rs`'s
`dirs::config_dir()/home_dir()` uses are the out-of-scope config-class per D4). It provisions
(`create_dir_all` + chmod 0700) on every call. Everything below reaches the real directory
through it — directly, or via `swap_path`, `write_atomic(&swap_path(..))`, `assess`,
`dispatch_swap_write`, `open_swap_paths`, `state::save`, or the save-epilogue `swap::delete`.

Writers/scanners of the real dir (wordcartel lib test binary):

| test | touch |
|---|---|
| `swap::tests::write_atomic_writes_0600_and_roundtrips_via_parse` | write + remove |
| `swap::tests::recovery_hash_equal_discards_silently`, `recovery_diverged_prompts` | write + remove |
| `swap::tests::assess_over_cap_swap_opens_normally` | writes **64 MiB + 1** (`MAX_OPEN_BYTES + 1`) into the real state dir on every run, then removes — the single heaviest offender |
| `swap::tests::dispatch_swap_write_writes_a_recoverable_swap`, `…uses_the_injected_fs_and_a_failed_write_does_not_latch`, `stale_path_swap_does_not_relatch_after_rekey` | write via the production dispatch path + remove |
| `swap::tests::swap_is_cleanable_only_for_valueless_dead_pid_swaps`, `…gives_the_same_verdict_from_a_cached_header`, `…excludes_relocated_and_realpath_none` (via `make_doc_with_swap`) | write + remove |
| `swap::tests::enumerator_scan_includes_discard_silently_excludes_prompt`, `kept_recoverable_count_reports_what_the_sweep_deliberately_spares` | write + **scan the shared dir** + remove; assertions deliberately litter-tolerant |
| `recovery::tests::write_dump_writes_named_0600_file_with_body`, `write_dump_handles_scratch_buffer` | write + remove |
| `session_restore::tests::persist_session_stamps_the_active_documents_id`, `…captures_scratch_even_when_active_unnamed`, `…clears_stale_scratch_when_oversized` | **overwrite the real `session.toml`, no restore at all** (D4's named worst case; C5 fixed the sibling reader `recents::open_recent_in` and documented this exact interleave hazard in its comment) |
| `prompts::tests::clean_recovery_confirm_skips_a_path_that_became_recoverable_while_prompt_open`, `the_clean_recovery_modal_names_kept_recoverable_files` | write + scan + remove; the modal test's own comment documents working around ambient real-dir litter (`KEPT_SHOWN` elision), and it failed once in map-flakes' contention window |
| provision-only / remove-only: `swap::tests::swap_path_named_is_deterministic_and_in_state_dir`, `swap_path_scratch_uses_pid`, `state_dir_is_0700`, `recovery_no_swap_opens_normally`, `open_swap_paths_covers_open_buffers_and_session_scratch`; plus every save-path test whose epilogue runs `swap::delete` (e.g. `e2e.rs`'s save journey, which carries a comment noting the real-dir `create_dir_all`) | create/chmod the real dir; delete self-keyed files |

The two prompts tests above are not named in D4's list but fall squarely inside its rule-based
class ("every test writing the real `state_dir()`") — this is rule membership, not a widening.
Tests already isolated and needing nothing:
`swap::tests::find_orphan_scratch_swap_finds_dead_pid_and_skips_self` (unique dir via the
`find_orphan_scratch_swap_in` seam), all `unique_dir`-based enumerator tests, `state.rs` tests
(`save_in`/`load_in` at private temp dirs), the e2e recents journey (uses `open_recent_in`).

### 2.2 D5 mechanism: structural redirect at the `state_dir` chokepoint

In `#[cfg(test)]` builds, `swap::state_dir()` **never consults `dirs::state_dir()` /
`dirs::home_dir()`**; it resolves the base to a per-process temp dir and runs the *same*
provisioning on it:

```rust
pub fn state_dir() -> io::Result<PathBuf> {
    // Test builds: every caller — production code reached from tests included — lands in a
    // per-process temp dir, never the developer's real $XDG_STATE_HOME. A test that forgets a
    // seam damages nothing ambient. (Effort ① D5: structural, nothing to evade or allow-list.)
    #[cfg(test)]
    let base = std::env::temp_dir().join(format!("wcartel-test-state-{}", std::process::id()));
    #[cfg(not(test))]
    let base = dirs::state_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no state dir"))?;
    let dir = base.join("wordcartel");
    /* existing create_dir_all + cfg(unix) chmod 0700, existing fs-chokepoint-allow markers */
    Ok(dir)
}
```

Properties, each verified against the census:

- **Damage class closed for the lib test binary — precisely that, no more.** Every row in §2.1
  — including the 64 MiB write and the `session.toml` overwrites — lands in the redirected dir,
  as does any future test *in the lib test binary or the in-source `e2e` module* that forgets a
  seam. That is the mechanism's exact guarantee, and the §2.1 census is entirely inside it.
  **It does NOT extend to future `wordcartel/tests/*` integration tests**, which link the
  library compiled without `cfg(test)` and would reach the real dir (see the boundary note
  below — this is the same fact, stated here so the guarantee is not overclaimed). No per-test
  migration, no allow-list, no scanner (D5's measured argument against a second textual guard:
  5/6 evasion routes uncaught, self-checks covering none).
- **Assertions keep their meaning.** The provisioning body runs identically, so
  `state_dir_is_0700` still exercises the production `create_dir_all` + chmod on the returned
  dir, and `swap_path_named_is_deterministic_and_in_state_dir`'s `starts_with(state_dir())` is
  self-consistent. The enumerator/kept/modal tests keep their litter-tolerant containment
  assertions but now see only in-process suite litter, never ambient real-session orphans — the
  modal test's `KEPT_SHOWN`-elision hazard (its own documented workaround, and its one observed
  contention failure) shrinks strictly.
- **The persist trio needs no seam churn.** All three assert on the **in-memory**
  `SessionState` only (verified: `s.entries[..]`, `session.scratch`); the file write was pure
  uninspected side effect. Redirect alone removes the damage. This is D3 proportionality: the
  chokepoint redirect *is* the seam for the damage class; per-call-site `_in` injection is
  reserved for tests whose assertions need a private dir, and none newly do.
- **Compile-time, thread-safe.** `cfg(test)` is a compile-time property of the whole test
  binary, so worker threads (`dispatch_swap_write`'s executor job, listing threads) are covered;
  no thread-local propagation problem.
- **Zero production delta.** The shipped binary compiles the `cfg(not(test))` arm only — the
  literal current body.

**Durable guard (structural, not textual):** one new test in `swap::tests`, e.g.
`state_dir_in_test_builds_is_redirected_off_the_real_xdg_dir`, asserting
`state_dir().unwrap().starts_with(std::env::temp_dir())` — a compiled, behavioral assertion that
the redirect exists, immune to the evasion classes the H26 map measured.

**Documented residual boundary (not a gap in the current tree, but a real limit on the
mechanism):** `cfg(test)` covers the lib test binary and the in-source `e2e` module (gated
`#[cfg(test)] mod e2e;` in `lib.rs`) — **not** integration-test binaries under
`wordcartel/tests/`, which link the library compiled without it. A future integration test
calling `state_dir()` would therefore reach the developer's real dir, and this redirect would
not stop it. Verified for the current tree: none of the seven integration targets
(`backlog`, `edit_seam`, `fs_chokepoint`, `module_budgets`, `sentence_differential`,
`harper_ls_integration`, `harper_ls_probe`) calls into `swap`/`state` at all, so the census in
§2.1 is fully covered today. Closing the integration-binary boundary would need a different
mechanism (e.g. an env-var override honored in non-test builds) and is **not** in this effort's
scope; it is recorded here so a future author sees the limit rather than assuming coverage. The PTY smoke suite likewise drives
the real binary against the real dir — **intentionally**: that is where real-dir behavior
(s8 kill → swap → recovery) is proven end-to-end, and it stays outside the redirect. Both facts
go in `state_dir`'s doc comment.

**Cleanup:** the redirected dirs are pid-keyed litter in the OS temp dir, same class as the
suite's existing `wc-*` temp files; tests keep their own `remove_file` hygiene; no global
teardown exists to hook (no CI, no harness exit hook), and none is added.

### 2.3 D5's open question, answered: how many tests legitimately need the REAL `state_dir()`?

**Zero.** Checked every candidate: `state_dir_is_0700` and the `swap_path` tests assert
properties of *whatever directory the function returns* (still exercised under redirect); the
enumerator/kept/modal tests are litter-tolerant but do not *require* ambient litter; the persist
trio never reads the file back; the save-epilogue touches are incidental provisioning; and the
real-XDG end-to-end case is owned by the smoke suite (a non-`cfg(test)` binary, outside the
redirect by construction). No escape hatch is needed, so the "allow-list in disguise" risk D5
flagged does not materialize and D5 stands clean.

### 2.4 D3 for `recovery::LAST_GOOD`: a serialization gate with a same-thread bypass

The flake (3/60 default threads): `editor::tests::undo_and_redo_refresh_the_recovery_snapshot`
drives `Buffer::apply`/`undo`/`redo` (each ends in `recovery::record_snapshot`) and then reads
the process-global `LAST_GOOD` mutex; any concurrent test's edit overwrites it between this
test's write and read. Its doc comment claims a serialization strategy ("taking it, seeding a
sentinel, dropping the guard") that is **not in the body** — and would not work anyway.

A verified constraint shapes the mechanism: `record_snapshot` writes through
`LAST_GOOD.try_lock()` and **silently skips on contention** (deliberate — the panic hook must
never deadlock). So the obvious lock — the asserting test holding `LAST_GOOD` itself across its
acts — is self-defeating: the test's *own* `apply`/`undo`/`redo` snapshots would be skipped and
the assertion would fail. D3's ratified choice ("a lock, because the assertion reads the
global's value") therefore needs a *separate* gate with a same-thread bypass:

```rust
// recovery.rs — test builds only; production record_snapshot is unchanged in shipped builds.
#[cfg(test)]
pub(crate) static SNAPSHOT_GATE: Mutex<()> = Mutex::new(());
#[cfg(test)]
thread_local! { static GATE_HELD: Cell<bool> = const { Cell::new(false) }; }

pub fn record_snapshot(path: Option<&Path>, rope: ropey::Rope) {
    #[cfg(test)]
    let _serial = if GATE_HELD.with(Cell::get) { None } else {
        // Neutralize poisoning: a panicking gate-holder must not cascade into every
        // editing test's apply().
        Some(SNAPSHOT_GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    };
    if let Ok(mut g) = LAST_GOOD.try_lock() { /* existing write */ }
}
```

A small RAII helper in `recovery`'s `#[cfg(test)]` surface (acquire `SNAPSHOT_GATE`, set
`GATE_HELD`, clear+release on drop) is used by the asserting test. Semantics: while the test
holds the gate, every *other* thread's `record_snapshot` blocks **before touching `LAST_GOOD`**;
the test's own thread bypasses the gate (flag), so its snapshots land through the real
`try_lock` path uncontended; the test then reads `LAST_GOOD.lock()` and asserts the true global
value. Lock order is gate → `LAST_GOOD` on all paths — no cycle; the try_lock semantics, the
panic hook, and production behavior are untouched (`cfg(test)` only); test-build cost is one
uncontended mutex per edit outside the assertion window. The test's false comment is rewritten
to describe the gate accurately.

Rejected alternatives, recorded for the reviewer: (a) holding `LAST_GOOD` itself — shown
self-defeating above; (b) a `cfg(test)` thread-local recorder inside `record_snapshot` — house-
precedented (`LOAD_BUDGET_OVERRIDE`, `FAIL_NEXT_COMMIT_WRITE`) but asserts only that the call
happened, not the global's end state, weakening the test below what it verifies today; (c)
order-independent assertion — impossible, per D3, since the assertion is on the value.

The global-path evidence beyond this unit test stays where it already lives: the smoke suite's
s7 panic → restore → recovery-dump check exercises `dump_on_panic` → `LAST_GOOD` end-to-end
against the real binary.

### 2.5 The two fixed `/tmp` paths (D4)

`search_ui::tests::diag_apply_selected_add_dict_writes_file_once_and_nudges_reload`
(`/tmp/wordcartel_adddict_<pid>`) and
`diagnostics_run::tests::append_word_to_dict_creates_parent_dir` (`/tmp/wordcartel_test_<pid>`)
migrate to `tempfile::tempdir()` — already a dev-dependency and already the idiom in `save.rs`
and `e2e.rs` tests. RAII cleanup replaces the manual pre/post `remove_dir_all`; assertions are
unchanged.

### 2.6 In-scope hygiene (one line, droppable if the human objects)

`session_restore::tests::file_browser_enter_on_file_opens_it_when_clean` carries the fifth
false-invariant comment ("simulate Enter via the browser's open path") while actually calling
`open_into_current` directly. This effort corrects the **comment** to state what the test does.
No behavior change; the underlying is-the-warning-reachable question stays with deferred H28
(D1) and is explicitly not addressed here.

### 2.7 Explicitly OUT (D4 — do not widen)

`plugin::INTERN_POOL`, `file_browser::LISTING_EPOCH`, `cursor_style::restore::EVER_WROTE` — no
observed failure; the last is already written order-independent. Untouched. Also out: H26
(needs a `syn`-class dependency decision), H27 (blast radius 10 production vs ~150 test call
sites; the second differently-shaped `registry::Ctx` bundle wouldn't collapse), H28 (a behavior
question about Row-1/Row-2 precedence, not test hygiene — "fixing" the two tests by pumping
deletes the assertion). Each is deferred for the mapped reason, not quietly absorbed.

---

### 1.4.2 Exit-path coverage: covered, added, and deliberately not

Gate round 3, Minor 3. Every exit path of `run_subprocess`, with an explicit decision:

| exit path | status | how |
|---|---|---|
| `Ok` (exit 0) | **covered, existing** | `run_filter_identity_cat`, `run_filter_transform_tr`, `filter_output_above_old_1mib_cap_succeeds_under_new_cap`, `shell_pipeline_survives_quoted_whitespace` |
| `NonZero` | **covered, existing + added** | `run_filter_non_zero_exit_carries_stderr` (existing); **T2** adds the early-exit race form |
| `Ok`/`NonZero` with a descendant holding stdin | **added** | **T7** |
| `TooLarge` (phase 1, combined cap) | **covered, existing** | `run_filter_rejects_oversized`, `run_filter_large_stderr_does_not_deadlock` — both must stay green unmodified |
| `Spawn` (spawn failure, `Popen::create`) | **covered, existing** | `run_filter_missing_binary_is_spawn_error` |
| `NotUtf8` (in `run_filter`) | **covered, existing** | `run_filter_rejects_non_utf8` |
| **`Timeout` (phase 1** — child alive, still producing/silent with streams open) | **NOT covered before; NOT added — deliberate** | Phase 1 keeps the **same guard structure** as today — same `limit_time` per-iteration bound, same deadline check, same cadence — so this path's boundedness is unchanged, though the phase is not literally untouched (stdin no longer flows through `communicate_start`, and early returns now pass through `ReapGuard`). A test would need a ≥1 s real sleep. T5 covers the *changed* risk (phase-2 timeout), which is where the regression lived. Recorded as a known gap rather than silently skipped. |
| **`Timeout` (phase 2** — streams closed, child alive) | **added** | **T5** — the round-1 Critical |
| **`Cancelled` (phase 2)** | **added** | **T6** |
| **`Cancelled` (phase 1)** | **NOT covered; NOT added — deliberate** | Same reasoning as phase-1 `Timeout`: **same guard structure** as today, semantically unchanged and bounded. T6 proves the flag is still honoured in the phase this effort introduced. |
| **`Spawn` (non-`TimedOut` stdout/stderr read error, phase 1)** | **NOT covered; NOT added — deliberate** | Requires injecting an OS-level pipe read failure that is neither EOF nor timeout; there is no seam for it (`run_subprocess` takes no `Fs`-style injection point, and adding one for this arm is out of scope). Note this arm becomes *strictly harder* to reach after this change, since stdin writes — the only routine source of non-timeout errors, and the entire bug — no longer occur inside the communicator. |
| **`Drop`-after-kill-failure** | **NOT covered; NOT added — deliberate, but now structurally defused** | Cannot be provoked without making `kill(2)` fail on our own child, which needs privilege manipulation or a synthetic seam. §1.3.2 removes the need for a test by removing the blocking call: `Drop` cannot wait on any path, because we detach whenever the bounded reap does not confirm `Finished`. A reviewer should check that mechanism by reading, not by test. |

The three "deliberate" rows are stated so a reviewer can disagree with the *decision* rather than
discover the *absence*. Two of them (phase-1 timeout/cancel) are unchanged code; the third has no
seam and gets safer under this change.

## 3. Evidence-of-working table

Per the project caution ("built, unit-tested against a struct, never exercised on the real
path"), each behavior lists the evidence that proves it on the real path:

| behavior | evidence |
|---|---|
| EPIPE fix semantics | T1–T4: real early-exiting children (`head -c`, `sh -c … exit 3`) with 1 MiB stdin forcing a guaranteed EPIPE through the real `run_filter → run_subprocess → Popen` path; deterministic FAIL against the old code, PASS against the new |
| timeout still enforced while stdin is unwritten and the child is alive | **T5**: real child that closes stdout/stderr then sleeps 600 s, 1 MiB stdin, 1 s timeout, bounded harness wait → `Timeout` in ~1 s (hangs 600 s against a blocking-`wait()` implementation) |
| cancel still enforced on the same child | **T6**: same child, cancel fired at ~200 ms → `Cancelled` promptly |
| success path returns promptly when a descendant inherits stdin | **T7**: real child that backgrounds `sleep 30 <&3` (shipped value — 600 in this rev's design and §1.4.1's probe, shortened for soak hygiene; same mechanism, see History) on the *inherited* stdin (fd-0 holder confirmed by `/proc/<pid>/fd/0` probe, §1.4.1) and exits 0, 1 MiB stdin → `Stdout("")` in < 5 s against a 10 s timeout (hangs ~30 s against a join-after-reap implementation) |
| each test's scenario actually occurs | **§1.4.1 probe**, re-run on the implementing machine: `/proc/<pid>/fd/0` readlink showing the intended holder — the acceptance criterion is the probe output, not shell reasoning (rev 3's T7 read plausibly and created nothing) |
| EPIPE fix under the measured failure mode | §7 protocol: 60× full lib binary at **default** `--test-threads` + 6×6 process contention — the exact conditions that produced 4/60 and 14/36 — with zero filter-test failures |
| cap/deadlock reasoning preserved | `run_filter_large_stderr_does_not_deadlock` and `run_filter_rejects_oversized` pass unmodified |
| filter feature end-to-end | smoke suite run (advisory) plus the untouched `dispatch_filter` tests through the real submit path |
| state-dir redirect exists and holds | new compiled guard test (`starts_with(env::temp_dir())`); plus the full suite passing with `$HOME`-relative state untouched — verifiable manually by checking `~/.local/state/wordcartel` mtimes across a suite run |
| `LAST_GOOD` gate | rewritten test through the real `Buffer::apply/undo/redo` path; §7 protocol at default threads (the 3/60 repro basis) with zero failures; smoke s7 covers the panic-hook consumer |
| `/tmp` migrations | the two tests pass with assertions unchanged; no fixed `/tmp/wordcartel_*` literal remains (review check, not a scanner) |

---

## 4. Non-goals

- No `FilterError` taxonomy change; no new user-facing status wording.
- No change to `dispatch_filter`, `run_filter`, or the export path signatures.
- No pumping changes to the H28 picker tests; no `DispatchCtx` collapse; no `fs_chokepoint`
  rework (H26/H27/H28 deferred per D1).
- No new dependencies; no global test-harness hooks; no scanner-style guards.
- No `cargo fmt`; no reflow of untouched code.

---

## 5. Risks and mitigations

- **`subprocess` crate drift:** the design depends on `Popen.stdin` being a public takeable
  field, `communicate(None-stdin, None-input)` being legal, and `Popen::wait_timeout` being
  bounded — all verified in the pinned 0.2.15. The lockfile pins it; a future upgrade
  re-verifies §1.2's and §1.3's facts.
- **Unbounded-blocking-call class (the Critical this spec was revised for, twice):** the
  invariant is that `run_subprocess` contains **no** blocking call whose bound depends on any
  process — child, descendant, or pipe peer — choosing to cooperate. §1.3.1 enumerates every
  such call with its bound; that table is the artifact to re-check on any future edit, and it is
  the thing to extend rather than reason about ad hoc. A reviewer should treat **any** of these
  as Critical: a bare `child.wait()`; a `JoinHandle::join()` on the writer; any new blocking read
  or write on a pipe outside the `limit_time`-bounded loop. Two rounds of this effort's own gate
  were spent on exactly this class, each time on a call the previous revision had not listed —
  the enumeration exists so a third does not happen during execution.
- **Detached writer on kill-failure:** bounded to the already-accepted leaked-child envelope;
  documented in-function (§1.3.6).
- **Gate poisoning:** neutralized via `PoisonError::into_inner` with a one-line rationale
  (guarded, deliberate — satisfies the house unwrap rule).
- **Redirect boundary:** integration binaries and the smoke suite are outside `cfg(test)` —
  today zero integration tests reach `state_dir()`; the doc comment records the boundary so a
  future integration test author sees it.
- **`too_many_lines`:** `run_subprocess` gains and loses lines; the `spawn_stdin_writer` split
  narrows the gap but does not close it — the shipped function still carries
  `#[allow(clippy::too_many_lines)]` with a one-line rationale (a flat, cohesive two-phase
  drain/reap loop whose state must be read together), reviewed and accepted in Task 1's review.
  See History.

---

## 6. Task shape (for the plan; TDD per task)

1. **T-EPIPE:** failing tests T1–T4 **and T5–T7** → writer-thread restructure
   (`spawn_stdin_writer`, `communicate_start(None)`, **phase-2 `wait_timeout` reap loop under
   the cancel/deadline guard**, **no join on any path**) → green; doc comments rewritten (the old
   "we feed stdin during the same loop that drains stdout" text is now false; the two-phase
   guard, the never-join rule and its detached-writer envelope, and §1.3.1's bound-per-call
   reasoning are documented in-function); stale `#[allow(dead_code)]` dropped. T5–T7 must be
   written and seen to fail (T5/T6 wrong-error, T7 harness-timeout) *before* the reap loop and
   the join removal exist — they are the task's TDD anchor, not an afterthought. The implementer
   is told explicitly: **do not add a `join()` "for tidiness"** — §1.3.1 row 6 records why it is
   not a bound, and T7 exists to catch its return.
2. **T-REDIRECT:** failing guard test → `state_dir` cfg(test) redirect → green; doc comment
   records the residual boundary and the `dirs::state_dir` single-caller invariant.
3. **T-LASTGOOD:** gate + RAII helper + rewritten test/comment.
4. **T-TMP:** the two `tempfile::tempdir()` migrations.
5. **T-HYGIENE:** the one-line comment truth fix (§2.6).
6. **T-VALIDATE:** §7 protocol + gates + smoke, results quoted in the pre-merge report.

---

## 7. Validation protocol (mandatory, at default threading)

All on this 32-core machine, no `--test-threads` narrowing anywhere:

1. `cargo test --workspace` — green (gate).
2. `cargo clippy --workspace --all-targets` — clean (gate); `cargo build` warning-free.
3. **Repro-basis soak:** the prebuilt `wordcartel` lib test binary run 60× at default threads —
   zero failures of `editor::tests::undo_and_redo_refresh_the_recovery_snapshot` and
   `filter::tests::run_filter_non_zero_exit_carries_stderr`, and no new failures (this matches
   map-flakes §1b, which measured 3/60 and 4/60 respectively on the unfixed tree).
4. **Contention soak:** 6 rounds × 6 concurrent full-binary processes (map-flakes §1d, which
   measured 14/36 filter failures) — zero failures of the named tests, and the
   `prompts::tests::the_clean_recovery_modal_names_kept_recoverable_files` co-failure not
   reproduced.
5. `scripts/smoke/run.sh` — one-line summary quoted verbatim in the pre-merge report
   (advisory-pass, mandatory-run).

Because there is no CI, the pre-merge report must show these commands and their output; nothing
"is enforced" except by this process.

---

## 8. Decision conformance (D1–D5)

| decision | conformance |
|---|---|
| D1 scope | exactly the EPIPE bug + the isolation class; H26/H27/H28 deferred with reasons restated (§2.7) |
| D2 EPIPE is production | §1.1; the fix is standard Unix filter semantics; the delegated drain-then-wait mechanics are specified in §1.3 as a two-phase loop (drain, then bounded reap) that keeps timeout/cancel enforced end-to-end |
| D3 proportional mechanism | chokepoint redirect = the seam for the damage class (§2.2); gate lock for `LAST_GOOD` (§2.4) — the ratified "lock" choice, with a same-thread bypass forced by the verified `try_lock`-skip fact; no uniform mechanism imposed |
| D4 membership | all IN items covered (census §2.1, including two rule-member prompts tests); OUT items untouched (§2.7) |
| D5 structural durability | redirect + compiled guard test; no scanner, no allow-list; the open count question answered **zero** (§2.3), so no escape hatch exists |

---

## History

- 2026-07-19 (rev 9) — **Fable whole-branch gate, Minor 1: two spec-vs-code drift corrections,
  code governs, no design change.** (a) §5 claimed the `spawn_stdin_writer` split keeps
  `run_subprocess` under the `too_many_lines` threshold "without an `#[allow]`"; the shipped
  function keeps `#[allow(clippy::too_many_lines)]` with a one-line rationale, reviewed and
  accepted in Task 1's review — the split narrows the gap, it does not close it. (b) §1.4/T7 and
  the §3 evidence table said the descendant sleeps 600 s; the shipped T7 uses `sleep 30`,
  deliberately shortened during implementation for soak hygiene — T7's grandchild outlives the
  test by design (it is never joined or waited on), and 600 s across a 60× soak would strand
  sixty ten-minute orphan processes. The shorter sleep proves the identical mechanism (which
  process holds fd 0, and that the harness bound is crossed against a broken implementation) at a
  fraction of the residual cost. Both are the code catching up to a reality the spec hadn't
  recorded, not a redesign — §1.4.1's probe table is left as the historical record of what was
  actually probed (with 600) before the sleep was shortened.
- 2026-07-19 (rev 8) — **Codex gate round 7, sole Minor: wording scope only, no design content.**
  Rev 7's "no hard wall-clock claim survives anywhere" was false as stated — T5/T7 assert concrete
  thresholds (`recv_timeout`, `elapsed < 5s`). Per the coordinator's ruling the **claim's scope**
  was corrected, **not the tests**: §1.3.3 now says the requested-duration framing governs
  production contracts and explicitly does not govern test assertions, which need a finite
  threshold to detect an indefinite hang at all. Softening them would recreate the vacuous-test
  failure mode of §1.4.1. No test value, design text, or other passage changed.
- 2026-07-19 (rev 7) — **Codex gate round 6 folded; three accuracy-of-claim corrections, no design
  change.** (Round 6 verified the `jobs_apply`/`input.rs` wiring grounding in full.) IMPORTANT:
  "worst-case ≤ 70 ms" was not a bound the crate can support — `os_wait_timeout` loops
  `waitpid(WNOHANG)` + `thread::sleep`, and `thread::sleep` guarantees a minimum, not a maximum,
  so under scheduler pressure the total can exceed 70 ms and cross the ~100 ms budget. §1.3.3 now
  states the honest contract: **bounded in requested durations on every path, ~70 ms under normal
  scheduling, degrading with load, never unbounded** — the guaranteed property being the absence
  of an *indefinite* hang. Option (b) was not re-litigated; this finding does not bear on it.
  MINOR 1: `"cancelling…"` is delivered immediately but not necessarily *painted* — `set_status`
  passes through status-slot arbitration and can be history-only under a stricter
  `messages_min_kind` or a more severe occupant; the cancellation itself is unaffected. MINOR 2:
  the detached-child residual is **one per affected run**, not one overall — `apply_filter_done`
  clears `filter_in_flight` when the worker returns, so repeated cancel/timeout runs hitting the
  20 ms miss branch each leave one, the same accumulation shape already acknowledged for writer
  threads. Three sites outside §1.3.3 that still asserted hard numbers were brought into line.
- 2026-07-19 (rev 6) — **Codex gate round 5 folded; both accepted.** (Round 5 verified `ReapGuard`
  sound at the crate source.) IMPORTANT: `REAP_GRACE` ≈ 200 ms contradicted this spec's own
  "< ~100 ms" cancel claim, since the guard's grace is spent before `run_subprocess` returns.
  Grounded the judgment first in `input.rs`/`jobs_apply.rs`: Esc **acknowledgment** is immediate
  and not gated on this function (`filter_in_flight.take().cancel()` + "cancelling…" on the main
  thread), but the **terminal status** is gated on `FilterDone`, i.e. on this function returning —
  so the budget governs how long "cancelling…" lingers, a real visible transient. Chose **option
  (a)**: `REAP_GRACE = 20 ms` applied uniformly, worst case ≤ 50 ms (poll cadence) + ~0
  (signals) + ≤ 20 ms (grace) = **≤ 70 ms**, inside the law with 30 ms margin, arithmetic shown in
  the new **§1.3.3** and the number defined in exactly one place. Rejected (b) cancel-specific
  skip — it buys no latency (a) doesn't, while making a routine Esc routinely orphan a child,
  and adds a second contract to keep consistent; rejected (c) documenting ~250 ms — a 20 ms grace
  meets the stated law at no real cost, so raising the documented number to preserve an arbitrary
  constant is backwards. Scattered inline latency claims now defer to §1.3.3. MINOR: withdrew the
  overstated "no panic source after spawn" argument — row 11 is safe **because of RAII**, and its
  real precondition is now an explicit **ordering constraint in §1.3.2**: the `Popen` must enter
  `ReapGuard` immediately after `Popen::create`, before `communicate_start`, the writer spawn, or
  any other panic-capable work.
- 2026-07-19 (rev 5) — **Codex gate round 4 folded; all three accepted, none disputed.** (Round 4
  independently re-probed T5/T6 and the corrected T7 on this machine and confirmed both, incl.
  the 1 MiB writer-block assumption.) IMPORTANT: phase 2's `Err(_) => break ExitStatus::Undetermined`
  left `child_state == Running`, so `Popen::drop` could still block there — verified at the
  source (`Popen::waitpid` sets `Finished` only on a successful reap or `ECHILD`; every other
  errno returns `Err(e)` with the state unchanged), contradicting rev 4's "Drop can no longer
  block on any path." Rather than patch the named arm — which would have been the fourth
  consecutive round of that — the guarantee is now **structural**: a `ReapGuard(Popen)` RAII
  wrapper whose `Drop` does `poll` → (terminate, kill, bounded `wait_timeout(REAP_GRACE)`) →
  `detach` if unconfirmed, so at least one of `detached`/`!Running` is true before `Popen::drop`
  runs. §1.3.1 gained an exhaustive **re-walk table of all 11 ways control leaves the function**,
  including the unwind path (row 11) that no per-return-statement spelling could cover and which
  the RAII shape now handles for free. MINOR 1: deleted the stale invalid T7 command from §1.3.6,
  which still showed the disproved `sleep 600 & exit 0` shape and would have let a broken
  implementation pass the join-regression check; the section now carries the probe-verified
  command plus an explicit warning about the trap. The rev-3 History entry is annotated so its
  false claim cannot be read as current. MINOR 2: "byte-identical" softened to "same guard
  structure" for both phase-1 exclusions, acknowledging that stdin no longer flows through
  `communicate_start` and early returns now pass through `ReapGuard`.
- 2026-07-19 (rev 4) — **Codex gate round 3 folded; all three accepted, none disputed.**
  CRITICAL: rev 3's T7 was **hollow** — `/bin/sh` is bash here and a non-interactive background
  job takes fd 0 from `/dev/null`, so `sleep 600 & exit 0` never held the filter's stdin; a
  join-restored implementation would have **passed** the test written to catch the round-2
  Critical. Re-probed empirically (`readlink /proc/<pid>/fd/0` against a fifo standing in for the
  stdin pipe): rev 3's shape → *nobody* holds stdin; corrected shape
  `exec 3<&0; exec >/dev/null 2>/dev/null; sleep 600 <&3 & exit 0` → the **grandchild** holds it
  while the direct child exits 0. Applied the same suspicion to T5/T6 as instructed and probed
  them too — they survive, because their `sleep` is the foreground `exec`-replaced child rather
  than a background job. Probe method, command, and results table recorded in **§1.4.1**, with
  the rule that the probe (not shell reasoning) is the acceptance criterion and must be re-run on
  the implementing machine. IMPORTANT: the `Drop` row claimed a failed `kill()` implies `ESRCH`;
  the crate neither enforces nor inspects that (`kill()` leaves `child_state` `Running`), so the
  promised bound did not exist — and worse, this design's writer thread owning stdin removes the
  incidental EOF that today's `Communicator` drop provides, making `Drop`'s wait *more* reachable
  than today. Replaced the argument with a mechanism (**§1.3.2**): bounded `wait_timeout(REAP_GRACE)`
  after kill, then `child.detach()` if unconfirmed, so `Popen::drop` can never wait on any path —
  stronger than today's behaviour, established by construction. MINOR 1: §1.3.1 now states its
  scope (blocking calls on `run_subprocess`'s **return path**) and explicitly lists the detached
  writer's `write_all` as deliberately unbounded but off that path. MINOR 2: corrected the
  `filter_in_flight` claim — it bounds in-flight *filters*, not live *writers*, and the prompt
  success path can return while a writer is still blocked. MINOR 3: added **§1.4.2**, an
  exit-path coverage table marking each path covered-existing / added / deliberately-not with
  reasons (phase-1 timeout and cancel are unchanged code; the read-error `Spawn` arm has no
  injection seam and gets harder to reach; `Drop`-after-kill-failure is defused structurally
  rather than tested).
- 2026-07-19 (rev 3) — **Codex gate round 2 Critical folded; accepted, not disputed.** Rev 2's
  "join after a confirmed reap" was still unbounded: reaping the direct child says nothing about
  **descendants**, and `sh -c "exec >/dev/null 2>/dev/null; sleep 600 & exit 0"` was believed to
  leave a grandchild holding the inherited stdin read end — **that command does NOT do so; see
  rev 4, which disproved it by probe and corrected the shape** — hanging the join **on the
  success path**
  after all timeout/cancel checks have stopped — a regression against today's shipped code,
  which returns at the deadline. Verified: `PopenConfig::default()` has `setpgid: false`, pipe
  ends reach the child via `dup2`, Unix pipe fds inherit across fork/exec absent `CLOEXEC`, and
  `waitpid(pid, WNOHANG)` speaks only for `pid`. **The join is removed entirely — the writer is
  never joined on any path** — and it turns out to have had no purpose: stdout/stderr were
  already drained to EOF and the status already obtained, so the writer's completion feeds no
  result. Added **§1.3.1**, an exhaustive enumeration of every blocking operation with its bound
  and what that bound rests on; writing it surfaced a call *neither* gate round had raised —
  `Popen::drop → self.wait()` — which is bounded in practice (SIGKILL precedes every early
  return; the success path leaves `child_state` `Finished` so `Drop` does not wait) and is
  unchanged from today, including the fact that `Drop`'s `self.stdin = None` fallback is
  *already* a no-op in shipped code because `communicate_start` takes stdin first. Added **T7**
  (descendant-holds-stdin, asserting a *prompt success* — a different assertion shape from
  T5/T6's error paths, since no deadline remains to rescue that path). The gate's meta-point is
  recorded in the risks section: two rounds were lost to the same class, each on a call the
  prior revision had not enumerated, which is why the enumeration is now a spec artifact.
- 2026-07-19 (rev 2) — **Codex gate round 1 findings folded; all four accepted, none disputed.**
  CRITICAL: the writer-thread redesign dropped timeout/cancel enforcement once stdout/stderr hit
  EOF, so a child that closes its outputs and keeps running (`sleep 600`) would block the
  blocking `child.wait()` indefinitely — the first draft's "timeout semantics are unchanged" and
  "wait() is safe after drain" claims were false. Fixed with a **phase-2 bounded reap loop**
  (`Popen::wait_timeout(slice)` under the same cancel/deadline checks as the drain loop); §1.3
  now carries a per-exit-path coverage table. IMPORTANT: the join-safety argument was circular
  (it assumed the very `wait()` it justified would return) — the join is now bounded by a
  confirmed reap established *before* the join. IMPORTANT: T1–T4 all use exiting children and
  would have passed a broken implementation — added **T5** (closes outputs, sleeps 600 s, 1 s
  timeout, bounded harness wait → `Timeout`) and **T6** (same child, cancel at ~200 ms →
  `Cancelled`). MINOR: §2.2's "every future test that forgets a seam" overstated the mechanism —
  `cfg(test)` covers the lib test binary and the in-source `e2e` module only, not
  `wordcartel/tests/*` integration binaries; wording tightened and the boundary stated in both
  places. The writer thread itself was this author's mechanism, not a ratified decision, and it
  survives — the defect was the reaping step, not the stdin split.
- 2026-07-19 — authored (Fable), after an independent grounding pass over `filter.rs`, the
  vendored `subprocess 0.2.15` source, `swap.rs`/`state.rs`/`recovery.rs`/`session_restore.rs`/
  `prompts.rs`/`recents.rs`/`e2e.rs` test surfaces, and the five effort maps. No ratified
  decision found wrong; D3's `LAST_GOOD` mechanism refined within its ratified choice (§2.4).
