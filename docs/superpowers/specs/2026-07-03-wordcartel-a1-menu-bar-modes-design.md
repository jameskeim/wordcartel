# A1: menu bar modes (hidden | auto | pinned) + dwell reveal — design

**Status:** spec-review CLEAN (Codex x3 + Fable5; r3 residual — the pin toggle REQUIRES Editor.menu_bar_unpinned_mode, Ctx has no Config — folded); ready for user review + planning
**Date:** 2026-07-03
**Effort:** a1-menu-bar-modes — the second effort off `docs/ux-backlog.md` (A1; design settled at
the 2026-07-03 triage, `auto` default confirmed; the one open fork — reveal geometry — resolved
2026-07-03: **reserve uniformly**).

## Context

Today the menu bar exists only while the dropdown is open: `editor.menu: Option<MenuView>`
conflates "bar painted at row 0" with "dropdown open" (the bar renders only inside
`if let Some(ref menu)`, render.rs:906; row-0 reservation is `u16::from(editor.menu.is_some())`,
render.rs:248). F10 is the only door (keymap.rs:243 → the `"menu"` toggle command,
registry.rs:210-227). Mouse plumbing for the OPEN menu is complete (mouse.rs:115-142), but a
closed bar cannot be summoned by mouse, kept visible, or discovered.

Two map findings shape the design:
1. **Motion events already arrive.** crossterm 0.28.1's `EnableMouseCapture` enables mode 1003
   (any-event tracking) — `MouseEventKind::Moved` events flow from all modern terminals into
   `mouse::handle` and die in the `_ => {}` catch-all (mouse.rs:258). Dwell detection is a new
   arm, not new plumbing.
2. **A pre-existing geometry bug.** Four nav.rs sites compute edit-height as `area.1 - 1`
   WITHOUT subtracting the menu row (`screen_pos` :90, `ensure_visible` :403 — INCLUDING its
   typewriter branch, `page_step` :761, `last_fully_visible_line` :795): with the menu open
   TODAY the caret can become invisible at the bottom edge, `ensure_visible` under-scrolls
   (plain and typewriter anchoring), and PageUp/Down overstep by one row. Transient today;
   PERMANENT under a pinned bar. A1 fixes it via the single geometry accessor — a behavior
   fix the merge report must state.

## Goals

- `[menu] bar = "hidden" | "auto" | "pinned"` (default **auto**): `hidden` = today; `auto` =
  bar appears when the pointer RESTS on row 0 for a dwell, hides after a short leave-grace (user decision A); `pinned` = bar
  always visible-inactive.
- **Reserve uniformly** (resolved fork): bar visible ⇒ row 0 reserved — one signal through all
  geometry; the one-row shift is the same one F10 already causes today.
- F10 stays the universal dropdown open/close; Esc closes the DROPDOWN only (in `pinned` the
  bar persists).
- A session-scoped `menu_bar_pin` toggle command (View menu + palette) — niggle #3.
- Fix the nav.rs menu-row geometry bug everywhere, by construction.

## Non-goals

- No accelerators (A4 dropped), no right-edge bar content (E1), no persistence of the pin
  toggle (D1 carries that), no bar-focused-without-dropdown state, no hover-highlight of bar
  labels while inactive. Dwell + leave-grace are CONSTS (MENU_DWELL_MS, MENU_LEAVE_GRACE_MS), not config (tunables if terminals prove
  jittery). PTY smoke cannot inject motion (documented boundary; the suite still runs verbatim
  pre-merge).

## Component 1 — state, config, and the geometry accessor (+ the nav bug fix)

### Config (the focus_granularity string→enum pattern, config.rs:71/:226-230/:325-330)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuBarMode { Hidden, Auto, Pinned }

#[derive(Debug, Clone)]
pub struct MenuConfig { pub bar: MenuBarMode }
impl Default for MenuConfig {
    fn default() -> Self { MenuConfig { bar: MenuBarMode::Auto } }
}
```

`RawMenu { bar: Option<String> }` (`#[serde(default)]`); `RawConfig` gains `menu: RawMenu`;
`Config` gains `pub menu: MenuConfig`; the fold matches `"hidden" | "auto" | "pinned"` with a
warning on unknown values (mirroring config.rs:325-330). TOML section `[menu]`.

