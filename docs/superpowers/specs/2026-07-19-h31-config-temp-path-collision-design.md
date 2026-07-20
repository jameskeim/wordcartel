# H31 — `config.rs` shared-temp-path collision: design spec

**Date:** 2026-07-19. **Author:** Fable (own source verification against `main` = `60be3d1`).
**Inputs:** `scratchpad/h31-gates/map-grounding.md` (facts-only map from four read-only scouts),
`scratchpad/h31-gates/decisions.md` (human-ratified D0–D4 + the binding verification contract).
Every claim below was re-read in the working tree. Claims are anchored on **symbol names**, never
line numbers — line anchors have drifted repeatedly in this project.

**Item:** H31 — `config::tests::files_type_filter_unknown_warns_and_defaults_documents` fails ~17%
of whole-binary runs at default threading; passes in isolation and at `--test-threads=1`. It is the
only known source of red runs in the suite. Sequenced deliberately small, immediately before the
caret/prose-window group (effort ②).

---

## 0. Standing constraints (restated so no reviewer has to infer them)

- **There is no CI.** Every gate below runs only because a human or agent follows `CLAUDE.md`.
  Nothing in this spec claims automated enforcement.
- **Merge gates:** `cargo test` green across suites; `cargo build` / `cargo test --no-run`
  warning-free for the touched crate (`wordcartel` only); `cargo clippy --workspace --all-targets`
  clean under workspace `clippy::all = "deny"`. Any `#[allow(...)]` introduced must be **item-local
  with a one-line rationale** — no blanket crate or workspace allow. This effort anticipates
  needing none.
- **Module size:** `clippy::too_many_lines` (threshold 100, `clippy.toml`) applies. The new helper
  is ~8 lines, so the threshold is not approached. `wordcartel/tests/module_budgets.rs` contains no
  entry for `config.rs` (verified by grep), and this change **reduces** `config.rs` by ~10 lines by
  folding three duplicated helpers into one, so no budget can be crossed in the adverse direction.
- **PTY smoke suite:** mandatory-run, advisory-pass; its one-line summary is quoted verbatim in the
  pre-merge report.
- **Formatting:** hand-formatted tree, no `rustfmt.toml`. Do **not** run `cargo fmt`. Match the
  neighbouring helpers by hand.
- `wordcartel-core` and `wordcartel-nlp` are untouched. No `unsafe` anywhere in this effort.

**Command-surface contract (`docs/design/command-surface-contract.md`): N/A — this effort does not
touch the command surface.** It changes only `#[cfg(test)]` code inside `wordcartel/src/config.rs`:
three test-local helper functions and the assertion *messages* of two tests. No command, palette
entry, menu item, keybinding hint, user-settable option, or config *key* is added, removed, or
renamed. The production parse arms (`raw.files.type_filter`, `raw.clipboard.provider`,
`raw.diagnostics.*`) and `load` / `load_with_fs` are not modified.

---

## 1. The defect (D0, established before any fork was ruled)

Three helpers in `config.rs`'s `#[cfg(test)] mod tests` — **`load_files`, `load_clip`,
`load_diag`** — are byte-for-byte identical. Each builds its scratch path from only
`std::env::temp_dir()`, `std::process::id()`, and a caller-supplied `name`:

```rust
let p = std::env::temp_dir().join(format!("wcartel-cfg-{}-{name}.toml", std::process::id()));
std::fs::write(&p, body).unwrap();
let out = load(std::slice::from_ref(&p));
let _ = std::fs::remove_file(&p);
out
```

The pid is constant for every test in one `cargo test` binary, so `name` is the **sole uniqueness
token**. Two call sites pass the identical name `"unknown"`:

- `files_type_filter_unknown_warns_and_defaults_documents` → `load_files("unknown", …)`
- `clipboard_provider_unknown_warns_and_defaults_auto` → `load_clip("unknown", …)`

Both resolve to `${TMPDIR}/wcartel-cfg-<pid>-unknown.toml`. A workspace grep for `wcartel-cfg-`
matches only the three helper lines in `config.rs`; no other file uses the prefix.

