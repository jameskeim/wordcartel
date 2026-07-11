# Wordcartel — project instructions

Markdown-first Rust terminal word processor. Functional-core / imperative-shell:
`wordcartel-core` (pure, `#![forbid(unsafe_code)]`) + `wordcartel` shell (binary `wcartel`;
ratatui 0.30, crossterm). Top priority: **instant typing, no data loss, no silent UI
waits.** Build toward an in-process Lua plugin system (Effort P, the 1.0 capstone).

**Backlog / progress tracking.** Open and completed work is tracked in **`backlog.toml`** (the
single source of truth for item state) → **`BACKLOG.md`** (a generated dashboard — READ THIS for
status; never hand-edit it). Rich triage prose lives in `docs/ux-backlog.md` +
`docs/engineering-health.md` (OPEN items) and `docs/backlog-archive.md` (shipped/dropped history),
keyed by `<!-- item: ID -->` markers. Status lives ONLY in `backlog.toml`. Drift is a `cargo test`
GATE (`wordcartel/tests/backlog.rs`: schema + full marker↔manifest bijection across all three docs
+ dashboard freshness). **To change an item** (status/size, mark shipped/dropped, add a
dependency): edit its `[[item]]` block in `backlog.toml`, then `scripts/backlog bless`; when an item
ships, move its prose section from the live doc to `docs/backlog-archive.md` (and repoint its `doc =`
field) so the marker bijection stays green. **To capture** a new idea: `scripts/backlog add <ID>
<THEME> "<title>"` (or a `bl:` message — see [[backlog-shorthand-bl]]) files a `triage` item + prose
stub and regenerates, left uncommitted by default. `scripts/backlog {open,shipped}` print filtered
views. Never hand-edit `BACKLOG.md`; never put status words in the prose headings.

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
5. **Two final gates (both must pass).** A Fable whole-branch review (cross-task invariants
   no single task could see; Fable compiles scratch probes against the real branch — reserved
   for THIS gate, NOT run at spec/plan) AND a Codex pre-merge gate (independent GO/NO-GO).
   Re-run after fixes until clean/GO.
6. **Merge** (`superpowers:finishing-a-development-branch`, `--no-ff` to the trunk). Verify
   tests on the merged result; delete the branch. Push only when asked.

### Review layers (the point — different reviewers catch different things)
- **Codex** = external static cross-check. It reads the actual code adversarially and
  catches "spec says X, code is Y," missing accessors, overflow/edge panics, and unsound
  steps. Highest-yield gate and the SOLE spec/plan gate — use it on EVERY spec and plan,
  looping until clean.
- **Per-task reviewer** = focused gate on one diff (spec + quality).
- **Fable whole-branch review** = synthesis across tasks (invariants, regressions,
  data-loss/panic classes), compiling probes against the real branch. Fable gates ONLY here —
  the entire effort before merge — NOT during spec or plan development (decided 2026-07-06, to
  cut per-effort gate latency). Residual duty: when a spec/plan fork leans on math or a claim
  that can't be verified by reading — exactly what Fable's early pass used to backstop — flag
  it to the human explicitly rather than letting it ride to the branch gate.
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

### Command-surface contract (conformance is explicit in every spec & plan)
The **command-surface contract** — `docs/design/command-surface-contract.md` — is the authoritative
App law for how commands, the palette, the menu, and keybinding hints relate (registry = single
source of truth; every user-settable option IS a command; palette exhaustive; menu ⊆ palette; one
shared setter per option that profiles also call; hints track the active keymap and prefer the
user's explicit binding; multi-state options = set-per-state primitives + a cycle; commands are the
Effort-P plugin/automation spine). Any effort that touches commands, user-settable options, the
palette, the menu, or keybinding hints MUST conform: the **spec AND the plan each state how they
honor it** (or explicitly "N/A — does not touch the command surface"), and the contract's invariant
tests (palette-completeness, every-option-has-a-command, hint re-resolution) are merge GATEs.
Amending the contract is a deliberate act recorded in that doc's History — not a silent per-effort
deviation.

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

