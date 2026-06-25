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
| **`wordcartel/src/mouse.rs`** (new) | All mouse-event handling: coord translation, click/drag/wheel, multi-click, scrollbar drag, palette hit-testing + outside-dismiss. | `nav::offset_at_cell`, `commands::scope_range_at`/`Scope`, `dispatch_overlay_command`, render geometry helpers |
| `wordcartel/src/term.rs` (extend) | `TerminalGuard::new(enable_mouse)` runs `EnableMouseCapture` in setup when enabled; `DisableMouseCapture` at all 3 teardown sites. | crossterm |
| `wordcartel/src/commands.rs` (extend) | `pub fn scope_range_at(editor, offset, Scope)` (existing private `scope_range` delegates). | 5c textobj/nav |
| `wordcartel/src/editor.rs` (extend) | `Editor.mouse_capture: bool`; `Editor.mouse: MouseState`. | — |
| `wordcartel/src/app.rs` (extend) | `Msg::Input(Event::Mouse(ev))` arm → `mouse::handle`; recompute `scrollbar_visible`; main-loop `reconcile_mouse_capture` (post-guard + per-iteration) + feed `scrollbar_until_ms` into the loop's `recv_timeout` deadline. | — |
| `wordcartel/src/render.rs` (extend) | Render the auto-hiding scrollbar; **extract overlay geometry into shared helpers** (`palette_overlay_rect`, `palette_row_at`, `menu_bar_area`) so `render` and `mouse` agree (DRY — no drift). | ratatui Scrollbar |
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

When `editor.palette` or `editor.menu` is open, `mouse::handle` routes clicks against the overlay BEFORE the text area.

**Palette (fully clickable):**
- **Geometry (DRY):** extract the palette overlay rect + list-row layout (currently inline at render.rs:193-223) into shared helpers in `render.rs` — `palette_overlay_rect(area) -> Rect` and `palette_row_at(area, palette, col, row) -> Option<usize>`. `render` and `mouse` both call them so a layout change can never desync hit-testing from the paint.
- A `Down(Left)` inside the list area → the row index → dispatch that row's command via `dispatch_overlay_command` (5b — closes the overlay, dispatches, drains, hydrates). A click **outside** the overlay rect → close the palette.

**Menu (click-outside-to-dismiss only — scoped down per Codex Important):** `tui-menu 0.3` exposes no mouse/hit API, and `MenuView` retains only `MenuState<CommandId>` + `built` (the grouped label model is discarded after `build`, and the dropdown geometry is internal to tui-menu and not queryable). So in-menu item clicking is **not feasible in 5c-m without replacing tui-menu's rendering with our own** (a larger change to 5b's menu, out of scope here). For 5c-m the menu stays **keyboard-driven** (F10 + arrows, as 5b), with one mouse affordance: a `Down(Left)` **outside the menu's drawn area** dismisses it (`menu_bar_area`, render.rs:246, exposed as a shared helper for the outside-test). Clicking a menu item is a documented non-goal for this effort; a follow-up can add full menu mouse by retaining the grouped model + self-rendering the dropdowns.

A captured click while the palette is open dispatches/dismisses; while the menu is open it dismisses on an outside click — so a captured click is never fully dead.

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
  - Menu open + click outside→menu closed (in-menu item click is out of scope this effort).
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
- **Render extraction (DRY for hit-testing):** `palette_overlay_rect` / `palette_row_at` / `menu_bar_area` shared between `render` and `mouse`.
- **Main-loop additions:** `reconcile_mouse_capture` (post-guard + per-iteration, clears drag state on off); `scrollbar_until_ms` fed into the `recv_timeout` deadline.
- **Reuses from 5c/5b:** `nav::offset_at_cell`, `commands::scope_range_at`/`Scope`, `sel_history` seeding, `dispatch_overlay_command`, `ensure_visible`, the scroll fields.

## 12. Deliberate decisions (for review)

1. **Capture ON by default + `toggle_mouse_capture` + Shift-bypass** — useful out of the box, native copy always reachable (§4).
2. **Wheel scrolls the view, caret unchanged** — reading vs editing (§6).
3. **Double=word, triple=paragraph**, seeding the 5c expand ladder; no quad-click (§6).
4. **`toggle_mouse_capture` has no default keybinding** — rare power toggle; saves key space (§3).
5. **Auto-hiding scrollbar, not always-on; overlays the last column while visible (no text reflow on scroll); fade driven by a loop `recv_timeout` deadline, not idle `Tick`** (§5/§7).
6. **Palette is fully mouse-clickable via extracted shared render geometry; the menu is click-outside-to-dismiss only** — in-menu item clicking is deferred because `tui-menu 0.3` exposes no mouse/geometry API and `MenuView` discards its label model (Codex; §8). A follow-up can add full menu mouse by self-rendering the dropdowns.
7. **Capture honored from frame 1** via `TerminalGuard::new(enable_mouse)` + a post-guard reconcile; toggling off clears drag state (§4).
8. **Mouse ignored during `pending_mark` capture; click below content → caret to document end** (§9).
9. **Middle-click paste deferred** to its own effort (§1).
