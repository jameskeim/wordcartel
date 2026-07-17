# A21 — Overlay mouse interaction model — design note (spec)

**Date:** 2026-07-17
**Effort size:** SM (small–medium) — ~5 tasks, all in the shell crate (`wordcartel/src/mouse.rs`
+ `wordcartel/src/list_window.rs` + tests). No `wordcartel-core` change.
**Anchor:** A21 (backlog theme A — command surface; `docs/ux-backlog.md`, marker `<!-- item: A21 -->`).
**Severity:** interaction-only. No data-loss surface (no slot mutates the document buffer);
hover rides an existing per-event render loop, so no new hot-path class is introduced.
**Grounding packet:** `scratchpad/a21-overlay-mouse/grounding-packet.md`.
**Grounding report:** returned to the coordinator (this thread) — the per-overlay inventory in §3
below is lifted verbatim from it, re-anchored on real symbol names.
**Three human rulings folded in (coordinator, 2026-07-17):** Decision 1 = (a), Decision 2 = (ii),
Decision 3 = (A) — see §2.

**Command-surface contract:** **N/A — does not touch the command surface.** This effort adds and
removes no commands and does not change the registry, the palette contents, the menu *structure*
(`menu::grouped_commands` / `registry::MENU_ORDER` are untouched), or keybinding hints. It changes
mouse *interaction* only. Menu-bar hover-to-switch re-targets `MenuView::open` — the same field the
keyboard `←`/`→` arms in `menu::intercept` already move — it does not change which commands a
dropdown row dispatches. The contract's invariant tests (palette-completeness,
every-option-has-a-command, hint re-resolution) are unaffected. Both this spec and the plan carry
this line explicitly.

---

## 1. Problem statement

A mouse over an open list overlay today has three gestures with inconsistent, GUI-unfamiliar
meanings, and a fourth (hover) that does nothing:

- **Hover (`MouseEventKind::Moved`)** — silently consumed. Every list overlay's `mouse` slot in
  `mouse.rs` has exactly a `Scroll` arm and a `Down(Left)` arm; none has a `Moved` arm. Moving the
  pointer over a dropdown never moves the highlight. (Verified: the slots `mouse_palette`,
  `mouse_menu`, `mouse_theme_picker`, `mouse_cursor_picker`, `mouse_file_browser`, `mouse_outline`,
  `mouse_diag` in `mouse.rs`.)
- **Wheel** — moves the highlight `±1` and re-windows. This conflates "scroll the view" with "move
  the selection": one notch = one row, so a long list is slow to traverse, and the highlight is the
  thing that scrolls, not the viewport.
- **Click (`Down(Left)`)** — selects/dispatches the row under the pointer. Correct; unchanged.

The target is a single coherent model where the pointer *means* what it points at (hover
highlights; wheel scrolls the viewport and the highlight follows the pointer; click selects) —
matching every GUI list — while the keyboard path is untouched and the app's hot-path and
no-data-loss invariants hold.

### 1.1 Event delivery is already in place (the load-bearing precondition)

A bare `Moved` reaches the overlay `mouse` slots today. Chain, each link verified against source:

1. **Input thread** (`app::run`, the `wcartel-input` `std::thread::Builder` spawn): forwards every
   `crossterm::event::read()` result as `Msg::Input(ev)` — no kind filtering, no coalescing.
2. **Intercept chain** (`app::reduce_dispatch`): `OverlayId::Splash.row().intercept` →
   `marks::intercept` → `OverlayId::ALL[1..]` rows. Every list-overlay `intercept`
   (`palette::intercept`, `menu::intercept`, `theme_picker::intercept`, `cursor_picker::intercept`,
   `file_browser::intercept`, `outline_overlay::intercept`, `diag_overlay::intercept`) matches only
   `Event::Key` / paste arms and returns `Handled::Pass(msg)` for everything else. The **only**
   intercept that consumes a mouse event is `splash::intercept` (its
   `Msg::Input(Event::Mouse(m)) if matches!(m.kind, MouseEventKind::Down(_))` arm dismisses the
   splash). So `Moved` and `Scroll` pass the whole chain for all list overlays.
3. **`app::reduce`'s `Msg::Input(Event::Mouse(ev))` arm** → `mouse::handle` unconditionally.
4. **`mouse::handle`**: after the `pending_mark`/`mouse_capture` guard and the universal
   `Up(Left)` drag-clear, it runs `if !no_overlay_open(editor) { route_overlay(editor, ev, area,
   &ctx); return; }` — **before** any `ev.kind` match. `route_overlay` finds the active overlay via
   `overlays::OverlayId::ALL … .find(|id| (id.row().is_active)(editor))` and calls
   `(id.row().mouse)(editor, ev, area, ctx)` with the raw event.

So adding `Moved`/wheel behavior is purely a matter of adding arms to the existing slots; no
plumbing change is required. `MouseEvent { kind, column, row, modifiers }` carries non-optional
`column`/`row` on every kind (crossterm 0.28.1 — `wordcartel/Cargo.toml`), including `ScrollDown`/
`ScrollUp`, so the wheel handler can hit-test at the pointer.

**Cost context:** `app::run`'s main loop is `recv_timeout` → `reduce` → … → `terminal.draw` **per
message**; every `Moved` already costs a full render pass today. Hover work rides that existing
loop; the marginal cost is the added slot arm only (a hit-test + a bounded side effect), and §2's
dedupe invariant bounds it to *row-change* events, never per-frame-at-rest.

