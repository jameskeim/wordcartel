# C1 — The Writing Caret (B8 + B11): design spec

**Effort:** C1 · **Backlog:** absorbs **B8** (writing caret — DECSCUSR shape/blink + picker +
panic-safe restore) and closes **B11** (modal/overlay caret parked under the modal).
**Date:** 2026-07-13 · **Author:** Fable · **Status:** spec (pre-plan), for the Codex spec gate.
**Grounding source of record:** `docs/design/c1-writing-caret-scoping.md` (this author's prior
grounding; every symbol re-verified against the tree for this spec). Anchors are symbol NAMES.

> **Reviewer note.** This spec is gated by cross-checking every claim against the REAL source
> (not a diff; do not run cargo). Where a claim pins a specific arm I give the enclosing symbol so
> it survives line drift.

---

## 0. What C1 delivers (one paragraph)

The writing caret stays the **hardware terminal cursor**. C1 makes its **shape** and **blink**
user-settable via two orthogonal options wired through the command-surface contract, emits the
corresponding **DECSCUSR** escape through a new **edge-triggered, latch-guarded reconcile** (zero
writes at rest), previews changes live in a **cursor picker** cloned from the theme picker,
**restores** the caret on clean exit and on panic **only if we ever wrote a style**, and fixes the
**B11** wart so every modal/overlay query field shows its own caret and no caret is left parked in
the text area beneath a modal. No `wordcartel-core` change; no layout/hot-path change.

The eight brainstorm decisions are LAW here (§1). Consequences surfaced while grounding them are
flagged **[CONSEQUENCE]** for the Codex gate.

---

## 1. Settled decisions (LAW — not re-opened)

1. **One global caret style**, read through an internal `desired_caret_style(&Editor)` derivation
   seam (hygiene, NOT a user-facing per-context feature).
2. **Two orthogonal options:** `caret_shape ∈ {default, block, beam, underline}`,
   `caret_blink ∈ {on, off}`. The reconcile composes the pair into one DECSCUSR code.
3. **`caret_shape = default` is the shipped default and means we NEVER emit DECSCUSR.**
   `caret_blink` is **inert under `default`** — it selects nothing and emits nothing until a
   concrete shape is chosen.
4. **First managed stop = blinking block:** `cycle_caret_shape` enters `block` first off
   `default`. (Config default stays `default`.) **Picker clause refined 2026-07-14 (final-gate F2,
   see History):** the picker opens on the row matching the CURRENT caret state, not a fixed
   "first managed" row — so the highlight always matches the caret and Enter-on-open commits it,
   never a lie about what pressing Enter would do.
5. **Picker preview = both:** descriptive glyph rows AND a live caret morph, on the theme-picker
   preview → Esc-restore → Enter-commit funnel.
6. **Unsupported terminal = silent best-effort no-op;** doc note covers tmux `terminal-overrides`;
   no capability detection.
7. **B11 = overlays own their query carets** (each places on its local `query_area`; overlay paint
   runs after `place_cursor`, last-write-wins) **+ an arm-3 hide-guard** so caret-less input owners
   (menu/prompt/splash/diag) get a hidden caret.
8. **Restore = latch-aware:** restore on clean exit AND panic **only if the reconcile ever wrote**;
   one process-global flag shared with the panic hook.

---

## 2. Grounding — the real surfaces C1 touches

### 2.1 Caret placement (`render.rs::place_cursor`)

`place_cursor(frame, editor, area, edit_top, edit_height, status_row, tg)` (render phase 12, called
from `render::render`, which then calls `render_overlays::paint`). Three arms, all
`frame.set_cursor_position(Position { .. })` (ratatui 0.30):

1. `editor.search` open → status-row caret (column from `chrome_geom::search_field_prefix_cols`).
2. `editor.minibuffer` open → status-row caret at `prompt + text` char columns.
3. else → editor caret at `nav::screen_pos(editor)`, D2-clamped to `tg.text_width`.

**Two grounded facts C1 depends on:**
- Ratatui hides the hardware cursor for any frame in which no `set_cursor_position` runs. Arm
  guards already rely on this (an out-of-view row simply doesn't place → hidden that frame).
- `render_overlays::paint` runs AFTER `place_cursor` within the same `draw`, and ratatui's LAST
  `set_cursor_position` wins. This is the mechanism B11 fix #7 uses.

**[CONSEQUENCE C-1] Arm 3 has no overlay gating.** While palette/outline/theme-picker/file-browser
are open, arm 3 still places the caret at the *editor* caret's text-area cell — the B11 defect.
The arm-3 hide-guard (§6) must test EVERY input-owning surface, because arm 3 is the fallthrough.

### 2.2 The escape-write seam (`chrome.rs::reconcile_mouse_capture`)

`pub fn reconcile_mouse_capture<W: std::io::Write>(editor: &mut Editor, backend: &mut W, applied:
&mut bool)` — called in `app::run` at **TWO** sites with `guard.terminal().backend_mut()`, both
threaded the SAME `run`-local latch `let mut applied_mouse = editor.mouse_capture;`: (1) a
**pre-first-draw** standalone call before the startup `terminal().draw(...)`, and (2) an **in-loop**
call each iteration (between `drain_clipboard_intents` and `advance`). This two-site pattern is what
C1's reconcile mirrors (§4.2). It is the exemplar for C1's reconcile: it writes escapes ONLY when
`editor.mouse_capture != *applied`, updates `*applied` ONLY on IO success
(`crossterm::execute!(...).is_ok()`), and is unit-tested against a `Vec<u8>` backend (`chrome.rs`
tests use `&mut buf`). This is the shape that satisfies the idle-free law by construction.
**Second precedent:** `clipboard.rs` writes OSC 52 via `write_all` on the same backend in the
clipboard drain stage.

### 2.3 crossterm caret-style API (version-pinned)

`wordcartel/Cargo.toml` pins `crossterm = "0.28"`. crossterm 0.28 provides
`crossterm::cursor::SetCursorStyle` with variants: `DefaultUserShape`, `BlinkingBlock`,
`SteadyBlock`, `BlinkingUnderScore`, `SteadyUnderScore`, `BlinkingBar`, `SteadyBar` — the full
DECSCUSR 0–6 surface. `execute!`/`queue!` accept it as a `Command`. **[CONSEQUENCE C-2] DECSCUSR
encodes blink INTO the shape code** (there is no separate blink escape and no blink-*speed*): our
two options are composed at write time (§4.3), not emitted as two escapes. There is also **no
DECSCUSR query** — we can restore only to `DefaultUserShape`, never to a prior user shape.

### 2.4 The multi-state option pattern (scrollbar / status_line, traced)

The registry's own comments name scrollbar as the 3-state pattern and status_line as the 2-state
(TransientMode) pattern. C1's `caret_shape` is a **4-state, non-TransientMode** option (a new small
enum) and `caret_blink` is a **2-state bool**. The wiring to mirror:

- **Set-per-state primitives, palette-only** (`menu: None`): `registry` rows `scrollbar_off/auto/on`
  each call the shared setter `Editor::set_scrollbar_mode`.
- **Stateful menu representative, state-in-label:** `register_stateful(id, label, Some(cat),
  state_fn, handler)` where `state_fn: fn(&Editor) -> MenuMark`. `MenuMark` = `{ OnOff(bool),
  Value(&'static str), Text(String) }` (registry.rs). `cycle_scrollbar` uses `MenuMark::Value`;
  `toggle_status_line` uses `MenuMark::Value` over TransientMode.
- **One shared setter:** `Editor::set_scrollbar_mode` — called by the commands, the density profile
  bundle (`density.rs`), and startup config apply (`app::run`). Law 6.
- **Config field:** `config::ViewConfig.scrollbar: TransientMode`.
- **Persistence:** `settings::SettingsSnapshot.view_scrollbar` + `settings::OView.scrollbar:
  Option<String>` + snapshot-from-config + snapshot-from-editor + a diff-law entry (`diff_key` +
  the `any_view` OR); the LAW-2 test destructures the snapshot exhaustively (§7.1).
- **Config override parse:** `config.rs`'s override block maps the raw string
  (`"off"/"auto"/"on"`) to the enum with a warn-on-unknown arm.

### 2.5 The theme picker (clone target)

- **State:** `theme_picker::ThemePicker { query, selected, rows: Vec<String>, scroll_top, original:
  Theme, previewed: Option<String> }`; held as `Editor::theme_picker: Option<ThemePicker>`.
- **Summon + XOR:** registry command `"theme"` (label `Select Theme…`, `MenuCategory::View`) →
  `Editor::open_theme_picker()` (enforces overlay XOR — opening it clears other overlays; tested).
- **Intercept-stage registration seam:** `theme_picker::intercept(msg, editor, ex, clock, msg_tx)
  -> Handled` is one stage of `app::reduce_dispatch`'s flat interceptor chain (the chain lists
  splash, marks, menu, palette, **theme_picker**, file_browser, prompts, minibuffer, search_ui,
  diag_overlay, outline_overlay). Adding the cursor picker = adding ONE stage here — the sanctioned
  registration seam, not dispatcher growth.
- **Preview funnel:** `theme_cmds::preview_selected_theme(editor)` is the single funnel; Esc →
  `editor.apply_theme(tp.original)`; Enter → `theme_cmds::commit_theme_picker(editor)`.
- **Render:** `render_overlays::paint` draws `palette_overlay_rect`, a bordered box, the `> {query}`
  line as a plain `Paragraph` (style `cs.ov_query`), a `list_window`-windowed row list. Mouse
  wheel/click support in `mouse.rs` also calls the preview funnel.

### 2.6 Restore sites (`term.rs`) — three managed, one provably-exempt

Three sites end with the full restore chain `execute!(io::stdout(), DisableMouseCapture,
DisableBracketedPaste, LeaveAlternateScreen, Show)` and are the caret-restore edit targets:
1. `TerminalGuard::Drop` — clean exit (RAII; also `?`-early-returns from `run`).
2. `TerminalGuard::new`'s `Terminal::new(backend)` failure arm — the full-chain rollback.
3. `install_panic_hook`'s hook body — main-thread-gated (M4), runs `recovery::dump_on_panic()` then
   restores then chains the previous hook.

**A fourth, earlier rollback path exists and needs NO caret restore.** `TerminalGuard::new` also has
an `EnterAlternateScreen`-failure arm that does only `let _ = disable_raw_mode(); return Err(e)` —
NOT the full chain. It is provably safe to leave untouched: it fires before the run loop ever spins,
so `reconcile_cursor_style` has not run, no DECSCUSR style can have been written, and the restore
latch (`ever_wrote()`, §5) is provably `false` there — a caret restore would be a guaranteed no-op.
We therefore edit exactly the three managed sites above and leave the fourth as-is.

**[CONSEQUENCE C-3]** The panic hook is a `'static` closure installed via `std::panic::set_hook`; it
has no `&Editor`. A latch it reads must therefore be **process-global** (an `AtomicBool` in a module
static), not an `Editor` field — see §5.

### 2.7 B11 surfaces (input-owning fields)

Full census of input-owning `Editor` fields and their current caret correctness:

| Surface (field) | Query caret today | C1 action |
|---|---|---|
| `search` (SearchState) | ✓ arm 1 places it | none |
| `minibuffer` (Minibuffer) | ✓ arm 2 places it | none |
| `palette` (Palette — has `cursor: usize`, real mid-string editing) | ✗ no caret | place at `> ` + `cursor` char-col |
| `outline` (OutlineOverlay) | ✗ no caret | place at end-of-query |
| `theme_picker` (ThemePicker) | ✗ no caret | place at end-of-query |
| `file_browser` (FileBrowser) | ✗ no caret | place at end-of-query |
| `menu` (MenuView) | — no text field | hide (no place) |
| `prompt` (Prompt — y/n modal, message on status row) | — no text field | hide (no place) |
| `splash` (Splash) | — no text field | hide (no place) |
| `diag` (DiagOverlay) | — no text field | hide (no place) |

**[CONSEQUENCE C-4] Palette is the only overlay that EDITS mid-string.** Both `palette.rs` and
`outline_overlay.rs` carry a `cursor: usize` field, but only the palette moves it into the middle of
the query (left/right/insert/backspace); its caret column is therefore
`"> ".len_cols + palette.query[..cursor].chars().count()`. Outline's `cursor` merely tracks the query
END — `OutlineOverlay::set_query` sets `self.cursor = self.query.len()` and `intercept` only
pushes/pops — so outline, like `theme_picker` and `file_browser`, is end-of-query: caret column
`"> ".len_cols + query.chars().count()`. (Robustness note, plan's call not a spec requirement: outline
could read its own `cursor` for the column to future-proof against ever adding mid-string editing;
end-of-query is exactly equivalent today.)

### 2.8 Anti-regrowth budgets

`wordcartel/tests/module_budgets.rs`: `app.rs` ≤ 1000, `render.rs` ≤ 900 production lines;
`clippy::too_many_lines` threshold 100. C1's landing zones (§8) respect the seams.

---

## 3. The option model

### 3.1 New core-of-shell types (in `config.rs`, beside `TransientMode`)

```rust
/// Writing-caret shape. `Default` = never emit DECSCUSR (terminal's own shape); the shipped
/// default. The three concrete shapes map to DECSCUSR when composed with blink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaretShape { Default, Block, Beam, Underline }

impl Default for CaretShape { fn default() -> Self { CaretShape::Default } }

pub fn caret_shape_str(s: CaretShape) -> &'static str {
    match s { CaretShape::Default => "default", CaretShape::Block => "block",
              CaretShape::Beam => "beam", CaretShape::Underline => "underline" }
}
```

