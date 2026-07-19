# Test & Gate Landscape Map — groundwords / wordcartel

Generated on-branch `main`, clean tree, read-only survey. Facts only.

## 1. Every test target

### Workspace members
`wordcartel-core`, `wordcartel`, `wordcartel-nlp` (Cargo.toml `[workspace] members`).

### Unit suites (in `src/`, `#[cfg(test)] mod tests` / doctests)
- `wordcartel` lib: 1767 passing tests + 1 ignored in the single `--lib` binary (per-file counts of
  `#[test]` occurrences vary; the crate has ~65 source files under `src/`, most carrying a co-located
  `mod tests`).
- `wordcartel-core` lib: 0 `#[test]`-tagged unit tests reported by `cargo test` in this run's `--lib`
  target (its behavior tests live in `tests/` instead — see below); doctests: 1 passing.
- `wordcartel-nlp` lib: 48 passing tests, 0 ignored (files `classify.rs` 31 `#[test]` occurrences,
  `lib.rs` 17).

### Files under `tests/` (integration targets), one cargo test binary each

`wordcartel-core/tests/`:
| file | `#[test]` count | kind |
|---|---|---|
| `block_roles_integration.rs` | 1 | behaviour (block-role rendering E2E) |
| `block_tree_oracle.rs` | 42 | behaviour/property (parser oracle + proptest regressions, port of an external spike's oracle suite) |
| `integration.rs` | 1 | behaviour/property (kernel law: edit+undo round-trip, proptest) |
| `render_integration.rs` | 2 | behaviour (cursor motion across logical lines) |

`wordcartel/tests/`:
| file | `#[test]` count | kind |
|---|---|---|
| `backlog.rs` | 7 | **guard/invariant** |
| `edit_seam.rs` | 2 | **guard/invariant** |
| `fs_chokepoint.rs` | 10 | **guard/invariant** |
| `harper_ls_integration.rs` | 1 | behaviour, real-binary; `#[ignore]`-gated |
| `harper_ls_probe.rs` | 1 | behaviour, real-binary; `#[ignore]`-gated |
| `module_budgets.rs` | 5 | **guard/invariant** |
| `sentence_differential.rs` | 4 | behaviour/differential (compares our sentence detector against `repar`'s ventilate) |

Observed `cargo test` binary tallies for the `tests/` targets in one run: block_roles_integration=1,
block_tree_oracle=42, integration=1, render_integration=2, backlog=7, edit_seam=2, fs_chokepoint=10,
harper_ls_integration=0 run/1 ignored, harper_ls_probe=0 run/1 ignored, module_budgets=5,
sentence_differential=4 — all matching the static `#[test]` counts above.

### Guard/invariant tests — full list and what each enforces

- **`wordcartel/tests/module_budgets.rs`** — asserts production line-count of the shell's dispatch
  hubs (`app.rs`, `render.rs`, etc.) stays under fixed per-file budgets; an anti-regrowth tripwire so
  a dispatcher can't silently regrow into a god-object (CLAUDE.md "Module structure").
- **`wordcartel/tests/backlog.rs`** — renders `backlog.toml` into `BACKLOG.md` via one `render()` used
  for both generation and verification, and checks the rendered file is byte-identical to what's
  committed, plus schema/bijection invariants I1–I7; keeps the backlog dashboard from drifting out of
  sync with its source of truth.
- **`wordcartel/tests/fs_chokepoint.rs`** — scans production source text for raw filesystem access
  (`std::fs`, inherent `Path` methods, and in-crate wrapper-bypass) outside an explicit allow-list, so
  every durable/read/enumerate/stat call in the `wordcartel` crate is forced through the `fsx::Fs` seam.
- **`wordcartel/src/test_support.rs`-adjacent `wordcartel/tests/edit_seam.rs`** — heuristic source scan
  asserting `Buffer::apply` (the sole raw-mutation channel post the universal-edit-chokepoint migration)
  is never reached through an accessor-chain one-liner (`active_mut().apply(..)`) in production, only via
  the sanctioned two-statement pattern.

(Confirmed by direct search: no other `tests/*.rs` file or `src/*.rs` file matches this
structural/anti-regrowth-guard shape; the remaining suites are behaviour, property, or differential
tests over program logic, not over the codebase's own shape/process.)

## 2. Total workspace test count and wall-clock

Two consecutive `cargo test --workspace` runs were executed.

- **Run A** (first invocation, output truncated by a `tail`, but the failure was captured in full):
  `wordcartel` lib: **1766 passed, 1 FAILED, 1 ignored**, finished in 4.20s. Failure:
  `editor::tests::undo_and_redo_refresh_the_recovery_snapshot` —
  `assertion left == right failed: undo refreshes the recovery snapshot; left: Some("body\n"), right: Some("abc\n")`.
- **Run B** (immediately after, no code changes): **all green**, same test set, `wordcartel` lib
  finished in 4.30s.

Summing every `test result:` block from run B: 1767+7+2+10+0+0+5+4+306+1+42+1+2+48+12+12+2 passed =
**2221 passed, 0 failed, 5 ignored** (2226 total across every binary: `wordcartel` lib+doctests,
`wordcartel-core` lib+doctests+4 `tests/` binaries, `wordcartel-nlp` lib, `wordcartel-core/fuzz`
excluded — fuzz targets are not part of `cargo test`). Wall-clock for the whole `cargo test --workspace`
invocation (build + run all targets), per `time`: **run A ~4.4s to first failure (build cached);
run B real ≈9.2s** (`9.211 total`, `29.55s user / 6.06s system` — parallel across cores).

**This flip (fail → pass, no source change) is itself evidence for item 3**: `editor::tests::undo_and_redo_refresh_the_recovery_snapshot` reads `recovery::LAST_GOOD`, a process-wide
`Mutex`, and its assertion content (`"body\n"` vs `"abc\n"`) is consistent with a different, concurrently-running test's `record_snapshot` write winning the race under `cargo test`'s default parallel
test execution.

## 3. Shared mutable state across tests (production `static`/`OnceLock`/`Atomic*`/`thread_local`)

| Item | Location | What it is | Reset by any test? | Concurrent-test interference possible? |
|---|---|---|---|---|
| `recovery::LAST_GOOD` | `wordcartel/src/recovery.rs:8` | `pub static Mutex<Option<(Option<PathBuf>, Rope)>>` — last-good snapshot for panic-dump | No explicit reset found | **Yes — observed**: see item 2's run-to-run flip on `editor::tests::undo_and_redo_refresh_the_recovery_snapshot` |
| `plugin::INTERN_POOL` | `wordcartel/src/plugin/mod.rs:127` | `static Mutex<Option<HashSet<&'static str>>>` — permanent leak-once string interner | No reset (can't be — leaks are permanent by design); test reader `intern_pool_contains` uses membership-of-a-unique-string, not a count, specifically because a whole-pool-size comparison was "reproduced empirically" to drift under parallel tests | Yes, by the code's own comment: a full-suite run "saw the count drift by 2 during a single guardrail test's window" |
| `file_browser::LISTING_EPOCH` | `wordcartel/src/file_browser.rs:397` | `pub(crate) static AtomicU64`, starts at 1, monotonically bumped | No reset found | Plausible (process-global monotonic counter observed by any test asserting exact epoch values) but not verified failing |
| `fsx::TEMP_SEQ` | `wordcartel/src/fsx.rs:359` | `static AtomicU32` — disambiguates generated temp filenames | No reset (by design — sequence should never repeat) | Low risk (used to avoid collisions, not asserted on directly) |
| `cursor_style::restore::EVER_WROTE` | `wordcartel/src/cursor_style.rs:67` | `static AtomicBool` — "process-global restore latch" for the panic hook | No reset; **tests explicitly document they cannot rely on its initial value** — comment: "process-global static shared across all tests in the binary, so assert only the monotonic (false→true) transition, never that it is false at start" | Yes, acknowledged in-repo; tests are written defensively (order-independent) specifically because of this |
| `term::HOOK_INSTALLED` | `wordcartel/src/term.rs:100` | `static Once` inside `install_panic_hook()` — guards one-time panic-hook installation | N/A (Once by design) | Low (idempotent installer) |
| `panicx::CAUGHT_GUARD` | `wordcartel/src/panicx.rs:10` | `thread_local! Cell<bool>` — marks "a `catch` is active on this thread" | Reset via RAII `GuardReset` on drop within the same call | Low if thread-local semantics hold; cargo test's worker-thread pool reuse across tests is a latent factor not verified here |
| `export::probe_pandoc`'s `CACHE` | `wordcartel/src/export.rs:45` | function-local `static OnceLock<bool>` — caches whether `pandoc` is on PATH | No reset | Low (idempotent probe, environment-derived) |
| `derive::LAYOUT_RUNS`, `HEADING_STARTS_WALKS`, `bench_spans::PHASE_SPANS` | `wordcartel/src/derive.rs:26,31,49` | `#[cfg(test)] thread_local!` counters/spans for perf-instrumentation assertions (R1 invariant: "a no-folds keystroke must not increment this") | Not globally reset; scoped to `#[cfg(test)]` only, so out of production's reach | thread_local, but cargo test's OS-thread pool is reused across tests, so cross-test leakage on the same worker thread is a latent risk class, not independently verified |
| `fold::SECTIONS_WALKS`, `NORMALIZE_CARET_WALKS` | `wordcartel/src/fold.rs:11-17` | `#[cfg(test)] thread_local!` walk counters, same R1-guard purpose as above | Same as above | Same as above |
| `plugin::load::LOAD_BUDGET_OVERRIDE` | `wordcartel/src/plugin/load.rs:280-285` | `#[cfg(test)] pub(crate) thread_local! Cell<Option<Duration>>` — test-only override of the exec-phase time budget | Presumably set/unset per test (not verified for every call site) | thread_local, same latent pool-reuse risk |
| `plugin::host::FAIL_NEXT_COMMIT_WRITE` | `wordcartel/src/plugin/host.rs:36-39` | `#[cfg(test)] pub(crate) thread_local! Cell<bool>` — fault-injection latch for the next commit write | Presumably set/unset per test | Same as above |
| Assorted per-module `SEQ`/`N` `Atomic*` | `file.rs:238`, `editor.rs:67`, `config.rs:939`, `settings.rs:631`, `session_restore.rs:231`, `save.rs:481`, `file_browser_commit.rs:465`, `jobs_apply.rs:420`, `state.rs:166`, `swap.rs:578`, `app.rs:949`, `fsx.rs:483,687`, `plugin/load.rs:881` | All are **function-local `static` inside `#[cfg(test)]` blocks**, used only to generate unique temp-file names/ids within that module's own tests | N/A — monotonic-by-design counters, not asserted on | Low; scoped per-module, not cross-module shared |

Given example `recovery::LAST_GOOD` has at least **2 close production-code siblings that are genuinely
cross-test-observable and undocumented-as-racy in behavior** (`plugin::INTERN_POOL`,
`file_browser::LISTING_EPOCH`), plus **1 sibling whose race is known and defused by writing
order-independent assertions** (`cursor_style::restore::EVER_WROTE`), plus a handful of lower-risk
process-wide latches (`Once`, `OnceLock` cache). The `#[cfg(test)]`-only `thread_local!` fault-injection
seams (`LOAD_BUDGET_OVERRIDE`, `FAIL_NEXT_COMMIT_WRITE`) are a distinct class — not reachable from
production, but still process/thread-pool-scoped state a test could in principle leave armed.

## 4. Tests touching a real shared resource outside their own tempdir

- **`wordcartel/src/swap.rs` `mod tests`** — multiple tests call `state_dir()` directly, which resolves
  to `$XDG_STATE_HOME/wordcartel` or falls back to the REAL `~/.local/state/wordcartel`
  (`swap::state_dir()`, `swap.rs:31-42`, calls `dirs::state_dir()`/`dirs::home_dir()` with no test
  seam) and does `create_dir_all` + `chmod 0700` on it:
  - `swap_path_named_is_deterministic_and_in_state_dir` (`swap.rs:627`) — asserts a path `starts_with(state_dir().unwrap())`; no write, but does provision the real dir.
  - `state_dir_is_0700` (`swap.rs:646`) — asserts the mode of the real state dir.
  - `write_atomic_writes_0600_and_roundtrips_via_parse` (`swap.rs:764`) — writes `test-write-<pid>.swp` into the real state dir; **does** `remove_file` at the end.
  - `enumerator_scan_includes_discard_silently_excludes_prompt` (`swap.rs:1131`) — writes fixture swap files into the real state dir via `make_doc_with_swap`; comment: "End-to-end through the real state dir... the shared state dir carries litter." Restores via per-file `remove_file` at the end, not directory-wide.
  - `kept_recoverable_count_reports_what_the_sweep_deliberately_spares` (`swap.rs:1152`) — same pattern; comment explicitly says: *"the shared state dir may hold unrelated diverged swaps from other tests or a prior real session"* and *"`remove_dir_all(&dir)` here would delete the developer's real state dir"* (comment truncated at the read boundary, but the intent — deliberately NOT wiping the directory — is stated). Cleans up only its own named fixtures.
- **`wordcartel/src/recovery.rs` `mod tests`** — `write_dump_writes_named_0600_file_with_body` (`recovery.rs:70`) calls `crate::swap::state_dir().unwrap()` and writes a `recovered-notes.md-*.md` file into the real state dir. Restoration not confirmed read past the shown excerpt.
- **`wordcartel/src/session_restore.rs` `mod tests`** — `persist_session_stamps_the_active_documents_id` (`:536`), `persist_session_captures_scratch_even_when_active_unnamed` (`:556`), `persist_session_clears_stale_scratch_when_oversized` (`:573`) all call `persist_session_for_test(...)`, a `#[cfg(test)]` thin wrapper (`session_restore.rs:191`) over the real `persist_session`, which ends `let _ = session.save();` → `SessionState::save()` (`state.rs:104`) → `self.save_in(&crate::swap::state_dir()?)`. This writes the developer's real `$XDG_STATE_HOME/wordcartel/session.toml` (or `~/.local/state/wordcartel/session.toml`). **No restore/cleanup of that file is done by these three tests.**
  - The repo has already partially fixed this class once: `recents.rs`'s `open_recent_in` (`recents.rs:82-89`, `#[cfg(test)]`) is a directory-injectable sibling of `open_recent`, added specifically because — quoting the comment — *"`open_recent` was the last session-store reader with no directory seam, so a test exercising it had to read — and therefore restore — the DEVELOPER'S real `$XDG_STATE_HOME/wordcartel/session.toml`. Load-then-restore is not exclusive: the `persist_session_for_test` tests write that same file on a sibling thread of the same process, so an interleave could silently discard their write. Tests point this at a temp dir instead and touch nothing ambient."* — i.e. the repo's own comments confirm both the hazard and that `persist_session_for_test` itself is the still-unfixed half of the same interleave risk.
- **Fixed-but-not-tempdir `/tmp/...` paths** (not `~`, not XDG, but a hardcoded `/tmp` rather than `tempfile::tempdir()`), each PID-namespaced and each doing manual pre/post `remove_dir_all`:
  - `wordcartel/src/search_ui.rs:394` — `diag_apply_selected_add_dict_writes_file_once_and_nudges_reload` uses `/tmp/wordcartel_adddict_<pid>`.
  - `wordcartel/src/diagnostics_run.rs:543` — `append_word_to_dict_creates_parent_dir` uses `/tmp/wordcartel_test_<pid>`.
  - Several `workspace.rs`/`file_browser.rs`/`render_status.rs`/`editor.rs`/`save.rs`/`render.rs` tests use literal strings like `/tmp/a.md`, `/tmp/notes.md`, `/tmp/wc-classify` purely as **`PathBuf` values in in-memory `Editor` state** (never written to real disk) — these do NOT touch the real filesystem and are not a resource-sharing concern.
- No test was found touching `~/.config` directly for writes; `dirs::config_dir()`/`dirs::home_dir()` reads in `config.rs` tests (`diagnostics_default_dictionary_is_not_none`, `dictionary_tilde_is_expanded`, `dictionary_bare_tilde_expands_to_home`) only assert path-string equality against the real home/config dir — they read the *value* `dirs::home_dir()`/`dirs::config_dir()` return but perform no I/O against it.

## 5. Existing test seams and helpers

**`wordcartel/src/test_support.rs`** (278 lines, `//! Shared #[cfg(test)] helpers for the shell's test modules (app::tests, e2e)`), all items `pub(crate)`:
- `TestClock` — deterministic virtual `Clock` impl (`now_ms()` returns a fixed value).
- `key_char(c)` — builds a printable-char `KeyEvent`.
- `press(code, mods)` — builds a `Msg::Input` key-press `Msg`.
- `test_fs()` — returns an `Arc<dyn Fs>` wrapping plain `RealFs`, for callers that need to supply a
  handle but exercise no fault behavior.
- `install_enabled_harper(e)` — installs an enabled Harper `RecordingProvider` into a bare test editor.
- **`FaultFs` / `FaultAt` / `FaultHandle`** — the shared fault-injecting `Fs` implementation ("promoted
  from `fsx.rs`'s private test mod, C5 Task 1... every migrated call site needs to inject faults from
  its OWN module's tests"). `FaultAt` variants: `Create`, `Write{after}`, `SetMode`, `Flush`, `Sync`,
  `Rename`, `SyncDir`, `ReadCapped`, `Stat`, `ListDir`, `RemoveFile`.
- `press_key_fb` / related file-browser keystroke helpers (added C5 Task 12, "reused by Tasks 13-26").

Consumers of the shared `FaultFs`/`FaultAt` (via `crate::test_support::{FaultAt, FaultFs}`):
`wordcartel/src/fsx.rs:606`, `wordcartel/src/swap.rs:938`, `wordcartel/src/file.rs:258`,
`wordcartel/src/save.rs:1073,1206`, `wordcartel/src/file_browser.rs:1134-1135`.

**Duplicate fault-injection `Fs` impls NOT going through `test_support::FaultFs`** — a second,
independently-hand-written `struct FailFs;` implementing the full `Fs` trait appears **three separate
times in the same file**, `wordcartel/src/settings.rs`:
- `settings.rs:838` — inside `save_overrides_surfaces_io_failure`.
- `settings.rs:916` — inside `save_failure_surfaces_io_error`.
- `settings.rs:952` — inside `save_failure_is_a_sticky_error_that_survives_a_later_info`.

All three `FailFs` definitions are byte-for-byte identical (same 8 trait methods, same bodies:
`create_excl` returns `Err(io::Error::other("boom"))`, `read_capped`/`stat`/`list_dir` delegate to
`RealFs`, `rename`/`sync_dir` are `unreachable!()`, `remove_file` is `Ok(())`). None of the three
reuses `test_support::FaultFs`/`FaultAt` (which already has a `FaultAt::Create` variant that would
cover this exact scenario) — each is a hand-rolled, module-local, fully-duplicated `impl Fs`.

Other `#[cfg(test)]`-local structures (not shared, not full `Fs` impls, so not counted as
`FailFs`-class duplicates but noted for completeness): `file_browser.rs:759-789` `CountingFs`
(wraps `RealFs` + an `AtomicUsize` call counter — a spy, not a fault injector, distinct purpose).

## 6. `#[ignore]`d tests

- `wordcartel/tests/harper_ls_probe.rs:37` — `#[ignore = "requires harper-ls on PATH; run with --ignored"]`.
- `wordcartel/tests/harper_ls_integration.rs:50` — `#[ignore = "requires harper-ls on PATH; run with --ignored"]`.
- `wordcartel/src/e2e.rs:2947` — `#[ignore = "release-only bench; run with --release --ignored (see comment)"]`.

(3 total; none unexplained.)

## 7. CI configuration

**None found.** No `.github/workflows` directory exists in the repo (confirmed: no `.github` directory
at all outside the `.claude/worktrees/*` copies, and no `.yml`/`.yaml` file anywhere in the tree outside
`target/` and `.claude/`). No `.gitlab-ci.yml`, `.circleci/`, `Jenkinsfile`, or other CI-equivalent
config was found either. **Everything the docs call a "gate" (`cargo test`, workspace clippy) is
presently enforced only by human/agent process (CLAUDE.md instructions to run it before merge), not by
any automated CI system.**

## 8. `scripts/smoke/run.sh`

High-level flow (88 lines, POSIX `sh`):
1. Pre-flight, builtins only: requires `tmux` on PATH and tmux `>= 3.0` (parses `tmux -V`); either
   check failing prints a one-line `smoke: SKIP — ...` and exits 0 (advisory: a skip is a clean exit).
2. Sets up a private, per-run tmux socket (`wcartel-smoke-$$`) and a per-run tempdir
   (`mktemp -d "${TMPDIR:-/tmp}/wcartel-smoke-run.XXXXXX"`), both exported to the per-check scripts;
   `trap cleanup EXIT` kills the tmux server and `rm -rf`s the run dir on exit.
3. **Builds the debug profile**: `cargo build -p wordcartel` (not release, not release-dist) —
   explicit comment: `# --- build (debug: S7 requires debug_assertions).` Cargo output is redirected to
   a log file under the run's tempdir and only `cat`'d to stderr on build failure.
4. Resolves the binary at `${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wcartel`.
5. Runs each `checks/s[1-9]-*.sh` (9 checks: `s1-startup-quit`, `s2-open-errors`, `s3-save-roundtrip`,
   `s4-dirty-quit-modal`, `s5-clipboard-osc52`, `s6-tiny-terminal`, `s7-panic-recovery`,
   `s8-kill-swap-recovery`, `s9-live-splash`) sequentially, printing `PASS <name>` / `FAIL <name>` per
   check, and on failure dumps that check's captured log inline.
6. Prints a one-line summary (`smoke: N/M PASS` or `smoke: FAIL <first> — advisory (N/M passed)`),
   appends it with a timestamp to the gitignored `scripts/smoke/.history`, and **exits 1 if any check
   failed** — but per CLAUDE.md the caller/process treats that exit code as advisory, not a merge
   blocker (this script itself does not know or enforce that; the exit code is a real 1).

Observed run history (`scripts/smoke/.history`, most recent 5 lines, all green): `9/9 PASS` at
`2026-07-19T09:39:00`, `10:36:34`, `10:41:59`, `10:46:09`, `10:48:34`.

Timing: not independently re-measured in this survey (would require running the tmux-driven suite);
the script's own comments describe it as building + running 9 sequential real-PTY-driven checks, i.e.
materially slower than the unit/integration suite (`cargo test --workspace` itself completed in ~9s).

## Docs-vs-tree cross-check (CLAUDE.md's stated gates)

- CLAUDE.md states: `cargo test` green + workspace clippy clean are merge GATEs; PTY smoke is
  mandatory-run/advisory-pass, NOT a gate; `cargo deny check` is a release-checklist step, NOT a merge
  gate. **The tree's actual mechanism for enforcing any of this is human/agent-run commands — there is
  no CI config anywhere that runs them automatically.** No disagreement was found between what
  CLAUDE.md claims about smoke/deny being advisory and what the scripts themselves do (`run.sh`'s exit
  code is real but the policy layer treating it as advisory lives entirely in CLAUDE.md prose, not in
  any enforcement code). The one concrete disagreement uncovered is empirical, not doc-vs-code: the
  claimed-green `cargo test` gate is **not deterministically green** — this survey's own two
  back-to-back runs produced fail-then-pass on `editor::tests::undo_and_redo_refresh_the_recovery_snapshot`
  with zero source changes between them, traceable to the shared `recovery::LAST_GOOD` global.
