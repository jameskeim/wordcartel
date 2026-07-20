# H31 — grounding map

Assembled 2026-07-19 by the controller from four independent read-only agents, each instructed to
report FACTS ONLY (no diagnosis, no recommendations) so that the analysis is done once, by Fable,
against the real code rather than inherited from a scout.

Every line/quote below came from an agent reading the current working tree at `main` = `60be3d1`.
Anchor on symbol NAMES, not line numbers — they drift.

---

## 0. The item as filed (docs/ux-backlog.md, marker `<!-- item: H31 -->`)

`config::tests::files_type_filter_unknown_warns_and_defaults_documents` fails ~10% of whole-binary
runs. Passes in isolation and at `--test-threads=1`. Failure is always the same assertion at
`config.rs:1340` — `assertion failed: warns.iter().any(...)`.

Added by C5 Task 24. Effort ① measured it and kept it out of scope, flagging it as **plausibly the
same process-global-state class** ① existed to address. The backlog entry is explicit that this
"is a hypothesis from the failure shape, not a diagnosis; nobody has traced it."

It is now the only known source of red runs in the suite.

---

## 1. MEASUREMENT (agent 3) — reproduced, with exact commands audited

Machine: `nproc` = 32. `RUST_TEST_THREADS` unset, so libtest default = 32 threads.

Binary selected from cargo's JSON artifact stream (NOT an `ls -t` glob):
`target/debug/deps/wordcartel-179fab54caed2816`, package `wordcartel#0.5.0`, target kind `['lib']`.
Verified executable, and verified the target test is present via `--list`.

**60 whole-binary runs, default (32) threading:**
- **10 of 60 failed** (16.7%). Exit code 101. Failing indices 4, 7, 9, 24, 28, 31, 39, 40, 47, 56.
- All 10 failures attributed by parsing the `failures:` BLOCK (not a bare test-name grep — libtest
  prints the test name for passing runs too). All 10 blocks contain exactly one entry:
  `config::tests::files_type_filter_unknown_warns_and_defaults_documents`.
- **No other test failed in any of the 60 runs.**
- Log integrity checked: all 60 logs exist; each has exactly one `test result:` line; passing runs
  report `1776 passed; 0 failed`, failing runs `1775 passed; 1 failed` — consistent, no filtered or
  crashed run masquerading as a pass.

**20 whole-binary runs at `--test-threads=1`:** 0 of 20 failed. All report `1776 passed; 0 failed`.
Single-threaded runs took ~10.3s vs ~4.2s at 32 threads (so the clean runs really ran).

**Verbatim failure (identical byte-for-byte across all 10 but for thread id and timing):**
```
---- config::tests::files_type_filter_unknown_warns_and_defaults_documents stdout ----

thread 'config::tests::files_type_filter_unknown_warns_and_defaults_documents' (1754185) panicked at wordcartel/src/config.rs:1340:9:
assertion failed: warns.iter().any(|w| w.contains("files.type_filter"))
```
No other captured stdout for the test beyond the panic line.

---

## 2. THE TEST AND ITS PATH (agent 1)

The test, `wordcartel/src/config.rs:1336-1341`:
```rust
#[test]
fn files_type_filter_unknown_warns_and_defaults_documents() {
    let (cfg, warns) = load_files("unknown", "[files]\ntype_filter = \"spreadsheets\"\n");
    assert_eq!(cfg.files.type_filter, FileTypeFilter::Documents);
    assert!(warns.iter().any(|w| w.contains("files.type_filter")));
}
```
Note the FIRST assertion (`cfg.files.type_filter == Documents`) passes even on failing runs — only
the warning assertion fails. `Documents` is also the struct default, so that assertion cannot
distinguish "parsed our file" from "parsed nothing".

The helper, `config.rs:1319-1325`:
```rust
fn load_files(name: &str, body: &str) -> (Config, Vec<String>) {
    let p = std::env::temp_dir().join(format!("wcartel-cfg-{}-{name}.toml", std::process::id()));
    std::fs::write(&p, body).unwrap();
    let out = load(std::slice::from_ref(&p));
    let _ = std::fs::remove_file(&p);
    out
}
```

`load` → `load_with_fs(&crate::fsx::RealFs, paths)`. The warnings accumulator is a **plain local**,
`let mut warns = Vec::new();` at the top of `load_with_fs`, returned by value. **No static, no
thread-local, no field, no global cache anywhere on this path.** The `[files]` arm:
```rust
if let Some(t) = raw.files.type_filter {
    match t.as_str() {
        "documents" => cfg.files.type_filter = FileTypeFilter::Documents,
        "all"       => cfg.files.type_filter = FileTypeFilter::All,
        other => warns.push(format!("files.type_filter \"{other}\" invalid; using documents")),
    }
}
```
So the warning is emitted iff the parsed TOML actually contains a `[files] type_filter` key whose
value is neither `documents` nor `all`.

