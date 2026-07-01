# Wordcartel — project instructions

Markdown-first Rust terminal word processor. Functional-core / imperative-shell:
`wordcartel-core` (pure, `#![forbid(unsafe_code)]`) + `wordcartel` shell (binary `wcartel`;
ratatui 0.30, crossterm). Top priority: **instant typing, no data loss, no silent UI
waits.** Build toward an in-process Lua plugin system (Effort P, the 1.0 capstone).

---

## Development process: gated, review-driven, subagent-executed

Use this pipeline for any non-trivial feature, refactor, or hardening work. Every artifact
is reviewed by an INDEPENDENT perspective before it advances; nothing is trusted until
checked against the REAL code. Work effort-by-effort on a branch off the trunk; commit/push
ONLY when explicitly asked.

### The pipeline (per effort)
1. **Brainstorm** (`superpowers:brainstorming`). First map the real code surface (a
   read-only Explore agent for exact files/signatures). Then resolve design forks ONE
   question at a time, plain-text A/B/C options with a recommendation — never the picker
   UI. Get section-by-section approval → an approved design.
2. **Spec → Codex spec review → re-run until clean.** Write the design doc, then dispatch a
   Codex review that cross-checks every claim AGAINST THE REAL SOURCE (not the diff; do not
   run cargo). Fold findings; re-run Codex until it returns no Critical/Important/Minor.
   Treat "not ready" verdicts as blocking.
3. **Plan** (`superpowers:writing-plans`) **→ Codex plan review → re-run until clean.**
   Task-by-task, COMPLETE code, TDD steps, grounded in real signatures. Codex re-checks
   snippets, line anchors, and migration sites.
4. **Execute** (`superpowers:subagent-driven-development`). Fresh implementer subagent PER
   TASK (TDD: failing test → impl → green → commit), then a per-task reviewer subagent
   returning TWO verdicts — spec compliance AND code quality. Critical/Important findings →
   fix subagent → re-review; Minor → record in the ledger for the final pass.
5. **Two final gates (both must pass).** An opus whole-branch review (cross-task invariants
   no single task could see) AND a Codex pre-merge gate (independent GO/NO-GO). Re-run after
   fixes until clean/GO.
6. **Merge** (`superpowers:finishing-a-development-branch`, `--no-ff` to the trunk). Verify
   tests on the merged result; delete the branch. Push only when asked.

### Review layers (the point — different reviewers catch different things)
- **Codex** = external static cross-check. It reads the actual code adversarially and
  catches "spec says X, code is Y," missing accessors, overflow/edge panics, and unsound
  steps. Highest-yield gate — use it on EVERY spec and plan.
- **Per-task reviewer** = focused gate on one diff (spec + quality).
- **Opus whole-branch review** = synthesis across tasks (invariants, regressions,
  data-loss/panic classes). Use the most capable model here.
- Re-running until clean is mandatory: each round the reviewer reads the REVISED code, so a
  fix that introduces a new problem is caught.

### Findings discipline
- Triage Critical / Important / Minor. Critical+Important → dispatch a fix subagent (one
  fixer for the final-review findings LIST, not per-finding). Minor → record in the ledger;
  let the final review triage them.
- A finding that contradicts the plan, or any design change, is the HUMAN's decision —
  present the finding beside the plan text and ask which governs. Do not silently apply a
  fix that changes an approved design.
- Never tell a reviewer what not to flag or pre-rate a severity.

### Machinery (keeps the controller's context clean)
- The controller curates exactly what each subagent needs and hands artifacts as FILES
  (`scripts/task-brief`, `scripts/review-package`, per-task report files) — never paste bulk
  diffs/reports into dispatch prompts.
- A dispatch prompt describes ONE task + the interfaces it touches + global constraints. No
  session history.
- Choose the cheapest model that fits each role: cheap for transcription tasks, standard for
  integration, most-capable for whole-branch reviews and the hardest implementers. Always
  set the model explicitly.
- Track progress in a durable LEDGER (`$(git rev-parse --git-path sdd)/progress.md`): one
  line per completed task with its commit range. After any compaction, trust the ledger +
  `git log` over recollection; never re-dispatch a task it marks complete.
- Save non-obvious decisions to memory (why, not what).

### Commit/push rules
- Branch per effort off the trunk; never implement on the default branch without consent.
- Commit/push only when explicitly asked.
- Every commit ends with the project trailers, verbatim — the `Co-Authored-By` line below,
  then a `Claude-Session:` line with the current session's URL (provided by the environment):
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: <current session URL>
  ```

---

## Rust conventions

Style and quality standards for all code in this repo. Items marked **GATE** must pass
before merge.

**GATEs (before any merge):**
- `cargo test` green across all suites (`wordcartel-core` lib + oracle, `wordcartel` lib).
- `cargo build` and `cargo test --no-run` warning-free for the crate(s) you touched.
- **Workspace clippy clean is a GATE.** The clippy-debt cleanup (2026-06-30) cleared all `clippy::all` warnings and enabled `[workspace.lints.clippy] all = "deny"`. `cargo clippy --workspace --all-targets` MUST pass clean before merge. New warnings fail the clippy run; deliberate exceptions require an item-local `#[allow(clippy::…)]` with a one-line rationale (never a blanket crate/workspace allow).
- New code matches the surrounding **house style** (see below) by review.

