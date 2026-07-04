# A6: overlay list reachability — windowed scrolling for palette + siblings — design

**Status:** Codex rounds 1-2 folded (r2 caught a silently-failed fold + the render area-source wording + the :1388 anchor); re-verify pending
**Date:** 2026-07-04
**Effort:** a6-palette-reachability — slot 1 of the recorded working order (`docs/ux-backlog.md`).
User decisions: scope = ALL FOUR diseased overlays (fork 1 = B); wheel moves the SELECTION
(fork 2 = A); the `12/110` position indicator approved.

## Context

The command palette is exhaustive in data (~110 commands) but not in REACH: the `Palette`
struct has no scroll state (palette.rs:19-28), render slices only the first
`list_h = min(rows, 15, h-4)` rows into the ratatui `List` (render.rs:760-777, height logic
:150-159), while Up/Down move `selected` over the FULL row set (app.rs:1240-1249). Past the
window the highlight VANISHES (ratatui gets a window-relative-impossible index) and **Enter
still dispatches the invisible selection** — a silent-wrong-action hazard shared by the
palette (app.rs:1225-1226 reads the absolute index) AND the file browser (app.rs:1388,
`fb.entries.get(fb.selected)` — Codex round 1: the identical hazard, named explicitly; the
outline's is partially pre-empted by its opened_version guard but live for fresh documents). PgUp/PgDn/Home/End are consumed by the wildcard arm (:1287) and
do nothing. The palette mouse block returns early for ALL events (mouse.rs:122-145) — wheel
never reaches the scroll arms. Clicks on visible rows work (`palette_row_at`, render.rs:163-174,
test-pinned); query-row/border clicks are silent no-ops (deliberate, kept).

**The disease is a family trait (2026-07-04 map):**

| Overlay | scroll field | slices `take(list_h)` | selection escapes | wheel | notes |
|---|---|---|---|---|---|
| Palette | none | yes (:760) | YES (~110 cmds; ALSO PaletteKind::Buffers — the buffer switcher, editor.rs:687, exceeds 15 on large sessions and is cured by the same fix) | swallowed | click works on visible rows |
| Outline | none | yes (:805) | YES (16+ headings) | falls through to the DOC | no mouse block at all |
| ThemePicker | none | yes (:851) | LATENT (13 themes; breaks at 16 — E4 adds themes) | swallowed | live PREVIEW follows the invisible selection (app.rs:1086-1091) |
| FileBrowser | none | yes (:897) | YES (16+ dir entries) | swallowed | clicks swallowed entirely |
| Diag | none | yes (:990) | NO — `down()` clamps (diag_overlay.rs) | falls through | IMMUNE; do not touch |

**A3 fold-in — already satisfied, verified:** palette rows ALREADY show right-aligned
keybinding chords (`PaletteRow.chord`, populated via `keymap.chord_for` in `rebuild_rows`,
palette.rs:66-79; painted render.rs:760-768) — pinned by
`rebuild_rows_empty_query_lists_all_in_order_with_chords` (palette.rs:133), which ALSO already
pins the completeness invariant (`rows.len() == reg.commands().count()` on empty query). No
A3 work lands here; A3's residue (the menu curation pass) stays with E1/E2 per the working
order.

## Goals

- Every row of every windowed overlay is reachable by keyboard (arrows/PgUp/PgDn/Home/End)
  and wheel, with the window following the selection.
- **The invariant that names the effort: the selection is ALWAYS visible.** Kills the
  invisible-dispatch hazard (palette Enter) and the invisible-preview hazard (theme picker)
  by construction.
- One shared, house-establishing idiom (no per-overlay reinvention).
- A `12/110`-style position indicator in the overlay border (answers "is this everything?").

## Non-goals

- Diag overlay (immune; correctly clamped).
- Click-to-select for ThemePicker/FileBrowser and ANY outline mouse block (outline's wheel
  continues to scroll the document beneath — a known oddity) — recorded as a backlog
  follow-up at ship time ("overlay mouse parity").
- Palette dead zones stay deliberate no-ops (query row, borders); click-away-outside still
  closes. No fuzzy-ranking changes. No new key semantics beyond the four navigation keys +
  wheel.

## Component 1 — the shared windowing module

A small new shell module (working name `list_window.rs`; ~30 lines + tests — plan-confirm the
name/home) with two pure helpers:

```rust
/// Visible row budget for a windowed overlay list — THE single source of the
/// min(rows, 15, h-4) computation (today duplicated by render.rs:150-159's
/// palette_overlay_rect and palette_row_at render.rs:163-174).
pub(crate) fn list_h_for(row_count: usize, area_h: u16) -> usize

/// Slide the window so `selected` is visible: on exit,
/// selected ∈ [scroll_top, scroll_top + list_h) (for list_h > 0), and
/// scroll_top <= row_count.saturating_sub(list_h) (no over-scroll after shrink).
pub(crate) fn keep_visible(selected: usize, row_count: usize, list_h: usize, scroll_top: &mut usize)
```

`keep_visible` semantics: `selected < scroll_top` → `scroll_top = selected`;
`selected >= scroll_top + list_h` → `scroll_top = selected + 1 - list_h`; then clamp
`scroll_top` to `row_count.saturating_sub(list_h)` (covers filter-shrink). `list_h == 0`
(degenerate terminal) → no-op, scroll_top = 0.

The four structs gain `pub scroll_top: usize` (default 0): `Palette` (palette.rs:19-28),
`OutlineOverlay` (outline_overlay.rs:14-22), `ThemePicker` (theme_picker.rs:7-13),
`FileBrowser` (file_browser.rs:14-19).

## Component 2 — keys (all four overlays)

In each overlay's key-intercept block (palette app.rs:1218-1292; theme picker :1325-1334;
file browser :1421-1428; outline :1648-1657):

