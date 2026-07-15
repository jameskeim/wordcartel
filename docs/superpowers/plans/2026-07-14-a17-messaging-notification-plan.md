# A17 — Messaging / notification system: implementation plan

<!-- item: A17 -->

Spec: `docs/superpowers/specs/2026-07-14-a17-messaging-notification-design.md` (Codex-clean, 3 rounds).
Branch: `effort-a17-messaging-notification`. Author: Fable. Date: 2026-07-14.

Execution model: fresh implementer subagent PER TASK, TDD (failing test → run-fails → minimal impl →
run-passes → commit). Anchor on symbol NAMES, not line numbers (they drift; re-locate via grep /
`documentSymbol`). For compile/usage questions on code you are editing, trust `cargo`/`grep`, never an
editor "unused/undefined" hint.

---

## Global Constraints (every task honors these)

**Project laws (from `CLAUDE.md`):**
- **No silent UI waits.** Every user-facing message routes to the status line (the app owns the
  terminal — no console). Typed errors surface to the status line.
- **Idle is free / edge-triggered.** Background/periodic work is edge-triggered by a real
  content/state change, NEVER level-triggered off a wall-clock. A17 adds **no timer** in v1; the ring
  and slot are written only on an emit (an edit-class event). The input loop still BLOCKS at rest.
- **Core is `#![forbid(unsafe_code)]`.** A17 is **shell-only** (`wordcartel/src/`). Do NOT add A17
  types or a `ReadOnly` variant to `wordcartel-core`.
- **Dependency weight is an active constraint.** Zero new crates — a `VecDeque` ring, enums, a pure
  function. No `tracing`, no framework.
- **Hot path stays `O(visible)+O(edited)`.** `set_status`/`finish_topic` are O(1); the read-only guard
  is one bool compare; `paint_status` gains one `match kind`.

**Locked decisions (LAW — do not re-litigate; verbatim from the spec §1):**
- **F1** — the idle-is-free law means "no LEVEL-triggered clocks," not "no timers." One-shot armed dwell
  deadlines are permitted. A17 v1 builds no timer.
- **Q1** — severity-ranked slot occupancy: a new message takes the visible slot iff its severity ≥ the
  current occupant's; every message appends to history regardless of display outcome. Error+Warning
  survive the next-keypress clear until dismissed/superseded.
- **Q2** — Progress is NOT a fifth kind. `StatusKind` stays exactly LSP's four (Error/Warning/Info/Log).
  Progress is an orthogonal self-replacing LIFETIME flag, superseded by its own completion/failure,
  collapsed in history.
- **Q3** — Info/Log clear on next input; Warning+Error hold until dismissed/superseded. NO new timer.
- **Q4** — `view_messages` opens a read-only scratch-style buffer (no 12th bespoke overlay; the ring is
  the source of truth; a later overlay/lens view stays open post-H21).
- **Q5** — keep `wc.status(msg)` working (routes to `set_status`, plugin-tagged source, Info default,
  4096-byte cap); ADD `wc.notify(severity, msg)` severity-explicit; emit-side rate-limit + dedup.
- **Q6** — ship exactly ONE option `messages_min_kind` (verbosity floor: ≥Info vs ≥Warning), full
  command-surface treatment. NOT the fuller per-kind matrix.

**GATEs (must pass before merge):**
- `cargo test` green across `wordcartel-core` (lib + oracle) and `wordcartel` (lib).
- `cargo build` and `cargo test --no-run` warning-free for touched crates.
- `cargo clippy --workspace --all-targets` clean (`[workspace.lints.clippy] all = "deny"`); any
  exception is an **item-local** `#[allow(clippy::…)]` with a one-line rationale.
- **Do NOT run `cargo fmt`** (hand-formatted repo, no `rustfmt.toml`). Match neighbors by hand.
- Module-size budgets: `clippy::too_many_lines` (threshold 100) and `wordcartel/tests/module_budgets.rs`
  hub budgets. New model goes in a NEW `wordcartel/src/status.rs` so `editor.rs`/`render.rs` stay off
  the budget. A genuinely-flat exhaustive dispatch may carry an item-local
  `#[allow(clippy::too_many_lines)]` with a reason.
- Command-surface-contract invariant tests stay green (§ "Command-surface conformance" below).
- We touch NO backlog files → the backlog drift test is untouched.
- Pre-merge: run `scripts/smoke/run.sh` and quote its one-line summary (mandatory-run / advisory-pass).

---

## File structure

**New:** `wordcartel/src/status.rs` — `StatusKind`, `StatusLifetime`, `StatusTopic`, `StatusSource`,
`Status`, `SlotOutcome`, `resolve_slot`, `StatusHistory`, `StatusKind::from_str`. Declared `mod status;`
in `wordcartel/src/lib.rs`.

**Touched (shell only):** `editor.rs` (field swap + setter/accessor methods + `Buffer.read_only`),
`render_status.rs` + `render.rs` (read via accessor + kind→style), `chrome.rs` (visibility predicate),
`input.rs` (Esc dismiss + clear-transient), `transact.rs` (read-only guard), the sweep files (F4
table), `save.rs`/`commands.rs`/`filter.rs`/`transform.rs`/`jobs_apply.rs`/`derive.rs` (progress
topics), `plugin/api.rs` (`wc.status`/`wc.notify` + rate-limit), `config.rs` + `settings.rs` +
`registry.rs` + `app.rs` (`messages_min_kind` option), `limits.rs` (constants), `workspace.rs`/`scratch.rs`
(view_messages buffer). Test-module assertions that read `.status` migrate to the new accessor.

---

## Task list (overview)

1. **T1** — `status.rs` core types + `resolve_slot` precedence (Q1 + floor) + `from_str`. Pure, unit-tested.
2. **T2** — `StatusHistory` ring: `push`(+dedup/`repeat`), `collapse_topic`, `entries`, cap-evict. Unit-tested.
3. **T3** — Editor integration + **mechanical blanket sweep** (field flip → all sites compile as
   Info/Transient + CASE-correct clears) + accessor + render/chrome reads + kind→style. Behavior-preserving.
4. **T4** — F4 refinement batch A: **Error** sites (Sticky). Testable against the F4 table.
5. **T5** — F4 refinement batch B: **Warning-Sticky** sites. Testable against the F4 table.
6. **T6** — StatusTopic / progress plumbing: Save instance key + Filter/Transform + parse-recovery
   `ParseDegraded` set + `clear_topic`. Concurrency-sound.
7. **T7** — Esc **dismiss** of a held message in `input.rs::handle_key`.
8. **T8** — `view_messages` command + `Buffer.read_only` guard: TWO closed content-change categories —
   (a) in-place `Buffer::{apply,undo,redo}` + (b) whole-buffer `Editor::replace_buffer` — plus delegator
   Sticky-Warning/Q1 feedback, 3 search feedback guards, and a mechanical epilogue-completeness sweep.
9. **T9** — plugin migration: `wc.status` reroute + `wc.notify` + emit rate-limit.
10. **T10** — `messages_min_kind` toggle + full command-surface wiring (config/settings/registry/app).
11. **T11** — final gates: `limits.rs` constant-stability test, module budgets, clippy, full suite, smoke.

---

## The F4 per-site CASE table (Codex cross-checks every row)

**Counts (cfg(test)-robust):** 203 `.status =` assignments (**188 production, 15 test-gated**: 14 in
`mod tests` blocks + 1 inline `#[cfg(test)]` arm — jobs_apply.rs line 143, Codex-r4 #1) + 14
`.status.clear()` (6 production, 8 in `mod tests`). Test-module sites migrate mechanically to the
accessor / `set_status` in T3 and are not classified for kind (they are fixtures).

**Classification principle (the rule the exception tables apply):** severity/lifetime is decided by
**consequence**, not merely by wording. A message is **Sticky** (Error or Warning) when it reports a
genuine **failure** OR a **consequence the user might miss** — anything that arrives **asynchronously**
(a background job/save/reconcile/plugin/state-change completing later), a **blocked action** that needs
a retry, an **input-validation refusal** of a prompt the user submitted, or a **data/IO/config
failure**. A message is the **Info / Transient DEFAULT** only when it is **immediate decline-with-reason
feedback** for a synchronous action the user just took and is looking at, with **no lasting
consequence** (the goal state already holds, or they will simply retry). The exception tables below are
**exhaustive for all failure/refusal/warning wording** (re-verified by grepping every production
`.status =` for `fail|error|refus|discard|unavailable|appeared|cannot|can't|no…|warn|degrad|timed
out|too large|too long|invalid|already|empty|in progress|truncat|stale|changed|closed|e.to_string|
describe_error|{e}`); DEFAULT is reserved for genuine acks/decline-feedback only.

**DEFAULT rule (every production `set-message` site NOT in an exception table below):**
`editor.status = <text>` → `editor.set_status(StatusKind::Info, <text>)` → Info / **Transient** (clears
on next input, as today). This covers immediate decline feedback with no consequence: "no marked
block", "no sentence here", "already at a paragraph start", "no section here", "no heading", "empty
block", "no other buffer", "can't close the scratch buffer", "buffer already closed" (the close goal
already holds), "no other analysis engine", "{engine} is not enabled", "{engine} disabled — no analysis
engine enabled", "No matches", "can't move a block into itself", "no selection to mark", "no
bookmark/diagnostic/heading here", "No recovery files to clean", plus every genuine ack (counts,
"Copied", "Saved"/mode confirmations, "block copied/moved/deleted/cleared/marked", "keymap: …",
"chrome: … (no effect …)", "canvas: …", "wrap column: {n}"). The `diagnostics_run.rs` `"starting
{source}…"` echo is DEFAULT (Info/Transient), preserving today's next-key clear.

