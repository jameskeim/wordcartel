# Effort 5c-m — Mouse (text-first augmentation) (design)

**Status:** design / pre-plan
**Date:** 2026-06-25
**Depends on:** 5c (`nav::offset_at_cell`, `nav::screen_pos`, `textobj`, `select_word`/`select_paragraph` scopes, `sel_history` ladder, `ensure_visible`), 5b (palette/menu overlays + `dispatch_overlay_command`), 5a (config), 4b (`Tick`/`Clock`).

## 1. Goal & philosophy

Make the mouse genuinely useful **without** displacing the keyboard-first, text-first interface. Every mouse gesture has a keyboard equivalent; the mouse adds nothing the keyboard can't do, it just makes pointing faster. Native terminal copy stays reachable (capture is toggleable). No persistent chrome — the only new on-screen element is a scrollbar that appears on scroll and fades.

**In scope (5c-m):**
- Terminal mouse-capture lifecycle, toggleable (config + command), with a documented native-selection escape hatch.
- Text-area gestures: click→caret, drag→select (with edge auto-scroll), Shift+click→extend, double-click→word, triple-click→paragraph, wheel→scroll-the-view.
- An auto-hiding, draggable scrollbar.
- Mouse on the 5b overlays: click a palette/menu row to run it; click outside to dismiss.

**Out of scope (deferred):**
- **Middle-click paste / primary-selection clipboard** → its own later effort (separate primary-selection plumbing, Wayland/platform fiddliness).
- Mouse in a future gutter/outline (5g), drag-and-drop text move, column/box selection.

## 2. Crate & widget posture

No new dependencies — and 5c-m **removes** one (`tui-menu`, see §8).
- **`crossterm` (0.28, present):** `event::{EnableMouseCapture, DisableMouseCapture, MouseEvent, MouseEventKind, MouseButton}`. Once capture is enabled, the existing input thread already forwards `Event::Mouse` as `Msg::Input` — today it lands in `reduce`'s `Msg::Input(_) => {}` no-op (app.rs:764); 5c-m adds the handling arm.
- **`ratatui` (0.29, present):** ratatui is a pure immediate-mode renderer with **no interactive/clickable widgets and no event system** (verified: its 0.29 widget set is block/paragraph/list/table/tabs/gauge/chart/barchart/sparkline/scrollbar/calendar/canvas/clear/logo — none handle input). "Clickable" therefore means *we own the `Rect` and hit-test mouse coords ourselves* (exactly how the palette already works). 5c-m uses `widgets::{Scrollbar, ScrollbarState, ScrollbarOrientation}` for the scrollbar and `widgets::{List, Block, Clear, Paragraph}` for the self-rendered menu (§8).
- **`tui-menu` (=0.3.0) — REMOVED:** it has no mouse/hit API and hides its dropdown geometry, so 5c-m replaces it with a self-rendered menu and drops the dependency + its version pin (§8).

## 3. Architecture & modules

| Unit | Responsibility | Depends on |
|------|----------------|-----------|
| **`wordcartel/src/mouse.rs`** (new) | All mouse-event handling: coord translation, click/drag/wheel, multi-click, scrollbar drag, palette hit-testing + outside-dismiss. | `nav::offset_at_cell`, `commands::scope_range_at`/`Scope`, `dispatch_overlay_command`, render geometry helpers |
| `wordcartel/src/term.rs` (extend) | `TerminalGuard::new(enable_mouse)` runs `EnableMouseCapture` in setup when enabled; `DisableMouseCapture` at all 3 teardown sites. | crossterm |
| `wordcartel/src/commands.rs` (extend) | `pub fn scope_range_at(editor, offset, Scope)` (existing private `scope_range` delegates). | 5c textobj/nav |
| `wordcartel/src/editor.rs` (extend) | `Editor.mouse_capture: bool`; `Editor.mouse: MouseState`. | — |
| `wordcartel/src/app.rs` (extend) | `Msg::Input(Event::Mouse(ev))` arm → `mouse::handle`; recompute `scrollbar_visible`; main-loop `reconcile_mouse_capture` (post-guard + per-iteration) + feed `scrollbar_until_ms` into the loop's `recv_timeout` deadline. | — |
| `wordcartel/src/menu.rs` (rewrite) | Self-rendered shallow menu: `MenuView { groups, open, highlighted, built }`; drop `tui-menu`/`MenuState`. | render geometry helpers |
| `wordcartel/src/render.rs` (extend) | Auto-hiding scrollbar; self-render the menu bar+dropdown; **shared geometry helpers** (`palette_overlay_rect`, `palette_row_at`, `menu_bar_layout`, `menu_dropdown_rect`, `menu_dropdown_row_at`) so `render` and `mouse` agree (DRY — no drift). | ratatui Scrollbar/List/Clear |
| `wordcartel/Cargo.toml` (edit) | Remove `tui-menu = "=0.3.0"`. | — |
| `wordcartel/src/config.rs` (extend) | `mouse.capture: bool` (default `true`). | 5a config |
| `wordcartel/src/registry.rs` / `keymap.rs` (extend) | `toggle_mouse_capture` command (palette-only, no default chord). | 5b registry |