**Read failure behaviour is the open question for this arm:** `load_with_fs` reads via
`fs.read_capped(p, MAX_CONFIG_BYTES)`. What it does when the file is MISSING (returns `Err`) vs
OVER CAP (returns `Ok(None)`) determines whether a vanished file yields an empty `warns` (silent
skip) or a warning. Verify this directly — it decides whether deletion alone can produce the
observed failure.

**Non-local state on the path, exhaustive:** `std::env::temp_dir()`, `std::process::id()`, the
filesystem write/read/delete at the computed path, and `crate::limits::MAX_CONFIG_BYTES` (a const).
Nothing else. No env var is set anywhere on the path.

---

## 3. THE SHARED-PATH FACT (agents 1 + 4)

Three helper fns in `config.rs`'s test module build a path from ONLY temp-dir + pid + a
caller-supplied name. The pid is constant for every test in one `cargo test` binary, so the `name`
argument is the sole uniqueness token:

- `load_files` — `config.rs:1319`
- `load_clip`  — `config.rs:1347`
- `load_diag`  — `config.rs:1389`

**Two call sites pass the identical name `"unknown"`:**
- `files_type_filter_unknown_warns_and_defaults_documents` → `load_files("unknown", "[files]\ntype_filter = \"spreadsheets\"\n")` (`config.rs:1338`)
- `clipboard_provider_unknown_warns_and_defaults_auto` → `load_clip("unknown", "[clipboard]\nprovider = \"telepathy\"\n")` (`config.rs:1366`)

Both therefore resolve to `${TMPDIR}/wcartel-cfg-<pid>-unknown.toml`.

Other names in use: `load_clip` loop values `auto`/`native`/`osc52`/`off`; `load_diag`
`harper-grammar` and `linters`. No other duplicate pair was found.

`grep -n "wcartel-cfg-"` matches ONLY `config.rs` lines 1320, 1348, 1390 — no other file in the
workspace uses that prefix.

**A correct idiom already exists in the same test module**: `tempdir()` at `config.rs:936-945` uses
`static N: AtomicU64` for uniqueness and a different prefix (`wc-cfg-{pid}-{counter}`, single dash).
The three helpers above simply do not use it. 13 further per-module scratch-path helpers across the
crate use the same atomic-counter idiom (agent 2, item A21).

`config_over_cap_degrades_like_an_unreadable_file` (`config.rs:1430`) also builds a temp path by
hand — NOT verified whether its name can collide. Check it.

### >>> THE TENSION THAT MUST BE RESOLVED <<<

The controller's unverified hypothesis is that the two `"unknown"` tests clobber/delete each other's
file, so the reader parses the wrong TOML (or none) and `warns` lacks the expected string.

**That hypothesis predicts a roughly SYMMETRIC failure. The measurement says otherwise: the
clipboard test failed 0 times in 60 runs while the files test failed 10 times.**

Do not accept the hypothesis without explaining that asymmetry, and do not discard it without
explaining the collision. Candidate lines of inquiry (not conclusions): libtest schedules tests in
sorted order, and `clipboard_*` sorts before `files_*`; the two bodies differ in length; the
failure window for each test is the gap between its own write and its own read, and those windows
may not overlap equally; the clobbering party may be a third site entirely. It is also possible the
collision is a REAL defect that is nevertheless NOT the cause of H31 — in which case both facts
need reporting.

---

## 4. PROCESS-GLOBAL STATE CENSUS (agent 2)

21 mutable/interior-mutable statics in `wordcartel/src/**`. **None are on the failing test's path.**
The ones carrying deliberate isolation already (all from effort ①, and the local idiom to imitate):

- `filter.rs` `SPAWN_GATE` — `RwLock<()>`, shared arm for 14 short-lived-child tests, exclusive arm
  for 3 long-lived ones; poison-tolerant via `unwrap_or_else(PoisonError::into_inner)`. Documented
  as a mitigation for filed bug H30.
- `recovery.rs` `SNAPSHOT_GATE` + thread-local `GATE_HELD` re-entrancy flag, RAII `SnapshotGate`;
  documented lock order gate → `LAST_GOOD`.