### Exhaustiveness — the MECHANICALLY-RECONCILED 188-site partition (Codex-r3/r4 Critical A)
**Reconciliation was run (not eyeballed), 2026-07-14.** Ground truth = the **188** production `.status =`
sites — `rg -n '\.status\s*=' wordcartel/src` minus every `#[cfg(test)]`-gated region (both `mod tests`
blocks AND inline `#[cfg(test)]` items/arms, computed by brace-tracking each `#[cfg(test)]` attribute's
gated span — the fix for the jobs_apply.rs line 143 inline-arm miss, Codex-r4 #1). The exception set below
(by `file:line`) was diffed programmatically against that dump; the diff is **empty**: (a) all 188 appear
exactly once across (exceptions ∪ DEFAULT), (b) no site is in both, (c) no listed site is absent from the
dump. **78 exception sites + 110 DEFAULT sites = 188.** DEFAULT is defined as the exact complement
`truth − exceptions` (so it cannot gap or double-list). The authoritative exception partition (the
semantic conversion for each category is in the tables that follow):

- **PROGRESS → `set_progress`/`finish_topic` (T6), 15:** `save.rs:64`, `save.rs:141`, `commands.rs:461`,
  `commands.rs:466`, `commands.rs:470`, `commands.rs:474`, `filter.rs:359`, `jobs_apply.rs:218`,
  `jobs_apply.rs:221`, `jobs_apply.rs:241`, `jobs_apply.rs:259`, `transform.rs:232`, `transform.rs:267`,
  `transform.rs:275`, `transform.rs:292`.
- **CLEAR-ALL → `clear_status()`, 5:** `session_restore.rs:123`, `workspace.rs:146`, `workspace.rs:210`,
  `workspace.rs:228`, `workspace.rs:241`.
- **CLEAR-TOPIC set (Warning + `ParseDegraded`), 1:** `derive.rs:329`.
- **ERROR / Sticky, 18:** `save.rs:227`, `app.rs:515`, `app.rs:801`, `export.rs:268`, `file_browser.rs:86`,
  `jobs_apply.rs:126`, `jobs_apply.rs:130`, `jobs_apply.rs:297`, `jobs_apply.rs:309`, `jobs_apply.rs:314`,
  `plugin/mod.rs:183`, `prompts.rs:161`, `search_ui.rs:33`, `search_ui.rs:176`, `session_restore.rs:127`,
  `settings.rs:560`, `swap.rs:451`, `workspace.rs:149`. (jobs_apply.rs line 143 "job failed (internal
  error)" is under an inline `#[cfg(test)]` arm — NOT prod, Codex-r4 #1 — so it is not swept.)
- **WARNING / Sticky, 38:** `save.rs:173` (external-mod refusal — Codex-r3 miss), `save.rs:214`,
  `commands.rs:457`, `clipboard.rs:172`, `clipboard.rs:184`, `clipboard.rs:190`, `jobs_apply.rs:43`,
  `jobs_apply.rs:77`, `jobs_apply.rs:283`, `jobs_apply.rs:321`, `jobs_apply.rs:360`, `workspace.rs:164`,
  `transform.rs:215`, `filter.rs:344`, `search_ui.rs:145`, `search_ui.rs:178`, `outline_overlay.rs:95`,
  `mouse.rs:365`, `editor.rs:973`, `prompts.rs:125`, `prompts.rs:147`, `prompts.rs:333`, `prompts.rs:366`,
  `prompts.rs:382`, `timers.rs:36`, `timers.rs:43`, `timers.rs:48`, `minibuffer.rs:123`,
  `plugin/pump.rs:223`, `diag_provider.rs:136`, `export.rs:261`, `settings.rs:549`, `settings.rs:553`,
  `app.rs:693`, `plugin/reload.rs:130`, `theme_cmds.rs:22`, `diagnostics_run.rs:140`,
  `diagnostics_run.rs:183`.
- **PLUGIN source → `Plugin{label}` (T9), 1:** `plugin/api.rs:420` (`wc.status`).

**DEFAULT (Info/Transient) = the 110 complement** — every site NOT above. It is exactly the
consequence-free acks + declines, e.g. `blocks_marked` "block …"/"no marked block", `prose_ops`
"split/merged/…"/"no sentence here", `scratch` "block … to scratch"/"already in the scratch buffer",
`registry` bookmarks/chrome/canvas/keymap/splash/"unknown command", `marks` set/jump/ring, `search_ui:46`
"No matches" + `search_ui:57` "Replaced {n}", `transform.rs:220` "nothing to transform" (Codex-r3 gap),
`registry.rs:796` plugin_list "plugins: {ok} ok …" summary (Codex-r3 gap), `plugin/reload.rs:132`
"plugins reloaded ({} ok)" (Codex-r3 de-duplicated — DEFAULT only), the `diagnostics_run.rs:164`
"starting …" echo, `jobs_apply.rs:294`/`:305` export success acks, and `app.rs`/`save.rs` startup/reload
acks. The `.status.clear()` sites (14; §4.3 clear-transient/clear-topic) are a SEPARATE set — not among
these 188 `.status =` assignments.
> A DEFAULT site that MUTATES the active buffer and reports an ack (e.g. `blocks_marked` "block moved",
> `prose_ops` "split paragraph", `scratch` "block moved to scratch", `search_ui` "Replaced N") keeps its
> Info kind here but is guarded against a *read-only* false-success at the **command-dispatch** level in
> T8 (Critical B) — kind classification and the read-only guard are orthogonal.

### Exceptions — CASE = clear-all (`= String::new()` → `clear_status()`)
| file:symbol | current expr |
|---|---|
| `workspace.rs` (switch/close/goto reset, ×4) | `editor.status = String::new()` |
| `session_restore.rs` (restore reset) | `editor.status = String::new()` |

### Exceptions — CASE = clear-transient (`.status.clear()` → `clear_transient_status()`)
| file:symbol | current expr | note |
|---|---|---|
| `input.rs::handle_key` (×3: Esc-pending, Resolution::Command, Resolution::None) | `editor.status.clear()` | next-key idiom |
| `marks.rs` (Esc cancels `pending_mark`) | `editor.status.clear()` | |
| `theme_cmds.rs` (`if editor.status.ends_with('…') { clear }`) | `editor.status.clear()` | ellipsis-preview echo; `clear_transient_status()` subsumes the `ends_with` guard (it only clears a Transient occupant) — drop the `ends_with` read |

### Exceptions — CASE = clear-topic (parse-degraded state indicator)
| file:symbol | current expr | becomes |
|---|---|---|
| `derive.rs::apply_parse_result` (recovery arm) | `editor.status.clear()` | `editor.clear_topic(StatusTopic::ParseDegraded)` |
| `derive.rs::apply_parse_result` (failure arm) | `editor.status = "markdown parse failed — styling may be stale"` | `set_status_full(Warning, …, Sticky, Host, Some(ParseDegraded))` |

### Exceptions — CASE = set-message, Progress pair (`set_progress` / `finish_topic`) — see T6
| op | start site | start → | completion site(s) → |
|---|---|---|---|
| Save (async) | `save.rs::do_save_to` `"Saving…"` | `set_progress(Save(bid, v), "Saving…")` | `save.rs` JobDone closure (`editor.status = status`) → `finish_topic(Save(bid, v), kind, status)`; `jobs_apply.rs` `apply_panic` Save arm ("save failed (internal error)") → `finish_topic(Save(bid, v), Error, …)` |
| Save (sync path) | `commands.rs` save arm `"Saving…"` | `set_progress(Save(bid, v), …)` | same arm: `"Saved"` / `"(unchanged)"` → `finish_topic(Save(bid,v), Info, …)`; `e.to_string()` → `finish_topic(Save(bid,v), Error, …)` |
| Filter | `filter.rs::run_filter` `"running {…} ..."` | `set_progress(Filter, …)` | `jobs_apply.rs`: `"filter applied"` → `finish_topic(Filter, Info, …)`; `"filter discarded - buffer changed"` → `finish_topic(Filter, Warning, …)`; `describe_error(&err)` → `finish_topic(Filter, Error, …)` |
| Transform | `transform.rs` `"{gerund}…"` | `set_progress(Transform, …)` | `transform.rs`: `"transform failed: {e}"` → `finish_topic(Transform, Error, …)`; `past_tense()` → `finish_topic(Transform, Info, …)`; `jobs_apply.rs` `"transform discarded — buffer changed"` → `finish_topic(Transform, Warning, …)` |

> **NOT progress (spec §4.2 enumeration):** `diagnostics_run.rs` `"starting {source}…"` has NO paired
> completion → maps to DEFAULT (Info/Transient), preserving today's next-key clear. Export writes only
> terminal messages ("exported …"/"export … failed") → ordinary Info/Error, NO topic. `input.rs`
> `"cancelling…"` and the pending-chord `format!("{} …")` are transient echoes → DEFAULT (Info/Transient).

### Exceptions — CASE = set-message, Error (`set_status_full(Error, …, Sticky, Host, None)`)
Genuine failures (IO/config/internal). Every `e.to_string()` of a typed error is here.
| file:symbol | current string / expr |
|---|---|
| `save.rs::do_save_to` save-as fail arm | `Err(e) => editor.status = e.to_string()` (SaveError) — the SaveAs entry, distinct from the async completion (T6 finish_topic) |
| **`save.rs::reload_from_disk` (NEW — Codex-r1 #1)** | `Err(e) => { editor.status = e.to_string(); return; }` — reload-from-disk read failure |
| `jobs_apply.rs` (×3 internal-error) | `"save failed (internal error: {msg})"`; `"swap failed (internal error: {msg})"`; `"job failed (internal error: {msg})"` |
| `jobs_apply.rs` export | `"export write failed: {e}"`; `"export rename failed: {e}"` |
| `jobs_apply.rs` paste-clipboard | `describe_error(&e)` (non-progress paste path) |
| `prompts.rs` write-block | `Err(e) => editor.status = e.to_string()` |
| `search_ui.rs` | `"invalid regex"`; `"add to dictionary failed: {e}"` |
| `workspace.rs` open | `Err(e) => editor.status = e.to_string()` |
| `settings.rs` save | `Err(e) => editor.status = format!("settings: {e}")` |
| `export.rs` | `"pandoc not found — install it to export"` |
| `session_restore.rs` | `Err(e) => editor.status = e.to_string()` |
| `file_browser.rs` | `"cannot read directory: {display}"` |
| `swap.rs` | `"swap write failed"` |
| `app.rs` open | `editor.status = e.to_string()` (OpenError) |
| `app.rs` plugin-attach | `"plugin bridge failed to attach: {e}"` |

### Exceptions — CASE = set-message, Warning (`set_status_full(Warning, …, Sticky, Host, None)`)
Async/missable consequences, blocked actions, prompt-input refusals, capability/config refusals.
| file:symbol | current string |
|---|---|
| `jobs_apply.rs` (async) | `"edited during save — quit cancelled"`; `"edited during save — close cancelled"`; `"paste too large ({} MiB) — skipped"`; `"system clipboard unavailable — copy/paste work in-editor (register only)"` |
| **`jobs_apply.rs` (NEW — Codex-r1 #1)** | `"export target {} appeared — re-run export to overwrite"` — export TOCTOU refusal (recoverable → Warning, not Error) |
| `workspace.rs` | `"another save or quit is in progress — try again"` (blocked action). NOTE: `"buffer already closed"` is DEFAULT/Info (the close goal already holds — no consequence) |
| `transform.rs` (blocked) | `"a transform is already running"` |
| `filter.rs` (blocked) | `"a filter is already running"` |
| `search_ui.rs` (async state-change / config) | `"document changed; re-open"`; `"no dictionary path configured"` |
| `outline_overlay.rs`, `mouse.rs` (async state-change) | `"document changed; outline closed"` |
| `editor.rs` | `UNDO_EVICTED_HINT` ("Undo history trimmed to fit …") — the M5 louder-hint (consequence: history lost) |
| `prompts.rs` (prompt-input refusals) | `"save-as: empty path"`; `"write block: empty path"`; `"filter: no command given"`; `"wrap column: not a number"`; `"not a line number"` |
| `timers.rs` | `"Save still running — choose again"`; `"save timed out — quit cancelled"`; `"save timed out — close cancelled"` |
| `clipboard.rs` (×3) | `"clipboard unavailable"` |
| `minibuffer.rs` | `"plugin: command arg too long"` |
| `pump.rs` | `"plugin work truncated (chain cap)"` |
| `diag_provider.rs` | `ProviderEvent::Degraded(hint)` → `editor.status = hint` (a degradation notice) |
| `export.rs` (refusal) | `"save the file first before exporting"` |
| **`settings.rs` (NEW)** | `"settings: no config directory"` — settings-save refusal (consequence: not saved) |
| **`diagnostics_run.rs` (NEW)** | `"document too large for grammar checking"` — capability refusal |
| **`save.rs` + `commands.rs` (NEW)** | `"No file name — use Save As"` (both the `overwrite_save`/`dispatch_save` guard in `save.rs` AND the `commands.rs` save arm) — a save refusal (consequence: not saved) |
| **`app.rs` startup (NEW — Codex-r1 #1)** | `if let Some(w) = warns.first() { editor.status = w.clone(); }` — startup config/keymap/plugin/diagnostic warnings (warnings by construction) |
| **`diagnostics_run.rs` (NEW — Codex-r2)** | `editor.status = hint.into()` — the install/unavailable hint (concrete text `harper_ls.rs` "grammar checker unavailable — install harper-ls…"); async capability refusal |
| **`theme_cmds.rs` (NEW — Codex-r2)** | `if let Some(w) = kw.first() { editor.status = w.clone(); }` — keymap-rebuild warnings (same warning-by-construction pattern as the `app.rs` startup row). NOTE: distinct from the `theme_cmds.rs` ellipsis-preview `.status.clear()` (a clear-transient site) |
| **`settings.rs` (NEW — Codex-r2)** | `"settings: disabled by --no-config"` — settings-save refusal (same "not saved" consequence as `"settings: no config directory"`) |
| **`save.rs:173` (NEW — Codex-r3)** | `"File changed on disk — choose [R]eload or [O]verwrite"` — external-mod save refusal (blocked save + choice consequence). NOT a Save-progress site (that's `save.rs:64`/`:141`) |

### Exceptions — plugin/source (T9, not the host sweep)
| file:symbol | becomes |
|---|---|
| `plugin/api.rs:420::install_status` (`wc.status`) | `set_status_full(Info, capped, StatusLifetime::default_for(Info), Plugin{label}, None)` |
| `plugin/mod.rs:183` (`"plugin {name}: {capped}"` — surfaced plugin error) | `set_status_full(Error, …, Sticky, Plugin{label}, None)` — an ERROR-category row |
| `plugin/reload.rs:130` (`warns.first()`) | warn → Warning/Sticky (in the WARNING partition). **`plugin/reload.rs:132` `"plugins reloaded ({} ok)"` is DEFAULT/Info — NOT an exception (Codex-r3 de-dup).** |
| `registry.rs:786` reload-plugins echo (`"reloading plugins…"`) + `registry.rs:796` plugin_list summary | both DEFAULT (Info/Transient) — status echoes / success summaries, not exceptions |

> Every row above is grounded from the string already present. The implementer re-locates each by
> symbol name (line numbers drift) and confirms the string before rewriting. Any site whose wording is
> ambiguous on inspection (a `format!` with runtime-variable severity) is surfaced to the human, not
> guessed.

---

## Task 1 — `status.rs`: core types + `resolve_slot` + `from_str`

**Files/Interfaces:** NEW `wordcartel/src/status.rs`; `wordcartel/src/lib.rs` gains `mod status;`. No
`Editor` dependency yet (pure). `StatusTopic::Save` references `crate::editor::BufferId`
(`pub struct BufferId(pub u64)`, derives `Clone, Copy, PartialEq, Eq, Hash, Ord`).

**Step 1 (failing test).** In `status.rs` `#[cfg(test)] mod tests`, write the precedence + parse tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::BufferId;

    fn s(kind: StatusKind) -> Status {
        Status::new(kind, "x", StatusLifetime::default_for(kind), StatusSource::Host, None, 1)
    }

    #[test]
    fn ord_is_error_smallest() {
        assert!(StatusKind::Error < StatusKind::Warning);
        assert!(StatusKind::Warning < StatusKind::Info);
        assert!(StatusKind::Info < StatusKind::Log);
    }

    #[test]
    fn empty_slot_always_takes() {
        let cand = s(StatusKind::Log);
        assert_eq!(resolve_slot(None, &cand, StatusKind::Info), SlotOutcome::Take);
    }

    #[test]
    fn equal_or_more_severe_takes_the_slot() {
        let occ = s(StatusKind::Warning);
        assert_eq!(resolve_slot(Some(&occ), &s(StatusKind::Warning), StatusKind::Info), SlotOutcome::Take);
        assert_eq!(resolve_slot(Some(&occ), &s(StatusKind::Error),   StatusKind::Info), SlotOutcome::Take);
    }

    #[test]
    fn less_severe_is_history_only() {
        let occ = s(StatusKind::Error);
        assert_eq!(resolve_slot(Some(&occ), &s(StatusKind::Info), StatusKind::Info), SlotOutcome::HistoryOnly);
    }

    #[test]
    fn below_floor_is_history_only_even_on_empty_slot() {
        // floor = Warning → an Info candidate is below the floor → history-only.
        assert_eq!(resolve_slot(None, &s(StatusKind::Info), StatusKind::Warning), SlotOutcome::HistoryOnly);
        // Log is always below an Info floor.
        assert_eq!(resolve_slot(None, &s(StatusKind::Log), StatusKind::Info), SlotOutcome::HistoryOnly);
    }

    #[test]
    fn from_str_round_trips() {
        assert_eq!(StatusKind::from_str("error"),   Some(StatusKind::Error));
        assert_eq!(StatusKind::from_str("warning"), Some(StatusKind::Warning));
        assert_eq!(StatusKind::from_str("info"),    Some(StatusKind::Info));
        assert_eq!(StatusKind::from_str("log"),     Some(StatusKind::Log));
        assert_eq!(StatusKind::from_str("bogus"),   None);
    }

    #[test]
    fn save_topic_instance_keys_differ_by_version() {
        let a = StatusTopic::Save(BufferId(1), 5);
        let b = StatusTopic::Save(BufferId(1), 6);
        assert_ne!(a, b);
        assert_eq!(a, StatusTopic::Save(BufferId(1), 5));
    }
}
```
Run: `cargo test -p wordcartel status::` → fails to compile (types absent).

**Step 2 (minimal impl).** Full module body:
```rust
//! A17 — the one routed user-message model (shell-only; core has no status concept).
//! `Status` is the single value every user-facing message becomes; `resolve_slot` is the pure
//! Q1 severity-ranked slot rule; `StatusHistory` (Task 2) is the browsable ring.

/// User-message severity, mirroring LSP `window/showMessage` `MessageType` (Error=1 … Log=4).
/// Variant ORDER is most-severe first, so the derived `Ord` gives `Error < Warning < Info < Log` —
/// the MORE severe a kind, the SMALLER it compares. The Q1 rule is "candidate takes the slot iff
/// `candidate.kind <= occupant.kind`". This inversion is load-bearing; do NOT describe it as
/// "Error > Warning > …".
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum StatusKind { Error, Warning, Info, Log }

impl StatusKind {
    /// Parse the `wc.notify` / `[view] messages_min_kind` severity string. `None` on an unknown
    /// spelling (the caller surfaces a typed error — never a silent default).
    pub fn from_str(s: &str) -> Option<StatusKind> {
        match s {
            "error"   => Some(StatusKind::Error),
            "warning" => Some(StatusKind::Warning),
            "info"    => Some(StatusKind::Info),
            "log"     => Some(StatusKind::Log),
            _ => None,
        }
    }
    /// The persisted / round-trip spelling (mirrors `config::transient_mode_str`).
    pub fn as_str(self) -> &'static str {
        match self {
            StatusKind::Error => "error", StatusKind::Warning => "warning",
            StatusKind::Info => "info",   StatusKind::Log => "log",
        }
    }
}

/// Orthogonal to severity. `Transient` clears on next input (Info/Log). `Sticky` holds until
/// dismissed or superseded (Warning/Error). `Progress` is held but expected to be superseded by its
/// own operation's completion (a `finish_topic` naming the same `StatusTopic`) and collapsed in
/// history; never traps (`Esc` dismisses, its completion always supersedes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusLifetime { Transient, Sticky, Progress }

impl StatusLifetime {
    /// Default lifetime for a kind when the caller does not override.
    pub fn default_for(kind: StatusKind) -> StatusLifetime {
        match kind {
            StatusKind::Info | StatusKind::Log => StatusLifetime::Transient,
            StatusKind::Warning | StatusKind::Error => StatusLifetime::Sticky,
        }
    }
}

/// Correlation handle (spec §3.3). Instance-keyed for the one same-op-concurrent progress (Save);
/// static for the global single-slot progresses (Filter/Transform) and the singleton parse-degraded
/// state indicator. An EXACT-MATCH key: a Filter finish can never collapse a Save entry, and a Save
/// of (buffer B, version 7) can never collapse the Save of (buffer B, version 5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusTopic {
    Save(crate::editor::BufferId, u64),
    Filter,
    Transform,
    ParseDegraded,
}

/// Where a message originated. There is NO stable plugin id in the shipped plugin system
/// (`plugin/host.rs::Bridge` holds only `InvokeState { current: Option<String>, … }`), so plugin
/// attribution is by that invocation LABEL, best-effort. Never a shadowing field.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StatusSource { Host, Plugin { label: Option<String> } }

/// The one value every user-facing message becomes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Status {
    kind: StatusKind,
    text: String,
    lifetime: StatusLifetime,
    source: StatusSource,
    topic: Option<StatusTopic>,
    seq: u64,
    repeat: u32,
}

impl Status {
    /// Construct a message, capping `text` to `MESSAGES_MAX_TEXT_LEN` on a char boundary and
    /// stamping `repeat = 1`. `seq` is the caller's monotonic emit counter (history ordering + dedup).
    pub fn new(
        kind: StatusKind, text: impl Into<String>, lifetime: StatusLifetime,
        source: StatusSource, topic: Option<StatusTopic>, seq: u64,
    ) -> Status {
        let mut text = text.into();
        // Cap on a char boundary — never split a UTF-8 sequence (multibyte-safe).
        if text.len() > crate::limits::MESSAGES_MAX_TEXT_LEN {
            let mut end = crate::limits::MESSAGES_MAX_TEXT_LEN;
            while !text.is_char_boundary(end) { end -= 1; }
            text.truncate(end);
        }
        Status { kind, text, lifetime, source, topic, seq, repeat: 1 }
    }
    #[inline] pub fn kind(&self) -> StatusKind { self.kind }
    #[inline] pub fn text(&self) -> &str { &self.text }
    #[inline] pub fn lifetime(&self) -> StatusLifetime { self.lifetime }
    #[inline] pub fn source(&self) -> &StatusSource { &self.source }
    #[inline] pub fn topic(&self) -> Option<StatusTopic> { self.topic }
    #[inline] pub fn repeat(&self) -> u32 { self.repeat }
    #[inline] pub(crate) fn bump_repeat(&mut self) { self.repeat = self.repeat.saturating_add(1); }
}

/// The outcome of the Q1 slot rule for one candidate against the current occupant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotOutcome { Take, HistoryOnly }

/// Pure Q1 precedence (spec §4.1). `floor` is `messages_min_kind` (verbosity floor). A candidate
/// strictly less severe than the floor is history-only. Otherwise it takes the slot iff there is no
/// occupant OR it is at least as severe as the occupant (`candidate.kind <= occupant.kind`).
pub fn resolve_slot(occupant: Option<&Status>, candidate: &Status, floor: StatusKind) -> SlotOutcome {
    if candidate.kind > floor {
        return SlotOutcome::HistoryOnly; // below the verbosity floor (more severe = smaller)
    }
    match occupant {
        None => SlotOutcome::Take,
        Some(occ) if candidate.kind <= occ.kind => SlotOutcome::Take,
        Some(_) => SlotOutcome::HistoryOnly,
    }
}
```
Add the `MESSAGES_MAX_TEXT_LEN` constant in `limits.rs` now (T11 asserts stability):
`pub const MESSAGES_MAX_TEXT_LEN: usize = PLUGIN_MAX_STATUS_LEN;` (4096).

Run: `cargo test -p wordcartel status::` → green. Commit `A17 T1: status core types + resolve_slot`.

---

## Task 2 — `StatusHistory` ring (push/dedup/collapse_topic/entries)

**Files/Interfaces:** `wordcartel/src/status.rs` (append). `limits.rs`: add
`MESSAGES_HISTORY_CAP: usize = 256` and `MESSAGES_DEDUP_WINDOW: u64 = 1`. Uses
`std::collections::VecDeque`.

**Step 1 (failing test).** Append to `status.rs` tests:
```rust
#[test]
fn push_evicts_oldest_at_cap() {
    let mut h = StatusHistory::new();
    for i in 0..(crate::limits::MESSAGES_HISTORY_CAP + 10) as u64 {
        h.push(Status::new(StatusKind::Info, format!("m{i}"), StatusLifetime::Transient,
                           StatusSource::Host, None, i));
    }
    assert_eq!(h.entries().len(), crate::limits::MESSAGES_HISTORY_CAP);
    assert_eq!(h.entries().front().unwrap().text(), "m10"); // oldest 0..9 evicted
}

#[test]
fn adjacent_identical_coalesces_repeat() {
    let mut h = StatusHistory::new();
    h.push(Status::new(StatusKind::Info, "loop", StatusLifetime::Transient, StatusSource::Host, None, 1));
    h.push(Status::new(StatusKind::Info, "loop", StatusLifetime::Transient, StatusSource::Host, None, 2));
    assert_eq!(h.entries().len(), 1);
    assert_eq!(h.entries().back().unwrap().repeat(), 2);
}

#[test]
fn collapse_topic_replaces_matching_lineage_in_place() {
    use crate::editor::BufferId;
    let mut h = StatusHistory::new();
    let t = StatusTopic::Save(BufferId(1), 5);
    h.push(Status::new(StatusKind::Info, "Saving…", StatusLifetime::Progress, StatusSource::Host, Some(t), 1));
    h.push(Status::new(StatusKind::Info, "other",   StatusLifetime::Transient, StatusSource::Host, None, 2));
    let done = Status::new(StatusKind::Info, "Saved", StatusLifetime::Transient, StatusSource::Host, Some(t), 3);
    h.collapse_topic(t, done);
    // The "Saving…" entry was replaced in place by "Saved"; no new trailing entry appended.
    let texts: Vec<&str> = h.entries().iter().map(|s| s.text()).collect();
    assert_eq!(texts, vec!["Saved", "other"]);
}

#[test]
fn collapse_topic_no_match_appends() {
    use crate::editor::BufferId;
    let mut h = StatusHistory::new();
    let done = Status::new(StatusKind::Info, "Saved", StatusLifetime::Transient, StatusSource::Host,
                           Some(StatusTopic::Save(BufferId(1), 5)), 1);
    h.collapse_topic(StatusTopic::Save(BufferId(1), 5), done);
    assert_eq!(h.entries().len(), 1);
}
```
Run → fails (type absent).

**Step 2 (impl).** Append:
```rust
use std::collections::VecDeque;

/// Bounded in-memory ring of recent user messages (spec §5). M5 resource-cap ethos: fixed capacity,
/// oldest evicted, no growth at rest. Written only on an emit — never on a timer.
#[derive(Debug, Default)]
pub struct StatusHistory { entries: VecDeque<Status> }

impl StatusHistory {
    pub fn new() -> StatusHistory { StatusHistory { entries: VecDeque::new() } }
    pub fn entries(&self) -> &VecDeque<Status> { &self.entries }

    /// Append `msg`, coalescing an immediately-repeated identical message (spec §5.2). Evicts the
    /// oldest when at `MESSAGES_HISTORY_CAP`.
    pub fn push(&mut self, msg: Status) {
        if let Some(last) = self.entries.back_mut() {
            if last.kind == msg.kind && last.text == msg.text && last.source == msg.source
                && msg.seq.saturating_sub(last.seq) <= crate::limits::MESSAGES_DEDUP_WINDOW
            {
                last.bump_repeat();
                return;
            }
        }
        self.entries.push_back(msg);
        while self.entries.len() > crate::limits::MESSAGES_HISTORY_CAP {
            self.entries.pop_front();
        }
    }

    /// Progress-completion collapse (spec §4.2): replace the most-recent entry whose `topic` exactly
    /// equals `topic` with `terminal` (in place — no append), else fall back to `push`. Exact-match on
    /// the full topic value (Save carries `(BufferId, version)`) makes a Filter finish unable to
    /// collapse a Save entry, and a same-buffer different-version Save unable to collapse another.
    pub fn collapse_topic(&mut self, topic: StatusTopic, terminal: Status) {
        if let Some(slot) = self.entries.iter_mut().rev().find(|s| s.topic == Some(topic)) {
            *slot = terminal;
        } else {
            self.push(terminal);
        }
    }
}
```
Run → green. Commit `A17 T2: bounded StatusHistory ring`.

---

## Task 3 — Editor integration + mechanical blanket sweep (behavior-preserving)

This is the compile-forcing migration: swap the field type and route every production + test site
through the new setters/accessor. Everything defaults to **Info / Transient** (plus the CASE-correct
`clear_status`/`clear_transient_status`), which reproduces today's behavior (Info replaces Info under
Q1, exactly like last-writer-wins; Transient clears on next key exactly like today). The F4 refinement
(T4–T6) upgrades the flagged minority afterward. **The F4 table's CASE column governs THIS task**
(set-message vs clear-all vs clear-transient); the kind/lifetime columns are consumed in T4–T6.

**Files/Interfaces:**
- `editor.rs`: `Editor` — remove `pub status: String` (and its `status: String::new()` init); add
  `status: Option<crate::status::Status>`, `status_history: crate::status::StatusHistory`,
  `messages_min_kind: crate::status::StatusKind`, `status_seq: u64`. Add methods (below). `Buffer`
  gains nothing yet (read_only is T8).
- `render_status.rs::status_left_text`: reads `editor.status` → `editor.status_text()` (`&str`, `""`
  when none).
- `render.rs::paint_status`: normal-branch style picks by kind (kind→style, see below).
- `chrome.rs::status_line_visible`: `!editor.status.is_empty()` → `editor.has_visible_status()`.
- The 188 prod set-message sites, 5 prod clear-all, 5 prod clear-transient (the `derive.rs`
  ParseDegraded pair is deferred to T6 — in T3 it maps to the DEFAULT: failure → `set_status(Info)`,
  recovery → `clear_transient_status()`, behavior-preserving), and the 15 test-gated `.status =` sites.

**Editor methods (add to `impl Editor`):**
```rust
use crate::status::{Status, StatusKind, StatusLifetime, StatusSource, StatusTopic, SlotOutcome};

impl Editor {
    fn next_status_seq(&mut self) -> u64 { self.status_seq += 1; self.status_seq }

    /// The ONE writer for the user-message slot. Info/Warning/Error/Log with default lifetime.
    pub fn set_status(&mut self, kind: StatusKind, text: impl Into<String>) {
        self.set_status_full(kind, text, StatusLifetime::default_for(kind), StatusSource::Host, None);
    }

    /// Full form: explicit lifetime, source, topic. Applies Q1 to the display slot and ALWAYS records
    /// to history (spec §4.1).
    pub fn set_status_full(
        &mut self, kind: StatusKind, text: impl Into<String>,
        lifetime: StatusLifetime, source: StatusSource, topic: Option<StatusTopic>,
    ) {
        let seq = self.next_status_seq();
        let cand = Status::new(kind, text, lifetime, source, topic, seq);
        match crate::status::resolve_slot(self.status.as_ref(), &cand, self.messages_min_kind) {
            SlotOutcome::Take => { self.status_history.push(cand.clone()); self.status = Some(cand); }
            SlotOutcome::HistoryOnly => { self.status_history.push(cand); }
        }
    }

    /// Start a progress message (spec §4.2): Info, Progress lifetime, Host, tagged `topic`.
    pub fn set_progress(&mut self, topic: StatusTopic, text: impl Into<String>) {
        self.set_status_full(StatusKind::Info, text, StatusLifetime::Progress, StatusSource::Host, Some(topic));
    }

    /// Complete/fail progress for `topic`: apply Q1 to the display slot AND collapse the topic's
    /// most-recent history lineage in place (spec §4.2).
    pub fn finish_topic(&mut self, topic: StatusTopic, kind: StatusKind, text: impl Into<String>) {
        let seq = self.next_status_seq();
        let cand = Status::new(kind, text, StatusLifetime::default_for(kind), StatusSource::Host, Some(topic), seq);
        match crate::status::resolve_slot(self.status.as_ref(), &cand, self.messages_min_kind) {
            SlotOutcome::Take => { self.status_history.collapse_topic(topic, cand.clone()); self.status = Some(cand); }
            SlotOutcome::HistoryOnly => { self.status_history.collapse_topic(topic, cand); }
        }
    }

    /// Clear the slot iff the occupant is Transient (the next-key idiom). Warning/Error/Progress hold.
    pub fn clear_transient_status(&mut self) {
        if matches!(self.status.as_ref().map(|s| s.lifetime()), Some(StatusLifetime::Transient)) {
            self.status = None;
        }
    }

    /// Clear the slot iff the occupant carries exactly `topic` (targeted state-indicator clear).
    pub fn clear_topic(&mut self, topic: StatusTopic) {
        if self.status.as_ref().and_then(|s| s.topic()) == Some(topic) { self.status = None; }
    }

    /// Unconditional slot reset (context change — buffer switch, session restore). History unaffected.
    pub fn clear_status(&mut self) { self.status = None; }

    /// Esc dismiss: clear a HELD occupant (Sticky or Progress). Transient is handled by the next key.
    pub fn dismiss_status(&mut self) {
        if matches!(self.status.as_ref().map(|s| s.lifetime()),
                    Some(StatusLifetime::Sticky) | Some(StatusLifetime::Progress)) {
            self.status = None;
        }
    }

    /// Slot accessors.
    #[inline] pub fn status(&self) -> Option<&Status> { self.status.as_ref() }
    #[inline] pub fn status_text(&self) -> &str { self.status.as_ref().map_or("", |s| s.text()) }
    #[inline] pub fn has_visible_status(&self) -> bool { self.status.is_some() }
    #[inline] pub fn status_history(&self) -> &crate::status::StatusHistory { &self.status_history }
}
```
Init in `Editor::new`: `status: None, status_history: crate::status::StatusHistory::new(),
messages_min_kind: StatusKind::Info, status_seq: 0,`.

**kind→style (render.rs `paint_status`, spec §10.2 — compose existing faces, no new `SemanticElement`):**
in the normal (visible, no-overlay) branch, replace the unconditional `cs.menu_closed` with:
```rust
let base = cs.menu_closed;
let status_style = match editor.status().map(|s| s.kind()) {
    Some(StatusKind::Error)   => base.patch(compose::compose(&editor.theme, editor.depth, &[SE::ChromeAccent])
                                   .add_modifier(Modifier::REVERSED | Modifier::BOLD)),
    Some(StatusKind::Warning) => base.add_modifier(Modifier::BOLD),
    _ => base, // Info / Log / none: unchanged chrome face
};
```
(The exact face/modifier composition is finalized against `ChromeStyles`/`compose` during
implementation — the invariant is: Error/Warning are distinguishable from Info **and legible under
`no-color`/`terminal-plain` via modifiers, never color alone**. No new `SemanticElement` variant → the
theme-completeness contract is untouched.)

**Step 1 (failing test).** Add to `editor.rs` tests:
```rust
#[test]
fn set_status_shows_text_and_reveals_bar() {
    let mut e = Editor::new_from_text("hi\n", None, (40, 6));
    e.set_status(crate::status::StatusKind::Info, "hello");
    assert_eq!(e.status_text(), "hello");
    assert!(e.has_visible_status());
    assert_eq!(e.status_history().entries().len(), 1);
}
#[test]
fn clear_transient_clears_info_but_history_keeps_it() {
    let mut e = Editor::new_from_text("hi\n", None, (40, 6));
    e.set_status(crate::status::StatusKind::Info, "x");
    e.clear_transient_status();
    assert!(!e.has_visible_status());
    assert_eq!(e.status_history().entries().len(), 1);
}
```
Run: fails to compile (field/methods absent).

**Step 2 (impl).** Add fields + methods; then the **mechanical sweep** (compiler-driven — `cargo build`
lists every remaining site): rewrite each per the F4 **DEFAULT/CASE** rules (all set-messages →
`set_status(Info, …)`; the 5 `= String::new()` → `clear_status()`; the 5 `.status.clear()` →
`clear_transient_status()`; `derive.rs` failure/recovery to Info/`clear_transient_status` for now). Test
sites: `assert_eq!(e.status, "…")` → `assert_eq!(e.status_text(), "…")`; `e.status = "…"` fixtures →
`e.set_status(StatusKind::Info, "…")`. Update `render_status`/`chrome`/`render` reads.

**Verification against the F4 table:** the reviewer diffs T3 against the F4 table's DEFAULT + CASE
columns — every rewritten line is `set_status(Info,…)` OR one of the two clear verbs (per its CASE row);
no site yet carries a non-Info kind (those are T4–T6). Run full `cargo test -p wordcartel` → green.
Commit `A17 T3: field flip + blanket sweep to set_status (behavior-preserving)`.

---

## Task 4 — F4 refinement A: Error sites (Sticky)

**Files/Interfaces:** the Error rows of the F4 table (save/jobs_apply/prompts/search_ui/workspace/
settings/export/session_restore/file_browser/swap/app). Each `set_status(Info, e)` at an Error row →
`set_status_full(StatusKind::Error, e, StatusLifetime::Sticky, StatusSource::Host, None)`.

**Step 1 (failing test).** Drive the real reload-failure Error site (`reload_from_disk(&mut Editor)`,
which reads `active().document.path` from disk and on `Err` sets `editor.status = e.to_string()`) in the
`save.rs` test module:
```rust
#[test]
fn reload_from_disk_failure_is_a_sticky_error_that_survives_a_later_info() {
    // A path that will fail to read → the reload Err arm fires.
    let missing = std::path::PathBuf::from("/nonexistent/definitely/missing-a17.md");
    let mut e = Editor::new_from_text("hello\n", Some(missing), (80, 24));
    crate::save::reload_from_disk(&mut e);
    assert_eq!(e.status().unwrap().kind(), StatusKind::Error);
    assert_eq!(e.status().unwrap().lifetime(), StatusLifetime::Sticky);
    e.set_status(StatusKind::Info, "later ack");
    // Q1: an Info does NOT displace a held Error.
    assert_eq!(e.status().unwrap().kind(), StatusKind::Error);
}
```
Run → fails (still Info from T3; `reload_from_disk` is a `pub fn` in `save.rs`).

**Step 2 (impl).** Convert the Error rows. Run → green. Commit `A17 T4: F4 Error sites (Sticky)`.

**Batch verification:** reviewer confirms every changed line matches an Error row in the F4 table and no
non-Error row was touched.

---

## Task 5 — F4 refinement B: Warning-Sticky sites

**Files/Interfaces:** the Warning rows of the F4 table. Each → `set_status_full(StatusKind::Warning, …,
StatusLifetime::Sticky, StatusSource::Host, None)`. Includes the `editor.rs` `UNDO_EVICTED_HINT`
louder-hint (M5 follow-up) and the `diag_provider.rs` `Degraded(hint)`.

**Step 1 (failing test).** Drive a real Warning site (the `wrap_column_submit` prompt-input refusal,
`pub(crate)`), plus the lifetime-behavior test:
```rust
#[test]
fn wrap_column_not_a_number_is_a_sticky_warning() {
    let mut e = Editor::new_from_text("x\n", None, (40, 6));
    crate::prompts::wrap_column_submit(&mut e, "abc"); // non-numeric → the "wrap column: not a number" arm
    assert_eq!(e.status().unwrap().kind(), StatusKind::Warning);
    assert_eq!(e.status().unwrap().lifetime(), StatusLifetime::Sticky);
}
#[test]
fn a_warning_holds_through_a_keypress_clear() {
    let mut e = Editor::new_from_text("x\n", None, (40, 6));
    e.set_status_full(StatusKind::Warning, "w", StatusLifetime::Sticky, StatusSource::Host, None);
    e.clear_transient_status(); // simulates the next-key idiom
    assert!(e.has_visible_status(), "a Warning is not cleared by the next key");
}
```
Run → fails (`wrap_column_submit` still emits Info from T3). **Step 2:** convert the Warning rows (incl.
the `editor.rs` `UNDO_EVICTED_HINT` louder-hint and `diag_provider.rs` `Degraded(hint)`). Run → green.
Commit `A17 T5: F4 Warning-Sticky sites`.

---

## Task 6 — StatusTopic / progress plumbing (Save instance key, Filter/Transform, ParseDegraded)

**Grounding tasks (do first, by name):** in `save.rs` locate the `"Saving…"` start (`do_save_to`) — it
already binds `buffer_id = active().id` and `v = active().document.version` before dispatch; confirm the
JobDone closure (`editor.status = status`) and `jobs_apply.rs::apply_panic` Save arm both have
`buffer_id`/`version` in hand (`(kind, version, buffer_id, class) = (r.kind, r.version, r.buffer_id, …)`).
In `commands.rs` locate the synchronous save arm (`"Saving…"`/`"Saved"`/`"(unchanged)"`/`e.to_string()`)
and confirm it has the active buffer id + version in scope. Locate `filter.rs::run_filter`,
`transform.rs`, and the filter/transform completion arms in `jobs_apply.rs`/`transform.rs`. Locate
`derive.rs::apply_parse_result`.

**Files/Interfaces:** `save.rs`, `commands.rs`, `filter.rs`, `transform.rs`, `jobs_apply.rs`,
`derive.rs`. Uses `StatusTopic::{Save, Filter, Transform, ParseDegraded}` and `set_progress` /
`finish_topic` / `clear_topic` from T1/T3.

**Conversions (per the F4 progress + clear-topic rows):**
- Save start → `set_progress(StatusTopic::Save(buffer_id, v), "Saving…")`; each Save completion/failure
  → `finish_topic(StatusTopic::Save(buffer_id, v), kind, text)` (Info for "Saved"/"Saved v…"/"(unchanged)",
  Error for the failure/panic text). Reconstruct the identical `(buffer_id, v)` at the completion from
  `r.buffer_id`/`r.version` (async) or the in-scope ids (sync).
- Filter start → `set_progress(StatusTopic::Filter, …)`; completions → `finish_topic(Filter, {Info
  applied | Warning discarded | Error describe_error})`.
- Transform start → `set_progress(StatusTopic::Transform, …)`; completions → `finish_topic(Transform,
  {Info past_tense | Warning discarded | Error "transform failed"})`.
- `derive.rs` failure arm → `set_status_full(Warning, "markdown parse failed — styling may be stale",
  Sticky, Host, Some(StatusTopic::ParseDegraded))`; recovery arm → `clear_topic(StatusTopic::ParseDegraded)`.

**Step 1 (failing tests).**
```rust
#[test]
fn two_saves_of_same_buffer_different_version_collapse_independently() {
    use crate::status::StatusTopic;
    let mut e = Editor::new_from_text("x\n", None, (40, 6));
    let b = e.active().id;
    e.set_progress(StatusTopic::Save(b, 5), "Saving…");
    e.set_progress(StatusTopic::Save(b, 6), "Saving…");
    e.finish_topic(StatusTopic::Save(b, 5), StatusKind::Info, "Saved v5");
    // The v5 lineage collapsed; the v6 progress entry is untouched.
    let h = e.status_history();
    assert!(h.entries().iter().any(|s| s.text() == "Saved v5"));
    assert!(h.entries().iter().any(|s| s.text() == "Saving…"
        && s.topic() == Some(StatusTopic::Save(b, 6))));
}
#[test]
fn filter_finish_does_not_collapse_a_save() {
    use crate::status::StatusTopic;
    let mut e = Editor::new_from_text("x\n", None, (40, 6));
    let b = e.active().id;
    e.set_progress(StatusTopic::Save(b, 1), "Saving…");
    e.finish_topic(StatusTopic::Filter, StatusKind::Info, "filter applied");
    assert!(e.status_history().entries().iter()
        .any(|s| s.text() == "Saving…" && s.topic() == Some(StatusTopic::Save(b,1))));
}
#[test]
fn parse_recovery_clears_only_the_parse_warning() {
    use crate::status::StatusTopic;
    let mut e = Editor::new_from_text("x\n", None, (40,6));
    // (a) An unrelated held Error must SURVIVE a ParseDegraded clear.
    e.set_status_full(StatusKind::Error, "unrelated", StatusLifetime::Sticky, StatusSource::Host, None);
    e.clear_topic(StatusTopic::ParseDegraded);
    assert_eq!(e.status().unwrap().text(), "unrelated", "clear_topic touches ONLY its named topic");
    // (b) A ParseDegraded warning IS cleared by its own topic.
    let mut e2 = Editor::new_from_text("x\n", None, (40, 6));
    e2.set_status_full(StatusKind::Warning, "markdown parse failed — styling may be stale",
                       StatusLifetime::Sticky, StatusSource::Host, Some(StatusTopic::ParseDegraded));
    e2.clear_topic(StatusTopic::ParseDegraded);
    assert!(!e2.has_visible_status(), "the parse-degraded warning is retired by its recovery clear");
}
```
Run → fails. **Step 2:** wire the conversions. Run → green. Commit `A17 T6: progress topics + parse-recovery clear_topic`.

---

## Task 7 — Esc dismiss of a held message (`input.rs::handle_key`)

**Files/Interfaces:** `input.rs::handle_key` — the Esc arm. Current precedence: pending-chord cancel >
filter-cancel. Add a THIRD branch: when neither fires and a HELD message is showing, dismiss it.

**Step 1 (failing test).** The behavior is `Editor::dismiss_status()` — the method the Esc arm calls.
Test it directly (fully runnable; `input.rs::handle_key` has no existing unit harness and needs a full
`(reg, keymap, ex, clock, msg_tx)` context, so the ARM is a one-line wiring verified by the method test
+ the existing Esc-precedence tests staying green):
```rust
#[test]
fn dismiss_status_clears_a_held_message_but_leaves_a_transient() {
    let mut e = Editor::new_from_text("x\n", None, (40, 6));
    // Held (Sticky) → dismissed.
    e.set_status_full(StatusKind::Error, "boom", StatusLifetime::Sticky, StatusSource::Host, None);
    e.dismiss_status();
    assert!(!e.has_visible_status(), "Esc dismisses a held message");
    // Transient → untouched by dismiss (the NEXT key clears it — today's behavior is preserved).
    e.set_status(StatusKind::Info, "t");
    e.dismiss_status();
    assert!(e.has_visible_status(), "dismiss leaves a Transient alone");
}
```
Run → fails (`dismiss_status` exists from T3, but this test also guards the Esc-arm wiring). **Step 2:**
in `handle_key`'s Esc arm, after the existing `pending_keys`/`filter_in_flight` branches, add
`else { editor.dismiss_status(); }` — preserving order so Esc still cancels a pending chord / filter
first. Run → green. Commit `A17 T7: Esc dismisses a held message`.

---

## Task 8 — `view_messages` command + read-only buffer guard

**Files/Interfaces:**
- `editor.rs`: `Buffer` gains `pub read_only: bool` (default `false` everywhere it is constructed;
  set `true` only for the history buffer). Grep every `Buffer { … }` literal / constructor and add the
  field (compiler-forced). Add `Editor::replace_buffer(slot, new) -> bool` (the category-(b) chokepoint,
  below) and route the 5 content-install sites through it (`save.rs` reload/recover, `session_restore.rs`
  open_into_current + restore_scratch, `app.rs` startup launch).
- **Read-only guard — the mutation surface is TWO provably-closed guarded categories, not an open set of
  callers (Codex-r5/r6 reframe).** A read-only buffer's content can change only two ways, each with a
  closed chokepoint: **(a) in-place content mutation** via the closed `Buffer` content-mutator set, and
  **(b) whole-buffer replacement** via a single `Editor::replace_buffer` method. Guarding both closes the
  surface with **zero open caller enumeration** — no "single `Buffer::apply` backstop" claim (that was
  incomplete: undo/redo and whole-buffer swaps bypass it).

  **CATEGORY (a) — in-place content mutators. Grep-verified complete set** (`impl Buffer`, `editor.rs`
  190–343; every `&mut self` method whose body writes `document.buffer`): exactly **`Buffer::apply`,
  `Buffer::undo`, `Buffer::redo`** (`from_text`/`from_file` are constructors; `invalidate_layout` is
  view-only). Guard each:
  ```rust
  // impl Buffer — the closed content-mutator set.
  pub fn apply(&mut self, txn, edit, kind, clock) { if self.read_only { return; }        /* …body… */ }
  pub fn undo(&mut self) -> bool { if self.read_only { return false; }                    /* …body… */ }
  pub fn redo(&mut self) -> bool { if self.read_only { return false; }                    /* …body… */ }
  ```

  **CATEGORY (b) — whole-buffer replacement. `Editor::replace_buffer` is the single chokepoint
  (Codex-r6).** Several sites swap a whole `Buffer` into a slot, bypassing category (a). Add ONE method
  and route them all through it:
  ```rust
  impl Editor {
      /// The ONE way to swap a whole Buffer into an existing slot. Read-only guard: if the buffer
      /// CURRENTLY at `slot` is read-only, no-op + Sticky Warning and return false; else replace + true.
      /// Callers MUST check the bool and skip their post-replace epilogue/ack on false.
      pub fn replace_buffer(&mut self, slot: usize, new: crate::editor::Buffer) -> bool {
          if self.buffers[slot].read_only {
              self.set_status_full(StatusKind::Warning, "buffer is read-only",
                                   StatusLifetime::Sticky, StatusSource::Host, None);
              return false;
          }
          self.buffers[slot] = new;
          true
      }
  }
  ```
  **Grep-verified complete set of whole-`Buffer` slot assignments** (`rg '\*.*active_mut\(\)\s*=|\.buffers\[[^]]+\]\s*='` minus `mod tests`; `buffers.push` is CREATION — appends a new slot, never overwrites an existing read-only buffer, so it is not in this category):
  - **Routed through `replace_buffer` (5 content-installs):** `save.rs:255` `reload_from_disk`
    (`*active_mut() = Buffer{id,..new_buf}`), `save.rs:302` `load_recovered` (same), `session_restore.rs:113`
    `open_into_current` (`buffers[a] = b`, reached via `workspace::open_as_new_buffer`'s reusable-throwaway
    branch, `workspace.rs:131`), `session_restore.rs:87` `restore_scratch` (`buffers[idx] = from_text`,
    startup), `app.rs:500` startup launch (`buffers[0] = b`). Each becomes
    `if !editor.replace_buffer(slot, new) { return; }` (see the ack note below).
  - **NOT routed — `workspace.rs:199` (close/dispose, last-ordinary-buffer reset to a fresh empty):** this
    is buffer DISPOSAL, not content-replacement — `read_only` gates content EDITS, never a buffer's
    CLOSE; the replacement is a fresh writable empty; and the read-only history buffer's content is a
    regenerable view (the ring is the source of truth), so a reset causes no data loss. It stays a direct
    assignment, classified here so the enumeration is complete and the exclusion is principled (a
    content-install vs. dispose split, not an ad-hoc omission).

  Together (a)+(b) are a CLOSED, enumerated surface: every content change of any buffer goes through a
  guarded `Buffer::{apply,undo,redo}` (in-place) or `Editor::replace_buffer` (whole-swap). Making a
  read-only buffer's content change is impossible via **every** path — keyboard edit, undo/redo, search
  replace/step, `diag_apply_selected`, filter/transform completion, plugin `wc.insert`/`wc.replace`
  (`plugin/api.rs` → `submit_transaction` → `editor.apply` → `Buffer::apply`), scratch, **reload / recover
  / open-into-current**, and any future path. Provable against two closed sets, not an open caller list.

  **Post-replace acks (Codex-r6 #3): callers CHECK the result — no false "Reloaded".** `reload_from_disk`
  sets `"Reloaded"` AFTER the swap (`save.rs:273`), with a long fold-carry/rebuild epilogue in between.
  Route it as `let prev_folded = …; if !editor.replace_buffer(editor.active, Buffer{id,..new_buf}) {
  return; } editor.active_mut().folds.replace_folded(prev_folded); … editor.status = "Reloaded".into();`
  — on a read-only buffer `replace_buffer` returns `false`, the caller returns early, and neither the
  epilogue nor the `"Reloaded"` ack runs; the Sticky Warning it set stays visible. Same early-return for
  `load_recovered` and `open_into_current`. (This is cleaner than relying on Q1 to suppress the ack,
  though Q1 would also hold since the Warning is Sticky.)

  **FEEDBACK — Status feedback, universal-by-construction via the delegators + Q1.** The three
  `Editor::{apply, undo, redo}` **delegators** (`editor.rs` 260/298/316 → `active_mut().{apply,undo,redo}`)
  are the path every INTERACTIVE mutation takes (keyboard edits, undo/redo, `textops`/`prose_ops`/
  `blocks_marked`/`scratch`-source via `editor.apply`, `diag_apply_selected` via `editor.apply`, plugin
  edits via `submit_transaction` → `editor.apply`). Guard each delegator to set the **Sticky Warning**
  and return before delegating:
  ```rust
  // impl Editor — the delegators (have status access; Buffer methods do not).
  pub fn apply(&mut self, txn, edit, kind, clock) {
      if self.active().read_only {
          self.set_status_full(StatusKind::Warning, "buffer is read-only", StatusLifetime::Sticky,
                               StatusSource::Host, None);
          return;
      }
      self.active_mut().apply(txn, edit, kind, clock);
  }
  // undo/redo: same guard, then `self.active_mut().undo()` / `.redo()`.
  ```
  Because the delegator sets the Sticky Warning, **Q1 auto-suppresses ANY caller's later success ack**
  ("Replaced N" / "block moved" / "applied") to history-only — no per-caller feedback set to keep
  complete. (Grounded: `Command::mutates()` and a `commands::run` guard are UNNEEDED and DROPPED — keyboard
  edits get content safety from category (a) and feedback from the `Editor::apply` delegator; Undo/Redo need
  no command-level mark, the `Buffer`/`Editor` undo/redo guards cover them.)

  **The bounded feedback gap — 3 direct-`Buffer::apply` success-reporters that bypass the delegator.**
  Grep-verified, the ONLY production callers of `active_mut().apply` (direct, not via the delegator) are
  `search_ui::search_replace_all` (`:55`), `search_step_apply` (`:76`), `search_step_rest` (`:110`) — each
  reports "Replaced N"/step success. (`diag_apply_selected` uses `editor.apply`, the DELEGATOR — so it is
  already covered by the delegator feedback, correcting the round-5 assumption; `jobs_apply`/`transform`
  `b.apply` are programmatic to the ORIGIN buffer, never the history buffer.) Add the Sticky-Warning reject
  as the first line of those **three** search fns so their ack is Q1-suppressed:
  ```rust
  if editor.active().read_only {
      editor.set_status_full(StatusKind::Warning, "buffer is read-only", StatusLifetime::Sticky,
                             StatusSource::Host, None);
      return;
  }
  ```

  **HONEST guarantee split + the one residual.** Content + status-feedback are UNIVERSAL: content by the
  two closed sets — in-place category (a) + replacement category (b); feedback by the delegators + the 3
  search guards + Q1. The ONLY residual is a **non-content, non-status epilogue side effect** — a handler
  that changes editor STATE (e.g. clears `marked_block`) *after* its `editor.apply` no-ops (`blocks_marked::
  block_move`, `scratch::move_block_to_scratch`). This is **cosmetic — no data loss, no false success**
  (category (a)/(b) already blocked the content change; the delegator feedback already suppressed the false
  ack). To keep
  even this clean, retain the `CommandMeta.mutates` + `register_mut` + `registry.dispatch` guard **scoped
  to the epilogue residual**, whose completeness is enforced mechanically (below). A path ever missed here
  costs at worst a cleared mark on a niche read-only buffer — never data loss or a false success.

  **`register_mut` set = whatever the completeness test demands (not a hand-list).** Add `mutates: bool`
  to `CommandMeta` (default `false`) + `register_mut`/`register_mut_stateful`; guard in
  `Registry::dispatch_with_arg` before the handler (`if ctx.editor.active().read_only &&
  self.entries[i].meta.mutates { set Sticky Warning; return Noop; }`). The set is DEFINED as "whatever
  makes the epilogue test pass" — the test is the source of truth; execution adds whatever it demands
  (e.g. `block_copy`, a direct `editor.apply` mutator omitted from earlier hand-lists — covered by
  construction). Illustrative: the `textops`, `prose_ops`-surgery, `blocks_marked` move/delete/copy,
  `scratch` move commands. `dispatch_filter`/`dispatch_transform` also take an entry guard so a filter/
  transform *started* on a read-only buffer neither schedules work nor runs its own epilogue (they cover
  `submit_filter_line` + `PromptAction::Transform` + `reflow`/`unwrap`); do NOT mark the `filter`/
  `transform` menu OPENERS (they only open a minibuffer/prompt — no epilogue).

  **Mechanical completeness test — the source of truth for the epilogue residual.** Iterate
  `reg.commands()`; for EACH command dispatch it against a **read-only ACTIVE buffer** with a
  `marked_block` set and known content, asserting **the ACTIVE buffer's content AND its `marked_block`
  are unchanged**. Any registry handler that runs a mutating epilogue clears `marked_block` and FAILS —
  forcing a `register_mut` (or entry guard) until green. **Scope, stated precisely:** the test guarantees
  no mutation OF THE ACTIVE (read-only) buffer — exactly the requirement (the history buffer is ALWAYS
  active while `view_messages` is open). It does NOT constrain writes to *other* buffers:
  `copy_block_to_scratch` appends to the **scratch** buffer (a different id, never read-only), so it is
  correctly out of scope and safe — not a claim that "no command mutates anything."
- A new thin `wordcartel/src/status_view.rs` (keeps `registry.rs`/`editor.rs` off the budget): renders
  `editor.status_history().entries()` into a fresh path-less `read_only` buffer and switches to it via
  the existing `workspace` switch seam; re-invoking regenerates (does not stack). Row format:
  `"[<kind>] <text>"`, plugin attribution `"[Info · plugin:<label>]"`, `"(×N) "` when `repeat > 1`.
- `registry.rs`: `r.register("view_messages", "Message History", Some(MenuCategory::View), |c| {
  crate::status_view::open(c.editor); CommandResult::Handled });`

**Step 1 (failing tests).** Cover the closed `Buffer`-mutator set (content), the delegator feedback (incl.
undo/redo), the search reporters, and the epilogue completeness sweep:
```rust
// editor.rs tests — CATEGORY (a): the closed Buffer content-mutator set no-ops on read-only.
#[test]
fn read_only_buffer_mutators_are_all_no_ops() {
    use wordcartel_core::change::ChangeSet;
    use wordcartel_core::history::{Transaction, EditKind};
    use wordcartel_core::block_tree::Edit;
    let mut e = Editor::new_from_text("abc\n", None, (40, 6));
    let clk = TestClock(std::cell::Cell::new(0));
    // First make an undoable edit while writable, then flip read-only.
    let doc_len = e.active().document.buffer.len();
    e.active_mut().apply(Transaction::new(ChangeSet::insert(0, "Z", doc_len)),
                         Edit { range: 0..0, new_len: 1 }, EditKind::Other, &clk);
    let baseline = e.active().document.buffer.to_string(); // "Zabc\n"
    e.active_mut().read_only = true;
    // Buffer::apply
    let dl = e.active().document.buffer.len();
    e.active_mut().apply(Transaction::new(ChangeSet::insert(0, "X", dl)),
                         Edit { range: 0..0, new_len: 1 }, EditKind::Other, &clk);
    assert_eq!(e.active().document.buffer.to_string(), baseline, "Buffer::apply no-op on read-only");
    // Buffer::undo / redo
    assert!(!e.active_mut().undo(), "Buffer::undo no-op on read-only");
    assert!(!e.active_mut().redo(), "Buffer::redo no-op on read-only");
    assert_eq!(e.active().document.buffer.to_string(), baseline, "undo/redo did not change content");
}

// editor.rs tests — FEEDBACK: the Editor delegators (apply/undo/redo) set the Sticky Warning.
#[test]
fn read_only_delegators_set_the_sticky_warning() {
    let mut e = Editor::new_from_text("abc\n", None, (40, 6));
    e.active_mut().read_only = true;
    e.undo();  // Editor::undo delegator
    assert_eq!(e.status_text(), "buffer is read-only");
    assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
}

// save.rs tests — CATEGORY (b): whole-buffer REPLACEMENT (reload) on a read-only buffer is refused,
// NOT replaced, and reports "buffer is read-only" — never a false "Reloaded".
#[test]
fn reload_on_read_only_buffer_is_refused_not_replaced() {
    // A real on-disk file so reload_from_disk has a path to read; then mark the active buffer read-only.
    let path = scratch(); // save.rs test helper → a temp path
    std::fs::write(&path, "disk contents\n").unwrap();
    let mut e = Editor::new_from_text("buffer contents\n", Some(path.clone()), (80, 24));
    let before = e.active().document.buffer.to_string();
    e.active_mut().read_only = true;
    crate::save::reload_from_disk(&mut e);
    assert_eq!(e.active().document.buffer.to_string(), before, "read-only buffer NOT replaced by reload");
    assert_eq!(e.status_text(), "buffer is read-only");
    assert_ne!(e.status_text(), "Reloaded", "must NOT report a false Reloaded");
    let _ = std::fs::remove_file(&path);
}

// commands.rs tests — keyboard edit routed via commands::run → Editor::apply delegator: no-op + feedback.
#[test]
fn read_only_buffer_rejects_keyboard_edits_with_a_message() {
    let mut e = Editor::new_from_text("abc\n", None, (40, 6));
    e.active_mut().read_only = true;
    let clk = TestClock(0);
    let before = e.active().document.buffer.to_string();
    run(Command::InsertChar('x'), &mut e, &clk);
    assert_eq!(e.active().document.buffer.to_string(), before, "read-only: keyboard edit is a no-op");
    assert_eq!(e.status_text(), "buffer is read-only");
}

// registry.rs tests — the EPILOGUE residual: block_move must NOT clear marked_block on a read-only
// buffer (content + status already covered by Guarantees 1/2; this forces the register_mut set).
#[test]
fn block_move_on_read_only_leaves_marked_block_and_content_intact() {
    let reg = Registry::builtins();
    let mut e = Editor::new_from_text("one two three\n", None, (40, 6));
    // Set a marked block over "one", then flip read-only.
    e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 3, hidden: false });
    e.active_mut().read_only = true;
    let before = e.active().document.buffer.to_string();
    let mark_before = e.active().marked_block;
    dispatch_id(&mut e, "block_move");   // registry.dispatch mutates-guard fires before the handler
    assert_eq!(e.active().document.buffer.to_string(), before, "read-only: no content change");
    assert_eq!(e.active().marked_block, mark_before, "read-only: marked_block NOT cleared (no epilogue)");
    assert_eq!(e.status_text(), "buffer is read-only");
    assert_ne!(e.status_text(), "block moved", "must NOT report false success");
}

