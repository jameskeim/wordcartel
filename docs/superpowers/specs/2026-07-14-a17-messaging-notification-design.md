# A17 — Messaging / notification system: design spec

<!-- item: A17 -->

Status: draft for Codex spec gate. Author: Fable. Date: 2026-07-14.
Backlog: `A17` (needs-design, size M, ungated, first item in the "unify ad-hoc surfaces" arc; twin = `H21`).
Grounding memo (accepted): the scope-zero memo whose corrections are folded in below.

---

## 0. What A17 delivers (one paragraph)

Every user-facing message in `wcartel` flows through **one typed value and one setter** instead of 203
ad-hoc `editor.status = String` pokes. A17 introduces a `Status { kind: StatusKind, text, source,
lifetime }` value, a private field behind a single `Editor::set_status()` setter that owns
**severity-ranked slot precedence** (a lower-severity message can never silently clobber a higher one),
a **bounded in-memory history ring** every message appends to regardless of display outcome, a
`view_messages` command that opens the ring as a **read-only scratch-style buffer**, a migration of the
shipped `wc.status` plugin API plus a new severity-explicit `wc.notify`, one user-settable
**`messages_min_kind`** verbosity-floor option carried through the full command-surface contract, and
**kind→style** on the status bar that stays legible under `no-color`/`terminal-plain`. It adds no
concurrency, no framework, and no wall-clock timer.

---

## 1. Settled decisions (LAW — resolved in the 2026-07-14 brainstorm; not re-opened here)

- **F1 — the idle-is-free law means "no LEVEL-triggered clocks," not "no timers."** One-shot *armed*
  dwell deadlines (the `status_dwell`/`sb_dwell` rows in `timers.rs::SUBSYSTEMS`) are idle-safe and
  permitted. A17 v1 builds no timer, but the dwell-fade option is designed-not-built and legal.
- **Q1 = severity-ranked slot occupancy.** A new message takes the visible slot **iff its severity ≥
  the current occupant's**. Every message appends to history regardless of whether it took the slot.
  Errors (and Warnings — Q3) survive the next-keypress clear until dismissed or superseded.
- **Q2 = Progress is NOT a fifth kind.** `StatusKind` stays exactly LSP `MessageType`'s four
  (Error / Warning / Info / Log). Progress is an **orthogonal self-replacing lifetime flag** on
  `Status` — superseded by its own completion/failure message, collapsed in history.