`caret_blink` is a plain `bool` (mirrors `wrap_guide`/`word_count` bools). Default `true`
(blinking) — but **inert while shape is `Default`** (§4.3), so the shipped default emits nothing.

### 3.2 `ViewConfig` fields

Add to `config::ViewConfig` (beside `scrollbar`, `status_line`, `splash`):

```rust
pub caret_shape: CaretShape,   // default: CaretShape::Default
pub caret_blink: bool,         // default: true (inert until a concrete shape is chosen)
```

Update `ViewConfig::Default` accordingly. Add raw override fields to the `RawView` struct and the
override-apply block in `config.rs` (string → enum with warn-on-unknown for shape, bool for blink),
mirroring the `scrollbar` override arm.

### 3.3 `Editor` runtime state + shared setters

Add to `Editor` (beside `scrollbar_mode`):

```rust
pub caret_shape: CaretShape,
pub caret_blink: bool,
```

Two shared setters (Law 6 — the ONLY mutators of these fields; startup config apply and any future
profile call them too):

```rust
pub fn set_caret_shape(&mut self, s: CaretShape) { self.caret_shape = s; }
pub fn set_caret_blink(&mut self, on: bool)      { self.caret_blink = on; }
```

Startup apply in `app::run` calls both from `cfg.view.caret_shape` / `cfg.view.caret_blink`
(mirrors the `set_scrollbar_mode(cfg.view.scrollbar)` line).

### 3.4 The `desired_caret_style` seam (decision #1)

A pure derivation, single source for the reconcile:

```rust
/// The caret style the caret SHOULD currently have, as OUR own `Copy + PartialEq`
/// representation — the `(CaretShape, bool)` pair `(shape, blink)`. Global today (reads only
/// the two options); the seam exists so a per-context map could slot in later WITHOUT rewiring
/// the reconcile — it is NOT a user-facing feature and must not be documented as one.
pub fn desired_caret_style(editor: &Editor) -> Option<(CaretShape, bool)>
```

Returns `None` when `caret_shape == Default` (emit nothing — decision #3); otherwise `Some((shape,
blink))` with a concrete shape. Lives in the reconcile's module (§8 decides which).

**[CONSEQUENCE C-11] The latch representation must be OURS, not `crossterm::cursor::SetCursorStyle`.**
crossterm 0.28's `SetCursorStyle` derives only `Clone, Copy` — **NOT `PartialEq`/`Eq`** — so it
cannot back the edge-trigger comparison (`*applied != desired`). We therefore latch our own
`(CaretShape, bool)` pair (both fields already derive `Copy + PartialEq + Eq`) and map to a crossterm
variant ONLY at the `execute!` call via a separate total mapper:

```rust
/// Map our own (shape, blink) pair to the crossterm DECSCUSR command. Total; `Default` maps to
/// `DefaultUserShape` (never reached via `desired_caret_style`, which returns `None` for Default —
/// but kept total so the mapper has no unreachable arm).
fn to_set_cursor_style(shape: CaretShape, blink: bool) -> crossterm::cursor::SetCursorStyle
```

---

## 4. The reconcile seam

### 4.1 Location decision — a new `cursor_style.rs` module

**Decision:** `reconcile_cursor_style`, `desired_caret_style`, and the DECSCUSR composition live in
a **new `wordcartel/src/cursor_style.rs`**, NOT as `chrome.rs` siblings.

**Justification.** `chrome.rs` owns the six-face chrome-elevation model (bars/overlays/canvas) and
mouse-capture reconcile; caret style is a distinct single responsibility (terminal cursor
appearance + its restore latch). One file per responsibility (house rule). The module is small
(setter-free: state lives on `Editor`; the module holds the derivation, the composition, the
reconcile, and the restore latch/static — see §5). It keeps `chrome.rs` from accreting a second
unrelated reconcile and gives the restore latch a natural home reachable from both `app::run` and
the `term.rs` panic hook.

### 4.2 Signature and call site

```rust
/// Edge-triggered: emits a DECSCUSR escape ONLY when the desired style differs from what was
/// last applied. Never writes at rest. Latches success into `applied` and (on the first real
/// write) into the process-global restore flag (§5). Best-effort: an ignored/failed write does
/// not update the latch, so it is retried next change — never spun on. `applied` holds OUR
/// `(CaretShape, bool)` pair (crossterm's `SetCursorStyle` is not `PartialEq` — C-11); the
/// crossterm variant is produced by `to_set_cursor_style` only at the `execute!` call.
pub fn reconcile_cursor_style<W: std::io::Write>(
    editor: &Editor,
    backend: &mut W,
    applied: &mut Option<(CaretShape, bool)>,
);
```

Called in `app::run` at **BOTH** the same two sites `reconcile_mouse_capture` is called, sharing one
`run`-local latch exactly as `applied_mouse` is shared:
1. **Pre-first-draw** — the standalone `reconcile_mouse_capture(&mut editor, guard.terminal()
   .backend_mut(), &mut applied_mouse)` call that runs before the first `terminal().draw(...)`. The
   caret reconcile goes here too so a persisted concrete `caret_shape` from config is applied to the
   **startup frame**, not deferred to the first input/tick.
2. **In-loop** — the per-iteration call in the reconcile band (after `drain_clipboard_intents`,
   before `advance`/`draw`), with `guard.terminal().backend_mut()`.

Both calls pass the **same** `applied: &mut Option<(CaretShape, bool)>` latch — a single `run`-local
initialized `None`, mirroring how `let mut applied_mouse = editor.mouse_capture;` is declared once and
threaded to both `reconcile_mouse_capture` sites. Because both calls are edge-triggered against that
shared latch, the pre-draw call writes at most once (the persisted style) and the in-loop call writes
nothing thereafter at rest — the idle-free guarantee is intact across both sites.

**[CONSEQUENCE C-10] Two call sites, one latch.** The startup call is what makes a config-persisted
`caret_shape` visible on frame one; omitting it (in-loop only) would show the terminal-default caret
until the first event. The plan must add both calls and the one latch local, mirroring `applied_mouse`
verbatim — same variable passed to both sites, not two independent latches (two latches would let the
in-loop call re-emit what the pre-draw call already wrote).

**[CONSEQUENCE C-5] Ordering vs. draw.** `reconcile_mouse_capture` today runs BEFORE the frame
`draw`. The caret POSITION is set inside `draw` (by `place_cursor`/overlays); the caret STYLE is set
by our reconcile. DECSCUSR (style) and cursor-position are independent terminal state, so order
between them does not matter for correctness — the style persists across position moves. We keep the
reconcile in the pre-draw band with its sibling for consistency. State this in the plan so no one
"fixes" a non-bug by moving it into the draw.

### 4.3 Desired-style composition (decision #2 + #3 + #4)

`desired_caret_style` returns our own `(CaretShape, bool)` pair; `to_set_cursor_style` maps a concrete
pair to the crossterm variant only at the `execute!` call (blink is meaningful only for a concrete
shape):

| `caret_shape` | `caret_blink` | `desired_caret_style` | `to_set_cursor_style` |
|---|---|---|---|
| `Default` | any | `None` — **emit nothing** (decision #3; blink inert) | (`Default` ⇒ `DefaultUserShape`, unused via desired) |
| `Block` | `true` | `Some((Block, true))` | `BlinkingBlock` |
| `Block` | `false` | `Some((Block, false))` | `SteadyBlock` |
| `Beam` | `true` | `Some((Beam, true))` | `BlinkingBar` |
| `Beam` | `false` | `Some((Beam, false))` | `SteadyBar` |
| `Underline` | `true` | `Some((Underline, true))` | `BlinkingUnderScore` |
| `Underline` | `false` | `Some((Underline, false))` | `SteadyUnderScore` |

Reconcile body:

```rust
let desired = desired_caret_style(editor); // Option<(CaretShape, bool)>
match desired {
    Some(style) if *applied != Some(style) => {
        let cs = to_set_cursor_style(style.0, style.1);
        if crossterm::execute!(backend, cs).is_ok() {
            *applied = Some(style);            // latch OUR pair (Copy + PartialEq)
            restore::mark_written();           // §5 — first real write arms restore
        }
    }
    None if applied.is_some() => {
        // shape moved BACK to Default at runtime: restore to terminal default so the
        // caret stops reflecting a style we no longer manage.
        if crossterm::execute!(backend, crossterm::cursor::SetCursorStyle::DefaultUserShape).is_ok() {
            *applied = None; // we no longer manage a concrete style
        }
    }
    _ => {} // desired == applied, or (None && applied None): ZERO writes at rest.
}
```

**Idle-free guarantee:** on any iteration where the options are unchanged, `desired == *applied`
(or both express "unmanaged") → the reconcile writes nothing. This is pinned by a guardrail test
(§7.4): call the reconcile twice with a `Vec<u8>` backend; the second call's buffer must be empty.

**[CONSEQUENCE C-6] Runtime `→ Default` restores to DefaultUserShape** (the `None if
applied.is_some()` arm). This means once C1 has managed the caret in a session, switching the option
back to `default` still emits ONE escape (to un-manage) rather than leaving the last concrete shape
stuck. This is correct behavior but note: it means `mark_written()` stays armed for the rest of the
session (we DID write), so exit-restore still fires — which is what we want (we last set a shape, so
we should hand back the terminal default on exit).

---

## 5. Restore (decision #8) — latch-aware, panic-safe

### 5.1 The process-global "ever wrote" flag

Because the panic hook is a `'static` closure with no `&Editor` (C-3), the latch is a module static
in `cursor_style.rs`:

```rust
mod restore {
    use std::sync::atomic::{AtomicBool, Ordering};
    static EVER_WROTE: AtomicBool = AtomicBool::new(false);
    /// Called by the reconcile the first (and every) time it successfully writes a concrete style.
    pub fn mark_written() { EVER_WROTE.store(true, Ordering::Relaxed); }
    /// True iff the reconcile ever emitted a DECSCUSR style this process.
    pub fn ever_wrote() -> bool { EVER_WROTE.load(Ordering::Relaxed) }
    /// Emit DefaultUserShape iff we ever wrote — used by all three term.rs restore sites.
    pub fn restore_caret_if_written<W: std::io::Write>(backend: &mut W) {
        if ever_wrote() { let _ = crossterm::execute!(backend, SetCursorStyle::DefaultUserShape); }
    }
}
```

`Ordering::Relaxed` is sufficient: the flag is a monotone one-way latch (false→true), the writer is
the main loop, and the readers are the same thread (Drop, setup-rollback) or the panic hook (which
§term.rs already gates to the main thread via `should_handle_panic`). No cross-thread ordering
dependency exists.

### 5.2 The three managed `term.rs` edits

Insert a `cursor_style::restore::restore_caret_if_written(&mut io::stdout())` (or the backend in
hand) into each of the three full-chain restore paths, alongside the existing `Show`:
1. `TerminalGuard::Drop`.
2. `TerminalGuard::new`'s `Terminal::new` failure (full-chain) rollback arm.
3. `install_panic_hook`'s hook body (after `dump_on_panic`, before/with the existing restore
   `execute!`).

The fourth `EnterAlternateScreen`-failure rollback path (§2.6) is deliberately NOT edited: the latch
is provably `false` there, so a restore call would be a guaranteed no-op. `restore_caret_if_written`
would in fact be harmless if added (it self-guards on `ever_wrote()`), but we leave that path minimal
to keep the "three managed sites" claim exact and the early-failure path untouched.