// search_ui.rs tests — set (3): the Codex-named FALSE-SUCCESS path — no mutation AND no "Replaced N".
#[test]
fn search_replace_all_on_read_only_is_rejected_not_falsely_reported() {
    let mut e = Editor::new_from_text("aaa\n", None, (40, 6));
    e.active_mut().read_only = true;                 // the entry guard fires before any search work
    let clk = TestClock(0);
    let before = e.active().document.buffer.to_string();
    crate::search_ui::search_replace_all(&mut e, &clk);
    assert_eq!(e.active().document.buffer.to_string(), before, "no mutation on read-only");
    assert_eq!(e.status_text(), "buffer is read-only");
    assert_ne!(e.status_text(), "Replaced 3 occurrences", "must NOT report false success");
}

// filter/transform tests — set (3): the TRUE dispatch points (not the openers) reject on read-only,
// before any job is scheduled. `dispatch_transform` also covers the reflow/unwrap registry commands.
#[test]
fn dispatch_transform_on_read_only_is_rejected() {
    let mut e = Editor::new_from_text("hello world\n", None, (40, 6));
    e.active_mut().read_only = true;
    let clk = TestClock(0);
    let (tx, _rx) = std::sync::mpsc::channel();
    let before = e.active().document.buffer.to_string();
    crate::transform::dispatch_transform(&mut e, crate::transform::TransformKind::Reflow, None, &clk, &tx);
    assert!(!e.transform_in_flight, "no transform scheduled on a read-only buffer");
    assert_eq!(e.active().document.buffer.to_string(), before);
    assert_eq!(e.status_text(), "buffer is read-only");
}
#[test]
fn dispatch_filter_on_read_only_is_rejected() {
    // Drive through the real submit path (builds the spec, then calls dispatch_filter, whose
    // read-only guard fires before scheduling) — avoids constructing a private FilterSpec.
    let mut e = Editor::new_from_text("hello\n", None, (40, 6));
    e.active_mut().read_only = true;
    let (tx, _rx) = std::sync::mpsc::channel();
    crate::prompts::submit_filter_line(&mut e, "cat", &tx);
    assert!(e.filter_in_flight.is_none(), "no filter scheduled on a read-only buffer");
    assert_eq!(e.status_text(), "buffer is read-only");
}

