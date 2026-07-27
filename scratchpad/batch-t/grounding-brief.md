# Batch T — grounding brief

Tree: `main` at `6d3a213`, clean except untracked `scratchpad/`.

Batch T is **H38 + H28 + H36**, sequenced #2 in `docs/design/backlog-sequence.md` on the
rationale: *"Fix the measuring instrument before taking more measurements. Eleven could-not-fail
tests surfaced in E11 alone and two more are filed. Nothing in this batch can change shipped
behavior, so the ~105-site sweep is safe volume."*

This file is **FACTS ONLY** — filed text, verified code locations, and measurements taken at
`6d3a213`. It contains no recommendation. **Verify every claim against the real source.** §6 lists
places where the controller's sweep already found the FILINGS to be wrong or incomplete — including
one that contradicts the batch's own sequencing rationale.

---

## 1. The three items as filed

`scripts/backlog open` rows; prose in `docs/ux-backlog.md` under `<!-- item: H38|H28|H36 -->`.

### H38 — `classify_spell_heuristic`'s spell-substring branch is pinned by no test
Status `triage`, kind `debt`, size `S`.

Filed: a PRE-EXISTING coverage hole surfaced 2026-07-26 by the vale-ls removal review, **not caused
by it**. `lsp_client::classify_spell_heuristic` has a branch testing the rule code for a `spell`
substring; mutating it reddens no test, and did not before the removal either. It LOOKED covered:
the deleted `vale_ls.rs` test `classify_spelling_checks_by_name_else_heuristic` appeared to exercise
it, but `ValeEngine::classify` short-circuited on its own `"Spelling"` check and never reached the
shared branch — the test passed for a reason unrelated to its name.

Filed fix shape: *"a fixture whose engine classify returns undecided for a code containing `spell`,
asserted Spelling, with the kill condition stated (deleting the branch must redden it)."*

Anchors: `lsp_client::{classify_spell_heuristic, LspEngine::classify}`.

### H28 — Un-pumped picker tests assert unreachable states
Status `triage`, size TBD.

Filed: `save_as_empty_path_is_a_sticky_warning` and its Write-Block twin pass only because they act
on the picker **before pumping the async directory listing**. Once a listing lands on any non-root
directory the warning they assert becomes unreachable, so they assert a state real usage never
reaches.

**The filing carries a 2026-07-19 re-grounding (effort ①, which DEFERRED it) that changes what the
item is.** Its three findings, verbatim in substance:
1. Of the 20 tests that press Enter at a Destination picker, **18 already pump** and all 20 drive
   the real intercept. A two-test remainder, not a systemic gap.
2. Pumping was already tried on these two **and reverted** — their doc comments say so.
3. The mechanism is verified: before a listing lands `entries` is empty → `highlighted` is `None` →
   commit falls to `Nothing` → the warning fires. After `apply_listing_done`, `rederive` puts `".."`
   at `entries[0]`, `selected` stays 0, Row 1's guard becomes true off `trimmed.is_empty()` →
   `Descend`.

And its governing question, verbatim:

> **So the real question is behavioural, not test hygiene: is that warning reachable in production
> at all?** If it is not, the tests are asserting a state no writer can reach and should be retired
> along with the dead branch — not "made to pump." **Making them pump would delete the assertion
> while appearing to fix it**, which is the exact defect class both efforts spent their review
> rounds catching (eight instances in effort ① alone).

### H36 — Sweep the ~105 inline `temp_dir()` scratch-path constructions onto the `test_support` seam
Status `triage`, size TBD. The explicit follow-on to H32 (shipped `636f036`).

Filed: H32 consolidated the 15 named scratch-path *helpers* onto `test_support::{scratch_path,
scratch_dir}` and deliberately left *"~105 inline `temp_dir()` scratch-path constructions across ~30
files"* (heaviest: `file_browser.rs` ~17, `app.rs` ~15, `prompts.rs` ~14, `jobs_apply.rs` ~10,
`swap.rs` ~10, `render_overlays.rs` ~7).

