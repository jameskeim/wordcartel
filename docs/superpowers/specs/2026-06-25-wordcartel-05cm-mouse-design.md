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

No new dependencies.
- **`crossterm` (0.28, present):** `event::{EnableMouseCapture, DisableMouseCapture, MouseEvent, MouseEventKind, MouseButton}`. Once capture is enabled, the existing input thread already forwards `Event::Mouse` as `Msg::Input` — today it lands in `reduce`'s `Msg::Input(_) => {}` no-op (app.rs:764); 5c-m adds the handling arm.
- **`ratatui` (0.29, present):** `widgets::{Scrollbar, ScrollbarState, ScrollbarOrientation}` for rendering the scrollbar. Drag hit-testing is ours.

## 3. Architecture & modules

| Unit | Responsibility | Depends on |
|------|----------------|-----------|
| **`wordcartel/src/mouse.rs`** (new) | All mouse-event handling: coord translation, click/drag/wheel, multi-click, scrollbar drag, overlay hit-testing. | `nav::offset_at_cell`, `commands::scope_range`/`SelectScope`, `dispatch_overlay_command`, render geometry helpers |
| `wordcartel/src/term.rs` (extend) | `EnableMouseCapture` in setup; `DisableMouseCapture` at all 3 teardown sites. | crossterm |
| `wordcartel/src/editor.rs` (extend) | `Editor.mouse_capture: bool`; `Editor.mouse: MouseState`. | — |
| `wordcartel/src/app.rs` (extend) | `Msg::Input(Event::Mouse(ev))` arm → `mouse::handle`; `Tick` updates scrollbar-visible bool; main-loop `reconcile_mouse_capture`. | — |
| `wordcartel/src/render.rs` (extend) | Render the auto-hiding scrollbar; **extract overlay-rect geometry into shared helpers** (`palette_overlay_rect`, `menu_bar_area`) so `render` and `mouse` agree (DRY — no drift). | ratatui Scrollbar |
| `wordcartel/src/config.rs` (extend) | `mouse.capture: bool` (default `true`). | 5a config |
| `wordcartel/src/registry.rs` / `keymap.rs` (extend) | `toggle_mouse_capture` command (palette-only, no default chord). | 5b registry |

## 4. Capture lifecycle & the escape hatch