---

## 2. The settled model — invariants (each testable)

The approved model with the three human rulings folded in. Each invariant is stated so a test can
assert it; §6 maps tests to them.

**I1 — Hover highlights within the list rect.** A `Moved` whose `(column, row)` hits a selectable
list row sets that overlay's highlight to the row under the pointer. "Hit" is the overlay's own
hit-tester returning `Some(idx)` (see §3); the highlight field is `selected` for six overlays and
`highlighted` for the menu (§3.1).

**I2 — Off-rect / non-row hover leaves the highlight as-is.** When the hit-tester returns `None`
(pointer outside the list interior, over a border column, a query row, or the menu overflow
indicator row), hover does nothing — the highlight stays wherever the last input (mouse or keyboard)
put it. "Whichever input last acted wins"; hover simply does not act off-rect. This makes I1's body
a single `if let Some(idx) = hit`.

**I3 — Wheel = scroll-the-viewport, then drag the highlight to the window edge, then re-hover
[Decision 1(a)].** Every overlay's wheel handling opens with an empty-list guard `if row_count == 0
{ return; }` (I3b) — an empty list never steps, scrolls, clamps, re-hovers, or fires a preview. Then,
on a `ScrollDown`/`ScrollUp` over a list whose `row_count > list_h` (the list overflows its window):
  1. `list_window::wheel_scroll(down, row_count, list_h, &mut scroll_top)` slides `scroll_top` by
     `WHEEL_STEP` (= 3), clamped to `[0, row_count.saturating_sub(list_h)]`.
  2. `list_window::clamp_into_window(&mut highlight, scroll_top, list_h, row_count)` pulls the
     highlight into the new visible window `[scroll_top, scroll_top + list_h)` — an **active wheel
     gesture drags the highlight**, so the keyboard-set highlight is only displaced when the user
     deliberately scrolls it off. (This is the ruling that reconciles the wheel with the shipped
     painter re-window layer — see §2.1.)

  The `list_h` fed to steps 1 and 2 is the **effective item-row budget** of the overlay, not
  necessarily its raw window height. For the six palette-family overlays these are equal
  (`list_window::list_h_for(row_count, area_h)`); for the **menu dropdown** the item budget reserves
  the bottom row for the overflow indicator (§3, Wrinkle 1) — so both `wheel_scroll`'s `max_top` and
  `clamp_into_window`'s window agree with the painter and hit-tester and the highlight can never land
  on the reserved indicator row.
  3. Re-hover: hit-test at the wheel event's own `(ev.column, ev.row)`; if it hits a row, set the
     highlight to that row (overriding step 2). A wheel emits no `Moved` (the pointer is physically
     stationary; only content slides beneath it), so the handler must re-hover itself. Result: while
     the pointer stays over the list, the highlight is pinned to the pointer and never scrolls
     off-screen.

**I3b — Empty list is a total wheel no-op.** When `row_count == 0` (a legitimate state:
`palette::rebuild_rows` / `theme_picker::rebuild_rows` on an empty fuzzy-filter,
`outline_overlay::set_query` with no match, `file_browser::rebuild_entries` on an empty read) the
wheel does nothing at all — no step, no scroll, no clamp, no re-hover, no preview. This is the DRYest
single guard at the head of each wheel arm and covers every sub-branch; it mirrors the hit-testers,
which degrade to `None` on an empty list. The short-list step math (I4) is *also* written with the
saturating idiom so it cannot underflow even independent of this guard (belt-and-suspenders).

**I4 — Short-list wheel steps the highlight ±1 [Decision 2(ii)].** When `0 < row_count <= list_h`
(nothing to scroll — the whole list is visible), the wheel instead moves the highlight `±1`. The new
code standardizes on the **saturating idiom**: down = `(sel + 1).min(row_count.saturating_sub(1))`,
up = `sel.saturating_sub(1)` — non-underflowing at `row_count == 0` on its own (and I3b guards it
anyway). This matches the form five of the six palette-family arms already use; it deliberately does
NOT reproduce `mouse_menu`'s `(highlighted + 1).min(n - 1)`-inside-`if n > 0` shape (also safe, but a
different idiom — see §5). I3 and I4 compose into one rule: **the
wheel always moves you *through* the list — it scrolls when the list is long, steps when it is
short.** On the preview overlays this `±1` step fires the preview funnel too (§ I8), dedupe-guarded.

**I5 — Dedupe on row-change (hot-path law, theme-R).** For hover (I1) and for the wheel's re-hover
(I3.3) and short-list step (I4), the side effects — writing the highlight, calling keep-visible, and
(for the preview overlays) firing the preview funnel — fire **only when the resolved row differs
from the current highlight value**. Never redundantly at the same row on every motion frame. Expressed
as `if hit != Some(cur) { … }`. This bounds preview re-derives to *rows crossed*, not motion frames,
and also removes the pre-existing redundancy where the theme/cursor wheel arms re-fire preview at the
clamp boundary.

**I6 — Menu-bar hover-to-switch [Decision option (ii)].** While a dropdown is **already open**, a
`Moved` onto a *different* bar category label (the `File Edit View …` strip) switches the open
dropdown to that category live. The switch applies the **reset triple** — `open = cat;
highlighted = 0; scroll_top = 0` — identical to the keyboard `←`/`→` arms in `menu::intercept` and
to the existing `Down(Left)` bar-hit arm in `mouse_menu`. Guarded by `cat != m.open` (I5 dedupe: no
re-reset while the pointer wanders within the same label). **First-open stays deliberate:** hovering
the bar with *no* menu open does not auto-open a dropdown — hover-switch lives entirely inside
`mouse_menu` (the open-overlay path); the no-overlay dwell-reveal path (A1) is untouched. **Off-menu
hover does not close** the menu — closing stays click-outside / Escape / select (the existing
`Down(Left)` close-away arm and `menu::intercept` Esc).

**I7 — Hover-to-switch is provably disjoint from the A1 dwell timers.** In `mouse::handle`, the
`route_overlay` branch `return`s before all three dwell-arming blocks (menu-bar, scrollbar,
status-line), which run only under `no_overlay_open(editor)` (= `!overlays::any_active`). A `Moved`
can therefore never both hover-switch a dropdown and arm a dwell timer in the same event. (Noted
pre-existing, unchanged: a `menu_reveal_due` armed *before* a menu opened still fires in
`chrome::recompute_menu_bar` while the menu is open — it gates only on `MenuBarMode::Auto` — setting
`menu_bar_revealed = true`; harmless, the bar paints under an open dropdown anyway. A21 neither
introduces nor worsens this.)

**I8 — Embrace hover-preview on `theme_picker` + `cursor_picker` [Decision 3(A)].** On these two
overlays, moving the highlight is not just a highlight — it fires a live preview. Hover (I1), the
wheel re-hover (I3.3), and the short-list step (I4) each set `selected` and then fire the same
preview funnel the keyboard/wheel already fire:
  - theme: `theme_cmds::preview_selected_theme(editor)` — re-derives + applies the theme and records
    the name in `tp.previewed`.
  - cursor: `cursor_picker::preview_selected(editor)` — sets caret shape/blink via the shared
    setters (the DECSCUSR emission is the edge-triggered `cursor_style::reconcile_cursor_style` loop
    seam).
  The restore funnels are unchanged: Esc and click-away restore `tp.original` via
  `Editor::apply_theme` (theme) / `original_shape`+`original_blink` via `set_caret_shape`/
  `set_caret_blink` (cursor); commit takes `previewed` into `theme_identity`
  (`theme_cmds::commit_theme_picker`) / closes (`commit_cursor_picker`). Because `tp.previewed`
  tracks the *last preview regardless of which input set `selected`*, hover-preview needs zero funnel
  change. I5's dedupe keeps it bounded to rows crossed. A recorded UX consequence: a hover-crossed
  theme stays applied when the pointer leaves the list (until Esc/click-away/commit) — identical to
  today's wheel-preview persistence, just easier to trigger.

**I9 — `Drag` is not `Moved` (stated as a decision).** crossterm reports pointer motion with a
button held as `MouseEventKind::Drag(button)`, not `Moved`. Every list slot ignores `Drag` today and
continues to: **hover does not track during a drag-through of an overlay.** This is deliberate — a
drag over a modal has no meaning here, and there is no data-loss or panic risk in ignoring it.

**I10 — No-motion-tracking terminals degrade gracefully.** On a terminal that does not report bare
`Moved` (only clicks/wheel), hover simply never fires — no error, no fallback needed. The wheel
re-hover (I3.3) still works, because it hit-tests at the *wheel event's* own coordinates, which every
terminal supplies. Click and keyboard are unaffected.

**I11 — No data loss, no click-through.** No list `mouse` slot mutates the document buffer. A hover
or wheel over an open overlay never leaks to the editor gesture path (the `route_overlay` early
`return` guarantees it). Adding `Moved`/wheel arms preserves this: the completeness sweep asserts it
per slot (§6, T5).

### 2.1 Why 1(a) and not an independent scroll-state (design rationale, recorded)

Every overlay painter re-runs keep-visible with the live frame geometry **every frame** — verified
in `render_overlays.rs`: `keep_overlay_visible(h, p.selected, p.rows.len(), &mut p.scroll_top)` for
palette/outline/theme/cursor/file_browser/diag, and `list_window::keep_visible(m.highlighted, …)`
for the menu dropdown. This is the shipped **two-layer invariant** (`list_window.rs` module header):
the window follows the *selection*, and each painter re-windows against the true item-row budget so
resize survives without an event hook.

Consequence: "scroll the viewport, leave the highlight" (the packet's naive wheel wording) is
**unsatisfiable** against this painter — if a wheel moved `scroll_top` without moving `selected`, and
the pointer were over a non-row cell so the re-hover found nothing, the very next frame's painter
would snap `scroll_top` back to keep `selected` visible, and the wheel would visibly do nothing. The
human ruled **1(a)**: an active wheel gesture drags the highlight into the new window
(`clamp_into_window`), so `selected` stays inside the visible item window and the painter does not
fight it. This keeps the two-layer invariant intact (no painter change) and keeps the wheel honest.
(Rejected: 1(b) dead-zone wheel — felt broken over borders; 1(c) independent scroll state — amends
the resize-survival invariant, materially bigger.)

For the agreement to hold the wheel must window against the **same effective item budget the painter
uses** — for the menu that is the overflow-adjusted budget, not the raw dropdown height. The menu
painter (`render_overlays::paint_menu_dropdown`) computes `let overflows = leaves_len > list_h; let
keep_h = if overflows { list_h.saturating_sub(1) } else { list_h };` and windows with `keep_h`; the
menu hit-tester (`chrome_geom::menu_dropdown_row_at`) computes the identical `item_rows`. So
`wheel_scroll`/`clamp_into_window` for the menu are fed that same overflow-adjusted `list_h` (§3,
Wrinkle 1). The palette family has no such reservation, so it feeds `list_h_for` unchanged.

Both helpers also mirror `keep_visible`'s **`list_h == 0` behavior** (it forces `scroll_top = 0` and
leaves the selection alone) so the wheel state never diverges from the per-frame painter *at any
window height* — including the degenerate cases where the effective budget is 0 (a tiny terminal
whose window height collapses, or a 1-row menu window on an overflowing category → `raw_window − 1 ==
0`). `wheel_scroll` clamps `max_top` to 0 when `list_h == 0`; `clamp_into_window` is a no-op there
(the window is empty, nothing renders, the highlight is moot until a real window exists). This is a
robustness/saturating-arithmetic guard (H7 / forbid-unsafe), not new behavior — `menu_dropdown_row_at`
already returns `None` for every cell when `item_rows == 0`, so no wrong dispatch was ever possible;
the guard only keeps the wheel's own `scroll_top`/highlight state from transiently disagreeing with
the painter and removes a latent underflow in the clamp.

Symmetrically, an **empty list (`row_count == 0`)** is a total wheel no-op in every overlay (I3b) —
the wheel arm returns before any step/scroll/clamp/re-hover/preview, mirroring the hit-testers, which
already return `None` on an empty list. This too is a saturating-arithmetic robustness guard (H7 /
forbid-unsafe), not new behavior — five of the six palette-family arms already clamp with their own
row-count's `saturating_sub(1)` (`mouse_palette`/`mouse_theme_picker`/`mouse_outline` via
`rows.len().saturating_sub(1)`, `mouse_file_browser` via `entries.len().saturating_sub(1)`,
`mouse_diag` via `d.row_count().saturating_sub(1)`), and `mouse_cursor_picker` via
`ROW_ACTIONS.len().saturating_sub(1)`, while `mouse_menu` steps a guarded `(highlighted + 1).min(n -
1)` under `if n > 0`; all are non-underflowing today, and I4 standardizes the new code on the
`saturating_sub` form. A naive `n - 1` step (which the spec never adopts) would have been the
regression.

---

## 3. Scope surface + per-overlay inventory (ground truth)

**IN — the seven list overlays** (each has a highlight index + `scroll_top`): `palette`, `menu`
(dropdown), `theme_picker`, `cursor_picker`, `file_browser`, `outline`, `diag`.

**OUT — explicitly** (each already no-ops `Moved`; A21 does not touch them):
- `minibuffer` — `Minibuffer { prompt, text, cursor, … }`; `mouse_minibuffer` acts only on
  `Down(Left)` (caret placement). No highlight/scroll to drive.
- `search` — `SearchState` (fields + matches, no list highlight); `mouse_search` acts only on
  `Down(Left)`.
- `prompt` — `Prompt { message, choices }` (no `selected`/`scroll_top`); `mouse_prompt` acts only on
  `Down(Left)` over a `[K]` marker.
- `splash` — `splash::mouse` swallows `Moved`/`Scroll` and dismisses on `Down`.

These four require **zero code change** — they already consume `Moved` as no-ops.

### 3.1 Inventory table (all seven IN overlays)

Every hit-tester lives in `chrome_geom.rs`, takes `(area: Rect, &<State>, col: u16, row: u16)`, and
returns `Option<usize>` — an **absolute** index already accounting for `scroll_top`. `list_h` inside
keep-visible is `list_window::list_h_for(row_count, area_h)` for the six palette-family overlays; the
menu is the sole exception (see the wrinkle note below the table).

| Slot (`mouse.rs`) | State field(s) | Highlight field | Row count expr | Hit-tester (`chrome_geom.rs`) | keep-visible today | `Down` side effect | Wheel arm has early `return`? |
|---|---|---|---|---|---|---|---|
| `mouse_palette` | `editor.palette` (`Palette`): `rows`, `selected`, `scroll_top` | `selected` | `p.rows.len()` | `palette_row_at` | `app::keep_overlay_visible` | dispatch cmd / buffer-switch (`workspace::switch_to`) | yes |
| `mouse_menu` | `editor.menu` (`MenuView`): `groups`, `open`, `highlighted`, `scroll_top`, `built` | **`highlighted`** | `groups[open].1.len()` | bar: `menu_bar_layout(menu_area(area), groups)` → `(group_idx, Rect)`; rows: `menu_dropdown_row_at(menu_area(area), groups, open, scroll_top, col, row)` | `list_window::keep_visible` (menu `list_h`, below) | bar → reset triple; row → `menu::dispatch_row_action` | yes |
| `mouse_theme_picker` | `editor.theme_picker` (`ThemePicker`): `rows`, `selected`, `scroll_top`, `original`, `previewed` | `selected` | `tp.rows.len()` | `theme_picker_row_at` | `app::keep_overlay_visible` | set+kv → `preview_selected_theme` → `commit_theme_picker` | no (see §7) |
| `mouse_cursor_picker` | `editor.cursor_picker` (`CursorPicker`): `selected`, `scroll_top`, `original_shape`, `original_blink` | `selected` | `cursor_picker::ROW_ACTIONS.len()` (fixed 7) | `cursor_picker_row_at` | `app::keep_overlay_visible` | set+kv → `preview_selected` → `commit_cursor_picker` | no (see §7) |
| `mouse_file_browser` | `editor.file_browser` (`FileBrowser`): `entries`, `selected`, `scroll_top` | `selected` | `fb.entries.len()` | `file_browser_row_at` | `app::keep_overlay_visible` | set+kv → `file_browser::file_browser_enter` | no (see §7) |
| `mouse_outline` | `editor.outline` (`OutlineOverlay`): `rows`, `selected`, `scroll_top`, `opened_version` | `selected` | `o.rows.len()` | `outline_row_at` | `app::keep_overlay_visible` | set+kv → stale-version guard (inline) → `outline_overlay::outline_jump_to` | yes |
| `mouse_diag` | `editor.diag` (`DiagOverlay`): `selected`, `scroll_top`, `opened_version`; count via `row_count()` | `selected` | `d.row_count()` (= `anchor.suggestions.len() + 2`) | `diag_row_at` | `app::keep_overlay_visible` | set+kv → `search_ui::diag_apply_selected` | yes |

**Wrinkle 1 — `list_h` is not one formula, and the menu's value is overflow-adjusted.** The shared
wheel helper **takes `list_h` as a parameter** — it must not recompute it — so each slot passes its
own **effective item-row budget** (the count of rows that actually hold selectable items, which is
what both the painter's `keep_visible` and the hit-tester window against):

- **Palette family (six overlays)** — window via `app::keep_overlay_visible(area_h, sel, n, &mut
  top)`, which computes `list_h` internally as `list_window::list_h_for(n, area_h)` (=
  `n.min(15).min(area_h.saturating_sub(4))`). There is **no reserved row** (Wrinkle-note below), so
  the effective budget *is* `list_h_for(n, area_h)`. These six pass that value unchanged.

- **Menu dropdown** — the raw window height is `raw_window = n.min(15).min(menu_area(area).height
  .saturating_sub(1))` (`n = groups[open].1.len()`; see `render_overlays::paint_menu_dropdown` and
  `chrome_geom::menu_dropdown_rect`). When the category **overflows** that window the bottom row is
  reserved for the `n/total` indicator, so the effective item budget is
  `if n > raw_window { raw_window.saturating_sub(1) } else { raw_window }`. This mirrors
  `paint_menu_dropdown`'s `keep_h` and `menu_dropdown_row_at`'s `item_rows` **exactly**
  (both: `let overflows = leaves_len > list_h; let … = if overflows { list_h.saturating_sub(1) }
  else { list_h };`), so the wheel's `max_top` and the post-scroll highlight clamp agree with the
  painter and hit-tester, and the highlight can never land on the reserved indicator row. The menu
  slot passes this overflow-adjusted budget to both `wheel_scroll` and `clamp_into_window`.

  Note — this **corrects a pre-existing latent mismatch**: today's `mouse_menu` wheel arm passes the
  raw `n.min(15).min(avail_below)` (without the overflow `−1`) to `keep_visible`, so on an
  overflowing dropdown its `keep_visible` uses a budget one larger than the painter's `keep_h`. It is
  latent today because the arm only moves `highlighted` by `±1` and the per-frame painter re-windows
  with the correct `keep_h` immediately after; A21's wheel computes a `max_top` and clamps the
  highlight itself, so it must use the painter's budget to stay consistent.

**Wrinkle 2 — `menu_dropdown_row_at` takes `scroll_top` as a parameter** (unlike the palette-family
testers, which read `scroll_top` off the borrowed state struct). Its signature:
`menu_dropdown_row_at(area, groups, open, scroll_top, col, row) -> Option<usize>`. It also reserves
the overflow-indicator row (returns `None` for a click there) and carries a defensive
`abs < leaves_len` guard.

**Wrinkle 3 — clamp before hit-test on the palette family.** The palette-family hit-testers compute
the absolute index as `(row - list_top) + scroll_top` and do **not** re-verify `abs < row_count`
(only `menu_dropdown_row_at` does). They rely on `keep_visible`'s post-condition
`scroll_top <= row_count.saturating_sub(list_h)`. So the wheel handler must apply `wheel_scroll`'s
clamp (which enforces exactly that bound) **before** the re-hover hit-test, or a hit at the bottom
visual row of an over-scrolled window could yield an out-of-range absolute index. `wheel_scroll`
clamps `scroll_top` to `[0, row_count - list_h]` by construction, so ordering step I3.1 before I3.3
satisfies this.

**Wrinkle 4 — the naming split.** Six overlays name the highlight `selected`; the menu names it
`highlighted`. Any shared helper must be name-agnostic — it takes `&mut usize`, never a field name
(see §5).

**Geometry note (I2 cleanliness).** All six palette-family testers bound-check
`col ∈ [r.x+1, r.x+r.width-1)` and `row ∈ [list_top, list_top+list_h)` and return `None` outside —
borders, query rows (`cursor_picker`/`diag` start their list at `ov_y+1`; palette/theme/file/outline
at `ov_y+2`), and empty regions all cleanly miss. The palette-family "n/total" overflow indicator is
a border *title* (`chrome_geom::windowed_indicator`), not a list row, so there is no reserved row to
dodge there; only the menu dropdown reserves one (handled inside `menu_dropdown_row_at`).

---

## 4. Preview-overlay cost (why I8 is cheap — grounded)

`theme_cmds::preview_selected_theme` → `Theme::builtin(name)` → depth-policy branch
(`theme_resolve::apply_ansi16_chrome_policy` at `Depth::Ansi16` on an Rgb theme, else
`Theme::derive_chrome(disposition)` — fixed-size color math) → `Editor::apply_theme`, which assigns
the theme then calls `derive::rebuild` + `nav::ensure_visible`. With an **unchanged document
version**, `derive::rebuild` skips the parse phase entirely (its `version != blocks_version` guard)
and runs only the downstream phase: the generation-gated fold-anchor prune, `FoldView::compute`
(O(1) when the buffer has no folds — the R1 `folds.is_empty()` early return; the O(document)
`sections()` walk only when folds exist, and that walk already runs on every rebuild today), and the
O(visible) layout-cache refresh. So one hover-preview ≈ one extra pre-draw derive pass — the **same
cost class** as the shipped per-notch wheel preview and the per-keypress keyboard preview
(`theme_picker::intercept` already fires `preview_selected_theme` on every Up/Down). `cursor_picker`
preview is two field stores — effectively free. I5 bounds a hover sweep to ≤ (rows crossed) such
passes, never per-frame-at-rest. This satisfies theme-R.

---

## 5. Shared-helper decision

**One pure helper module addition, in `list_window.rs`; the hover-set + side effect stays
per-slot.** Justified against the existing `apply_list_nav` precedent: that helper's own doc comment
states "Per-overlay SIDE EFFECTS (theme-preview, outline re-query, etc.) stay in the caller, outside
this pure helper." A21 follows the identical split — the arithmetic is shared and pure; the
`selected`-vs-`highlighted` write and the preview/jump/apply side effect cannot be generic and stay
in each slot.

New in `list_window.rs` (all name-agnostic via `&mut usize` — Wrinkle 4). In every signature `list_h`
is the **effective item-row budget** the caller supplies (Wrinkle 1): `list_h_for(n, area_h)` for the
palette family, the overflow-adjusted budget for the menu. The helper never recomputes it, so the
menu's reserved indicator row is honored identically to the painter and hit-tester:

```rust
/// Wheel step: rows the viewport slides per notch, uniform across overlays (A21).
pub(crate) const WHEEL_STEP: usize = 3;