// MECHANICAL completeness sweep — the source of truth for the register_mut set (EPILOGUE residual).
// Content-unchanged is already guaranteed universally by categories (a)+(b) (closed Buffer-mutator set +
// replace_buffer); the load-bearing assertion here is marked_block-unchanged, which fails for any
// registry handler that runs a mutating epilogue on a read-only buffer — forcing its register_mut.
#[test]
fn no_registry_command_runs_a_mutating_epilogue_on_a_read_only_buffer() {
    let reg = Registry::builtins();
    let ids: Vec<_> = reg.commands().map(|(id, _)| id).collect();
    for id in ids {
        let mut e = Editor::new_from_text("one two three\n", None, (40, 6));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 3, hidden: false });
        e.active_mut().read_only = true;
        let (content, mark) = (e.active().document.buffer.to_string(), e.active().marked_block);
        dispatch_id(&mut e, id.0);
        assert_eq!(e.active().document.buffer.to_string(), content, "{} mutated a read-only buffer", id.0);
        assert_eq!(e.active().marked_block, mark, "{} ran a mutating epilogue on a read-only buffer", id.0);
    }
}

// editor.rs (or status_view.rs) tests — the view itself.
#[test]
fn view_messages_opens_a_read_only_buffer_listing_history() {
    let mut e = Editor::new_from_text("x\n", None, (40, 6));
    e.set_status(crate::status::StatusKind::Info, "first");
    e.set_status_full(crate::status::StatusKind::Error, "boom",
        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
    crate::status_view::open(&mut e);
    assert!(e.active().read_only);
    let body = e.active().document.buffer.to_string();
    assert!(body.contains("first") && body.contains("boom"));
}
```
Run → fails (no `read_only` field; no guards; no `replace_buffer`; no `status_view`). **Step 2:** add the
`Buffer.read_only` field + the **category (a)** guards in `Buffer::{apply, undo, redo}` + the **category
(b)** `Editor::replace_buffer` method routing the 5 content-install sites (each `if
!replace_buffer(slot,new) { return; }`, dropping the raw `buffers[slot] =`/`*active_mut() =`) + the
**FEEDBACK** Sticky-Warning guards in the `Editor::{apply, undo, redo}` delegators + the three search-fn
feedback guards (`search_replace_all`/`search_step_apply`/`search_step_rest`) + `CommandMeta.mutates`/
`register_mut` + the `dispatch_with_arg` epilogue guard (mark the registry mutators the completeness test
demands — NOT the `filter`/`transform` openers) + the `dispatch_filter`/`dispatch_transform` entry guards
+ `status_view::open` + registry row. Do NOT add `Command::mutates()`/a `commands::run` guard — the closed
`Buffer` set + delegators already cover keyboard/undo/redo; do NOT route `workspace.rs:199` close-reset
through `replace_buffer` (dispose, not content-install). Run the completeness sweep and add `register_mut`
until green. Commit `A17 T8: view_messages + two-category read-only guard`.

---

## Task 9 — plugin migration: `wc.status` reroute + `wc.notify` + emit rate-limit

**Files/Interfaces:** `plugin/api.rs` (`install_status`; new `install_notify`), `plugin/host.rs`
(`Bridge`/`InvokeState` — read `current` label at emit; a per-source last-tick for the throttle),
`limits.rs` (`MESSAGES_EMIT_MAX_PER_TICK: usize = 1`). `wc.status` keeps its 4096-byte borrowed-bytes
cap (`crate::plugin::cap_status`).

- `wc.status(msg)` → `e.set_status_full(StatusKind::Info, capped, StatusLifetime::default_for(Info),
  StatusSource::Plugin { label }, None)` where `label = invoke_state.borrow().current.clone()`.
- `wc.notify(severity, msg)` (new): parse `severity` via `StatusKind::from_str`; unknown → a typed Lua
  error (`mlua::Error::runtime`), never a silent Info. Route to `set_status_full(kind, capped,
  default_for(kind), Plugin { label }, None)`.
- Emit rate-limit: at the plugin boundary, drop the **display-slot** update beyond
  `MESSAGES_EMIT_MAX_PER_TICK` per source-label per pump tick (history still records — coalesced by the
  ring dedup). Ground the pump tick against `plugin/pump.rs`; key the throttle on the label (label-less
  emits share one conservative bucket — a tighter, never-looser throttle; spec §9.3).

**Step 1 (failing tests).** Use the shipped `plugin/host.rs` test harness — `make(src, doc)` →
`(host, editor, id)`, driven by `host.pump_test(&editor)`, asserted via `editor.borrow()` (the existing
`wc.status` tests use exactly this; `.status` reads become `.status_text()` after the T3 migration):
```rust
use crate::status::{StatusKind, StatusLifetime, StatusSource};

#[test]
fn wc_status_still_works_as_a_plugin_tagged_info() {
    let (mut host, editor, _id) = make("wc.status('hi')", "x");
    host.pump_test(&editor);
    let e = editor.borrow();
    assert_eq!(e.status_text(), "hi");
    assert_eq!(e.status().unwrap().kind(), StatusKind::Info);
    assert!(matches!(e.status().unwrap().source(), StatusSource::Plugin { .. }));
}
#[test]
fn wc_notify_error_sets_a_sticky_error_from_a_plugin() {
    let (mut host, editor, _id) = make("wc.notify('error', 'compile failed')", "x");
    host.pump_test(&editor);
    let e = editor.borrow();
    assert_eq!(e.status().unwrap().kind(), StatusKind::Error);
    assert_eq!(e.status().unwrap().lifetime(), StatusLifetime::Sticky);
}
#[test]
fn wc_notify_unknown_severity_does_not_emit_a_silent_info() {
    // Unknown severity is a typed Lua error (wrapped in pcall so the plugin doesn't abort),
    // never a silent Info write.
    let (mut host, editor, _id) = make("pcall(function() wc.notify('bogus', 'x') end)", "y");
    host.pump_test(&editor);
    assert!(!editor.borrow().has_visible_status(), "unknown severity must NOT emit a silent Info");
}
```
Run → fails. **Step 2:** implement `install_status` reroute + `install_notify` + the boundary
rate-limit. Run → green. Commit `A17 T9: wc.status reroute + wc.notify + rate-limit`.

---

## Task 10 — `messages_min_kind` toggle + full command-surface wiring

**Files/Interfaces:**
- `config.rs`: `ViewConfig` gains `messages_min_kind: StatusKind` (default `Info`); `RawView` gains
  `messages_min_kind: Option<String>`; parse arm mirrors `status_line` (`"info"|"warning"` via
  `StatusKind::from_str`; unknown → a `warns.push` + default). Add `messages_min_kind_str(StatusKind)
  -> &'static str` (mirrors `transient_mode_str`; returns `"info"`/`"warning"`).
- `editor.rs`: `set_messages_min_kind(&mut self, k: StatusKind)` — the ONE setter (writes the field).
- `registry.rs`: three rows —
  - `r.register("messages_min_info", "Messages: Info & Above", None, |c| { c.editor.set_messages_min_kind(StatusKind::Info); CommandResult::Handled });`
  - `r.register("messages_min_warning", "Messages: Warnings & Errors Only", None, |c| { c.editor.set_messages_min_kind(StatusKind::Warning); CommandResult::Handled });`
  - `r.register_stateful("toggle_messages_verbosity", "Message Verbosity", Some(MenuCategory::View), |e| MenuMark::Value(match e.messages_min_kind() { StatusKind::Warning => "Warnings & Errors Only", _ => "Info & Above" }), |c| { let next = if c.editor.messages_min_kind() == StatusKind::Warning { StatusKind::Info } else { StatusKind::Warning }; c.editor.set_messages_min_kind(next); CommandResult::Handled });`
  Add a `messages_min_kind(&self) -> StatusKind` accessor.
- `app.rs`: seed at startup exactly like `set_status_line_mode(cfg.view.status_line)` —
  `editor.set_messages_min_kind(cfg.view.messages_min_kind);`.
- `settings.rs`: `SettingsSnapshot` gains `view_messages_min_kind: StatusKind`; add it to the two
  snapshot constructors (config-baseline + runtime); add it to the compile-time destructure in
  `every_persisted_setting_has_a_command` (no `..`) with the assertion (all THREE commands — Codex-r1 #4)
  `assert!(has("toggle_messages_verbosity") && has("messages_min_info") && has("messages_min_warning"), "view_messages_min_kind");`;
  mirror it in `compute_overrides` via `diff_key` + `messages_min_kind_str` (like `view_status_line`).

**Step 1 (failing tests).**
```rust
#[test]
fn verbosity_floor_hides_info_below_warning() {
    let mut e = Editor::new_from_text("x\n", None, (40,6));
    e.set_messages_min_kind(StatusKind::Warning);
    e.set_status(StatusKind::Info, "quiet");
    assert!(!e.has_visible_status(), "Info is below the Warning floor → history-only");
    assert_eq!(e.status_history().entries().len(), 1, "still recorded in history");
    e.set_status_full(StatusKind::Warning, "loud", StatusLifetime::Sticky, StatusSource::Host, None);
    assert_eq!(e.status_text(), "loud");
}
#[test]
fn toggle_flips_between_two_states() {
    // `dispatch_id(&mut Editor, &str)` is the registry-test helper (registry.rs tests), used exactly
    // as the shipped `toggle_status_line` test does.
    let mut e = Editor::new_from_text("x\n", None, (40, 6));
    e.set_messages_min_kind(StatusKind::Info);
    dispatch_id(&mut e, "toggle_messages_verbosity");
    assert_eq!(e.messages_min_kind(), StatusKind::Warning);
    dispatch_id(&mut e, "toggle_messages_verbosity");
    assert_eq!(e.messages_min_kind(), StatusKind::Info);
}
#[test]
fn set_per_state_primitives_set_the_floor_directly() {
    let mut e = Editor::new_from_text("x\n", None, (40, 6));
    dispatch_id(&mut e, "messages_min_warning");
    assert_eq!(e.messages_min_kind(), StatusKind::Warning);
    dispatch_id(&mut e, "messages_min_info");
    assert_eq!(e.messages_min_kind(), StatusKind::Info);
}
```
Plus the existing `settings::every_persisted_setting_has_a_command` and
`palette::palette_is_exhaustive_over_the_registry` must stay green (they now cover the new rows).
Run → fails. **Step 2:** implement config/setter/registry/app/settings. Run → green. Commit
`A17 T10: messages_min_kind option + command-surface wiring`.

---

## Task 11 — final gates

**Files/Interfaces:** `limits.rs` constant-stability test; whole workspace.

- `limits.rs` tests: assert the new constants (`MESSAGES_HISTORY_CAP == 256`, `MESSAGES_MAX_TEXT_LEN ==
  PLUGIN_MAX_STATUS_LEN`, `MESSAGES_DEDUP_WINDOW == 1`, `MESSAGES_EMIT_MAX_PER_TICK == 1`) — add a
  `messages_caps_are_stable` test alongside the existing `plugin_caps_are_sane` in `limits.rs` tests.
- `cargo test` (both crates) green; `cargo build` + `cargo test --no-run` warning-free for touched
  crates; `cargo clippy --workspace --all-targets` clean (any `#[allow]` item-local with a reason).
- Anti-regrowth gates (Codex-r1 #5): `wordcartel/tests/module_budgets.rs` budgets ONLY `app.rs`,
  `render.rs`, `timers.rs`, `plugin/host.rs`, `plugin/pump.rs` — there is **no** `editor.rs`/`registry.rs`
  file-size budget. The relevant gates for the new `editor.rs` methods are the `status.rs`/`status_view.rs`
  **module split** (keeps the model out of `editor.rs`) plus **per-function `clippy::too_many_lines`**
  (threshold 100). So: keep each new setter/method small; if `render.rs` grew from kind→style, confirm it
  stayed under its existing `module_budgets` budget (extract, do NOT bump). Do not add new budgets.
- Run `scripts/smoke/run.sh`; quote the one-line summary in the pre-merge report (mandatory-run /
  advisory-pass; a red result is surfaced, not a blocker).
Commit `A17 T11: constants + final gate pass`.

---

## Command-surface-contract conformance (MERGE GATE)

A17 adds the `view_messages` command and the `messages_min_kind` option. Conformance (spec §13):
- **Registry = single source of truth (law 1).** `set_status`/`finish_topic` are host *effects*, not
  settable options → not commands (like `save`'s internal status writes). The only settable option is
  `messages_min_kind`, which IS a command. The new `CommandMeta.mutates` flag + `register_mut` variant
  (T8) is a non-user-facing dispatch *attribute* on existing commands — it changes no command's identity,
  label, menu placement, or palette membership (`register_mut` mirrors `register`), so laws 2–7 and the
  palette/menu invariant tests are unaffected; it only lets `registry.dispatch` reject a mutating command
  on a read-only buffer.
- **Every option is a command (law 2).** `messages_min_kind` → `messages_min_info` +
  `messages_min_warning` (set-per-state, `menu: None`) + `toggle_messages_verbosity` (stateful menu rep,
  `MenuMark::Value`, `MenuCategory::View`). Enforced by the new line in
  `settings.rs::every_persisted_setting_has_a_command`.
- **Palette exhaustive (law 3).** All four new commands (`view_messages`, the two set-per-state, the
  toggle) are registered → they appear automatically; `palette::palette_is_exhaustive_over_the_registry`
  (and `_over_a_plugin_loaded_registry`) stay green.
- **Menu ⊆ palette (law 4).** Only `view_messages` and `toggle_messages_verbosity` carry a
  `MenuCategory::View`; both are registered commands. Set-per-state primitives are `menu: None`.
- **One setter per option; profiles too (law 6).** `set_messages_min_kind` is the sole mutator; the
  startup config-seed (`app.rs`) and all three commands call it.
- **Hints track the active keymap (law 7).** The new commands are palette-driven (no reserved chord);
  hints re-resolve via `keymap.chord_for` like every command — nothing keymap-specific to special-case.
- **2-state = toggle, not cycle (rule 8).** `messages_min_kind` is 2-state (≥Info / ≥Warning) → a
  **toggle** representative (mirrors the shipped `toggle_status_line`).

Invariant tests that must stay green: `settings::every_persisted_setting_has_a_command`,
`palette::palette_is_exhaustive_over_the_registry`, `palette::palette_is_exhaustive_over_a_plugin_loaded_registry`.

---

## Definition of Done
All 11 tasks committed; both crates' `cargo test` green; touched crates build/clippy clean; module
budgets green; the three command-surface invariant tests green; the F4 table fully applied (every prod
site's CASE + kind/lifetime/topic matches its row); smoke summary quoted. No `cargo fmt`. No core
changes. No new crates. No timer.