- **Setup (`term.rs`):** add `EnableMouseCapture` to the guard's setup `execute!` (after `EnableBracketedPaste`, term.rs:40). **Teardown:** add `DisableMouseCapture` to all three cleanup `execute!` calls (term.rs:46, 62, 82) so a panic/exit always releases the mouse.
- **Config:** `mouse.capture` (RawConfig `Option<bool>`, default `true`, merged per the 5a layered-config pattern). On startup the initial capture state follows the config.
- **Toggle:** `toggle_mouse_capture` command flips `editor.mouse_capture`. The **main loop reconciles** the flag with the terminal — mirroring `drain_clipboard_intents` (app.rs:1039): a `reconcile_mouse_capture(&mut editor, terminal.backend_mut(), &mut applied)` call compares `editor.mouse_capture` to the last-applied state and runs `execute!(Enable/DisableMouseCapture)` on change. (The command can't do terminal IO itself — the guard owns stdout — so it only sets the flag.)
- **Escape hatch:** with capture **off**, the terminal's native click-drag-select-and-copy works normally. The spec/README documents that most emulators (kitty, foot, VTE, iTerm2, Windows Terminal) also let you hold **Shift** to bypass app capture for a one-off native drag without toggling.

## 5. Coordinate translation

Mouse `(column, row)` are full-screen 0-based cells. The editing area (mirroring render.rs:61-64): `menu_rows = u16::from(menu.is_some())`, `edit_top = menu_rows`, `edit_height = h - (1 + menu_rows)`, status row = `h - 1`. A helper

```
fn editing_cell(editor, col: u16, row: u16) -> CellHit
```

classifies a point: `MenuBar` (row < menu_rows, menu open), `Status` (row == h-1), `Scrollbar` (scrollbar visible and col == w-1), `Text { col, erow }` where `erow = row - menu_rows` and `erow < edit_height`, else `Outside`. For a `Text` hit, the document offset is `nav::offset_at_cell(editor, col, erow)` (5c — already editing-area-relative; returns `None`/clamps past content). **The scrollbar overlays the last text column only while visible (transient); text layout is NOT reflowed on scroll** (avoids reflow jank — a click in that column during the ~1.2 s window routes to the scrollbar, otherwise it's text).

## 6. Core text-area gestures

Driven by `MouseEventKind` on `MouseButton::Left` (handler ignores Right; Middle is reserved for the deferred paste effort). All selection changes clear `sel_history` (consistent with 5c) except the multi-click ladder, which *seeds* it.

- **Down(Left)** on Text:
  - **Multi-click:** consult `MouseState.last_click = Option<{offset, at_ms, count}>`. If `clock.now_ms() - at_ms <= 400` and the new offset is on the same cell, increment `count`; else `count = 1`. Then:
    - `count == 1` → place caret: `Selection::single(offset)`, clear `sel_history`.
    - `count == 2` → `select_word` at offset (via `commands::scope_range(.., Scope::Word)`), seed `sel_history`.
    - `count == 3` → `select_paragraph` at offset, seed `sel_history`.
    - `count >= 4` → wrap to `1` (place caret).
  - Set `MouseState.anchor = Some(offset)`, `dragging = true`. Update `last_click`.
  - **Shift held** (`ev.modifiers` contains SHIFT) → *extend*: keep the current selection anchor, set head = clicked offset (`Selection::range(anchor, offset)`); does not start a fresh multi-click sequence.
- **Drag(Left)** on Text (or past the edges): head = `offset_at_cell(clamped to area)`, `Selection::range(MouseState.anchor, head)`. **Edge auto-scroll:** if `row < edit_top` scroll up one line; if `row >= edit_top + edit_height` scroll down one line (then recompute head at the clamped edge row). `ensure_visible` is NOT called mid-drag (the drag itself drives scroll).
- **Up(Left):** `dragging = false`; selection persists.
- **ScrollUp / ScrollDown:** scroll the **view** by 3 logical rows (adjust `view.scroll`/`scroll_row` via the existing scroll helpers), **caret unchanged**. Set `MouseState.scrollbar_until_ms = now + 1200`. The next caret-moving command re-centers via `ensure_visible`.
- A mouse selection populates the normal `Selection`/`primary` range, so `Ctrl+C` (copy) and the register work unchanged — mouse-select → keyboard-copy is seamless.

## 7. Auto-hiding scrollbar

- **Visibility:** `MouseState.scrollbar_until_ms` is set on every scroll/scrollbar-drag. The `Msg::Tick` handler sets a plain bool `MouseState.scrollbar_visible = clock.now_ms() < scrollbar_until_ms` each tick (so `render`, which has no clock, just reads the bool). Hidden by default; appears for ~1.2 s after the last scroll, then fades.
- **Render:** when visible, a `ratatui::Scrollbar` (`ScrollbarOrientation::VerticalRight`) on the rightmost editing column, with `ScrollbarState` built from `view.scroll` (position) and `derive::total_logical_lines` (content length) — computed locally in `render`, no stored widget state.
- **Drag/scrub:** a `Down`/`Drag(Left)` on the scrollbar column maps the row within the track to a scroll position proportionally (`scroll = (erow / edit_height) * max_scroll`), clamped. Sets the visibility window so it stays up during the drag. (Approximate against wrapped lines — acceptable for v1; the keyboard remains exact.)

## 8. Mouse on overlays

When `editor.palette` or `editor.menu` is open, `mouse::handle` routes clicks against the overlay BEFORE the text area:
- **Geometry (DRY):** extract the palette overlay rect and list-row layout (currently inline at render.rs:193-223) and the menu bar area (render.rs:246) into shared helpers in `render.rs` (`palette_overlay_rect(area) -> Rect`, `palette_row_at(area, col, row) -> Option<usize>`, `menu_bar_hit(area, col, row) -> Option<…>`). `render` and `mouse` both call them so a layout change can never desync hit-testing from the paint.
- **Palette:** a `Down(Left)` inside the list area → the row index → dispatch that row's command via `dispatch_overlay_command` (5b — closes the overlay, dispatches, drains, hydrates). A click outside the overlay rect → close the palette. Scroll on the palette → move the selected row (optional; v1 may ignore).
- **Menu:** a click on a top-level menu label → open/activate it through the existing `tui_menu::MenuState` nav; a click outside → close the menu. (If `tui-menu` 0.3 lacks usable mouse hit APIs, fall back to our own label hit-test → drive `MenuState`.)
- A captured click while a modal is open therefore never feels dead.

## 9. State, error handling, edge cases

- **`Editor` additions:** `pub mouse_capture: bool` (config-seeded) and `pub mouse: MouseState`, where `MouseState { anchor: Option<usize>, last_click: Option<ClickRecord>, dragging: bool, scrollbar_dragging: bool, scrollbar_until_ms: u64, scrollbar_visible: bool }`, `ClickRecord { offset: usize, at_ms: u64, count: u8 }`. The main loop tracks an `applied_mouse_capture: bool` for reconciliation.
- Click past end-of-line / last line → `offset_at_cell` clamps to the line/content end (5c behavior).
- Drag that produces an empty range (anchor == head) → a collapsed caret (fine).
- Rapid clicks beyond triple wrap to single (count modulo).
- Resize between events → coordinates recomputed per event from current `view.area`; no stale geometry.
- Capture currently off → mouse events still arrive only if the terminal sends them; the handler early-returns when `!editor.mouse_capture` (defensive — though disabling capture stops the stream).
- Overlay open + click: handled by §8 before text; `pending_mark` capture (5c) ignores mouse (it only consumes `Event::Key`), so a mouse event while a mark capture is pending falls through harmlessly — spec note: a click during mark-capture is ignored (capture resolves on the next *key*).

## 10. Testing strategy

- **`mouse::handle` unit suite** driving synthesized `MouseEvent`s with a `TestClock`:
  - Down→caret at the `offset_at_cell` of a cell (reuse 5c's `screen_pos`/`offset_at_cell` round-trip fixtures to pick cells with known offsets).
  - Drag→`Selection::range(anchor, head)`; edge auto-scroll advances `view.scroll`.
  - Shift+Down→extend keeps the anchor.
  - Double-click (two Downs within 400 ms, same cell)→word selection; triple→paragraph; both seed `sel_history` (a following `expand_selection` grows).
  - Wheel→`view.scroll` changes, `selection` (caret) unchanged.
  - Scrollbar drag→`view.scroll` proportional; `scrollbar_until_ms` set.
  - Palette open + click on a row→that command dispatched + palette closed; click outside→palette closed.
  - `toggle_mouse_capture`→flips `editor.mouse_capture`.
- **`term.rs`:** a guard test (or doc-asserted) that teardown includes `DisableMouseCapture` (mirrors the existing bracketed-paste teardown check).
- **Tick:** `scrollbar_visible` flips false once `now_ms()` passes `scrollbar_until_ms`.
- No pre-existing test weakened; full workspace green, zero warnings.

## 11. Module & command summary

- **New file:** `wordcartel/src/mouse.rs`.
- **New command:** `toggle_mouse_capture` (palette-only).
- **New config:** `mouse.capture` (bool, default true).
- **New `Editor` state:** `mouse_capture`, `mouse: MouseState`.
- **Render extraction (DRY for hit-testing):** `palette_overlay_rect` / `palette_row_at` / `menu_bar_hit` shared between `render` and `mouse`.
- **Reuses from 5c/5b:** `nav::offset_at_cell`, `commands::scope_range`/`Scope`, `sel_history` seeding, `dispatch_overlay_command`, `ensure_visible`, the scroll fields.

## 12. Deliberate decisions (for review)

1. **Capture ON by default + `toggle_mouse_capture` + Shift-bypass** — useful out of the box, native copy always reachable (§4).
2. **Wheel scrolls the view, caret unchanged** — reading vs editing (§6).
3. **Double=word, triple=paragraph**, seeding the 5c expand ladder; no quad-click (§6).
4. **`toggle_mouse_capture` has no default keybinding** — rare power toggle; saves key space (§3).
5. **Auto-hiding scrollbar, not always-on; overlays the last column while visible (no text reflow on scroll)** (§5/§7).
6. **Overlay hit-test geometry extracted into shared render helpers** so paint and hit-testing can't desync (§8).
7. **Middle-click paste deferred** to its own effort (§1).