**The failing interleaving** (C = the clipboard test, F = the files test):

1. libtest dispatches tests in sorted-name order, so C starts before F.
2. C writes the shared path, `load` reads it, C's assertions pass.
3. F writes the same path — after C's read, before C's `remove_file`.
4. C's `remove_file` deletes the file **F just wrote**.
5. F's read goes through `RealFs::read_capped`, which begins `let f = fs::File::open(path)?;` — so
   a **missing file is `Err(NotFound)`, not `Ok(None)`**. `load_with_fs`'s `Err` arm pushes
   `format!("config: cannot read {}: {e}", p.display())` and `continue`s. `warns` therefore never
   contains `"files.type_filter"`.
6. F's first assertion still passes, because `FilesConfig::default()` is
   `{ show_clutter: false, type_filter: FileTypeFilter::Documents }` — that assertion cannot
   distinguish "parsed my file" from "parsed nothing". The second assertion panics. This is exactly
   the measured failure: one test, always the same assertion, byte-identical panic.

**Why 10/60 for F and 0/60 for C.** The mechanism is symmetric; the reachable interleaving space is
not. Every failure mode requires the opposing event to land inside the victim's write→read gap, and
that gap closes at `File::open` (POSIX unlink of an already-open fd is harmless). Three of the four
modes require F to start at or before C; sorted-order dispatch pins the sign of that offset, so only
"C's `remove_file` kills F's file" is reachable. **The clipboard test is not safe — it is shielded
by scheduling.** Fixing the collision fixes both.

**Verified clean, so out of the fix:** `config_over_cap_degrades_like_an_unreadable_file` builds its
own path but under a distinct prefix (`wc-cfg-cap-{pid}/config.toml`) and cannot collide. The
`tempdir()` helper in the same module uses `static N: AtomicU64` with prefix `wc-cfg-{pid}-{n}` and
is already correct — it is the idiom this spec adopts.

**Correction to the grounding map, recorded:** map §2 left the `Err` vs `Ok(None)` question open and
raised the possibility that a vanished file yields a *silently empty* `warns`. It does not; it
yields a non-empty `warns` whose single entry is the read-error string. Both routes fail the same
assertion, so the conclusion is unchanged, but the mechanism statement is now exact.

**Measured baseline:** 10/60 failures at default (32) threading, 0/20 at `--test-threads=1`, all 10
attributed by parsing libtest's `failures:` block.

---

## 2. Scope (D1 = C)

**In scope:** the `config.rs` temp-path collision class — the three helpers and their five call
sites, plus the two assertion messages from D4.

**Out of scope, filed rather than fixed:**

