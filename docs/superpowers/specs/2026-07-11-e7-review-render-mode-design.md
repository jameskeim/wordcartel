# E7 — Review render mode: grammar/spelling as a deliberate view — design spec

**Date:** 2026-07-11
**Status:** spec for review (Codex gate)
**Verified against:** `main` @ `8d00a4e` (post-curation-merge). All symbol names below were
grep/LSP-verified against this tree; anchors are symbol names, never line numbers.
**Approved design inputs:** the E7 decisions ledger (all six forks settled) + the Phase-0 surface map.

---

## 0. Summary and the product change

Wordcartel today runs Harper spelling/grammar diagnostics on a 400 ms debounce after **every
edit, in every render mode**, and paints the resulting underlines in **every render mode**. E7
turns diagnostics into a deliberate act: a fourth render mode, **`RenderMode::Review`**, that
renders exactly like LivePreview (rendered prose) and is the **only** mode in which diagnostics
are computed or shown.

**This flips the default experience** — from "live squiggles while drafting" to "quiet
drafting; diagnostics when you ask for them by entering Review." That flip is the intended
product change (decisions ledger F5, confirmed). It is delivered structurally: every buffer
starts in `RenderMode::LivePreview` (the `View` constructor in `wordcartel/src/editor.rs`;
render mode is not config-seedable — `config.rs` has no render-mode key), and with E7 no
diagnostics arm or paint happens outside Review. `DiagnosticsConfig::default()` keeps
`enabled: true, grammar: true, debounce_ms: 400` (`wordcartel/src/config.rs`) — the feature
stays on by default; only *when it runs* changes.

Scope is **seq-1 only**: the embedded Harper backend is unchanged — it merely moves off the
live path. See §5.

---

## 1. The `Review` render mode

### 1.1 The variant

`RenderMode` (`wordcartel/src/editor.rs`) gains a fourth variant:

```rust
pub enum RenderMode { LivePreview, SourceHighlighted, SourcePlain, Review }
```

The enum keeps its existing derives (`Clone, Copy, PartialEq, Eq, Debug`; no `Default` — the
`View` constructor supplies `LivePreview` explicitly). Render mode remains **per-buffer** state
(`View::mode` on `Buffer::view`), and `Buffer` already holds its own
`diagnostics: diagnostics_run::DiagStore` — mode and diagnostics store are both per-buffer, so
no cross-buffer plumbing is needed.

`Review` is an **unconditional** variant: it exists in the enum and the cycle regardless of
`diag_cfg.enabled`. With `enabled = false` (user opted out of checking entirely), Review still
renders — it just shows no diagnostics (F5). No conditional variant, no feature gating of the
mode itself.

### 1.2 Every RenderMode site (sweep-verified: the production sites below are the complete set)

A full-tree grep for the `RenderMode` token confirms the surface map's enumeration —
production references to the type name live only in `editor.rs`, `lines.rs`, `render.rs`,
`render_status.rs`, `derive.rs`, `commands.rs`, and `registry.rs`. `nav.rs::layout_line_on_demand`
is also part of the surface — it consumes the mapping via `line_render_for(view.mode, …)`
without naming the type, so a raw token-grep misses it (covered as site 5 below). `input.rs`
has no production render-mode surface at all: both its translation helpers (`key_to_command`,
`key_to_command_id`, which carry the F1/ctrl-\ rows) are `#[cfg(test)]`, and production key
routing goes through `input.rs::handle_key` + the `KeyTrie`. Site by site:

1. **`lines.rs::line_render_for(mode, is_active_line) -> LineRender`** — the ONE exhaustive
   `match RenderMode` in the tree; adding the variant is a compile error until handled.
   **Review mirrors LivePreview exactly** (ledger F1 = A — a prose lens, not a source view):
   the caret line renders `RawPlain`, every other line `Concealed` (rendered). Concretely the
   `LivePreview` arm becomes `LivePreview | Review => if is_active_line { RawPlain } else
   { Concealed }`. The function's doc comment (which currently enumerates the three modes'
   mappings) is updated to name all four.

2. **`render.rs::gather_row_ctx` — the non-exhaustive `plain_source` equality.** The site is
   `let plain_source = editor.active().view.mode == crate::editor::RenderMode::SourcePlain;`.
   This is an `==` comparison, not a match, so the compiler will NOT force a decision here —
   the spec makes it explicitly: **Review must NOT be treated as plain-source**, and the
   equality already delivers that (`Review != SourcePlain` → the styled/"colored" branch),
   which is precisely correct for a mode that mirrors LivePreview. **No code change at this
   site**; it is listed so the reviewer sees it was considered, and it gets a pinning test
   (§7.1) so a future refactor of `plain_source` cannot silently absorb Review.

3. **`render_status.rs::status_left_text`** — the `[MODE]` label match over `view.mode`
   (currently `PREVIEW` / `SRC-HI` / `SOURCE`) gains `RenderMode::Review => "REVIEW"`, i.e.
   the status line shows **`[REVIEW]`** (ledger F6).

4. **`derive.rs` — `LayoutKey.mode: RenderMode`** is already a key field (`mode: b_mode` where
   `b_mode = b.view.mode` is extracted before the key is built), and `rebuild` already calls
   `line_render_for(b_mode, l == active_line)` per line. A mode change is already a tracked
   rebuild trigger. **No change** — `Review` participates automatically because `RenderMode`
   is `Copy + Eq`.

5. **`nav.rs::layout_line_on_demand`** — calls `line_render_for(view.mode, …)`; picks up
   Review for free once the match arm exists. No change.

6. **`commands.rs::Command::CycleRenderMode`** and **`registry.rs`'s `cycle_render_mode`
   registration** — reworked in §3 (new cycle order, shared setter, stateful promotion).

7. **`input.rs::key_to_command_id`** — the legacy key-translation table is
   `#[cfg(test)]`-retired ("Retired from production use in Task 4"); production key routing
   goes through the `KeyTrie` built from `keymap.rs`'s static preset tables. **No production
   change in `input.rs`.** (Its test-only F1/ctrl-\ rows keep working — they name the
   `cycle_render_mode` id, which survives.)