/// Wheel notch → viewport scroll. `list_h` is the caller's effective ITEM-ROW budget (the menu
/// passes its overflow-adjusted value, not the raw dropdown height — Wrinkle 1). Slide `scroll_top`
/// by ±WHEEL_STEP, clamped to [0, max_top] where `max_top = 0` when `list_h == 0` (nothing renders)
/// and `row_count.saturating_sub(list_h)` otherwise — the SAME bound `keep_visible` enforces,
/// including its `list_h == 0 → scroll_top = 0` reset, so wheel state never diverges from the
/// per-frame painter at any window height (degenerate tiny terminal / 1-row-overflow included).
/// A subsequent hit-test on the palette family (which does not re-verify abs < row_count) is
/// therefore safe. Selection is untouched; the caller drags the highlight in via
/// `clamp_into_window`, then re-hovers. All-saturating: no height can panic (H7 / forbid-unsafe).
pub(crate) fn wheel_scroll(down: bool, row_count: usize, list_h: usize, scroll_top: &mut usize) {
    let max_top = if list_h == 0 { 0 } else { row_count.saturating_sub(list_h) };
    *scroll_top = if down { (*scroll_top + WHEEL_STEP).min(max_top) }
                  else { scroll_top.saturating_sub(WHEEL_STEP) };
}

