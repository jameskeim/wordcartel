# H31 Task 3 — post-fix measurement and the attribution check

Both results were produced with the single audited harness `scratchpad/h31-gates/run_n.sh`,
committed by Task 1 at `940b061` and **verified unmodified** before use (`git diff HEAD --
scratchpad/h31-gates/run_n.sh` was empty). It was consumed, never edited.

`expected_total = 1777`, derived two independent ways (§0 below), not taken as a constant.

---

## §0 — `expected_total` derivation

Per the brief: baseline **1776** (`main` @ `60be3d1`) + `#[test]`s this branch adds − removes.

```
git diff main --unified=0 -- 'wordcartel/src/**' | grep -c '^+.*#\[test\]'   → 1
git diff main --unified=0 -- 'wordcartel/src/**' | grep -c '^-.*#\[test\]'   → 0
```

1776 + 1 − 0 = **1777**. The one addition is Task 2's
`scratch_cfg_paths_are_unique_even_for_one_name`.

**Second, independent derivation (not required by the brief).** The lexical `grep` count is
not Rust-aware, so it was cross-checked against the actual binary:

```
"$BIN" --list | grep -c ': test$'   → 1778
```

1778 listed = 1777 runnable + the 1 `#[ignore]`d `r1_typing_latency_bench`, which is exactly
the `1777 passed; 0 failed; 1 ignored` shape observed on every clean run. The two derivations
agree from opposite directions (source diff vs. compiled artifact).

**Binary-identity check (not required by the brief).** The binary path
`target/debug/deps/wordcartel-801988c60b196269` is byte-for-byte the same *filename* recorded in
Task 1's pre-fix `observation-prefix.md` — cargo's metadata hash does not change when source
changes, so an identical path is NOT evidence of an identical build and was worth disproving:

- binary mtime `2026-07-20 00:54:17` is **after** the fix commit `be22a1d` (`2026-07-20 00:49:44`);
- `scratch_cfg_paths_are_unique_even_for_one_name` is present in `--list` (it does not exist pre-fix).

The `passed + failed == 1777` pin is itself the durable guard here: a stale pre-fix binary has
1776 runnable tests and would have hard-failed every run.

---

## §1 — Part A: post-fix measurement (spec §7.2)

```
scratchpad/h31-gates/run_n.sh 200 "$OUT" 1777
```

`SUMMARY:` line, **verbatim**:

```
SUMMARY: runs=200 failures=0 expected_total=1777 threads=32 binary=/home/jkeim/projects/groundwords/target/debug/deps/wordcartel-801988c60b196269
```

Harness exit code: **`rc=0`** — trustworthy, not void. No `FATAL:` line was emitted.

**Result: 0 failures in 200 runs at 32 threads.** Meets the ruled criterion.

### Plausibility (an implausibly fast green did not run)

- 200 of 200 `run-N.log` files present.
- Wall time **835 s ≈ 13 m 55 s** (`run-1.log` 00:57:05 → `run-200.log` 01:11:00), consistent with
  the ~4.2 s per-run suite time and the brief's ~15–20 min estimate.
- Every one of the 200 `test result:` lines has the identical clean shape:
  `test result: ok. 1777 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out`
  (only the `finished in` duration varies, 4.08–4.35 s).

### Harness guards that were live on this run

Confirmed present in the copy executed (unmodified since `940b061`): binary from cargo's JSON
artifact stream; **both** `files_type_filter_unknown_warns_and_defaults_documents` and
`clipboard_provider_unknown_warns_and_defaults_auto` checked present by exact `--list` line match;
per-file `test result:` line count pinned to exactly 1; `filtered = 0`; `passed + failed == 1777`;
failures attributed by parsing the `failures:` block rather than a bare name grep; completed-
iteration count reported rather than the requested count.

### A void first attempt (disclosed)

A first 200-run attempt was launched inside the agent's own process group and was **killed
externally after 5 runs** (`run-5.log` truncated mid-suite with no `test result:` line; no
`SUMMARY:`, no `FATAL:`, no `rc=`). No number from that attempt is interpreted or reported —
there was none to interpret. The measurement above is a clean re-run launched detached via
`setsid`, and its completion was confirmed by blocking on the harness PID rather than by
inspecting a partial log.

---

## §2 — Part B: attribution check (spec §7.3)

Scratch branch `h31-attribution-scratch` off `be22a1d`; **discarded, never merged**.

### The revert — uniqueness only

The `static N` and the counter component were removed from `scratch_cfg_path`, returning the
format string to `"wcartel-cfg-{}-{name}.toml"` with only `std::process::id()`. The fold, the
messages, the call sites and the guard test were left untouched.