**Honesty caveat (decision #6, restated for restore):** we restore only to `DefaultUserShape` — no
DECSCUSR query exists, so a style the user's shell/tmux set before launching wcartel cannot be
recovered. This matches Helix/Neovim. The latch keeps the promise of decision #3 airtight: if
`caret_shape` stayed `Default` all session (we never wrote), `ever_wrote()` is false and **all three
managed restore sites emit nothing** — wcartel never touches the caret, including on the way out.

---

## 6. B11 — modal/overlay caret (decision #7)

### 6.1 Arm-3 hide-guard

`place_cursor` arm 3 (the editor-caret fallthrough) is gated so it does NOT place while any
input-owning surface is active. Concretely, arm 3 runs only when none of the overlay/modal fields is
`Some`. Those surfaces split into three kinds: **place-their-own-caret** (search/minibuffer via arms
1–2; the four query overlays and the **cursor picker's sample cell** via their own
`render_overlays::paint` placements), and **hide-the-caret** (menu/prompt/splash/diag — no text
field, no sample cell). The guard is a single boolean expression over the `Editor` option fields
(`search`, `minibuffer`, `palette`, `outline`, `theme_picker`, `file_browser`, `menu`, `prompt`,
`splash`, `diag`, and the new `cursor_picker`). Search/minibuffer keep their own arms (1, 2); the
guard prevents arm 3 from firing under the rest. Note the cursor picker is a caret-**placing** surface
(it owns a sample-cell caret — §6.3, §8.4), not a hide surface: suppressing arm 3 hands caret
ownership to the picker's own sample-cell placement, which is exactly what makes the live morph
visible.

**Design:** add a helper `Editor::has_active_input_overlay(&self) -> bool` (or a `place_cursor`-local
predicate) enumerating the surfaces, so the census lives in one place and the test (§7.5) asserts it
exhaustively. This avoids a `_`-style silent gap if a future overlay is added (house rule: no
catch-all that absorbs a new variant).

### 6.2 Overlay query-caret placements

Each of the four query overlays places its own caret inside its `render_overlays::paint` arm, on its
local `query_area` (which it already computes), AFTER drawing the `> {query}` paragraph. Because
overlay paint runs after `place_cursor` and last-write-wins, this overrides arm 3's (now-suppressed)
placement and lands the caret in the field:
- **palette:** column = `query_area.x + prefix_cols + palette.query[..palette.cursor].chars().count()`
  where `prefix_cols = "> ".chars().count()` (= 2). Mid-string cursor honored (C-4).
- **outline / theme_picker / file_browser:** column = `query_area.x + prefix_cols +
  query.chars().count()` (end-of-query — these edit end-only).
- Row = `query_area.y`. Guard the column against `query_area`'s right edge (clamp/skip past width),
  mirroring `place_cursor`'s existing `< w` guards, so an over-long query hides rather than wraps.

**[CONSEQUENCE C-7] Column math must use char counts, not byte offsets** (multibyte queries), and
must share the `"> "` prefix width with the painter to avoid painter/caret drift — the same
single-source discipline `place_cursor` already applies via `chrome_geom::search_field_prefix_cols`.
The prefix is a literal `"> "` here (2 cols); the plan should factor it to one const so paint and
caret can't diverge.

### 6.3 The cursor picker as a caret-PLACING surface (the sample cell)

The cursor picker is a **fixed short list** (no query field, arrow-navigated — §8), but it is NOT a
hide-the-caret surface: Fork 5-C ("both" — descriptive rows AND a visible live morph) **requires a
real caret on screen** for the run-loop `reconcile_cursor_style` to morph as the selection changes.
Since ratatui hides the hardware cursor on any frame with no `set_cursor_position`, an overlay that
placed no caret would silently degrade "both" to observe-on-commit.

**Resolved (the previously-deferred "live-sample-cell vs observe-on-commit" question — decided in
favor of the sample cell, as Fork 5-C dictates):** the picker renders a dedicated **sample cell** —
a `Preview: ▮`-style row/cell inside the picker overlay whose column/row the picker owns — and places
`frame.set_cursor_position` **at that sample cell**, inside its `render_overlays::paint` arm, AFTER
`place_cursor`. The run-loop reconcile then morphs THAT visible caret to the selected shape/blink
each iteration (the preview funnel sets the options → reconcile emits the DECSCUSR → the sample-cell
caret changes shape in place). This is the picker's own caret; the arm-3 guard suppresses the
text-area caret so the sample cell is the sole caret while the picker is open.

**Honest on a DECSCUSR-ignoring terminal:** the sample cell also paints a descriptive glyph + label
for the selected row (the "descriptive rows" half of "both"), so a terminal that ignores the style
escape still shows WHAT the selection is — the caret simply won't visibly morph there. The picker is
added to the arm-3 census as a caret-placing surface.

---

## 7. Commands & command-surface-contract conformance

**This effort touches commands, user-settable options, the palette, and the menu — full conformance
is mandatory and enumerated here.**

### 7.1 New commands (exact ids, labels, menu placement)

Registered in `registry::Registry::builtins`, in the `View` category band beside the scrollbar rows:

**`caret_shape` — 4-state (Rule 8: set-per-state primitives + a stateful cycle representative):**
- `caret_shape_default` — "Caret Shape: Default" — `menu: None` — `set_caret_shape(Default)`
- `caret_shape_block`   — "Caret Shape: Block"   — `menu: None` — `set_caret_shape(Block)`
- `caret_shape_beam`    — "Caret Shape: Beam"    — `menu: None` — `set_caret_shape(Beam)`
- `caret_shape_underline` — "Caret Shape: Underline" — `menu: None` — `set_caret_shape(Underline)`
- `cycle_caret_shape` — "Caret Shape" — `menu: Some(View)` — `register_stateful`, state
  `MenuMark::Value(caret_shape_str(e.caret_shape))`, handler cycles **Default → Block → Beam →
  Underline → Default** (decision #4: first stop off Default is Block).

**`caret_blink` — 2-state (Rule 8: set-per-state primitives + a stateful toggle representative):**
- `caret_blink_on`  — "Caret Blink: On"  — `menu: None` — `set_caret_blink(true)`
- `caret_blink_off` — "Caret Blink: Off" — `menu: None` — `set_caret_blink(false)`
- `toggle_caret_blink` — "Caret Blink" — `menu: Some(View)` — `register_stateful`, state
  `MenuMark::OnOff(e.caret_blink)`, handler flips the bool via `set_caret_blink`.

**Picker-open:**
- `cursor` — "Caret…" — `menu: Some(View)` — `Editor::open_cursor_picker()` (opens the picker,
  enforces overlay XOR). Named `cursor` (not `caret`) to read naturally beside `theme` / `palette`;
  the label uses "Caret…" for the writer-facing term.

### 7.2 Law-by-law conformance

- **LAW 1 (registry is SSOT).** All caret state mutates only through the two setters, reached only
  via registered commands (and the picker, which calls the same setters). No out-of-registry mutation.
- **LAW 2 (every option is a command).** Two new persisted fields (`caret_shape`, `caret_blink`)
  each map to commands. The exhaustive-destructure guard in
  `settings::every_persisted_setting_has_a_command` gains two arms:
  `assert!(has("cycle_caret_shape") && has("caret_shape_block"), "view_caret_shape")` and
  `assert!(has("toggle_caret_blink") && has("caret_blink_on"), "view_caret_blink")`. Adding the
  snapshot fields makes `field_guard`'s destructure fail to compile until they're added — the
  intended recurrence trip.
- **LAW 3 (palette exhaustive).** All eight new commands (four sets, two blink sets, two
  representatives) plus `cursor` are non-hidden → they appear in the palette automatically; the
  palette-completeness test enforces. The picker is **not the only door** to any state — the
  set-per-state primitives guarantee palette reachability of every value.
- **LAW 4 (menu ⊆ palette).** Menu gets exactly the curated subset: `cycle_caret_shape`,
  `toggle_caret_blink`, `cursor` (View category). All are in the palette (Law 3). The six
  set-per-state primitives are `menu: None` (palette-only).
- **LAW 5 (every mouse affordance has a keyboard path).** Falls out of Law 3; the picker's mouse
  wheel/click (cloned from theme picker) mirror keyboard nav.
- **LAW 6 (one setter; profiles use it).** `set_caret_shape` / `set_caret_blink` are the sole
  mutators — commands, the picker preview/commit, and startup config apply all call them. No bypass.
- **RULE 8 (multi-state shape).** `caret_shape` (4-state) = set-per-state primitives + a `cycle`
  representative with `MenuMark::Value` state-in-label; `caret_blink` (2-state) = set-per-state
  primitives + a `toggle` representative with `MenuMark::OnOff`. Matches the scrollbar/status_line
  precedents exactly.
- **LAW 7 (hints track active keymap).** C1 ships **no default chord** for any caret command
  (access is via palette/menu/picker), so hint re-resolution is inherited for free — the
  `hints_reresolve_on_preset_switch` invariant is unaffected. **Any** binding a user adds via a
  keymap patch re-resolves normally (LAW 7 is a property of the hint renderer, not of C1). Stated so
  the Codex gate sees the N/A is deliberate, not an omission.
- **RULE 10 (commands are the plugin/automation spine).** All new commands are nullary (the set
  variants are deterministic set-to-X primitives), so Effort P can later collapse the four
  `caret_shape_*` into one parameterized `set_caret_shape("beam")` without breaking this contract —
  the set-value semantics are kept clean for exactly that.

### 7.3 Persistence wiring (mirror scrollbar)

- `settings::SettingsSnapshot`: add `view_caret_shape: CaretShape`, `view_caret_blink: bool`.
- `settings::OView`: add `caret_shape: Option<String>`, `caret_blink: Option<bool>`.
- Snapshot-from-config and snapshot-from-editor: populate both from `cfg.view` / `editor`.
- Diff law: a `diff_key` entry for `caret_shape` (string via `caret_shape_str`) and a bool diff for
  `caret_blink`, folded into the `any_view` OR that gates emitting an `OView`, and destructured in
  the settings round-trip guards.
- `config.rs` override apply: `"default"/"block"/"beam"/"underline"` → `CaretShape` (warn-on-unknown,
  coerce to `Default`); `caret_blink` bool.

### 7.4 Tests for the option surface

- `every_persisted_setting_has_a_command` extended (LAW 2).
- Cycle/set command tests (mirror `scrollbar_commands_set_and_cycle`): dispatch each set command,
  assert the field; dispatch `cycle_caret_shape` from Default and assert Default→Block→Beam→
  Underline→Default; `toggle_caret_blink` flips.
- Palette-completeness (automatic; no new test needed beyond registration).
- Diff-law round-trip test for both fields (mirror `scrollbar_status_line_round_trip_via_diff_law`).

---

## 8. The cursor picker (`cursor_picker.rs`)

### 8.1 Module + state

New `wordcartel/src/cursor_picker.rs`, cloned in shape from `theme_picker.rs`:

```rust
pub struct CursorPicker {
    pub selected: usize,          // index into the fixed row list
    /// Options captured on open — restored on Esc (preview cancel).
    pub original_shape: CaretShape,
    pub original_blink: bool,
}
```

Rows are a **fixed list** (no query filter needed — the option space is tiny): the descriptive glyph
rows (decision #5, "both"). The row→action mapping is **total over BOTH persisted fields** and is the
single source both preview and commit apply. Each row maps to `(CaretShape, Option<bool>)` where the
`Option<bool>` is the blink action: `Some(b)` sets blink, `None` leaves `caret_blink` unchanged:

| row | label | glyph mock | shape action | blink action |
|---|---|---|---|---|
| 0 | Default (terminal) | (dim `default` sample) | `Default` | **`None` — leave `caret_blink` unchanged** |
| 1 | Block · blinking | `▉` | `Block` | `Some(true)` |
| 2 | Block · steady | `▉` | `Block` | `Some(false)` |
| 3 | Beam · blinking | `▏` | `Beam` | `Some(true)` |
| 4 | Beam · steady | `▏` | `Beam` | `Some(false)` |
| 5 | Underline · blinking | `▁` | `Underline` | `Some(true)` |
| 6 | Underline · steady | `▁` | `Underline` | `Some(false)` |

**[IMPORTANT resolution] Row 0 leaves `caret_blink` untouched.** `caret_blink` is an independent
persisted field that always holds a concrete value; because shape=`Default` makes blink INERT
(decision #3, emits nothing), row 0 has no honest concrete blink to assert. It therefore sets
`shape=Default` only (`blink = None`), PRESERVING the user's current blink preference — a user who set
"blink off", visits Default, and leaves keeps "blink off" latent for when they next pick a concrete
shape. Every other row sets both fields (`Some(_)`). The preview/commit funnel applies the row action
as: `set_caret_shape(shape); if let Some(b) = blink { set_caret_blink(b); }` — one code path, total
over the table, no row leaving a field in an undefined state.

(The plan finalizes the exact glyphs; they are DESCRIPTIVE cells painted in the row, honest even on a
DECSCUSR-ignoring terminal.) **Refined per the final-gate F2 resolution (see History):** the picker
opens on the row matching the CURRENT caret state, not a fixed "first managed" row — from `Default`
that is row 0 (the `Default` row itself), not row 1 (blinking block). Decision #4's *cycle* clause
(`cycle_caret_shape` enters `block` first off `default`) is UNCHANGED; only the *picker's* initial
highlight is refined.

**[CONSEQUENCE C-8] The picker collapses shape×blink into a 7-row combined list**, even though the
persisted OPTIONS are two orthogonal fields (§3). This is a UI convenience only — the picker's
preview/commit apply the `row → (shape, Option<blink>)` table above via the two setters. This keeps
the picker a single scannable list while the command/config surface stays orthogonal (decision #2).
The single `ROW_ACTIONS` table is the one source both the render (labels/glyphs) and the funnel
(setters) read, so rows and setters cannot drift.

### 8.2 Summon, XOR, intercept registration

- `Editor::open_cursor_picker()` sets `cursor_picker = Some(..)` capturing `original_shape/blink`,
  and enforces overlay XOR (clears other overlays — mirror `open_theme_picker`). Add `cursor_picker:
  Option<CursorPicker>` to `Editor`. `selected` is seeded from `cursor_picker::initial_row_for(shape,
  blink)`, **which opens on the row matching the CURRENT caret state** (row 0 for `Default`; else the
  concrete `(shape, Some(blink))` match, falling back to row 0) — refined per the final-gate F2
  resolution (see History). The highlight therefore always matches the live caret on open, so opening
  the picker writes nothing (no preview-on-open needed) and Enter-immediately-after-open commits
  exactly the state that was already active — the highlight never lies about what Enter would do.
- `cursor_picker::intercept(msg, editor, ex, clock, msg_tx) -> Handled` added as ONE new stage in
  `app::reduce_dispatch`'s interceptor chain (next to `theme_picker`). Handles Esc/Enter/list-nav
  (via `list_window::apply_list_nav`). It swallows BOTH paste arms as no-ops — `Msg::ClipboardPaste`
  (mirroring theme_picker's no-op drop of the async result) AND `Msg::Input(Event::Paste(_))`, the
  latter a no-op *because the cursor picker has no query field to append to* (UNLIKE theme_picker,
  which appends `Event::Paste` text to its query); both must be consumed so a bracketed paste cannot
  fall through to document insertion. No query char input (fixed list) — char keys are ignored/pass
  per the picker's design.
- The `menu` command's overlay-clear block (which nils palette/prompt/minibuffer/search/diag/
  outline/theme_picker/file_browser) must also nil `cursor_picker`.

### 8.3 Preview funnel (decision #5 — live morph on the sample cell + glyph rows)

- `preview_selected` fires on each nav (list-nav key or wheel), NOT on open — the row `initial_row_for`
  seeds already matches the live caret (F2, see History), so opening the picker writes nothing; a real
  live morph starts only once the user moves the selection. On every selection move, `cursor_picker`
  calls its preview: apply the selected row's `ROW_ACTIONS` entry via the setters —
  `set_caret_shape(shape); if let Some(b) = blink { set_caret_blink(b); }`. The run-loop reconcile
  (§4) then emits the DECSCUSR the same iteration and
  the **sample-cell caret** (§6.3, §8.4) morphs in place. The morph is VISIBLE because the picker
  places `frame.set_cursor_position` at its sample cell every frame (arm 3 is suppressed, so the
  sample cell owns the only caret on screen). This is the concrete resolution of Fork 5-C's "live
  morph" — a real, on-screen caret that changes shape as you arrow. The DESCRIPTIVE glyph + label in
  each row (and in the sample cell) is always painted, so on a DECSCUSR-ignoring terminal the
  selection is still fully legible even though the sample caret won't visibly change shape (decision
  #6).
- **Esc:** restore `original_shape`/`original_blink` via the setters (reconcile then restores the
  live sample caret next iteration, then arm 3 resumes owning the text-area caret once the picker
  closes). Close the picker.
- **Enter:** commit — the options already hold the previewed values (they were set live, or — if no
  nav happened — they already held the current state that `initial_row_for` matched on open); just
  close the picker. (No separate identity to record, unlike the theme picker's `previewed` name — the
  values ARE the state.) Because commit leaves the options as-previewed, the settings-save path
  persists them on the next `settings_save_requested` cycle exactly like any option change.
  Enter-immediately-after-open is therefore a true no-op that commits exactly the caret state the
  user already had (F2) — never a silent jump to an unrelated managed row.

**[CONSEQUENCE C-9] Live preview writes DECSCUSR on each arrow** (through the reconcile's normal
edge-trigger). This is bounded by user keypresses (not wall-clock) → still idle-free and
proportional-to-input; no new timer. On a DECSCUSR-ignoring terminal the live morph is a silent
no-op and the glyph rows carry the meaning — exactly decision #6. State both in the plan so a
reviewer doesn't read the per-arrow write as a spin.

### 8.4 Render + mouse

- Render arm in `render_overlays::paint` (bordered box; the row list with glyph + label; **plus a
  dedicated sample cell** — a `Preview: ▮`-style row/cell whose column/row the picker owns and can
  hand to `set_cursor_position`; no query line). The sample cell paints the selected row's glyph +
  label (the descriptive half) and is the anchor for the live caret.
- **Caret placement:** after painting, the picker calls `frame.set_cursor_position` at the sample
  cell (row/col of the `Preview:` value), the same last-write-wins-after-`place_cursor` mechanism the
  four query overlays use (§6.2). This is the ONLY caret on screen while the picker is open (arm 3 is
  suppressed). The picker is therefore a caret-PLACING surface in the arm-3 census (§6.1/§6.3) — the
  one that makes the live morph visible. Guard the sample-cell column against the overlay's right
  edge, mirroring the query-overlay clamp.
- Mouse wheel/click in `mouse.rs` mirror the theme-picker handlers (wheel → move selection +
  preview; click → commit), reusing the shared preview/commit funnel.

---

## 9. Module structure & anti-regrowth

| Change | Landing zone | Seam respected |
|---|---|---|
| Option enum + config fields + override parse | `config.rs` | data types + existing override match arms |
| Runtime fields + two setters + `open_cursor_picker` | `editor.rs` | field cluster + methods (not a dispatcher) |
| 9 command rows | `registry::builtins` | **data-table rows** — the sanctioned growth spot |
| Persistence (snapshot/OView/diff/LAW-2) | `settings.rs` | mirrors scrollbar; the destructure guard is the gate |
| `desired_caret_style` + `reconcile_cursor_style` + restore latch/static | **new `cursor_style.rs`** | one file, one responsibility (§4.1) |
| TWO reconcile calls (pre-first-draw + in-loop) + one shared latch local | `app::run` | +~3 lines vs the 1000-line budget (safe; verify at plan); mirrors `applied_mouse`'s two sites |
| 3 restore-site edits | `term.rs` | one line each into existing `execute!` chains |
| Arm-3 hide-guard + 4 overlay caret placements | `render.rs::place_cursor`, `render_overlays::paint` | bounded, non-dispatcher; guard census in one predicate |
| Picker module | **new `cursor_picker.rs`** | one file; +1 interceptor stage (registration seam) |

`app.rs` (≤1000) and `render.rs` (≤900) budgets: C1 adds only a couple of lines to each hub;
`clippy::too_many_lines` (100) is respected because the new logic lives in the new modules and in
short per-arm additions. **Plan must re-check both budgets after edits** (they are GATE tests).

---

## 10. Testing

- **LAW 2:** extend `every_persisted_setting_has_a_command` (the exhaustive destructure fails to
  compile until the two snapshot fields get arms — intended).
- **Palette-completeness:** automatic on registration (existing test).
- **Command tests:** each set-per-state primitive sets its field; `cycle_caret_shape` walks
  Default→Block→Beam→Underline→Default; `toggle_caret_blink` flips; mirror
  `scrollbar_commands_set_and_cycle`.
- **Diff-law round-trip:** both fields, mirror `scrollbar_status_line_round_trip_via_diff_law`.
- **Reconcile unit tests (`Vec<u8>` backend):** (a) `Default` shape → zero bytes written;
  (b) `Block`+blink → writes once, latch set; (c) **no-write-at-rest guardrail** — reconcile twice,
  second call's buffer empty (the idle-free pin); (d) runtime `→ Default` after a managed shape
  emits `DefaultUserShape` once then rests; (e) `desired_caret_style` composition table asserted for
  all 7 shape×blink combos.
- **Restore latch tests:** `restore::ever_wrote()` false before any write, true after
  `mark_written()`; `restore_caret_if_written` emits nothing when never written, `DefaultUserShape`
  when written (`Vec<u8>` backend).
- **Startup-apply (C-10):** with a config-persisted concrete shape (e.g. `beam`), the pre-first-draw
  reconcile emits the style once against a fresh `applied = None` latch (`Vec<u8>` backend, unit-level
  — the two `run` call sites themselves are integration-covered by the e2e journey / smoke, since
  `run` owns the terminal). Asserts a persisted shape is not deferred to the first event.
- **B11 `TestBackend::cursor_position` assertions** (the `render.rs` precedent) for EVERY
  input-owning surface: caret in-field for palette (incl. a mid-string `cursor`), outline,
  theme_picker, file_browser; **caret at the sample cell for the cursor picker** (position asserted
  against the picker's known sample-cell coordinates — this pins that Fork 5-C's live morph has a
  real on-screen caret to morph, independent of the DECSCUSR byte); caret hidden/absent for menu,
  prompt, splash, diag; unchanged for search, minibuffer; and the editor caret correctly suppressed
  (arm-3 guard) while any of these is open. A single table-driven test enumerating the census makes
  an added-but-unguarded future overlay fail loudly.
- **e2e picker journey** (`e2e.rs`, in-process `reduce → advance → render` on `TestBackend`):
  open cursor picker → assert the sample-cell caret is placed (position) → move selection (preview
  applies the row's `(shape, Option<blink>)` action; assert `caret_shape`/`caret_blink` updated,
  including that visiting row 0 leaves `caret_blink` unchanged from a prior "off") → Esc → assert
  `caret_shape`/`caret_blink` == originals; reopen → move → Enter → assert options == previewed and
  that a settings-save persists them.

**Limitations to state in the spec (honesty for the gate):**
- **DECSCUSR bytes bypass `TestBackend`.** The escape is written to the crossterm backend, not the
  ratatui cell buffer, so e2e `TestBackend` journeys cannot observe the actual style byte — the
  `Vec<u8>` reconcile unit tests are the mechanism-level coverage, and **`scripts/smoke` S7
  (panic → restore) is the advisory real-terminal eyeball** (mandatory-run / advisory-pass per the
  PTY-smoke policy).
- No test can assert a terminal *honored* DECSCUSR (decision #6, no detection) — coverage is "we
  emitted the right bytes," not "the terminal changed."

---

## 11. Consequences-of-grounding, collected (for the Codex gate)

- **C-1** Arm 3 currently has no overlay gating → the B11 hide-guard must enumerate ALL
  input-owning surfaces (arm 3 is the fallthrough).
- **C-2** DECSCUSR encodes blink into the shape code; no separate blink escape, no blink speed, no
  style query → two options compose at write time; restore is DefaultUserShape only.
- **C-3** The panic hook is `'static`/no-`&Editor` → the restore latch must be a process-global
  static, not an `Editor` field.
- **C-4** Palette is the only overlay that EDITS mid-string; outline also has a `cursor` field but
  `set_query` pins it to the query end, so outline/theme_picker/file_browser are all end-of-query →
  two distinct column formulas (mid-string `query[..cursor]` for palette; end for the rest).
- **C-10** `reconcile_cursor_style` runs at BOTH `reconcile_mouse_capture` sites (pre-first-draw +
  in-loop) sharing one `run`-local latch (mirrors `applied_mouse`), so a config-persisted shape shows
  on frame one; both edge-triggered → idle-free intact.
- **C-11** crossterm 0.28's `SetCursorStyle` is `Clone, Copy` but NOT `PartialEq` → the edge-trigger
  latch stores OUR `(CaretShape, bool)` pair (derives `Copy + PartialEq + Eq`); `to_set_cursor_style`
  maps to the crossterm variant only at the `execute!` call.
- **C-5** Reconcile stays in the pre-draw band (style vs. position are independent terminal state);
  do not "fix" its order into `draw`.
- **C-6** Runtime `→ Default` emits one `DefaultUserShape` to un-manage, and leaves the restore latch
  armed for exit — intended.
- **C-7** Overlay caret columns use char counts and a single-source `"> "` prefix width (no
  painter/caret drift).
- **C-8** The picker is a 7-row combined shape×blink list over two orthogonal persisted fields — one
  total `row → (shape, Option<blink>)` table is the single source; **row 0 (Default) leaves
  `caret_blink` unchanged** (`None`), preserving the user's blink preference under the inert shape.
- **C-9** Live preview writes DECSCUSR per arrow (input-bounded, still idle-free), morphing the
  picker's **sample-cell caret** (the on-screen caret Fork 5-C's live morph requires — arm 3 is
  suppressed so the sample cell owns the caret); on an ignoring terminal the morph is a silent no-op
  and the sample cell's glyph/label carries the meaning.

---

## 12. Out of scope (restated)

Painted caret / brightness / theme color (CUT 2026-07-13); blink speed (not in DECSCUSR); OSC 12
color; B12/B13 marker visuals (C2 — disjoint code); idle caret dimming; focus events / unfocused
panes (no panes; terminal natively hollows the caret); typewriter/focus/measure/marked-block
interaction (none — the hardware caret positions identically under all and natively overlays painted
cells). Effort P will later parameterize the set-value commands (Rule 10) — kept clean for that,
built now as nullary.

---

## 13. Self-review (brainstorming skill's spec checklist)

- **Placeholder scan:** no TODO/TBD/`???`/`<placeholder>` remain; the one remaining "plan
  finalizes" item (the exact glyph characters for the row/sample-cell mocks) is a cosmetic detail,
  not an open design fork — the eight forks are all resolved in §1, and the previously-deferred
  preview mechanism is now decided (sample cell — §6.3/§8.4).
- **Internal consistency:** option model (§3) ↔ composition (§4.3) ↔ commands (§7.1) ↔ persistence
  (§7.3) ↔ picker (§8) all reference the same two fields and the same setters; the LAW-2 arms name
  the exact command ids registered in §7.1; the picker preview/commit and the arm-3 census agree
  that the picker is a caret-PLACING (sample-cell) surface (§6.1/§6.3/§8.3/§8.4).
- **Scope:** every §0/§1 promise has a mechanism section; nothing outside the eight decisions is
  introduced; OUT list (§12) fences the cut items.
- **Ambiguity:** the one genuine plan-level deferral (the exact glyph characters) is cosmetic and
  bounded; the Fork 5-C preview mechanism, the row→(shape,blink) totality, the restore sites, and
  all contract/persistence mechanics are fully specified.
- **Grounding:** every cited symbol (`place_cursor`, `reconcile_mouse_capture` + its two call sites
  and shared `applied_mouse` latch, `SetCursorStyle`, `reduce_dispatch` chain, `ThemePicker`,
  `OutlineOverlay::{cursor, set_query}`, `SettingsSnapshot`/`OView`,
  `every_persisted_setting_has_a_command`, the three managed `term.rs` sites + the fourth exempt
  path, `MenuMark`, `register_stateful`) was re-verified in the tree for this spec.

---

## History

- **2026-07-14 (final-gate F2 resolution):** the whole-branch pre-merge gate flagged that the picker
  opened with row 1 (blinking block) highlighted even when the live caret was `Default`, but preview
  did not fire on open — so Enter-immediately-on-open silently committed `block · blinking`, a state
  the highlight implied but the user never asked for. Resolution (human-approved): **the picker opens
  on the row matching the CURRENT caret state** (`cursor_picker::initial_row_for` now returns row 0
  for `Default`, or the matching concrete row, instead of always landing on row 1). This refines
  decision #4's *picker* clause only (§1 item 4, §8.2, §8.3, and the row-table note in §8) — the
  *cycle* clause (`cycle_caret_shape` enters `block` first off `default`) is UNCHANGED. `preview_selected`
  continues to fire on nav (live morph, Fork 5-C) but not on open, since open now seeds a highlight that
  already matches the caret.