**No new diagnostic-rendering code anywhere.** The underline paint already exists and is
RenderMode-independent: `render.rs::gather_row_ctx` computes
`diag_active = editor.active().diagnostics.valid_for(editor.active().document.version)`, feeds
the diagnostic slice into the row-span builder, and paints `SE::DiagSpelling` /
`SE::DiagGrammar` faces per overlapping glyph. E7 only *gates* that computation (§2); the
whole problem is scheduling/gating, not rendering.

`wordcartel-core` is RenderMode-agnostic (zero references) — the entire effort is shell-side.

---

## 2. Draft-quiet diagnostics gating

### 2.1 The two helpers

Two public helpers in `wordcartel/src/diagnostics_run.rs`, beside `DiagStore`/`diag_due`:

```rust
/// Compute gate: diagnostics are armed/dispatched only when the user has the feature
/// enabled AND the active buffer is in the Review render mode.
pub fn should_run_diagnostics(editor: &Editor) -> bool {
    editor.diag_cfg.enabled && editor.active().view.mode == crate::editor::RenderMode::Review
}

/// Display gate: underlines paint under exactly the same predicate. A distinct name for
/// the distinct role (compute vs paint); delegates so the two cannot drift.
pub fn should_show_diagnostics(editor: &Editor) -> bool { should_run_diagnostics(editor) }
```

`diag_cfg.enabled` stays the user's **master** "do I want checking at all" switch;
`diag_cfg.grammar` is untouched (it is a *content* filter reaching only
`dispatch_diagnostics`'s `CheckOpts`, orthogonal to *when* checks run). No struct changes to
`DiagnosticsConfig`.

### 2.2 The gated sites — compute

`should_run_diagnostics` **replaces** the bare `diag_cfg.enabled` check at the arm/dispatch
sites below (all verified in source; all currently test `editor.diag_cfg.enabled` only, never
render mode). Arming falls into **two distinct trigger classes** — (A) any *document edit*
(a `document.version` delta), covered by ONE unified seam; and (B) the two diagnostic-relevant
*non-edit* state changes (adding a word to the ignore-set / the dictionary), which do not touch
`document.version` and so are covered by their own explicit re-arms. This is a *seam* design,
not a per-path enumeration: class A arms uniformly regardless of which code path (or future
interceptor) performed the edit.

1. **The unified edit-arm seam — an `(active BufferId, document.version)`-delta check wrapping
   ALL of `reduce` (class A).** Verified structure of `app.rs::reduce`: it captures
   `let before = editor.active().document.version;` **only after all eleven overlay/modal
   interceptors** (`splash`, `marks`, `menu`, `palette`, `theme_picker`, `file_browser`,
   `prompts`, `minibuffer`, `search_ui`, `diag_overlay`, `outline_overlay`), each of which
   returns `Handled::Done(k) => return k` **before** that snapshot. So an edit performed *inside*
   an interceptor is invisible to the current epilogue's `version != before` arm even in
   principle — the snapshot is taken after the mutation. Three interceptor families edit and
   early-return this way (verified): `diag_overlay.rs::intercept` (quick-fix suggestion →
   `search_ui::diag_apply_selected`), the search overlay `search_ui.rs::intercept`
   (`search_replace_all` / `search_step_apply` / `search_step_rest`), and
   `prompts.rs::intercept` (a prompt-held `Msg::FilterDone` / `TransformDone` / `ClipboardPaste`
   → `jobs_apply::apply_filter_done` / `apply_transform_done` / `apply_clipboard_paste` →
   `Buffer::apply`). Enumerating them is whack-a-mole; the fix is a single seam.

   **The seam keys on buffer IDENTITY as well as version** — arming on a bare `version` delta
   would falsely fire on a **buffer switch** (activating buffer B at v1 from buffer A at v0 is a
   version change on the active slot, but no edit occurred), contradicting §2.3's rule that
   activating a stale buffer does not auto-arm. So the seam snapshots **both** the active
   `BufferId` and its `document.version`, and arms iff the active id is **unchanged** AND its
   version **increased**. A switch (active id changed) → **no arm** — becoming Review-active is
   refreshed only by the arm-on-enter in `set_render_mode` (§2.4), the single becoming-active
   refresh path. (`BufferId` is `Copy, PartialEq, Eq`; `editor.active().id` is the accessor.)

   **Design:** capture the snapshot at the **top** of `reduce` (before the first interceptor),
   extract the current interceptor-chain-plus-match body into an inner `reduce_dispatch(…)`
   (same parameters), and after it returns call one shared helper on the way out:

   ```rust
   pub fn reduce(msg, editor, reg, keymap, ex, clock, msg_tx) -> bool {
       // (the #[cfg(debug_assertions)] smoke-panic trigger stays the first statement)
       let before_id = editor.active().id;
       let before_version = editor.active().document.version;
       let keep = reduce_dispatch(msg, editor, reg, keymap, ex, clock, msg_tx);
       crate::diagnostics_run::arm_if_edited(editor, before_id, before_version, clock);
       keep
   }
   ```

   with the helper co-located beside `should_run_diagnostics` in `diagnostics_run.rs`:

   ```rust
   /// The single diagnostics re-arm seam. After a `reduce` message, if the SAME buffer is still
   /// active AND its document.version advanced since the snapshot, arm the debounced recheck —
   /// but only when in Review with checking enabled. Wrapping ALL of reduce's exit paths (every
   /// interceptor early-return AND the normal dispatch tail), it re-arms every active-buffer edit
   /// uniformly, exactly once, with no per-path enumeration, no double-arm, and no false arm on a
   /// buffer switch (§2.3).
   pub fn arm_if_edited(editor: &mut Editor, before_id: crate::editor::BufferId,
       before_version: u64, clock: &dyn wordcartel_core::history::Clock) {
       if editor.active().id == before_id
           && editor.active().document.version != before_version
           && should_run_diagnostics(editor)
       {
           let debounce_ms = editor.diag_cfg.debounce_ms;
           editor.active_mut().diagnostics.arm(clock.now_ms(), debounce_ms);
       }
   }
   ```

   The old inline arm block (the `if editor.diag_cfg.enabled { …diagnostics.arm(…) }` in the
   epilogue) is **removed** — `arm_if_edited` subsumes it. The epilogue's `last_edit_at` update
   stays exactly where it is (its own post-interceptor `before`, normal-path only) — untouched,
   so swap-subsystem timing is unchanged and out of scope. Because `arm_if_edited` sees every
   return path, it covers direct commands, undo/redo (which change version without going through
   `Editor::apply`), paste, transforms, AND all three interceptor edit families above — and any
   future editing interceptor — arming **exactly once** per edit to the **active** buffer, and
   never on a switch. The mouse quick-fix path (`Msg::Input(Event::Mouse)` → `mouse.rs::handle`,
   in the normal match) is covered by the same single seam, so it does **not** double-arm (the
   round-1 per-path re-arms that caused that risk are removed — see item 4).

   **Scope of "exactly once" — active buffer only (honest claim, cross-ref §2.3).** The seam arms
   on an edit to the **active** buffer. An edit to a **non-active** buffer does not arm it. This
   is the general class of `by_id_mut(…).apply(…)` edits that can target a buffer other than the
   active one — verified instances: `scratch.rs::append_to_scratch` (used by
   `copy_block_to_scratch` / `move_block_to_scratch` to write the scratch buffer *by id* while a
   different buffer stays active), and the by-id **async result merges**
   `jobs_apply.rs::apply_filter_done`, `transform.rs::merge_transform_into`, and
   `jobs_apply.rs::insert_paste_text` (each applies to its `buffer_id`; whether they *rebuild render
   state* varies — `apply_filter_done` rebuilds unconditionally, the other two only when the id is
   active — but that is orthogonal to *arming diagnostics*) — when their target buffer isn't the
   active one. Any of these
   bumps that buffer's `document.version` without arming (it isn't active). This is **not** a
   display bug: `DiagStore::valid_for(version)` is version-gated, so the buffer's now-stale
   diagnostics are **suppressed** (never painted wrong), and per §2.3 activating a stale buffer
   does not auto-arm either — so the buffer simply shows no underlines until its next **in-Review**
   active edit or a manual `recheck_diagnostics`, fully consistent with the approved model. No
   special-casing is added (it would be over-engineering for a case with zero display-correctness
   impact).

   *Module-structure & hot-path:* the `reduce`/`reduce_dispatch` split *reduces* the
   dispatcher function (thinning the god-function toward the `too_many_lines` budget rather than
   growing it) and keeps arming inside the pure, testable `reduce` seam. `arm_if_edited` costs
   one id compare + one integer compare on the common (non-editing) message and short-circuits
   before `should_run_diagnostics` — negligible on the input hot path.