```
 wordcartel/src/config.rs | 7 ++-----
 1 file changed, 2 insertions(+), 5 deletions(-)
```

```diff
     fn scratch_cfg_path(name: &str) -> PathBuf {
-        use std::sync::atomic::{AtomicU64, Ordering};
-        static N: AtomicU64 = AtomicU64::new(0);
         std::env::temp_dir().join(format!(
-            "wcartel-cfg-{}-{name}-{}.toml",
-            std::process::id(),
-            N.fetch_add(1, Ordering::Relaxed)
+            "wcartel-cfg-{}-{name}.toml",
+            std::process::id()
         ))
     }
```

Confinement confirmed: one file, one expression, inside `scratch_cfg_path` only. The diff reaches
no assertion, no call site, and no other function.

### Result

`SUMMARY:` line, **verbatim**:

```
SUMMARY: runs=30 failures=30 expected_total=1777 threads=32 binary=/home/jkeim/projects/groundwords/target/debug/deps/wordcartel-801988c60b196269
```

Harness exit code: **`rc=0`**. No `FATAL:` line.

### Deviation from the brief, with reason — why `failures=30` is NOT the signal

The brief anticipated reading the aggregate `failures=` count as the attribution signal. **On this
branch that count is confounded and carries no attribution information.** Task 2's guard test
`scratch_cfg_paths_are_unique_even_for_one_name` fails *deterministically* once uniqueness is
reverted — that is precisely what the guard is for — so `failures=30` would print identically
whether or not the flake returned. Reading it as "the flake came back" would be exactly the defect
class this effort exists to remove: a number that reads as PASS while the thing it names is
unverified.

Attribution was therefore done per-test, by parsing the `failures:` block of each of the 30 logs.
The guard test was **not** removed to tidy the count, because the `git diff` confinement above is
what proves the uniqueness change is the operative one; a broader revert would buy a neater number
at the cost of the evidence.

**Reusable lesson for the next reader of this plan: a guard test added by an earlier task can
confound a later task's aggregate failure count. Attribute by test name, not by the total.**

The confound also turned out to be an asset — the guard test is a **positive control** proving the
revert actually took effect, which the diff alone cannot establish.

### The three quantities, reported separately

Per-test failure tallies across the 30 runs (parsed from the `failures:` block):

| test | runs failed | role |
|---|---|---|
| `config::tests::scratch_cfg_paths_are_unique_even_for_one_name` | **30 / 30** | positive control — revert landed |
| `config::tests::files_type_filter_unknown_warns_and_defaults_documents` | **3 / 30** (runs 1, 11, 18) | **the attribution signal — the flake RETURNED** |
| `config::tests::clipboard_provider_unknown_warns_and_defaults_auto` | 1 / 30 (run 1) | the *other collider* — same mechanism |
| `cursor_style::tests::restore_caret_if_written_gated_by_latch` | 1 / 30 (run 19) | **unrelated pre-existing flake — see §3** |

1. **Positive control: 30/30 as required.** The revert demonstrably took effect.
2. **Attribution signal: 3/30 ≈ 10%**, consistent with the load-dependent pre-fix range
   (4/60 quiet, 10/60 under load). The flake returned.
3. **Third names: two appeared** — one expected (the sibling collider), one genuinely unrelated
   (§3). Reported rather than suppressed.

### Integrity pin held

`passed + failed == 1777` on **every one of the 30 runs** — verified directly from the logs, not
assumed from `rc=0`. Observed result-line shapes, all summing to 1777:

```
test result: FAILED. 1774 passed; 3 failed; 1 ignored; 0 measured; 0 filtered out
test result: FAILED. 1775 passed; 2 failed; 1 ignored; 0 measured; 0 filtered out
test result: FAILED. 1776 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out
```

### Reproduced panic block — mechanism comparison

Run 11, verbatim:

```
thread 'config::tests::files_type_filter_unknown_warns_and_defaults_documents' (2596547) panicked at wordcartel/src/config.rs:1361:9:
the invalid-value arm must warn by name (H31 diagnostic); warns was: ["config: cannot read /tmp/wcartel-cfg-2596152-unknown.toml: No such file or directory (os error 2)"]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

Runs 1 and 18 are identical in form (differing only in pid):

```
run  1: warns was: ["config: cannot read /tmp/wcartel-cfg-2576223-unknown.toml: No such file or directory (os error 2)"]
run 18: warns was: ["config: cannot read /tmp/wcartel-cfg-2610075-unknown.toml: No such file or directory (os error 2)"]
```

**Comparison against `observation-prefix.md` (Task 1, pre-fix):**

| property | pre-fix record | reproduced here | match |
|---|---|---|---|
| failing test | `files_type_filter_unknown_warns_and_defaults_documents` | same | ✓ |
| assertion | the warning-presence `assert!`, by custom message `the invalid-value arm must warn by name (H31 diagnostic)` | same message | ✓ |
| `warns` content | signature 1 — `config: cannot read …: No such file or directory (os error 2)` | same, all 3 runs | ✓ |
| path shape | `/tmp/wcartel-cfg-<pid>-unknown.toml` (no counter) | same | ✓ |

All three reproductions match **signature 1 (deleted file, `Err(NotFound)`)** — the variant seen in
3 of Task 1's 4 pre-fix failures. Signature 2 (torn write / truncated TOML) did not appear in this
sample; the brief accepts either, and its absence in 3 draws is unremarkable given it was 1 of 4
pre-fix. **No failure matching neither signature occurred**, so the STOP condition was not met.

The line number moved (`1340` pre-fix → `1361` post-fold) because Task 2's fold and its new guard
test shifted the file; the assertion is identified by its custom message, not by line, exactly as
the plan's anchor-on-names rule requires.

### Pass condition (spec §7.3)

- [x] the flake **returned** on a positive reproduction (3/30), not an absence
- [x] the failing test is `files_type_filter_unknown_warns_and_defaults_documents`
- [x] it failed at the **warning assertion**, identified by custom message
- [x] its printed `warns` **matches a mechanism recorded in `observation-prefix.md`** (signature 1)
- [x] the revert was confined to the path-construction expression
- [x] positive control 30/30 confirms the revert took effect

**Attribution established: the unique path is the operative change.** The improvement is not a
coincidental side effect of the fold or the message edits — restoring the shared path, and nothing
else, brings the flake back with the recorded mechanism.

---

## §3 — Incidental finding: an unrelated flake (`cursor_style`)

Run 19 additionally failed:

```
thread 'cursor_style::tests::restore_caret_if_written_gated_by_latch' (2612466) panicked at wordcartel/src/cursor_style.rs:168:13:
never wrote → restore emits nothing
```

**This is not caused by the revert and is not part of H31.** The revert touches only
`wordcartel/src/config.rs`; there is no mechanism by which a config scratch-path change reaches
`cursor_style`, which fails on its own process-global restore latch (the C1 caret latch is
process-global and therefore shared across concurrently-running tests in one binary).

**Honest limits of this observation.** It did not appear in any of the 200 post-fix runs (0/200)
and appeared 1/30 here. Those two rates do not sit comfortably together — a 3.3% rate would be
unlikely to yield 0 in 200 — so I am **not** claiming the numbers positively exonerate the revert;
I am claiming there is no mechanism connecting them, and that a single observation of a
process-global-latch race is weak evidence of any rate at all. The plausible reading is a rare
latent race whose window is widened by the extra panics/unwinding on this branch perturbing
scheduling, but that is speculation and is recorded as such.

**Recommendation: file this as a separate item; do not fold it into H31.** Flagging it for Task 4
and for the effort's final gate rather than silently absorbing it.

---

## §4 — Sibling collider note (`clipboard_provider_unknown_warns_and_defaults_auto`)

Run 1 also failed the sibling test — the *other* caller that passes `"unknown"`:

```
thread 'config::tests::clipboard_provider_unknown_warns_and_defaults_auto' (2576606) panicked at wordcartel/src/config.rs:1382:9:
the invalid-value arm must warn by name (H31 diagnostic); warns was: []
```

This is the same shared-path collision viewed from the other side, and its appearance is
corroborating rather than anomalous — it is the second of the two colliders named in
`DISPATCH-CONSTRAINTS.md`. Its `warns` is `[]` (it read a file that was empty or already replaced)
rather than either recorded signature; **the two recorded signatures were catalogued for
`files_type_filter`, and the brief's STOP condition is scoped to that test**, all three of whose
reproductions matched signature 1. Recording the sibling's distinct sub-variant here for
completeness so a future reader is not surprised by it.

---

## §5 — Restoration

```
git switch effort-h31-config-temp-path
git branch -D h31-attribution-scratch     # "Deleted branch h31-attribution-scratch (was be22a1d)"
git status --short                        # empty — clean
```

Working tree clean, branch `effort-h31-config-temp-path`, HEAD `be22a1d`, and
`scratch_cfg_path` confirmed back to the counter form (`"wcartel-cfg-{}-{name}-{}.toml"`).
The scratch edit was never committed, so the branch deleted at `be22a1d` with nothing to lose.
