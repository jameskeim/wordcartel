# Batch T — test integrity: H38 + H28 + H36

**Status:** design spec for effort branch `effort-batch-t-test-integrity` (base `c93cc80`).
**Decisions:** `scratchpad/batch-t/decisions.md` (D1–D5) is binding; this spec implements it and
does not re-open it. Grounding evidence: `scratchpad/batch-t/fable-grounding.md`, verified at
`6d3a213` (`c93cc80` adds only the A25 backlog filing on top — no code moved; every anchor stands).
**Backlog:** items `H38` (debt, S), `H28`, `H36`; prose `docs/ux-backlog.md` under their
`<!-- item: … -->` markers; sequenced #2 in `docs/design/backlog-sequence.md`.

All code references anchor on symbol names; line numbers, where given, are as observed at
`c93cc80` and are advisory only.

---

## 1. The governing premise, and what this batch is

**Nothing in Batch T changes shipped behavior.** No user-visible string, no status kind, no
control-flow branch, no signature of production code moves. Every change lands in `#[cfg(test)]`
code, in test doc comments, or in one production *doc comment* (§3.4 — prose, zero codegen).
This premise is not decoration: it is what licenses reviewing a 97-site sweep as safe volume
rather than as 97 opportunities to alter something. Any finding during implementation that would
require breaking it — however small, however "while we're here" — is out of scope and gets
FILED, not fixed (the D3 discipline; A25 is the standing example).

The batch is three test-integrity items on the same instrument, one effort, internal order
**H38 → H28 → H36-last** (H28 and H36 collide on `prompts.rs:502,522` — grounding §1.6; the
sweep must not rewrite lines H28 is still re-documenting):

