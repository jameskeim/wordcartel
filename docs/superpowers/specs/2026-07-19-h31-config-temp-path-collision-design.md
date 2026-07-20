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

**Why 10/60 for F and 0/60 for C — a strongly-supported explanation, NOT a proof.** The mechanism is
symmetric; the reachable interleaving space appears not to be. Every failure mode requires the
opposing event to land inside the victim's write→read gap, and that gap closes at `File::open`
(POSIX unlink of an already-open fd is harmless). Three of the four modes require F to *start* at or
before C. libtest's default order is alphabetical and its runner pops from the front while
`pending < concurrency`, so C is **dispatched** first, which biases the offset's sign and makes only
"C's `remove_file` kills F's file" likely.

Three limits on that argument, stated so no reviewer mistakes it for exclusion:

- **Dispatch order is not execution order.** Once threads are spawned, the OS may schedule a
  later-dispatched test sooner. The symmetric modes are improbable, not impossible; 0/60 for C is
  consistent with a small rate, not with a rate of zero.
- **`--shuffle` / `RUST_TEST_SHUFFLE` voids the assumption entirely**, randomizing order and making
  every mode roughly equally reachable. The verification runs in §7 must therefore not use shuffle,
  and must assert that they did not (§7.2) — a shuffled run silently changes what the measurement
  means.
- Accordingly: **the clipboard test is not safe — it is shielded by scheduling.** Both tests carry
  the same defect. Fixing the collision fixes both, and the fix does not depend on which direction
  fires.

**Verified clean against the current tree, so out of the fix:**
`config_over_cap_degrades_like_an_unreadable_file` builds its own path,
`wc-cfg-cap-{pid}/config.toml`, with no counter. It **does not collide with the current helpers or
call sites** — the prefix differs and it is the sole user of that scheme — but it is not
collision-*proof* by construction, and is a latent instance of exactly what H32's crate-wide seam
would remove. The `tempdir()` helper in the same module uses `static N: AtomicU64` with prefix
`wc-cfg-{pid}-{n}` and is already correct — it is the idiom this spec adopts.

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
`files_filters_default_on_absent` is not modified at all. The mutation-verified property therefore
survives by construction, and §7 criterion 4 proves it rather than asserting it.

One correction to `ea01138`'s own record, carried into §7.4: the commit's mutation (flipping
`FilesConfig::default()`) validates the **default-on-absent** guard but **not** the warning
assertion — with the default flipped, the warning test fails at its first assertion and never
reaches the second. Mutating the `other =>` arm's `warns.push` is what exercises the warning guard,
which is why §7.4 requires both mutations. This does not weaken `ea01138`'s conclusion that
`[files]` needed guarding; it narrows which of its two tests that particular mutation proved.

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

### 7.0 The mutation rule (binding on this spec AND on the plan's verification steps)

This criterion set got the same defect wrong **twice** — round 1 in the original §7.4, round 2
inside the fix for it. Both instances had one shape: **a struct-wide flip meeting a multi-assertion
test.** Rust's `assert!`/`assert_eq!` panic on the first failure, so a mutation that changes several
fields read by a test with several assertions proves only that *something* broke; every assertion
after the first is short-circuited away and could be deleted while the step still went green.

**The rule, stated once so nothing downstream re-derives it a third time:**

> A mutation must change **exactly one** property, and the required outcome must name the **one
> assertion** that must fail — never merely "the test fails". Where a single-property mutation is
> not possible, the required outcome must identify the failing assertion by its **custom message**
> (or `file:line`), and the step must state why the narrower mutation was unavailable.

Prefer splitting the mutation over reasoning about assertion order: a one-to-one mutation/assertion
pairing removes the ordering question entirely, and ordering is exactly what both misses relied on.

**Identification, precisely:** once §6 adds custom `assert!` messages, a failing assertion prints
the **custom message**, not the raw assertion expression. Steps below therefore identify failures by
custom message and/or `file:line` — never by "the assertion text".

1. **Pre-fix observation.** With the §6 messages in place and the paths still shared, run the whole
   test binary ~30× at default threading, no shuffle. Passing requires **at least one failing run**;
   that the failing test is **`files_type_filter_unknown_warns_and_defaults_documents`** failing at
   its **warning assertion** (by §6 custom message, per §7.0); and that the printed `{warns:?}`
   contains the read-error string (`config: cannot read`). **Zero failures in 30 runs is an
   INCONCLUSIVE result — not a pass**: re-run with more iterations or escalate. (At 16.7%, 30 clean
   runs has probability ≈ 0.4% — unlikely, not impossible.) Record the observed `warns` verbatim;
   criterion 3 compares against it.