- `swap.rs` `state_dir()` — `#[cfg(test)]` redirect to `temp_dir()/wcartel-test-state-<pid>`, so a
  test can never reach the developer's real XDG state dir.
- `plugin/mod.rs` `INTERN_POOL` — no gate; instead the TEST was designed around the sharing
  (membership probe rather than pool-size comparison, after a size comparison was observed to drift
  by 2 in a full-suite run).
- `cursor_style.rs` `EVER_WROTE` — no gate; tests assert only monotonic, order-independent facts.

That last pair is directly relevant precedent: **two accepted answers in this codebase are "gate it"
and "write the assertion so ordering cannot matter."**

### A SECOND, UNFILED HAZARD OF THE SAME CLASS

`wordcartel/src/file_browser_commit.rs:575`, inside test
`absolute_and_home_relative_fields_are_honoured`:
```rust
std::env::set_var("HOME", &home);
```
with restore at 577/578 (`set_var` back, or `remove_var` if previously unset). This is the ONLY
`set_var`/`remove_var` in the entire workspace. It mutates **process-wide** `HOME` for the window
between mutation and restore, with no synchronization.

Three `config.rs` tests read the real `HOME`/XDG dir as their assertion ORACLE inside that window's
reach: `diagnostics_default_dictionary_is_not_none` (`:975`), `dictionary_tilde_is_expanded`
(`:993`), `dictionary_bare_tilde_expands_to_home` (`:1017`). `dirs::home_dir()` is also read by
production `load_with_fs` for `~` expansion.

**This does NOT explain H31** — the failing test never reads `HOME`. It is reported because it is a
live instance of the same class, currently unfiled, and it bears on the SCOPING question below.

No `set_current_dir` exists anywhere in the workspace. `env::current_dir()` is read at 8 production
sites (fallbacks in `blocks_marked.rs`, `editor.rs`, `prompts.rs` ×2, `registry.rs`, `export.rs`,
`app.rs`) and 1 test site.

`wordcartel-core` and `wordcartel-nlp` have ZERO `std::env::*` calls. No `build.rs` in the workspace.

---

## 5. PROVENANCE — why this test exists (agent 4)

Added by commit `ea01138`, "test(c5): pin the [files] default-on-absent + invalid-value guard (I2)".
The commit message records that `FilesConfig::default()` was "correct but unguarded: flipping it to
`{true, All}` broke zero tests workspace-wide", unlike `ClipboardConfig::default()` which failed
three — and that the new tests were **mutation-verified** (default flipped, both new tests failed,
then reverted).

**The assertion is load-bearing and was proven so.** Deleting or weakening the test to stop the
flake would silently remove a guard that was added deliberately to close a measured gap. Any
proposed resolution that touches the assertion itself must state how the mutation-verified property
survives.

The type-filter parsing and its warning arm were added by `2f6c585`; the `FileTypeFilter` enum by
`1b94b4c`. `config.rs` is 1449 lines; `load_with_fs` spans ~489-707.

---

## 6. STANDING CONTEXT

- **There is no CI.** Every stated merge gate runs only because a human or agent follows CLAUDE.md.
- Merge gates: `cargo test` green; `cargo build` + `cargo test --no-run` warning-free for touched
  crates; `cargo clippy --workspace --all-targets` clean (warnings denied; any `#[allow]` must be
  item-local with a one-line rationale). PTY smoke suite `scripts/smoke/run.sh` is mandatory-run,
  advisory-pass. Do NOT run `cargo fmt` — the repo is hand-formatted, no `rustfmt.toml`.
- Four existing guard tests are the local idiom for enforcing invariants: `module_budgets`,
  `backlog`, `fs_chokepoint`, `edit_seam`. Effort ① measured `fs_chokepoint`'s weaknesses (5 of 6
  evasion routes uncaught) and its decision D5 concluded that answering a trust-in-gates problem by
  adding another textual scanner is self-defeating — prefer structural impossibility.
- Effort ①'s durable lesson, which binds this effort's verification steps: **a verification step
  whose name promises more than it tests.** Eight instances were found in one effort. Of every proof
  step ask: could this print PASS while the thing it names is false? And: if the behaviour it names
  were entirely absent, would it fail? Treat any green result whose runtime is implausibly small as
  not having run.
- **Attribution check** (effort ①): proving *your change* is what fixed it, not merely that the
  symptom stopped. ① found a fix that would have gone green for an unrelated reason, leaving the
  intended mechanism decorative. Validate at DEFAULT threading — this flake is invisible at
  `--test-threads=1` and in isolation, so an isolated green proves nothing.
