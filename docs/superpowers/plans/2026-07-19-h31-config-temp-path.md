# H31 — implementation plan: `config.rs` shared-temp-path collision

**Spec:** `docs/superpowers/specs/2026-07-19-h31-config-temp-path-collision-design.md`
(committed `bf2ea20`, Codex-clean at round 3).
**Branch:** `effort-h31-config-temp-path` off `main` (`60be3d1`).
**Date:** 2026-07-19.

## Goal

`config::tests::files_type_filter_unknown_warns_and_defaults_documents` fails ~17% of whole-binary
test runs (measured 10/60 at default 32-thread libtest concurrency; 0/20 at `--test-threads=1`). It
is the only known source of red runs in the suite.

**Cause (established, spec §1):** three byte-identical test helpers in `wordcartel/src/config.rs` —
`load_files`, `load_clip`, `load_diag` — build a scratch path from only `std::env::temp_dir()`,
`std::process::id()`, and a caller-supplied `name`. The pid is constant across every test in one
binary, so `name` is the sole uniqueness token, and **two call sites pass the identical name
`"unknown"`**. Both resolve to `${TMPDIR}/wcartel-cfg-<pid>-unknown.toml`. One test's `remove_file`
can therefore delete the other's file between that other's write and its read, after which
`RealFs::read_capped` → `File::open` returns `Err(NotFound)`, `load_with_fs` pushes
`"config: cannot read …"`, and the expected `"files.type_filter"` warning is absent.

**Outcome of this effort:** the collision is removed by making the path unique (an `AtomicU64`
counter, the idiom already used by `tempdir()` in the same module), the three identical helpers are
folded into one, and the fix is proven by measurement + attribution rather than by absence of the
symptom. Two production-facing assertion *messages* are added first, so the mechanism is observed
before it is altered.

**Not in scope** (filed, do not touch): **H33** — `std::env::set_var("HOME", …)` in
`wordcartel/src/file_browser_commit.rs`. **H32** — a crate-wide scratch-path seam replacing ~13
duplicated per-module idioms. Do not "improve" either while in the tree.

## Architecture

One file changes: `wordcartel/src/config.rs`, and only inside its `#[cfg(test)] mod tests`.

- A new `scratch_cfg_path(name: &str) -> PathBuf` owns path construction and carries a private
  `static N: AtomicU64`. Extracting it is what makes uniqueness **directly unit-testable** — the
  existing helpers return `(Config, Vec<String>)` and never expose the path, so uniqueness cannot
  otherwise be asserted without a flaky concurrency test.
- A single `load_cfg(name, body)` replaces `load_files` / `load_clip` / `load_diag`, which are
  deleted. The fold is not tidying: making the safe helper the *only* helper is the durability
  mechanism (spec D3 = A). **No guard test or textual scanner is added** — effort ①'s decision D5
  measured that answering a trust-in-gates problem with another scanner is self-defeating.
- No production code changes. `load`, `load_with_fs`, `RealFs::read_capped`, and every config parse
  arm are untouched. **Command-surface contract: N/A — this effort does not touch the command
  surface** (no command, palette entry, menu item, keybinding hint, user-settable option, or config
  key is added, removed, or renamed).

## Tech stack

Rust 2021, `wordcartel` shell crate only (`wordcartel-core` and `wordcartel-nlp` untouched). No new
dependency. `std::sync::atomic::{AtomicU64, Ordering}` only, imported function-locally exactly as
`tempdir()` already does. Verification tooling: `cargo`, `jq`, `awk`, `seq` (all confirmed present
at `/usr/bin`). Shell is **zsh 5.9**.

## Global constraints

- **House style, hand-formatted. NEVER run `cargo fmt`** — no `rustfmt.toml`; `cargo fmt` reflows
  the whole tree. Match surrounding code by hand. Em-dash `—` in prose comments, never `--`. No
  emoji. 4-space indent, ~100-char hand-wrapped lines.
- **Per-task gates, all must pass before commit:**
  - `cargo test` green
  - `cargo clippy --workspace --all-targets` clean (workspace `clippy::all = "deny"`; any
    `#[allow]` must be **item-local with a one-line rationale**, never blanket)
  - `cargo build` and `cargo test --no-run` warning-free for `wordcartel`
  - `clippy::too_many_lines` threshold is 100; nothing here approaches it.
- **There is no CI.** Every gate runs only because you run it. Paste command output in your task
  report; never write "CI will catch it."
- **Validation is at DEFAULT threading.** This machine has 32 cores. This flake is invisible at
  `--test-threads=1` and in isolation. **Never add a thread-count flag to make something pass.**
  Never set `--shuffle` or `RUST_TEST_SHUFFLE`.
