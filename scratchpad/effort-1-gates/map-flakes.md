# Flake mapping: H29 (editor recovery-snapshot race) and H20 (filter stderr test)

Machine: 32 logical cores (`nproc` = 32). Working tree: `main`, clean, no edits made.
Wall-clock budget used: ~23 min. All runs used the pre-built `wordcartel` lib test
binary (`target/debug/deps/wordcartel-179fab54caed2816`, from `cargo build --workspace --tests`)
invoked directly, except the "full workspace" condition which used `cargo test --workspace`.
Scratch logs: `/home/jkeim/.claude/jobs/1098ada9/tmp/*.log`.

## 1. Measurements

### 1a. Isolated (`--exact`, single test per process, `--test-threads=1`)
Commands (300 iterations each, separate process per iteration):
```
$BIN editor::tests::undo_and_redo_refresh_the_recovery_snapshot --exact --test-threads=1
$BIN filter::tests::run_filter_non_zero_exit_carries_stderr --exact --test-threads=1
```
- editor test: **0/300 failed**
- filter test: **0/300 failed**

### 1b. Full lib binary (all 1768 tests in-process), default `--test-threads` (=32, num_cpus)
Command: `$BIN` (no thread flag), repeated 60×.
- editor test: **3/60 failed**
- filter test: **4/60 failed**

### 1c. Full `cargo test --workspace`, repeated 5×
Command: `cargo test --workspace`.
- editor test: **0/5 failed**
- filter test: **1/5 failed**
(Small sample by design — each run costs ~9s wall but exercises the same in-process
concurrency as 1b for the crate that owns both tests; sampled 5 to keep budget for
the contention runs.)

### 1d. Deliberate CPU contention: 6 rounds × 6 concurrent full-binary processes (36 total),
all launched together per round (`$BIN & ... ; wait`), oversubscribing the 32 cores
(≈192 test threads contending at once across the 6 concurrent processes).
- editor test: **0/36 failed**
- filter test: **14/36 failed** (≈39%)
- (one co-occurring, unrelated failure also observed in this window: `config::tests::clipboard_provider_unknown_warns_and_defaults_auto`, and separately `prompts::tests::the_clean_recovery_modal_names_kept_recoverable_files` — neither is a target test, noted only because they landed in the same log grep)

### 1e. `--test-threads` sweep, full lib binary, default-load (no extra contention)
| threads | n  | editor fail | filter fail |
|---------|----|-------------|--------------|
| 1       | 15 | 0/15        | 0/15         |
| 4       | 25 | 0/25        | 0/25         |
| 32 (default) | 60 | 3/60   | 4/60         |
| 128     | 25 | 1/25        | 0/25         |

No isolated (`--test-threads=1`) run produced a failure of either test, at any sample size tried (300 single-test + 15 full-suite). Every observed failure of the editor test occurred at `--test-threads` ≥ 32 (i.e., default or oversubscribed); every observed failure of the filter test occurred either at default threads or under added process-level contention (1d), and its failure rate rose sharply (0/25 at t=4, ~7% at default t=32, ~39% under 6-way concurrent full-suite contention).

## 2. Captured panic messages (verbatim)

### editor::tests::undo_and_redo_refresh_the_recovery_snapshot (3 instances captured, all under default/high thread count, none under contention or low thread counts)
```
thread 'editor::tests::undo_and_redo_refresh_the_recovery_snapshot' panicked at wordcartel/src/editor.rs:1834:9:
assertion `left == right` failed: redo refreshes the recovery snapshot
  left: Some("hello\nabc")
 right: Some("Xabc\n")
```
```
thread 'editor::tests::undo_and_redo_refresh_the_recovery_snapshot' panicked at wordcartel/src/editor.rs:1830:9:
assertion `left == right` failed: undo refreshes the recovery snapshot
  left: Some("## H\nbody\n")
 right: Some("abc\n")
```
```
thread 'editor::tests::undo_and_redo_refresh_the_recovery_snapshot' panicked at wordcartel/src/editor.rs:1830:9:
assertion `left == right` failed: undo refreshes the recovery snapshot
  left: Some("Zabc\n")
 right: Some("abc\n")
```
Also one instance (t=128 run) at line 1834 with `left: Some("the quick ")`. In every captured case, `left` (the observed LAST_GOOD content) is text that belongs to some *other* test's buffer, not to this test's own `"abc\n"`/`"Xabc\n"` content — i.e. the value read out of `LAST_GOOD` was written by a concurrently-running unrelated test between this test's own write and its own read.