- **H33** — `std::env::set_var("HOME", …)` in `file_browser_commit.rs`'s
  `absolute_and_home_relative_fields_are_honoured` (the workspace's only `set_var`/`remove_var`).
  Same genus, different species: it has its own unresolved mechanism fork and has never been
  observed to fire. Filed with grounding and the edition-2024 `unsafe set_var` forcing function.
- **H32** — a crate-wide scratch-path seam replacing the ~13 duplicated per-module atomic-counter
  idioms. The real structural answer, but it touches ~13 modules — the opposite of a small effort.
  Filed with an explicit "do not answer this with a textual scanner" note carrying effort ①'s D5
  numbers.

`keep-efforts-whole` argues for folding H33 in; the human ruled against it here because H31 is
deliberately small and sequenced ahead of effort ②.

---

## 3. Mechanism (D2 = A — unique path, not a gate)

Fold the three identical helpers into **one** helper in `config.rs`'s test module whose path carries
a process-local `AtomicU64` counter, matching the `tempdir()` idiom already present in the same
module:

- One function (name to be fixed in the plan; `load_cfg` is the intended shape) taking the same
  `(name: &str, body: &str)` and returning the same `(Config, Vec<String>)`.
- `name` **stays** — it is what makes the call sites readable and is what a failure message would
  show. It no longer carries uniqueness.
- Uniqueness comes from `static N: AtomicU64` + `N.fetch_add(1, Ordering::Relaxed)` folded into the
  file name, so two concurrent calls can never name the same path even with identical `name`.
- The file name keeps the `wcartel-cfg-` prefix and stays distinct from `tempdir()`'s `wc-cfg-`
  directories; the pid component stays, so two `cargo test` binaries running concurrently still
  cannot collide.
- Body is otherwise unchanged: write → `load(slice::from_ref(&p))` → best-effort `remove_file` →
  return. The production call remains `load`, i.e. `load_with_fs(&RealFs, …)`; the `fsx` chokepoint
  is untouched and needs no new marker.

**Why not a gate.** A `SPAWN_GATE`/`SNAPSHOT_GATE`-style lock coordinates access to shared state
that has no reason to exist: two tests writing one path is a naming bug, not a resource conflict. A
gate leaves the trap armed for the next helper and serializes exactly at the concurrency where the
flake lives. **Why not an order-independent assertion.** The assertion means "the file *I* wrote
produced *this* warning"; there is no order-independent restatement that preserves the
mutation-verified guard — see §5.

---

## 4. Call sites under the fold, and observable behaviour

Five call sites move from three helper names to one. All are inside `config.rs`'s test module; the
helpers are private to it, so there are no external callers (confirmed: the `wcartel-cfg-` grep and
the helpers' lack of `pub`).

| Test | Was | Name argument |
|---|---|---|
| `files_type_filter_unknown_warns_and_defaults_documents` | `load_files` | `"unknown"` |
| `clipboard_provider_parses_all_values` (loop ×4) | `load_clip` | `"auto"`/`"native"`/`"osc52"`/`"off"` |
| `clipboard_provider_unknown_warns_and_defaults_auto` | `load_clip` | `"unknown"` |
| `harper_engine_table_overrides_grammar` | `load_diag` | `"harper-grammar"` |
| `linters_list_round_trips` | `load_diag` | `"linters"` |

**No test's asserted behaviour changes.** No test in the module asserts on the scratch path, its
name, or its existence; every assertion is over the returned `Config` / `Vec<String>`. The only
observable differences are (a) the on-disk file name gains a counter suffix, and (b) the two
`"unknown"` call sites now get distinct files, which is the fix. The `clipboard_provider_*` loop
already used distinct names and is unaffected in outcome — it simply routes through the folded
helper. `files_filters_default_on_absent`, `clipboard_provider_default_is_auto`, and the other
`load(&[])` tests touch no file at all and are untouched.

**Duplication note:** the fold is not incidental tidying — it is the durability mechanism (D3 = A).
Making the safe helper the *only* helper is what changes the ergonomics for the next author. No
scanner or guard test is added; effort ①'s D5 measured that answering a trust-in-gates problem with
another textual scanner is self-defeating (5 of 6 evasion routes uncaught on the existing one).

---

## 5. How the mutation-verified guard from `ea01138` survives

`ea01138` ("test(c5): pin the `[files]` default-on-absent + invalid-value guard (I2)") recorded that
`FilesConfig::default()` was correct but unguarded — flipping it to `{true, All}` broke zero tests
workspace-wide — and that the two new tests were mutation-verified: with the default flipped, both
failed; reverted, both passed.

This effort **does not touch either assertion expression**. The test
`files_type_filter_unknown_warns_and_defaults_documents` keeps
`assert_eq!(cfg.files.type_filter, FileTypeFilter::Documents)` and
`assert!(warns.iter().any(|w| w.contains("files.type_filter")))` verbatim; only a message argument
is appended to the second (§6), and only the helper it calls is renamed.
`files_filters_default_on_absent` is not modified at all. The mutation-verified property therefore survives by construction,
and §7 step 4 re-runs the original mutation to prove it rather than asserting it.

This is also the reason D2's option C (rewrite the assertion to be order-independent) was rejected:
any such rewrite would have to stop requiring that *this* input produced *this* warning, which is
precisely the property `ea01138` added.

---

## 6. Diagnosability (D4 = A, sequenced FIRST)

`assert!(warns.iter().any(...))` prints nothing about what `warns` held — part of why a ~17% flake
went untraced. Append a message argument to the two `"unknown"` tests' warning assertions, following
the pattern already used by `config_over_cap_degrades_like_an_unreadable_file`
(`"the warning must name the CAP, not merely any read failure: {warns:?}"`), so the panic prints the
actual `warns` vector.

**The human directed this be the first step, not a closing tidy-up.** Landing it before the path
change is what converts §1 step 5 from inference into observation: a failing run must then print the
`config: cannot read …` warning, directly evidencing the diagnosed mode. It also leaves the suite
self-diagnosing if this class recurs.

---

## 7. Acceptance criteria (binding — carried from `decisions.md` §"Verification contract")

These are gates for this effort, not suggestions. None may be softened, and none may be replaced by
an isolated or `--test-threads=1` run: **this flake is invisible at 1 thread and in isolation, so an
isolated green proves nothing.** Of every step below, effort ①'s question applies — could it print
PASS while the thing it names is false? A green whose runtime is implausibly small did not run.

1. **Pre-fix observation.** With the §6 messages in place and the paths still shared, run the whole
   test binary ~30× at default threading. A failing run **must** print the read-error warning in the
   `warns` vector. This is direct evidence of the mechanism, obtained before the fix exists.
2. **Post-fix measurement.** 60 whole-binary runs at default (32) threads. Failures attributed by
   parsing libtest's `failures:` **block** — never a bare test-name grep, since libtest prints the
   test name on passing runs too. Baseline 10/60 → required 0/60. At the measured 16.7% rate, 60
   clean runs is luck with probability ≈ 1.7×10⁻⁵. Expect ~4–5 min total; a far faster green did not
   run.
3. **Attribution check.** On a scratch branch, revert **only** the uniqueness (restore the shared
   `"unknown"` name, keeping the fold and the messages) and confirm the flake **returns** within ~30
   runs. This proves the unique path is the operative change rather than an incidental timing shift
   introduced by the fold. Effort ① found a fix that would have gone green for an unrelated reason;
   this step is what prevents a decorative mechanism.
4. **Guard preservation.** Re-run the `ea01138` mutation: flip `FilesConfig::default()` to
   `{ show_clutter: true, type_filter: FileTypeFilter::All }`, confirm **both** `files_*` tests
   fail, then revert. Proves the load-bearing assertion still bears load after the fold.
5. **Standard gates.** `cargo test` green; `cargo build` and `cargo test --no-run` warning-free for
   `wordcartel`; `cargo clippy --workspace --all-targets` clean; `scripts/smoke/run.sh` run and its
   one-line summary quoted verbatim in the pre-merge report (advisory-pass).

---

## 8. Decision conformance

| Decision | Ruling | Where honoured |
|---|---|---|
| D0 | Collision is the cause; asymmetry explained; `Err(NotFound)` correction | §1 |
| D1 | Scope = C (collision class only; H33 filed) | §2 |
| D2 | Mechanism = A (unique path via `AtomicU64`; fold; no gate) | §3, §4 |
| D3 | Durability = A now (the fold), C filed as H32; **no scanner** | §4 |
| D4 | Diagnosability = A, sequenced **pre-fix** | §6, §7 step 1 |

No decision required refinement; nothing in the source contradicted a ruling.

---

## 9. Open questions for the human

**None blocking.** Two items are recorded as explicitly *not* open, to prevent a reviewer reopening
them: the folded helper's exact identifier and the exact counter placement within the file name are
plan-level details within D2's ruling, not design forks; and the H33/H32 deferrals are ruled, not
pending.

One residual fact worth stating plainly rather than papering over: §1 step 5 is, until acceptance
criterion 1 runs, an **inference** from the code paths, not an observation — the current logs cannot
distinguish `Err(NotFound)` from "parsed the clipboard's TOML". Both are the same root cause and
both fail the same assertion, so no design choice depends on which fires; criterion 1 exists
precisely to settle it before the fix lands.