### Tooling: the rust-analyzer LSP READS, cargo VERIFIES
Split by code state, proven on the splash effort (2026-07-10): the LSP is a fast index of code *at rest*;
`cargo` is the ground truth for code *being changed*. Use each in its lane.
- **LSP for reading a settled tree** — `documentSymbol`/`workspaceSymbol`/`findReferences`/call-hierarchy/
  `hover` for mapping, spec/plan grounding, and review navigation. Accurate and cheap when the tree is quiet
  (the whole-app mapping sweeps used it heavily with zero misses). Warm it once up front — a cold
  `workspaceSymbol` returns "still indexing" for a few seconds, and the mapping phase both warms and uses it.
- **cargo for verifying changed code — NEVER gate on LSP diagnostics.** rust-analyzer re-indexes incrementally
  and asynchronously, so for a few seconds after ANY edit its view lags the file; a `<new-diagnostics>` reminder
  is a point-in-time snapshot that can fire pre-edit or mid-index. Subagent edits are the worst case — the
  controller's analyzer never "witnessed" them, so its diagnostics about subagent-touched files are the most
  stale (splash effort: `intercept`/`paint`/consts flagged "never used," `view_splash` E0063 "missing field" —
  all stale, all disproved by `touch <file> && cargo build`). Treat a diagnostic about recently-edited code as
  "go check," not "it's broken": verify with `cargo build`/`check`/`clippy`/`test` before acting. The
  subagent's own cargo run is authoritative over the controller's stale self-diagnostics — verify before ever
  surfacing one as a finding.
- **Anchor on symbol NAMES, not line numbers.** Spec/plan `:NNN` anchors drift as tasks edit files (recurring
  H1 + splash lesson); implementers/reviewers locate by name/structure. Use `workspaceSymbol`/`documentSymbol`
  to *re-locate* a symbol cheaply mid-effort rather than trusting a recorded line.
- **Tell subagents explicitly:** for compile/usage/signature questions on code they are editing, trust `cargo`
  + `grep`, not an editor "unused"/"undefined" hint.

### Commit/push rules
- Branch per effort off the trunk; never implement on the default branch without consent.
- Commit/push only when explicitly asked.
- Every commit ends with the project trailers, verbatim — the `Co-Authored-By` line below,
  then a `Claude-Session:` line with the current session's URL (provided by the environment):
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
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
- **Module-size anti-regrowth is a GATE.** `clippy::too_many_lines` (threshold 100, `clippy.toml`;
  enabled in `[workspace.lints.clippy]`) and the hub production budgets in
  `wordcartel/tests/module_budgets.rs` both enforce *Module structure* (below). A function over the
  threshold, or a hub over budget, fails the build until it is split — or, for `too_many_lines`, carries
  an item-local `#[allow(clippy::too_many_lines)]` with a one-line reason.

**PTY smoke suite — mandatory-run, advisory-pass (NOT a GATE):** every effort's pre-merge
report MUST run `scripts/smoke/run.sh` and quote its one-line summary verbatim (e.g.
`smoke: 8/8 PASS`). A red result NEVER blocks a merge — it is an advisory finding that
must be surfaced to the human explicitly (e.g. `smoke: FAIL s5 — advisory`). `cargo test`
+ workspace clippy remain the only merge gates. The suite drives the real `wcartel`
binary in a private per-run tmux server (`scripts/smoke/`, checks S1–S8); a skip on a
tmux-less machine (`smoke: SKIP — …`) is quoted the same way. Promotion to a gate later
is an edit to this paragraph, contingent on the stability record accumulating in the
gitignored `scripts/smoke/.history`.

**`cargo deny check` — RELEASE-CHECKLIST step, NOT a merge GATE.** Supply-chain scan (CVE
advisories, license policy, duplicate-version drift, registry sources) config'd in `deny.toml`
(workspace root; H18). Run it before cutting a release and record the result (clean, or the
advisory/license findings) in the release notes — a red result never blocks a merge. Promotion to
a GATE is a separate, deliberate edit to this paragraph (mirrors the PTY-smoke-suite pattern
above).

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

