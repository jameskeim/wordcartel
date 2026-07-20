# H31 — decisions ratified by the human

Brainstorm 2026-07-19, after the grounding sweep (`map-grounding.md`) and Fable's analysis pass.
Each entry records the choice AND the reasoning, so a later reader can tell whether a new fact
should reopen it.

---

## D0 — The cause, established before any fork was ruled

**The shared temp path IS the cause, and the asymmetry is explained.**

`load_files`, `load_clip`, `load_diag` (all three byte-for-byte identical) build a path from only
`temp_dir()` + `process::id()` + a caller-supplied name. The pid is constant across every test in
one binary, so the name is the sole uniqueness token — and two callers pass `"unknown"`:

- `files_type_filter_unknown_warns_and_defaults_documents` → `load_files("unknown", …)`
- `clipboard_provider_unknown_warns_and_defaults_auto` → `load_clip("unknown", …)`

Both resolve to `${TMPDIR}/wcartel-cfg-<pid>-unknown.toml`.

**The failing interleaving** (C = clipboard test, F = files test):
1. libtest dispatches in sorted-name order, so C starts first.
2. C writes, reads, asserts — passes.
3. F writes the same path, after C's read but before C's `remove_file`.
4. C's `remove_file` deletes the file F just wrote.
5. F's `read_capped` → `File::open` → `Err(NotFound)`; `load_with_fs` pushes
   `"config: cannot read {}: {e}"` and continues. `warns` therefore lacks `"files.type_filter"`.
6. `:1339` passes anyway (`Documents` is the struct default, so that assertion cannot distinguish
   "parsed my file" from "parsed nothing"); `:1340` panics.

**Why 10/60 for F and 0/60 for C.** The mechanism is symmetric; the reachable interleaving space is
not. Every failure mode needs the opposing event inside the victim's write→read gap (the gap closes
at `File::open` — POSIX unlink of an already-open fd is harmless). Three of the four modes require
F to start at or before C. Sorted-order dispatch pins the sign of that offset, so only the
"C's remove kills F's file" mode is reachable. **The clipboard test is not safe — it is shielded by
scheduling.** Fixing the collision fixes both.

Correcting the grounding map: `RealFs::read_capped` opens with `File::open(path)?`, so a missing
file is `Err(NotFound)`, NOT `Ok(None)`. `config_over_cap_degrades_like_an_unreadable_file` uses a
distinct prefix (`wc-cfg-cap-{pid}/config.toml`) and is clean.

Measured baseline: **10/60 failures at default (32) threading; 0/20 at `--test-threads=1`.** All 10
were this one test, byte-identical panic, attributed by parsing the `failures:` block.

---

## D1 — Scope: fix the collision class only; FILE the HOME hazard

**Chosen: C** (of A collision-only / B collision + HOME fix / C collision + file HOME).

Fixing the three `config.rs` helpers closes the whole `config.rs` collision class
(`config_over_cap` verified clean). The `set_var("HOME")` hazard in `file_browser_commit.rs` is the
same *genus* — a test observing process state another test mutates — but a different *species*: it
has its own unresolved mechanism fork (inject / gate / stop using real `HOME` as an oracle), and it
has **never been observed to fire** in measurement.

`keep-efforts-whole` argues for B. It loses here because H31 is deliberately small and sequenced
immediately before the caret/prose-window group (effort ②), and B would convert a one-file fix into
a second design problem. Filed as **H33** with full grounding, including the edition-2024
`unsafe set_var` forcing function.

---

## D2 — Mechanism: make the path unique, do not gate

**Chosen: A** (of A unique-path / B gate / C order-independent assertion).

Fold the three byte-identical helpers into ONE whose path incorporates the `AtomicU64` counter — the
idiom already present at `config.rs:937` and in 13 other modules (see H32). The `name` parameter may
stay for readability; the counter carries uniqueness.

**C is not genuinely available.** The assertion means "the file *I* wrote produced *this* warning."
There is no order-independent restatement preserving the mutation-verified guard from `ea01138`;
C would require weakening precisely the property the test exists to pin.

**A over B:** a gate coordinates access to shared state that has no reason to exist. Two tests
writing one path is a naming bug, not a resource conflict. B leaves the trap armed for the next
helper and would slow the suite at exactly the concurrency where the flake lives. A removes the
sharing, matches the local idiom, and the DRY fold makes the safe helper the only helper.

The assertion at `:1340` is untouched, so the mutation-verified property survives verbatim.

---

## D3 — Durability: the fold is the mechanism; file the crate-wide seam

**Chosen: A now, and FILE C** (of A fold-only / B guard scanner / C crate-wide seam).

**B is actively rejected.** It contradicts effort ①'s decision D5, which measured what the existing
`fs_chokepoint` textual scanner actually buys: 5 of 6 evasion routes uncaught, its own 7 self-checks
covering none of them, 37 of 51 markers being blanket wrapper exemptions never semantically checked.
Answering a trust-in-gates problem with a second textual scanner is self-defeating.

The governing distinction: **the fold changes what is *ergonomic*, which is durable. A scanner only
changes what is *caught*, which is the thing D5 measured as leaky.**

C is the real structural answer but touches ~13 modules — the opposite of a small effort, landing
right before the group that wants quiet tooling. Filed as **H32**, carrying the census and an
explicit "do not answer this with a scanner" note.