## 4. Capture lifecycle & the escape hatch

- **Config:** `mouse.capture` (RawConfig `Option<bool>`, default `true`, merged per the 5a layered-config pattern). The config-resolved value seeds `editor.mouse_capture` at startup.
- **Setup honors the config (Codex Important — `TerminalGuard::new()` is unconditional today):** `TerminalGuard::new(enable_mouse: bool)` takes the initial capture state; it runs `EnableMouseCapture` in the setup `execute!` (after `EnableBracketedPaste`, term.rs:40) **only when `enable_mouse`**. So `mouse.capture = false` is honored from the first frame — capture is never silently on while idle. **Teardown:** add `DisableMouseCapture` to all three cleanup `execute!` calls (term.rs:46 panic hook, 62, 82) so a panic/exit always releases the mouse (harmless if capture was never enabled).
- **Toggle:** `toggle_mouse_capture` command flips `editor.mouse_capture` (sets the flag only — it can't do terminal IO; the guard owns stdout). The **main loop reconciles** the flag with the terminal — mirroring `drain_clipboard_intents` (app.rs:1039): `reconcile_mouse_capture(&mut editor, terminal.backend_mut(), &mut applied)` compares `editor.mouse_capture` to the last-applied `bool` and runs `execute!(backend, Enable/DisableMouseCapture)` on change (valid: `backend_mut()` is a `CrosstermBackend: Write`). It is called once **immediately after the guard is constructed (before the first draw)** AND each loop iteration, so both the config-seeded initial state and runtime toggles are applied promptly. **When capture transitions OFF, `reconcile_mouse_capture` clears the drag state** (`mouse.dragging`/`scrollbar_dragging`/`anchor` → reset) so no `Up` event is awaited that will never arrive (Codex Minor).
- **Escape hatch:** with capture **off**, the terminal's native click-drag-select-and-copy works normally. The spec/README documents that most emulators (kitty, foot, VTE, iTerm2, Windows Terminal) also let you hold **Shift** to bypass app capture for a one-off native drag without toggling.

## 5. Coordinate translation

Mouse `(column, row)` are full-screen 0-based cells. The editing area (mirroring render.rs:61-64): `menu_rows = u16::from(menu.is_some())`, `edit_top = menu_rows`, `edit_height = h - (1 + menu_rows)`, status row = `h - 1`. A helper

```
fn editing_cell(editor, col: u16, row: u16) -> CellHit
```

classifies a point: `MenuBar` (row < menu_rows, menu open), `Status` (row == h-1), `Scrollbar` (scrollbar visible and col == w-1), `Text { col, erow }` where `erow = row - menu_rows` and `erow < edit_height`, else `Outside`. For a `Text` hit, the document offset is `nav::offset_at_cell(editor, col, erow)` (5c — already editing-area-relative; returns `None`/clamps past content). **The scrollbar overlays the last text column only while visible (transient); text layout is NOT reflowed on scroll** (avoids reflow jank — a click in that column during the ~1.2 s window routes to the scrollbar, otherwise it's text).

## 6. Core text-area gestures

Driven by `MouseEventKind` on `MouseButton::Left` (handler ignores Right; Middle is reserved for the deferred paste effort). All selection changes clear `sel_history` (consistent with 5c) except the multi-click ladder, which *seeds* it.

- **Offset selection helper (Codex Important — `commands::scope_range` is private and caret-anchored):** add `pub fn commands::scope_range_at(editor: &Editor, offset: usize, scope: Scope) -> (usize, usize)` that computes a scope's range at an ARBITRARY `offset` (not `nav::head`); refactor the existing private `scope_range` to delegate (`scope_range(editor, scope) = scope_range_at(editor, nav::head(editor), scope)`). `mouse.rs` uses `scope_range_at` so double/triple-click select at the *clicked* offset, not the caret.
- **Down(Left)** on Text:
  - **Multi-click:** `MouseState.last_click = Option<{cell: (u16,u16), at_ms, count}>`. If `clock.now_ms() - at_ms <= 400` AND the new click is on the **same screen cell `(col,row)`** (compare cells, not offsets — resolves the §-intro wording), increment `count`; else `count = 1`. Then, with `offset = offset_at_cell(...)`:
    - `count == 1` → place caret: `Selection::single(offset)`, clear `sel_history`.
    - `count == 2` → `Selection::range` from `scope_range_at(editor, offset, Scope::Word)`, seed `sel_history`.
    - `count == 3` → `scope_range_at(editor, offset, Scope::Paragraph)`, seed `sel_history`.
    - `count >= 4` → wrap to `1` (place caret).
  - Set `MouseState.anchor = Some(offset)`, `dragging = true`. Update `last_click` (cell + now_ms + count).
  - **Shift held** (`ev.modifiers` contains SHIFT) → *extend*: keep the current selection anchor, set head = clicked offset (`Selection::range(anchor, offset)`); does not start a fresh multi-click sequence.
- **Drag(Left)** on Text (or past the edges): head = `offset_at_cell(clamped to area)`, `Selection::range(MouseState.anchor, head)`. **Edge auto-scroll:** if `row < edit_top` scroll up one line; if `row >= edit_top + edit_height` scroll down one line (then recompute head at the clamped edge row). `ensure_visible` is NOT called mid-drag (the drag itself drives scroll).
- **Up(Left):** `dragging = false`; selection persists.
- **ScrollUp / ScrollDown:** scroll the **view** by 3 logical rows (adjust `view.scroll`/`scroll_row` via the existing scroll helpers), **caret unchanged**. Set `MouseState.scrollbar_until_ms = now + 1200`. The next caret-moving command re-centers via `ensure_visible`.
- A mouse selection populates the normal `Selection`/`primary` range, so `Ctrl+C` (copy) and the register work unchanged — mouse-select → keyboard-copy is seamless.

## 7. Auto-hiding scrollbar

- **Visibility:** `MouseState.scrollbar_until_ms` is set on every scroll/scrollbar-drag. A plain bool `MouseState.scrollbar_visible` is what `render` reads (render has no clock). The bool is recomputed (`scrollbar_visible = clock.now_ms() < scrollbar_until_ms`) whenever a message is processed (it is set true at scroll time, and re-evaluated on the next wake).
- **Fade timing (Codex Important — the idle loop timeout is up to 1 hour, so `Tick` alone won't fire to fade it):** `scrollbar_until_ms` must feed the main loop's `recv_timeout` deadline calculation, alongside the existing swap/save deadlines (app.rs loop). So while the scrollbar is up, the loop wakes at the fade time, flips `scrollbar_visible = false`, and redraws — the scrollbar reliably disappears ~1.2 s after the last scroll even with no other activity. (If the loop's deadline plumbing makes adding a source costly, the fallback is to keep the scrollbar visible until the next input event — acceptable but less polished; the deadline approach is preferred.)
- **Render:** when visible, a `ratatui::Scrollbar` (`ScrollbarOrientation::VerticalRight`) on the rightmost editing column, with `ScrollbarState` built from `view.scroll` (position) and `derive::total_logical_lines` (content length) — computed locally in `render`, no stored widget state.
- **Drag/scrub:** a `Down`/`Drag(Left)` on the scrollbar column maps the row within the track to a scroll position proportionally (`scroll = (erow / edit_height) * max_scroll`), clamped. Sets the visibility window so it stays up during the drag. (Approximate against wrapped lines — acceptable for v1; the keyboard remains exact.)

## 8. Mouse on overlays

**Prerequisite (Codex Important):** `dispatch_overlay_command` is currently a private `fn` in `app.rs:421`. `mouse.rs` needs it for both palette-row and menu-item clicks, so it must be made **`pub(crate)`** (or moved to a shared module). Both palette and menu mouse-dispatch route through it (it closes the overlay, dispatches, drains, hydrates).

When `editor.palette` or `editor.menu` is open, `mouse::handle` routes clicks against the overlay BEFORE the text area. **Render and mouse read the stored `MenuView.groups`** (and the palette's stored rows) for hit-testing — they do NOT call the private `menu::grouped_commands`/`leaf_label` (which stay private; only `menu::build` calls them).

**Palette (fully clickable):**
- **Geometry (DRY):** extract the palette overlay rect + list-row layout (currently inline at render.rs:193-223) into shared helpers in `render.rs` — `palette_overlay_rect(area) -> Rect` and `palette_row_at(area, palette, col, row) -> Option<usize>`. `render` and `mouse` both call them so a layout change can never desync hit-testing from the paint.
- A `Down(Left)` inside the list area → the row index → dispatch that row's command via `dispatch_overlay_command` (5b — closes the overlay, dispatches, drains, hydrates). A click **outside** the overlay rect → close the palette.

**Menu — self-rendered, fully clickable (replaces `tui-menu`):** `tui-menu 0.3` exposes no mouse/hit API and hides its dropdown geometry, so 5c-m **drops `tui-menu` and renders the menu with ratatui primitives** (the palette pattern — ratatui has no interactive widgets; "clickable" = we own the `Rect`s and hit-test them). The menu is *shallow* (categories → flat command lists, no nested submenus) and `menu::grouped_commands` (menu.rs:48) already builds the model, so this is bounded.

- **`MenuView` (rewritten, menu.rs):** `MenuView { groups: Vec<(MenuCategory, Vec<(String, CommandId)>)>, open: usize, highlighted: usize, built: bool }` — `groups` from the existing `grouped_commands`; `open` = index of the open category's dropdown; `highlighted` = index within that dropdown. `build()` stores `groups`, `open = 0`, `highlighted = 0`; `empty()` has empty groups. Drop `menu_items_from_groups`, `MenuState`, and the `tui_menu` import. (`MenuView` becomes `Clone`; update the editor.rs:117 "not Clone" note — `Editor` may stay non-Clone, no need to re-derive.) **Bounds safety (Codex):** `groups` is empty in `empty()` (before hydrate), so render and the keyboard/mouse nav MUST guard `groups.is_empty()` and clamp `open`/`highlighted` to valid indices — never index `groups[open]`/`[highlighted]` unchecked. The `built` flag still gates hydrate-on-open (an `empty()` menu is replaced by `build()` on first reduce, as in 5b).
- **Render (render.rs, replaces the `tui_menu::Menu` paint at render.rs:245-256):** paint the **bar row** (top row of `menu_area`) — category labels left-to-right with separators, the `open` category highlighted; and, below it, the **dropdown** for the `open` category (a `Clear` + `List` of its leaf labels, `highlighted` row reversed) at the open category's x-position. Layout via shared helpers so mouse agrees: `menu_bar_layout(area, &groups) -> Vec<(usize /*cat*/, Rect /*label*/)>` and `menu_dropdown_rect(area, &groups, open) -> Rect` + `menu_dropdown_row_at(area, &groups, open, col, row) -> Option<usize>`.
- **Keyboard (app.rs, replaces the `MenuState` nav at app.rs:487-533 — behavior preserved):** `Esc`/`F10` → close; `Left`/`Right` → move `open` to prev/next category (wrap), reset `highlighted = 0`; `Up`/`Down` → move `highlighted` within `groups[open].1` (clamp); `Enter` → dispatch `groups[open].1[highlighted].1` via `dispatch_overlay_command`. The F10-opens / F10-toggles-closed behavior and the menu→palette cross-link are unchanged.
- **Mouse (5c-m):** `Down(Left)` on a bar label (`menu_bar_layout` hit) → set `open` to that category, `highlighted = 0`; on a dropdown row (`menu_dropdown_row_at`) → dispatch that command via `dispatch_overlay_command`; **outside** both the bar and the open dropdown → close the menu.

**Both overlays are now fully click + keyboard.** A captured click is never dead. Dropping `tui-menu` also removes the `=0.3.0` dependency pin (Cargo.toml).

## 9. State, error handling, edge cases

- **`Editor` additions:** `pub mouse_capture: bool` (config-seeded) and `pub mouse: MouseState`, where `MouseState { anchor: Option<usize>, last_click: Option<ClickRecord>, dragging: bool, scrollbar_dragging: bool, scrollbar_until_ms: u64, scrollbar_visible: bool }`, `ClickRecord { offset: usize, at_ms: u64, count: u8 }`. The main loop tracks an `applied_mouse_capture: bool` for reconciliation.
- **`pending_mark` guard (Codex Important):** the new `Msg::Input(Event::Mouse(_))` arm sits at the bottom of `reduce` (below the key/paste interceptors), so a mouse event during a mark-capture would otherwise be *handled* (the 5c `pending_mark` block only intercepts `Event::Key`). Therefore **`mouse::handle` early-returns when `editor.pending_mark.is_some()`** — a click during mark-capture is ignored; the capture still resolves on the next key.
- **`offset_at_cell` None policy (Codex Minor):** `offset_at_cell` returns `None` when the click row is past rendered content (it does not clamp). On a `Text` hit that yields `None` (e.g. clicking below the last line), `mouse::handle` places the caret at **document end** (`clamp_snap(editor, buf.len())`) — the intuitive "click in the empty area below the text → go to end." A click that is `Outside`/`Status`/`MenuBar` (not a Text hit) is ignored (or routed per §8/§5).
- Click past end-of-line (within a real content row) → `offset_at_cell` snaps to the line end (5c behavior).
- Drag that produces an empty range (anchor == head) → a collapsed caret (fine).
- Rapid clicks beyond triple wrap to single (count modulo).
- Resize between events → coordinates recomputed per event from current `view.area`; no stale geometry.
- Capture off → the terminal stops sending mouse events; defensively, `mouse::handle` also early-returns when `!editor.mouse_capture`.
- Toggling capture off mid-drag → `reconcile_mouse_capture` resets `MouseState` drag fields (§4), so no stuck `dragging`.

## 10. Testing strategy

- **`mouse::handle` unit suite** driving synthesized `MouseEvent`s with a `TestClock`:
  - Down→caret at the `offset_at_cell` of a cell (reuse 5c's `screen_pos`/`offset_at_cell` round-trip fixtures to pick cells with known offsets).
  - Drag→`Selection::range(anchor, head)`; edge auto-scroll advances `view.scroll`.
  - Shift+Down→extend keeps the anchor.
  - Double-click (two Downs within 400 ms, same cell)→word selection; triple→paragraph; both seed `sel_history` (a following `expand_selection` grows).
  - Wheel→`view.scroll` changes, `selection` (caret) unchanged.
  - Scrollbar drag→`view.scroll` proportional; `scrollbar_until_ms` set.
  - Palette open + click on a row→that command dispatched + palette closed; click outside→palette closed.
  - Menu open + click a bar label→that category's dropdown opens; click a dropdown item→that command dispatched + menu closed; click outside→menu closed.
- **Menu keyboard parity (self-render must not regress 5b — these tests ADAPT, not inherit unchanged, per Codex):** `f10_opens_menu` / `f10_toggles_menu_closed_when_open` map cleanly (they assert `editor.menu.is_some()`). The `grouped_commands` grouping test is unchanged (model untouched). The `menu_select_for_test` shim (app.rs:~1250) references the old `MenuState` path and must be **rewritten** to drive the self-rendered nav (set `open`/`highlighted` then dispatch). Add NEW parity tests: `Left/Right` moves `open` across categories; `Up/Down` moves `highlighted`; `Enter` dispatches `groups[open][highlighted]`; and the menu→palette cross-link still dispatches `palette` + hydrates.
  - Mouse event while `pending_mark` is Some→ignored (no caret move, capture still pending).
  - Click below all content (offset_at_cell None)→caret at document end.
  - `toggle_mouse_capture`→flips `editor.mouse_capture`; toggling off mid-drag clears `MouseState` drag fields.
- **`term.rs`:** a guard test (or doc-asserted) that teardown includes `DisableMouseCapture`, and that `TerminalGuard::new(false)` does not enable capture.
- **Scrollbar visibility:** `scrollbar_visible` is false once `now_ms()` passes `scrollbar_until_ms`.
- No pre-existing test weakened; full workspace green, zero warnings.

## 11. Module & command summary

- **New file:** `wordcartel/src/mouse.rs`.
- **New command:** `toggle_mouse_capture` (palette-only).
- **New config:** `mouse.capture` (bool, default true).
- **New `Editor` state:** `mouse_capture`, `mouse: MouseState`.
- **New public helper:** `commands::scope_range_at(editor, offset, Scope)` (private `scope_range` delegates).
- **Signature change:** `TerminalGuard::new(enable_mouse: bool)` (update its call site).
- **Menu rewrite (drop `tui-menu`):** `MenuView { groups, open, highlighted, built }` self-rendered with ratatui primitives; rewire the app.rs menu keyboard block + the render paint; remove the `tui-menu` dependency from `Cargo.toml`. The 4 removal sites (Cargo.toml:24, menu.rs:3-7, render.rs:245-256, app.rs:521-524) are a mechanical checklist.
- **Visibility change:** `dispatch_overlay_command` (app.rs:421) → `pub(crate)` so `mouse.rs` can dispatch palette/menu clicks.
- **Render extraction (DRY for hit-testing):** `palette_overlay_rect` / `palette_row_at` / `menu_bar_layout` / `menu_dropdown_rect` / `menu_dropdown_row_at` shared between `render` and `mouse`.
- **Main-loop additions:** `reconcile_mouse_capture` (post-guard + per-iteration, clears drag state on off); `scrollbar_until_ms` fed into the `recv_timeout` deadline.
- **Reuses from 5c/5b:** `nav::offset_at_cell`, `commands::scope_range_at`/`Scope`, `sel_history` seeding, `dispatch_overlay_command`, `ensure_visible`, the scroll fields, `menu::grouped_commands`.

## 12. Deliberate decisions (for review)

1. **Capture ON by default + `toggle_mouse_capture` + Shift-bypass** — useful out of the box, native copy always reachable (§4).
2. **Wheel scrolls the view, caret unchanged** — reading vs editing (§6).
3. **Double=word, triple=paragraph**, seeding the 5c expand ladder; no quad-click (§6).
4. **`toggle_mouse_capture` has no default keybinding** — rare power toggle; saves key space (§3).
5. **Auto-hiding scrollbar, not always-on; overlays the last column while visible (no text reflow on scroll); fade driven by a loop `recv_timeout` deadline, not idle `Tick`** (§5/§7).
6. **Both overlays fully click + keyboard.** The palette uses extracted shared render geometry. The **menu is self-rendered with ratatui primitives, dropping `tui-menu`** (which has no mouse/geometry API) — bar labels and dropdown items become clickable, keyboard nav is preserved, and the `=0.3.0` dependency pin is removed (§8). Chosen over keeping `tui-menu` (menu would be click-outside-dismiss only) because the menu is shallow and `grouped_commands` already builds the model, so the self-render is bounded.
7. **Capture honored from frame 1** via `TerminalGuard::new(enable_mouse)` + a post-guard reconcile; toggling off clears drag state (§4).
8. **Mouse ignored during `pending_mark` capture; click below content → caret to document end** (§9).
9. **Middle-click paste deferred** to its own effort (§1).