### Editor / MouseState

- `Editor.menu_bar_mode: MenuBarMode` — default in `new_from_text`, seeded in `run()` beside
  `view_opts`/`diag_cfg`/`export_cfg`.
- `MouseState` (editor.rs:322-336) gains, beside its scrollbar twins:
  `menu_reveal_due: Option<u64>` (the dwell deadline), `menu_hide_due: Option<u64>` (the
  leave-grace deadline — I3 decision A), and `menu_bar_revealed: bool` (the auto-mode
  transient; all three meaningless in other modes and cleared on mode transitions).
- `editor.menu: Option<MenuView>` KEEPS its exact meaning: dropdown open. No write site's
  semantics change (the NINE `open_*` overlay closers — minibuffer, prompt, palette, search,
  diag, outline, theme picker, buffer switcher, file browser (Codex round 1 corrected the
  count) — plus `dispatch_overlay_command`, `menu_select_for_test`, the registry `"menu"`
  toggle, and the Esc/F10 arm all still mean "close the dropdown").

### The single geometry source

```rust
/// Rows reserved by the menu bar at the top of the frame (0 or 1). THE single
/// source of row-0 geometry truth — render/mouse/nav all read this, never
/// `menu.is_some()` directly.
pub fn menu_bar_rows(&self) -> u16 {
    let bar = match self.menu_bar_mode {
        MenuBarMode::Pinned => true,
        MenuBarMode::Auto => self.mouse.menu_bar_revealed,
        MenuBarMode::Hidden => false,
    };
    u16::from(bar || self.menu.is_some())
}
```

### The geometry sweep (incl. the pre-existing bug fix)

Replace every `editor.menu.is_some()`-as-geometry read with `menu_bar_rows()`:
- render.rs:248 (`menu_rows` → `edit_top`/`edit_height`).
- mouse.rs:19 (`editing_cell` — row 0 hits `CellHit::MenuBar` only when `menu_bar_rows() == 1`,
  which by construction makes a row-0 click while unrevealed a TEXT click), :216/:229 (drag
  edge-scroll), **and :154-156 — the scrollbar CLICK path (Codex round 1: it computes
  `menu_rows`/`edit_height`/`erow_in_track` from `menu.is_some()` too; missing it would desync
  scrollbar hit mapping under a pinned/revealed bar).**