### filter::tests::run_filter_non_zero_exit_carries_stderr (18 instances captured across 1b/1d/1c, all identical)
```
thread 'filter::tests::run_filter_non_zero_exit_carries_stderr' panicked at wordcartel/src/filter.rs:490:22:
expected NonZero, got Err(Spawn("Broken pipe (os error 32)"))
```
Every single captured filter failure (4 from 1b, 13 from 1d, 1 from 1c — 18 total, 0 exceptions) hit the same line and the same message: the **outer `other => panic!("expected NonZero, got {other:?}")` fallback arm** (filter.rs:490), not either of the two `assert!` lines inside the `NonZero` match arm (`code.contains('3')` at line 487, `stderr.contains("boom")` at line 488). The `code`/`stderr` sub-assertions were never observed to fire in any of the 415 filter-test executions collected (300 isolated + 60 full-lib default + 25 t4 + 25 t128 + 36 contention + 5 workspace... — every failure was the `Spawn("Broken pipe")` outer-arm panic).

## 3. Mechanism — editor test / `recovery::LAST_GOOD`

`recovery.rs:8`: `pub static LAST_GOOD: Mutex<Option<(Option<PathBuf>, ropey::Rope)>> = Mutex::new(None);` — one process-global mutex, not per-test/per-editor state.

Writers (production code, unconditional, called from *any* test that edits a buffer):
- `Editor::apply` → `recovery::record_snapshot` (editor.rs:362)
- `Editor::undo` → `recovery::record_snapshot` (editor.rs:373)
- `Editor::redo` → `recovery::record_snapshot` (editor.rs:397)

Reader in production code: `recovery::dump_on_panic` (recovery.rs:53-54, `try_lock`), called only from the panic hook in `term.rs:115` — not exercised by these tests.

Reader/writer in test code: only `editor::tests::undo_and_redo_refresh_the_recovery_snapshot` (editor.rs:1820-1835) directly touches `LAST_GOOD` (via `.lock()`, not `.try_lock()`) to assert its contents after its own `undo()`/`redo()`. No test resets, clears, or takes exclusive ownership of `LAST_GOOD` before or after use — the in-code comment above the test ("Serialize the shared LAST_GOOD latch for this thread by taking it, seeding a sentinel, dropping the guard, then acting") describes a serialization strategy that is **not present in the actual test body**; the test simply calls `apply`/`undo`/`redo` on its own local `Editor` and reads the global mutex with no isolation from other threads.

Because `record_snapshot` is invoked from `apply`/`undo`/`redo` unconditionally, essentially every test in the `wordcartel` lib crate that performs an edit (a large fraction of the suite — direct literal callers alone span at least 8 source files, before accounting for helper/command dispatch paths that call `apply` transitively) writes into `LAST_GOOD` as a side effect. Rust's default test harness runs library tests in-process on a thread pool sized to `--test-threads` (default = logical core count). `undo_and_redo_refresh_the_recovery_snapshot`'s own writes (via its `apply`, then `undo`) and its own reads (`.lock()` after `undo`, then after `redo`) are not atomic with respect to the rest of that thread pool: any other concurrently-scheduled test's `apply`/`undo`/`redo` can interleave between this test's write and its own subsequent read, replacing `LAST_GOOD`'s contents with unrelated buffer text. The captured panic values (`"hello\nabc"`, `"## H\nbody\n"`, `"Zabc\n"`, `"the quick "`) are exactly this: content belonging to other tests, not to this test's `"abc\n"`/`"Xabc\n"`.

One-line mechanism summary: **`undo_and_redo_refresh_the_recovery_snapshot` reads a process-global `Mutex<Option<(...,Rope)>>` (`LAST_GOOD`) that any other concurrently-running test's `apply`/`undo`/`redo` can overwrite between this test's own write and its own read, with no synchronization/isolation in the test; it only manifests at `--test-threads` > 1 (never observed isolated, at t=1, or at t=4 in this sampling; observed at default/high thread counts) and was not observed to be sensitive to added CPU contention beyond what multi-threaded in-process concurrency already provides.**