- **H38** — `lsp_client::classify_spell_heuristic`'s code-substring branch
  (`code.to_lowercase().contains("spell")`, `lsp_client.rs:69`) is pinned by no test, and the
  doc-comment claim that harper's private duplicate `harper_ls::classify_lsp` is "intentionally
  identical" is itself untested. Grounding §1.1 established the branch is live on **jar-grounded
  real rule ids**: `FR_SPELLING_RULE` (the French speller's own id, verified in the installed
  ltex-ls-plus LanguageTool 6.8 jars) and `EN_CONTRACTION_SPELLING` (plain-English) contain
  `spell` yet miss all three of `LtexEngine::classify`'s `MORFOLOGIK`/`HUNSPELL`/`SPELLER`
  short-circuits. The propagation of a LanguageTool rule id into the LSP diagnostic `code`
  string is **inferred** from the existing protocol handling and the T11 probe history, not
  live-probed (grounding's stated evidence boundary) — the tests and kill conditions do not
  depend on that inference, only the "reachable at runtime" prose does.
- **H28** — `save_as_empty_path_is_a_sticky_warning` and
  `block_write_empty_path_is_a_sticky_warning` (`prompts.rs:496,513`) were filed as asserting an
  unreachable state. Grounding §1.2 DISPROVED the filing: the warning is production-reachable by
  two routes (§4.1). The tests stay; their doc comments get the true story; the one genuinely
  unpinned path (Route B's kind-based fall-through) gets its pin.
- **H36** — sweep the 97 non-excluded `temp_dir().join(…)` scratch-path constructions onto
  `test_support::{scratch_path, scratch_dir}` (H32's seam), with a binding exclusion list, no
  textual scanner, and a verification strategy that does not consist of reading 97 hunks (§10).

### 1.1 The census (authoritative — every count elsewhere in this document REFERENCES this
table; no section restates a number it could cite)

Measured at `c93cc80` with `grep -rn 'temp_dir()' --include='*.rs' wordcartel/src
wordcartel/tests` (grounding §1.3, re-derived for this revision):

| quantity | value |
|---|---|
| total grep hits | **116** |
| files containing hits | **29** (27 under `wordcartel/src` + 2 under `wordcartel/tests`) |
| `.join` constructions | **101** = 99 same-line + 2 line-wrapped (`prompts.rs:1263,1383`) |
| — **sweepable joins (D4's population)** | **97** |
| — excluded joins (D4, binding) | **4**: `swap.rs:48` · `test_support.rs` `scratch_name` · `tests/harper_ls_probe.rs` · `tests/harper_ls_integration.rs` |
| bare code uses (never swept; 3 are D5's) | **12** (§5.3) |
| comment-line mentions | **3** (`prompts.rs:487,717`, `swap.rs:675`) |
| files the mechanical sweep touches | **25** (the 27 src files minus `chrome_geom.rs`, whose only hit is bare, and `test_support.rs`, whose only join is the seam itself) |
| post-batch code-line survivors | **13** (§10.2's exact list) |

Partition check: 97 + 4 + 12 + 3 = 116. Per-file hits, descending: `prompts.rs` 20 ·
`file_browser.rs` 17 · `app.rs` 11 · `jobs_apply.rs` 9 · `render_overlays.rs` 7 · `swap.rs` 6 ·
`workspace.rs` 5 · `file_browser_commit.rs` 5 · `session_restore.rs` 4 · `mouse.rs` 4 ·
`editor.rs` 3 · 7 files with 2 · 9 src files with 1 · 2 integration files with 1.

**Per-file SWEEPABLE joins — `E(f)`, the delegation ledger's expected values (§10.2), summing
to 97 over the 25 swept files:** `prompts.rs` 13 (11 same-line + 2 wrapped) ·
`file_browser.rs` 13 · `app.rs` 11 · `jobs_apply.rs` 9 · `render_overlays.rs` 7 ·
`workspace.rs` 5 · `file_browser_commit.rs` 5 · `session_restore.rs` 4 · `mouse.rs` 4 ·
`swap.rs` 3 (of its 6 hits: minus `:48` excluded, `:675` comment, `:682` bare) · `editor.rs` 3 ·
`timers.rs`, `render.rs`, `recovery.rs`, `file_browser_listing.rs`, `export.rs`,
`diagnostics_run.rs` 2 each · `save.rs`, `state.rs`, `search_ui.rs`, `recents.rs`, `fsx.rs`,
`e2e.rs`, `config.rs`, `clipboard.rs` 1 each. (Check: 77 + 12 + 8 = 97.)

### 1.2 Corrections to the filed record (carried to the archive at ship — §12)

- H28's title ("Un-pumped picker tests assert unreachable states") and its prose's central
  mechanism claim ("the empty-path warning is genuinely unreachable once a listing lands") are
  **disproven** (grounding §1.2, Routes A and B). The archive entry must carry the correction.
- H36's filed figures are stale: "~105 inline constructions across ~30 files" is really the
  §1.1 census (116 hits / 29 files / 101 joins / 97 sweepable), and the filed per-file
  estimates drifted too (`prompts.rs` 20, filed ~14; `app.rs` 11, filed ~15; `swap.rs` 6,
  filed ~10).

---

## 2. Decision summary (binding, from D1–D5)

| # | Decision |
|---|---|
| D1 | H38 pinned **three ways**: direct unit pin on `classify_spell_heuristic`; engine-level pin through `LtexEngine::classify` with the real LT ids `FR_SPELLING_RULE` + `EN_CONTRACTION_SPELLING`; fixture-matrix agreement test between the shared fn and harper's `classify_lsp`. Kill condition stated and mutation-verified (§3.5). |
| D2 | H28: **both tests stay**; both doc comments rewritten to the verified reachability story; the Route-B kind-based pin added on `classify_destination_enter`. Delete nothing — the `Nothing` arm also aborts a quit drain. |
| D3 | The behavioral tail (destination-commit refusal blames the field) is FILED OUT as **A25** (`c93cc80`, `docs/ux-backlog.md` `<!-- item: A25 -->`). Not fixed in this batch. |
| D4 | H36: the **full sweep of the 97 non-excluded joins** (census §1.1), sequenced LAST, binding exclusion list (12 bare sites + the 4 excluded joins: `swap.rs:48`, the seam's own impl, the 2 integration-test sites). **No textual scanner.** |
| D5 | The 3 PUMPED bare sites (`prompts.rs:714,794,818`) migrate to `scratch_dir()` seeds as ONE separate, clearly-labeled **semantic** commit. H28's two seeds (`prompts.rs:502,522`) stay bare. |

Internal order refined by this spec (D4 says "last", D5 says "separate"): **H38 → H28 → D5 →
D4-sweep**. D5 precedes the mechanical sweep so its semantic diff is never adjacent to (or
reviewable as) sweep noise in the same file, and so the prompts.rs sweep runs over settled lines.

---

## 3. H38 — pin the branch, three ways (D1)

No production code changes. `classify_spell_heuristic` (`lsp_client.rs:63-77`),
`LtexEngine::classify` (`ltex_ls.rs:83-91`), and `harper_ls::classify_lsp` (`harper_ls.rs:158`)
are all untouched. Three tests are added; one doc comment is amended.

### 3.1 T1 — direct unit pin, `lsp_client.rs` test mod

New test `classify_spell_heuristic_code_branch_pins_spelling`, a fixture matrix driving the
shared fn directly (it is `pub(crate)`; the test mod already has `serde_json::json`):

| fixture | expected | pins |
|---|---|---|
| `{"code":"FR_SPELLING_RULE","message":"x"}` | `Spelling` | **the code branch — the kill fixture** (message has no `spell`, so nothing rescues a deleted branch) |
| `{"code":7,"message":"x"}` | `Grammar` | the non-string-code arm (`other.to_string()` → `"7"`) |
| `{"source":"cspell","message":"x"}` | `Spelling` | the source half of the message-path — also previously unpinned on the shared fn |
| `{"message":"Possible spelling mistake"}` | `Spelling` | the message half |
| `{"message":"style"}` | `Grammar` | the fall-through |

### 3.2 T2 — engine-level pin with real ids, `ltex_ls.rs`

Extend the existing `classify_maps_languagetool_speller_rules_to_spelling` (`ltex_ls.rs:151`)
with two assertions, each commented as a **verified-real** LanguageTool 6.8 rule id (grounding
§1.1 — extracted from the installed ltex-ls-plus jars; `FR_SPELLING_RULE` is
`org.languagetool.rules.fr.MorfologikFrenchSpellerRule`'s id) that misses all three uppercase
short-circuits:

```rust
assert_eq!(LtexEngine::classify(&json!({"code":"FR_SPELLING_RULE","message":"x"})),
    DiagnosticKind::Spelling, "jar-verified LT 6.8 id: misses the engine short-circuits, exercises the shared code branch");
assert_eq!(LtexEngine::classify(&json!({"code":"EN_CONTRACTION_SPELLING","message":"x"})),
    DiagnosticKind::Spelling, "an English id — the code branch matters for en-US configs too");
```

(The ids are jar-grounded; their arrival as LSP `code` strings is inferred per §1's stated
evidence boundary — these fixtures pin the classifier's contract for such inputs either way.)

### 3.3 T3 — agreement matrix, `harper_ls.rs` test mod

New test `classify_lsp_agrees_with_the_shared_heuristic`: one fixture slice covering every path
(code-with-`spell` / code-without + message-with / source-with / non-string code / neither),
asserting `classify_lsp(d) == crate::lsp_client::classify_spell_heuristic(d)` per fixture (both
symbols are in-crate reach: `classify_lsp` is private to `harper_ls.rs`, so the test lives
there; the shared fn is `pub(crate)`). This retires the untested "intentionally identical" claim
by making divergence a test failure in either direction.

### 3.4 The doc-comment amendment (the one production-file touch in the batch)

`classify_spell_heuristic`'s doc comment (`lsp_client.rs:59-62`) currently asserts "the two
bodies are intentionally identical" with nothing enforcing it. Amend the sentence to cite the
pin, e.g. "…the two bodies are intentionally identical — pinned by
`harper_ls::tests::classify_lsp_agrees_with_the_shared_heuristic`." Prose only; zero codegen;
declared here so §10.5's premise audit stays exhaustive.

### 3.5 Kill conditions (D1 verbatim, mutation-verified)

Deleting the code branch (`lsp_client.rs:69`, the `contains("spell")` early return) MUST redden:
- T1's `FR_SPELLING_RULE` row, and
- BOTH T2 assertions (they pass the engine short-circuit un-short-circuited, and `message:"x"`
  forecloses the rescue), and
- T3 (divergence: harper's duplicate still classifies `Spelling`).

Separately, mutating `classify_lsp`'s own code branch MUST redden T3 (and harper's existing
`classify_lsp_spelling_vs_grammar`) while leaving T1/T2 green — proving T3 constrains BOTH
bodies, not one. The implementer verifies each by mutation — break, watch redden, restore,
confirm restoration with `git diff` — never by reading (§10.4).

---

## 4. H28 — keep, re-document, pin Route B (D2)

No production code changes. `classify_destination_enter`, `commit_destination_with_probe`, the
`Nothing` arm, its warning string, and its quit-drain abort (`file_browser_commit.rs:363-377`)
are all untouched.

### 4.1 The verified reachability story the doc comments must now tell

Two production routes to the "empty path" warning, both verified end-to-end (grounding §1.2):

- **Route A — the pre-listing async window.** `open_save_as` (`prompts.rs:83-93`) and
  `block_write` (`blocks_marked.rs:128-143`) both seed `field: String::new()`;
  `Editor::open_destination_picker` (`editor.rs:1042-1064`) starts with `entries: Vec::new()`
  and spawns the listing off-thread ("There is no synchronous listing path");
  `file_browser_intercept::intercept`'s Enter arm has NO pending-listing guard. Enter before
  `Msg::ListingDone` folds ⇒ `highlighted = None` ⇒ Row 2 `_ => Nothing` ⇒ the Sticky Warning.
  The un-pumped tests construct EXACTLY this state — their non-pumping is correct and
  load-bearing, not an oversight.
- **Route B — a landed listing, a navigated non-Dir/non-File highlight.** Production listings
  contain `Other` (fifo/socket/device, `fsx::kind_of`) and `Unknown` (stat failure; broken
  symlink) rows; `filter_and_rank` keeps broken entries unconditionally and extensionless
  `Other` entries via `is_document`. An arrowed-onto such row + empty field fails Row 1 (not
  `Dir`) and Row 2's commit (not `File`) ⇒ `Nothing` — with a fully landed listing.

### 4.2 Doc-comment rewrite — content checklist (binding; exact prose is the implementer's)

Both comments (`prompts.rs:482-494` and the twin's `:509-511`) must state:
1. what the test pins: the **pre-listing window**, a real production state by design (Route A's
   chain, summarized);
2. why the test must NOT pump: pumping moves the fixture into a different, already-covered
   state (the landed-listing `".."`-descend, covered by the pumped destination tests) and would
   delete this assertion while appearing to fix it;
3. that the kind-based companion state (Route B) is pinned separately in
   `file_browser_commit.rs` (§4.3), and the refusal-wording defect it exposes is filed as A25;
4. drop the reverted-pump experiment as the load-bearing rationale (one-line historical note is
   fine); drop any implication that the asserted state is an artifact.

The comments may keep reasoning about the shared `temp_dir()` seed's `".."` row (why pumping
would descend) — which is exactly why D5 leaves these two seeds bare (decisions, D5).

### 4.3 T4 — the Route-B pin, `file_browser_commit.rs`

New test `an_empty_field_with_a_non_writable_highlight_commits_nothing`, beside the existing
`an_empty_field_with_no_highlight_commits_nothing` (`file_browser_commit.rs:2276` — which pins
the `None`-highlight leg; the kind-based leg is pinned nowhere, grounding §1.11). Shape:

```rust
#[test]
fn an_empty_field_with_a_non_writable_highlight_commits_nothing() {
    let d = tmp("nothing-kind");
    for navigated in [false, true] {          // Row 2 does not consult the flag — pin that too
        for kind in [EntryKind::Dir, EntryKind::File, EntryKind::Other, EntryKind::Unknown] {
            let e = fe("entry", kind);
            let got = classify_destination_enter(&crate::fsx::RealFs, &d, "", Some(&e), navigated);
            // EXHAUSTIVE over EntryKind, no wildcard: a fifth variant fails to COMPILE here,
            // forcing its Enter fate to be decided rather than absorbed by `_ => Nothing`.
            match kind {
                EntryKind::Dir => assert!(matches!(got, CommitOutcome::Descend(_))),
                EntryKind::File => assert!(matches!(got,
                    CommitOutcome::Commit { from_highlight: true, .. })),
                EntryKind::Other | EntryKind::Unknown => assert_eq!(got, CommitOutcome::Nothing,
                    "not a writable regular file — never a commit target ({kind:?})"),
            }
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}
```

Honest limits, stated so review does not over-credit it: the exhaustive `match` compile-forces
attention on a future `EntryKind` variant, but the iteration ARRAY is not compiler-checked —
adding the new variant's match arm without adding it to the array compiles; the review checklist
carries that pairing. `broken`/`is_symlink` are deliberately not varied: `classify_destination_enter`
reads only `e.kind` and `e.name` (the `fe` builder's defaults are fine).

**Kill condition:** widening Row 2's commit to non-`File` kinds — e.g. replacing the
`Some(e) if matches!(e.kind, EntryKind::File)` guard with `Some(e)` — MUST redden the
`Other`/`Unknown` legs (they would read `Commit`). Verify by that exact mutation; note the
existing `None`-highlight test stays GREEN under it — demonstrating this is the discriminating
coverage being added, not duplication. Mutation-verified per §10.4.

---

## 5. H36 — the sweep (D4) and the three-site migration (D5)

### 5.1 The seam (exists; unchanged by this effort)

`test_support::{scratch_path, scratch_dir}` over `scratch_name` (`test_support.rs`):
`temp_dir()/wc-scratch-{pid}-{seq}-{label}`, `AtomicU64` seq — every returned path is unique by
construction; `scratch_dir` is created and **empty by construction** (subsumes every legacy
remove-then-create dance). Guardrail test `scratch_seam_is_collision_free_under_contention`
already pins it. H36 adds no seam behavior.

### 5.2 Population and rewrite templates (D4)

**Population: the 97 sweepable `.join` construction sites** (census §1.1 — the 101 joins minus
the 4 binding exclusions). Grounding §1.5 verified all join sites (the excluded four included)
are `format!`-built with `std::process::id()` on the same line and carry pairwise distinct
labels — individually correct today; the sweep is delegation, not repair.

Two templates; a rewrite whose hunk is pure or oversized is mandatorily read (§10.3-1), and a
wrong rewrite INSIDE a template-shaped hunk falls to the §10.3-2 sample and per-task review —
§10.3's two-strengths statement governs which is which:

- **Template F (file path):**
  `std::env::temp_dir().join(format!("wc-<stem>-{}.<ext>", std::process::id()))`
  → `crate::test_support::scratch_path("<stem>.<ext>")`.
  The label keeps the discriminating stem and the extension; the `wc-` prefix and the pid drop
  (the seam supplies `wc-scratch-` and the pid). Extra discriminators beyond the pid (e.g.
  `prompts.rs:1263`'s `{tag}`) ride in the label: `scratch_path(&format!("h5-inv-{tag}.txt"))`.
- **Template D (directory):** the construction PLUS any adjacent pre-provisioning —
  `let _ = remove_dir_all(&d);` and/or `create_dir_all(&d).unwrap()` — collapses to
  `crate::test_support::scratch_dir("<stem>")`. **Leading** remove-then-create dances are
  DELETED (counter-freshness subsumes them — the seam's doc comment says so and its guardrail
  proves it). **Trailing** cleanup lines (`let _ = std::fs::remove_dir_all(&d);` at test end)
  are PRESERVED — deleting them is behavior-neutral but changes the disk-litter profile and
  inflates the diff; the sweep's mandate is minimal mechanical delegation.

Multi-path tests make one seam call per path; labels stay distinct within a test for
readability (uniqueness is structural now, not conventional).

### 5.3 The exclusion list (D4, binding — these lines do not change in the sweep)

The 12 bare sites plus the 4 excluded joins (census §1.1):

| Excluded | Why |
|---|---|
| 12 bare sites: `prompts.rs:502,522,714,794,818` · `file_browser.rs:732,1086,1101,1120` · `save.rs:1265` · `chrome_geom.rs:882` · `swap.rs:682` | Not joins — the directory itself is the subject (picker seeds, `FileBrowser` literals, a `starts_with(temp_dir())` assertion); substitution is a semantic change, not delegation. Three of them are D5's, migrated in D5's own commit, not the sweep. |
| `swap.rs:48` (excluded join 1) | `#[cfg(test)]` branch inside PRODUCTION `state_dir()` with per-process STABILITY semantics; the counter-based seam returns a different path per call and would break swap/session/recovery structurally (grounding §1.7). |
| `tests/harper_ls_probe.rs`, `tests/harper_ls_integration.rs` (excluded joins 2–3) | Separate crate; cannot reach a `#[cfg(test)] pub(crate)` seam. |
| `test_support.rs`'s `scratch_name` (excluded join 4) | The seam's own implementation. |

Comment-line mentions (`prompts.rs:487`, `swap.rs:675`; `prompts.rs:717` until D5 rewrites it)
are not code and are not swept.

**No textual scanner** is added (D4): no gate banning raw `temp_dir().join(…)`. The
`fs_chokepoint` precedent measured 5 of 6 evasion routes uncaught; a second scanner is
self-defeating. Completeness is proven once, at review, by §10.2's residue oracle instead.

### 5.4 D5 — the three pumped seeds, one semantic commit

`prompts.rs:714` (`save_as_existing_target_raises_overwrite_prompt`), `:794`
(`block_write_failure_is_a_sticky_error_that_survives_a_later_info`), `:818`
(`block_write_existing_target_raises_overwrite`): each replaces its
`open_destination_picker(…, std::env::temp_dir(), field)` seed with a
`crate::test_support::scratch_dir("<test-stem>")` seed. These three PUMP the listing and then
act on it — the only sites in the population that load a directory other processes control.
Post-migration the listing is hermetic; the properties the tests rely on survive:
- the seeded dir still has a parent, so the listing still carries the `".."` row;
- the field is a non-empty ABSOLUTE path, so `resolve_field` ignores the seed dir and
  `highlight_is_navigated()` still gates Row 1 off — assertions are untouched.

**The commit must also rewrite the now-stale defense comment** at `prompts.rs:715-718`
("regardless of whatever `temp_dir()` happens to sort first … a shared system temp dir full of
other tests' leftovers"): the absolute-field/Row-1 reasoning stays, the shared-dir motivation
goes (the listing is now owned). The tests' target-file constructions (`:708,:784,:809`) are
ordinary Template-F join sites and belong to the LATER mechanical sweep, not this commit — the
seed dir and the target path are independent (absolute field passes through `resolve_field`
regardless of either).

One commit, subject line marked semantic (e.g. `test: [prompts] hermetic listing seeds for the
three pumped picker tests (Batch T D5 — semantic, not sweep)`), reviewed with eyes (§10.3).

---

## 6. Invariants

1. **Shipped behavior is unchanged.** No production code line changes anywhere in the batch;
   the sole non-`#[cfg(test)]` touch is §3.4's doc-comment sentence. Checked, not asserted:
   §10.2 (residue oracle), §10.5 (region check + enumerated-touch audit).
2. **No test weakens.** Every existing assertion survives verbatim; the batch only ADDS tests
   (T1–T4) and rewrites comments/labels. The two H28 tests keep asserting the same
   message/kind/lifetime through the same real intercept, un-pumped.
3. **The sweep is inventory-neutral.** No test is added, removed, or renamed by a sweep commit
   (§10.1). D5 renames nothing either — it changes seeds and a comment.
4. **Uniqueness becomes structural.** Post-sweep, swept scratch paths are unique by pid+seq
   construction rather than by label convention; no swept test depends on a path's exact shape
   (a shape-changing rewrite trips a §10.3-1 read trigger when its hunk is pure or oversized;
   one hiding in a template-shaped hunk falls to the §10.3-2 sample and per-task review —
   §10.3's two-strengths statement).
5. **Idle/resource behavior unchanged** — no new timers, background work, or production
   allocations; test-only code.

---

## 7. Command-surface contract conformance

Per `docs/design/command-surface-contract.md`: **N/A — this effort does not touch the command
surface**, argued rather than asserted:

- No command, option, palette row, menu entry, keybinding, or hint is added, removed, renamed,
  or re-worded. The registry is not edited; no `SettingsSnapshot` field moves.
- Every change is `#[cfg(test)]` code, test doc-comment prose, or one production doc comment
  (§3.4) — none of which the contract's laws range over.
- The contract's invariant tests (palette-completeness, every-option-has-a-command, hint
  re-resolution) are untouched and remain gating; the batch cannot alter their subjects.
- Law 10 ("should a plugin ever do this?") does not attach: no capability is created or moved.

---

## 8. GATEs (project law; all must pass on the branch tip before merge)

- `cargo test` green across all suites (`wordcartel-core` lib + oracle, `wordcartel` lib,
  integration tests) — plus this batch's stronger form: green at EVERY commit (§10.1's
  intermediate-greenness requirement, the Effort-④ lesson).
- `cargo build` and `cargo test --no-run` warning-free for the touched crate (`wordcartel`).
- `cargo clippy --workspace --all-targets` clean (workspace `clippy::all = "deny"`).
- `clippy::too_many_lines` (100) and the `wordcartel/tests/module_budgets.rs` hub budgets —
  no touched file gains production lines, so budgets cannot move; stated for completeness.
- Backlog drift gate (`wordcartel/tests/backlog.rs`) — relevant at ship time when H38/H28/H36
  move to the archive with §12's corrections.
- PTY smoke suite `scripts/smoke/run.sh`: mandatory-run, advisory-pass; the pre-merge report
  quotes its one-line summary verbatim. No smoke check exercises test-only code, so no change
  is expected.
- **`cargo fmt` is forbidden** (hand-formatted repo). Swept lines match their neighbours by
  hand; the sweep templates are formatted to the surrounding style, not rustfmt's.

---

## 9. What could go wrong (designed-for failure modes)

- **A sweep rewrite silently changes a fixture's semantics** (the headline risk — three efforts
  have had findings hide in mechanical-looking diffs). Answered structurally, not by
  vigilance where instruments reach: the shipped-behavior premise, the inventory, the residue,
  and the per-file seam counts are CHECKED properties; per-site correctness inside
  template-shaped hunks is read where triggered or sampled, and per-task-reviewed where not
  (§10.3's two-strengths statement).
- **A swept test depended on its path SHAPE** (asserted a filename substring, re-derived the
  path). The templates preserve stem+extension in the label, and any site that resists a
  template trips a §10.3-1 mandatory-read trigger when its hunk is pure or oversized —
  arithmetic on the diff, not a declaration by the implementer. What hunk shape misses falls
  to the delegation ledger (§10.2) and both-green-with-identical-inventory (§10.1) where those
  reach, and otherwise to the §10.3-2 sample — probabilistically, per §10.3's two-strengths
  statement.
- **The sweep touches an excluded line** (an H28 seed, `swap.rs:48`). The residue oracle
  (§10.2) fails loudly on a vanished survivor, and the survivor comparison (§10.2) fails on a
  modified one — the 13 excluded lines are pinned as an exact list AND byte-for-byte.
- **D5 breaks a pumped test's ordering assumptions.** The three tests' assertions are
  content-independent by construction (grounding §1.4); D5 changes seeds only, and runs the
  full suite plus those three tests explicitly.
- **A new pin passes vacuously** (the eleven-tests-that-cannot-fail class from E11). Every new
  test carries a stated kill condition executed by real mutation (§10.4), including the
  negative control for T3 (mutate harper's side, watch T1/T2 stay green).
- **The batch premise erodes by a thousand cuts** (a "tiny" wording fix riding a sweep commit).
  §10.5's region check rejects any sweep hunk outside a `#[cfg(test)]` region mechanically;
  D3/A25 is the standing precedent for filing instead.

---

## 10. Verification strategy — how a reviewer is convinced without reading 97 hunks

Reading the diff is NOT the strategy. The sweep's claims are decomposed into properties, each
with a mechanical instrument; human eyes are reserved for the short list §10.3 enumerates.
Instruments ruled in are concrete enough for the plan to carry as scripted task steps; ruled-out
candidates are recorded with reasons so review does not re-litigate them.

### 10.1 Inventory and pass-set invariance (mechanical)

- Capture `cargo test -- --list` (normalized: sorted, counts stripped) at the commit BEFORE the
  first sweep commit and at the sweep's tip. **The diff must be empty** — the sweep adds,
  removes, and renames nothing. (For the whole branch: the inventory delta versus base is
  exactly **three new test names** — T1 `classify_spell_heuristic_code_branch_pins_spelling`,
  T3 `classify_lsp_agrees_with_the_shared_heuristic`, T4
  `an_empty_field_with_a_non_writable_highlight_commits_nothing` — each at its task commit.
  T2 is an instrument but not a name: it adds two assertions to the EXISTING
  `classify_maps_languagetool_speller_rules_to_spelling` and registers nothing. Four
  instruments, three new names.)
- Run the FULL suite at the pre-sweep commit and at the tip; both green. Green+green with an
  identical inventory is the pass-set comparison — there is no per-test result to diff once the
  name sets are equal and both runs exit clean. Running at the parent is load-bearing: it rules
  out "the suite was already red and the sweep's run inherited it".
- **Intermediate greenness, one commit per file:** sweep commits are sized per file (or small
  file groups), and **each swept file's rewrites land in exactly ONE sweep commit — a file is
  never split across sweep commits** (binding; §10.3's read set depends on it to make the
  aggregate diff equal the per-commit union). Every commit builds and passes
  `cargo test -p wordcartel` — the Effort-④ cross-task intermediate-green lesson, applied to
  a 25-file mechanical series (census §1.1) where a mid-series breakage would otherwise be
  archaeology.

### 10.2 The residue oracle + the delegation ledger (mechanical — disappearance, exclusion-compliance, AND delegation)

At the sweep tip, `grep -rn 'temp_dir()' --include='*.rs' wordcartel/src wordcartel/tests`,
filtered to code lines (drop lines whose trimmed form starts with `//` or `///`), must return
**exactly this survivor list and nothing else**:

| survivor | why it remains |
|---|---|
| `prompts.rs` H28 seeds ×2 (the `open_destination_picker(…, std::env::temp_dir(), "   ")` lines) | D5 rejected migrating them; bare, never pumped |
| `file_browser.rs` ×4 (`open_file_browser` seed; two `FileBrowser` literals; `apply_listing_done` arg) | bare — the dir is the subject |
| `save.rs` ×1 (Esc-drain picker seed) · `chrome_geom.rs` ×1 (`FileBrowser` literal) | bare |
| `swap.rs` ×1 (`starts_with(std::env::temp_dir())` assertion) | semantic use of the real temp dir |
| `swap.rs:48` (`state_dir()`'s `#[cfg(test)]` branch) | binding exclusion (stability semantics) |
| `test_support.rs` ×1 (`scratch_name`) | the seam itself |
| `tests/harper_ls_probe.rs` ×1, `tests/harper_ls_integration.rs` ×1 | separate crate |

Anchor survivors by symbol/context, not line number (lines drift under the sweep itself). A
missing survivor = the sweep over-reached (touched an exclusion); an extra hit = the sweep is
incomplete. One grep settles both directions. The D5 tip additionally shows zero `temp_dir()`
in the three migrated seeds.

**The survivor comparison — exclusion integrity, byte-for-byte.** The oracle alone proves the
expected OCCURRENCES survive; a modification that retains the `temp_dir()` token would pass it
(the H28 seed lines at `prompts.rs:500-504` carry the seed AND other arguments on the same
line — editing another part of such a line leaves the grep survivor present). §5.3 binds "these
lines do not change," and 13 lines is a population small enough to deserve the real instrument:
extract the 13 survivor code lines at `<pre-sweep>` and at `<tip>` by the same
anchoring/filtering the oracle uses, pair them by the survivor table's anchors, and require
each pair **byte-for-byte identical**. The byte comparison is the primary procedure and is exact
on its own; the equivalent diff formulation is that no survivor line appears as an added or
removed line in the zero-context (`-U0`) aggregate patch — "touches" means *added or removed*,
NOT "falls within some hunk's range", which under `-U0` would be a different and weaker test. An edit to a survivor's ADJACENT lines (the rest of a multi-line call) is not a
sweep rewrite and is banned outright by the sweep-only interval rule (§10.3); if present it
surfaces in the read set per the two-strengths statement. Both the oracle and the ledger below
are REVIEW steps, run once at the gate — deliberately NOT committed as tests or CI gates,
which would be the scanner D4 forbids; the survivor comparison runs beside them.
**Interpretation, precisely:** the oracle proves the 97 non-excluded joins are GONE, and the
survivor comparison proves the excluded lines UNCHANGED — nothing more. Neither can prove a
vanished site was delegated rather than mangled or deleted, and §10.1 cannot either (unchanged
names + green cannot see a dropped assertion inside a surviving test, or a swap to a hardcoded
path that happens to pass). Delegation gets its own instrument:

**The delegation ledger.** Expected values come from the CENSUS, never from the implementer's
report. For every file `f`, let `E(f)` = its sweepable-join count derived from the BASE tree
(the §1.1 census's per-file breakdown, pinned in the plan before any sweep commit; sums to 97
over the 25 swept files, 0 elsewhere). Let `S(f)` = the count of `scratch_path(` +
`scratch_dir(` occurrences in `f`. Measure `S` at the same two endpoints §10.1 uses — the
commit BEFORE the first sweep commit (so D5's three added `scratch_dir` seeds and H32's
pre-existing seam calls are inside the baseline) and the sweep tip. **Require
`S_tip(f) − S_base(f) == E(f)` for every file in the tree.** Templates are one seam call per
swept site, so the equality is exact by construction. What it discriminates: a site rewritten
to any non-seam constructor, or deleted outright, leaves its file's delta short by one — the
ledger fails AT THAT FILE; a seam call invented where no site was swept overshoots the same
way. Two greps and a subtraction; the plan carries it beside the residue oracle as a one-shot
review step (not a committed gate — D4's no-scanner rule).

**Honest limits of the ledger:** it counts calls, not meaning. (a) A compensating error inside
ONE file — one site deleted, one spurious seam call added elsewhere in the same file — balances
the delta; §10.3-1's read triggers catch it WHEN the deletion and the spurious addition land
in separate hunks (each then a pure hunk, mandatorily read) — the common case. Halves that
coalesce into one mixed template-sized hunk evade the triggers and fall to the §10.3-2 sample
and the per-task reviewer (the residual R12 states; §10.3-1 says the same).
(b) It does not check the `scratch_path`-vs-`scratch_dir` CHOICE per site (both count once):
a wrong choice that matters breaks the test (a missing directory fails at runtime — §10.1's
green catches it); a choice that is merely unidiomatic (e.g. `scratch_path` + a RETAINED
`create_dir_all`) never enters the diff on its telltale line and survives every mechanical
instrument — it is caught only where the §10.3-2 sample draws that hunk (F-vs-D is on the
sample's checklist), a residual §10.3 states rather than papers over. (c) Labels are
unverified — any string passes; post-sweep, labels affect log readability only (uniqueness is
structural, §5.1).

### 10.3 The read set — eyes, honestly scoped and concretely specified

The strategy rests on the four instruments §10.1/§10.2/§10.5 specify, plus a READ SET whose
membership is decided by diff arithmetic and a commit hash — never by the implementer, and
never by a matcher that can be wrong about Rust syntax (the withdrawn classifier's failure
mode; §10.6 records why it went).

**What the four instruments prove, jointly:** (i) no production region changed (§10.5 region
check); (ii) no test dropped, renamed, or broken — at any commit (§10.1 inventory + green);
(iii) zero non-excluded `temp_dir()` joins survive and every excluded line is byte-identical
between the endpoints (§10.2's survivor comparison, riding the
oracle); (iv) each of the 25 swept files gained EXACTLY its census-expected number of seam
calls (§10.2 ledger). **What they do NOT prove — stated without hedging:** that a given site's
seam call is the one the test actually USES (a site could bind the seam call yet still build
its path some other way, if that other way avoids `temp_dir()`); and that the per-site F-vs-D
template choice is right. The read set addresses those residuals in two different strengths,
and this spec does not conflate them: the item-1 triggers are GUARANTEES for the shapes they
name (any pure or oversized hunk is read, every time); the item-2 sample is PROBABILISTIC —
it establishes the per-site properties for the sampled sites and raises the odds of catching
a one-off mistake elsewhere, but an unsampled template-shaped hunk is verified by no
instrument, only by the per-task reviewer. The threat model remains a MISTAKE, not an
adversary: against deliberate evasion no review procedure of any size certifies intent, and
this spec does not claim otherwise — that boundary is the pipeline's per-task-reviewer trust
boundary, the same one every effort's mechanical checks stand on.

**One diff representation for the whole read set — and a bound interval to compute it over.**
Both the item-1 triggers and the item-2 candidate population are computed over the SAME diff:
the single aggregate `git diff -U0 <pre-sweep>..<tip>`. Two sequencing rules, both BINDING on
the plan (the plan is what orders the commits), make this well-defined:
- **The interval is sweep-only.** `<pre-sweep>` is **D5's commit itself** — the commit BEFORE
  the first sweep commit (D4 is sequenced last, so D5 is the effort's last non-sweep commit;
  this is the same baseline §10.1 and §10.2 already name) — and **no non-sweep change of any
  kind lands between `<pre-sweep>` and `<tip>`**. Binding the REWRITES alone would not be
  enough: two swept files (`prompts.rs`, `file_browser_commit.rs`) also receive H28/D5
  changes earlier in the effort, and §10.5's region check accepts any test-region hunk, so a
  stray in-interval edit would silently join the candidate population. If a non-sweep change
  becomes necessary mid-sweep (a fix to H38/H28/D5 work, say), exactly two placements are
  legal: REORDER it before `<pre-sweep>` (rebase; the pre-sweep endpoint recomputes to the
  new last non-sweep commit), or land it strictly AFTER `<tip>` — **and the tip does NOT
  advance across it**: `<tip>` is the last sweep commit, full stop; a later fix sits outside
  the interval and outside these instruments' scope (it gets ordinary review). When the
  pre-sweep endpoint moves, `E(f)` stays the base-tree census, but **`S_base` is RE-MEASURED
  at the recomputed pre-sweep commit** — it is an endpoint quantity, not a census one. A
  reviewer who finds the interval violated does not reason around it: recompute the
  endpoints and re-run every interval-anchored instrument — §10.1's inventory/green
  endpoints, §10.2's ledger (with re-measured `S_base`), §10.2's residue oracle and survivor
  comparison (both evaluated at `<tip>`), and this read set. They all share the same two
  commits.
- **Each swept file's rewrites land in exactly ONE sweep commit** — a file is never split
  across sweep commits (a tightening of §10.1's per-file sizing). With both rules, sweep
  commits partition the 25 files, the aggregate diff restricted to any file IS that file's
  single commit diff, and "per-commit vs aggregate" is vacuous by construction — one command,
  one hunk set, reproducible by anyone. (§10.5's region check remains per-commit; its
  property — no commit mixes in a production-region hunk — is per-commit by nature and shares
  no state with the read set.)

1. **Mandatory reads — triggered by diff arithmetic, zero content inspection.** Over the
   aggregate diff:
   - every hunk with added lines but NO removed lines, or removed lines but NO added lines. A
     template rewrite always REPLACES (the construction goes, a seam call arrives), so a pure
     hunk cannot be a COMPLETE, SELF-CONTAINED template replacement — this is what surfaces a
     dropped assertion, a smuggled extra (helper, scanner, anything), and each half of a
     compensating add/delete pair, WHENEVER the stray change lands in its own hunk — the
     common case. The uniform caveat: a stray change adjacent enough to a template rewrite to
     coalesce into one mixed, template-sized `-U0` hunk (adjacent lines are real —
     `jobs_apply.rs:1000-1001`) evades both triggers and falls to the §10.3-2 sample and the
     per-task reviewer — the same residual R12 states. Correct work COULD land
     here in principle — a Template-D dance separated from its construction by an intervening
     unchanged line would arrive as a small pure-removal hunk — but the citation audit found
     ZERO detached dances in the current population: every dance in the 25 swept files is
     contiguous with its construction (e.g. `file_browser_listing.rs:387-388`,
     `recovery.rs:147-149`), forming one mixed hunk under `-U0`. The implementer should
     therefore expect NO false positives from this trigger; any that appears is itself worth
     the seconds it costs to read;
   - every hunk exceeding the template maxima: more than 4 removed or more than 3 added lines
     (join + up to two adjacent provisioning lines out; a wrapped seam call in).
2. **The stratified sample — minimum 18 template-shaped hunks (all of them if fewer remain
   after item 1).** Selection is derived from the sweep tip's commit SHA — reproducible by
   anyone with standard tools, chosen by no one, and unknowable to the implementer while
   writing the sweep (the SHA depends on the sweep's own content). Two reviewers computing it
   independently MUST get the identical set; the procedure, exactly:
   - *Candidate enumeration.* Candidates = every hunk of the aggregate diff (the shared
     representation defined above item 1) NOT claimed by item 1's triggers. **Path bytes,
     pinned once:** a hunk's file path is the `+++` header's repository-relative path with
     the `b/` prefix removed (e.g. `wordcartel/src/prompts.rs`), compared as raw bytes,
     ascending. EVERY path ordering in this algorithm — the canonical order here and the
     11-heavy-files top-up below — uses exactly these bytes in exactly this order. A hunk's
     canonical id is the string `<file path>:<new-file start line>` from its `@@` header;
     canonical order is a stable sort by (file path per the pinned bytes; then start line,
     numeric ascending). This identifies each site stably against the real shapes — the
     adjacent constructions at `jobs_apply.rs:1000-1001` are distinct hunks (or one merged
     hunk with one id; either way, one unambiguous identity), and the wrapped forms at
     `prompts.rs:1263,1383` are each one hunk.
   - *Ranking (no PRNG — a keyed digest is the randomness).* For each candidate, compute
     `sha256("<tip SHA>:<canonical id>")` over the exact ASCII bytes (full 40-hex lowercase
     tip SHA, one colon, the id). Rank candidates by digest, hex ascending; ties (a 2⁻²⁵⁶
     event) break by canonical order. No PRNG algorithm or version to name; `sha256sum` in a
     shell loop reproduces it.
   - *Order of operations.* (1) Unconditional inclusions first: every candidate whose ADDED
     lines contain `format!(` — the dynamic-label hunks, the only sites where transcription
     can silently drop a discriminator, which include the two wrapped forms. (2) The draw:
     the 18 lowest-ranked candidates (drawing is by rank, hence without replacement; the
     unconditional inclusions do not count toward the 18). (3) Floor top-ups, applied in this
     fixed order, each adding the LOWEST-RANKED qualifying candidate not already selected:
     for each file with `E(f)` ≥ 3 (the 11 heaviest, census §1.1) in the pinned path-byte
     order, one hunk from that file; then at least 3 hunks whose added line calls
     `scratch_path(` (Template F); then at least 3 whose added line calls `scratch_dir(`
     (Template D). Floors may push
     the total past 18 + inclusions — that is the intent, "minimum 18," not "exactly."
   The reviewer reads each sampled hunk against §5.2's templates: right seam fn (F vs D),
   label preserves stem + extension, dance lines actually deleted, the bound name is the one
   the test goes on to use, nothing else rode in the hunk.
3. **The D5 commit** — semantic by declaration: three seed changes + one comment rewrite, full
   human review (it is ~10 lines).
4. **The H28 doc-comment prose** (§4.2's checklist compliance) and §3.4's one-sentence
   production doc-comment amendment.
5. **The new tests T1–T4** — reviewed as tests (fixture soundness), with §10.4's mutation
   evidence attached rather than re-derived by reading.

### 10.4 Mutation discipline (every new test, no exceptions)

Each of T1–T4 states its kill condition in this spec (§3.5, §4.3). The implementer executes it:
apply the named mutation to the target, run the suite, WATCH the named test(s) redden, restore,
and confirm restoration with `git diff` (clean tree), recording the observed failing test names
in the task report. Reading the test and believing it is not verification — this batch exists
because a test named for a branch never executed it (H38's vale history) and eleven E11 tests
asserted their fixtures' default states. T3 additionally runs the negative control (§3.5's
second mutation) to prove it constrains both bodies independently.

### 10.5 The premise audit (mechanical + one enumerated list)

- **Region check, scripted — handling BOTH test-region shapes explicitly.** The tree has two
  ways a file's `temp_dir()` sites are test-gated, and the script must name both or it either
  fails a legitimate commit or silently skips a file:
  1. *Trailing test mod* — 96 of the 97 sweepable sites sit at/after their file's
     `#[cfg(test)] mod tests` line (grounding §1.3's per-file measurement).
  2. *Whole-file test module* — the 97th site is `e2e.rs`, which has NO `mod tests` marker:
     it is `#![cfg(test)]` at file top and declared `#[cfg(test)] mod e2e;` in `lib.rs:86`.
     Every line of it is test region.
  Algorithm, per sweep commit, per touched file, over `git diff -U0`: if the file's first
  inner attribute is `#![cfg(test)]` → all hunks pass; otherwise locate the
  `#[cfg(test)]`-annotated `mod tests` line and require every hunk's start ≥ that line; a file
  matching NEITHER shape, or any hunk before the marker, fails the check and the commit is
  rejected as mixed. (~15 lines of script; the plan carries it as a task step.)
- **Enumerated non-test-region touches for the whole batch:** exactly one — §3.4's doc-comment
  sentence in `lsp_client.rs` (prose). H28's comments live inside the `prompts.rs` test mod;
  D5's comment likewise. Ship-time backlog/docs edits are bookkeeping, not code.
- Together: "nothing in Batch T changes shipped behavior" is a checked property of the branch,
  not a reading conclusion.

### 10.6 Instruments considered and ruled OUT, with reasons

- **Binary-hash comparison** of the production `wcartel` artifact at parent vs tip (cfg(test)
  code is absent from it, so it "should" be identical): ruled out — Rust builds are not
  bit-reproducible by default (path/metadata embedding, incremental state); a mismatch would be
  noise and a match luck. §10.5's source-level region check delivers the same guarantee
  deterministically.
- **Scripted label-uniqueness check** across swept sites: ruled out as unnecessary — uniqueness
  is structural post-sweep (pid+seq in `scratch_name`, guardrail-tested); label collisions
  post-sweep would affect log readability only. Pre-sweep uniqueness was already verified in
  grounding (§1.5, zero duplicates).
- **A committed scanner/gate** on raw `temp_dir()`: excluded by D4 (binding); §10.2's oracle
  and ledger and §10.3's read triggers are one-time review instruments, not standing gates.
- **Trusting the implementer's reported template/deviation count:** ruled out — a check whose
  expected number comes from the checked party's own report is not independent (the round-2
  finding). The ledger's expectations come from the census at base; the read set comes from
  diff arithmetic and a commit-SHA seed.
- **A diff-shape CLASSIFIER (line- or statement-level anchored matching):** attempted in
  rounds 2–3 of this spec's own gate and withdrawn. Each repair demanded more Rust-awareness —
  wrapped statements, multi-statement physical lines (`file_browser_listing.rs:388`), comment
  stripping, macro forms — converging on a parser embedded in a design document, for a batch
  of 97 mechanical test-code rewrites. Disproportionate, and never quite sound: every
  granularity had either false positives on correct-but-unusual shapes or blessed some
  incorrect ones. Replaced by §10.3's arithmetic triggers + specified sample, whose soundness
  does not depend on parsing Rust.
- **AST-level verification of each rewritten expression:** ruled out as disproportionate for
  the same reason at higher cost — and the residual no classifier reaches (label semantics,
  F-vs-D choice) is exactly what an AST check could not decide either.
- **Reading all 97 hunks:** ruled out as a strategy (it is the failure mode, not the control);
  reading remains available as spot-checking on top of the instruments, never instead of them.

---

## 11. Carriage table — requirement → covering instrument, traced

Per the A22 lesson (its table asserted coverage a fixture bypassed): every row below names the
instrument that FAILS if the requirement is unmet, and rows whose coverage is human-only say so
plainly rather than borrowing a test's name.

| # | Requirement | Covered by | Fails how, if unmet |
|---|---|---|---|
| R1 | Code branch pinned directly (D1-1) | T1 (`classify_spell_heuristic_code_branch_pins_spelling`) + §10.4 mutation run | branch deleted → `FR_SPELLING_RULE` row reads `Grammar` |
| R2 | Code branch pinned at engine level with real ids (D1-2) | T2 (two assertions in `classify_maps_languagetool_speller_rules_to_spelling`) + §10.4 | same mutation → both read `Grammar` |
| R3 | Shared-fn/harper-duplicate agreement pinned (D1-3) | T3 + §10.4's BOTH-side mutations | either body edited alone → matrix inequality |
| R4 | H28 tests kept, assertions intact (D2) | §10.1 inventory (names present, no rename) + suite green (assertions unedited — the diff to `prompts.rs` in H28's task is comments-only, human-checked per §10.3-3) | test missing → inventory diff; assertion edited → eyes (declared human coverage, not test coverage) |
| R5 | Doc comments tell the verified story (D2-1) | §4.2 checklist, human review (§10.3-3) | **human-only** — no test reads prose; the checklist is the reviewable artifact |
| R6 | Route-B kind fall-through pinned (D2-2) | T4 + §10.4 mutation (widen Row 2's guard) | mutation leaves the `None`-highlight neighbor green and T4 red — the discrimination proof |
| R7 | Behavioral tail filed, not fixed (D3) | A25 exists at `c93cc80` (verified); §10.5 premise audit shows no production string changed | a "fix" would trip the region check / enumerated-touch list |
| R8 | All 97 non-excluded join sites swept AND delegated to the seam (D4; census §1.1) | §10.2 residue oracle (disappearance + exclusions) + §10.2 delegation ledger (per-file `S_tip − S_base == E(f)`, census-derived) + §10.3-1 read triggers (guaranteed, shape-triggered) and §10.3-2 sample (probabilistic — per §10.3's two-strengths statement) for the stated residual (site uses what it binds; F-vs-D) | unswept site → extra grep hit; deleted or non-seam rewrite → that file's ledger delta short by one; a compensating pair's halves trip pure-hunk read triggers when they land in separate hunks (the common case) — coalesced-adjacent halves fall to the sample and per-task review, per §10.2's limit (a) and R12's residual |
| R9 | Exclusions untouched (D4) | §10.2 oracle (survivor list exact) + §10.2 survivor comparison (each of the 13 excluded lines byte-identical between `<pre-sweep>` and `<tip>`) + §10.1 (suite green — `swap.rs:48` breakage would redden swap/session suites) | missing survivor → oracle fails; retained-token modification → its line pair differs byte-for-byte and the comparison fails |
| R10 | Sweep changes nothing: inventory, pass-set, regions | §10.1 (list-diff empty, green at parent and tip, green per commit) + §10.5 region check | any added/lost/renamed test, red run, or out-of-region hunk |
| R11 | D5: three seeds hermetic, one semantic commit, comment rewritten | D5 commit (eyes, §10.3-2) + suite green + §10.2 (those three lines no longer hit the grep) | **largely human** — the three tests' assertions cannot distinguish seed dirs by design (that content-independence is WHY migration is safe); green proves non-breakage, eyes prove intent |
| R12 | No scanner added (D4) — **narrowed to the surfaces the instruments establish**: no scanner as a new test, a new file, CI/build config, a pure-addition hunk, or in the fully-read non-sweep diffs | Each guaranteed surface has the instrument that FAILS when it is unmet: scanner-as-new-test → §10.1 inventory (a test must register a name to run; a new name beyond T1/T3/T4 is a diff); new file / CI / build-script → `git diff --stat` shows an unexpected path; added into an EXISTING test body in a sweep commit → added lines with no paired removal, a pure-addition hunk, §10.3-1 mandatorily read; inside the three non-sweep task diffs → small and fully read (§10.3-3/4/5) | Per-surface as listed. **Residual, stated as a residual and NOT as coverage:** a scanner WOVEN INTO a template-shaped replacement hunk (ample adjacent constructions exist, e.g. `jobs_apply.rs:1000`) preserves the ledger count, the inventory, and green — it is covered probabilistically by the §10.3-2 sample and by per-task review, not guaranteed by any instrument (§10.3's summary says the same) |
| R13 | Archive corrections carried at ship (§12) | ship-time checklist + backlog drift gate (marker↔manifest bijection forces the prose moves) | drift gate reds if prose/manifest disagree; the CONTENT of the correction is human-checked |

Rows R5, R11, R12 and half of R4 are declared human coverage. That is the honest shape:
prose and intent have no mechanical oracle, so the table says "eyes" where it means eyes.

---

## 12. Corrections the archive must carry (ship-time obligations)

When the three items move to `docs/backlog-archive.md` (and `doc =` repoints; `scripts/backlog
bless`):

- **H28's entry** records that its title claim and its "genuinely unreachable once a listing
  lands" mechanism claim were disproven (grounding §1.2: Route A = the pre-listing window is a
  production state with no Enter guard; Route B = a navigated `Other`/`Unknown` highlight
  reaches `Nothing` past a landed listing — the effort-① re-grounding modeled only the default
  `".."` highlight), and that the resolution was keep + re-document + pin, with the wording
  defect filed as A25.
- **H36's entry** records the measured census (§1.1's table, verbatim: 116 hits / 29 files /
  101 joins / 97 swept / 4 excluded joins / 12 bare / 3 comments) in place of the stale "~105
  across ~30" and its per-file estimates — this census ships as the authoritative correction,
  so it is copied from §1.1, never re-derived by hand at ship time.
- **The effort report** carries the process note D2 mandates: second consecutive item whose
  FILED FIX inverted under grounding — a deferred item's stated fix is its least reliable
  content; ground the fix, not just the dependency.

---

## 13. Out of scope / residuals (recorded, not silently dropped)

- **A25** — destination-commit refusals blame the field ("save-as: empty path") when the true
  reason is a non-writable highlighted entry; select mode's per-kind wording has no
  destination-path counterpart. Filed (`c93cc80`); shipped-behavior change; not this batch.
- **An intercept-driven Route-B end-to-end test** (pumped listing over a scratch dir containing
  a real broken symlink, navigate, Enter, assert the warning through the real intercept,
  unix-gated). Grounding offered it as optional; D2 names only the unit pin — omitted here,
  available as a follow-on if the unit pin is ever judged too shallow.
- **The Row-1-cede design question** the old H28 doc comment raised (should a bare Enter on an
  untouched directory highlight with an empty field descend?) — a real behavior question, now
  correctly understood as SHIPPED behavior working as designed; anyone reopening it files an
  item, they do not edit this batch.
- **The 9 remaining bare `temp_dir()` sites** — deliberate survivors (§10.2), inert by
  grounding §1.4; no follow-on filed because no decay exists.
- **`swap.rs:48`** — permanently out of the seam's reach unless someone deliberately designs a
  stable-per-process scratch (e.g. `OnceLock`); not wanted today.
- **Doc-tests / integration-crate reach of the seam** — unchanged; the two harper integration
  sites keep their inline constructions by necessity.