- **nav.rs — the bug fix, FIVE line-level reads (Fable M6):** `screen_pos` :90,
  `ensure_visible` :403 (typewriter branch) AND :437 (the plain branch is a SEPARATE read —
  list it explicitly or it gets missed), `page_step` :761, `last_fully_visible_line` :795.
  All currently use `area.1 - 1` with NO menu subtraction; they switch to a
  `menu_bar_rows()`-derived editing height (each has `editor` in scope — Codex-confirmed).
  Refinements: (a) typewriter anchoring is part of the bug (Codex r1); (b) `page_step`
  currently steps `editing_height.saturating_sub(1).max(1)` (a one-row context overlap) —
  the fix corrects the HEIGHT input, **preserves the `.saturating_sub(1).max(1)` overlap
  semantics verbatim**, and updates its now-stale doc comment (nav.rs:759 "matches
  nav.rs:62"). Pins: caret visible at the bottom edge with the bar shown; `page_step` = its
  current overlap formula over the corrected height; `ensure_visible` (plain AND typewriter)
  scrolls fully. **No existing test opens the menu and asserts geometry (Fable-swept) — no
  B3-style re-point sweep is expected.**
- **The split is two-sided (Codex-confirmed):** OVERLAY-state reads (`mouse.rs:115`,
  `app.rs:1137`, `registry.rs:221`) keep reading `menu.is_some()` — they mean "dropdown open,"
  not geometry, and must NOT switch.
- derive.rs:212 stays as-is (lays out one extra hidden row — harmless overdraw, documented).
- **Overlay collision — verified non-issue (Codex round 1):** palette/outline/theme-picker/
  file-browser/diag are centered rects drawn on top; search/minibuffer/prompt live on the
  status row — a pinned bar collides with none of them.

## Component 2 — pinned mode, inactive-bar rendering, commands, Esc nuance

- **Inactive-bar rendering** (render.rs, the same top-row block): when `menu_bar_rows() == 1`
  but `editor.menu.is_none()`, paint the A2 full-width Chrome fill + the category labels in
  `menu_closed_style` — NO dropdown, NO ChromeSelected highlight. Labels come from the STATIC
  category list — **visibility (corrected by Fable M5): `MENU_ORDER` is already `pub`
  (registry.rs:42) — render.rs simply adds `use crate::registry::MENU_ORDER` (a
  `menu::bar_labels()` helper remains an acceptable alternative if it reads better).**
  The label-width claim is Codex-verified: `menu_bar_layout` widths depend only on category
  labels, never group contents — so `menu_bar_layout` refactors to take the label list (or the
  helper) and hit-testing + painting share geometry in BOTH bar states.
- **Click-to-open on the inactive bar** (mouse.rs): a `Down(Left)` on row 0 with the bar
  visible-inactive opens the dropdown AT the clicked category:
  `editor.menu = Some(menu::empty_at(cat_idx))` (an `empty()` variant carrying `open`).
  **Hydration must PRESERVE `open`/`highlighted`** when replacing the placeholder
  (`hydrate_overlays`, app.rs:818-827 — today `build()` resets `open: 0`; carry the
  placeholder's values over, **clamped to the built group/item bounds** — Codex round 1).
  **CRITICAL wiring (Fable C1): the MOUSE path never hydrates today** — `hydrate_overlays`
  has exactly two production call sites (key dispatch app.rs:1716, `dispatch_overlay_command`
  app.rs:848); the `Msg::Input(Event::Mouse)` arm calls `mouse::handle` and returns without
  hydrating, so a placeholder set from a click would STRAND unbuilt: render skips the whole
  menu block on empty groups (render.rs:907 — row 0 reserved but painted empty, the A2 fill
  is inside that guard) and the next click hit-tests empty groups → silently closes. **The
  reduce Mouse arm gains a `hydrate_overlays` call after `mouse::handle`** (mirroring the
  key-dispatch site; alternative — building directly at the click site — rejected: it would
  need reg/keymap threaded into mouse and duplicate hydrate's job). Click-away and
  dropdown-row dispatch stay as they are.
- **Esc/F10 nuance:** the close arm (app.rs:1153-1154) stays `editor.menu = None` — it closes
  the DROPDOWN; in `pinned` (or `auto`-while-pointer-still-at-top) the bar persists because
  visibility flows from the mode, not from `menu.is_some()`. F10 with the dropdown closed
  still opens via the `"menu"` command (registry.rs:210-227, unchanged toggle semantics).
- **The `menu_bar_pin` command** ("Pin Menu Bar", `MenuCategory::View`, also in the palette by
  the three-surface contract): toggles `menu_bar_mode` between `Pinned` and the remembered
  non-pinned mode, stored as **`Editor.menu_bar_unpinned_mode: MenuBarMode`** — seeded at
  startup from config (`cfg.menu.bar`, with `Auto` as the fallback when config itself says
  `pinned`). **The Editor field is REQUIRED, not optional (Codex fold-verify r3): registry
  handlers receive only `Ctx { editor, clock, executor, msg_tx }` (registry.rs:26) — no
  `Config` — so a fieldless "compute from config" cannot recover the pre-pin mode at unpin
  time.** Toggle logic: `Pinned` → restore `unpinned_mode`; otherwise remember the current
  mode into `unpinned_mode` and set `Pinned`. The toggle clears all three auto fields on
  every transition (see Component 3 hygiene). Session-scoped; D1 persists it later; E2 makes
  it checkable.

## Component 3 — auto mode: the dwell machinery

Mirrors the scrollbar transient-chrome pattern (`scrollbar_until_ms` +
`recompute_scrollbar_visible` in `advance()`, app.rs:2242-2244 + :1864):

- **`pub const MENU_DWELL_MS: u64 = 250;`** and **`pub const MENU_LEAVE_GRACE_MS: u64 = 400;`**
  (named tunable consts — the grace honors the backlog's settled "hides after a grace period";
  user decision A, 2026-07-03: a one-row aiming wobble must not kill the bar and reset the
  dwell). `MouseState` accordingly gains a THIRD field, `menu_hide_due: Option<u64>`, beside
  the other two.
- **The `Moved` arm** (mouse.rs — placed **beside the universal Up-clear at mouse.rs:85-88,
  BEFORE ALL overlay branches**; Fable I1 generalized Codex round 2's finding: the palette
  (:91-114), theme-picker (:143-145), and file-browser (:146-148) blocks ALL return
  unconditionally for every mouse event, exactly like the menu block (:115-142) — an arm
  placed after any of them never sees Moved events while that overlay is open, so
  leave-bookkeeping would strand a pre-revealed bar and a pending deadline could reveal the
  bar MID-OVERLAY with the pointer nowhere near row 0) — deliberately trivial (it runs on
  every motion frame; integer compares + stores ONLY, no rebuild/redraw). **Arming
  additionally requires the mouse-intercepting overlays closed** (palette, theme-picker,
  file-browser — symmetry with the dropdown gate; Fable I1); leave-bookkeeping stays
  unconditional.
  **The predicate is RESOLVED (Codex round 1, replacing plan-confirm 7)** — the key insight:
  leave-bookkeeping must run EVEN WHILE the dropdown is open, so closing the dropdown never
  strands a stale revealed bar; arming, by contrast, only happens with the dropdown closed:

```rust
if editor.menu_bar_mode == MenuBarMode::Auto && kind == MouseEventKind::Moved {
    if ev.row > 0 {
        // Leave: disarm the dwell; arm the hide-grace ONCE (first leave only) —
        // runs regardless of dropdown/overlay state (Fable I1), so a bar revealed
        // before an overlay opened still clears after the grace.
        editor.mouse.menu_reveal_due = None;
        if editor.mouse.menu_bar_revealed && editor.mouse.menu_hide_due.is_none() {
            editor.mouse.menu_hide_due = Some(clock.now_ms() + MENU_LEAVE_GRACE_MS);
        }
    } else {
        // Back on row 0: a pending hide is cancelled (the grace's whole point).
        editor.mouse.menu_hide_due = None;
        if editor.menu.is_none()
            && editor.palette.is_none()
            && editor.theme_picker.is_none()
            && editor.file_browser.is_none()
            && !editor.mouse.dragging
            && !editor.mouse.scrollbar_dragging
            && !editor.mouse.menu_bar_revealed
        {
            editor.mouse.menu_reveal_due = Some(clock.now_ms() + MENU_DWELL_MS);
        }
    }
}
```

  **The two timers are deliberately ASYMMETRIC (I3 resolution, user decision A —
  grace per the backlog):** the DWELL re-arms on every row-0 motion (deadline tracks the
  LAST motion — reveal after the pointer RESTS), but the GRACE arms ONCE on the FIRST
  leave (`hide_due.is_none()` guard — re-arming per frame would push the hide forward
  forever while the pointer keeps moving below). Predicate unit cases pin BOTH: two row-0
  Moves 100 ms apart → reveal due = second + DWELL; two row-5 Moves 100 ms apart (while
  revealed) → hide due = FIRST + GRACE, unchanged by the second. A leave-then-return
  within the grace keeps the bar (hide cancelled).

  **The re-arm IS the rest semantics (Fable M1 — deliberate, do NOT "optimize"):** each row-0
  Moved pushes the deadline to `now + MENU_DWELL_MS`, so a pointer SLIDING ALONG row 0 keeps
  deferring the reveal and the bar appears 250 ms after the LAST motion — i.e. after the
  pointer RESTS, which is exactly the settled backlog semantics. Arming only when
  `due.is_none()` would silently change this to reveal-while-moving; a predicate unit case
  pins it (two row-0 Moves 100 ms apart → due = second + DWELL). A pointer that arms and then
  PARKS generates no further events — the deadline array still wakes the loop exactly on time
  (the sb_deadline pattern), so arm-then-park reveals correctly. (One factual note: real
  terminals report button-held motion as `Drag`, not `Moved` — the `!dragging` guards are
  belt-and-braces for lost-Up cases, not the primary drag defense.)
- **Both timers fire via the deadline machinery:** `menu_reveal_due` AND `menu_hide_due` join
  the run-loop deadline array (app.rs:2152-2183, beside `sb_deadline`) so a sleeping app wakes
  exactly on time; `recompute_menu_bar(editor, now_ms)` (handles both — reveal due fires →
  `revealed = true`, clear; hide due fires → `revealed = false`, clear; **gated on
  `mode == Auto`**, Fable M2) sits beside `recompute_scrollbar_visible` in the shared
  `advance()`. Because it lives in `advance()`, the e2e harness drives the whole flow with
  the virtual clock (inject `Moved`, `advance_ms(MENU_DWELL_MS + 1)`, `tick()`; leave-hide:
  `Moved(row 5)` + `advance_ms(MENU_LEAVE_GRACE_MS + 1)` + `tick()`). A hide firing while the
  dropdown is OPEN is harmless by construction (`menu.is_some()` keeps the row reserved; the
  bar simply doesn't linger after close).
- **Wheel events never arm** (separate `ScrollUp/Down` kinds). **A row-0 click while
  unrevealed is a text click** by construction (`menu_bar_rows() == 0` → row 0 is text).
- **Degradation:** `mouse_capture == false` (mouse.rs:76-78 early-return) or a terminal
  without mode-1003 motion reporting → the arm never fires; `auto` behaves exactly as
  `hidden`; F10 intact. **Capture-toggle stranding (Fable I2):** `reconcile_mouse_capture`'s
  DISABLE branch (app.rs:2251-2267) clears only the drag flags today — with a bar revealed
  (or a deadline pending) at the moment capture turns off, no Moved can ever hide it and the
  pending due could reveal it AFTER capture is gone. The disable branch additionally clears
  ALL THREE menu-bar fields (`menu_reveal_due`, `menu_hide_due`, `menu_bar_revealed`) beside the drag clears — one line + a unit test.
- **Mode-transition hygiene (Fable M2):** the `menu_bar_pin` toggle clears ALL THREE auto fields
  on every transition, and `recompute_menu_bar` is gated on `mode == Auto` (a pending due
  must not fire in Pinned/Hidden).
- **Reveal/hide does NOT re-scroll (Fable M3 — explicit chosen behavior):** hover must not
  move the document, so no `ensure_visible` runs on reveal. Accepted consequence: a caret
  sitting on the last text row may be unpainted while the bar is revealed (render's
  `row < edit_height` guard) until the next command; typewriter re-anchors on the next
  keystroke (a one-row jump). Documented, not a defect.
- Dropdown-close in auto: after Esc/click-away, `menu_bar_revealed` reflects the pointer —
  if it still rests at the top the bar stays revealed until leave (recomputed on the next
  Moved); otherwise the leave-grace (armed by the first below-row-0 Moved) hides it MENU_LEAVE_GRACE_MS later.

## Testing

- **e2e journeys** (the dwell flow is fully virtual-clock-drivable — `recompute_menu_bar`
  lives in the shared `advance()`):
  1. dwell-reveal: `Moved(row 0)` → `advance_ms(MENU_DWELL_MS + 1)` → `tick()` → bar labels
     render on row 0 AND the text shifted one row (`edit_top` moved — assert both); then
     `Moved(row 5)` + `advance_ms(MENU_LEAVE_GRACE_MS + 1)` + `tick()` → bar hidden, text back; AND the grace case — leave then re-enter row 0 WITHIN the grace → the bar stays (hide cancelled).
  2. drag-suppression: `Down(Left)` held + `Moved(row 0)` → never arms (no reveal after the
     dwell elapses).
  3. pinned: config/mode Pinned → bar visible at first render; F10 opens the dropdown; **Esc
     closes the dropdown, the bar persists** (the state-split pin).
  4. hidden: byte-identical-to-today regression pin — **non-vacuous form (Fable M4):** with
     `mode == Hidden`, inject `Moved(row 0)` + `advance_ms(MENU_DWELL_MS + 1)` + `tick()` and
     assert NO reveal (this pins the arm's mode gate — without the injection the journey
     passes under default Auto-unrevealed too and pins nothing); plus no bar until F10,
     closes fully on Esc.
  5. row-0 click while unrevealed (auto) edits TEXT (caret moves; no menu).
- **Unit tests:** the config fold (absent → Auto; each of the three strings; unknown →
  warning); the `menu_bar_rows()` truth table (mode × revealed × open); hydrate preserving
  `open`/`highlighted`; **the nav bug-fix pins** (caret visible at the bottom edge with the
  bar shown; `page_step` exact; `ensure_visible` full scroll — these pin the FIX, they fail
  on today's code with the menu open).
- **Suite + gates:** `cargo test -p wordcartel-core -p wordcartel` green; workspace clippy
  deny-gate clean; warning-free builds; **smoke run quoted verbatim** in the pre-merge report;
  the merge report states BOTH the feature and the nav-geometry bug fix.

## Decomposition (3 tasks)

1. **State + config + geometry** — `MenuBarMode`/`MenuConfig`/`RawMenu`/fold + tests;
   `Editor.menu_bar_mode` + `MouseState` fields + seeding; `menu_bar_rows()`; the full
   geometry sweep INCLUDING the nav.rs bug fix + its pinning tests. (Behavior identical for
   default config until Components 2-3 land — `Auto` with `revealed=false` ≡ today.)
2. **Pinned + bar rendering + commands** — inactive-bar render (static labels), the
   `menu_bar_layout` label refactor, click-to-open with `empty_at`/hydrate `open`-preservation,
   the Esc/F10 nuance verification, the `menu_bar_pin` command; pinned + hidden e2e journeys.
3. **Auto dwell** — the `Moved` arm, `MENU_DWELL_MS`, the deadline-array slot,
   `recompute_menu_bar` in `advance()`, degradation; the dwell/grace/drag/row-0-click journeys.

## Plan-confirms (resolve during the implementation plan, against real source)

1. Fresh anchors for every touched site (the map's lines are as of `fb10892` and will drift):
   render.rs:248/:906-945, mouse.rs:19/:76-78/:115-142/:216/:229/:258, nav.rs:90/:403/:761/:795,
   app.rs:818-827/:1153-1154/:1774-1798/:2152-2183/:2242-2244/:1864, registry.rs:210-227,
   editor.rs:322-336/:374/:444 + the nine `open_*` sites, keymap.rs:243, mouse.rs:154-156, menu.rs:4-45.
2. The nav.rs functions' signatures — confirm each of the four has `editor` (or can cheaply
   receive the row count) in scope for `menu_bar_rows()`.
3. The `menu_bar_layout` refactor shape for label-only rendering (groups vs a static label
   slice) such that mouse hit-testing and render share it in BOTH bar states.
4. `empty_at(open_idx)` + hydrate `open`/`highlighted` preservation — the minimal diff to
   `menu.rs`/`hydrate_overlays`.
5. RESOLVED (Codex fold-verify r3): `Editor.menu_bar_unpinned_mode` is REQUIRED — registry
   handlers see only `Ctx { editor, … }` (registry.rs:26), no `Config`, so a fieldless
   compute-from-config cannot recover the pre-pin mode. Seeded from `cfg.menu.bar` at
   startup (Auto fallback when config = pinned); toggle logic pinned in Component 2.
6. The deadline-array insertion (app.rs:2152-2183) + confirm `recompute_menu_bar`'s
   placement in `advance()` keeps the harness-drivable property (the e2e `step` calls
   `advance`).
7. RESOLVED (Codex rounds 1-2): the predicate is pinned in Component 3 verbatim (leave-
   bookkeeping runs even while the dropdown is open; arming requires dropdown-closed +
   no-drag + not-revealed). **PLACEMENT is also pinned (Codex round 2): the arm goes BEFORE
   the `if editor.menu.is_some()` overlay block** — that block returns UNCONDITIONALLY at
   mouse.rs:141 for every mouse event while the dropdown is open (not just Down(Left)), so
   an arm placed after it would never see Moved events mid-dropdown and the leave-bookkeeping
   would silently not run. Remaining plan detail: a unit test pinning the predicate table
   (including the dropdown-open leave case).
8. e2e `Moved` injection shape (`Msg::Input(Event::Mouse(MouseEvent { kind: Moved, … }))`) —
   confirm the harness needs no new sugar beyond a `mouse_move(col, row)` helper.