---

## D4 — Diagnosability: add the assertion message, as the FIRST step

**Chosen: A** (of A add `{warns:?}` messages / B leave as-is), **sequenced pre-fix, not as cleanup.**

`assert!(warns.iter().any(...))` prints nothing about what `warns` held. That is part of why a
10-16% flake went untraced for months, and it is a live gap in the analysis above: the logs cannot
distinguish `Err(NotFound)` from "read the clipboard's TOML", so D0 step 5 is **inference, not
observation**.

Add `, "…: {warns:?}"` to the two `unknown` tests (the pattern `config.rs:1446` already uses)
**before** changing the paths, then re-run at default threading and watch the diagnosed mechanism
print itself. This converts the verification from "the rate went to zero" into "I observed the
cause, then observed it stop" — the attribution discipline effort ① proved load-bearing — and
leaves the suite self-diagnosing if this class recurs.

---

## Verification contract (binding on the spec and plan)

1. **Pre-fix observation.** With the `{warns:?}` message in place, run the whole binary ~30× at
   default threading; a failing run must show the read-error warning. Direct evidence of the mode.
2. **Post-fix.** 60 whole-binary runs at default 32 threads, failures attributed by parsing the
   `failures:` block — NOT a bare test-name grep (libtest prints the name for passing runs too).
   Baseline 10/60 → expect 0/60. At a true 16.7% rate, 60 clean runs is luck with probability
   ≈ 1.7×10⁻⁵. Expect ~4-5 min runtime; an implausibly fast green did not run.
3. **Attribution check.** On a scratch branch, revert ONLY the uniqueness (restore the shared
   `"unknown"` name) atop the otherwise-fixed tree; confirm the flake RETURNS within ~30 runs. This
   proves the unique path is the operative change, not an incidental timing shift from the fold.
4. **Guard preservation.** ~~Re-run the `ea01138` mutation: flip `FilesConfig::default()` to
   `{true, All}`, confirm both `files_*` tests fail, revert.~~ **SUPERSEDED — see the amendment
   below. As written this step COULD NOT FAIL in the way it claimed.**
5. Standard gates: `cargo test`, warning-free build for touched crates, `cargo clippy --workspace
   --all-targets` clean, smoke suite run and its one-line summary quoted verbatim.

---

## AMENDMENT 2026-07-19 — criterion 4 was defective as ratified

Recorded rather than silently rewritten, because the superseded text was human-ratified and the
correction is instructive.

**What was wrong.** Criterion 4 above claimed that flipping `FilesConfig::default()` to
`{true, All}` and watching both `files_*` tests fail proves the warning assertion still bears load.
It does not. With the default flipped, `files_type_filter_unknown_warns_and_defaults_documents`
fails at its **first** assertion (`cfg.files.type_filter == Documents`) and never evaluates the
warning assertion — which could be deleted outright while this step still went green. The two
assertions test different things: `load_with_fs`'s `other =>` arm only pushes the warning, it does
not assign `cfg.files.type_filter`; the `Documents` the first assertion sees comes from
`Config::default()`.

Caught by the Codex spec gate (round 1), not by its author, not by the controller who carried it
into this record, and not at ratification. The first fix — retargeting the mutation at
`files_filters_default_on_absent` — **reproduced the same defect one level down**, because that test
asserts `show_clutter` first and the mutation flipped both fields. Caught by gate round 2.

**What governs now** (spec §7.4, plan Task 4), two mutations, each one property, each naming the one
assertion that must fail:
- **(a)** Remove/alter the `warns.push` in the `raw.files.type_filter` `other =>` arm as a
  *compiling* no-op; require failure **specifically at the warning assertion**. A build error is
  NOT a passing outcome. A failure at the test's first assertion means the mutation was not confined
  to the warning arm — that is a FAILED step.
- **(b)** Flip `type_filter` **alone** to `All`, leaving `show_clutter` at `false`; require failure
  specifically at the `files.type_filter must default to Documents` assertion of
  `files_filters_default_on_absent`.
- `git diff --exit-code -- wordcartel/src/config.rs` after EACH revert, before the next mutation —
  confirming the target test passes again does not prove byte-for-byte reversion.

**The rule this produced** (spec §7.0, inherited by the plan): *a mutation must change exactly ONE
property, and the required outcome must name the ONE assertion that must fail* — never merely "the
test fails". The generator of both misses was a struct-wide flip meeting a multi-assertion test,
with short-circuit evaluation eating everything after the first assertion.

**Also superseded:** criterion 2's "expect 0/60" is necessary but not sufficient — 0/60 is equally
what you get if the fold drops or renames the test. It now additionally pins
`passed + failed == expected_total` (a DERIVED count: baseline 1776 + tests this branch adds, not a
magic constant) and requires a `--list` presence check on both `unknown` tests.

## Standing context

- **There is no CI.** Every stated gate runs only because a human or agent follows CLAUDE.md.
- Do NOT run `cargo fmt` — hand-formatted repo, no `rustfmt.toml`.
- Effort ① lesson binding every proof step: **a verification step whose name promises more than it
  tests** (8 instances in one effort). Ask of each: could this print PASS while the thing it names
  is false? If the behaviour it names were absent, would it fail?