2. **`timers.rs::on_tick` — the dispatch side.** The guard
   `if editor.diag_cfg.enabled && crate::diagnostics_run::diag_due(…)` becomes
   `if crate::diagnostics_run::should_run_diagnostics(editor) && crate::diagnostics_run::diag_due(…)`.

3. **`registry.rs`'s `recheck_diagnostics` handler** — the manual force
   (`if c.editor.diag_cfg.enabled { …diagnostics.arm(c.clock.now_ms(), 0) }`) becomes
   `if crate::diagnostics_run::should_run_diagnostics(c.editor) { … }`. Recheck is a
   Review-surface verb; outside Review it is a no-op, same as today with `enabled = false`.

4. **The two ignore/add-dict re-arms in `diag_apply_selected` — the non-edit trigger (class B;
   these STAY, re-gated).** `search_ui.rs::diag_apply_selected` has three branches. Its
   **suggestion-fix** branch calls `editor.apply(txn, …)` → a `document.version` delta → it is
   class A, covered by the unified seam (item 1); it gets **no** per-path re-arm (the round-1
   re-arm added here is **removed**). Its **`is_ignore`** and **`is_add_dict`** branches insert
   into `session_ignores` / append to the dictionary and clear `editor.diag` **without editing
   the document** — `document.version` does **not** change — so the unified version-delta seam
   correctly does not fire for them, yet a recheck IS needed (the just-ignored word must stop
   being flagged; the just-added word must clear). These two therefore **keep** their existing
   explicit `arm(clock.now_ms(), debounce_ms)`, with the gate changed from
   `if editor.diag_cfg.enabled` to `if crate::diagnostics_run::should_run_diagnostics(editor)`.
   They are the complete set of diagnostic-relevant non-edit state changes; because they never
   coincide with a version delta in the same message (a single `diag_apply_selected` call takes
   exactly one branch), they can never double-arm with the class-A seam.

   Likewise the three search-mutation paths (`search_replace_all` / `search_step_apply` /
   `search_step_rest`) get **no** per-path re-arm — they are class A (each `Buffer::apply` bumps
   the version) and the unified seam handles them. The round-1 additions to these functions are
   **removed**. `search_step_skip` mutates nothing and was never a concern.