- **Q3 = per-kind clear policy.** Info/Log clear on next input (today's behavior). Warning + Error hold
  until dismissed or superseded. **No new timer in v1.**
- **Q4 = history view is a read-only scratch-style buffer** (reuse `install_scratch`/`workspace`
  machinery). This deliberately avoids birthing a 12th bespoke overlay into the surface `H21` is about
  to unify; the ring is the source of truth, so a later overlay/lens view stays open post-`H21`.
- **Q5 = keep `wc.status(msg)` working** (routes to `set_status`, plugin-tagged source, defaults to
  Info, keeps the 4096-byte cap) and **add `wc.notify(severity, msg)`** with severity explicit in the
  signature. Emit-side rate-limit + dedup. (Plugin source is by invocation label, not a stable id —
  §3.5, per Codex-r1 #6.)
- **Q6 = ship exactly ONE option: `messages_min_kind`** (a verbosity floor: show ≥ Info vs ≥ Warning),
  with full command-surface-contract treatment. The fuller per-kind routing matrix is deferred.

Deferred, **designed-not-built** (enumerated in §12 so scope is explicit): sticky-with-actions tier;
full per-kind routing/verbosity/retention matrix; plugin-registered sinks/renderers; aggregated warning
indicator; history file-spill; the dwell-fade option; progress-as-`$/progress` channel.

---

## 2. Grounding — the real surfaces A17 touches

All anchors are **symbol names** (line numbers drift; re-locate via `documentSymbol`/grep). Verified
against the tree at spec time.

### 2.1 The debt itself — `wordcartel/src/**`
`rg '\.status\s*=' wordcartel/src` → **203 writes across 36 files**; heaviest `jobs_apply.rs` (23),
`registry.rs` (21), `commands/prose_ops.rs` (20), `blocks_marked.rs` (18), `prompts.rs` (12). Kind is
implied by wording only: `save.rs` writes `"Saving\u{2026}"` (progress), `"Reloaded"` (ack/info),
`e.to_string()` from a `SaveError` (error); `jobs_apply.rs` writes `"save failed (internal error:
{msg})"` (error) and `"filter discarded - buffer changed"` (warning). All land in the same flat
`String`, last-writer-wins — an error CAN be silently overwritten by a later info/progress message.
This is the bug A17's Q1 precedence rule fixes.

### 2.2 The field + the real clear-edge — `wordcartel/src/editor.rs`, `wordcartel/src/input.rs`
- `Editor` holds **`pub status: String`** (`editor.rs`), seeded `String::new()` in `Editor::new`. No
  accessor, no setter — poked directly from 36 files.
- **CORRECTION to the grounding packet:** the real production clear-edge is
  **`input.rs::handle_key`**, not `render.rs`. `handle_key` runs `editor.status.clear()` on: Esc with a
  pending chord; a resolved `Resolution::Command` (before dispatch); and `Resolution::None`
  fallthrough. So the *current* message lifetime is already "until the next keypress," already
  edge-triggered. The `render.rs` `ed.status.clear()` hits are **tests** (`auto_idle_hides_status_line_
  on_mode_paints_it`), not production clears. Other explicit clears: `marks.rs`, `theme_cmds.rs`
  (ellipsis-preview idiom `if status.ends_with('…')`), `derive.rs`, `chrome.rs`, `clipboard.rs`.

### 2.3 The bar-visibility cluster (orthogonal to message lifetime) — `editor.rs`, `chrome.rs`, `mouse.rs`
- **CORRECTION:** `status_reveal_due` / `status_hide_due` / `status_revealed` (in `MouseState`) +
  `status_line_mode: TransientMode` (in `Editor`, default `On`) govern whether the **idle info line
  paints in calm/Auto mode** — a *chrome-visibility* axis, NOT message lifetime. Armed by `mouse.rs`
  Moved arm; fired by `chrome.rs::recompute_status_line` (these ARE one-shot armed dwell deadlines —
  precedent for F1).
- The load-bearing interaction: **`chrome.rs::status_line_visible` force-reveals the bar whenever
  `editor.status` is non-empty** (documented no-silent-UI). Consequence A17 must own: a Warning/Error
  that holds (Q3) keeps the calm-mode bar visible until it is dismissed/superseded. This is correct
  (no-silent-UI) but must be a stated, tested behavior, not an accident.

### 2.4 The render/paint home — `wordcartel/src/render.rs`, `wordcartel/src/render_status.rs`
- `render.rs::paint_status` composes the bottom row. In the **normal branch** (no search/minibuffer/
  prompt overlay) it calls `render_status::status_left_text(editor)` and styles the whole row with
  `cs.menu_closed` (a `ChromeStyles` face). `render_status::status_left_text` returns
  `"{head} [{mode}] {status}"` (a `String`) — **no per-severity styling today**.
- Prompt/minibuffer/search branches already use a *distinct* accent style (`cs.ov_accent`). This is the
  precedent that a message kind may carry its own style span; A17 extends it to Error/Warning.

### 2.5 The seam patterns to imitate — `diag_provider.rs`, `timers.rs`, `registry.rs`
- `diag_provider.rs::ProviderSet` (`Vec<ProviderEntry>`, per-source `enabled`, `install`/`set_enabled`/
  `is_enabled`) is *A17-for-diagnostics* — the closest cousin for a multi-source registry.
- `timers.rs::SUBSYSTEMS` (`&[TimedSubsystem { name, deadline: fn }]`) is the fn-ptr-table seam; the
  designed-not-built dwell-fade would add one row here.
- `registry.rs` — `register`/`register_stateful`/`register_plugin`, `CommandMeta { label, menu, state,
  arg }`, `MenuCategory`, `MenuMark`. The palette is **exhaustive over `reg.commands()`**
  (`palette.rs::rebuild_rows`), so any registered command appears in the palette automatically.

### 2.6 The plugin emit path (a live-API migration, not greenfield) — `wordcartel/src/plugin/api.rs`
- `plugin/api.rs::install_status` binds `wc.status(msg)`: `e.status = crate::plugin::cap_status(&msg.
  as_bytes(), crate::limits::PLUGIN_MAX_STATUS_LEN)` (4096-byte cap on borrowed Lua bytes; documented
  "the only user-visible plugin output channel"). It pokes `editor.status` directly with no severity,
  no source tag, no throttle. A17 reroutes it through `set_status` and adds `wc.notify`.
- The plugin system (P1–P3) has **shipped** (`plugin/{api,host,load,pump,reload,settings}.rs`,
  `plugin_timer` in `SUBSYSTEMS`), so this is a migration under a back-compat constraint.

### 2.7 The command-surface / settings machinery — `registry.rs`, `config.rs`, `settings.rs`
- The model to copy for `messages_min_kind` is the **status-line-mode option** already wired end to end:
  `Editor::set_status_line_mode` (the one setter) ← commands `status_line_auto` / `status_line_on`
  (set-per-state, `menu: None`) + `toggle_status_line` (stateful menu representative, `MenuMark::Value`,
  `MenuCategory::View`); `config.rs::ViewConfig.status_line: TransientMode` (parsed from `[view]
  status_line`); `config.rs::transient_mode_str` (overrides round-trip); `settings.rs::SettingsSnapshot.
  view_status_line` + the `every_persisted_setting_has_a_command` recurrence guard (a compile-time field
  destructure with no `..`, plus a per-field `assert!(has("…"))`).
- `app.rs` seeds the editor from config at startup via `editor.set_status_line_mode(cfg.view.status_
  line)` — the same setter the command calls (law 6). `messages_min_kind` mirrors this seam exactly.

### 2.8 The scratch-buffer machinery for `view_messages` — `editor.rs`, `workspace.rs`, `scratch.rs`
- `Editor::install_scratch` creates the permanent path-less scratch buffer and records `scratch_id`;
  `workspace::goto_scratch`/`enter_scratch` switch to it. A17's history buffer reuses this *pattern*
  (a path-less buffer switched to by a command) — see §7 for the read-only nuance.

### 2.9 Theme completeness contract — `wordcartel-core/src/theme.rs`
- `SemanticElement` is an **exhaustive enum** (the constructors use exhaustive literals on purpose —
  no catch-all `_`). `Theme::face` matches every variant; `no_color()` / `terminal-plain` built-ins
  and the `no_color_is_monochrome_with_modifier_cues` test enforce that **every element is legible
  with modifier cues (bold/italic/underline/reverse/dim) and `Color::Default`, no color**. Any new
  severity face A17 adds must satisfy this — Error/Warning distinction must survive under `no-color`.

### 2.10 Caps — `wordcartel/src/limits.rs`
`pub const` caps live here (`PLUGIN_MAX_STATUS_LEN = 4096`, `PLUGIN_PUMP_CHAIN_CAP = 64`, etc.), each
asserted in `limits_are_stable`-style tests. A17's ring-size and rate-limit constants land here (§8).

---

## 3. The typed model

### 3.1 `StatusKind` — mirror LSP `MessageType`
```rust
/// User-message severity, mirroring LSP `window/showMessage` `MessageType`
/// (Error=1, Warning=2, Info=3, Log=4). Variant ORDER is most-severe first, so the
/// derived `Ord` gives `Error < Warning < Info < Log` — i.e. the MORE severe a kind,
/// the SMALLER it compares. The Q1 slot rule is therefore "candidate takes the slot
/// iff `candidate.kind <= occupant.kind`" (candidate at least as severe). This inversion
/// is load-bearing; see §4.1.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum StatusKind { Error, Warning, Info, Log }
```
- Do **not** describe this as "Error > Warning > Info > Log" — the derived `Ord` is the opposite
  (`Error < Warning < …`) and the whole precedence rule keys off it. (Codex-r1 #4.)
- Exhaustive; no catch-all in any match on it (house rule). Placed in the **shell** crate
  (`wordcartel/src/`), not core — it is a UI-surface type, and core (`#![forbid(unsafe_code)]`) has no
  status concept. Home module: a new `wordcartel/src/status.rs` (see §11).

### 3.2 `StatusLifetime` — the Q2/Q3 flag
```rust
/// Orthogonal to severity. `Transient` = clears on next input (Info/Log default).
/// `Sticky` = holds until dismissed or superseded (Warning/Error default).
/// `Progress` = held like Sticky BUT expected to be superseded by its own operation's
/// completion/failure (a `finish_topic` call naming the same `StatusTopic`, §3.4/§4.2)
/// and collapsed to that terminal state in history. Never traps: `Esc` dismisses it and
/// its completion always supersedes it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusLifetime { Transient, Sticky, Progress }
```
- **Default lifetime is derived from kind** at the setter unless the caller overrides: Info/Log →
  `Transient`, Warning/Error → `Sticky`. `Progress` is only ever set explicitly via `set_progress`
  (§4.2). This keeps the 203-site sweep mostly kind-only (§6): most sites pass just a kind and get the
  right lifetime for free.

### 3.3 `StatusTopic` — the correlation handle (fixes Codex-r1 #1, #2, and the round-2 blocker)
A topic identifies WHICH host state/operation a message is about, so a later message can (a) collapse a
Progress start into its own completion in history and (b) clear a specific held message without touching
an unrelated one. The round-2 blocker: a *static* topic only correlates when the operation is a **global
single-slot**. Codex verified that Filter/Transform are (each guarded by one editor-wide
`filter_in_flight`/`transform_in_flight`), but **Save allows same-op concurrency** (`do_save_to`
dispatches `JobKind::Save` with **no save-in-flight guard**, `save.rs`), so two Saves would share a
static `Save` and collapse the wrong history entry. The fix is an **instance-keyed** topic for the
concurrent case, derived from identifiers the start AND the completion **already carry** — no new token,
no job-plumbing.
```rust
/// Correlation handle. Two roles, both requiring exact start↔finish/clear identity:
///   • progress history-collapse (a "…" start replaced in-place by its own terminal message);
///   • targeted clear of a held state-indicator.
/// The instance-carrying variant (`Save`) is keyed on data present at BOTH ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusTopic {
    /// Save progress. Instance key = (buffer, document version). Both are captured at the start
    /// (`save.rs`: `buffer_id = active().id`, `v = active().document.version`, carried on the job) and
    /// reconstructed verbatim at completion (`jobs_apply.rs`: `r.buffer_id`, `r.version`). No guard →
    /// same-op concurrency IS possible, so a bare `Save` would be unsound; `(BufferId, u64)` is not.
    Save(crate::editor::BufferId, u64),
    /// Filter progress. Global single-slot: `Editor::filter_in_flight` (`editor.rs`), guarded at
    /// `filter.rs`, cleared at `jobs_apply.rs`. One at a time → static is sound.
    Filter,
    /// Transform progress. Global single-slot: `Editor::transform_in_flight`, guarded at
    /// `transform.rs`, cleared at `jobs_apply.rs`. One at a time → static is sound.
    Transform,
    /// Parse-degraded state indicator (NOT progress). Singleton: one `Editor::parse_degraded` flag
    /// editor-wide (`derive.rs`). Set with the "markdown parse failed …" Warning; cleared by the
    /// recovery arm via `clear_topic(ParseDegraded)`. Static is sound.
    ParseDegraded,
}
```
- `BufferId(pub u64)` derives `Copy, PartialEq, Eq` (verified), so the enum stays `Copy` and cheap to
  compare — the collapse/clear match is `O(1)`.
- **What is NOT here, and why (round-2 enumeration):** `Export` and `Diagnostics` were dropped — they do
  **not** emit self-replacing progress pairs (see §4.2's enumeration), so they need no correlation key.
  Extensible; plugin progress topics + a stable plugin id are deferred (§12) — v1 plugin emits carry
  `topic: None`.

### 3.4 `Status` — the one value
```rust
pub struct Status {
    kind: StatusKind,
    text: String,               // already length-capped at construction (see §8)
    lifetime: StatusLifetime,
    source: StatusSource,
    topic: Option<StatusTopic>, // correlation handle (§3.3); None for ordinary one-shot messages
    seq: u64,                   // monotonic emit counter (history ordering + dedup adjacency)
    repeat: u32,                // coalesced-repeat count for the dedup "(×N)" render (§5.2)
}
```
- Fields **private**; expose accessors (`kind()`, `text()`, `lifetime()`, `source()`, `topic()`,
  `repeat()`) per house "private-by-default + validated constructor" rule. Constructor caps `text`,
  stamps `seq`, sets `repeat = 1`.

### 3.5 `StatusSource` — host vs plugin (fixes Codex-r1 #6)
```rust
/// Where a message originated. There is NO stable plugin id in the shipped plugin
/// system — `plugin/host.rs::Bridge` holds only `InvokeState { current: Option<String>,
/// observer }`, an invocation LABEL captured at call time (the command/hook label), which
/// may be `None`. So plugin attribution is by that label, best-effort — honest about the
/// fidelity limit, never an invented id. A plugin emit is one more entry in the SAME queue;
/// never a shadowing field.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StatusSource { Host, Plugin { label: Option<String> } }
```
- The `label` is read from `InvokeState.current` at emit time. A stable `PluginId` (which would sharpen
  both attribution and per-source rate-limit scoping — §9.3) is a deferred refinement, not v1.

---

## 4. The setter — `Editor::set_status` (the single writer)

The field `Editor::status: String` is **replaced** by `Editor::status: Option<Status>` (private). All
203 sites route through **one** setter; the field is never poked directly again (enforced by making it
private — the compiler finds every site).

### 4.1 Signature + the Q1 precedence rule
```rust
impl Editor {
    /// The ONE writer for the user-message slot. Applies the Q1 severity-ranked
    /// occupancy rule to the DISPLAY slot and ALWAYS appends to history.
    pub fn set_status(&mut self, kind: StatusKind, text: impl Into<String>) { … }

    /// Full form when the caller needs a non-default lifetime, a topic, or a plugin
    /// source. `set_status` is `set_status_full(kind, text, default_lifetime(kind), Host, None)`.
    pub fn set_status_full(
        &mut self, kind: StatusKind, text: impl Into<String>,
        lifetime: StatusLifetime, source: StatusSource, topic: Option<StatusTopic>,
    ) { … }
}
```
Precedence algorithm (a pure `status::resolve_slot`, §11), applied against the current occupant
`self.status`:
1. Build the candidate `Status` (cap text §8, stamp `seq`, `repeat = 1`).
2. **Verbosity floor (Q6):** if `candidate.kind` is **strictly less severe** than
   `self.messages_min_kind` (i.e. `candidate.kind > messages_min_kind` under the §3.1 Ord — recall
   more-severe = smaller), the message is **history-only**: it does NOT take the slot (skip step 3). It
   still appends (step 4). (`Log` is always below an `Info` floor, so `Log` is history-only by default
   — stated in §13.1 so the option labels are honest.)
3. **Slot rule (Q1):** the candidate takes the slot iff **no current occupant**, OR `candidate.kind <=
   occupant.kind` (candidate at least as severe — Error is smallest under §3.1). No special Progress
   clause is needed: a completion message is always same-or-higher severity than its own "…" start
   (`Info` ack `≤ Info` progress; an `Error` failure `<` `Info` progress), so **plain Q1 already lets a
   completion replace its Progress start** in the slot. Otherwise the candidate is history-only.
4. **History append (§5)** — display outcome is irrelevant to *whether* we record; but a topic-carrying
   completion may *collapse* rather than append (§4.2 / §5.2).

> FLAG F2 (RESOLVED by Q1/Q3): "does error-persistence annoy more than it protects" is settled —
> Error+Warning are Sticky, bounded by history either way. Recorded as resolved; no open question.

### 4.2 Progress + its sound completion (`set_progress` / `finish_topic`) — fixes Codex-r1 #2 + round-2 blocker

**Step 1 — enumerate what actually emits self-replacing Progress in v1.** A correlation key is needed
ONLY for a message that (a) is set with `lifetime: Progress` (a "…" start) AND (b) is later
replaced-in-place in history by its own terminal message. Grounded against the tree, that is exactly
three sites — and, critically, two candidates are NOT in the set:

| op | progress start (real site) | terminal completion (real site) | pair? | concurrency | topic key |
|---|---|---|---|---|---|
| Save | `save.rs` `"Saving…"` | `jobs_apply.rs` (JobKind::Save) `"Saved"`/`"Saved v{v} …"`/`err` | **yes** | **same-op possible** (no guard) | `Save(BufferId, u64)` |
| Filter | `filter.rs` `"running {…}"` | `jobs_apply.rs` `filter discarded`/applied | **yes** | global single-slot (`filter_in_flight`) | `Filter` (static) |
| Transform | `transform.rs` `"{gerund}…"` | `jobs_apply.rs` `transform discarded`/applied | **yes** | global single-slot (`transform_in_flight`) | `Transform` (static) |
| Export | — (**no** `"Exporting…"` start; `export.rs::do_export` spawns straight to the subprocess) | `jobs_apply.rs` `"exported …"`/`"export … failed"` | **no** | n/a | **none** — terminal-only, Q1 handles it |
| Diagnostics | `diagnostics_run.rs` `"starting {source}…"` (a no-silent-wait echo) | **none** (the result updates the diag overlay, writes no paired status) | **no** | per-`DiagSource` | **none** — see below |

- **Export** writes only terminal messages (there is no progress "…" start), so ordinary Q1 precedence
  governs them; no topic, no collapse. Dropped from the progress set.
- **Diagnostics** `"starting harper…"` has **no paired status completion** — nothing to collapse
  against. It is not a progress *pair*; it is a transient echo that today clears on the next keypress
  (`input.rs`). So it maps to a plain `set_status(Info, …)` with **`Transient`** lifetime (preserving
  today's exact behavior), NOT `set_progress`. This sidesteps the per-`DiagSource` concurrency Codex
  flagged entirely: without a self-replacing progress pair there is no correlation to get wrong.
- **Save** is the one same-op-concurrent progress pair, hence the instance key `Save(BufferId, u64)`.

**Step 2 — the sound API.** `StatusTopic` (§3.3) is the correlation key; each start and finish names its
topic and (for Save) reconstructs the identical instance key from data it already holds.
```rust
impl Editor {
    /// Start a progress message for `topic`: Info-severity, Progress lifetime, Host, tagged `topic`.
    pub fn set_progress(&mut self, topic: StatusTopic, text: impl Into<String>) {
        self.set_status_full(StatusKind::Info, text, StatusLifetime::Progress,
                             StatusSource::Host, Some(topic));
    }
    /// Complete/fail progress for `topic`: apply Q1 to the display slot AND collapse the most-recent
    /// history entry carrying exactly this `topic` into the terminal state (replace-in-place, not
    /// append), so the log reads "Saved" not "Saving… / Saved". The topic is an EXACT-MATCH key, so a
    /// Filter finish can never collapse a Save entry, and a Save of buffer B/version 7 can never
    /// collapse the Save of buffer B/version 5.
    pub fn finish_topic(&mut self, topic: StatusTopic, kind: StatusKind, text: impl Into<String>) { … }
}
```
- **Save start** (`save.rs`): `set_progress(StatusTopic::Save(buffer_id, v), "Saving…")`, where
  `buffer_id = active().id` and `v = active().document.version` are already captured before dispatch and
  carried on the `JobKind::Save` job. **Save completion** (`jobs_apply.rs`): the reducer already destructures
  `(kind, version, buffer_id, class) = (r.kind, r.version, r.buffer_id, …)`, so it reconstructs the
  **identical** `StatusTopic::Save(r.buffer_id, r.version)` — no new field, no token, no plumbing. The
  save-panic arm (`apply_panic`) likewise has `buffer_id`/`version` in hand. Soundness of the key: it can
  never collapse a *different* operation's entry (different buffer or different version ⇒ different key).
  Two concurrent Saves of the same buffer whose versions differ (an edit intervened) get separate
  lineages; two with the **identical** `(bid, version)` (a rapid double-press with no edit between) are
  the *same* logical save of the *same* content, so coalescing their history entries is correct, not a
  wrong-entry collapse. Either way the finding's failure mode — a Save collapsing an unrelated Save — is
  impossible.
- **Filter/Transform**: `set_progress(StatusTopic::Filter, …)` / `Transform`; their completions
  (`jobs_apply.rs`, statically the filter/transform arms) call `finish_topic` with the same static
  topic. Sound because the editor-wide `filter_in_flight`/`transform_in_flight` guarantee one at a time.
- Display: `finish_topic` routes through the same `resolve_slot` (Q1) as any message — the completion is
  same-or-higher severity than the `Info` progress, so it takes the slot. If a *higher-severity* held
  Error is already showing (unrelated code), the `Info` ack correctly does NOT displace it; the ack
  still lands in history. Sound either way.
- Progress is **never Sticky-forever**: for the three pairs a `finish_topic` always arrives (every async
  job posts completion/failure through `jobs_apply.rs`), and `Esc` dismisses it (§4.3) if a job is ever
  lost. (Diagnostics' echo, now Transient, clears on next input as today.)

### 4.3 Clearing (Q3) — FOUR verbs, edge-triggered, no timer (fixes Codex-r1 #1 and #3)
The 203-site sweep is NOT two cases (set / clear-transient) but **four** distinct verbs. The F4 table
(§6.2) carries a *case column* so every site is classified and Codex can cross-check:

1. **set-a-message** — the common case: `set_status(kind, text)` / `set_progress(topic, text)` /
   `finish_topic(...)`.
2. **clear-transient** — the `input.rs::handle_key` next-key idiom (and the Esc-cancel / pending-chord
   sites in `input.rs`, `marks.rs`, `derive.rs`, `chrome.rs`, `clipboard.rs`): becomes
   `editor.clear_transient_status()`, which clears the slot **iff the occupant's `lifetime ==
   Transient`**. A held Warning/Error/Progress is NOT cleared by a keypress.
3. **clear-specific (topic-targeted)** — `derive.rs::apply_parse_result`'s recovery arm currently does
   `parse_degraded = false; editor.status.clear()` to retire the "markdown parse failed — styling may
   be stale" Warning (which is now **Sticky**, so `clear_transient_status` would leave it stuck — the
   exact bug Codex-r1 #1 flags). It becomes `editor.clear_topic(StatusTopic::ParseDegraded)`, which
   clears the slot **iff the occupant carries that topic** — never an unrelated held Error. (The failure
   arm sets `set_status_full(Warning, "markdown parse failed …", Sticky, Host, Some(ParseDegraded))`.)
4. **clear-all (reset)** — the **clear-by-assignment** sites `editor.status = String::new()`
   (`session_restore.rs`, `workspace.rs` ×4, `diagnostics_run.rs`) that reset the slot on a
   context change (session restore, buffer switch). These map to `editor.clear_status()` (unconditional
   clear), **not** to an empty `set_status(Info, "")` — Codex-r1 #3. (A blanket clear on a buffer switch
   is acceptable: the user changed context; nothing held is silently lost — it remains in history.)

- **Explicit dismiss (Esc):** the Esc arm in `handle_key` (when no pending chord / no filter-in-flight)
  gains a `editor.dismiss_status()` step that clears a **held** occupant (Sticky *or* Progress). This is
  the always-present escape hatch (no-silent-UI: never trapped). Ordered AFTER the existing
  pending-cancel / filter-cancel precedence so it only fires when nothing else claims Esc.
- No wall-clock/dwell clear in v1. The dwell-fade option (a `status_dwell`-style armed one-shot) is
  designed-not-built (§12), legal per F1.

### 4.4 Bar-visibility reconciliation (§2.3)
`chrome.rs::status_line_visible`'s `!editor.status.is_empty()` test becomes
`editor.status.is_some()` (or a helper `editor.has_visible_status()`). Behavior is preserved: a held
Warning/Error keeps the calm-mode bar revealed (no-silent-UI), and clears reveal when dismissed. This
is a **stated, tested** behavior (§10), not incidental.

---

## 5. The history ring

### 5.1 Shape
```rust
/// Bounded in-memory ring of recent user messages. M5 resource-cap ethos:
/// fixed capacity, oldest evicted, released on nothing (it is a fixed baseline).
pub struct StatusHistory {
    entries: VecDeque<Status>,   // cap = MESSAGES_HISTORY_CAP (§8)
}
```
- Lives on `Editor` as a private field `status_history: StatusHistory`. Methods: `push(Status)` (the
  dedup path §5.2), `collapse_topic(topic, Status)` (the progress-completion path §4.2 — replaces the
  most-recent entry carrying `Some(topic)` in place, else falls back to `push`), and `entries()` for the
  `view_messages` renderer. `push` evicts the front when at cap (`VecDeque::pop_front`).
- **Idle-is-free:** the ring is written only on an emit (`set_status_full`/`finish_topic` — an
  edit-class event), never on a timer or idle wake. It holds a fixed baseline of `MESSAGES_HISTORY_CAP`
  capped strings; no growth at rest. No file spill in v1 (§12).

### 5.2 Dedup (F3)
On `push`, if the incoming `Status` has the **same `(source, kind, text)`** as the ring's most-recent
entry **and** its `seq` is within `MESSAGES_DEDUP_WINDOW` of that entry's `seq` (i.e. immediately
repeated), **coalesce**: increment the existing entry's **`repeat: u32`** field (§3.4) instead of
appending. The `view_messages` render shows `"(×N) text"` when `repeat > 1`. This bounds a looping
plugin's log footprint (Fresh only dedup'd the file; we dedup the ring). Dedup is a ring concern; the
display-slot rate-limit is separate (§9.3).

---

## 6. The ~203-site sweep

### 6.1 Strategy
Making `Editor::status` private turns the sweep into a **compiler-driven** migration: every `editor.
status = X` and every `editor.status.clear()` fails to compile (`Option<Status>` has neither
assignment-from-`String` nor `.clear()`) until rewritten. The work is classifying each site into one of
the **four verbs** of §4.3 and, for set-a-message sites, assigning the right `StatusKind`/lifetime.

**Clear-by-assignment is a real, distinct case (Codex-r1 #3).** `editor.status = String::new()` appears
at `session_restore.rs`, `workspace.rs` (×4), and `diagnostics_run.rs` — these CLEAR, and must map to
`clear_status()`, NOT to `set_status(Info, "")` (which would show an empty Info line). The naive rule
"every `editor.status = X` → `set_status`" is wrong for exactly these; the F4 table's case column
catches them.

### 6.2 F4 — the classification RULES (the enumerated per-site table is a PLAN deliverable)
This section defines the classification **rules**; it is deliberately NOT the enumerated 203-row table.
Producing the exhaustive **per-site CASE table — one row for every one of the 203 `.status =`
assignment sites and 14 `.status.clear()` sites (counts Codex-confirmed) — is an F4 PLAN deliverable**,
the artifact Codex cross-checks site-by-site at plan-gate time. The spec commits the rules; the plan
commits the rows. Table columns: `file:symbol | current expr | CASE | StatusKind | lifetime | topic`,
where **CASE ∈ {set-message, clear-transient, clear-topic, clear-all}** (the §4.3 verbs). Rules:
- **CASE = clear-all:** `editor.status = String::new()` reset sites (`session_restore.rs`,
  `workspace.rs` ×4, `diagnostics_run.rs`) → `clear_status()`.
- **CASE = clear-topic:** `derive.rs::apply_parse_result`'s recovery `status.clear()` →
  `clear_topic(StatusTopic::ParseDegraded)` (its paired failure arm is a set-message with
  `topic = ParseDegraded`, Warning, Sticky). Any other paired state-clear discovered in the sweep gets
  its own topic here.
- **CASE = clear-transient:** the `input.rs::handle_key` next-key / Esc-cancel / pending-chord clears,
  and the analogous clears in `marks.rs`, `chrome.rs`, `clipboard.rs` → `clear_transient_status()`.
- **CASE = set-message**, then assign `StatusKind`/lifetime/topic from the wording already present:
  - "failed"/"error"/"cannot"/`SaveError`/`OpenError`/`EditError`/`.to_string()` of a typed error →
    **Error**, Sticky.
  - "discarded"/"cancelled"/"no file name"/"changed"/"unchanged, not saved" (recoverable, non-fatal) →
    **Warning**, Sticky.
  - Acks ("Saved", "Reloaded", "filter applied", "Recovered …", counts, mode confirmations) → **Info**,
    Transient.
  - A **self-replacing progress pair** — only Save/Filter/Transform (§4.2's enumeration): the "…" start
    → `set_progress(topic, …)` with the matching `StatusTopic` (`Save(BufferId, version)` / `Filter` /
    `Transform`); its completion/failure site → `finish_topic(topic, kind, text)`.
  - A **no-silent-wait echo with NO paired completion** — the diagnostics `"starting {source}…"` (and
    the `"cancelling…"` / chord-pending echoes) → `set_status(Info, …)` **Transient** (today's next-key
    behavior). NOT `set_progress` (no completion to collapse against — §4.2).
  - Export terminal messages ("exported …" / "export … failed") → ordinary **Info** / **Error** (there
    is no export progress start; Q1 precedence governs — §4.2).
  - Purely diagnostic/internal noise a user need not see live → **Log**, Transient (history-only under
    an Info floor by default).
- **Ambiguous sites flagged for human/Codex review:** `prompts.rs` feedback strings; the
  `theme_cmds.rs` ellipsis-preview idiom `if status.ends_with('…') { clear }` — a **transient UI echo**
  (`editor.status = "ctrl-k …"` set at chord-start, retired when the chord resolves): map it to
  `set_status(Info, …)` (Transient) + `clear_transient_status()`, preserving today's next-key retire.
  The pending-chord display (`input.rs`'s `format!("{} …", chords_display)`) is the same transient-echo
  shape. Any site whose string is a `format!` with runtime-variable severity is flagged for per-site
  review.

### 6.3 What does NOT change
Transient UI echoes (chord-pending, ellipsis preview) and Esc-cancel semantics keep their current feel
(Transient, cleared next key). The sweep is behavior-preserving for Info/Log sites and only *adds*
protection for Error/Warning sites (they now survive a clobber). No site changes *what* text a user
sees; it changes *whether a later message can erase it*.

---

## 7. `view_messages` — the history view (Q4)

### 7.1 Behavior
A registry command `view_messages` renders the ring into a **read-only, path-less buffer** and switches
to it (the scratch pattern, §2.8). Each entry renders one line: `"[<kind>] <text>"`, with best-effort
source attribution for plugin messages (`"[Info · plugin:<label>] …"` where `<label>` is the
`InvokeState.current` label, or a bare `plugin` when it was `None` — §3.5) and `"(×N)"` when
`repeat > 1`. Newest last (append order), so the reader lands at the bottom (most recent) — or newest
first; decide in plan against how `install_scratch` positions the caret.

### 7.2 Read-only guard — a REQUIRED new buffer-level field (Codex-r1 #8)
`scratch` is *editable* (`scratch.rs` appends by applying edits); there is **no `read_only` model field
today** (grep-confirmed). A message log must not be accidentally mutated, and re-invoking `view_messages`
must refresh deterministically — so the plan MUST wire a new guard; this is not optional. Chosen
mechanism (stated here so the plan implements it, not re-decides it):
- **A per-buffer `read_only: bool`** consulted by the edit-command entry (`commands::run`'s
  insert/delete/paste arms), which early-returns (a Noop, optionally a `set_status(Info, "read-only")`)
  when set. The check is a **single bool compare on the edit entry — `O(1)`, hot-path safe**. The
  history buffer is created with `read_only = true`; ordinary buffers default `false`.
- This is also the seed the deferred snapshots view (S3) and any future read-only lens will reuse — one
  honest guard rather than a per-view hack. (The weaker "throwaway editable buffer" alternative is
  rejected: it gives no real guarantee and lets a stray keypress corrupt the log.)

### 7.3 Refresh
`view_messages` re-invoked while already on the history buffer **regenerates** its contents from the
current ring (messages emitted since last view now appear). It does not stack buffers.

---

## 8. Constants (F3) — `wordcartel/src/limits.rs`

Conservative, tunable, each asserted in the `limits` stability test:
- `MESSAGES_HISTORY_CAP: usize = 256` — ring capacity. 256 capped-length strings is a trivial fixed
  baseline (M5 ethos) and far more history than a session needs live; bounded so a chatty plugin cannot
  grow memory.
- `MESSAGES_MAX_TEXT_LEN: usize = PLUGIN_MAX_STATUS_LEN` (4096) — the per-message text cap for ALL
  sources (host included), so a `format!` with a pathological payload cannot blow the ring. Host
  messages are short today; the cap is a guardrail, not a normal path.
- `MESSAGES_DEDUP_WINDOW: u64 = 1` — dedup only against the *immediately previous* entry in v1
  (coalesce a tight loop). Widening later is a constant bump.
- `MESSAGES_EMIT_MAX_PER_TICK: usize = 1` — display-slot throttle for the plugin emit path (§9.3): at
  most one slot update per plugin per pump tick; excess emits are history-only (dedup-coalesced). A
  conservative default; the plan confirms "tick" against the pump clock and may express it as an
  interval-ms instead — either way one tunable constant.
These are v1 values; each is a one-line change and named in the deferred "retention matrix" (§12) as
the seam a future user option would expose.

---

## 9. Plugin emit migration (Q5) — `wordcartel/src/plugin/api.rs`

### 9.1 `wc.status(msg)` — preserved, rerouted
`install_status` keeps its signature and the 4096-byte borrowed-bytes cap, but instead of
`e.status = cap_status(...)` it calls `e.set_status_full(StatusKind::Info, capped,
default_lifetime(Info), source, None)`, where `source = StatusSource::Plugin { label }` and `label` is
read from `InvokeState.current` at emit time (§3.5 — the invocation label, which may be `None`; there
is no stable plugin id). Same visible behavior for existing plugins; now typed, attributed
best-effort, and throttled.

### 9.2 `wc.notify(severity, msg)` — new, severity-explicit
```lua
wc.notify("error", "compile failed")   -- severity is REQUIRED in the signature (Fresh's day-one lesson)
```
- Severity arg is a **string** (`"error"|"warning"|"info"|"log"`), not an integer constant. Rationale:
  Lua plugins read better with named strings, it matches how `[view] status_line` and clipboard-provider
  options already round-trip as strings (`transient_mode_str`, `clipboard_provider` strings), and an
  unknown/omitted string maps to a typed error surfaced back to the plugin (not a silent Info). Provide
  a parse helper `StatusKind::from_str` mirroring `config.rs`'s `"off"/"auto"/"on"` arms.
- Routes to `set_status_full(kind, capped_text, default_lifetime(kind), Plugin { label }, None)`.
  Progress from a plugin is out of v1's `wc.notify` surface (a plugin uses `wc.status` for transient
  progress; a dedicated plugin-progress API + a plugin `StatusTopic` are deferred §12).

### 9.3 Emit-side rate-limit (F3)
A looping plugin (`while true do wc.status("x") end`-style, or a hot on-edit hook) must not repaint the
slot every frame. Throttle **on emit, at the plugin boundary**:
- Track the last allowed slot-update tick and drop excess: at most `MESSAGES_EMIT_MAX_PER_TICK` (§8)
  slot updates per plugin per pump tick; the newest wins on the next allowed tick. **History still
  records** every emit (subject to §5.2 dedup, which coalesces the repeats).
- **Scoping caveat (from §3.5):** with no stable plugin id, "per plugin" is keyed on the
  `InvokeState.current` **label**; emits whose label is `None` share one conservative bucket. This is a
  strictly *tighter* (safer) throttle than per-stable-id would be — it can only over-throttle
  label-less emits, never leak an un-throttled flood. A stable id would refine this later (§12).
- This lives at the emit boundary in `plugin/api.rs` / the pump, NOT in `set_status` (host must stay
  un-throttled). Ground the exact clock (pump tick vs `Clock::now_ms`) in the plan; the value is a
  tunable constant (§8).

> FLAG (runtime, cannot be settled by reading): the *exact* rate-limit unit and value (per-frame vs
> per-N-ms) depends on the pump's drain cadence and what "chatty" feels like in practice. The plan
> commits a conservative default and marks it tunable; a whole-branch probe (or a smoke check) is the
> place to validate it, not a source read.

---

## 10. Kind→style on the bar

### 10.1 Where
`render.rs::paint_status`'s normal branch chooses the row style. Today it is unconditionally
`cs.menu_closed`. A17 selects the style by the occupant's `kind`:
- **Error / Warning** → a distinct accent (Error strongest). Reuse the existing overlay-accent path the
  prompt/search branches already use (`cs.ov_accent`) or a dedicated severity face — decide in plan.
- **Info / Log** → the existing `cs.menu_closed` chrome face (unchanged look).

### 10.2 Theme-completeness conformance (GATE — §2.9)
If A17 adds **new** `SemanticElement` variants (e.g. `StatusError`, `StatusWarning`):
- `Theme::face` must match them (exhaustive — the compiler forces it).
- `no_color()` and `terminal-plain` must give them **modifier-only** cues (Error = bold+reverse,
  Warning = bold, say) so the severity is legible **without color** — extend
  `no_color_is_monochrome_with_modifier_cues` to assert it. **Never color alone.**
- Simpler alternative (recommended to keep the theme surface small): do NOT add `SemanticElement`
  variants; instead compose the bar style from **existing** faces + a modifier (Error = existing accent
  + `Modifier::REVERSED`/bold), the way `paint_status` already composes the ghost-hint span with
  `ChromeMuted` + `remove_modifier`. This keeps the theme completeness contract untouched while still
  giving a no-color-legible distinction. Decide in plan; if new variants are chosen, the completeness
  test extension is mandatory.

---

## 11. Module structure & anti-regrowth (GATE)

- New module **`wordcartel/src/status.rs`** owns `StatusKind`, `StatusLifetime`, `StatusTopic`,
  `StatusSource`, `Status`, `StatusHistory`, the `resolve_slot` precedence function, and `from_str`.
  Keeps the model out of the `editor.rs` god-object (H13) and gives the ~203-site sweep one import.
- `Editor::set_status`/`set_status_full`/`set_progress`/`finish_topic`/`clear_transient_status`/
  `clear_topic`/`clear_status`/`dismiss_status` are thin methods on `Editor` that **delegate** the
  precedence decision to a pure free function in `status.rs`
  (`fn resolve_slot(current: Option<&Status>, candidate: &Status, floor: StatusKind) -> SlotOutcome`) —
  no `Editor` borrow, unit-testable in isolation. The method just applies the `SlotOutcome` (take-slot
  vs history-only) and drives the ring (`push` / `collapse_topic`). This keeps `editor.rs` from growing
  a fat body (`too_many_lines`/`module_budgets` GATEs) and makes the Q1 rule + the topic-collapse rule
  table-testable without a full `Editor`.
- `view_messages` is a **thin command handler** (registry row) delegating to a `status_view.rs` (or a
  fn in `status.rs`) that renders the ring into the buffer — no inline body in `registry.rs::builtins`.
- The dwell-fade seam (deferred) would be **one `SUBSYSTEMS` row**, not a new dispatcher — named here so
  the plan does not grow a match later.

---

## 12. Deferred — designed-not-built (explicit scope boundary)

Named so scope is unambiguous; each has a seam left open, none is built in v1:
1. **Sticky-with-actions tier** — a `Status` carrying typed actions (`ViewLog`/`Dismiss`/`CopyToClip`/
   `Custom`, the Fresh `WarningDomain` model) rendered as an actionable notification; the `DiagOverlay`
   pattern is the host. Seam: `Status` gains an `actions` field later; v1 has none.
2. **Full per-kind routing/verbosity/retention matrix** — v1 ships only `messages_min_kind`. The
   per-kind route table (status-line vs history-only vs suppressed, per kind) is the deferred payload;
   the `§8` constants are the values it would expose.
3. **Plugin-registered sinks/renderers** — a plugin registering a *renderer* into a messaging seam
   (the noice property done right). v1 plugins only *emit*. Seam: a `ProviderSet`-shaped sink registry
   later.
4. **Aggregated warning indicator** — Fresh's `highest_level()` persistent chrome indicator. Deferred;
   the ring already has the data.
5. **History file-spill** — optional on-disk overflow beyond the ring. v1 is in-memory only.
6. **Dwell-fade transient option** — a `status_dwell`-style one-shot armed clear (legal per F1). v1
   clears Transient on next input only.
7. **Progress-as-`$/progress` channel** — a first-class progress API (determinate %); v1 models
   progress as a `Progress` lifetime + a `StatusTopic` (static for Filter/Transform, instance-keyed
   `Save(BufferId, version)` for Save — §3.3).
8. **Stable `PluginId` + plugin `StatusTopic`** — v1 attributes plugin messages by the
   `InvokeState.current` label (§3.5) and gives plugins no progress-topic. A stable id would sharpen
   attribution and per-source rate-limit scoping (§9.3); a plugin-facing topic would let plugin progress
   collapse in history. Both deferred; the `StatusSource`/`StatusTopic` enums are the seams.

---

## 13. Command-surface-contract conformance (MERGE GATE — `docs/design/command-surface-contract.md`)

A17 touches the status surface, adds a `view_messages` command, and adds one user-settable option
(`messages_min_kind`). It conforms as follows.

### 13.1 New commands
| id | label | menu | shape |
|---|---|---|---|
| `view_messages` | "Message History" | `View` | nullary, stateless |
| `messages_min_info` | "Messages: Info & Above" | `None` (palette-only) | set-per-state |
| `messages_min_warning` | "Messages: Warnings & Errors Only" | `None` (palette-only) | set-per-state |
| `toggle_messages_verbosity` | "Message Verbosity" | `View` | stateful representative (`MenuMark::Value`) |

- `messages_min_kind` is a **2-state option in v1** (`Info` floor vs `Warning` floor). Per
  command-surface-contract rule 8, a **2-state** option's menu representative is a **toggle**, not a
  cycle (Codex-r1 #7). So: set-per-state primitives `messages_min_info` / `messages_min_warning`
  (`menu: None`) **plus** `toggle_messages_verbosity` (`menu: View`, `MenuMark::Value` showing
  "Info & Above" / "Warnings & Errors Only"). This mirrors the shipped `toggle_status_line` exactly
  (a 2-way toggle carried with `MenuMark::Value` state-in-label). **Deliberate scope choice:** we ship
  **2 states, toggle** — NOT a 3rd `Error`-only state with a cycle (that is deferred to the per-kind
  matrix, §12). Flagged to the human as the small scope call it is; if a 3rd state is wanted, it
  becomes a cycle + a third set-per-state command, a bounded change.
- **Label honesty (Codex-r1 #9):** the `Info` floor shows Error/Warning/Info but **excludes `Log`** —
  `Log` is history-only by design (§4.1 step 2, §2 imagined). Hence "Info & Above," never "Show All":
  `Log` is intentionally never on the bar, only in `view_messages`.

### 13.2 One shared setter (law 6) — `Editor::set_messages_min_kind`
- All three commands and the startup config-seed call **one setter** `Editor::set_messages_min_kind
  (StatusKind)` (the `set_status_line_mode` precedent). `app.rs` seeds it from config exactly as it
  seeds `set_status_line_mode(cfg.view.status_line)`. No bypass.

### 13.3 Persistence (law 2) — the recurrence guard
- `config.rs::ViewConfig` gains `messages_min_kind: StatusKind` (parsed from `[view] messages_min_kind`
  = `"info"|"warning"`, default `Info`), with a `messages_min_kind_str` round-trip helper mirroring
  `transient_mode_str`.
- `settings.rs::SettingsSnapshot` gains `view_messages_min_kind`; the compile-time field destructure in
  `every_persisted_setting_has_a_command` (no `..`) forces a new assertion line:
  `assert!(has("toggle_messages_verbosity") && has("messages_min_warning"), "view_messages_min_kind")`.
- `settings.rs::compute_overrides` mirrors it via the `diff_key` + `messages_min_kind_str` path, exactly
  like `view_status_line`.

### 13.4 Law-by-law
1. **Registry = single source of truth.** All A17 state changes (`set_status`, verbosity setter) are
   reached only through registry commands or the plugin emit API (which is itself a registered surface);
   `set_status` is called by host code, not a command — it is an *effect*, not a settable option, so it
   is not a command (like `save`'s internal status writes today). The only settable option is
   `messages_min_kind`, which IS a command (law 2). ✔
2. **Every user-settable option is a command.** `messages_min_kind` → three commands + shared setter +
   recurrence-guard assertion. ✔
3. **Palette exhaustive.** `view_messages`, `messages_min_info`, `messages_min_warning`,
   `toggle_messages_verbosity` are registered → they appear automatically (`palette.rs::rebuild_rows`
   is `commands()` over the whole registry; the palette-completeness test covers them). ✔
4. **Menu ⊆ palette.** `view_messages` and `toggle_messages_verbosity` are the only menu rows (both
   `View`); both are registered commands. The set-per-state primitives are `menu: None`. ✔
5. **Every mouse affordance has a keyboard path.** Falls out of law 3; no A17 mouse-only affordance. ✔
6. **One setter per option; profiles use it too.** `set_messages_min_kind` is the sole mutator;
   startup-seed and all three commands call it. ✔
7. **Hints track the active keymap.** The palette/menu hint for these commands re-resolves via
   `keymap.chord_for` like every other command; no default binding is asserted (they are palette-driven
   in v1, no reserved chord), so there is nothing keymap-specific to special-case. ✔

`view_messages` is **not** a settable option (it is an action), so it needs no setter/persistence — it
is a plain nullary command (like `goto_scratch`).

---

## 14. Project-law conformance

- **No silent UI waits.** Every message routes to the status line (the app owns the terminal); errors
  are *more* visible than today (Sticky, un-clobberable). Held messages force the calm-mode bar visible
  (§4.4). The Esc dismiss (§4.3) guarantees the user is never trapped.
- **Idle is free / edge-triggered.** The ring and slot are written only on emit (an edit-class event).
  No wall-clock/idle timer in v1; the input loop still BLOCKS at rest. The bar-reveal cluster's
  one-shot armed dwells are pre-existing and idle-safe (F1).
- **`#![forbid(unsafe_code)]` core.** The whole model lives in the **shell** (`wordcartel/src/status.rs`);
  core is untouched. No unsafe.
- **Dependency weight.** Zero new crates — a `VecDeque` ring, an enum, and a pure function. No
  `tracing`, no actor/broker framework (the build-not-buy lock).
- **Hot path.** `set_status`/`finish_topic` are `O(1)` (a severity compare + a `VecDeque` push/pop, or
  a bounded scan-back for `collapse_topic`/dedup against at most a small window — v1 collapses/dedups
  only against the most-recent entries, not the whole ring). The read-only edit-guard (§7.2) is a
  single bool compare on the edit entry. `paint_status` gains one `match kind` — O(1). No `O(document)`
  work.

---

## 15. Sizing (F5)

**M**, as backlogged. The bulk is the mechanical 203-site sweep (compiler-driven, behavior-preserving
for Info/Log), plus a compact new module, the history buffer plumbing, the plugin-API migration, and
one fully-wired option. What would push it to **L**: shipping the sticky-with-actions tier, the full
per-kind routing matrix, or plugin-registered renderers in v1 (all deferred §12); or if the read-only
buffer mechanism (§7.2) turns out to need broad edit-path changes rather than a single guard bool.

---

## 16. Open grounding tasks for the plan (not blockers)

1. Plugin attribution is by `InvokeState.current` label (§3.5, resolved) — the plan sites exactly where
   the label is read at the `wc.status`/`wc.notify` boundary and the fallback when it is `None`.
2. The exact edit-command entry points that must honor the new per-buffer `read_only` guard (§7.2) —
   enumerate the insert/delete/paste arms in `commands::run` (confirmed no existing `read_only` field).
3. The pump/tick clock for the emit rate-limit unit (§9.3).
4. Whether kind→style adds `SemanticElement` variants or composes existing faces (§10.2) — recommend
   compose-existing to keep the theme contract untouched.
5. Caret/scroll position on `view_messages` open (newest-first vs land-at-bottom) against how
   `install_scratch` seeds the buffer (§7.1).
6. The precise `finish_topic` completion/failure sites in `jobs_apply.rs` — the Save arm (keyed
   `Save(r.buffer_id, r.version)`), the filter arm (`Filter`), the transform arm (`Transform`), and the
   save-panic arm in `apply_panic` (§4.2). Export/Diagnostics are NOT in this set (§4.2 enumeration).