**Formatting — do NOT run `cargo fmt`.** This repo is hand-formatted in a deliberate dense
style and has **no `rustfmt.toml`**; `cargo fmt` reformats the whole tree to rustfmt
defaults (1000+ hunks), destroying the intentional style and blame. `cargo fmt --check` is
therefore NOT a gate. Match the neighbors by hand instead: do not reflow code you did not
otherwise change, and do not let an editor/agent auto-run rustfmt across files.

**Style (house, not rustfmt-default):** snake_case fns/vars/modules, PascalCase
types/traits, SCREAMING_SNAKE_CASE consts; 4-space indent; ~100-char lines but hand-wrapped
with judgment (single-line struct literals / match arms are kept inline where they read
better — do not explode them); imports grouped logically by hand (not rustfmt-reordered);
`—` (em-dash) in prose comments, never `--`. No emoji or emoji-like unicode in code — the
only exception is tests exercising multibyte text (we test with `é` / `中` / `🙂`).

**Types & errors:** make struct fields private by default; expose accessors / validated
constructors (see `ChangeSet`/`Selection`). Use newtypes for semantically distinct values
of the same primitive (`BytePos`, `BufferId`). Prefer `Option<T>` over sentinels. Errors are
typed enums (`SaveError`, `OpenError`, `EditError`) surfaced to the **status line** (no
silent UI) — never to the console, since the app owns the terminal.

**Unwrap:** no `.unwrap()` on fallible/external paths. A guarded unwrap (immediately after
establishing the invariant) is acceptable, but prefer `.expect("…invariant…")` with a message.

**Performance:** optimize the HOT path — per-keystroke work stays `O(visible)+O(edited)`,
never `O(document)`; cold paths favor clarity. Avoid needless allocation (`&str` over
`String`, `Cow<'_, str>` when ownership is conditional, `Vec::with_capacity` when size is
known). DRY; no dead code; no technical debt.

**Concurrency:** heavy/slow work goes off the hot path onto the `std::thread` + `mpsc` job
substrate (worker results are `Send`, version-discarded on staleness). Never block the input
loop.

**Pattern matching:** prefer exhaustive matches; avoid catch-all `_` where it would silently
absorb a new variant (the theme `SemanticElement` constructors use exhaustive literals on
purpose).

**Docs:** doc-comment every public item; document params/returns/errors; add a runnable
`# Examples` block for non-obvious public functions.

**Tests:** unit-test new functions/types; Arrange-Act-Assert; `#[cfg(test)]` modules,
`use super::*` allowed. Mock the filesystem via the `Fs` seam (M3) for durability/fault
tests. No committed commented-out tests, `dbg!`, debug `println!`, or commented-out code.

## Hardening campaign (minimal-viable spine COMPLETE, before Effort P)
Plan: `docs/superpowers/plans/2026-06-28-wordcartel-hardening-fuzz-proptest-plan.md`
(the real pre-plugin risk is shell durability/panics/untrusted input, not more core
fuzzing). **Done (all merged to main):** M1 (valid-by-construction), M2 (untrusted edit
boundary `submit_transaction`), M3 (IO fault injection), M5 (resource caps), M4 + M4-rest
(async/untrusted panic isolation + parse-panic isolation + input-thread supervision), M7
(core property tests + cargo-fuzz; the F2 oracle found + fixed 7 real incremental-parser
bugs), M6/BUG-2 (external-mod fingerprint), BUG-1 (worker panic isolation), plus the
clippy-debt cleanup (zero `clippy::all` + the durable `[workspace.lints.clippy] all = "deny"`
gate — see the Formatting/GATE notes above).

**Remaining before Effort P (none blocking; sequence by yield):** (1) deep
incremental-soundness — the F2 fuzz oracle still finds more block-tree `incremental≡full`
divergences in the tail (nested/loose-list; wrong-tree, not data-loss/panic); (2) M5
follow-ups — finish the undo louder-hint for buffer-level merges, and bound the last few
document-sized `fs::read` load paths (recovery content-hash, fingerprint, save
skip-unchanged); (3) optional — actually upgrade/patch `pulldown-cmark` (M4-rest only
*isolates* its parse panic). **New candidate:** an e2e/TUI harness (the live `wcartel`
binary has no end-to-end/interactive test coverage — the campaign's one untouched frontier).
Deferred product items: clipboard over SSH/tmux; the close-buffer Save/Discard prompt.