- **Shell rules — zsh, not bash.** Each was a real defect in the previous effort:
  - No `pkill` / `killall` / any pattern-matched kill. Ever.
  - No glob-selected test binaries. Select via cargo's JSON artifact stream and confirm with
    `--list` — both are already implemented in `scratchpad/h31-gates/run_n.sh`; use it rather than
    re-deriving them.
  - `mktemp -d` per step; no fixed temp paths in your scripts.
  - **Capture `$?` on the SAME LINE as the command.** `${PIPESTATUS[0]}` is bash-only and expands
    EMPTY in zsh. Write `cargo build -p wordcartel > "$L" 2>&1; rc=$?` — and note that a pipeline's
    status in zsh is the LAST command's, so `cargo … | tail` reads green even when cargo failed.
  - zsh does **not** word-split unquoted variables — use positional parameters or arrays if you
    need splitting. (`$(seq …)` *does* split — but the harness deliberately loops with zsh
    arithmetic instead, so its iteration count never depends on an external command's output.)
  - Quote grep's `--include='*.rs'`: unquoted, zsh glob-expands it and the command dies with
    `no matches found`.
- **Attribute test failures by parsing libtest's `failures:` BLOCK**, never a bare test-name grep —
  libtest prints the test name for PASSING runs too. The harness does this; do not hand-roll it.
- **Commit trailers, verbatim, on every commit:**
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG
  ```

### The mutation rule (spec §7.0) — binding on every verification step in this plan

> A mutation must change **exactly one** property, and the required outcome must name the **one
> assertion** that must fail — never merely "the test fails".

Rust panics on the first failing assertion, so a mutation touching several fields read by a
multi-assertion test proves only that *something* broke; every later assertion is short-circuited
away and could be deleted while the step still went green. This defect appeared twice in the spec's
own criteria. **Also:** once Task 1 lands custom `assert!` messages, a failing assertion prints the
**custom message**, not the assertion expression — identify failures by custom message and/or
`file:line`, never by "the assertion text".

---

## Task 1 — Diagnostic messages, then observe the mechanism (D4: this is FIRST)

**Sequencing is fixed and non-negotiable: the assertion messages and the observation run land
BEFORE any path change.** The point is to observe the diagnosed mechanism while it still fires. A
version of this effort that fixes the paths first and adds diagnostics afterwards is wrong and will
be rejected — the observation is unobtainable once the flake is gone.

### Files

- `wordcartel/src/config.rs` — two assertion messages, inside `#[cfg(test)] mod tests`.
- `scratchpad/h31-gates/run_n.sh` — **already committed and executable; consumed, not authored.**
  The same file serves Tasks 3 and 4.
- `scratchpad/h31-gates/observation-prefix.md` — new, **committed by this task**; the recorded
  evidence Task 3's attribution check compares against.

### Interfaces

Consumes: nothing new. Produces: no API change — only `assert!` message arguments. The two tests
keep their assertion *expressions* byte-identical; this is what preserves the mutation-verified
guard from `ea01138` (spec §5).

### Steps

1. **Locate the two tests by name** (anchor on names, not line numbers — they drift):
   `files_type_filter_unknown_warns_and_defaults_documents` and
   `clipboard_provider_unknown_warns_and_defaults_auto`, both in `wordcartel/src/config.rs`'s
   `#[cfg(test)] mod tests`.

2. **Add the message arguments.** In `files_type_filter_unknown_warns_and_defaults_documents`,
   change the final assertion from:
   ```rust
       assert!(warns.iter().any(|w| w.contains("files.type_filter")));
   ```
   to exactly:
   ```rust
       assert!(warns.iter().any(|w| w.contains("files.type_filter")),
           "the invalid-value arm must warn by name (H31 diagnostic); warns was: {warns:?}");
   ```
   In `clipboard_provider_unknown_warns_and_defaults_auto`, change:
   ```rust
       assert!(warns.iter().any(|w| w.contains("clipboard.provider")));
   ```
   to exactly:
   ```rust
       assert!(warns.iter().any(|w| w.contains("clipboard.provider")),
           "the invalid-value arm must warn by name (H31 diagnostic); warns was: {warns:?}");
   ```
   Change **nothing else** — not the assertion expressions, not the `assert_eq!` above them, not
   the helpers. This mirrors the existing message style in
   `config_over_cap_degrades_like_an_unreadable_file` in the same module.

3. **Gates — read the exception before you run them.** `cargo clippy --workspace --all-targets`
   clean and `cargo build`/`cargo test --no-run` warning-free are ordinary hard gates; the flake
   cannot perturb them.

   **`cargo test` is different at THIS task only.** The whole point of Task 1 is that the H31 flake
   is still present — it is not fixed until Task 2 — so a `cargo test` run here has roughly a 1-in-6
   chance of going red through the very defect this effort exists to remove. That is **expected and
   is NOT a gate failure**, but only for one exact test:

   > **`config::tests::files_type_filter_unknown_warns_and_defaults_documents`** failing at its
   > warning assertion is expected at this stage. Note it in your report and proceed.
   > **Any other failing test blocks the commit** — it is unrelated to H31 and must be investigated.

   Determine which by parsing the `failures:` block (never a bare test-name grep — libtest prints
   test names on passing runs too). If that one test is the only failure, re-run `cargo test` once
   to confirm it passes on a subsequent run; a *deterministic* failure of it would mean something
   other than the flake is wrong, and blocks. This exception exists only in Task 1: from Task 2
   onward `cargo test` is a hard green gate, because the fix has landed.

   Commit with the trailers:
   `test(h31): print warns on the two invalid-value assertions (D4, pre-fix diagnostic)`

4. **The run harness ALREADY EXISTS — verify it, do not re-author it.** It is committed at
   **`scratchpad/h31-gates/run_n.sh`**, executable, and its guards have been exercised. Tasks 3 and
   4 invoke the same file; a second transcribed copy would defeat the point of one audited harness.
   Confirm before use:
   ```zsh
   git ls-tree -r --name-only HEAD scratchpad/h31-gates/   # must list run_n.sh
   test -x scratchpad/h31-gates/run_n.sh && print -r -- "harness present and executable"
   ```
   If it is absent, STOP and report rather than reconstructing it — the committed file is the one
   that was reviewed, and a retyped copy is not.

   **Interface** (read the file itself for the implementation and its per-check rationale):
   ```zsh
   scratchpad/h31-gates/run_n.sh <N> <outdir> <expected_total>
   ```
   - `<N>` — whole-binary runs; must be a positive integer.
   - `<outdir>` — created if absent, must be writable; receives `list.txt` and `run-<i>.log`.
   - `<expected_total>` — `passed + failed` required on EVERY run. Derived per task (step 5), never
     a magic constant.
   - **Exit 2 = integrity violation: the measurement is VOID; do not interpret any number it
     printed.** Exit 0 = the runs happened and are trustworthy; test *failures* are reported in the
     output, not treated as harness errors, because Task 1 legitimately expects failures.
   - Output ends with `SUMMARY: runs=… failures=… expected_total=… threads=… binary=…`.

   It hard-fails (exit 2) on: a non-positive or non-numeric `N` or `expected_total`; an unwritable
   outdir; `RUST_TEST_SHUFFLE` set; `RUST_TEST_THREADS` **inherited** from the environment; no lib
   binary from cargo's JSON artifact stream; either `unknown` test missing from `--list` by **exact
   line match**; any log without exactly one `test result:` line; any run with `filtered != 0`; any
   run where `passed + failed != expected_total`; and **any shortfall between requested and
   completed runs**.

   **Concurrency:** the harness *sets* `RUST_TEST_THREADS=32` itself and records that value, rather
   than inferring one from `nproc` — libtest falls back to `std::thread::available_parallelism()`,
   which diverges from `nproc` under cgroup limits or affinity masks, so an inferred number could
   read 32 while libtest ran with 4. Setting it makes the recorded number the number libtest used,
   and makes runs reproducible. 32 matches the conditions the 10/60 baseline was measured under.
   Do not set `RUST_TEST_THREADS` or `RUST_TEST_SHUFFLE` yourself — an inherited value is refused
   rather than silently honoured.

   **`runs=` in the summary is the COUNTED number of completed iterations**, not the requested `N`,
   and a shortfall is fatal — a partial measurement must never be readable as a complete one.

5. **Derive this task's `expected_total`** — do not copy a constant. At this point in the branch
   Task 2 has not run, so the working tree still holds the `main` baseline — count it in place (do
   **not** switch or detach branches to check; you would risk the branch state for nothing):
   ```zsh
   grep -rn '#\[test\]' wordcartel/src --include='*.rs' | wc -l    # → 1777
   grep -rn '#\[ignore' wordcartel/src --include='*.rs'            # → exactly 1 (e2e.rs bench)
   ```
   (Quote the `--include` pattern: unquoted `*.rs` is glob-expanded by zsh and the command fails
   with `no matches found`.)
   1777 `#[test]` attributes, 1 of them `#[ignore]`d (`r1_typing_latency_bench` in `e2e.rs`), so a
   clean run reads `1776 passed; 0 failed; 1 ignored` and **`expected_total` for Task 1 is `1776`**
   (passed + failed, ignored excluded). Task 1 adds no `#[test]`.

6. **Run 30 iterations at default threading:**
   ```zsh
   chmod +x scratchpad/h31-gates/run_n.sh
   OUT=$(mktemp -d)
   scratchpad/h31-gates/run_n.sh 30 "$OUT" 1776; rc=$?
   print -r -- "harness rc=$rc outdir=$OUT"
   ```
   `rc=2` means the measurement is void — fix the cause and re-run; do not interpret the numbers.
   Keep `$OUT` for step 8; the failing run's log is `"$OUT"/run-<i>.log`.

7. **Pass condition — read it exactly (spec §7.1).** This step passes only if **all** hold:
   - **at least one** of the 30 runs failed;
   - the failing test is `files_type_filter_unknown_warns_and_defaults_documents`;
   - it failed at the **warning assertion**, identified by the custom message added in step 2
     (`the invalid-value arm must warn by name (H31 diagnostic)`) — *not* at the `assert_eq!`
     above it;
   - the printed `warns` in that message contains the read-error string `config: cannot read`.

   **Zero failures in 30 runs is INCONCLUSIVE — it is NOT a pass.** (At the measured 16.7% rate,
   30 clean runs has probability ≈ 0.4%: unlikely, not impossible.) On zero failures, re-run with
   60 iterations; if still zero, STOP and escalate to the human rather than proceeding — a
   silently-vanished flake changes what the rest of this effort means.

8. **Record the evidence** in `scratchpad/h31-gates/observation-prefix.md`: the binary path, the
   failure count out of 30, the harness `SUMMARY:` line (including its `threads=` field), and the
   **verbatim** panic block including the `warns` vector. Tasks 3 and 4 compare against this text,
   so it must be committed — `$OUT` is a `mktemp -d` and will not survive to their sessions:
   ```zsh
   git add scratchpad/h31-gates/observation-prefix.md
   ```
   `docs(h31): record the pre-fix mechanism observation (30 runs, D4)`

---

## Task 2 — Extract a unique-by-construction path, fold the three helpers

### Files

- `wordcartel/src/config.rs` — `#[cfg(test)] mod tests` only.

### Interfaces

**Produces** (both private to `mod tests`):
```rust
fn scratch_cfg_path(name: &str) -> PathBuf
fn load_cfg(name: &str, body: &str) -> (Config, Vec<String>)
```
**Removes:** `load_files`, `load_clip`, `load_diag` (all three byte-identical). They are private to
the test module and have no callers outside it.

**Consumes:** `load(&[PathBuf]) -> (Config, Vec<String>)` (existing, unchanged). `PathBuf` is
already in scope in this file (`tempdir()` returns one).

### Steps

1. **Write the failing test FIRST — and make it fail as an ASSERTION, not a compile error.** A test
   calling a function that does not exist yet fails to *compile*, and a compile error would satisfy
   "see it fail" while proving nothing about the property. So: first add `scratch_cfg_path` in its
   **pre-fix form**, reproducing today's formula with no counter, immediately above the existing
   `load_files` in the test module:
   ```rust
   fn scratch_cfg_path(name: &str) -> PathBuf {
       std::env::temp_dir().join(format!("wcartel-cfg-{}-{name}.toml", std::process::id()))
   }
   ```
   and the test, placed directly beneath it:
   ```rust
   #[test]
   fn scratch_cfg_paths_are_unique_even_for_one_name() {
       // Two call sites legitimately pass the SAME name ("unknown" — the [files] and
       // [clipboard] invalid-value tests), so `name` must not be what carries uniqueness.
       // A shared path is H31: one test's remove_file deletes another's file mid-read.
       let a = scratch_cfg_path("unknown");
       let b = scratch_cfg_path("unknown");
       assert_ne!(a, b, "same name must still yield distinct paths: {a:?} vs {b:?}");
   }
   ```

2. **Run it and see it fail for the right reason:**
   ```zsh
   cargo test -p wordcartel --lib scratch_cfg_paths_are_unique_even_for_one_name
   ```
   Required: it **compiles** and fails at `assert_ne!` with the two paths printed equal. If instead
   you get a build error, the step has not been performed — fix and repeat. Record the output.

3. **Implement uniqueness.** Replace the pre-fix body so `scratch_cfg_path` reads exactly:
   ```rust
   // Unique per call. Two call sites pass the same `name` ("unknown"), so `name` is for
   // readability only — the counter is what guarantees uniqueness. Mirrors `tempdir()`'s
   // idiom in this module. A shared path was H31: one test's remove_file deleted another
   // test's file between its write and its read.
   fn scratch_cfg_path(name: &str) -> PathBuf {
       use std::sync::atomic::{AtomicU64, Ordering};
       static N: AtomicU64 = AtomicU64::new(0);
       std::env::temp_dir().join(format!(
           "wcartel-cfg-{}-{name}-{}.toml",
           std::process::id(),
           N.fetch_add(1, Ordering::Relaxed)
       ))
   }
   ```
   The `wcartel-cfg-` prefix and the pid component are kept deliberately: the prefix stays distinct
   from `tempdir()`'s `wc-cfg-` directories, and the pid keeps two concurrent `cargo test` binaries
   from colliding.

4. **Run and see it pass:**
   ```zsh
   cargo test -p wordcartel --lib scratch_cfg_paths_are_unique_even_for_one_name
   ```
   Required: `1 passed`.

5. **Fold the three helpers into one.** Add, directly beneath `scratch_cfg_path`:
   ```rust
   fn load_cfg(name: &str, body: &str) -> (Config, Vec<String>) {
       let p = scratch_cfg_path(name);
       std::fs::write(&p, body).unwrap();
       let out = load(std::slice::from_ref(&p));
       let _ = std::fs::remove_file(&p);
       out
   }
   ```
   Then **delete** `load_files`, `load_clip`, and `load_diag` entirely, and repoint all five call
   sites to `load_cfg` — argument lists are unchanged, only the function name:

   | Test | Old call | New call |
   |---|---|---|
   | `files_type_filter_unknown_warns_and_defaults_documents` | `load_files("unknown", …)` | `load_cfg("unknown", …)` |
   | `clipboard_provider_parses_all_values` (inside its `for` loop) | `load_clip(s, &format!(…))` | `load_cfg(s, &format!(…))` |
   | `clipboard_provider_unknown_warns_and_defaults_auto` | `load_clip("unknown", …)` | `load_cfg("unknown", …)` |
   | `harper_engine_table_overrides_grammar` | `load_diag("harper-grammar", …)` | `load_cfg("harper-grammar", …)` |
   | `linters_list_round_trips` | `load_diag("linters", …)` | `load_cfg("linters", …)` |

   Keep each helper's surrounding section comments where they are; they document the config
   sections, not the helpers. Do **not** modify any assertion. Do **not** touch
   `config_over_cap_degrades_like_an_unreadable_file`, which builds its own distinct
   `wc-cfg-cap-{pid}/config.toml` path and does not collide with these call sites.

6. **Confirm no stragglers.** Both commands must print nothing:
   ```zsh
   grep -n 'load_files\|load_clip\|load_diag' wordcartel/src/config.rs
   grep -rn 'wcartel-cfg-{}-{name}\.toml' wordcartel/src
   ```

7. **Gates:** `cargo test` green (full suite, default threading, no thread flags),
   `cargo clippy --workspace --all-targets` clean, `cargo build` and `cargo test --no-run`
   warning-free. Capture each cargo exit code on the SAME LINE (`cargo build -p wordcartel > "$L" 2>&1; rc=$?`)
   — in zsh without `pipefail` a pipeline's status is the last command's, so `cargo … | tail` reads
   green on failure.

   **Report the test-count delta explicitly, because Task 3 pins it.** This task adds exactly **one**
   `#[test]` (`scratch_cfg_paths_are_unique_even_for_one_name`) and removes none — the fold deletes
   helper *functions*, which are not tests. Confirm and state the number:
   ```zsh
   git diff main --unified=0 -- 'wordcartel/src/**' | grep -c '^+.*#\[test\]'   # → 1
   git diff main --unified=0 -- 'wordcartel/src/**' | grep -c '^-.*#\[test\]'   # → 0
   ```
   Commit:
   `fix(h31): unique scratch path per call; fold three identical config test helpers`

---

## Task 3 — Post-fix measurement and the attribution check

Deliverables: two independent statistical results. Neither may be replaced by an isolated or
`--test-threads=1` run — **this flake is invisible at 1 thread, so an isolated green proves
nothing.**

### Files

- `scratchpad/h31-gates/measurement-postfix.md` — new; both results.
- `scratchpad/h31-gates/run_n.sh` — **consumed, not modified.** Committed by Task 1; invoked as
  `run_n.sh <N> <outdir> <expected_total>`. If it is missing, Task 1 did not complete — stop and
  report rather than reconstructing it, since a divergent second copy defeats the point of one
  audited harness.
- No source changes on the branch. (The attribution check edits a **scratch** branch that is
  discarded.)

### Steps — Part A: post-fix measurement (spec §7.2)

1. **Derive `expected_total` — it is NOT a constant.** The pin is strong because it is exact, and
   wrong for the same reason the moment a task changes the test count, so compute it:

   > `expected_total` = **1776** (the `main` @ `60be3d1` baseline: 1777 `#[test]` attributes under
   > `wordcartel/src`, minus the one `#[ignore]`d `r1_typing_latency_bench` in `e2e.rs`)
   > **+ `#[test]`s this branch adds − `#[test]`s it removes.**

   ```zsh
   git diff main --unified=0 -- 'wordcartel/src/**' | grep -c '^+.*#\[test\]'   # added
   git diff main --unified=0 -- 'wordcartel/src/**' | grep -c '^-.*#\[test\]'   # removed
   ```
   Task 2 adds exactly one (`scratch_cfg_paths_are_unique_even_for_one_name`) and removes none, so
   at this point **`expected_total` = 1777**, and a clean run reads
   `1777 passed; 0 failed; 1 ignored`. If the commands above disagree with that arithmetic, STOP:
   either the fold removed a test it should not have, or a task added one this plan does not know
   about. Re-derive, do not force the number.

   **Caveat — the delta count is lexical, not Rust-aware.** `grep -c '#\[test\]'` counts the
   attribute wherever it appears, including inside a comment or a string literal, and would miss one
   written unusually (e.g. `#[cfg_attr(…, test)]`). It is correct for this branch's edits, which add
   one ordinary `#[test]`, but do not over-trust it as a general census. The authoritative
   cross-check is the harness's own `passed + failed == expected_total` on a real run: if the
   derivation were wrong, every run would hard-fail with the actual total in the message.

2. **Run 200 iterations** with the harness committed by Task 1:
   ```zsh
   OUT=$(mktemp -d)
   scratchpad/h31-gates/run_n.sh 200 "$OUT" 1777; rc=$?
   print -r -- "harness rc=$rc outdir=$OUT"
   ```
   **Required result: `rc=0` and `SUMMARY: runs=200 failures=0`.** Expect ~15-20 min.
   `rc=2` means an integrity check tripped and the measurement is **void** — diagnose and re-run;
   never interpret the failure count from a void run.

   **Why 200 and not the 60 this plan originally specified — read this before shortening it.**
   The 60-run figure was justified by "at the measured 16.7% rate, 60 clean runs is luck with
   probability ≈ 1.7×10⁻⁵". That justification is no longer sound: **Task 1 measured the pre-fix
   rate at 4/60 on a quiet machine**, against 10/60 during the grounding sweep when four agents
   were loading the box. Pooling Task 1's observation runs gives ≈4-7%. At 6.7%, 60 clean runs is
   luck with probability ≈ 1.6×10⁻² — a thousand times weaker than the number the criterion cites,
   and weak enough that a clean 60 would prove very little.
   The race window is **load-dependent**, not constant (effort ① saw 4/60 at default load vs 39%
   under six-way contention). Since we cannot rely on reproducing the load, the answer is more
   runs: at a 4.4% true rate, 200 clean runs is luck with probability ≈ 1.2×10⁻⁴.
   **This is a strengthening of a ruled acceptance criterion, not a relaxation.** If you find
   yourself tempted to drop back to 60 because 200 takes longer, that is the wrong trade — the
   whole effort exists to be able to trust this number.

   **The attribution check in step 6 is the stronger evidence and is not optional.** A clean run is
   an absence; the attribution check produces a *positive* result (the flake returns when the
   uniqueness is reverted), and a positive result cannot be manufactured by accidentally removing
   the test. If the two disagree, believe the attribution check.

3. **What the harness enforced, and why each matters** (verify these are in the copy you ran; if the
   file was modified since Task 1, that is a finding to report, not to fix silently):
   - binary from cargo's JSON artifact stream, never an `ls -t` glob;
   - **both** `files_type_filter_unknown_warns_and_defaults_documents` **and**
     `clipboard_provider_unknown_warns_and_defaults_auto` present via `--list` — load-bearing,
     because **a clean run is also exactly what you get if the fold silently dropped or renamed
     the flaky test out of the suite**;
   - **per-file** `test result:` line count of exactly 1 (an aggregate `sort | uniq -c` across logs
     would let a log with zero result lines cancel one with two — it reads like a per-file guarantee
     and is not one);
   - `filtered = 0` on every run;
   - `passed + failed == expected_total` on every run — any other total, high or low, voids the run;
   - failures attributed by parsing the `failures:` **block**, never a bare test-name grep.
   - Additionally check yourself: all 200 logs exist (`ls "$OUT"/run-*.log | wc -l` → 200), and
     total runtime was ~14-17 min (the loop is strictly sequential — one binary invocation at a
     time, at ~4-5 s each; `RUST_TEST_THREADS=32` governs libtest's internal pool, not the
     harness's). **An implausibly fast green did not run.**

### Steps — Part B: attribution check (spec §7.3)

This is what distinguishes "my change fixed it" from "the symptom stopped." Effort ① found a fix
that would have gone green for an unrelated reason.

4. From the branch tip, create a scratch branch (it is discarded; never merged):
   ```zsh
   git switch -c h31-attribution-scratch
   ```

5. **Revert ONLY the uniqueness** — keep the fold, keep the messages. In `scratch_cfg_path`, delete
   the `static N` and the counter component so the format string returns to
   `"wcartel-cfg-{}-{name}.toml"` with only `std::process::id()`. Change nothing else.

6. **Confirm the revert is confined**, or the step misattributes:
   ```zsh
   git diff --stat        # must be: wordcartel/src/config.rs only
   git diff               # must touch ONLY the path-construction expression
   ```
   If the diff reaches any assertion, call site, or other function, redo it.

7. Run 30 iterations with the same committed harness. The scratch branch changes no `#[test]`, so
   `expected_total` is still 1777:
   ```zsh
   OUT=$(mktemp -d)
   scratchpad/h31-gates/run_n.sh 30 "$OUT" 1777; rc=$?
   print -r -- "harness rc=$rc outdir=$OUT"
   ```

8. **Pass condition:** the flake **returns**, AND the failure is
   `files_type_filter_unknown_warns_and_defaults_documents` failing at the **warning assertion**
   (by custom message), AND its printed `{warns:?}` **matches the mechanism recorded in
   `observation-prefix.md`** from Task 1. The `warns` comparison is required because two distinct
   sub-mechanisms produce an identical panic line — `Err(NotFound)`, or having read the
   `[clipboard]` TOML and collected only a `clipboard.provider` warning. Without it, this step
   proves only that shared naming reintroduces *a* flake, not *this* one.

   **Failure to reproduce within 30 runs is INCONCLUSIVE — not a pass.** This step passes only on a
   positive, mechanism-matched reproduction; never on absence. On zero reproductions, extend to 60,
   then escalate.

9. Discard the scratch branch and return:
   ```zsh
   git switch effort-h31-config-temp-path
   git branch -D h31-attribution-scratch
   git status --short      # must be clean
   ```

10. Record both parts in `scratchpad/h31-gates/measurement-postfix.md` — the harness's full
    `SUMMARY:` line verbatim (it carries `runs=`, `failures=`, `threads=` and `expected_total=`),
    its exit code, the reproduced panic block, and the mechanism comparison. Commit:
    `docs(h31): record post-fix measurement (0/200) and the attribution check`

---

## Task 4 — Guard preservation: prove both `[files]` assertions still bear load

`ea01138` added these tests to close a measured gap and mutation-verified them. This task proves
the guard survived the fold. **Every mutation here obeys the §7.0 rule: one property changed, one
named assertion required to fail.** Each mutation is reverted before the next.

**Read this before starting:** a mutation that fails to compile makes `cargo test` exit non-zero
for the wrong reason. A **build error is NOT a passing outcome** for any step below — the required
result is a compiled binary whose named test fails at its named assertion.

### Files

- `wordcartel/src/config.rs` — temporary mutations only; **the tree must be byte-identical to the
  task's starting state when you finish.** Verify with `git status --short` (clean) and
  `git diff` (empty).
- `scratchpad/h31-gates/mutation-log.md` — new; the record.

### Steps

1. **Mutation (a) — the warning guard.** Target: the `warns.push` in the `other =>` arm. In
   `load_with_fs`, locate the `raw.files.type_filter` match (arms `"documents"`, `"all"`, `other`)
   and comment out **only** the `other =>` arm's `warns.push(...)`, replacing it with a no-op so it
   still compiles:
   ```rust
                   other => { let _ = other; },
   ```
   Change nothing else — not the `"documents"` arm, not `FilesConfig::default()`.

2. Run the one test:
   ```zsh
   cargo test -p wordcartel --lib files_type_filter_unknown_warns_and_defaults_documents
   ```
   **Required outcome:** it compiles, and the test fails **specifically at the warning assertion**,
   identified by Task 1's custom message `the invalid-value arm must warn by name (H31 diagnostic)`.
   A failure at the `assert_eq!(cfg.files.type_filter, FileTypeFilter::Documents)` above it means
   the mutation was not confined to the warning arm — that is a **FAILED step**, not a pass; revert
   and redo. Record the verbatim output.

   **Then revert, and prove the reversion is byte-for-byte:**
   ```zsh
   git diff --exit-code -- wordcartel/src/config.rs; rc=$?   # rc MUST be 0
   ```
   Re-running the target test is **not** sufficient proof: a leftover edit elsewhere in the warning
   arm would not affect the *next* mutation's test (`files_filters_default_on_absent`) and could
   persist silently into step 3, contaminating it. `git diff --exit-code` is both cheaper and
   strictly stronger than a full-suite rerun. Do not apply mutation (b) until `rc=0`.

   Why this mutation and not `ea01138`'s: the invalid-value arm only *pushes the warning* — it does
   **not** assign `cfg.files.type_filter`. The `Documents` the first assertion sees comes from
   `Config::default()`. So flipping the default cannot exercise the warning assertion at all; it
   kills the test one assertion earlier.

3. **Mutation (b) — the default-on-absent guard.** Target: `type_filter` only. In
   `FilesConfig`'s `Default` impl, change **only** `type_filter` and leave `show_clutter` at
   `false`:
   ```rust
           FilesConfig { show_clutter: false, type_filter: FileTypeFilter::All }
   ```

4. Run the one test:
   ```zsh
   cargo test -p wordcartel --lib files_filters_default_on_absent
   ```
   **Required outcome:** it fails **specifically at the `"files.type_filter must default to
   Documents"` assertion** (that test's own existing message). A failure at its `show_clutter`
   assertion means you flipped both fields — **FAILED step**; redo with only `type_filter` changed.
   Record the verbatim output. **Then revert and prove it byte-for-byte, as in step 2:**
   ```zsh
   git diff --exit-code -- wordcartel/src/config.rs; rc=$?   # rc MUST be 0
   ```

   Why one field and not `ea01138`'s `{ show_clutter: true, type_filter: All }`: that test asserts
   `show_clutter` **first**, so the struct-wide flip kills it at the first assertion and never
   evaluates the `type_filter` one — which could then be deleted while the step still went green.
   Flipping one field makes mutation and asserted property one-to-one and removes assertion-order
   reasoning entirely (§7.0).

5. **Confirm the tree is restored:**
   ```zsh
   git status --short     # must print nothing
   git diff               # must be empty
   cargo test             # full suite green
   ```

6. **Final gates and the pre-merge report.** Two traps, both of which have bitten this project:
   a warning-free build proves nothing if **nothing was rebuilt** (a cached `cargo build`/`clippy`
   emits no warnings by construction), and in zsh **a pipeline's exit status is the LAST command's**
   — `cargo build 2>&1 | tail -20` reports `tail`'s success even when the build failed. So: force a
   recompile, capture each exit code on the SAME LINE, log to a file, and tail the *file*.
   ```zsh
   L=$(mktemp -d)
   touch wordcartel/src/config.rs
   cargo build -p wordcartel > "$L/build.log" 2>&1; build_rc=$?
   touch wordcartel/src/config.rs
   cargo clippy --workspace --all-targets > "$L/clippy.log" 2>&1; clippy_rc=$?
   cargo test --no-run > "$L/norun.log" 2>&1; norun_rc=$?
   print -r -- "build=$build_rc clippy=$clippy_rc norun=$norun_rc"
   tail -20 "$L/build.log"; tail -20 "$L/clippy.log"; tail -20 "$L/norun.log"
   ```
   **All three `rc` values must be 0**, and the logs must contain no `warning:` lines for
   `wordcartel`. Treat an implausibly fast "clean" as not having run. Then run the PTY smoke suite,
   capturing its status the same way, and **quote its one-line summary verbatim** in the report:
   ```zsh
   scripts/smoke/run.sh > "$L/smoke.log" 2>&1; smoke_rc=$?
   print -r -- "smoke_rc=$smoke_rc"; tail -5 "$L/smoke.log"
   ```
   It is mandatory-run, advisory-pass: a red result does not block the merge but must be surfaced
   explicitly (e.g. `smoke: FAIL s5 — advisory`). A `smoke: SKIP — …` line is quoted the same way
   and is **not** evidence the suite passed.

7. Record mutations, outcomes, and gate output in `scratchpad/h31-gates/mutation-log.md`. Commit:
   `docs(h31): record guard-preservation mutations (§7.0 one-property/one-assertion)`

---

## Plan self-audit (§7.0 applied to this plan's own verification steps)

Assume the first draft contained one. It contained three; all are fixed above, and are called out
here so a reviewer can check the fix rather than rediscover the hole.

1. **Task 2 step 2 would have passed on a compile error.** The natural TDD phrasing — write a test
   calling `scratch_cfg_path`, "run it and see it fail" — fails because the function does not exist.
   That is a *build* failure and says nothing about uniqueness; the test could assert `1 == 1` and
   still "fail" at that step. Fixed by introducing `scratch_cfg_path` in its **pre-fix form first**,
   so the red is a genuine `assert_ne!` failure with two equal paths printed, and by requiring the
   step to be redone if a build error appears instead.
2. **Task 4 had the same hole in a second form.** A mutation that does not compile makes
   `cargo test` exit non-zero, which reads as "the test failed" — the required outcome would be
   satisfied by a typo. Fixed by making mutation (a) a compiling no-op (`other => { let _ = other; },`)
   and by stating explicitly that a build error is not a passing outcome for any mutation step.
3. **Task 3's 0/60 was indistinguishable from "the test no longer exists."** The fold edits the very
   call sites of the flaky test, so a botched fold that dropped it would produce a *perfect* result.
   Fixed by the `--list` presence check on both `unknown` tests before any run, and by pinning
   `passed + failed` to a derived expected total rather than accepting "the suite was green."

Applying the same question to the rest: Task 1 step 7 cannot pass vacuously (it requires ≥1 failure
with a named test, named assertion, and a matched `warns` string, and calls zero failures
inconclusive). Task 3 Part B cannot pass on absence (same rule) and cannot misattribute a broad
revert (the `git diff` confinement check). Task 4 step 6 cannot pass on a cached build (the `touch`).

### Round 2 — defects found by the Codex plan gate, and what replaced them

Two of these were introduced by round 1's own fixes. That is now the established pattern in this
effort (spec round 2, plan round 2), and it is the reason this section exists.

4. **The exact-count pin was self-contradictory and would have failed every run** (Critical). Task 2
   adds one `#[test]`, but Task 3 pinned the post-fix count to the pre-fix 1776 while Task 2
   separately claimed no test was added. The pin was strong *because* exact and wrong for the same
   reason the moment a task changed the count. Fixed by making the total a **derived quantity with
   its derivation shown** (baseline 1776 + added − removed, with the `git diff | grep -c '#\[test\]'`
   commands), by passing it to the harness as an argument rather than hard-coding it, and by
   checking `passed + failed == expected_total` — an invariant that holds in Task 1 where failures
   are *expected*, not only on clean runs.
5. **Task 3 named an artifact its executor would not possess** (Critical). The harness lived under
   Task 1's `mktemp -d`, but each task's subagent sees only its own brief and a fresh shell. Fixed
   by committing the harness once to `scratchpad/h31-gates/run_n.sh` (tracked, verified not
   gitignored) and giving every consumer that exact path and its argument contract — one audited
   copy rather than three inlined copies free to diverge.
6. **"Exactly one `test result:` line per log" did not check that** (Important). The aggregate
   `grep | sort | uniq -c` prints 60 even when one log has zero result lines and another has two —
   they cancel. It *reads* like a per-file guarantee and is not one, the same species of defect this
   effort keeps finding. Fixed by a per-file `grep -c` inside the harness that hard-fails the run.
7. **Cargo steps piped to `tail -20` could read green on failure** (Important). In zsh without
   `pipefail`, a pipeline's status is the last command's, so a failed build/clippy/`--no-run` was
   invisible. Fixed by capturing each exit code on the same line into a log file and tailing the
   *file*; the whole plan was swept for the pattern.
8. **Reverting a mutation was not proven byte-for-byte** (Important, and a correction to this
   author's own proposal). Re-running the target test after reverting mutation (a) proves nothing
   about leftovers that only affect *other* tests — such a leftover would ride silently into
   mutation (b). Fixed with `git diff --exit-code -- wordcartel/src/config.rs` after each revert:
   cheaper than the full-suite rerun this author suggested, and strictly stronger.
9. **The harness's `$OUT` was scoped to the harness process** (Minor) — the caller's awk loop
   referenced a variable it did not own. Fixed by folding attribution into the harness and showing
   the caller set `OUT=$(mktemp -d)` explicitly.

One further hazard, caught while applying the rule to the steps touched this round: the count
derivation originally suggested `git switch --detach 60be3d1` to confirm the baseline. Task 1 runs
before any source change, so the working tree *is* the baseline — the switch bought nothing and put
branch state at risk in a subagent's hands. Removed.

### Round 3 — the fix for round 2's Critical-2 recurred one layer up

10. **The harness did not exist** (Critical). Round 2's Critical-2 was "Task 3 references a harness
    that will not be in its environment." The fix described the harness's interface, named its path,
    told three tasks to depend on it — and never created the file. The plan asserted "new,
    **committed**" of a path that `git ls-tree` showed absent. **The lesson is the check, not the
    file:** for anything a revision claims to create, run the command that proves the artifact
    exists and read its output; do not infer existence from having intended it. Fixed by writing
    `scratchpad/h31-gates/run_n.sh`, exercising its guards, committing it, and verifying with
    `git ls-tree -r --name-only HEAD scratchpad/h31-gates/`. Task 1 now *verifies* the harness
    rather than authoring it, and the plan carries its interface rather than a second copy of its
    source — a retyped copy would not be the reviewed one.
11. **The harness rejected `RUST_TEST_SHUFFLE` but not `RUST_TEST_THREADS`** (Critical). The entire
    effort is invisible at one thread, so an executor whose environment carried
    `RUST_TEST_THREADS=1` would have received a clean summary from a run that never exercised the
    property. Fixed by rejecting `RUST_TEST_THREADS` outright, enforcing a ≥16 effective-thread
    floor (the 10/60 baseline was measured at 32; a lower count is not comparable), and **recording
    the observed thread count in the summary** so concurrency is evidence in the artifact rather
    than an assumption.
12. **The harness could report success having run zero tests** (Important). `seq 1 0` expands to
    nothing, the loop body never executes, and `SUMMARY: runs=0 failures=0` still prints — the
    purest instance of the defect class this effort exists to remove. Fixed by validating argument
    *values*, not just their count: `N` and `expected_total` must be positive integers, the outdir
    must exist and be writable. All guards were exercised and observed to exit 2.
13. **The `--list` presence check was substring-based** (Important). `grep -q "$t"` would be
    satisfied by a *renamed* test that merely contained the original name — defeating exactly the
    check finding 3 above claims to close. Fixed with `grep -qx -- "<full::path>: test"`, matching
    libtest's exact line format.
14. **The spec had gone stale against the plan** (Important). Spec §7.2 still pinned "exactly the
    baseline 1776" while the plan derived 1777. Fixed by carrying the *derived* form into the spec,
    with `passed + failed` rather than `passed` so it holds on pre-fix runs too.
15. **The `#[test]` delta count is lexical, not Rust-aware** (Minor) — noted in the plan so a future
    reader does not over-trust it, with the harness's own total check named as the authoritative
    cross-check.

### Round 4 — both Criticals were inside the instrument itself

The harness is the tool built to detect "reports success while the thing it names is false." Round 4
found two instances of that defect *in the harness*. Worth stating plainly: building an instrument
for a defect class does not exempt the instrument.

16. **The harness could report success for runs that never happened** (Critical). The loop walked
    `$(seq 1 $N)`, so its iteration count depended on an external command's output length; if `seq`
    were missing, shadowed, or truncated, the body could run fewer times — or zero — while the
    summary still printed `runs=$N failures=0`. Fixed by iterating with zsh arithmetic
    (`for (( i = 1; i <= N; i++ ))`), counting completed iterations, hard-failing on any shortfall,
    and **reporting the counted value rather than the requested one**.
17. **The thread-floor check measured the wrong quantity** (Critical). It derived the count from
    `nproc`, but libtest uses `std::thread::available_parallelism()` when `RUST_TEST_THREADS` is
    unset, and the two diverge under cgroup CPU limits or affinity masks — so the check added
    specifically to prevent a concurrency false-green could itself false-green, recording
    `threads=32` for a run libtest performed with 4. Fixed by having the harness **set**
    `RUST_TEST_THREADS=32` and record that: the number in the summary is then the number libtest
    used by construction, and runs become reproducible across machines. The pre-existing
    "reject `RUST_TEST_THREADS`" guard is reconciled by scoping it to an **inherited** value —
    refusing an executor's environment while deliberately setting our own. 32 is the value the
    10/60 baseline was measured at; the comment says so, so nobody lowers it.
18. **Task 1's `cargo test` gate was probabilistic** (Important). Task 1 runs while the flake is
    still present by design, so its own green gate had ~1-in-6 odds of tripping on the known defect,
    with no instruction telling the executor whether to retry, stop, or report. Fixed by scoping the
    exception to exactly one named test failing at its warning assertion — anything else still
    blocks — requiring one confirming re-run (a *deterministic* failure means something else is
    wrong), and stating that the exception exists in Task 1 only.

**Process note, recorded because it cost a review round:** the round-3 report cited commit
`2e5e5d7`, which does not exist; the real commit was `60f7acb`. A reviewer chased the phantom and a
review run died partway through. Report hashes read back from `git log --oneline -1`, never ones you
expect to have been created — the same discipline as verifying a created artifact with `git ls-tree`
rather than trusting intent.

## Underdetermined in the spec, resolved here

- **How uniqueness gets a unit test.** The spec mandates the counter and the fold but not how to
  *prove* uniqueness: the helpers return `(Config, Vec<String>)` and never expose the path, so the
  property is unobservable through them, and a concurrency test that recreates the interleaving
  would itself be flaky. Resolved by extracting `scratch_cfg_path` as a separately testable unit —
  which is also why the extraction is in the plan at all, rather than inlining the counter into
  `load_cfg`. This is a plan-level mechanism choice inside D2's ruling, not a design change.
- **The folded helper's exact identifier and the counter's position in the filename.** The spec
  explicitly left both as plan-level details. Chosen: `load_cfg`, and
  `wcartel-cfg-{pid}-{name}-{counter}.toml` (counter last, so the human-readable `name` stays
  adjacent to the pid and directory listings sort sensibly).