5. **`timers.rs::diag_deadline` — the wake-computation site (code-forced addition; see the
   note below).** The subsystem-deadline fn currently returns `recheck_due_at` whenever
   `in_flight_version.is_none()`. It must additionally gate on the compute predicate:

   ```rust
   fn diag_deadline(e: &Editor, _now: u64) -> Option<u64> {
       if crate::diagnostics_run::should_run_diagnostics(e)
           && e.active().diagnostics.in_flight_version.is_none()
       { e.active().diagnostics.recheck_due_at } else { None }
   }
   ```

   **Why this fifth site is mandatory** (it extends the ledger's four-site enumeration, for a
   reason the real code forces): `DiagStore::arm` sets `recheck_due_at = Some(…)`, and only
   `dispatch_diagnostics` consumes it (`recheck_due_at = None`). If the user edits in Review
   (arm) and switches mode *before* the debounce elapses, the store is left armed. Ungated,
   `diag_deadline` would then return a past-due `Some` on every loop iteration while the
   gated `on_tick` dispatch (site 2) never consumes it — `next_wake` → `recv_timeout(0)` →
   a 100 % CPU spin, exactly the "de-gated past-due Some" A3-spin class that `timers.rs`'s
   module doc declares each deadline fn must prevent ("each subsystem's `deadline` embeds its
   own in-flight/pending gate so a de-gated past-due Some can never reach recv_timeout(0)").
   Gating the deadline fn is a read-side gate, not transition bookkeeping, so it stays within
   F3 = A's "no transition bookkeeping" decision; the stale `recheck_due_at` is harmlessly
   inert (superseded by the `arm(now, 0)` on the next Review entry, or by the next in-Review
   edit). This preserves both the anti-spin invariant and **idle-is-free**: a non-Review
   buffer with a residually-armed store contributes `None` to `next_wake`.

Result-landing is deliberately NOT gated: `apply_diagnostics_done` stays version-gated only.
If the user leaves Review while a check is in flight, the result still lands in the per-buffer
store (clearing `in_flight_version` — load-bearing for the deadline gate above) and is simply
not painted (§2.3). No work is wasted and no transition bookkeeping is introduced.

The **startup warm** of Harper's `FstDictionary` `LazyLock` (`app.rs::run`, the
`wcartel-diag-warm` thread) stays keyed on `diag_cfg.enabled` alone, unchanged: it is a
one-shot cache warm on a background thread, not an arm site, and warming at launch is exactly
what keeps the *first* Review entry from paying the ~11 s dictionary init.

### 2.3 The gated site — display

`render.rs::gather_row_ctx`'s `diag_active` gains the display gate:

```rust
let diag_active = crate::diagnostics_run::should_show_diagnostics(editor)
    && editor.active().diagnostics.valid_for(editor.active().document.version);
```

`diag_active` already feeds both the diagnostic slice selection (`diag_src`) and the
`use_placed` fast-path decision, so this single change makes leaving Review hide the
underlines **instantly** on the next frame — including underlines from a check that completed
during a Review visit and would otherwise linger (paint is mode-independent today; ledger
F3 = A gates BOTH compute and display, no clear-on-leave bookkeeping). Re-entering Review with
still-valid stored diagnostics (no edits since they were computed) re-paints them immediately,
and the arm-on-enter (§2.4) refreshes them shortly after.

Buffer-switch parity: switching to a buffer whose stored diagnostics are stale (edited since
computed) paints nothing until an in-Review edit or a manual recheck — identical to today's
`valid_for` semantics; no regression and no new behavior. The compute seam (§2.2 item 1)
realizes this exactly: keyed on `(active BufferId, version)`, a switch **does not arm** (active
id changed), so activating a stale buffer never triggers an auto-recheck — the only
becoming-active refresh is the explicit arm-on-enter-Review in `set_render_mode` (§2.4). The
non-active-edit case (`copy_block_to_scratch` writing scratch while another buffer is active,
§2.2 item 1) lands here too: scratch's version advances without arming, its stale diagnostics
are version-suppressed by `valid_for`, and it shows underlines again only on its next in-Review
edit or a manual recheck — the same stale-buffer rule.

### 2.4 Arm-on-entering-Review

Entering Review triggers a fresh check on arrival (ledger F2 = A): the shared setter
`Editor::set_render_mode` (§3.1) ends with

```rust
if crate::diagnostics_run::should_run_diagnostics(self) {
    self.active_mut().diagnostics.arm(now_ms, 0);
}
```