**Module structure — dispatchers delegate, they don't implement (anti-regrowth).** The H1 debt
(`app.rs`/`render.rs` god-objects, decomposed 2026-07-09) came from *dispatch attractors*: a central
`match`/loop that every feature had to edit, so it grew monotonically with feature count — and prior
*leaf* extraction didn't stop it because new wiring still landed in the hub. Prevent the recurrence:
a `match` arm or loop iteration is a **thin delegation** into a domain module — or a **row in a data
table** — never an inline body. New behavior enters through a **registration seam** (a `timers.rs`
`SUBSYSTEMS` row; a feature-module `intercept`/handler fn; an exhaustive-enum variant the compiler
forces you to place), NOT by growing the dispatcher. This is Open–Closed in Rust: the hub is closed to
editing, open to new modules. **Effort P conforms** — plugin message-arms/hooks register into the seam;
they must not add bulk to `reduce`/`run`. Enforced by the two GATEs above (`clippy::too_many_lines` on
the god-*function*, the `module_budgets` test on the god-*file*). Counter-caveat: do NOT over-fragment
in reaction — the target is *"one person can hold this module's single responsibility in their head"*
(one axis of change per module), not maximal splitting; a tangle of 40-line files with cross-imports is
its own pathology. A long function is fine when it is a genuinely flat, cohesive dispatch (a
table-builder, an exhaustive command `match`) — mark it with a reasoned `#[allow(clippy::too_many_lines)]`.

**Concurrency:** heavy/slow work goes off the hot path onto the `std::thread` + `mpsc` job
substrate (worker results are `Send`, version-discarded on staleness). Never block the input
loop.

**Resource behavior — proportional to work, free at rest.** A well-behaved editor spends
resources in proportion to what the user is doing and ~nothing at rest. **Idle is free:** with
no input and nothing animating, the input loop BLOCKS — no CPU spin, no polling, no background
disk writes. Background/periodic work (swap, reconcile, diagnostics, autosave) is **edge-triggered
by a real content/state change, never level-triggered off wall-clock or a monotonic timestamp**
(the class behind the 2026-07-08 swap-thrash bug — a swap file rewritten every idle wake because
`due` keyed off a never-cleared `last_edit_at`). **Disk writes track saves/edits, never idle
duration** (a settled buffer writes its recovery swap at most once per edit-version — SSD
endurance). **Memory** ≈ fixed baseline + `O(content held)` + bounded undo, released on buffer
close, capped (M5) — not 1:1 with file size. Back these with guardrail tests where practical (the
swap SSD-wear guardrails are the first instance): idle/settled states asserted to do no background
work.

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
follow-ups — finish the undo louder-hint for buffer-level merges. (The document-sized
`fs::read` load paths flagged earlier — recovery content-hash, fingerprint, save
skip-unchanged, and the swap read — are already capped via `bounded_read_opt`/
`read_swap_capped`; the personal-dictionary read is now capped too, so **no document-class
unbounded `fs::read` remains**. Small config/theme reads — `config.rs`, startup
overrides/mask in `app.rs`, `theme_resolve.rs` — are deliberately unbounded config-class
files, out of scope.) (3) optional — actually upgrade/patch `pulldown-cmark` (M4-rest only
*isolates* its parse panic). **e2e/TUI frontier — now covered by two layers:** in-process
journeys drive the real `reduce → advance → render` loop against a `TestBackend`
(`wordcartel/src/e2e.rs`); the PTY smoke suite (`scripts/smoke/`, mandatory-run /
advisory-pass — see the Rust-conventions note above) drives the live `wcartel` binary in
a private tmux server: startup/exit codes, open errors, real-terminal save, dirty-quit
modal, OSC 52 → tmux buffer, tiny-terminal guard, panic → restore → recovery dump, and
hard-kill → swap → recovery.
Deferred product items: clipboard over SSH/tmux; the close-buffer Save/Discard prompt.