/// Pull `highlight` into the visible item window [scroll_top, scroll_top + list_h) after a wheel
/// scroll (Decision 1a) — an active wheel gesture drags the highlight to the window edge. No-op
/// when it is already inside. `list_h` is the effective item budget (menu = overflow-adjusted), so
/// the highlight can never land on the menu's reserved indicator row. `list_h == 0` (degenerate
/// tiny terminal, or a 1-row menu window overflowing → effective budget 0): the window is EMPTY and
/// nothing renders, so leave the highlight and scroll_top untouched — mirrors `keep_visible`, whose
/// `list_h == 0` arm resets scroll_top to 0 (which the wheel already did via `wheel_scroll`) and
/// leaves the selection alone; the highlight position is moot until a real window exists. All
/// arithmetic saturating: no height can underflow or panic (H7 / forbid-unsafe).
pub(crate) fn clamp_into_window(highlight: &mut usize, scroll_top: usize, list_h: usize, row_count: usize) {
    if row_count == 0 || list_h == 0 { return; }
    let last = row_count - 1;
    let lo = scroll_top.min(last);
    let hi = scroll_top.saturating_add(list_h - 1).min(last);
    *highlight = (*highlight).clamp(lo, hi);
}
```

Both are pure (no `Editor`, no side effect), unit-testable without a fixture, and reused by all seven
slots. Each slot's wheel arm opens with `if row_count == 0 { return; }` (I3b — empty list is a total
no-op, mirroring the hit-testers' `None`), then branches
`if row_count <= list_h { step ±1 } else { wheel_scroll; clamp_into_window; re-hover }`, where
`list_h` is the effective item budget. The short-list `±1` step standardizes on the **saturating
idiom** — down = `(sel + 1).min(row_count.saturating_sub(1))`, up = `sel.saturating_sub(1)` — so it
is non-underflowing at `row_count == 0` even without the I3b guard; no new helper needed. This is the
form five of the six palette-family arms already use — each clamps with its OWN row-count's
`saturating_sub(1)` (`mouse_palette`/`mouse_theme_picker`/`mouse_outline` on `rows.len()`,
`mouse_file_browser` on `entries.len()`, `mouse_diag` on `d.row_count()`) — and `mouse_cursor_picker`
uses the equivalent `ROW_ACTIONS.len().saturating_sub(1)`. `mouse_menu` is the outlier: it steps
`(highlighted + 1).min(n - 1)` guarded by `if n > 0` — also non-underflowing, but a different idiom;
the new code adopts the `saturating_sub` form uniformly rather than reproducing the guarded-`n - 1`
shape. For the menu this means a category that exactly fills its window with no
overflow (`n == raw_window`) is treated as short (steps ±1), while an overflowing one
(`n > raw_window`, effective budget `raw_window − 1`) scrolls — consistent with the painter, which
only reserves the indicator row when the category overflows.

Hover is a per-slot three-liner, not a helper: `let hit = <tester>(area, state, col, row); if hit !=
Some(cur) { if let Some(idx) = hit { set highlight; keep-visible; <side effect> } }`. The
side-effect line (`preview_selected_theme` / `preview_selected` / nothing / nothing) is the part that
resists genericization, exactly as `apply_list_nav` anticipated.

---

## 6. Test plan (TDD boundaries → tasks)

Anchor on symbol names; `:NNN` line anchors will drift. Each test names the invariant it pins.

**T1 — pure helpers (`list_window.rs`, `#[cfg(test)]`).** Unit-test `wheel_scroll` (down/up,
clamp at `0` and at `row_count - list_h`, `WHEEL_STEP` magnitude, `list_h >= row_count`, and
**`list_h == 0` → `scroll_top == 0` in BOTH directions** — the painter-agreement guard) and
`clamp_into_window` (already-inside no-op; below/above window pulled to edge; empty `row_count`;
**`list_h == 0` is a no-op that leaves the highlight unchanged and does not underflow/panic**). Pins
I3.1, I3.2 and the degenerate-geometry robustness guard. No editor fixture.