## 4. Mechanism — filter test / subprocess spawn under load

`filter::tests::run_filter_non_zero_exit_carries_stderr` (filter.rs:472-492) spawns `sh -c "echo boom >&2; exit 3"` via `run_filter` → `run_subprocess` (filter.rs:111-289), with `timeout: Duration::from_secs(10)` set directly in the test's `FilterSpec` (filter.rs:482); this is the "~10s subprocess spawn budget" — it is a per-test literal, not a shared/configurable constant, and it bounds the poll loop's deadline (`let deadline = Instant::now() + timeout;`, filter.rs:168), not the `Popen::create` spawn call itself.

`run_subprocess` calls `child.wait()` once the poll loop observes stdout/stderr EOF (filter.rs:275): `let status = child.wait().unwrap_or(ExitStatus::Undetermined);`. `child.wait()` here is `subprocess::Popen::wait()`, which itself can return `Err` (e.g. on an OS-level `waitpid` failure) — `.unwrap_or(ExitStatus::Undetermined)` silently converts any such error into the `Undetermined` variant rather than propagating it. `status` is then matched (filter.rs:277-287):
- `ExitStatus::Exited(0)` → `Ok` (not applicable here, exit code is 3)
- `ExitStatus::Exited(code)` → `Err(NonZero { code: code.to_string(), stderr })` — the path the test's two `assert!`s (`code.contains('3')`, `stderr.contains("boom")`) are written to check
- `other` (covers `Undetermined`, `Signaled`, etc.) → **also** `Err(NonZero { code: format!("{other:?}"), stderr })`

So an undetermined/failed `child.wait()` still produces `FilterError::NonZero`, just with `code == "Undetermined"` instead of `"3"` — which would fail the *first* `assert!(code.contains('3'))`, not the stderr one. **But this was not what fired in any captured run.** All 18 captured failures instead came from `run_subprocess`'s earlier bail-out path: `Popen::create(...)` succeeded, but a later I/O operation on the child's pipes returned `std::io::ErrorKind::Other`-class error ("Broken pipe (os error 32)"), which the `Err(ce) => { ... } else { return Err(FilterError::Spawn(ce.error.to_string())) }` branch (filter.rs:266) converts into `FilterError::Spawn`, not `FilterError::NonZero`, entirely bypassing the `wait()`/`ExitStatus` logic above. Back in the test, `run_filter` returns `RunResult::Err(FilterError::Spawn(...))`, which does not match the `RunResult::Err(FilterError::NonZero { .. })` arm at all, so control falls to `other => panic!("expected NonZero, got {other:?}")` (filter.rs:490) — the outer catch-all, not either inner `assert!`.

One-line mechanism summary: **every captured failure is a subprocess-pipe I/O error ("Broken pipe (os error 32)") converted to `FilterError::Spawn` inside `run_subprocess`'s poll loop (filter.rs:266), which the test's `match` never anticipated — it hits the outer `panic!("expected NonZero, got {other:?}")` fallback, not the `code`/`stderr` `assert!`s inside the `NonZero` arm; failure rate rose sharply under added process-level CPU contention (0/25 at low concurrency to ~39% under 6-way concurrent full-suite load), consistent with a load-sensitive race in subprocess/pipe teardown rather than pure randomness, and was reproduced in-process (not only under separately-invoked contention) at default `--test-threads`.**

## 5. Commands run (verbatim, condensed)

```
nproc                                                     # => 32
cargo build --workspace --tests
cargo test --workspace --no-run --message-format=json     # locate test binaries

BIN=target/debug/deps/wordcartel-179fab54caed2816

# 1a isolated, 300x each
$BIN editor::tests::undo_and_redo_refresh_the_recovery_snapshot --exact --test-threads=1
$BIN filter::tests::run_filter_non_zero_exit_carries_stderr --exact --test-threads=1

# 1b full lib binary, default threads, 60x
$BIN

# 1c full workspace, 5x
cargo test --workspace

# 1d contention: 6 rounds of 6 concurrent full-binary processes
for r in 1..6; do for p in 1..6; do $BIN & done; wait; done

# 1e thread sweep
$BIN --test-threads=1     # 15x
$BIN --test-threads=4     # 25x
$BIN --test-threads=128   # 25x
```