Deferred out of H32 deliberately: *"the ~105 sites are individually correct today (pid-unique, one
label per test — not the H31 collision class), so they have no decay; sweeping them is ~30 files of
mechanical churn whose review blast radius dwarfs the value on a no-decay item."*

Two constraints the filing states:
- **Do NOT answer this with a textual scanner** banning raw `temp_dir().join(...)` — same reasoning
  as H32 (H31 fork 3 / effort ① D5): the `fs_chokepoint` scanner was measured to leave 5 of 6
  evasion routes uncaught; a second scanner is self-defeating. The work is mechanical delegation
  onto the seam, not a new gate.
- Two integration-test sites (`tests/harper_ls_probe.rs`, `tests/harper_ls_integration.rs`) are a
  **separate crate** and cannot reach a `#[cfg(test)] pub(crate)` seam at all — out of scope.

---

## 2. Verified code surface — H38

`wordcartel/src/lsp_client.rs`

```rust
/// The shared spelling-vs-grammar fallback heuristic (E10 §7/§8): a lowercase "spell"
/// substring across code/source/message. Engine classifiers try their own rule-id tables
/// first and fall through to this. (harper's `classify_lsp` keeps its original private copy —
/// the T1 pin; the two bodies are intentionally identical.)
pub(crate) fn classify_spell_heuristic(d: &Value) -> DiagnosticKind {
    let code = match d.get("code") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    if code.to_lowercase().contains("spell") { return DiagnosticKind::Spelling; }   // <-- the unpinned branch
    let source = d.get("source").and_then(|s| s.as_str()).unwrap_or("");
    let message = d.get("message").and_then(|m| m.as_str()).unwrap_or("");
    if format!("{source} {message}").to_lowercase().contains("spell") {
        DiagnosticKind::Spelling
    } else {
        DiagnosticKind::Grammar
    }
}
```

**Callers — exactly one in the whole workspace** (`grep -rn classify_spell_heuristic --include="*.rs" .`):
`ltex_ls.rs:90`, the tail of `LtexEngine::classify`.

`LtexEngine::classify` (`ltex_ls.rs:83-91`), verbatim:

```rust
fn classify(d: &Value) -> DiagnosticKind {
    if let Some(code) = d.get("code").and_then(|c| c.as_str()) {
        let up = code.to_uppercase();
        if up.contains("MORFOLOGIK") || up.contains("HUNSPELL") || up.contains("SPELLER") {
            return DiagnosticKind::Spelling;
        }
    }
    crate::lsp_client::classify_spell_heuristic(d)
}
```

`LspEngine` impls in the workspace (`grep -rn "impl LspEngine for"`): `LtexEngine` (`ltex_ls.rs:22`),
`TestEngine` (`lsp_client.rs:1147`), `TestEngineAllRaws` (`lsp_client.rs:1772`). Both test engines
define `fn classify(_d: &Value) -> DiagnosticKind { DiagnosticKind::Grammar }` — they ignore the
argument and never reach the shared fn.

`HarperEngine::classify` (`harper_ls.rs:107`) is `classify_lsp(d)` — the private duplicate at
`harper_ls.rs:158`. That duplicate **is pinned**, by `classify_lsp_spelling_vs_grammar`
(`harper_ls.rs:529-533`):

```rust
assert_eq!(classify_lsp(&json!({"code":"SpellCheck","message":"x"})), DiagnosticKind::Spelling);
assert_eq!(classify_lsp(&json!({"code":"LongSentences","message":"x"})), DiagnosticKind::Grammar);
assert_eq!(classify_lsp(&json!({"message":"possible spelling mistake"})), DiagnosticKind::Spelling);
assert_eq!(classify_lsp(&json!({"message":"style"})), DiagnosticKind::Grammar);
```