**T2 — the four side-effect-free list slots** (`palette`, `file_browser`, `outline`, `diag`), in
`mouse.rs` `#[cfg(test)]`. Per slot: (a) a `Moved` over a list row sets the highlight to it (I1);
(b) a `Moved` off-rect (bottom-left, or a border column) leaves a keyboard-set highlight unchanged
(I2); (c) a wheel on an overflowing list slides `scroll_top` by 3 and the re-hover pins the highlight
to the pointer row (I3); (d) a wheel on a short list (all rows visible) steps the highlight ±1 (I4);
(e) **outline hover does NOT jump** (`outline_overlay::outline_jump_to` not reached — assert the
document view/scroll is unchanged and the overlay stays open) and **diag hover does NOT apply**
(buffer version unchanged); (f) **empty-list wheel is a total no-op — verifies I3b in full** — with a filtered-to-zero row set
(`row_count == 0`, e.g. palette with a no-match query), a wheel event (up AND down) leaves the
overlay state **byte-identical** before/after: `selected` unchanged AND `scroll_top` unchanged (the
observable proxy for no step / no scroll / no clamp / no re-hover), with no underflow/panic. Pins
I1–I5, I3b, I11 for these four.

**T3 — menu** (`mouse_menu`). (a) dropdown hover sets `highlighted` to the pointer row within the
dropdown rect (I1) using the menu's effective item budget (Wrinkle 1); (b) wheel on a tall category
slides `scroll_top` by 3 via the menu's overflow-adjusted `list_h` and re-hovers, and on a short
category (fills window, no overflow) steps ±1 (I3/I4); (c) **hover-to-switch**: with a dropdown open,
a `Moved` onto a different bar label switches `open` to that category and resets
`highlighted`/`scroll_top` to 0 (I6, reset triple); (d) `cat != m.open` dedupe — a `Moved` within the
*same* open label does not reset the window (I5/I6); (e) first-open deliberateness — a `Moved` onto
the bar with **no menu open** does not open a menu and does not arm `menu_reveal_due` when armed via
the dwell path is separately covered (I6/I7); (f) off-menu hover does not close an open menu (I6);
(g) **indicator-row safety** — on an *overflowing* category (build a synthetic tall group à la
`chrome_geom`'s `tall_menu_groups` test helper, 20 leaves in a short frame so `n > raw_window`),
after a wheel scroll to the tail AND a hover at the geometric indicator row, `highlighted` is never
equal to a hidden index and is always within `[scroll_top, scroll_top + effective_budget)` — i.e. the
highlight never lands on the reserved `n/total` row (Wrinkle 1; mirrors
`dropdown_indicator_row_hit_test_returns_none`). Pins I6 + the effective-budget fix.

**T4 — preview overlays** (`theme_picker`, `cursor_picker`). (a) hover over a row fires the preview
funnel (theme applied / caret shape changed) once (I8); (b) the wheel short-list step fires preview
too (I4+I8); (c) **restore funnels intact** — after a hover *sweep* across several rows, Esc /
click-away restores `tp.original` (theme reverts) / `original_shape`+`original_blink` (caret reverts)
(I8); (d) **dedupe mutation-guardrail** — a repeated `Moved` at the *same* row fires the preview
exactly once, and a sweep across N distinct rows fires it exactly N times (I5). The dedupe test must
be written so that removing the `hit != Some(cur)` guard makes it fail (mutation-test the
completeness assertion — the H21 lesson: assert the *count*, not just "≥1"); (e) **empty-list wheel
is a total no-op — verifies I3b in full** — on a `theme_picker` filtered to zero rows
(`row_count == 0`), a wheel event (up AND down) leaves state byte-identical: `selected` unchanged AND
`scroll_top` unchanged (proxy for no step / no scroll / no clamp / no re-hover) AND **zero preview
funnel calls** (the preview-overlay-specific proxy for no re-hover), with no underflow/panic. Pins
I3b for the preview overlays.

**T5 — cross-cutting completeness** (`overlays.rs` `#[cfg(test)]`). Extend the existing
`every_overlay_is_active_xor_and_consumes_key_and_click` sweep (or add a sibling) with a **`Moved`
leg**: for each of the 11 `OverlayId`s, open it, call its own `row().mouse` slot directly with a
`MouseEventKind::Moved` at a text-band cell, and assert (i) no panic and (ii) the document buffer
version is unchanged (I11 — hover/move never loses data, for all slots including the four OUT ones and
splash). Plus an in-process **e2e journey** in `e2e.rs` (it already has mouse-step helpers driving the
real `reduce → advance → render` loop against `TestBackend`): open the palette (or theme picker),
hover a row (highlight moves), wheel down (window scrolls + highlight follows the pointer), click
(row dispatches / overlay closes) — the full hover→wheel→click path through the real message loop.

**Gate mapping:** all of `cargo test` (core + shell), `cargo clippy --workspace --all-targets`
(deny), `too_many_lines` (100) — the per-slot arms stay small; if a slot's combined
`Moved`+`Scroll`+`Down` body crosses 100 lines, extract the wheel/hover bodies into slot-local
helpers rather than adding a blanket allow — and `module_budgets` for `mouse.rs`. PTY smoke suite
run + summary quoted in the pre-merge report (advisory).

---

## 7. In-passing normalization (fold in only if clean)

The wheel arms of `mouse_theme_picker`, `mouse_cursor_picker`, and `mouse_file_browser` lack the
early `return` that `mouse_palette`/`mouse_menu`/`mouse_outline`/`mouse_diag` have after handling a
scroll — in the three exceptions the `Down(Left)` block is simply skipped because the event is a
scroll, so it is harmless today, but it is an inconsistency. Since every wheel arm is being rewritten
for I3/I4 anyway, normalize all seven to the same shape (scroll handled → `return`) as a natural part
of the rewrite. This is zero added scope (the arms are already being touched); it is **not** a
separate task and must not grow into one. The theme/cursor "preview fires unconditionally even at the
clamp boundary" redundancy is likewise removed for free by I5's dedupe guard.

---

## 8. Risks / edges (once-over)

- **Clamp-before-hit-test (Wrinkle 3)** — the single ordering constraint; I3 states it (step 1 before
  step 3) and `wheel_scroll` enforces the bound. T1 + T2(c) pin it.
- **Menu empty-groups window** — a bar-click opens `menu::empty_at(order_idx)` (empty `groups`,
  `built: false`); `app::hydrate_overlays` builds groups the same reduce cycle before any later
  `Moved`. In the empty window, `menu_bar_layout` over `[]` returns `[]` → `None` → no-op. No panic.
- **`ROW_ACTIONS` fixed 7 vs terminal height** — `cursor_picker_row_at` sizes its rect `n+1` but its
  visible `list_h` still equals `list_h_for(n, area_h)` (the `+1`/`+3`/`-3` terms cancel — documented
  in `chrome_geom`); the wheel helper is fed the same `list_h`, so short-list detection
  (`row_count <= list_h`) is correct at all heights.
- **`diag` `+2` synthetic rows** — "ignore once" / "add to dictionary" are ordinary selectable rows
  under `row_count()`; hover highlights them fine, apply stays `Down`-only.
- **Drag-through (I9)** and **no-motion terminals (I10)** — stated decisions, no code, no risk.

---

## 9. Verdict

The model is sound and fully grounded: event delivery is already in place (§1.1), the wheel/painter
conflict is resolved by Decision 1(a) without touching the shipped two-layer invariant (§2.1), the
preview embrace rides the existing preview cost class (§4), and the shared surface is one pure
name-agnostic helper pair plus per-slot hover arms (§5), matching the `apply_list_nav` precedent. The
command surface is untouched (N/A). Five TDD tasks with the completeness sweep gaining a `Moved` leg
and a mutation-guarded dedupe test (§6). Ready for the Codex spec gate.
