# Batch T — test integrity: implementation plan

**Branch:** `effort-batch-t-test-integrity` off `main` (spec committed `73b8ca2`).
**Spec:** `docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md` (gate-clean after
9 Codex rounds; BINDING). **Decisions:** `scratchpad/batch-t/decisions.md` D1–D5 (binding, D4
count amended to 97). **Grounding:** `scratchpad/batch-t/fable-grounding.md`.

Nine tasks, internal order BINDING: **Task 1 (H38) → Task 2 (H28) → Task 3 (D5) → Tasks 4–8
(the D4 sweep) → Task 9 (instruments + gates)**. Every task boundary is green
(`cargo test --workspace`), and within the sweep EVERY COMMIT is green
(`cargo test -p wordcartel` minimum; spec §10.1).

## Global constraints (every task; violations are STOP-and-escalate, not judgment calls)

- **THE PREMISE: nothing in Batch T changes shipped behavior.** No production code line moves.
  The batch's only non-`#[cfg(test)]` touch is ONE doc-comment sentence (Task 1d). Any finding
  that would change a user-visible string, a status kind, or a control-flow branch is out of
  scope and gets FILED via `scripts/backlog add`, never fixed here (spec §1; A25 is the
  precedent).
- **Hand-formatted repo. `cargo fmt` is FORBIDDEN.** Match the neighbouring code's indentation,
  wrapping, and import grouping by hand. Never reflow lines you did not otherwise change.
- **GATEs** (before merge; per task where applicable): `cargo test` green across all suites;
  `cargo build` + `cargo test --no-run` warning-free for touched crates;
  `cargo clippy --workspace --all-targets` clean (workspace `clippy::all = deny`);
  `clippy::too_many_lines` (100) and `wordcartel/tests/module_budgets.rs` budgets (no touched
  file gains production lines, so these cannot move — stated for completeness); PTY smoke
  `scripts/smoke/run.sh` mandatory-run / advisory-pass at Task 9.
- **The BINDING exclusion list — these lines are never edited by the sweep** (spec §5.3):
  the 12 bare sites — `prompts.rs` H28 seeds ×2 (in `save_as_empty_path_is_a_sticky_warning` /
  `block_write_empty_path_is_a_sticky_warning`) · `file_browser.rs` ×4 (`open_file_browser`
  seed in `open_file_browser_enforces_xor`; the two `FileBrowser` literals in the footer
  tests; the `apply_listing_done` arg in the no-picker discard test) · `save.rs` ×1 (the
  Esc-drain picker seed) · `chrome_geom.rs` ×1 · `swap.rs` ×1 (the `starts_with` assertion) ·
  plus the three D5 seeds, which Task 3 migrates and the sweep must then NOT touch — AND
  `swap.rs`'s `state_dir()` `#[cfg(test)]` branch (per-process STABILITY semantics; the
  counter seam would break swap/session/recovery structurally) — AND `test_support.rs`'s
  `scratch_name` — AND both files under `wordcartel/tests/` (separate crate).
- **No textual scanner.** Nothing in this plan commits a test/gate that greps for
  `temp_dir()`. All instruments are one-shot review scripts under `scratchpad/batch-t/`
  (spec D4 / §10.6).