- **Up/Down:** existing selection moves, THEN `keep_visible(...)`.
- **NEW PgUp/PgDn:** `selected` moves by one window (`saturating_sub(list_h)` /
  `+list_h` clamped to `rows-1`), then `keep_visible`. (Today these keys are silently
  consumed by each block's wildcard arm — adding arms changes no other behavior.)
- **NEW Home/End:** `selected = 0` / `rows.len()-1`, then `keep_visible`.
- **After every filter/query rebuild** (Char/Backspace/Paste paths that call
  `rebuild_rows`/equivalents): the existing selected-clamp is followed by `keep_visible`
  (which also re-clamps `scroll_top` after the row set shrinks).
- **ThemePicker ordering pin:** `keep_visible` runs BEFORE `preview_selected_theme`
  (app.rs:1086-1091) on every selection-changing path — the previewed theme is always the
  visibly-highlighted one.
- `list_h` inside key handlers derives from `list_h_for(rows.len(), area_h)` where
  `area_h` is read from **`editor.active().view.area`** — the same source MOUSE uses
  (mouse.rs:120). Render reads `frame.area()` (render.rs:241), a different SOURCE carrying
  the same dimensions (`view.area` is updated on every Resize event) — Codex round 2
  wording fix. **The overlay key blocks read no area today (Codex round 1) — this read is a
  deliberate addition**, giving key handlers the same numbers render will use that frame.
- Enter semantics UNCHANGED (absolute `rows.get(selected)`) — safe by the always-visible
  invariant. Outline's `opened_version` guard (app.rs:1660-1667) untouched.

## Component 3 — render (all four) + the position indicator

- Items become `rows[scroll_top .. min(scroll_top + list_h, rows.len())]`;
  `list_state.select(Some(selected - scroll_top))` (window-relative). Sites: palette
  render.rs:760-777, outline :805-818, theme picker :851-864, file browser :897-910.
  A `debug_assert!(selected >= scroll_top && selected < scroll_top + list_h.max(1) || rows.is_empty())`
  documents the invariant at the render boundary (unreachable when the Component-2 discipline
  holds).
- **The position indicator (user-approved):** the overlay's BOTTOM border row shows a
  right-aligned `{selected+1}/{total}` (e.g. `12/110`) whenever `rows.len() > list_h`;
  hidden when everything fits (no noise on short lists). Styled with the border's existing
  `SE::Chrome`. Exact mechanism (ratatui `Block` bottom title vs painting after the block)
  is a plan-confirm; the CONTENT and the only-when-scrollable rule are the contract. Applies
  to all four overlays (shared helper).
- `palette_overlay_rect` (render.rs:150-159) is UNCHANGED (the overlay's size doesn't depend
  on scroll position); its `list_h` computation is replaced by a call to `list_h_for`
  (deduplication, not behavior change).

## Component 4 — mouse

- **`palette_row_at`** (render.rs:163-174): returns `(row - list_top) as usize + palette.scroll_top`
  (it already receives `&Palette`). The existing mouse click test (mouse.rs:446-465) adjusts
  for the windowed math — NOT weakened: it pins the same dispatch with an explicit
  `scroll_top == 0` precondition, and a NEW scrolled-click test pins that after PgDn the
  clicked cell maps to the correct ABSOLUTE row.
- **Wheel = selection ± 1 via `keep_visible`** (fork 2 = A), added INSIDE the three overlay
  mouse blocks that exist: palette (mouse.rs:122-145 — arms for `ScrollDown`/`ScrollUp`
  before the block's return), theme picker (:174-176 — the bare return gains wheel arms +
  the preview call with the same ordering pin), file browser (:177-179 — same, no preview).
  Outline: keyboard-only this effort (no mouse block exists; creating one is the deferred
  mouse-parity follow-up).
- Palette dead zones unchanged (query row + borders no-op; outside closes).

## Testing

- **Unit — the helpers:** `keep_visible` table (selection above window / below window /
  inside → no-op / jump to End / window re-clamp after shrink / list_h 0 / short list) and
  `list_h_for` (the three-way min).
- **Unit — per overlay:** (a) Down past the window keeps the highlight visible
  (`selected - scroll_top < list_h` after every step); (b) **the hazard pin:** walk
  `selected` to an off-first-window index (e.g. 50 in the palette) and assert BOTH that
  Enter dispatches `rows[50]`'s command AND that row 50 was within the visible window at
  dispatch time — this test FAILS today (the highlight vanishes; the window shows 0-14);
  (c) PgDn/PgUp/Home/End land where specified; (d) a filter change re-clamps selection AND
  window; (e) ThemePicker: after Down past the window, the PREVIEWED theme equals the
  visibly-highlighted row (fails today once >15 themes — pin with an artificially extended
  row list, not by adding themes); (f) FileBrowser/Outline equivalents of (a)-(c),
  INCLUDING the file browser's own Enter-dispatches-visible hazard pin (its absolute read at
  app.rs:1388); (g) a `PaletteKind::Buffers` scroll case — the buffer switcher
  (editor.rs:687, kind set :708) rides the same palette windowing; make the coverage
  explicit, not accidental (Codex round 1).