2. **Post-fix measurement.** 60 whole-binary runs at **32 threads, pinned and recorded** — the
   conditions the 10/60 baseline was measured under. The thread count must be *set* and reported
   rather than inferred from `nproc`, because libtest falls back to
   `std::thread::available_parallelism()`, which diverges from `nproc` under cgroup limits or CPU
   affinity; an inferred figure could record 32 for a run performed with far fewer, false-greening
   the very concurrency the measurement depends on. **No `--shuffle` and no `RUST_TEST_SHUFFLE`**
   (assert this, per §1 — a shuffled run changes what the measurement means). Baseline 10/60 → required 0/60. At the measured 16.7% rate, 60 clean runs is
   luck with probability ≈ 1.7×10⁻⁵. Run-integrity checks, all required, carried from the grounding
   map's audited protocol — parsing the `failures:` block **alone** goes false-green if a run aborts
   before printing it, output is dropped, or the wrong binary/filter ran:
   - the test binary is selected from cargo's JSON artifact stream, not an `ls -t` glob, and
     **both** `files_type_filter_unknown_warns_and_defaults_documents` **and**
     `clipboard_provider_unknown_warns_and_defaults_auto` are confirmed present via `--list`. This
     closes the most direct false-green available to this effort: 0/60 is also what you get if the
     fold silently dropped or renamed the flaky test out of the suite;
   - all 60 logs exist;
   - each log has **exactly one** `test result:` line;
   - **`passed + failed` equals a DERIVED expected total on every run** — never a hard-coded
     constant, because any task that adds a `#[test]` changes it. The derivation is: baseline
     **1776** (`main` @ `60be3d1`: 1777 `#[test]` attributes under `wordcartel/src`, minus the one
     `#[ignore]`d `r1_typing_latency_bench` in `e2e.rs`) **+ `#[test]`s the branch adds − removes**.
     The fold itself removes helper *functions*, not tests; the branch's uniqueness unit test adds
     one, so the post-fix total is **1777**. Any other total, high or low, is a failed integrity
     check, not a pass. Stating `passed + failed` rather than `passed` alone keeps the check usable
     on the pre-fix runs, where failures are expected;
   - failures attributed by parsing libtest's `failures:` **block** — never a bare test-name grep,
     since libtest prints the test name on passing runs too;
   - total runtime ~4–5 min; a far faster green did not run.
3. **Attribution check.** On a scratch branch, revert **only** the uniqueness (restore the shared
   `"unknown"` name, keeping the fold and the messages) and confirm the flake returns within ~30
   runs — **and that the returned failure's `{warns:?}` matches the mechanism recorded in criterion
   1**, not merely that the same test fails at the same assertion. Two distinct sub-mechanisms
   (`Err(NotFound)`, or having read C's `[clipboard]` TOML and collected only a `clipboard.provider`
   warning) produce an identical panic line, so without the `warns` comparison this step would only
   prove that shared naming reintroduces *a* flake, not *this* one. D4's message argument is what
   makes the tighter check cheap. Effort ① found a fix that would have gone green for an unrelated
   reason; this step is what prevents a decorative mechanism.

   Two further conditions, from re-asking §7.0's question of this step: **(i)** the scratch revert
   must be confined to the uniqueness token — confirm by `git diff` that the only change is the
   path-construction expression, since a broader revert would re-establish the flake for reasons
   this step would then misattribute; **(ii)** failing to reproduce within ~30 runs is
   **INCONCLUSIVE, not a pass** (same ≈0.4% luck floor as criterion 1) — this step passes only on a
   positive, mechanism-matched reproduction, never on absence.