So the code-substring branch is pinned **on the duplicate** and unpinned **on the shared original**.
The doc comment calls the two bodies "intentionally identical" — no test asserts that they are.

## 3. Verified code surface — H28

### 3.1 The two tests
`wordcartel/src/prompts.rs:496` `save_as_empty_path_is_a_sticky_warning` and `:513`
`block_write_empty_path_is_a_sticky_warning`. Both: `Editor::new_from_text`,
`open_destination_picker(..., std::env::temp_dir(), "   ".into())`, one `press_key_fb(Enter)`, then
assert `status_text()`, `kind() == Warning`, `lifetime() == Sticky`. Neither pumps.

`save_as_…` carries a 9-line doc comment recording the reverted pump experiment verbatim:
*"Confirmed live (pump added, ran, status came back empty instead of 'save-as: empty path';
reverted) — reported as a FINDING in the task report, not fixed here: whether Row 1 should ever cede
to Row 2 on an untouched directory highlight with an empty field is a design question, not a
mechanical one."* The Write-Block twin's comment says it breaks identically if pumped.

### 3.2 The decision table
`file_browser_commit::classify_destination_enter(fs, dir, field, highlighted, highlight_navigated)`
(`:77`). Rows 1 and 2 are what matter:

```rust
let trimmed = field.trim();

// Row 1 — a highlighted directory descends, EVEN with a non-empty field ...
if let Some(e) = highlighted {
    if matches!(e.kind, EntryKind::Dir) && (highlight_navigated || trimmed.is_empty()) {
        let target = if e.name == ".." { dir.parent()... } else { dir.join(&e.name) };
        return CommitOutcome::Descend(target);
    }
}

// Row 2 — an empty field commits onto the highlighted FILE ...
if trimmed.is_empty() {
    return match highlighted {
        Some(e) if matches!(e.kind, EntryKind::File) => {
            CommitOutcome::Commit { path: dir.join(&e.name), from_highlight: true }
        }
        // Other/Unknown are refused in select mode and are not commit targets here
        // either — we do not know they are writable regular files.
        _ => CommitOutcome::Nothing,
    };
}
```

`CommitOutcome::Nothing` is what produces the empty-path warning (`file_browser.rs:256` returns
`None`; the caller sets the Sticky Warning).

**So `Nothing` is reached with an empty field whenever `highlighted` is `None` OR is an entry whose
kind is neither `Dir` (passing Row 1) nor `File`.**

### 3.3 `EntryKind` is a four-variant enum with two non-Dir/File variants, both produced in production
From `fsx.rs`:
- `:275` — `if is_file { File } else if is_dir { Dir } else { Other }` (a fifo/socket/device/door).
- `:242` — `Err(_) => (EntryKind::Unknown, false, false)` (stat failure).
- `:268` — `Err(_) => (EntryKind::Unknown, true, true)` (broken symlink;
  `file_browser_listing.rs:233` builds `kind: Unknown, is_symlink: true, broken: true`).

`recents.rs:142,145` **deliberately** sets `entry.kind = EntryKind::Unknown` for unavailable rows so
that Enter refuses them; `editor.rs:1083-1098` documents the same. `file_browser.rs:174-175` gives
them glyphs (`%` / `?`); `file_browser.rs:212-216` gives them per-kind `EnterOutcome::Refuse`
messages **in select mode**.

## 4. Verified measurements — H36

Counted at `6d3a213` with `grep -rn "temp_dir()" --include="*.rs"` over `wordcartel/src/`,
`wordcartel-core/src/`, `wordcartel/tests/`.

**Total: 116 sites across 28 files** (filed as "~105 across ~30").