- **Unit — mouse:** the adjusted visible-click test + the new scrolled-click test; wheel
  moves selection and the window follows (palette, theme picker, file browser).
- **Render:** a scrolled palette shows `rows[scroll_top..]` content (assert a row string at
  scroll_top > 0); the indicator shows `n/total` right-aligned in the bottom border when
  scrollable and is ABSENT when the list fits.
- **e2e journey:** open the palette, navigate to the LAST command with End (and PgDn
  stepping), assert the highlight is visible at each checkpoint and Enter dispatches the
  highlighted command — the "reach everything without typing" journey, impossible today.
- **Existing tests kept honest:** palette.rs:133 (completeness + chords) gains only the
  `scroll_top: 0` default; render.rs:1294 (overlay rect) unaffected; app.rs palette tests
  (:2468, :3756, :3809, :3980, :4001) unaffected (small indices / no scrolling).
- Suite green (`cargo test -p wordcartel-core -p wordcartel`); workspace clippy deny-gate
  clean; warning-free builds; **smoke run quoted verbatim** in the pre-merge report.

## Decomposition (2 tasks)

1. **The shared module + the full palette fix** — `list_window.rs` (helpers + unit tables);
   `Palette.scroll_top`; the palette key arms (PgUp/PgDn/Home/End + keep_visible wiring);
   windowed render + the indicator; `palette_row_at` + wheel + the mouse-test adjustments;
   the palette hazard pin + the e2e journey.
2. **The three siblings** — `scroll_top` on OutlineOverlay/ThemePicker/FileBrowser; their key
   arms + rebuild re-clamps; windowed renders + indicators; wheel for theme picker + file
   browser; the ThemePicker scroll-before-preview ordering + its pin; the sibling unit tests.

## Ship-time bookkeeping

- Add the "overlay mouse parity" follow-up to `docs/ux-backlog.md` (theme-picker/file-browser
  click-to-select; an outline mouse block so wheel/clicks stop falling through to the
  document).
- Mark A6 SHIPPED and A3 reduced to the curation pass (its hints + invariant halves verified
  already-done by this effort's map).

## Plan-confirms (resolve during the implementation plan, against real source)

1. Fresh anchors for every touched site (the map's lines are as of `e939c32` and may drift):
   palette.rs:19-28/:66-86/:133, render.rs:150-174/:760-777/:805-818/:851-864/:897-910/:1294,
   app.rs:1086-1091/:1218-1292/:1325-1334/:1421-1428/:1648-1667, mouse.rs:122-145/:174-179/
   :292-299/:446-468, outline_overlay.rs:14-22, theme_picker.rs:7-13, file_browser.rs:14-19.
2. The shared module's name/home (`list_window.rs` proposed) + wiring `list_h_for` into
   `palette_overlay_rect`/`palette_row_at` without changing the rect math.
3. The indicator mechanism in ratatui 0.30 (Block bottom-title vs post-paint into the border
   row) — content/right-alignment/only-when-scrollable are the contract; also confirm each
   sibling overlay's border rendering supports the same mechanism.
4. The exact wheel-arm placement inside each of the three mouse blocks (before each block's
   return; the palette block's structure at mouse.rs:122-145).
5. Each overlay's key-arm wildcard behavior (the plan must add arms WITHOUT disturbing each
   block's paste/char handling; the palette's three-tier intercept at app.rs:1200-1292).
6. The area-height source for `list_h_for` inside key handlers (the same `view.area` the
   renderer uses — confirm no resize-race between a key event and the next frame matters,
   since keep_visible re-runs on every selection change and render re-derives list_h).
7. The e2e journey's harness needs (key() covers PgDn/End? — KeyCode::PageDown/End exist;
   confirm the palette opens via the ctrl-p binding in the harness keymap).