— a single `arm(now_ms, 0)`, i.e. due immediately; the next tick dispatches through the
existing single-in-flight `diag_due` machinery. The condition is evaluated *after* the mode is
written, so it is exactly "the buffer is now in Review and checking is enabled." Invoking
`view_review` (or cycling onto Review) while already in Review re-arms at 0 — deliberately
idempotent, equivalent to `recheck_diagnostics`, and cheaper than tracking the prior mode
(F3's no-transition-bookkeeping stance). Entering any non-Review mode arms nothing.

### 2.5 The diagnostic-action commands are Review-only (resolved by the human)

`quick_fix`, `diag_next`, and `diag_prev` (`registry.rs`) act on stored diagnostics and today
gate only on `DiagStore::valid_for`. Because `valid_for` stays true after leaving Review
(until the next edit), an ungated version would let a user open a fix or jump the caret to an
underline that is **no longer painted** — the display gate (§2.3) hides the underlines, but the
action commands would still reach the stale-but-valid store. To keep the whole diagnostics
*experience* inside Review (human decision, 2026-07-11), all three additionally gate on
`should_show_diagnostics`:

- **`quick_fix`** — outside Review, short-circuit before opening the overlay and set the
  status `"no diagnostic here"` (the handler's existing not-found message; no new string). This
  runs *before* the `valid_for` check, so the message is identical whether the store is empty
  or the mode is wrong.
- **`diag_next` / `diag_prev`** — outside Review, return `CommandResult::Handled` with no caret
  movement (they already early-return `Handled` when `!valid_for`; the mode gate is the same
  shape, evaluated first).

Concretely each handler gains a leading
`if !crate::diagnostics_run::should_show_diagnostics(c.editor) { … return CommandResult::Handled; }`
guard (with the status set only for `quick_fix`). The commands remain registered and
palette-visible in every mode (law 3) — they are runtime no-ops outside Review, exactly as the
recheck verb is. This closes the invisible-underline window §8.2 previously flagged.

---

## 3. Command surface (contract multi-state pattern)

Render mode becomes a first-class multi-state option, built to the exact template the registry
already uses for `scrollbar_off/auto/on` + `cycle_scrollbar` and
`clipboard_provider_{auto,native,osc52,off}` + `clipboard_provider_cycle`
(`wordcartel/src/registry.rs`).

### 3.1 The shared setter — `Editor::set_render_mode`

Today `Command::CycleRenderMode` (`commands.rs`) writes `editor.active_mut().view.mode`
directly and then calls `derive::rebuild` + `nav::ensure_visible` — there is **no shared
setter**, unlike every other multi-state option (`Editor::set_scrollbar_mode`,
`Editor::set_clipboard_provider`). E7 closes that gap:

```rust
/// The single setter for the render mode (command-surface contract, law 6). All render-mode
/// mutation — the four view_* primitives, the cycle, and any future profile/plugin — routes
/// here. `now_ms` feeds the arm-on-entering-Review debounce timestamp.
pub fn set_render_mode(&mut self, mode: RenderMode, now_ms: u64) {
    self.active_mut().view.mode = mode;
    crate::derive::rebuild(self);
    crate::nav::ensure_visible(self);
    if crate::diagnostics_run::should_run_diagnostics(self) {
        self.active_mut().diagnostics.arm(now_ms, 0);
    }
}
```

(Method on `Editor` in `editor.rs`, beside `set_scrollbar_mode` / `set_clipboard_provider`;
`derive::rebuild(editor: &mut Editor)` and `nav::ensure_visible(editor: &mut Editor)` are the
verified free-function signatures, callable with `self`. The `now_ms: u64` parameter follows
the codebase's clock-threading convention — registry handlers pass `c.clock.now_ms()`,
`commands::run` callers pass their `Clock`. Setting the current mode again is permitted and
cheap: `rebuild` is `LayoutKey`-memoized, and re-arming in Review is the documented idempotent
recheck, §2.4.)

`Command::CycleRenderMode`'s arm in `commands.rs` is rewritten to compute the successor and
delegate to the setter — the direct field write and the trailing `rebuild`/`ensure_visible`
calls are removed from the arm (the setter owns them).

### 3.2 The four set-per-state primitives

Four new registry rows (rule 8's "set-value primitives"), `menu: None` (palette-only), each a
thin closure over the setter — exactly the `scrollbar_off` shape:

| id | label | effect |
|---|---|---|
| `view_live_preview` | `View: Live Preview` | `set_render_mode(RenderMode::LivePreview, now)` |
| `view_source_highlighted` | `View: Source Highlighted` | `set_render_mode(RenderMode::SourceHighlighted, now)` |
| `view_source_plain` | `View: Source Plain` | `set_render_mode(RenderMode::SourcePlain, now)` |
| `view_review` | `View: Review` | `set_render_mode(RenderMode::Review, now)` |

Each handler is `|c| { c.editor.set_render_mode(…, c.clock.now_ms()); CommandResult::Handled }`.
`now` = `c.clock.now_ms()` from the registry `Ctx` (verified fields: `editor`, `clock`,
`executor`, `msg_tx`).

### 3.3 The stateful cycle (the menu representative)

`cycle_render_mode` is **promoted** from plain `register("cycle_render_mode", "Cycle Render
Mode", Some(MenuCategory::View), …)` to `register_stateful` — the convention gap the surface
map identified (every sibling multi-state option already carries a live-state menu label):

- **id:** `cycle_render_mode` (unchanged — preserves both presets' existing bindings and the
  retired test table's rows).
- **label:** `Render Mode` (ledger F6's menu label; replaces "Cycle Render Mode" — matches the
  noun-label convention of `Scrollbar`/`Clipboard`/`Keymap`).
- **menu:** `Some(MenuCategory::View)` (unchanged).
- **state fn:** `fn(&Editor) -> MenuMark`, returning `MenuMark::Value(…)` with the 4-state
  live label — `"Live"` / `"Review"` / `"SRC-HI"` / `"Source"` for
  `LivePreview`/`Review`/`SourceHighlighted`/`SourcePlain` respectively (menu renders a
  `Value` verbatim beside the label, per `menu.rs`'s `MenuMark` match).
- **handler:** unchanged shape — dispatches `Command::CycleRenderMode`.

**Cycle order changes** (ledger F4) to put Review one keypress from drafting:

```
LivePreview → Review → SourceHighlighted → SourcePlain → LivePreview
```

The `Command::CycleRenderMode` match in `commands.rs` implements this order (exhaustive over
all four variants — no `_` arm) and calls `set_render_mode` on the result. Cycling *onto*
Review arms the fresh check via the setter, uniformly with the direct command.

### 3.4 Keybindings

- **`view_review` gets a DEFAULT chord in BOTH preset tables** (`static CUA` and
  `static WORDSTAR` in `keymap.rs`). The **exact chord is deferred to the plan**, per the
  approved design ("exact chord = plan's, conflict-checked"): its selection must be
  collision-checked against both full tables (including WordStar's `^Q`/`^K` prefix families
  and the `wordstar_has_no_chord_collisions_or_prefix_shadows` invariant) at plan time, with
  the real tables in front of the implementer. The spec's requirement is only: one chord per
  preset, present in both, surfaced by hints (law 7).
- **WordStar gains `("f1", "cycle_render_mode")`** — today the `WORDSTAR` table has NO binding
  for `cycle_render_mode` (nor `quick_fix`/`diag_next`/`diag_prev` — a verified pre-existing
  gap), and `"f1"` is unbound there (grep-verified: `"f1"` appears only in the CUA table and a
  doc comment). CUA already binds both `f1` and `ctrl-\`; unchanged.
- The other three primitives (`view_live_preview`, `view_source_highlighted`,
  `view_source_plain`) are **palette-first in both presets** — no default chords (ledger F4).
  The WordStar `quick_fix`/`diag_next`/`diag_prev` gap stays open (out of E7's approved
  scope; palette reaches them under WordStar today).

### 3.5 Registration order

All new and changed registrations (`view_*` ×4, the promoted `cycle_render_mode`) live in the
View block of `Registry::builtins`, **before `save_settings`** — the registry's own comments
pin `save_settings` as the last-registered command
(`journey_palette_end_reaches_last_command` + the registration-order invariant both rely on
it). `cycle_render_mode` is currently registered near the top of `builtins` and stays put; the
four primitives slot beside it.

---

## 4. Command-surface contract conformance

This effort touches commands, an option, the palette, the menu, and keybinding hints — full
conformance with `docs/design/command-surface-contract.md`, law by law:

- **Law 1 (registry is the single source of truth).** All render-mode mutation flows through
  registry commands dispatching `Editor::set_render_mode`. E7 *removes* the one existing
  bypass-shaped site (the direct `view.mode` field write inside `Command::CycleRenderMode`)
  by routing it through the setter. No non-registry mutation path is added.
- **Law 2 (every user-settable option is a command).** Render mode is per-buffer **runtime
  view state**, not a persisted setting — there is no `SettingsSnapshot` field and no config
  key (verified: `settings.rs` and `config.rs` contain no render-mode key; E7 adds none), so
  law 2's *test* (`every_persisted_setting_has_a_command`, `settings.rs`) is structurally
  unaffected. The option nevertheless gets full command coverage (rule 10: a plugin should be
  able to put a buffer in Review — now it can, deterministically).
- **Law 3 (palette exhaustive).** The four primitives and the promoted cycle are ordinary
  registry rows; the palette picks them up automatically.
  `palette_is_exhaustive_over_the_registry` (`palette.rs`) stays green with the new rows.
- **Law 4 (menu ⊆ palette).** Only `cycle_render_mode` carries a menu category (View); the
  primitives are `menu: None`. The menu row names a registered command that is in the
  palette. No dynamic-section machinery involved.
- **Law 5 (every mouse affordance has a keyboard path).** The menu's Render Mode row is
  keyboard-reachable via the cycle's chords (both presets, after §3.4) and via the palette.
- **Law 6 (one setter per option).** `Editor::set_render_mode` is NEW and is the single
  mutation path — the four primitives, the cycle, and any future profile or plugin all call
  it. This *closes* the current no-shared-setter gap for render mode.
- **Law 7 (hints track the active keymap).** `view_review`'s default chord exists in both
  preset tables, so palette/menu hints re-resolve across a CUA↔WordStar switch;
  `hints_reresolve_on_preset_switch` (`keymap.rs`) and
  `custom_bind_surfaces_in_menu_and_palette` (`menu.rs`) cover the mechanism, and
  `both_presets_resolve_against_builtins` (`keymap.rs`) verifies every new binding names a
  registered id (it would catch a typo'd `view_review` row in either table).
- **Rule 8 (multi-state = set-per-state primitives + a stateful menu representative).**
  Followed exactly: four `menu: None` set-value primitives + `cycle_render_mode` promoted to
  `register_stateful` with a `MenuMark::Value` 4-state live label — the
  `scrollbar_off/auto/on` + `cycle_scrollbar` template, verbatim.
- **Rule 9 (a preset is never the only door).** No profile/preset sets render mode; N/A but
  trivially satisfied by the primitives.
- **Rule 10 (commands are the plugin spine).** All five commands stay nullary; the set-value
  primitives keep clean set-to-X semantics so Effort P can later collapse them into one
  parameterized command without breaking the contract.

**Invariant-test GATEs exercised by this effort:** `palette_is_exhaustive_over_the_registry`,
`every_persisted_setting_has_a_command`, `hints_reresolve_on_preset_switch`,
`custom_bind_surfaces_in_menu_and_palette`, `both_presets_resolve_against_builtins` — all five
must pass on the merged branch; none needs structural modification (the new rows/bindings ride
the existing harnesses).

---

## 5. Scope boundary (explicit)

**Seq-1 only.** The diagnostics backend is the embedded Harper engine, exactly as shipped —
same `wordcartel_core::diagnostics::check`, same worker-thread dispatch with panic isolation,
same `DiagStore`, same quick-fix overlay. E7 changes *when* it runs and *where* it shows,
nothing else. The **`harper-ls` LSP swap is seq-2 and OUT of scope** — no dependency changes,
no protocol work, no new crates, no `Cargo.toml` edits. `diag_cfg.linters` stays the unused
placeholder it is today.

---

## 6. Responsiveness note

Gating diagnostics to Review removes the grammar-check cost from the drafting path entirely:
in LivePreview (the default drafting mode) an edit no longer arms the 400 ms diagnostics
debounce, no Harper worker is ever spawned, and `gather_row_ctx` never takes the
diagnostics-placed span path (one fewer `use_placed` trigger). The check's cost lands only
inside Review, where the user is deliberately reviewing and lag-tolerant. The base
per-keystroke parse/layout pipeline is **unchanged** — incremental-parse and render-diff cost
is R1 territory, not this effort. This is a responsiveness side-benefit of E7, not a separate
effort.

The gating also upholds the resource-behavior conventions: a non-Review buffer with a
residually-armed `DiagStore` contributes `None` to `next_wake` (§2.2 site 5), so idle stays
free and no wake fires for work the gate would refuse.

---

## 7. Test intent

New tests use the house Arrange-Act-Assert style in the existing `#[cfg(test)]` modules of the
files they exercise.

### 7.1 Review mode rendering

- `lines.rs`: `line_render_for(Review, true) == RawPlain` and
  `line_render_for(Review, false) == Concealed` — Review mirrors LivePreview (the exhaustive
  match makes omission a compile error; the test pins the *mapping*).
- `render.rs`: a pinning test that Review is not plain-source — e.g. a styled heading row in
  Review renders through the colored branch, byte-for-byte like LivePreview (guards the
  `plain_source` equality in `gather_row_ctx` against a future refactor absorbing Review).
- `render_status.rs`: `status_left_text` shows `[REVIEW]` when the active buffer's mode is
  Review.

### 7.2 Draft-quiet gating

- `diagnostics_run.rs`: unit tests for `should_run_diagnostics` /
  `should_show_diagnostics` truth table — (enabled × mode) → only `enabled && Review` is true.
- `diagnostics_run.rs` (`arm_if_edited` unit tests, class A): with the SAME active buffer, a
  version increase in Review arms at `now + debounce_ms`; the same increase in LivePreview does
  not arm; equal `before`/`after` versions never arm regardless of mode; `enabled = false` never
  arms; and — the buffer-identity guard — when the active `BufferId` differs from `before_id`
  (a switch), it does **not** arm even though the new active version differs.
- `app.rs` (buffer-switch does NOT arm, §2.2 item 1 / §2.3): with two buffers A (v0) and B (v1)
  both in Review, switching A→B through `reduce` (a `workspace::switch_to` path — palette/mouse
  buffer row or Documents-menu row) leaves `recheck_due_at == None` on the newly-active B; an
  **edit to the active buffer** in Review through `reduce` arms it exactly once. Together these
  pin "arm on active-buffer edit, never on switch."
- `app.rs` (the unified seam through `reduce`, all three interceptor families — the
  live-in-Review completeness guard, §2.2 item 1): driving a **direct** text edit, a **quick-fix
  suggestion** (via the diag-overlay intercept), a **search-replace** (via the search intercept),
  and a **prompt-held `Msg::FilterDone`** (via the prompts intercept) through `reduce` each arms
  the store (`recheck_due_at == Some(now + debounce_ms)`) when the buffer is in Review, and each
  arms **nothing** (`recheck_due_at` stays `None`) when not in Review. These are the regression
  tests for the Important finding: the two overlay families and the prompt-job family bypass the
  post-interceptor epilogue, so without the top-of-`reduce` seam they would fail to arm. Existing
  epilogue-arm tests that assumed arm-on-edit under the default mode are updated to put the buffer
  in Review first.
- `app.rs` (no double-arm, class-A single-fire): a **mouse** quick-fix (which returns through
  `reduce`'s normal match) arms the store exactly once — asserted via a single `recheck_due_at`
  transition, guarding against the removed round-1 per-path re-arms reappearing.
- `search_ui.rs` (`diag_apply_selected` class-B, non-edit trigger): the **ignore** and
  **add-to-dictionary** branches — which do not change `document.version` — still arm in Review
  (`recheck_due_at == Some(now + debounce_ms)`) and do not arm outside Review; the **suggestion**
  branch carries no internal re-arm (the class-A seam covers it), verified by driving it through
  `reduce` per the item above rather than by a per-path assertion here.
- `editor.rs` (setter): `set_render_mode(Review, now)` arms `(now, 0)` when enabled; does not
  arm when `enabled = false`; `set_render_mode(LivePreview, now)` never arms; the setter
  rebuilds and keeps the caret visible (mirror of `cycle_render_mode_keeps_caret_visible`).
- `render.rs` (display gate): valid, current-version diagnostics are painted in Review and
  NOT painted in LivePreview — the leave-Review-hides-instantly property. The existing
  `diagnostics_underline_the_flagged_glyphs` and `stale_diagnostics_are_not_painted` tests are
  updated to set `view.mode = Review` (they exercise the paint path, which is now gated).
- `timers.rs` (anti-spin/idle-is-free guardrail): an armed `DiagStore` on a non-Review buffer
  yields `diag_deadline == None` (and `next_wake == None` with everything else settled); the
  same armed store in Review yields the due time. This is the §2.2-site-5 spin-class
  regression test, in the same family as `settled_editor_arms_no_deadline` and the existing
  diagnostics-deadline tests.
- `registry.rs`: `recheck_diagnostics` arms in Review and no-ops in LivePreview.
- `registry.rs` (Review-only action commands, §2.5): with valid stored diagnostics under the
  caret, `quick_fix` opens the overlay in Review but outside Review sets `"no diagnostic here"`
  and opens no overlay (`editor.diag` stays `None`); `diag_next`/`diag_prev` move the caret in
  Review but leave the selection unchanged outside Review — each with `valid_for` true, so the
  test isolates the mode gate from the store-empty path.

### 7.3 Command surface

- `commands.rs`: `cycle_render_mode_rotates_through_modes` updated to the new 4-state order
  (Live → Review → SRC-HI → Source → Live); its doc comment updated likewise.
- `registry.rs`: the four `view_*` primitives are registered with `menu: None` (the
  `clipboard_provider_commands_registered_with_correct_menu_tags` pattern); dispatching
  `view_review` puts the active buffer in Review AND arms the store at 0; `cycle_render_mode`
  carries a state fn whose `MenuMark::Value` tracks the live mode across all four states.
- `keymap.rs`: WordStar `f1` resolves to `cycle_render_mode`; `view_review`'s chord resolves
  in BOTH presets; `wordstar_has_no_chord_collisions_or_prefix_shadows` and
  `both_presets_resolve_against_builtins` stay green with the new rows.
- The five contract GATE tests of §4 pass unmodified.

### 7.4 Affected harnesses (updates, not new coverage)

- `e2e.rs::diagnostics_probe` (the perf-sampling diagnostics-landing probe) injects
  `Msg::DiagnosticsDone` and measures the placed-span render path. With the display gate it
  MUST set the probed buffer to Review, or it silently measures the un-placed path and the
  "diagnostics-landing" sample becomes meaningless. (The e2e journeys that set
  `diag_cfg.enabled = false` for hermeticity are unaffected.)

---

## 8. Flagged notes against the approved design

Three places where the real code forced a spec-level statement beyond the ledger's literal
text. None re-opens a settled fork; all are recorded for the reviewer per process. §8.2 was an
open question at spec-draft time, resolved by the human (2026-07-11); §8.3 folds an Important
gate finding (2026-07-11) that UPHOLDS the approved live-in-Review decision (F2) by completing
its coverage.

### 8.1 The fifth gating site (`timers.rs::diag_deadline`) — code-forced, specified as normative

The ledger (and the surface map's "timers.rs dispatch needs no change — nothing armed →
`diag_due` never fires") enumerates four `diag_cfg.enabled` sites. The map's claim is wrong
for one reachable state: *leave Review while armed* (edit in Review, switch mode inside the
400 ms window) leaves `recheck_due_at = Some(past)` with nothing to consume it, and an ungated
`diag_deadline` would spin the loop at `recv_timeout(0)` — the documented A3-spin class the
timers hub exists to prevent. §2.2 site 5 therefore gates the deadline fn on
`should_run_diagnostics` as normative spec behavior, with a guardrail test (§7.2). This is a
completion of F3's gating at a site the enumeration missed, not a design change — but it is
called out here because it extends the approved site list.

### 8.2 `quick_fix`/`diag_next`/`diag_prev` outside Review — RESOLVED: Review-only

The ledger describes these three commands as **reused unchanged** ("ZERO new diagnostic-render
code"), which — gated on `DiagStore::valid_for` only — would leave a window: diagnostics
computed during a Review visit stay *stored and valid* until the next edit, so a user who
switched back to LivePreview could still `quick_fix` a diagnostic under the caret or
`diag_next`/`diag_prev` to it — acting on underlines that are **no longer painted** (the window
closes on any edit, which invalidates `valid_for`). That is a visible asymmetry with the
"leave Review → diagnostics vanish" display story.

**Resolved by the human (2026-07-11): make all three Review-only.** §2.5 specifies the
resolution — each handler gains a leading `should_show_diagnostics` guard (evaluated before the
existing `valid_for` check), so outside Review `quick_fix` reports its existing
`"no diagnostic here"` status and `diag_next`/`diag_prev` are caret-preserving no-ops. This
keeps the entire diagnostics experience — compute, display, and action — inside Review, and
closes the invisible-underline window. It is a small, contained addition to the three handlers
(no structural change; the commands stay registered and palette-visible in all modes, law 3).
Recorded here because it goes beyond the ledger's "reused unchanged" wording.

### 8.3 The `reduce` epilogue is not a universal edit chokepoint — a single `(buffer-id, version)`-keyed seam replaces it

The `reduce` epilogue arms diagnostics off `document.version != before`, but `before` is
snapshotted **after** all eleven overlay/modal interceptors (`app.rs::reduce`, verified), each
of which can `return Handled::Done(k)` before that line. So an edit performed inside an
interceptor is invisible to the epilogue *even in principle* — the snapshot post-dates the
mutation. Three interceptor families edit and early-return this way (verified against real
source): the diagnostics overlay (`diag_overlay.rs::intercept` → `search_ui::diag_apply_selected`
suggestion-fix), the search overlay (`search_ui.rs::intercept` → `search_replace_all` /
`search_step_apply` / `search_step_rest`), and — the class round 2 surfaced —
**prompt-held async job results** (`prompts.rs::intercept` consumes `Msg::FilterDone` /
`TransformDone` / `ClipboardPaste` → `jobs_apply::apply_*` → `Buffer::apply`). A quick-fix
accepted, a find-and-replace run, or a filter/transform result landing *while in Review* would
leave diagnostics stale with no auto-recheck — breaking F2.

An earlier round of this spec chased these per-path (arming inside each mutating function). That
is whack-a-mole: round 2 found the prompt-job family a per-path list had missed, and a future
editing interceptor would reintroduce the gap. The spec now adopts a **single seam keyed on
`(active BufferId, document.version)`** (§2.2 item 1): the snapshot is captured at the **top**
of `reduce` (before any interceptor), the dispatch body moves to an inner `reduce_dispatch(…)`,
and `reduce` calls one shared `diagnostics_run::arm_if_edited(editor, before_id, before_version,
clock)` on the way out — covering **every** exit path (all interceptor early-returns and the
normal tail) uniformly, arming exactly once per **active-buffer** edit, gated on
`should_run_diagnostics`. Keying on buffer identity (not version alone) is load-bearing: round 3
noted a bare-version seam would falsely arm on a **buffer switch** (A v0 → B v1 changes the
active version without an edit), contradicting §2.3; requiring the active id to be **unchanged**
makes a switch a no-arm, with becoming-Review-active refreshed only by `set_render_mode`'s
arm-on-enter (§2.4). The old inline epilogue arm and all round-1 per-path re-arms are removed;
the mouse quick-fix no longer risks a double-arm. The split thins the `reduce` god-function
(module-structure GATE) rather than growing it.

Two nuances the seam makes explicit, both consistent with the approved model:
- **Non-edit diagnostic-state changes.** The **ignore-once** and **add-to-dictionary** branches
  of `diag_apply_selected` change the ignore-set / dictionary **without** changing
  `document.version`, so the seam correctly does not fire for them — they keep their own explicit
  `should_run_diagnostics`-gated re-arm (§2.2 item 4). Arming thus has two clean classes: any
  active-buffer edit (the unified seam) and the two non-edit diagnostic-state mutations (their
  explicit re-arms). The two never coincide in one message, so there is no double-arm.
- **Non-active-buffer edits.** A `by_id_mut(…).apply(…)` edit whose target isn't the active
  buffer is not armed by the active-buffer seam. This is a general class, not one path: scratch
  via copy/move-to-scratch (`scratch.rs::append_to_scratch`), and the by-id async result merges
  `jobs_apply.rs::apply_filter_done` / `transform.rs::merge_transform_into` /
  `jobs_apply.rs::insert_paste_text` (rebuild-of-render-state varies — `apply_filter_done`
  unconditional, the other two active-gated — orthogonal to arming), when their target buffer isn't
  active. This has **no display-correctness impact**:
  `DiagStore::valid_for` is version-gated, so such a buffer's stale diagnostics are suppressed
  (not shown wrong), and per §2.3 activating a stale buffer doesn't auto-arm either — it shows
  underlines again only on its next in-Review active edit or a manual `recheck_diagnostics`.
  §2.2's "exactly once" claim is scoped to the active buffer accordingly; no special-casing is
  added.

Backed by the §7.2 tests, which exercise all three interceptor edit families, the buffer-switch
no-arm case, the active-buffer arm-once case, the non-edit ignore/add-dict branches, and the
mouse single-fire guard.