Per-file, descending: `prompts.rs` 20 · `file_browser.rs` 17 · `app.rs` 11 · `jobs_apply.rs` 9 ·
`render_overlays.rs` 7 · `swap.rs` 6 · `workspace.rs` 5 · `file_browser_commit.rs` 5 ·
`session_restore.rs` 4 · `mouse.rs` 4 · `editor.rs` 3 · `timers.rs` 2 · `save.rs` 2 · `render.rs` 2 ·
`recovery.rs` 2 · `file_browser_listing.rs` 2 · `export.rs` 2 · `diagnostics_run.rs` 2 ·
`state.rs` 1 · `search_ui.rs` 1 · `recents.rs` 1 · `fsx.rs` 1 · `e2e.rs` 1 · `config.rs` 1 ·
`clipboard.rs` 1 · `chrome_geom.rs` 1 · `test_support.rs` 1 (the seam's own impl) ·
`tests/harper_ls_probe.rs` 1 + `tests/harper_ls_integration.rs` 1 (separate crate, filed out of scope).

**The population is not homogeneous.** Splitting it:
- **96** are `temp_dir().join(...)` — path CONSTRUCTIONS, the shape the filing describes.
- **17** are a bare `temp_dir()` with no `.join` — passed as *an existing directory to operate on*,
  not a scratch path. Examples: `file_browser.rs:732` `open_file_browser(..., std::env::temp_dir())`;
  `file_browser.rs:1086,1101` `dir: std::env::temp_dir()` in a `FileBrowser` literal;
  `file_browser.rs:1120` `apply_listing_done(&mut e, 12345, std::env::temp_dir(), Ok(l))`;
  `save.rs:1265`; `prompts.rs:502,522,714,794,818`.
- The remaining 3 are the `test_support.rs` implementation line and the 2 out-of-scope
  integration-test sites.

### 4.1 The seam, verbatim
```rust
static SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);

fn scratch_name(label: &str) -> PathBuf {
    let seq = SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("wc-scratch-{}-{seq}-{label}", std::process::id()))
}

/// A unique scratch PATH under the system temp dir — NOT created.
pub(crate) fn scratch_path(label: &str) -> PathBuf { scratch_name(label) }

/// A unique scratch DIRECTORY under the system temp dir — created, and empty by
/// construction: the pid+seq pair has never been issued before, so no prior content
/// can exist (this subsumes every legacy remove-then-create dance).
pub(crate) fn scratch_dir(label: &str) -> PathBuf {
    let d = scratch_name(label);
    std::fs::create_dir_all(&d).expect("scratch_dir: create");
    d
}
```
Guardrail: `test_support.rs:502` `scratch_seam_is_collision_free_under_contention` — THREADS × PER
calls across threads, asserts every returned path is distinct, `scratch_path` does not exist, and
`scratch_dir` is an empty directory.

**Note the semantic difference the sweep must respect:** `scratch_dir` returns a directory that is
**empty by construction**; a bare `temp_dir()` returns the **shared system temp dir**, which is
non-empty, has a parent (so its listing carries a `".."` row), and whose contents are controlled by
other processes on the machine.

---

## 5. Process constraints

- Project law: `CLAUDE.md` — gated review-driven pipeline; Rust house style; GATEs.
- Command-surface contract `docs/design/command-surface-contract.md`: the spec AND the plan must
  each state how they honor it, or state "N/A — does not touch the command surface."
- GATEs: `cargo test` green; `cargo build` + `cargo test --no-run` warning-free for touched crates;
  `cargo clippy --workspace --all-targets` clean; `clippy::too_many_lines` (100) and
  `wordcartel/tests/module_budgets.rs` hub budgets.
- **`cargo fmt` is FORBIDDEN** (hand-formatted repo, no `rustfmt.toml`). Match neighbours by hand.
- PTY smoke `scripts/smoke/run.sh` — mandatory-run, advisory-pass.
- Backlog bookkeeping: status lives ONLY in `backlog.toml`; `scripts/backlog bless` after edits; on
  ship, move prose to `docs/backlog-archive.md` and repoint `doc =`.
- Commit/push only when explicitly asked.

## 6. Claims that did NOT survive the controller's first check

Stated so they are re-verified, not inherited. **Verify each independently — the controller's
reasoning below may itself be wrong.**

1. **H36's site count and homogeneity.** 116 sites, not ~105; 28 files, not ~30. More importantly
   the population appears to be **two populations** (§4): ~96 scratch-path constructions and ~17
   bare `temp_dir()` uses where the directory itself is the subject. Mechanically sweeping the
   second group onto `scratch_dir()` would substitute an **empty, freshly-created** directory for
   the **shared, populated** system temp dir — a behavioural change in the test, not a delegation.

2. **H36's "no decay" premise may be inverted for the bare-`temp_dir()` group.** Those ~17 tests
   read a directory whose contents other processes control. `prompts.rs:717` carries a comment
   about gating "regardless of whatever `temp_dir()` happens to sort first" — i.e. a test already
   defending itself against content it does not own. If that is a real fragility class, then the
   group the filing calls out of scope has decay and the group it calls in scope does not. This
   directly bears on the batch's sequencing rationale (*"the ~105-site sweep is safe volume"*).

3. **H36 and H28 collide on the same lines.** The two H28 tests (`prompts.rs:502,522`) are members
   of the bare-`temp_dir()` group, and their doc comments state that the shared temp dir's `".."`
   row is exactly what makes the post-pump behaviour differ. A naive H36 sweep would rewrite the
   lines H28 is deliberating over.

4. **H28's "unreachable in production" premise looks false, by two independent routes** — which
   would invert the item's stated fix ("retire the tests and the dead branch"):
   - **The pre-listing window is a production state.** The picker's listing is async by design
     (`editor.rs` `open_destination_picker` → `file_browser::start_listing`; its doc comment says a
     synchronous refetch would block the input loop). A writer who presses Enter between opening the
     picker and the listing landing is in exactly the state the tests construct. Rare on a local
     disk; not rare on a slow or network filesystem.
   - **A highlighted `Other`/`Unknown` entry reaches `Nothing` with a fully-landed listing.** Per
     §3.2/§3.3, Row 1 requires `Dir` and Row 2 commits only on `File`; every other kind falls to
     `_ => CommitOutcome::Nothing`. `Other` and `Unknown` are produced by `fsx.rs` in production for
     fifos/sockets/devices, broken symlinks, and stat failures, and `recents.rs` sets `Unknown`
     deliberately.
   If the second route holds, there is a further observation the controller could not settle: in
   that state the emitted status is *"save-as: empty path"* — but the field being empty is not why
   the commit refused. The per-kind refusal messages that select mode already has
   (`file_browser.rs:212-216`) have no counterpart on the destination-commit path. Whether that is a
   defect, a separate filing, or nothing is a judgment, not a code fact.

5. **H38's branch may be dead for its only real caller — or may not be.** With vale-ls removed,
   `classify_spell_heuristic` has exactly one production caller, `LtexEngine::classify`, which
   short-circuits first on `MORFOLOGIK`/`HUNSPELL`/`SPELLER` (uppercased). Whether a real ltex
   diagnostic code can contain `spell` yet miss all three of those substrings is an
   engine-behaviour question the controller did not settle. It bears directly on whether the right
   fix is *pin the branch* or *delete the branch*, and on whether a `TestEngine`-based fixture pins
   a path any production input can traverse.

## 7. What is wanted back

Grounding verification of §2–§4 and §6 against the real source, then scoping:

- Whether Batch T is one effort or should be resequenced/resized — the human's stated preference is
  to keep related scope **whole** in one effort, but say so plainly if the grounding argues
  otherwise.
- For each of H38, H28, H36: the resolution options with their real blast radius.
- The forks the human must decide, one at a time, plain-text A/B/C with a recommendation.
- Anything the controller's sweep missed.

Options for human consideration — not a chosen answer.