4. **Guard preservation — mutate the WARNING ARM, not the default.** The `ea01138` mutation as
   originally recorded (flip `FilesConfig::default()` to `{ show_clutter: true, type_filter: All }`)
   **cannot validate the warning assertion** and must not be used alone for that purpose: with the
   default flipped, `files_type_filter_unknown_warns_and_defaults_documents` fails at its **first**
   assertion (`cfg.files.type_filter == Documents`) and never evaluates the warning assertion, which
   could be deleted outright while this step still went green. The two assertions test genuinely
   different things, because `load_with_fs`'s `other =>` arm only pushes the warning — it does
   **not** assign `cfg.files.type_filter = Documents`; the `Documents` value observed by the first
   assertion comes from `Config::default()`. Required instead, both parts:
   Both parts obey §7.0: one property per mutation, one named assertion per outcome.
   - **(a) Warning-arm mutation** — target: the warning guard. Remove or alter **only** the
     `warns.push(...)` in the `other =>` branch of the `raw.files.type_filter` match, changing
     nothing else. Required outcome:
     `files_type_filter_unknown_warns_and_defaults_documents` fails **specifically at the warning
     assertion** — identified by the §6 custom message (and/or its `file:line`), since that message,
     not the assertion expression, is what Rust prints. A failure at the test's first assertion
     instead means the mutation was not confined to the warning arm; that is a FAILED step, not a
     pass. Revert.
   - **(b) Single-field default mutation** — target: the default-on-absent guard. Flip **only**
     `FilesConfig::default()`'s `type_filter` to `FileTypeFilter::All`, **leaving `show_clutter` at
     `false`**. Required outcome: `files_filters_default_on_absent` fails **specifically at its
     `"files.type_filter must default to Documents"` assertion**. Revert.

     **Why the split rather than `ea01138`'s struct-wide flip:** that test asserts `show_clutter`
     **first** and `type_filter` second, so the recorded `{ show_clutter: true, type_filter: All }`
     mutation kills it at the `show_clutter` assertion and never evaluates the `type_filter` one —
     which could then be deleted while this step still went green. Flipping one field makes the
     mutation target and the asserted property one-to-one and removes the need to reason about
     assertion order at all (§7.0). Optionally flip `show_clutter` alone as a separate mutation to
     cover that assertion too; it is not load-bearing for H31 and is not required here.

   **On the record: this criterion carried the very defect class §0 and the effort-① lesson warn
   about — twice.** Round 1 caught the original form (a step that would have printed PASS while "the
   warning assertion still bears load" was false). Round 2 caught the *fix* reproducing the identical
   shape one level down, in 4(b). Both were caught by the Codex spec gate, not by their author, and
   both came from the same generator: a struct-wide mutation meeting a multi-assertion test. That is
   why §7.0 now states the rule once, rather than leaving each step to re-derive it — and why
   criteria 1, 2, 3, and 5 were re-interrogated against the same question in this revision.
5. **Standard gates.** `cargo test` green; `cargo build` and `cargo test --no-run` warning-free for
   `wordcartel`; `cargo clippy --workspace --all-targets` clean; `scripts/smoke/run.sh` run and its
   one-line summary quoted verbatim in the pre-merge report (advisory-pass).

   Re-asked against §7.0: **a warning-free build proves nothing if nothing was rebuilt.** A cached
   `cargo build`/`clippy` over an already-compiled tree emits no warnings by construction, so the
   build and clippy runs must actually recompile the touched crate — `touch wordcartel/src/config.rs`
   first (the project's own remedy for stale-analysis questions), and treat an implausibly fast
   "clean" as not having run, exactly as criterion 2 treats a fast green. A `smoke: SKIP — …` line is
   quoted verbatim as required, but is **not** evidence the smoke suite passed.

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

Three residuals stated plainly rather than papered over, all held to the same standard:

1. **The read-error mechanism is inference until criterion 1 runs.** §1 step 5 is derived from the
   code paths, not observed — the current logs cannot distinguish `Err(NotFound)` from "parsed the
   clipboard's TOML". Both are the same root cause and both fail the same assertion, so no design
   choice depends on which fires; criterion 1 exists precisely to settle it before the fix lands,
   and criterion 3 then holds the post-fix evidence to the same mechanism.
2. **The asymmetry explanation is not an exclusion proof.** Dispatch order is verified alphabetical,
   but dispatch order is not execution order, so the symmetric failure modes are improbable rather
   than impossible, and 0/60 for the clipboard test is evidence of a low rate, not a zero one. The
   fix does not depend on resolving this: the collision is a defect in both directions and is
   removed in both.
3. **The measurement's validity is conditional on no shuffle.** `--shuffle` /
   `RUST_TEST_SHUFFLE` would void the ordering assumption silently, which is why §7.2 requires
   asserting their absence rather than assuming it.