- **Sweep sequencing rules** (spec §10.1/§10.3, binding): each swept file's rewrites land in
  exactly ONE sweep commit; NO non-sweep change of any kind lands between `<pre-sweep>`
  (Task 3's commit) and `<tip>` (the last sweep commit) — a necessary fix to earlier work is
  REORDERED before `<pre-sweep>` (rebase, endpoints recompute, Task 9 re-measures `S_base`)
  or lands strictly AFTER `<tip>` (the tip does NOT advance).
- **Anchor by symbol name, never line number.** Locate with `grep -n`; line anchors in this
  plan are as-observed hints at `73b8ca2` only.
- For compile/usage questions on code you are editing, trust `cargo` + `grep`, never an
  editor's stale "unused"/"undefined" diagnostic.
- **Mutation protocol for every new test (spec §10.4) — MUTATE AGAINST A COMMITTED TREE.**
  Pin tests are GREEN on first run; the red step of TDD is the mutation. The baseline rule,
  exact: FIRST bring the task fully green and COMMIT it (the task's commit step), THEN apply
  the named mutation, run the named tests, WATCH them redden, restore, and prove restoration
  **scoped to the mutation's target files** — `git diff --exit-code -- <target file(s)>` AND
  `git status --porcelain -- <target file(s)>` prints nothing. (Whole-tree cleanliness is NOT
  the instrument: untracked scratchpad artifacts legitimately exist and would make it
  unpassable; "did I put the mutated line back" is a question about the target files.)
  Record the observed failing test names in your task report. If the mutation step reveals a defect in
  the pin, fix forward with a follow-up commit and re-run the mutation. Reading the test is
  not verification.
- Commit at the end of your task (Task 4–8: per file group, as specified) with the project
  trailers, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01FAx3iA5vRBiLXEfCnudR6j
  ```
  Do not push.

## Command-surface contract

Per spec §7: **N/A — argued, not asserted.** No command, option, palette row, menu entry,
keybinding, or hint is added, removed, renamed, or re-worded; the registry is untouched; every
change is `#[cfg(test)]` code, test doc-comment prose, or one production doc comment. The
contract's invariant tests remain gating and their subjects are untouched. No task in this
plan may register, rename, or re-categorize a command; if an implementer believes their task
needs to, STOP and escalate.

## Spec-instrument → plan-task ownership

| Spec item | Owner |
|---|---|
| T1 direct pin, T2 engine pin, T3 agreement matrix, §3.4 comment (D1) | Task 1 |
| §4.2 doc rewrites, T4 Route-B pin (D2) | Task 2 |
| D5 three-seed migration + comment rewrite | Task 3 |
| D4 sweep, 97 sites / 25 files / 12 commits | Tasks 4–8 |
| §10.1 inventory+green, §10.2 oracle+survivor comparison+ledger, §10.5 region check, §10.3 read set (triggers + sample), PTY smoke, mutation-evidence bundle | Task 9 |

Per-file sweepable-join counts `E(f)` (re-derived from the tree for this plan; they match spec
§1.1 exactly — Task 9's ledger uses these):

| file | E | file | E | file | E |
|---|---|---|---|---|---|
| prompts.rs | 13 | session_restore.rs | 4 | export.rs | 2 |
| file_browser.rs | 13 | mouse.rs | 4 | diagnostics_run.rs | 2 |
| app.rs | 11 | swap.rs | 3 | state.rs, search_ui.rs | 1 each |
| jobs_apply.rs | 9 | editor.rs | 3 | save.rs, recents.rs | 1 each |
| render_overlays.rs | 7 | timers.rs, render.rs | 2 each | fsx.rs, e2e.rs | 1 each |
| workspace.rs | 5 | recovery.rs | 2 | config.rs, clipboard.rs | 1 each |
| file_browser_commit.rs | 5 | file_browser_listing.rs | 2 | (Σ = 97 over 25 files) | |

---

## Task 1 — H38: pin the spell branch three ways (D1)

**Files:** `wordcartel/src/lsp_client.rs`, `wordcartel/src/ltex_ls.rs`,
`wordcartel/src/harper_ls.rs`. No production code changes; one production doc-comment
sentence.

### 1a. T1 — the direct unit pin (`lsp_client.rs`, inside `#[cfg(test)] mod tests`)

The test mod opens with `use super::*;`, and `lsp_client.rs` has
`use serde_json::{json, Value};` at module top — `classify_spell_heuristic`, `json!`, and
`DiagnosticKind` are all in scope. Add, near the mod's other free-function tests:

```rust
    /// Batch T / H38 (D1-1): the code-substring branch of the SHARED heuristic, pinned
    /// directly. `FR_SPELLING_RULE` is a real LanguageTool 6.8 rule id (jar-verified —
    /// `MorfologikFrenchSpellerRule`'s own id); its message deliberately lacks "spell" so
    /// nothing rescues a deleted code branch. KILL: delete the
    /// `if code.to_lowercase().contains("spell")` early return in
    /// `classify_spell_heuristic` — the first assertion reads Grammar and reddens.
    #[test]
    fn classify_spell_heuristic_code_branch_pins_spelling() {
        assert_eq!(classify_spell_heuristic(&json!({"code":"FR_SPELLING_RULE","message":"x"})),
            DiagnosticKind::Spelling, "the code branch alone must decide this");
        assert_eq!(classify_spell_heuristic(&json!({"code":7,"message":"x"})),
            DiagnosticKind::Grammar, "a non-string code stringifies and falls through");
        assert_eq!(classify_spell_heuristic(&json!({"source":"cspell","message":"x"})),
            DiagnosticKind::Spelling, "the source half of the message path");
        assert_eq!(classify_spell_heuristic(&json!({"message":"Possible spelling mistake"})),
            DiagnosticKind::Spelling, "the message half");
        assert_eq!(classify_spell_heuristic(&json!({"message":"style"})),
            DiagnosticKind::Grammar, "the fall-through");
    }
```

Run: `cargo test -p wordcartel classify_spell_heuristic_code_branch` → GREEN (it pins existing
behavior; the mutation below is the red step).

### 1b. T2 — the engine-level pin (`ltex_ls.rs`)

Extend the EXISTING `classify_maps_languagetool_speller_rules_to_spelling` (test mod, anchor:
that fn name). After the current fourth assertion (`{"message":"Possible spelling mistake"}`),
add:

```rust
        // Batch T / H38 (D1-2): jar-verified LT 6.8 rule ids that contain SPELL yet miss all
        // three engine short-circuits (MORFOLOGIK/HUNSPELL/SPELLER) — they reach the SHARED
        // heuristic's code branch. Messages lack "spell" so the code branch alone decides.
        // (Ids are jar-grounded; their arrival as LSP `code` strings is inferred from the
        // protocol handling and probe history — spec §1's evidence boundary.)
        assert_eq!(LtexEngine::classify(&json!({"code":"FR_SPELLING_RULE","message":"x"})),
            DiagnosticKind::Spelling,
            "the French speller's id: misses the short-circuits, exercises the shared code branch");
        assert_eq!(LtexEngine::classify(&json!({"code":"EN_CONTRACTION_SPELLING","message":"x"})),
            DiagnosticKind::Spelling,
            "an English id — the code branch matters for en-US configs too");
```

Run: `cargo test -p wordcartel classify_maps_languagetool` → GREEN.

### 1c. T3 — the agreement matrix (`harper_ls.rs`, inside `#[cfg(test)] mod tests`)

The test mod opens with `use super::*;` (so `classify_lsp` — private to this file — and
`DiagnosticKind` are in scope; `json!` via the module-top `use serde_json::{json, Value};`).
Add beside `classify_lsp_spelling_vs_grammar`:

```rust
    /// Batch T / H38 (D1-3): the shared `lsp_client::classify_spell_heuristic` and harper's
    /// private duplicate `classify_lsp` are documented as "intentionally identical" — this
    /// matrix makes divergence a failure in either direction, retiring the untested claim.
    /// One fixture per path: code-with-spell / non-string code / source / message / neither.
    /// KILL (either direction): edit EITHER body's code branch alone — the matrix reads
    /// unequal on the code-with-spell fixture and reddens.
    #[test]
    fn classify_lsp_agrees_with_the_shared_heuristic() {
        let fixtures = [
            json!({"code":"FR_SPELLING_RULE","message":"x"}),
            json!({"code":7,"message":"x"}),
            json!({"source":"cspell","message":"x"}),
            json!({"message":"Possible spelling mistake"}),
            json!({"message":"style"}),
        ];
        for d in &fixtures {
            assert_eq!(classify_lsp(d), crate::lsp_client::classify_spell_heuristic(d),
                "the two bodies must agree — diverged on {d}");
        }
    }
```

Run: `cargo test -p wordcartel classify_lsp_agrees` → GREEN.

### 1d. The one production doc-comment touch (`lsp_client.rs`)

In `classify_spell_heuristic`'s doc comment, the sentence ending
`the two bodies are intentionally identical.)` becomes:

```rust
/// the T1 pin; the two bodies are intentionally identical — pinned by
/// `harper_ls::tests::classify_lsp_agrees_with_the_shared_heuristic`.)
```

Prose only; zero codegen. This is the batch's SOLE non-test-region touch (spec §10.5) —
declare it in your task report.

### 1e. Green + commit FIRST (the mutation baseline)

`cargo test --workspace` green; `cargo build` + `cargo test --no-run` warning-free;
`cargo clippy --workspace --all-targets` clean. Commit:
`test: [lsp] pin classify_spell_heuristic's code branch three ways (Batch T / H38, D1)`.

### 1f. Mutation verification (post-commit, per the Global-Constraints baseline rule)

1. Delete the line `if code.to_lowercase().contains("spell") { return DiagnosticKind::Spelling; }`
   from `classify_spell_heuristic` (`lsp_client.rs`). Run
   `cargo test -p wordcartel classify` → MUST redden ALL of:
   `classify_spell_heuristic_code_branch_pins_spelling` (T1),
   `classify_maps_languagetool_speller_rules_to_spelling` (T2 — both new assertions),
   `classify_lsp_agrees_with_the_shared_heuristic` (T3, divergence). Restore; prove it,
   scoped to the target:
   `git diff --exit-code -- wordcartel/src/lsp_client.rs` and
   `git status --porcelain -- wordcartel/src/lsp_client.rs` prints nothing.
2. Negative control (proves T3 constrains BOTH bodies): delete the same line from
   `classify_lsp` (`harper_ls.rs`) instead. Run the same filter → MUST redden T3 AND the
   existing `classify_lsp_spelling_vs_grammar`, while T1 and T2 stay GREEN. Restore; prove:
   `git diff --exit-code -- wordcartel/src/harper_ls.rs` and
   `git status --porcelain -- wordcartel/src/harper_ls.rs` prints nothing. Record both
   observed redden-sets in the task report.

---

## Task 2 — H28: re-document the two picker tests, pin Route B (D2)

**Files:** `wordcartel/src/prompts.rs` (doc comments ONLY — no test body, assertion, or seed
changes), `wordcartel/src/file_browser_commit.rs` (one new test). Production code untouched —
D2 is explicit: delete NOTHING (the `Nothing` arm also aborts a quit drain).

### 2a. T4 first — the Route-B pin (`file_browser_commit.rs`)

The test mod already has `use crate::fsx::EntryKind;`, `use super::*;` (for
`classify_destination_enter`, `CommitOutcome`, `FileEntry`), and the helpers
`fe(name, kind)` and `tmp(label)` (which already uses `scratch_dir`). `EntryKind` is
`Clone, Copy, Debug, PartialEq, Eq`; `CommitOutcome` is `Debug, PartialEq, Eq`. Add beside the
existing `an_empty_field_with_no_highlight_commits_nothing`:

```rust
    /// Batch T / H28 Route B (D2-2): an empty field with a highlighted entry that is NEITHER
    /// a directory NOR a regular file refuses — `Other`/`Unknown` are not commit targets (we
    /// do not know they are writable regular files), and both occur in production listings
    /// (fifos/devices; stat failures and broken symlinks). The `None`-highlight leg is
    /// pinned by the neighbour above; THIS is the kind-based leg, previously pinned nowhere.
    /// `navigated` is looped to pin that Row 2 does not consult it when the field is empty.
    /// KILL: widen Row 2's guard from `Some(e) if matches!(e.kind, EntryKind::File)` to
    /// `Some(e)` — the Other/Unknown legs read `Commit` and THIS test reddens while the
    /// `None`-highlight neighbour stays green (the discrimination this coverage adds).
    #[test]
    fn an_empty_field_with_a_non_writable_highlight_commits_nothing() {
        let d = tmp("nothing-kind");
        for navigated in [false, true] {
            for kind in [EntryKind::Dir, EntryKind::File, EntryKind::Other, EntryKind::Unknown] {
                let e = fe("entry", kind);
                let got = classify_destination_enter(&crate::fsx::RealFs, &d, "", Some(&e), navigated);
                // EXHAUSTIVE over EntryKind, no wildcard: a fifth variant fails to COMPILE
                // here, forcing its Enter fate to be decided rather than silently absorbed
                // by the production `_ => Nothing`. (The iteration array above is not
                // compiler-checked — pair any new match arm with a new array entry.)
                match kind {
                    EntryKind::Dir => assert!(matches!(got, CommitOutcome::Descend(_)),
                        "empty field on a highlighted dir descends (Row 1), navigated={navigated}"),
                    EntryKind::File => assert!(matches!(got,
                        CommitOutcome::Commit { from_highlight: true, .. }),
                        "empty field on a highlighted file commits (Row 2), navigated={navigated}"),
                    EntryKind::Other | EntryKind::Unknown => assert_eq!(got,
                        CommitOutcome::Nothing,
                        "{kind:?} is not a writable regular file — never a commit target \
                         (navigated={navigated})"),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }
```

Run: `cargo test -p wordcartel an_empty_field` → both tests GREEN.

### 2b. Doc-comment rewrite, Save-As test (`prompts.rs`)

In the doc comment of `save_as_empty_path_is_a_sticky_warning`, KEEP the opening paragraph
(the `A17 T5 …` / `commit_destination turns into the SAME message/kind/lifetime …` lines) and
REPLACE the entire paragraph beginning `/// DELIBERATELY does NOT pump the async listing`
through `…is a design question, not a mechanical one.` with:

```rust
    /// DELIBERATELY does NOT pump the async listing — the non-pump is CORRECT and
    /// load-bearing, not an oversight (Batch T re-grounding, 2026-07-27). This test pins the
    /// PRE-LISTING WINDOW, a real production state by design: `open_save_as` seeds an EMPTY
    /// field, `open_destination_picker` starts with `entries` empty and lists off-thread
    /// (there is no synchronous listing path), and the Enter intercept has no
    /// pending-listing guard — a writer pressing Enter before the listing lands (slow disk,
    /// network fs) reaches EXACTLY this state: no highlight, empty field,
    /// `CommitOutcome::Nothing`, this Sticky Warning. Pumping would move the fixture into a
    /// DIFFERENT, already-covered state — with a landed listing the untouched `".."` row is
    /// highlighted and a bare Enter descends (Row 1), a path the pumped destination tests
    /// own — deleting this assertion while appearing to fix it. The kind-based companion
    /// state (a landed listing, a navigated `Other`/`Unknown` highlight, same `Nothing`) is
    /// pinned in `file_browser_commit.rs`; its refusal-WORDING defect is filed as A25. (A
    /// pump experiment was once tried here and reverted; the reachability grounding above is
    /// what settles it.)
```

### 2c. Doc-comment rewrite, Write-Block twin

Replace `block_write_empty_path_is_a_sticky_warning`'s doc comment (the `A17 T5 …
INCLUDING the same deliberate non-pump: confirmed to break identically if pumped (same
finding).` text) with:

```rust
    /// A17 T5: an empty Write-Block path refusal is a Sticky Warning. Migrated (Task 21)
    /// from the retired `block_write_submit`. See the Save-As twin above — the SAME
    /// deliberate, load-bearing non-pump: this pins the pre-listing window for the
    /// Write-Block purpose (production `block_write` also seeds an empty field), and
    /// pumping would likewise land the listing and descend instead of refusing.
```

Both rewrites must satisfy spec §4.2's four-point checklist — re-read it before committing.
The seeds (`std::env::temp_dir(), "   ".into()`) are UNTOUCHED (D5 rejected migrating them;
the comments above reason about the shared dir on purpose).

### 2d. Green + commit FIRST (the mutation baseline)

Full gate battery as Task 1e. Commit:
`test: [picker] H28 — re-document the pre-listing-window pins, add the Route-B kind pin (Batch T, D2)`.

### 2e. Mutation verification (post-commit)

In `classify_destination_enter`'s Row 2 (`file_browser_commit.rs`), change
`Some(e) if matches!(e.kind, EntryKind::File) => {` to `Some(e) => {`. Run
`cargo test -p wordcartel an_empty_field` →
`an_empty_field_with_a_non_writable_highlight_commits_nothing` MUST redden (Other/Unknown
legs read `Commit`) and `an_empty_field_with_no_highlight_commits_nothing` MUST stay green.
Restore; prove it, scoped to the target:
`git diff --exit-code -- wordcartel/src/file_browser_commit.rs` and
`git status --porcelain -- wordcartel/src/file_browser_commit.rs` prints nothing. Record
both observations in the task report.

---

## Task 3 — D5: hermetic seeds for the three pumped picker tests (ONE semantic commit)

**File:** `wordcartel/src/prompts.rs`. This commit is SEMANTIC by declaration and is the
`<pre-sweep>` endpoint — record its SHA in `scratchpad/batch-t/endpoints.txt` as `PRE=<sha>`.

Three tests, three seed replacements (locate by test name, not line):

1. `save_as_existing_target_raises_overwrite_prompt` — the call
   ```rust
        e.open_destination_picker(&fs, &tx, crate::file_browser::DestinationPurpose::SaveAs,
            std::env::temp_dir(), p.to_str().unwrap().to_string());
   ```
   becomes
   ```rust
        e.open_destination_picker(&fs, &tx, crate::file_browser::DestinationPurpose::SaveAs,
            crate::test_support::scratch_dir("saveas-ow-seed"), p.to_str().unwrap().to_string());
   ```
   AND its pump comment (the four lines beginning `// Pump the async listing to completion`)
   becomes:
   ```rust
        // Pump the async listing to completion — the state real usage actually reaches. The
        // seed dir is a fresh scratch_dir, so the listing is HERMETIC (a ".." row and
        // nothing else); and the typed field is a non-empty ABSOLUTE path, so
        // `FileBrowser::highlight_is_navigated()` gates Row 1 off regardless of highlight.
   ```
2. `block_write_failure_is_a_sticky_error_that_survives_a_later_info` — same seed
   substitution with label `"blkw-fail-seed"`; its one-line pump comment is unchanged.
3. `block_write_existing_target_raises_overwrite` — label `"blkw-ow-seed"`; pump comment
   unchanged.

The tests' TARGET paths (`p` / `parent` / `target`, still `temp_dir().join(…)`) are
Template-F join sites and belong to Task 4's sweep, NOT this commit — the absolute field
passes through `resolve_field` regardless of the seed dir, so seed and target are independent.

Verification: `cargo test -p wordcartel save_as_existing block_write_` green; assertions
byte-identical (`git diff` shows ONLY the three seed arguments + one comment). Full gate
battery. Commit (subject marks it semantic):
`test: [prompts] hermetic listing seeds for the three pumped picker tests (Batch T D5 — semantic, not sweep)`.

---

## Tasks 4–8 — the D4 sweep: 97 sites, 25 files, 12 commits

### The templates (spec §5.2 — memorize before starting)

- **Template F (file path):**
  `std::env::temp_dir().join(format!("wc-<stem>-{}.<ext>", std::process::id()))`
  → `crate::test_support::scratch_path("<stem>.<ext>")`.
  Label rule: drop the `wc-`/`wcartel-` prefix and the pid interpolation (the seam supplies
  both); KEEP the discriminating stem and the extension. Extra discriminators ride in the
  label via `&format!(…)`.
  Worked example (`workspace.rs`, `open_as_new_buffer` test):
  ```rust
  // BEFORE
  let tmp = std::env::temp_dir().join(format!("wc-open-{}.md", std::process::id()));
  // AFTER
  let tmp = crate::test_support::scratch_path("open.md");
  ```
  Dynamic-label example (`prompts.rs`, the wrapped H5 site):
  ```rust
  // BEFORE
  let doc = std::env::temp_dir()
      .join(format!("wc-h5-inv-{}-{}-{}.txt", std::process::id(), tag, TestClock(0).0));
  // AFTER
  let doc = crate::test_support::scratch_path(&format!("h5-inv-{}-{}.txt", tag, TestClock(0).0));
  ```
- **Template D (directory):** the construction PLUS any adjacent LEADING provisioning for the
  SAME path (`let _ = remove_dir_all(&d);` and/or `create_dir_all(&d)…`) collapse to ONE
  `scratch_dir` call. TRAILING cleanups (`let _ = std::fs::remove_dir_all(&d);` at test end)
  are PRESERVED. Worked example (`recovery.rs`):
  ```rust
  // BEFORE
  let dir = std::env::temp_dir().join(format!("wcartel-collisiontest-{}", std::process::id()));
  let _ = std::fs::remove_dir_all(&dir);
  std::fs::create_dir_all(&dir).unwrap();
  // AFTER
  let dir = crate::test_support::scratch_dir("recovery-collisiontest");
  ```

### THE 97-SITE MAPPING (the complete code — every row grounded in the source at `73b8ca2`)

Notation: line anchors are hints — locate by the label string in the old `format!`. **T** = F
(`scratch_path`) / D (`scratch_dir`). "− dance" = DELETE the named adjacent provisioning
line(s) for THAT path; everything else on those lines stays. **⚠ rows are read
UNCONDITIONALLY at Task 9f** (pre-declared deviations, each with its reason inline). Every
replacement is the full new statement, `crate::test_support::` prefix included.

**`prompts.rs` (13):**
| anchor | T | replacement |
|---|---|---|
| :607 `wc-recover-orphan` | F | `let p = crate::test_support::scratch_path("recover-orphan.swp");` |
| :708 `wc-ow` | F | `let p = crate::test_support::scratch_path("ow.md");` |
| :784 `wc-blkw-fail` | F | `let parent = crate::test_support::scratch_path("blkw-fail.md");` (a FILE by design — the ENOTDIR fixture) |
| :809 `wc-blkw-ow` | F | `let p = crate::test_support::scratch_path("blkw-ow.md");` |
| :1148 `wc-close-save` | F | `let p = crate::test_support::scratch_path("close-save.md");` |
| :1172 `wc-close-discard` | F | `let p = crate::test_support::scratch_path("close-discard.md");` |
| :1226 `recovered-…snap-a` | ⚠D | `let a = crate::test_support::scratch_dir("h5-snap-a").join("recovered-a.md");` — **the file NAME must start `recovered-` and end `.md`**: production `swap.rs` (`recovery_file_decision`, anchor `fname.starts_with("recovered-")`) re-verifies snapshot entries by name at confirm time; a `scratch_path` basename (`wc-scratch-…`) fails that re-verify and the deletes never happen. One seam call per site keeps the ledger exact. |
| :1227 `…snap-b` | ⚠D | `let b = crate::test_support::scratch_dir("h5-snap-b").join("recovered-b.md");` |
| :1228 `…snap-late` | ⚠D | `let latecomer = crate::test_support::scratch_dir("h5-snap-late").join("recovered-late.md");` |
| :1263-64 wrapped, dynamic | F | `let doc = crate::test_support::scratch_path(&format!("h5-inv-{}-{}.txt", tag, TestClock(0).0));` (ONE line replaces both; `tag` + clock discriminators KEPT) |
| :1313 `wc-h5-cancel` | F | `let a = crate::test_support::scratch_path("h5-cancel.swp");` |
| :1331 `wc-h5-esc` | F | `let a = crate::test_support::scratch_path("h5-esc.swp");` |
| :1383-84 wrapped, dynamic | F | `let doc = crate::test_support::scratch_path(&format!("kept-{}.txt", tag));` (ONE line; `tag` KEPT) |

**`file_browser.rs` (13):**
| anchor | T | replacement |
|---|---|---|
| :639 `wc-fb` | D | `let dir = crate::test_support::scratch_dir("fb");` — KEEP `create_dir_all(dir.join("sub"))` (it provisions the SUBDIR) |
| :670 `wc-fb-unreadable` | D | `let parent = crate::test_support::scratch_dir("fb-unreadable");` — KEEP `create_dir_all(&secret)` |
| :745 `wc-fb-symdir` | D | `let dir = crate::test_support::scratch_dir("fb-symdir");` − dance `let _ = …remove_dir_all(&dir);`; KEEP `create_dir_all(dir.join("real_sub"))` |
| :791 `wc-fb-cache` | D | `let dir = crate::test_support::scratch_dir("fb-cache");` − dance (remove + create of `&dir`) |
| :832 `wc-aba-a` | D | `let dir_a = crate::test_support::scratch_dir("aba-a");` |
| :833 `wc-aba-b` | D | `let dir_b = crate::test_support::scratch_dir("aba-b");` − dance: DELETE the whole `for d in [&dir_a, &dir_b] { … }` provisioning loop line |
| :880 `wc-faildescend` | D | `let dir = crate::test_support::scratch_dir("faildescend");` − dance (both lines) |
| :927 `wc-footer` | D | `let d = crate::test_support::scratch_dir("footer");` − dance (both) |
| :985 `wc-footer-descend` | D | `let d = crate::test_support::scratch_dir("footer-descend");` − dance (remove only); KEEP `create_dir_all(d.join("chapter-one"))` |
| :1010 `wc-footer-redirect` | D | `let d = crate::test_support::scratch_dir("footer-redirect");` − dance (both) |
| :1035 `wc-footer-refused` | D | `let d = crate::test_support::scratch_dir("footer-refused");` − dance (both) |
| :1060 `wc-footer-broken` | D | `let d = crate::test_support::scratch_dir("footer-broken");` − dance (both) |
| :1137 `wc-fbfault` | D | `let dir = crate::test_support::scratch_dir("fbfault");` − dance (both) |

**`app.rs` (11):**
| anchor | T | replacement |
|---|---|---|
| :2225 | F | `let p = crate::test_support::scratch_path("savequit.md");` |
| :2262 | F | `let target = crate::test_support::scratch_path("clobber-open.md");` |
| :2264 | F | `let named = crate::test_support::scratch_path("clobber-named.md");` |
| :2282 | F | `let named = crate::test_support::scratch_path("clobber-new.md");` |
| :2301 | F | `let target = crate::test_support::scratch_path("clean-open.md");` |
| :2303 | F | `let named = crate::test_support::scratch_path("clean-named.md");` |
| :2343 | F | `let named = crate::test_support::scratch_path("clean-new.md");` |
| :4253 | D | `let dir = crate::test_support::scratch_dir("a6-fb-nav");` − dance (`create_dir_all(&dir)`) |
| :4335 | D | `let parent = crate::test_support::scratch_dir("a6-descend");` − dance (create) |
| :4551 | F | `let p = crate::test_support::scratch_path("c4t2-quit.md");` |
| :4606 | F | `let p = crate::test_support::scratch_path("c4t2-esc.md");` |

**`jobs_apply.rs` (9)** — all F, no dances:
| anchor | replacement |
|---|---|
| :440 | `let p = crate::test_support::scratch_path("savequit-cmd.md");` |
| :463 | `let p = crate::test_support::scratch_path("pas.md");` |
| :487 | `let p = crate::test_support::scratch_path("sqflight.md");` |
| :966 | `let parent = crate::test_support::scratch_path("c4-exportwrite.md");` (a FILE by design) |
| :984 | `let target = crate::test_support::scratch_path("c4-exportbytes-fault.html");` |
| :1000 | `let missing_tmp = crate::test_support::scratch_path("c4-exportrename-missing.tmp");` |
| :1001 | `let target = crate::test_support::scratch_path("c4-exportrename-target.html");` |
| :1012 | `let target = crate::test_support::scratch_path("c4-exportpandoc.html");` |
| :1025 | `let target = crate::test_support::scratch_path("c4-exporttoctou.html");` |

**`render_overlays.rs` (7):**
| anchor | T | replacement |
|---|---|---|
| :1094 | D | `let d = crate::test_support::scratch_dir("render-field");` − dance (both) |
| :1161 | F | `let dir = crate::test_support::scratch_path("render-title");` — NOTE: the original is an UNCREATED dir-valued path (no provisioning exists); `scratch_path` preserves never-created semantics, so F is correct despite the dir-shaped name. Same note for :1341/:1388/:1430/:1615. |
| :1296 | D | `let d = crate::test_support::scratch_dir("render-c2");` − dance (both) |
| :1341 | F | `let dir = crate::test_support::scratch_path("render-mr2");` |
| :1388 | F | `let dir = crate::test_support::scratch_path("render-m5");` |
| :1430 | F | `let dir = crate::test_support::scratch_path("render-empty");` |
| :1615 | F | `let dir = crate::test_support::scratch_path("render-rect");` |

**`workspace.rs` (5):** :429 F `let tmp = crate::test_support::scratch_path("open.md");` ·
:441 F `…scratch_path("oic-mru.md");` · :464 D `let dir =
crate::test_support::scratch_dir("open-isdir");` − dance (create) · :478 F
`…scratch_path("open2.md");` · :532 F `…scratch_path("c.md");`

**`file_browser_commit.rs` (5)** — all D; each deletes its ONE-LINE dance
(`let _ = …remove_dir_all(&d); …create_dir_all(&d).expect("dir");`):
:1085 `let d = crate::test_support::scratch_dir("saveas-e2e");` · :1385
`…scratch_dir("saveas-cancel");` · :1659 `…scratch_dir("wb-e2e");` · :2059
`…scratch_dir("exp-e2e");` · :2116 `…scratch_dir("exp-seam-e2e");`

**`session_restore.rs` (4):** :338 F `let p = crate::test_support::scratch_path("oic.md");` ·
:354 D `let dir = crate::test_support::scratch_dir("oic-isdir");` − dance (create) · :368 D
`…scratch_dir("fbopen");` − dance (create) · :538 F `…scratch_path("idstamp.md");`

**`mouse.rs` (4):**
| anchor | T | replacement |
|---|---|---|
| :1634 | D | `let dir = crate::test_support::scratch_dir("a6-fbwheel");` − dance (create) |
| :2299 | D | `let dir = crate::test_support::scratch_dir("t10-fbclick");` − dance (create of `&dir`); KEEP the `sub` provisioning below |
| :2350 | D | `let d = crate::test_support::scratch_dir("t24-recents-open");` − dance (remove + create) |
| :2387 | ⚠F | `let d = crate::test_support::scratch_path("t24-recents-refuse");` − dance (`let _ = …remove_dir_all(&d);`) — **the dir must NOT exist** (the test needs `gone.md`'s stat to fail under a never-created parent); `scratch_path` is never created, preserving exactly that; `scratch_dir` would create it. |

**`swap.rs` (3)** — all F; **KEEP** :823's `let _ = …remove_file(swap_path(Some(&p)).unwrap());`
(it removes a DERIVED path, not this one — not a dance on the scratch path):
:822 `let p = crate::test_support::scratch_path("norec.md");` · :829
`…scratch_path("eq.md");` · :841 `…scratch_path("div.md");`
(**`state_dir()`'s `#[cfg(test)]` branch and the `:682` `starts_with` assertion are OFF LIMITS.**)

**`editor.rs` (3):** :1736 F `let p = crate::test_support::scratch_path("fromfile.md");` ·
:1750 ⚠F `let p = crate::test_support::scratch_path("missing.md");` − dance
(`let _ = …remove_file(&p);`) — never-created is the tested property; the remove was the old
way to guarantee it, the seam guarantees it by construction · :1761 F
`…scratch_path("bin.bin");`

**`timers.rs` (2):** :269 F `let p = crate::test_support::scratch_path("c4t2-timeout.md");` ·
:315 F `…scratch_path("c4t2-drain-timeout.md");`

**`render.rs` (2):** :3324 D `let dir = crate::test_support::scratch_dir("a6-fbrender");` −
dance (create) · :3354 D `let small_dir = crate::test_support::scratch_dir("a6-fbrender-small");`
− dance (create)

**`recovery.rs` (2)** — D, − dance (both lines each); the `wcartel-` prefix drops like `wc-`:
:147 `let dir = crate::test_support::scratch_dir("recovery-collisiontest");` · :172
`let dir = crate::test_support::scratch_dir("recovery-dumptest");`

**`file_browser_listing.rs` (2)** — D, each deletes its one-line dance: :387
`let d = crate::test_support::scratch_dir("destfilter");` · :451
`…scratch_dir("destsibling");`

**`export.rs` (2):** :403 D `let d = crate::test_support::scratch_dir("exp-seed");` − dance
(both) · :437 D `…scratch_dir("exp-nopandoc");` − dance (both)

**`diagnostics_run.rs` (2):**
| anchor | T | replacement |
|---|---|---|
| :770 | ⚠F | `let d = crate::test_support::scratch_path("dict-atomic");` − dance (`let _ = …remove_dir_all(&d);`) — **the parent must be ABSENT**: the test exercises `append_word_to_dict_with_fs` provisioning its own parent; `scratch_dir` would pre-create `d` and gut the tested property. `scratch_path` is never created. |
| :800 | D | `let d = crate::test_support::scratch_dir("dict-link");` − dance (create) |

**Singletons (8):** `state.rs:348` D `let d = crate::test_support::scratch_dir("fid-broken");`
− one-line dance · `search_ui.rs:655` F `let parent =
crate::test_support::scratch_path("adddict-fail.md");` (a FILE by design) · `save.rs:1057` D
`let d = crate::test_support::scratch_dir("fp-broken");` − dance (create) · `recents.rs:156` D
`let d = crate::test_support::scratch_dir("recents");` − dance (remove + create) ·
`fsx.rs:580` D `let dir = crate::test_support::scratch_dir("faultfs-promo");` − dance (create)
· `e2e.rs:3087` D `let d = crate::test_support::scratch_dir("c5-journey");` − dance (remove +
create) · `config.rs:1549` D `let d = crate::test_support::scratch_dir("cfg-cap");` − dance
(create) · `clipboard.rs:704` D `let dir = crate::test_support::scratch_dir("clip-test");` −
dance (`let _ = std::fs::create_dir_all(&dir);`)

⚠ census: 6 rows (`prompts.rs:1226/1227/1228`, `mouse.rs:2387`, `editor.rs:1750`,
`diagnostics_run.rs:770`) — Task 9f reads all six unconditionally, on top of the spec's
triggers and sample (a plan-level strengthening; the spec's read set is a floor).

### Per-file procedure (identical in every sweep task)

1. `grep -n 'temp_dir()' wordcartel/src/<file>` — enumerate. Every hit must be (a) a mapping
   row you apply, (b) a Global-Constraints exclusion you do NOT touch, or (c) a comment line
   you do NOT touch. If a hit fits none of these, STOP and escalate.
2. Apply THE MAPPING — it is the code. Do not re-decide F-vs-D, labels, or dance membership;
   if the source at your anchor does not match what the mapping assumes, STOP and escalate
   (the mapping was grounded at `73b8ca2`; a mismatch means the tree moved).
3. Preserve assertions, trailing cleanups, and all neighbouring formatting byte-for-byte.
4. Self-check: `grep -c 'temp_dir()' wordcartel/src/<file>` equals the file's expected
   residual (table below); `grep -n 'temp_dir()' <file>` shows ONLY exclusions/comments.
5. `cargo test -p wordcartel` GREEN, `cargo clippy -p wordcartel --all-targets` clean →
   commit THIS FILE GROUP (never split a file across commits):
   `test: [<area>] sweep temp_dir joins onto the scratch seam (Batch T D4, <n> sites)`.

Expected post-sweep `temp_dir()` residuals (ALL hits — code + comments) per swept file:
`prompts.rs` **3** (2 H28 seeds + 1 comment in `save_as_empty…`'s doc) · `file_browser.rs`
**4** (bare) · `save.rs` **1** (bare) · `swap.rs` **3** (`state_dir()` branch + `starts_with`
assertion + 1 comment) · every other swept file **0**. (Unswept `chrome_geom.rs` keeps 1,
`test_support.rs` keeps 1.)

Each sweep task below = "apply THE MAPPING's rows for your files, per the per-file
procedure." The mapping is the code; the task lists only scope, commit boundaries, and
off-limits reminders.

### Task 4 — Sweep A: the two heavies (2 commits, 26 sites)
`prompts.rs` (13 — includes the 2 wrapped sites; the 5 bare/former-D5 seed lines and both
comment mentions are OFF LIMITS) → commit 1. `file_browser.rs` (13; the 4 bare sites are OFF
LIMITS) → commit 2.

### Task 5 — Sweep B: the middleweights (3 commits, 27 sites)
`app.rs` (11) → `jobs_apply.rs` (9) → `render_overlays.rs` (7), one commit each.

### Task 6 — Sweep C: the four-to-fives (4 commits, 18 sites)
`workspace.rs` (5) → `file_browser_commit.rs` (5) → `session_restore.rs` (4) → `mouse.rs`
(4), one commit each.

### Task 7 — Sweep D: the twos-and-threes (2 commits, 18 sites)
Commit 1: `swap.rs` (3 — **`state_dir()`'s branch and the `starts_with` assertion are OFF
LIMITS**; the three sweepable sites are in `mod tests`), `editor.rs` (3), `timers.rs` (2),
`render.rs` (2). Commit 2: `recovery.rs` (2), `file_browser_listing.rs` (2 — note the
multi-statement dance line `let _ = remove_dir_all(&d); create_dir_all(&d)…` collapses under
Template D like any other provisioning), `export.rs` (2), `diagnostics_run.rs` (2).

### Task 8 — Sweep E: the singletons (1 commit, 8 sites)
`state.rs`, `search_ui.rs`, `save.rs` (its bare Esc-drain seed OFF LIMITS), `recents.rs`,
`fsx.rs`, `e2e.rs`, `config.rs`, `clipboard.rs` — one site each, ONE commit. After this
commit, record `TIP=<sha>` in `scratchpad/batch-t/endpoints.txt`.

---

## Task 9 — the instruments, executed; gates; report

Everything below runs from the repo root at `<tip>`, with `PRE`/`TIP` from
`scratchpad/batch-t/endpoints.txt`. Scripts live in `scratchpad/batch-t/verify/` — they are
review artifacts, NEVER committed to the tree (D4: no scanner). Attach every output to the
review package verbatim.

### 9a. Inventory invariance + green at both endpoints (spec §10.1)

Every script below is fail-capable: it `exit 1`s on the property it checks — a check that
cannot exit nonzero is not an instrument (the defect class this batch exists to fix). Run
each with `bash <script>; echo exit=$?` (bash, not sh — 9f uses process substitution) and
treat ANY nonzero exit as a NO-GO finding.

**THE PREAMBLE.** The blocks are STANDALONE scripts — shell variables do not survive between
`bash` invocations — so every block that uses `V`, `PRE`, or `TIP` begins with these exact
four lines (duplication is correct here; a preamble that lives only in this intro is a
preamble that gets skipped). Run every block from the MAIN repo checkout, never from inside a
`/tmp/bt-*` worktree (`git rev-parse --show-toplevel` would re-root `V` there):

```sh
set -eu -o pipefail   # PREAMBLE — identical in every block below; each block is standalone
R="$(git rev-parse --show-toplevel)"; V="$R/scratchpad/batch-t/verify"; mkdir -p "$V"
PRE=$(sed -n 's/^PRE=//p' "$R/scratchpad/batch-t/endpoints.txt")
TIP=$(sed -n 's/^TIP=//p' "$R/scratchpad/batch-t/endpoints.txt")
```

Blocks that ACCUMULATE failures (9d, 9e) switch to `set +e` immediately after the preamble,
loudly commented — under `-e` a failing `grep -q … && continue` guard would abort the loop
instead of accumulating; the block's final assertion restores the fail-capable exit.

**THE POPULATION RULE (applies to every accumulate-then-assert loop):** an empty loop reports
PASS, which is the forbidden shape wearing a loop. So every loop that feeds an assertion
first CAPTURES its population to a file, ASSERTS the population's size against a known
expectation (exact where known — 12 sweep commits, 25 ledger rows; a hard floor where not),
and only then iterates. A loop that ran zero times — or over a population the wrong size —
is a hard failure, never a pass.

```sh
set -eu -o pipefail   # PREAMBLE — identical in every block below; each block is standalone
R="$(git rev-parse --show-toplevel)"; V="$R/scratchpad/batch-t/verify"; mkdir -p "$V"
PRE=$(sed -n 's/^PRE=//p' "$R/scratchpad/batch-t/endpoints.txt")
TIP=$(sed -n 's/^TIP=//p' "$R/scratchpad/batch-t/endpoints.txt")
git worktree add /tmp/bt-pre "$PRE"; git worktree add /tmp/bt-tip "$TIP"
( cd /tmp/bt-pre && cargo test --workspace -- --list 2>/dev/null | grep ': test$' | sort \
    > "$V/inv-pre.txt" && cargo test --workspace )
( cd /tmp/bt-tip && cargo test --workspace -- --list 2>/dev/null | grep ': test$' | sort \
    > "$V/inv-tip.txt" && cargo test --workspace )
diff "$V/inv-pre.txt" "$V/inv-tip.txt"    # set -e: a nonempty diff ABORTS here
echo "SWEEP INVENTORY: identical"
```
Also per-commit greenness evidence: for each of the 12 commits in 9e's
`$V/sweep-commits.txt` (the population file — do not re-derive the list by hand), confirm
the owning sweep task's report recorded its per-commit `cargo test -p wordcartel` run.

Branch-level inventory (the exactly-three-new-names claim), executable:
```sh
set -eu -o pipefail   # PREAMBLE — identical in every block below; each block is standalone
R="$(git rev-parse --show-toplevel)"; V="$R/scratchpad/batch-t/verify"; mkdir -p "$V"
PRE=$(sed -n 's/^PRE=//p' "$R/scratchpad/batch-t/endpoints.txt")
TIP=$(sed -n 's/^TIP=//p' "$R/scratchpad/batch-t/endpoints.txt")
BASE=$(git merge-base main "$TIP")
git worktree add /tmp/bt-base "$BASE"
( cd /tmp/bt-base && cargo test --workspace -- --list 2>/dev/null | grep ': test$' | sort \
    > "$V/inv-base.txt" )
# POPULATION FIRST: both inventories must be non-empty before comm's verdict means anything.
[ -s "$V/inv-base.txt" ] || { echo "INVENTORY FAIL: empty base inventory"; exit 1; }
[ -s "$V/inv-tip.txt" ]  || { echo "INVENTORY FAIL: empty tip inventory (run 9a first)"; exit 1; }
comm -13 "$V/inv-base.txt" "$V/inv-tip.txt" > "$V/new-names.txt"
comm -23 "$V/inv-base.txt" "$V/inv-tip.txt" > "$V/lost-names.txt"
[ ! -s "$V/lost-names.txt" ] || { echo "INVENTORY FAIL: pre-existing test names lost/renamed:";
    cat "$V/lost-names.txt"; exit 1; }
[ "$(wc -l < "$V/new-names.txt")" -eq 3 ] || { echo "INVENTORY FAIL: expected exactly 3 new \
names, got:"; cat "$V/new-names.txt"; exit 1; }
for n in classify_spell_heuristic_code_branch_pins_spelling \
         classify_lsp_agrees_with_the_shared_heuristic \
         an_empty_field_with_a_non_writable_highlight_commits_nothing; do
    grep -q "$n" "$V/new-names.txt" || { echo "INVENTORY FAIL: missing $n"; exit 1; }
done
echo "BRANCH INVENTORY: exactly the three expected names"
```

### 9b. Residue oracle at `<tip>` (spec §10.2)

```sh
set -eu -o pipefail   # PREAMBLE — identical in every block below; each block is standalone
R="$(git rev-parse --show-toplevel)"; V="$R/scratchpad/batch-t/verify"; mkdir -p "$V"
PRE=$(sed -n 's/^PRE=//p' "$R/scratchpad/batch-t/endpoints.txt")
TIP=$(sed -n 's/^TIP=//p' "$R/scratchpad/batch-t/endpoints.txt")
cd /tmp/bt-tip    # AFTER the preamble — $R/$V are already absolute
grep -rn 'temp_dir()' --include='*.rs' wordcartel/src wordcartel/tests > "$V/residue-raw.txt"
# code lines only (drop //-comment lines), keep the file for the anchored set check:
awk -F: '{ f=$1; s=$0; sub(/^[^:]*:[0-9]+:[ \t]*/,"",s); if (s !~ /^\/\//) print f }' \
    "$V/residue-raw.txt" | sort | uniq -c | sed 's/^ *//' > "$V/residue-by-file.txt"
diff "$V/residue-by-file.txt" - <<'EOF'
1 wordcartel/src/chrome_geom.rs
4 wordcartel/src/file_browser.rs
2 wordcartel/src/prompts.rs
1 wordcartel/src/save.rs
2 wordcartel/src/swap.rs
1 wordcartel/src/test_support.rs
1 wordcartel/tests/harper_ls_integration.rs
1 wordcartel/tests/harper_ls_probe.rs
EOF
echo "RESIDUE ORACLE: 13 code survivors, anchored set exact"
```
(A nonempty diff aborts under `set -e` — an extra hit = incomplete sweep, a missing one =
over-reach; the per-file counts ARE the anchored survivor set: `prompts.rs` = the 2 H28
seeds; `file_browser.rs` = its 4 bare; `swap.rs` = `state_dir()`'s branch + the
`starts_with` assertion.)

### 9c. Survivor comparison — byte-for-byte (spec §10.2, strengthened)

```sh
set -eu -o pipefail   # PREAMBLE — identical in every block below; each block is standalone
R="$(git rev-parse --show-toplevel)"; V="$R/scratchpad/batch-t/verify"; mkdir -p "$V"
PRE=$(sed -n 's/^PRE=//p' "$R/scratchpad/batch-t/endpoints.txt")
TIP=$(sed -n 's/^TIP=//p' "$R/scratchpad/batch-t/endpoints.txt")
git diff -U0 "$PRE".."$TIP" > "$V/aggregate.patch"
# RAW survivor line contents at tip (leading whitespace preserved; comments included — cheap):
( cd /tmp/bt-tip && grep -rn 'temp_dir()' --include='*.rs' wordcartel/src wordcartel/tests ) \
  | sed 's/^[^:]*:[0-9]*://' > "$V/survivors-raw.txt"
grep -E '^[+-]' "$V/aggregate.patch" | grep -vE '^(\+\+\+|---)' > "$V/patch-changed-lines.txt"
# POPULATION FIRST: both loop inputs must be non-trivially sized before the loop's verdict
# means anything (13 code survivors — 9b pins the exact set — plus comment mentions; a sweep
# patch always has changed lines).
[ "$(wc -l < "$V/survivors-raw.txt")" -ge 13 ] || { echo \
  "SURVIVOR COMPARISON FAIL: only $(wc -l < "$V/survivors-raw.txt") survivor lines captured"; exit 1; }
[ -s "$V/patch-changed-lines.txt" ] || { echo "SURVIVOR COMPARISON FAIL: empty aggregate patch"; exit 1; }
fail=0
while IFS= read -r content; do
  if grep -qF -- "$content" "$V/patch-changed-lines.txt"; then
    echo "SURVIVOR TOUCHED (added or removed in the -U0 patch): $content"; fail=1
  fi
done < "$V/survivors-raw.txt"
[ "$fail" -eq 0 ] || { echo "SURVIVOR COMPARISON: FAILED"; exit 1; }
echo "SURVIVOR COMPARISON: clean"
```
("Touched" = the line's content appears as an ADDED or REMOVED line in the `-U0` patch — the
spec's pinned definition. The three D5-migrated seeds are not survivors: they contain no
`temp_dir()` at tip.)

### 9d. Delegation ledger (spec §10.2 — expected values from the `E(f)` table above)

```sh
set -eu -o pipefail   # PREAMBLE — identical in every block below; each block is standalone
R="$(git rev-parse --show-toplevel)"; V="$R/scratchpad/batch-t/verify"; mkdir -p "$V"
PRE=$(sed -n 's/^PRE=//p' "$R/scratchpad/batch-t/endpoints.txt")
TIP=$(sed -n 's/^TIP=//p' "$R/scratchpad/batch-t/endpoints.txt")
cat > "$V/ef-table.txt" <<'EOF'
13 wordcartel/src/prompts.rs
13 wordcartel/src/file_browser.rs
11 wordcartel/src/app.rs
9 wordcartel/src/jobs_apply.rs
7 wordcartel/src/render_overlays.rs
5 wordcartel/src/workspace.rs
5 wordcartel/src/file_browser_commit.rs
4 wordcartel/src/session_restore.rs
4 wordcartel/src/mouse.rs
3 wordcartel/src/swap.rs
3 wordcartel/src/editor.rs
2 wordcartel/src/timers.rs
2 wordcartel/src/render.rs
2 wordcartel/src/recovery.rs
2 wordcartel/src/file_browser_listing.rs
2 wordcartel/src/export.rs
2 wordcartel/src/diagnostics_run.rs
1 wordcartel/src/state.rs
1 wordcartel/src/search_ui.rs
1 wordcartel/src/save.rs
1 wordcartel/src/recents.rs
1 wordcartel/src/fsx.rs
1 wordcartel/src/e2e.rs
1 wordcartel/src/config.rs
1 wordcartel/src/clipboard.rs
EOF
# POPULATION FIRST (still under -e): sizes asserted before anything is trusted.
[ "$(wc -l < "$V/ef-table.txt")" -eq 25 ] || { echo "LEDGER FAIL: ef-table != 25 rows"; exit 1; }
# SELF-MAINTAINING population check — no magic count to rot (the tree has 128 tracked .rs
# files at 73b8ca2; it will drift, so the assertion is "two authoritative enumerations
# agree", not "equals a number written down in July"): the sweep interval must not add,
# remove, or rename ANY .rs file, so the PRE and TIP enumerations must be IDENTICAL — a
# truncated capture would have to truncate both endpoints identically to sneak through, and
# the 25-ledger-membership check below still pins the floor.
git ls-tree -r --name-only "$PRE" | grep '\.rs$' | sort > "$V/rs-files-pre.txt"
git ls-tree -r --name-only "$TIP" | grep '\.rs$' | sort > "$V/rs-files.txt"
diff "$V/rs-files-pre.txt" "$V/rs-files.txt" || { echo \
  "LEDGER FAIL: .rs file set differs between PRE and TIP (sweep added/removed/renamed a file)"; exit 1; }
[ "$(wc -l < "$V/rs-files.txt")" -gt 25 ] || { echo \
  "LEDGER FAIL: population ($(wc -l < "$V/rs-files.txt")) not even larger than the 25 ledger files"; exit 1; }
while read -r _ f; do
  grep -qxF "$f" "$V/rs-files.txt" || { echo "LEDGER FAIL: ledger file $f absent from tree"; exit 1; }
done < "$V/ef-table.txt"
# count_seam REV:PATH — prints the seam-call count; returns 1 iff the READ failed. A failed
# `git show` must NOT read as "zero seam calls": zero is a legitimate count, unreadable is a
# broken instrument, and the two must never produce the same value.
count_seam() {
  local src n s=0
  src=$(git show "$1" 2>/dev/null) || return 1
  # grep exit codes DISCRIMINATED, never blanket-absorbed: 1 = zero matches (a legitimate
  # count), >1 = grep itself broke (which must not read as any count at all).
  n=$(printf '%s\n' "$src" | grep -cE 'scratch_(path|dir)\(') || s=$?
  [ "$s" -le 1 ] || return 1
  printf '%s\n' "$n"
}
set +e   # ACCUMULATION SECTION — deliberately off -e (a failing `grep -q … && continue`
         # guard would abort the loop instead of accumulating); the final assertion below
         # restores the fail-capable exit.
fail=0
while read -r n f; do
  b=$(count_seam "$PRE:$f") || { echo "LEDGER FAIL: unreadable $PRE:$f"; fail=1; continue; }
  t=$(count_seam "$TIP:$f") || { echo "LEDGER FAIL: unreadable $TIP:$f"; fail=1; continue; }
  if [ $((t-b)) -ne "$n" ]; then echo "LEDGER FAIL $f: delta $((t-b)), expected $n"; fail=1
  else echo "ok   $f (+$n)"; fi
done < "$V/ef-table.txt"
# zero-delta everywhere else in the tree:
while read -r f; do
  grep -q " $f\$" "$V/ef-table.txt" && continue
  b=$(count_seam "$PRE:$f") || { echo "LEDGER FAIL: unreadable $PRE:$f"; fail=1; continue; }
  t=$(count_seam "$TIP:$f") || { echo "LEDGER FAIL: unreadable $TIP:$f"; fail=1; continue; }
  [ "$b" = "$t" ] || { echo "LEDGER FAIL: unexpected seam-call delta in $f ($b -> $t)"; fail=1; }
done < "$V/rs-files.txt"
[ "$fail" -eq 0 ] || { echo "DELEGATION LEDGER: FAILED"; exit 1; }
echo "DELEGATION LEDGER: PASS (25 files exact, all others zero-delta)"
```

### 9e. Region check, per commit (spec §10.5 — both test-region shapes)

```sh
set -eu -o pipefail   # PREAMBLE — identical in every block below; each block is standalone
R="$(git rev-parse --show-toplevel)"; V="$R/scratchpad/batch-t/verify"; mkdir -p "$V"
PRE=$(sed -n 's/^PRE=//p' "$R/scratchpad/batch-t/endpoints.txt")
TIP=$(sed -n 's/^TIP=//p' "$R/scratchpad/batch-t/endpoints.txt")
# POPULATION FIRST (still under -e): capture and size-assert before any accumulation — an
# empty or wrong-sized population is a hard failure, never a silent zero-iteration pass.
git rev-list --reverse "$PRE".."$TIP" > "$V/sweep-commits.txt"
[ "$(wc -l < "$V/sweep-commits.txt")" -eq 12 ] || { echo \
  "REGION FAIL: expected exactly 12 sweep commits, got $(wc -l < "$V/sweep-commits.txt")"; exit 1; }
set +e   # ACCUMULATION SECTION — deliberately off -e; the final assertion restores the
         # fail-capable exit.
fail=0
while read -r c; do
  git diff --name-only "$c^" "$c" > "$V/files-of-commit.txt"
  [ -s "$V/files-of-commit.txt" ] || { echo "REGION FAIL $c: empty changed-file list"; fail=1; continue; }
  while read -r f; do
    case "$f" in *.rs) ;; *) echo "REGION FAIL $c: non-Rust file $f"; fail=1; continue ;; esac
    # shape 2: whole-file test module (e2e.rs — `#![cfg(test)]` at file top):
    if git show "$c:$f" | sed -n '1,10p' | grep -q '^#!\[cfg(test)\]'; then continue; fi
    # shape 1: trailing `mod tests` (its `#[cfg(test)]` attribute sits on the preceding line):
    m=$(git show "$c:$f" | grep -n 'mod tests' | head -1 | cut -d: -f1)
    [ -n "$m" ] || { echo "REGION FAIL $c: $f matches neither test-region shape"; fail=1; continue; }
    git diff -U0 "$c^" "$c" -- "$f" \
      | grep -oE '^@@ [^+]*\+[0-9]+' | grep -oE '[0-9]+$' > "$V/hunk-starts.txt"
    [ -s "$V/hunk-starts.txt" ] || { echo "REGION FAIL $c: no hunks extracted for $f"; fail=1; continue; }
    while read -r start; do
      [ "$start" -ge "$m" ] || { echo "REGION FAIL $c: $f hunk at +$start precedes mod tests ($m)"; fail=1; }
    done < "$V/hunk-starts.txt"
  done < "$V/files-of-commit.txt"
done < "$V/sweep-commits.txt"
[ "$fail" -eq 0 ] || { echo "REGION CHECK: FAILED — the offending commit is REJECTED"; exit 1; }
echo "REGION CHECK: PASS (12 commits, every sweep hunk in a test region, both shapes handled)"
```
(`e2e.rs` passes via its file-top `#![cfg(test)]`; every other swept file has a trailing
`mod tests`.)

### 9f. Read set — triggers, then the seeded sample (spec §10.3)

```sh
set -eu -o pipefail   # PREAMBLE — identical in every block below; each block is standalone
R="$(git rev-parse --show-toplevel)"; V="$R/scratchpad/batch-t/verify"; mkdir -p "$V"
PRE=$(sed -n 's/^PRE=//p' "$R/scratchpad/batch-t/endpoints.txt")
TIP=$(sed -n 's/^TIP=//p' "$R/scratchpad/batch-t/endpoints.txt")
# Hunk table: id added removed has_format has_scratch_path has_scratch_dir
awk '
  function flush() { if (id) printf "%s %d %d %d %d %d\n", id, add, rem, fmt, sp, sd }
  /^diff --git/ { flush(); file=$4; sub(/^b\//,"",file); id="" }
  /^@@/ { flush(); n=$0; sub(/^@@ [^+]*\+/,"",n); sub(/[ ,].*$/,"",n);
          id=file ":" n; add=0; rem=0; fmt=0; sp=0; sd=0 }
  /^\+/ && !/^\+\+\+/ { add++; if ($0 ~ /format!\(/) fmt=1;
                        if ($0 ~ /scratch_path\(/) sp=1; if ($0 ~ /scratch_dir\(/) sd=1 }
  /^-/  && !/^---/   { rem++ }
  END { flush() }' "$V/aggregate.patch" > "$V/hunks.txt"
# POPULATION FIRST: adjacent-site merging can shrink 97 sites below 97 hunks, but never
# below one hunk per swept file — and the draw needs 18 candidates to mean anything.
[ "$(wc -l < "$V/hunks.txt")" -ge 25 ] || { echo \
  "SAMPLE FAIL: only $(wc -l < "$V/hunks.txt") hunks parsed — patch or parser broken"; exit 1; }
# Item-1 triggers — MANDATORY reads:
awk '$2==0 || $3==0 || $3>4 || $2>3' "$V/hunks.txt" > "$V/mandatory.txt"
# Candidates + the keyed-digest ranking (spec §10.3-2, exactly — no PRNG, sha256 IS the draw):
awk '!($2==0 || $3==0 || $3>4 || $2>3)' "$V/hunks.txt" > "$V/candidates.txt"
[ "$(wc -l < "$V/candidates.txt")" -ge 18 ] || { echo \
  "SAMPLE FAIL: only $(wc -l < "$V/candidates.txt") candidates — cannot draw 18"; exit 1; }
: > "$V/ranked.txt"
while read -r id a r fmt sp sd; do
  h=$(printf '%s:%s' "$TIP" "$id" | sha256sum | cut -d' ' -f1)
  echo "$h $id $fmt $sp $sd" >> "$V/ranked.txt"
done < "$V/candidates.txt"
sort -o "$V/ranked.txt" "$V/ranked.txt"
# (1) unconditional inclusions: every dynamic-label (format!) candidate:
awk '$3==1 {print $2}' "$V/ranked.txt" > "$V/sample.txt"
# (2) the 18 lowest-ranked non-format! candidates. Via a FILE, not `awk | head` — under
# pipefail, head's early close SIGPIPEs awk and a correct pipeline reads as failed:
awk '$3!=1 {print $2}' "$V/ranked.txt" > "$V/nonfmt-ranked.txt"
head -18 "$V/nonfmt-ranked.txt" >> "$V/sample.txt"
# (3a) heavy-file floors, E(f)>=3, PINNED path-byte order:
for hf in wordcartel/src/app.rs wordcartel/src/editor.rs wordcartel/src/file_browser.rs \
          wordcartel/src/file_browser_commit.rs wordcartel/src/jobs_apply.rs \
          wordcartel/src/mouse.rs wordcartel/src/prompts.rs wordcartel/src/render_overlays.rs \
          wordcartel/src/session_restore.rs wordcartel/src/swap.rs wordcartel/src/workspace.rs; do
  # if-form, NOT `grep -q … && continue` — under `set -e` a failing bare AND-list aborts
  # the loop instead of proceeding to the top-up:
  if ! grep -q "^$hf:" "$V/sample.txt"; then
    cand=$(awk -v p="$hf:" 'index($2, p)==1 {print $2; exit}' "$V/ranked.txt")
    [ -n "$cand" ] || { echo "SAMPLE FAIL: no candidate hunk in heavy file $hf"; exit 1; }
    echo "$cand" >> "$V/sample.txt"
  fi
done
# (3b) seam-kind floors: >=3 hunks adding scratch_path, >=3 adding scratch_dir:
for kind in 4:scratch_path 5:scratch_dir; do
  col=${kind%%:*}; name=${kind##*:}
  # exit codes DISCRIMINATED (grep: 0/1 = count found/zero, >1 = real error — a failed awk
  # or unreadable file must never read as "zero"); no process substitution, so awk's own
  # failure aborts under -e instead of vanishing inside <(…):
  awk -v c="$col" '$c==1 {print $2}' "$V/ranked.txt" > "$V/kind-ranked.txt"
  s=0; have=$(grep -cFf "$V/sample.txt" "$V/kind-ranked.txt") || s=$?
  [ "$s" -le 1 ] || { echo "SAMPLE FAIL: grep error ($s) counting the $name floor"; exit 1; }
  while [ "$have" -lt 3 ]; do
    # via files, not `… | head -1` (the pipefail/SIGPIPE hazard of (2)); grep -v may
    # legitimately match nothing (exit 1: every qualifier already sampled) but a >1 exit is
    # grep itself breaking — discriminated, not blanket-absorbed:
    s=0; grep -vFf "$V/sample.txt" "$V/kind-ranked.txt" > "$V/kind-avail.txt" || s=$?
    [ "$s" -le 1 ] || { echo "SAMPLE FAIL: grep error ($s) filtering $name candidates"; exit 1; }
    cand=$(head -1 "$V/kind-avail.txt")
    [ -n "$cand" ] || { echo "SAMPLE FAIL: cannot satisfy the >=3 $name floor"; exit 1; }
    echo "$cand" >> "$V/sample.txt"; have=$((have+1))
  done
done
sort -u -t: -k1,1 -k2,2n -o "$V/sample.txt" "$V/sample.txt"   # dedupe, canonical order
n=$(wc -l < "$V/sample.txt"); [ "$n" -ge 18 ] || { echo "SAMPLE FAIL: only $n hunks"; exit 1; }
echo "SAMPLE: $n hunks selected (deterministic; re-run reproduces identically)"
```
Reads, in order, all with verdicts + hunk ids recorded:
1. every `mandatory.txt` hunk — expect ZERO pure hunks (the M-2 audit found every dance in
   the population adjacent to its construction) BUT expect the `prompts.rs` ⚠ trio's hunks
   here if their added lines exceed 3 in one hunk; any unexpected entry means a non-template
   edit;
2. **the six ⚠ mapping rows, unconditionally** (`prompts.rs` trio, `mouse.rs:2387`,
   `editor.rs:1750`, `diagnostics_run.rs:770`) — verify each against its mapping rationale;
3. the `sample.txt` hunks per spec §10.3-2's checklist: right seam fn (F/D per the mapping),
   label keeps stem+extension, dance lines actually deleted, the bound name is what the test
   goes on to use, nothing else rode in the hunk.

### 9g. Gates, smoke, evidence bundle

- `cargo test --workspace` · `cargo build` + `cargo test --no-run` warning-free ·
  `cargo clippy --workspace --all-targets` clean — at `<tip>`.
- `scripts/smoke/run.sh` — quote its one-line summary VERBATIM in the report
  (advisory-pass; a red result is surfaced, never hidden).
- Bundle: `endpoints.txt`, all `$V/` outputs, the three tasks' mutation evidence (observed
  reddened-test names), and the mandatory/⚠/sample read log. Clean up (one path per
  invocation): `git worktree remove /tmp/bt-pre` · `git worktree remove /tmp/bt-tip` ·
  `git worktree remove /tmp/bt-base`.

### 9h. Ship-time notes (merge, not this branch)

At merge (`superpowers:finishing-a-development-branch`): H38/H28/H36 move to
`docs/backlog-archive.md` carrying spec §12's corrections VERBATIM (H28's disproven
title/mechanism claims; H36's measured census copied from spec §1.1, never re-derived by
hand); `doc =` repoints; `scripts/backlog bless`; the effort report carries D2's process note
(second consecutive inverted filed fix — ground the fix, not just the dependency).
