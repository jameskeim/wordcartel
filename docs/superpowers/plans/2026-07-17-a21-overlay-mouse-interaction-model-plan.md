# A21 — Overlay Mouse Interaction Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the mouse a single coherent meaning over an open list overlay — hover highlights the row under the pointer, the wheel scrolls the viewport (dragging the highlight to the pointer) or steps a short list, and the menu bar switches dropdowns on hover — while the keyboard path, the command surface, and every no-data-loss / hot-path invariant stay untouched.

**Architecture:** Shell-only. Two pure windowing helpers land in `wordcartel/src/list_window.rs`; each of the seven overlay `mouse` slots in `wordcartel/src/mouse.rs` gains a `Moved` (hover) arm and a rewritten wheel arm that calls the helpers. No `wordcartel-core` change, no plumbing change — a bare `Moved` already reaches the slots via `mouse::handle`'s `route_overlay` early-return (verified: `app::reduce_dispatch` intercepts all `Pass` mouse events except `splash::intercept`'s `Down` arm).

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.28.1. Hand-formatted dense house style (NEVER `cargo fmt`). TDD with `cargo test -p wordcartel`.

## Global Constraints

Every task's requirements implicitly include this section. Values copied verbatim from the spec (`docs/superpowers/specs/2026-07-17-a21-overlay-mouse-interaction-model-design.md`).

- **Ruling 1(a) — wheel drags the highlight to the window edge.** On a wheel over an overflowing list: slide `scroll_top` by `WHEEL_STEP` (clamped), then pull the highlight into the new window, then re-hover at the pointer (which overrides). No painter change; the shipped two-layer `keep_visible` invariant stays.
- **Ruling 2(ii) — short-list wheel steps ±1.** When `row_count <= list_h` (nothing to scroll), the wheel moves the highlight ±1 like an arrow key. Composed rule: the wheel always moves you *through* the list — scrolls when long, steps when short.
- **Ruling 3(A) — embrace hover-preview** on `theme_picker` + `cursor_picker`: hover (and the wheel's re-hover / short-step) fires the SAME preview funnel the keyboard/wheel already fire; restore-on-Esc/click-away funnels unchanged.
- **I3b — empty-list wheel is a TOTAL no-op** (`row_count == 0`): no step, no scroll, no clamp, no re-hover, no preview. Guard at the head of every wheel arm.
- **H7 saturating arithmetic throughout (forbid-unsafe spirit; shell stays clean).** EVERY add/sub in the helpers and the wheel arms saturates. `wheel_scroll`: `max_top = if list_h == 0 { 0 } else { row_count.saturating_sub(list_h) }`, down uses `scroll_top.saturating_add(WHEEL_STEP)`. `clamp_into_window`: no-op when `row_count == 0 || list_h == 0`; `list_h - 1` only reached after that guard; `scroll_top.saturating_add(list_h - 1)`. `wheel_list` short-step uses `selected.saturating_add(1).min(row_count.saturating_sub(1))` / `selected.saturating_sub(1)` — never a bare `+ 1` or `n - 1`.
- **Menu effective item-budget.** The value the menu passes as `list_h` is `if n > raw_window { raw_window - 1 } else { raw_window }` where `raw_window = n.min(15).min(menu_area(area).height.saturating_sub(1) as usize)` — mirrors `paint_menu_dropdown`'s `keep_h` and `menu_dropdown_row_at`'s `item_rows`, so wheel state agrees with painter + hit-tester and the highlight never lands on the reserved indicator row. The palette family passes `list_window::list_h_for(n, area_h)` (no reservation — its "n/total" is a border title, not a list row).
- **Dedupe on row-change (hot-path law, theme-R) — at BOTH the hover AND the wheel path.** SELECTION-derived side effects (the highlight write, the window-follows-selection `keep_overlay_visible`/`keep_visible`, and the preview funnel) fire ONLY when the resolved row differs from the row before the gesture. Hover is expressed as `if hit != Some(cur) { … }`; each wheel arm captures `before = selected`, lets `wheel_list` move `scroll_top` (the viewport genuinely scrolls every notch) and possibly `selected`, re-hovers, then fires the selection-derived side effects inside a single `if after != before { … }` guard. This prevents a clamp-boundary notch (where `selected` did not move) from re-deriving `scroll_top` FROM the selection and fighting the wheel, or re-firing the preview. The guard is visible at all seven slots.
- **Off-rect hover leaves the highlight as-is.** When the hit-tester returns `None`, hover does nothing — a one-line `if let Some(idx) = hit`.
- **Menu-bar hover-to-switch [option ii]:** while a dropdown is already open, a `Moved` onto a *different* bar category switches it live with the **reset triple** `open = cat; highlighted = 0; scroll_top = 0` (identical to `menu::intercept`'s ←/→ arms and the existing `Down` bar arm), guarded by `cat != m.open`. **First-open stays deliberate** (no auto-open on bar hover with no menu up). **Off-menu hover does not close.**
- **The highlight field naming split:** six overlays name it `selected`; the **menu** names it `highlighted`. Helpers are name-agnostic (`&mut usize`).
- **`menu_dropdown_row_at` takes `scroll_top` as a parameter** (unlike the palette-family testers, which read it off the borrowed struct); it also reserves the indicator row.
- **`Drag` is not `Moved`** — hover does not track mid-drag (unchanged; slots ignore `Drag`). **No-motion terminals** degrade gracefully (no `Moved` arrives; wheel re-hover still works from the wheel event's own coords).
- **Command-surface contract: N/A — does not touch the command surface.** No command added/removed; registry, palette contents, menu structure (`menu::grouped_commands` / `registry::MENU_ORDER`), and keybinding hints untouched. Hover-switch re-targets `MenuView::open` only.
- **House rules (merge GATEs):** `cargo test` green (core lib + oracle, shell lib); `cargo clippy --workspace --all-targets` clean under `deny`; `clippy::too_many_lines` threshold 100 per fn; `module_budgets` for `mouse.rs`; NEVER run `cargo fmt` (hand-format, match neighbors); no `.unwrap()` on new fallible paths (guarded `.unwrap()` on an established `is_some` invariant is acceptable, mirroring the existing `Down` arms); em-dash `—` in prose comments.

---

## File structure

- `wordcartel/src/list_window.rs` — add `WHEEL_STEP`, `wheel_scroll`, `clamp_into_window`, `wheel_list` (pure windowing helpers) + unit tests. **T1.**
- `wordcartel/src/mouse.rs` — rewrite the seven overlay slots' arms (`mouse_palette`, `mouse_file_browser`, `mouse_outline`, `mouse_diag` — **T2**; `mouse_menu` — **T3**; `mouse_theme_picker`, `mouse_cursor_picker` — **T4**) + per-slot tests. Down arms unchanged.
- `wordcartel/src/overlays.rs` — add a `Moved`-leg completeness sweep test. **T5.**
- `wordcartel/src/e2e.rs` — add a `mouse_wheel` harness helper + a hover→wheel→click journey test. **T5.**

**Anchor by symbol name, not line number** — the `:NNN` in the spec/plan drift as tasks edit files. Re-locate with `grep`/`rg` on the fn name.

---

### Task 1: Pure windowing helpers in `list_window.rs`

**Files:**
- Modify: `wordcartel/src/list_window.rs` (add four items after the existing `keep_visible` fn; add tests in the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing (pure; no `Editor`).
- Produces (all `pub(crate)`, all name-agnostic via `&mut usize`):
  - `const WHEEL_STEP: usize` (= 3)
  - `fn wheel_scroll(down: bool, row_count: usize, list_h: usize, scroll_top: &mut usize)`
  - `fn clamp_into_window(highlight: &mut usize, scroll_top: usize, list_h: usize, row_count: usize)`
  - `fn wheel_list(down: bool, row_count: usize, list_h: usize, selected: &mut usize, scroll_top: &mut usize) -> bool` — returns `true` iff it took the SCROLL path (caller then re-hovers). Realizes spec §5's branch `if row_count <= list_h { step ±1 } else { wheel_scroll; clamp_into_window }`, factored to avoid repeating it across seven slots.

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` block in `wordcartel/src/list_window.rs`:

```rust
    #[test]
    fn wheel_scroll_slides_by_step_and_clamps() {
        let mut top = 0;
        wheel_scroll(true, 100, 10, &mut top);
        assert_eq!(top, 3, "one notch down slides by WHEEL_STEP");
        wheel_scroll(true, 100, 10, &mut top);
        assert_eq!(top, 6, "second notch accumulates");
        // clamp at the tail: max_top = 100 - 10 = 90.
        let mut top = 89;
        wheel_scroll(true, 100, 10, &mut top);
        assert_eq!(top, 90, "clamped to row_count - list_h, not 92");
        // up saturates at 0.
        let mut top = 2;
        wheel_scroll(false, 100, 10, &mut top);
        assert_eq!(top, 0, "up saturates at 0 (2 - 3)");
    }

    #[test]
    fn wheel_scroll_list_h_zero_pins_to_zero() {
        let mut top = 7;
        wheel_scroll(true, 5, 0, &mut top);
        assert_eq!(top, 0, "list_h == 0 → max_top 0 → scroll_top 0 (mirrors keep_visible), down");
        let mut top = 7;
        wheel_scroll(false, 5, 0, &mut top);
        assert_eq!(top, 0, "list_h == 0 → 0, up (saturating)");
    }

    #[test]
    fn clamp_into_window_pulls_highlight_to_edge() {
        // window [3, 3+10) = [3,13); a highlight of 1 is below → pulled to 3.
        let mut h = 1;
        clamp_into_window(&mut h, 3, 10, 100);
        assert_eq!(h, 3, "below window → lower edge");
        // a highlight of 50 is above [3,13) → pulled to 12.
        let mut h = 50;
        clamp_into_window(&mut h, 3, 10, 100);
        assert_eq!(h, 12, "above window → upper edge (scroll_top + list_h - 1)");
        // already inside → unchanged.
        let mut h = 7;
        clamp_into_window(&mut h, 3, 10, 100);
        assert_eq!(h, 7, "inside window → no move");
    }

    #[test]
    fn clamp_into_window_degenerate_is_noop_no_underflow() {
        let mut h = 4;
        clamp_into_window(&mut h, 0, 0, 10);
        assert_eq!(h, 4, "list_h == 0 → no-op (empty window; no underflow)");
        let mut h = 4;
        clamp_into_window(&mut h, 0, 5, 0);
        assert_eq!(h, 4, "row_count == 0 → no-op");
    }

    #[test]
    fn wheel_list_short_steps_and_long_scrolls() {
        // short list (row_count <= list_h): steps ±1, returns false (no re-hover).
        let (mut sel, mut top) = (0, 0);
        let scrolled = wheel_list(true, 5, 10, &mut sel, &mut top);
        assert!(!scrolled, "short list does not scroll");
        assert_eq!((sel, top), (1, 0), "short list steps selection down by 1");
        let scrolled = wheel_list(false, 5, 10, &mut sel, &mut top);
        assert!(!scrolled, "short list up does not scroll");
        assert_eq!((sel, top), (0, 0), "short list steps back up");
        // long list (row_count > list_h): scrolls + drags highlight, returns true.
        let (mut sel, mut top) = (0, 0);
        let scrolled = wheel_list(true, 100, 10, &mut sel, &mut top);
        assert!(scrolled, "long list scrolls");
        assert_eq!(top, 3, "scrolled by WHEEL_STEP");
        assert_eq!(sel, 3, "highlight dragged to the window's lower edge");
    }

    #[test]
    fn wheel_list_empty_is_total_noop() {
        let (mut sel, mut top) = (0, 0);
        let scrolled = wheel_list(true, 0, 0, &mut sel, &mut top);
        assert!(!scrolled, "empty list never scrolls");
        assert_eq!((sel, top), (0, 0), "empty list is a total no-op, no underflow");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p wordcartel --lib list_window:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function 'wheel_scroll'` / `'clamp_into_window'` / `'wheel_list'` in this scope (not yet defined).

- [ ] **Step 3: Write the implementation**

Insert after the existing `keep_visible` fn (before `pub(crate) enum ListNav`) in `wordcartel/src/list_window.rs`:

```rust
/// Wheel step: rows the viewport slides per notch, uniform across overlays (A21).
pub(crate) const WHEEL_STEP: usize = 3;

/// Wheel notch → viewport scroll. `list_h` is the caller's effective ITEM-ROW budget (the menu
/// passes its overflow-adjusted value, not the raw dropdown height). Slide `scroll_top` by
/// ±WHEEL_STEP, clamped to `[0, max_top]` where `max_top == 0` when `list_h == 0` (nothing
/// renders) and `row_count.saturating_sub(list_h)` otherwise — the SAME bound `keep_visible`
/// enforces, including its `list_h == 0 → scroll_top = 0` reset, so wheel state never diverges
/// from the per-frame painter at any window height. Selection is untouched; the caller drags the
/// highlight in via `clamp_into_window`, then re-hovers. All-saturating: no height can panic.
pub(crate) fn wheel_scroll(down: bool, row_count: usize, list_h: usize, scroll_top: &mut usize) {
    let max_top = if list_h == 0 { 0 } else { row_count.saturating_sub(list_h) };
    *scroll_top = if down { scroll_top.saturating_add(WHEEL_STEP).min(max_top) }
                  else { scroll_top.saturating_sub(WHEEL_STEP) };
}

/// Pull `highlight` into the visible item window `[scroll_top, scroll_top + list_h)` after a
/// wheel scroll (ruling 1a) — an active wheel gesture drags the highlight to the window edge;
/// a no-op when it is already inside. `list_h` is the effective item budget (menu =
/// overflow-adjusted), so the highlight can never land on the menu's reserved indicator row.
/// `row_count == 0` or `list_h == 0` (empty window, nothing renders): leave `highlight`
/// untouched — the position is moot until a real window exists, and this avoids the `list_h - 1`
/// underflow. All arithmetic saturating.
pub(crate) fn clamp_into_window(highlight: &mut usize, scroll_top: usize, list_h: usize, row_count: usize) {
    if row_count == 0 || list_h == 0 { return; }
    let last = row_count - 1;
    let lo = scroll_top.min(last);
    let hi = scroll_top.saturating_add(list_h - 1).min(last);
    *highlight = (*highlight).clamp(lo, hi);
}

/// One wheel notch over a windowed list — the spec §5 branch, factored so the seven overlay
/// slots do not each repeat it. Empty list (`row_count == 0`) is a total no-op (returns false).
/// Short list (`row_count <= list_h`, nothing to scroll) steps `selected` ±1 (returns false).
/// Long list scrolls the viewport by `WHEEL_STEP` then drags `selected` into the new window
/// (returns `true` — the caller then re-hovers at the pointer, which overrides). `list_h` is the
/// caller's effective item budget; name-agnostic via `&mut usize` (the menu passes `highlighted`,
/// the others `selected`). Pure — the caller owns keep-visible and any per-overlay side effect.
pub(crate) fn wheel_list(down: bool, row_count: usize, list_h: usize,
    selected: &mut usize, scroll_top: &mut usize) -> bool {
    if row_count == 0 { return false; }
    if row_count <= list_h {
        *selected = if down { selected.saturating_add(1).min(row_count.saturating_sub(1)) }
                    else { selected.saturating_sub(1) };
        false
    } else {
        wheel_scroll(down, row_count, list_h, scroll_top);
        clamp_into_window(selected, *scroll_top, list_h, row_count);
        true
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p wordcartel --lib list_window:: 2>&1 | tail -20`
Expected: PASS (all list_window tests, old + new).

- [ ] **Step 5: Clippy the crate**

Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -20`
Expected: no new warnings.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/list_window.rs
git commit -m "feat(a21): pure wheel/window helpers (wheel_scroll, clamp_into_window, wheel_list)"
```

---

### Task 2: Hover + wheel on the four side-effect-free slots

`mouse_palette`, `mouse_file_browser`, `mouse_outline`, `mouse_diag`. All four take the SAME shape — a `Moved` hover arm and a rewritten wheel arm — differing only in the state field, the row-count expression, and the hit-tester. The `Down(Left)` arms are UNCHANGED (do not touch them). Highlight field is `selected` for all four.

**Files:**
- Modify: `wordcartel/src/mouse.rs` — fns `mouse_palette`, `mouse_file_browser`, `mouse_outline`, `mouse_diag`
- Test: `wordcartel/src/mouse.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `list_window::{wheel_list, WHEEL_STEP}` (T1); `list_window::list_h_for`; `app::keep_overlay_visible`; the existing `chrome_geom` hit-testers `palette_row_at`, `file_browser_row_at`, `outline_row_at`, `diag_row_at` (all `(area, &State, col, row) -> Option<usize>`).
- Produces: nothing new (behavior on the existing slots).

Per-slot map (used below):

| slot | state | row-count `n` | hit-tester |
|---|---|---|---|
| `mouse_palette` | `editor.palette` | `p.rows.len()` | `palette_row_at(area, p, ev.column, ev.row)` |
| `mouse_file_browser` | `editor.file_browser` | `fb.entries.len()` | `file_browser_row_at(area, fb, ev.column, ev.row)` |
| `mouse_outline` | `editor.outline` | `o.rows.len()` | `outline_row_at(area, o, ev.column, ev.row)` |
| `mouse_diag` | `editor.diag` | `d.row_count()` | `diag_row_at(area, d, ev.column, ev.row)` |

- [ ] **Step 1: Write the failing tests**

Add to `#[cfg(test)] mod tests` in `wordcartel/src/mouse.rs`. These use the existing `ctx()`/`down()` helpers already in that module; add a `moved()`/`wheel()` local helper alongside them.

```rust
    fn moved(col: u16, row: u16) -> MouseEvent {
        MouseEvent { kind: MouseEventKind::Moved, column: col, row, modifiers: KeyModifiers::NONE }
    }
    fn wheel_ev(down_dir: bool, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: if down_dir { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp },
            column: col, row, modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn palette_hover_moves_highlight_to_pointer_row() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.palette = Some(crate::palette::Palette::default());
        let (reg, ex, clk, tx, km) = ctx();
        crate::app::hydrate_overlays(&mut e, &reg, &km);
        assert_eq!(e.palette.as_ref().unwrap().scroll_top, 0);
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, e.palette.as_ref().unwrap().rows.len());
        // Hover the 4th visible list row (list starts at rect.y + 2).
        handle(&mut e, moved(rect.x + 1, rect.y + 2 + 3), &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.palette.as_ref().unwrap().selected, 3, "hover set highlight to the pointer row");
    }

    #[test]
    fn palette_hover_off_rect_leaves_highlight() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.palette = Some(crate::palette::Palette::default());
        let (reg, ex, clk, tx, km) = ctx();
        crate::app::hydrate_overlays(&mut e, &reg, &km);
        e.palette.as_mut().unwrap().selected = 2; // a keyboard-set highlight
        handle(&mut e, moved(0, 0), &reg, &km, &ex, &clk, &tx); // top-left, off the overlay
        assert_eq!(e.palette.as_ref().unwrap().selected, 2, "off-rect hover leaves the highlight as-is");
    }

    #[test]
    fn palette_wheel_scrolls_and_re_hovers() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &crate::registry::Registry::builtins(),
            &{ let (_r, _e2, _c, _t, km) = ctx(); km });
        e.palette = Some(p);
        let (reg, ex, clk, tx, km) = ctx();
        let n = e.palette.as_ref().unwrap().rows.len();
        let list_h = crate::list_window::list_h_for(n, 24);
        assert!(n > list_h, "precondition: palette overflows its window");
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, n);
        // Wheel down with the pointer over the top visible row → scroll by 3, re-hover pins the
        // highlight to that top row (absolute row scroll_top).
        handle(&mut e, wheel_ev(true, rect.x + 1, rect.y + 2), &reg, &km, &ex, &clk, &tx);
        let p = e.palette.as_ref().unwrap();
        assert_eq!(p.scroll_top, 3, "wheel scrolled the viewport by WHEEL_STEP");
        assert_eq!(p.selected, p.scroll_top, "re-hover pinned the highlight to the pointer's top row");
    }

    #[test]
    fn palette_wheel_empty_list_is_total_noop() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let mut p = crate::palette::Palette::default();
        p.query = "zzz_no_such_command_zzz".into(); // filter to zero rows
        crate::palette::rebuild_rows(&mut p, &crate::registry::Registry::builtins(),
            &{ let (_r, _e2, _c, _t, km) = ctx(); km });
        assert!(p.rows.is_empty(), "precondition: zero rows");
        p.scroll_top = 0; p.selected = 0;
        e.palette = Some(p);
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, wheel_ev(true, 40, 12), &reg, &km, &ex, &clk, &tx);
        handle(&mut e, wheel_ev(false, 40, 12), &reg, &km, &ex, &clk, &tx);
        let p = e.palette.as_ref().unwrap();
        assert_eq!((p.selected, p.scroll_top), (0, 0), "empty-list wheel is a total no-op (I3b)");
    }

    #[test]
    fn outline_hover_does_not_jump() {
        let doc = "# A\n\ntext\n\n# B\n\nmore\n";
        let mut e = Editor::new_from_text(doc, None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_outline();
        let (reg, ex, clk, tx, km) = ctx();
        let scroll_before = e.active().view.scroll;
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let n = e.outline.as_ref().unwrap().rows.len();
        assert!(n >= 2, "precondition: two headings");
        let rect = crate::chrome_geom::palette_overlay_rect(area, n);
        handle(&mut e, moved(rect.x + 1, rect.y + 2 + 1), &reg, &km, &ex, &clk, &tx);
        assert!(e.outline.is_some(), "hover keeps the outline open (no jump)");
        assert_eq!(e.outline.as_ref().unwrap().selected, 1, "hover moved the highlight");
        assert_eq!(e.active().view.scroll, scroll_before, "hover did NOT jump the document");
    }

    #[test]
    fn diag_hover_does_not_apply() {
        let mut e = Editor::new_from_text("helo world\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let d = wordcartel_core::diagnostics::Diagnostic {
            range: 0..4, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "spelling".into(), suggestions: vec!["hello".into()],
        };
        e.open_diag(d);
        let (reg, ex, clk, tx, km) = ctx();
        let v0 = e.active().document.version;
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let n = e.diag.as_ref().unwrap().row_count();
        let rect = crate::chrome_geom::palette_overlay_rect(area, n);
        // diag list starts at rect.y + 1 (no query row).
        handle(&mut e, moved(rect.x + 1, rect.y + 1 + 1), &reg, &km, &ex, &clk, &tx);
        assert!(e.diag.is_some(), "hover keeps the diag overlay open (no apply)");
        assert_eq!(e.active().document.version, v0, "hover did NOT apply a fix (buffer unchanged)");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p wordcartel --lib mouse::tests::palette_hover 2>&1 | tail -20`
Expected: FAIL — the hover arm does not exist yet, so `selected` stays 0 (or the outline/diag hover assertions fail).

- [ ] **Step 3: Rewrite the four slots**

In `wordcartel/src/mouse.rs`, replace the wheel-only body of `mouse_palette` (keep the `Down(Left)` arm verbatim). The new `mouse_palette` reads:

```rust
pub(crate) fn mouse_palette(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    ctx: &crate::overlays::DispatchCtx) {
    // Hover: move the highlight to the row under the pointer (dedupe: only on row change; I2:
    // off-rect leaves it as-is because the hit-tester returns None).
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.palette.as_ref()
            .and_then(|p| crate::chrome_geom::palette_row_at(area, p, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            if let Some(p) = editor.palette.as_mut() {
                if p.selected != idx {
                    p.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, p.rows.len(), &mut p.scroll_top);
                }
            }
        }
        return;
    }
    // Wheel: the viewport scrolls every notch (wheel_list moves scroll_top); the SELECTION-derived
    // side effect (window-follows-selection) fires ONLY when the row actually changes (I5 dedupe).
    // Empty list is a total no-op (I3b).
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.palette.as_ref() { Some(p) => p.selected, None => return };
        let scrolled = if let Some(p) = editor.palette.as_mut() {
            let n = p.rows.len();
            if n == 0 { return; } // I3b: empty list is a total no-op
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut p.selected, &mut p.scroll_top)
        } else { return };
        if scrolled {
            // Re-hover: the pointer is stationary, so pin the highlight to its row (ruling 1a).
            if let Some(idx) = editor.palette.as_ref()
                .and_then(|p| crate::chrome_geom::palette_row_at(area, p, ev.column, ev.row))
            {
                if let Some(p) = editor.palette.as_mut() { p.selected = idx; }
            }
        }
        // I5 dedupe: re-window from the selection ONLY when the row moved. Skips the redundant
        // re-derive at a clamp boundary that would re-compute scroll_top FROM selection and fight
        // the wheel. In the scroll path `after` is already in-window, so keep_overlay_visible is a
        // no-op on scroll_top; in the short-step path it pins scroll_top for the fully-visible list.
        let after = editor.palette.as_ref().map(|p| p.selected).unwrap_or(before);
        if after != before {
            let n = editor.palette.as_ref().map(|p| p.rows.len()).unwrap_or(0);
            if let Some(p) = editor.palette.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut p.scroll_top);
            }
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // ⟨UNCHANGED — the existing Down(Left) arm body verbatim⟩
    }
}
```

Apply the identical transformation to `mouse_file_browser`, `mouse_outline`, `mouse_diag`, substituting per the table above (`editor.file_browser`/`fb`/`fb.entries.len()`/`file_browser_row_at`; `editor.outline`/`o`/`o.rows.len()`/`outline_row_at`; `editor.diag`/`d`/`d.row_count()`/`diag_row_at`). Keep each slot's `Down(Left)` arm verbatim. The full `mouse_file_browser`:

```rust
pub(crate) fn mouse_file_browser(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    _ctx: &crate::overlays::DispatchCtx) {
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.file_browser.as_ref()
            .and_then(|fb| crate::chrome_geom::file_browser_row_at(area, fb, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            if let Some(fb) = editor.file_browser.as_mut() {
                if fb.selected != idx {
                    fb.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, fb.entries.len(), &mut fb.scroll_top);
                }
            }
        }
        return;
    }
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.file_browser.as_ref() { Some(fb) => fb.selected, None => return };
        let scrolled = if let Some(fb) = editor.file_browser.as_mut() {
            let n = fb.entries.len();
            if n == 0 { return; }
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut fb.selected, &mut fb.scroll_top)
        } else { return };
        if scrolled {
            if let Some(idx) = editor.file_browser.as_ref()
                .and_then(|fb| crate::chrome_geom::file_browser_row_at(area, fb, ev.column, ev.row))
            {
                if let Some(fb) = editor.file_browser.as_mut() { fb.selected = idx; }
            }
        }
        let after = editor.file_browser.as_ref().map(|fb| fb.selected).unwrap_or(before);
        if after != before {
            let n = editor.file_browser.as_ref().map(|fb| fb.entries.len()).unwrap_or(0);
            if let Some(fb) = editor.file_browser.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut fb.scroll_top);
            }
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // ⟨UNCHANGED — the existing Down(Left) arm body verbatim⟩
    }
}
```

The full `mouse_outline`:

```rust
pub(crate) fn mouse_outline(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    _ctx: &crate::overlays::DispatchCtx) {
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.outline.as_ref()
            .and_then(|o| crate::chrome_geom::outline_row_at(area, o, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            if let Some(o) = editor.outline.as_mut() {
                if o.selected != idx {
                    o.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, o.rows.len(), &mut o.scroll_top);
                }
            }
        }
        return;
    }
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.outline.as_ref() { Some(o) => o.selected, None => return };
        let scrolled = if let Some(o) = editor.outline.as_mut() {
            let n = o.rows.len();
            if n == 0 { return; }
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut o.selected, &mut o.scroll_top)
        } else { return };
        if scrolled {
            if let Some(idx) = editor.outline.as_ref()
                .and_then(|o| crate::chrome_geom::outline_row_at(area, o, ev.column, ev.row))
            {
                if let Some(o) = editor.outline.as_mut() { o.selected = idx; }
            }
        }
        let after = editor.outline.as_ref().map(|o| o.selected).unwrap_or(before);
        if after != before {
            let n = editor.outline.as_ref().map(|o| o.rows.len()).unwrap_or(0);
            if let Some(o) = editor.outline.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut o.scroll_top);
            }
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // ⟨UNCHANGED — the existing Down(Left) arm body verbatim, incl. the stale-version guard⟩
    }
}
```

The full `mouse_diag`:

```rust
pub(crate) fn mouse_diag(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    ctx: &crate::overlays::DispatchCtx) {
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.diag.as_ref()
            .and_then(|d| crate::chrome_geom::diag_row_at(area, d, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            if let Some(d) = editor.diag.as_mut() {
                let rc = d.row_count();
                if d.selected != idx {
                    d.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, rc, &mut d.scroll_top);
                }
            }
        }
        return;
    }
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.diag.as_ref() { Some(d) => d.selected, None => return };
        let scrolled = if let Some(d) = editor.diag.as_mut() {
            let n = d.row_count();
            if n == 0 { return; }
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut d.selected, &mut d.scroll_top)
        } else { return };
        if scrolled {
            if let Some(idx) = editor.diag.as_ref()
                .and_then(|d| crate::chrome_geom::diag_row_at(area, d, ev.column, ev.row))
            {
                if let Some(d) = editor.diag.as_mut() { d.selected = idx; }
            }
        }
        let after = editor.diag.as_ref().map(|d| d.selected).unwrap_or(before);
        if after != before {
            let n = editor.diag.as_ref().map(|d| d.row_count()).unwrap_or(0);
            if let Some(d) = editor.diag.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut d.scroll_top);
            }
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // ⟨UNCHANGED — the existing Down(Left) arm body verbatim⟩
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p wordcartel --lib mouse:: 2>&1 | tail -25`
Expected: PASS (new hover/wheel tests + all pre-existing `mouse::tests`, incl. the `Down`-arm tests, still green).

- [ ] **Step 5: Clippy + too_many_lines check**

Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -25`
Expected: no new warnings. If `clippy::too_many_lines` fires on any slot (threshold 100), extract that slot's wheel body into a slot-local `fn <slot>_wheel(editor, ev, area) -> ()` returning early — prefer the extraction over an `#[allow]`.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/mouse.rs
git commit -m "feat(a21): hover + through-list wheel on palette/file_browser/outline/diag"
```

---

### Task 3: Menu — dropdown hover, effective-budget wheel, bar hover-to-switch

**Files:**
- Modify: `wordcartel/src/mouse.rs` — fn `mouse_menu` (keep its `Down(Left)` arm verbatim)
- Test: `wordcartel/src/mouse.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `list_window::{wheel_list, keep_visible}`; `chrome_geom::{menu_area, menu_bar_layout, menu_dropdown_row_at}`; the `MenuView` fields `groups`, `open`, `highlighted`, `scroll_top`.
- Produces: nothing new.

Menu specifics (Global Constraints): highlight field is `highlighted`; `list_h` is the overflow-adjusted **effective item budget**; `menu_dropdown_row_at(hit_area, groups, open, scroll_top, col, row)` takes `scroll_top` as a parameter and reserves the indicator row; bar hover-to-switch uses the reset triple guarded by `cat != m.open`.

- [ ] **Step 1: Write the failing tests**

Add to `#[cfg(test)] mod tests` in `wordcartel/src/mouse.rs`. Build a synthetic tall menu the same way `chrome_geom`'s tests do — via a real `menu::build` on `Registry::builtins()`, then assert on category geometry from `chrome_geom::menu_bar_layout`.

```rust
    /// Helper: open a real built menu on category 0 (File), hydrated.
    fn open_menu(e: &mut Editor, reg: &crate::registry::Registry, km: &crate::keymap::KeyTrie) {
        e.menu = Some(crate::menu::empty_at(0));
        crate::app::hydrate_overlays(e, reg, km);
    }

    #[test]
    fn menu_hover_bar_switches_category_with_reset_triple() {
        let mut e = Editor::new_from_text("hi\n", None, (100, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        open_menu(&mut e, &reg, &km);
        // Move the highlight/scroll off zero so we can prove the reset.
        { let m = e.menu.as_mut().unwrap(); m.highlighted = 2; m.scroll_top = 1; }
        let open0 = e.menu.as_ref().unwrap().open;
        let area = ratatui::layout::Rect::new(0, 0, 100, 24);
        let hit_area = crate::chrome_geom::menu_area(area);
        let groups = e.menu.as_ref().unwrap().groups.clone();
        // Find a DIFFERENT category's bar label rect.
        let bar = crate::chrome_geom::menu_bar_layout(hit_area, &groups);
        let (other_cat, other_rect) = bar.iter().find(|(c, _)| *c != open0).copied()
            .expect("a second category exists");
        handle(&mut e, moved(other_rect.x, other_rect.y), &reg, &km, &ex, &clk, &tx);
        let m = e.menu.as_ref().unwrap();
        assert_eq!(m.open, other_cat, "hover onto a different bar label switched the open category");
        assert_eq!((m.highlighted, m.scroll_top), (0, 0), "switch reset the highlight + scroll (triple)");
    }

    #[test]
    fn menu_hover_same_category_does_not_reset() {
        let mut e = Editor::new_from_text("hi\n", None, (100, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        open_menu(&mut e, &reg, &km);
        { let m = e.menu.as_mut().unwrap(); m.highlighted = 2; m.scroll_top = 0; }
        let open0 = e.menu.as_ref().unwrap().open;
        let area = ratatui::layout::Rect::new(0, 0, 100, 24);
        let hit_area = crate::chrome_geom::menu_area(area);
        let groups = e.menu.as_ref().unwrap().groups.clone();
        let bar = crate::chrome_geom::menu_bar_layout(hit_area, &groups);
        let (_, own_rect) = bar.iter().find(|(c, _)| *c == open0).copied().unwrap();
        handle(&mut e, moved(own_rect.x, own_rect.y), &reg, &km, &ex, &clk, &tx);
        let m = e.menu.as_ref().unwrap();
        assert_eq!(m.open, open0, "hover on the SAME open label keeps the category");
        assert_eq!(m.highlighted, 2, "cat == open dedupe: no reset of the highlight");
    }

    #[test]
    fn menu_hover_dropdown_row_sets_highlight() {
        let mut e = Editor::new_from_text("hi\n", None, (100, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        open_menu(&mut e, &reg, &km);
        let area = ratatui::layout::Rect::new(0, 0, 100, 24);
        let hit_area = crate::chrome_geom::menu_area(area);
        let (open, scroll_top) = { let m = e.menu.as_ref().unwrap(); (m.open, m.scroll_top) };
        let groups = e.menu.as_ref().unwrap().groups.clone();
        // Only run the row assertion if the open category has ≥ 2 rows.
        if groups.get(open).map(|g| g.1.len()).unwrap_or(0) >= 2 {
            let drop = crate::chrome_geom::menu_dropdown_rect(hit_area, &groups, open).unwrap();
            handle(&mut e, moved(drop.x, drop.y + 1), &reg, &km, &ex, &clk, &tx);
            let want = crate::chrome_geom::menu_dropdown_row_at(hit_area, &groups, open, scroll_top, drop.x, drop.y + 1).unwrap();
            assert_eq!(e.menu.as_ref().unwrap().highlighted, want, "dropdown hover set highlighted to the pointer row");
        }
    }

    #[test]
    fn menu_hover_bar_with_no_menu_open_does_not_open() {
        let mut e = Editor::new_from_text("hi\n", None, (100, 24));
        crate::derive::rebuild(&mut e);
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        let (reg, ex, clk, tx, km) = ctx();
        // No overlay open → the event routes to the DWELL path, not the menu slot.
        handle(&mut e, moved(2, 0), &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_none(), "first-open stays deliberate: bar hover with no menu open does not auto-open");
    }

    /// Build a menu opened on ONE category (Edit) with `n` synthetic leaves — the mouse-test
    /// analogue of chrome_geom's `tall_menu_groups`. `built: true` so hydrate leaves it alone.
    fn tall_menu(n: usize) -> crate::menu::MenuView {
        let leaves: Vec<(String, crate::menu::MenuRowAction)> = (0..n)
            .map(|i| (format!("item{i}"),
                crate::menu::MenuRowAction::Command(crate::registry::CommandId("move_right"))))
            .collect();
        crate::menu::MenuView {
            groups: vec![(crate::registry::MenuCategory::Edit, leaves)],
            open: 0, highlighted: 0, built: true, scroll_top: 0,
        }
    }

    #[test]
    fn menu_wheel_tall_category_scrolls_without_landing_on_indicator_row() {
        // 100×8 terminal: menu_area.height = 7, raw_window = 20.min(15).min(6) = 6, overflow →
        // effective budget = 5, item_rows = 5, indicator row reserved at the dropdown bottom.
        let mut e = Editor::new_from_text("hi\n", None, (100, 8));
        crate::derive::rebuild(&mut e);
        e.menu = Some(tall_menu(20));
        let (reg, ex, clk, tx, km) = ctx();
        let area = ratatui::layout::Rect::new(0, 0, 100, 8);
        let hit_area = crate::chrome_geom::menu_area(area);
        let groups = e.menu.as_ref().unwrap().groups.clone();
        let drop = crate::chrome_geom::menu_dropdown_rect(hit_area, &groups, 0).expect("dropdown rect");
        assert_eq!(drop.height, 6, "raw dropdown window is 6 (min(20,15,6))");
        let item_rows = drop.height as usize - 1; // = 5 (overflow reserves the indicator row)
        // Wheel down several notches with the pointer OFF the dropdown (re-hover finds nothing;
        // the highlight is driven by the wheel's clamp — the fragile path).
        for _ in 0..3 { handle(&mut e, wheel_ev(true, 0, 7), &reg, &km, &ex, &clk, &tx); }
        let (st, hl) = { let m = e.menu.as_ref().unwrap(); (m.scroll_top, m.highlighted) };
        assert!(st > 0, "tall category scrolled the dropdown viewport");
        assert!(hl >= st && hl < st + item_rows,
            "highlight stays within the item window [{st}, {}), never the reserved indicator row", st + item_rows);
        // Ground it against the REAL hit-tester: the indicator row is not a dispatchable item.
        let indicator_row = drop.y + drop.height - 1;
        assert_eq!(
            crate::chrome_geom::menu_dropdown_row_at(hit_area, &groups, 0, st, drop.x, indicator_row),
            None, "the reserved indicator row returns None (never a hidden dispatch)");
    }

    #[test]
    fn menu_wheel_short_category_steps_by_one() {
        // A 3-leaf category on a tall terminal fits entirely (no overflow) → wheel STEPS ±1 (2ii).
        let mut e = Editor::new_from_text("hi\n", None, (100, 24));
        crate::derive::rebuild(&mut e);
        e.menu = Some(tall_menu(3));
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, wheel_ev(true, 0, 5), &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.menu.as_ref().unwrap().highlighted, 1, "short category: wheel down steps the highlight to 1");
        assert_eq!(e.menu.as_ref().unwrap().scroll_top, 0, "short category does not scroll");
        handle(&mut e, wheel_ev(false, 0, 5), &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.menu.as_ref().unwrap().highlighted, 0, "wheel up steps back to 0");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p wordcartel --lib mouse::tests::menu_hover 2>&1 | tail -20`
Expected: FAIL — the menu `Moved` arm does not exist; hover leaves `open`/`highlighted` unchanged.

- [ ] **Step 3: Rewrite `mouse_menu` (keep the `Down(Left)` arm verbatim)**

```rust
pub(crate) fn mouse_menu(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    ctx: &crate::overlays::DispatchCtx) {
    // Hover: (1) onto a DIFFERENT bar category → switch the open dropdown live (reset triple,
    // dedupe on cat != open); (2) onto a dropdown row → set highlighted. Off both → no-op (I2).
    if let MouseEventKind::Moved = ev.kind {
        let hit_area = crate::chrome_geom::menu_area(area);
        let bar_hit: Option<usize> = {
            let groups = &editor.menu.as_ref().unwrap().groups;
            crate::chrome_geom::menu_bar_layout(hit_area, groups).into_iter()
                .find(|(_, r)| ev.column >= r.x && ev.column < r.x + r.width && ev.row == r.y)
                .map(|(cat, _)| cat)
        };
        if let Some(cat) = bar_hit {
            let m = editor.menu.as_mut().unwrap();
            if cat != m.open {
                // Reset triple — identical to menu::intercept's ←/→ arms and the Down bar arm.
                m.open = cat; m.highlighted = 0; m.scroll_top = 0;
            }
            return;
        }
        let (open, scroll_top) = { let m = editor.menu.as_ref().unwrap(); (m.open, m.scroll_top) };
        let row_hit: Option<usize> = {
            let groups = &editor.menu.as_ref().unwrap().groups;
            crate::chrome_geom::menu_dropdown_row_at(hit_area, groups, open, scroll_top, ev.column, ev.row)
        };
        if let Some(idx) = row_hit {
            // menu_dropdown_row_at only returns in-window rows, so no keep_visible needed.
            let m = editor.menu.as_mut().unwrap();
            if m.highlighted != idx { m.highlighted = idx; }
        }
        return;
    }
    // Wheel: viewport scrolls every notch over the dropdown, windowed by the EFFECTIVE item budget
    // (overflow-adjusted, mirroring paint_menu_dropdown + menu_dropdown_row_at); the SELECTION-
    // derived re-window fires ONLY on row-change (I5). Empty → total no-op (I3b).
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let hit_area = crate::chrome_geom::menu_area(area);
        let before = match editor.menu.as_ref() { Some(m) => m.highlighted, None => return };
        // Effective item budget: raw dropdown window minus the reserved indicator row on overflow —
        // identical to paint_menu_dropdown's keep_h + menu_dropdown_row_at's item_rows.
        let (scrolled, n, list_h) = if let Some(m) = editor.menu.as_mut() {
            let n = m.groups.get(m.open).map(|g| g.1.len()).unwrap_or(0);
            if n == 0 { return; }
            let raw_window = n.min(15).min(hit_area.height.saturating_sub(1) as usize);
            let list_h = if n > raw_window { raw_window.saturating_sub(1) } else { raw_window };
            let s = crate::list_window::wheel_list(down, n, list_h, &mut m.highlighted, &mut m.scroll_top);
            (s, n, list_h)
        } else { return };
        if scrolled {
            let (open, scroll_top) = { let m = editor.menu.as_ref().unwrap(); (m.open, m.scroll_top) };
            let row_hit = {
                let groups = &editor.menu.as_ref().unwrap().groups;
                crate::chrome_geom::menu_dropdown_row_at(hit_area, groups, open, scroll_top, ev.column, ev.row)
            };
            if let Some(idx) = row_hit { editor.menu.as_mut().unwrap().highlighted = idx; }
        }
        // I5 dedupe: window-follows-selection only when the row moved (uses the SAME effective
        // budget, so scroll_top never disagrees with the painter or lands on the indicator row).
        let after = editor.menu.as_ref().map(|m| m.highlighted).unwrap_or(before);
        if after != before {
            if let Some(m) = editor.menu.as_mut() {
                crate::list_window::keep_visible(after, n, list_h, &mut m.scroll_top);
            }
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // ⟨UNCHANGED — the existing Down(Left) arm body verbatim⟩
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p wordcartel --lib mouse:: 2>&1 | tail -25`
Expected: PASS (menu hover tests + all pre-existing menu tests).

- [ ] **Step 5: Dwell-disjointness regression (already covered) + clippy**

Confirm the existing `overlays.rs` test `no_dwell_arming_while_splash_is_up` and the routing test `click_under_overlay_does_not_move_caret` still pass, and add a focused dwell-disjointness assertion inside `menu_hover_bar_with_no_menu_open_does_not_open` is sufficient (a bar hover with a menu OPEN never touches `menu_reveal_due` because `mouse::handle` returns into `route_overlay` before the dwell block). Run:

Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -20`
Expected: no new warnings; `too_many_lines` clean (if it fires on `mouse_menu`, extract the wheel arm to a local `fn menu_wheel(...)`).

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/mouse.rs
git commit -m "feat(a21): menu dropdown hover + effective-budget wheel + bar hover-to-switch"
```

---

### Task 4: Preview overlays — hover fires the preview funnel (dedupe-bounded)

`mouse_theme_picker`, `mouse_cursor_picker`. Same hover + wheel shape as T2, PLUS the preview funnel fires whenever `selected` moves (dedupe: exactly once per row crossed). Restore-on-Esc/click-away funnels are UNCHANGED (they live in the `Down`/`intercept` paths, not touched here). The `Down(Left)` arms stay verbatim.

**Files:**
- Modify: `wordcartel/src/mouse.rs` — fns `mouse_theme_picker`, `mouse_cursor_picker`
- Test: `wordcartel/src/mouse.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `theme_cmds::preview_selected_theme`, `cursor_picker::preview_selected`, `cursor_picker::ROW_ACTIONS`; `chrome_geom::{theme_picker_row_at, cursor_picker_row_at}`; `list_window::{wheel_list, list_h_for}`.
- Produces: nothing new.

Preview specifics: capture `before = selected` at the head; after all mutations, fire the preview funnel iff `selected != before` (dedupe). `n == 0` returns before comparing (I3b — no preview).

- [ ] **Step 1: Write the failing tests**

The dedupe count test needs an observable preview counter. Use the caret shape as the proxy for cursor preview (each row is a distinct `CaretShape`/blink), and for theme use a preview call-count via the applied theme changing. Simplest robust proxy: `mouse_cursor_picker` — hovering distinct rows changes `editor.caret_shape`; a repeated hover at the same row does not re-fire (assert via a sentinel). Add:

```rust
    #[test]
    fn cursor_picker_hover_previews_the_row() {
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_cursor_picker();
        let (reg, ex, clk, tx, km) = ctx();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let r = crate::chrome_geom::palette_overlay_rect(area, n + 1);
        // Hover row 3 (Beam · blinking) — list starts at r.y + 1 (no query row).
        handle(&mut e, moved(r.x + 1, r.y + 1 + 3), &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.cursor_picker.as_ref().unwrap().selected, 3, "hover set the highlight");
        let (_, _, shape, _) = crate::cursor_picker::ROW_ACTIONS[3];
        assert_eq!(e.caret_shape, shape, "hover fired the preview funnel (caret shape changed live)");
    }

    #[test]
    fn cursor_picker_hover_same_row_does_not_re_preview() {
        // A repeated Moved at the SAME row must be a no-op (dedupe I5). We prove it by mutating
        // caret_shape out from under the picker and asserting a same-row hover does NOT restore it.
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_cursor_picker();
        let (reg, ex, clk, tx, km) = ctx();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let r = crate::chrome_geom::palette_overlay_rect(area, n + 1);
        handle(&mut e, moved(r.x + 1, r.y + 1 + 3), &reg, &km, &ex, &clk, &tx); // preview row 3
        e.set_caret_shape(crate::config::CaretShape::Default); // tamper
        handle(&mut e, moved(r.x + 1, r.y + 1 + 3), &reg, &km, &ex, &clk, &tx); // SAME row again
        assert_eq!(e.caret_shape, crate::config::CaretShape::Default,
            "same-row hover did NOT re-fire the preview (dedupe on row-change)");
    }

    #[test]
    fn cursor_picker_wheel_empty_guard_and_theme_restore() {
        // cursor_picker has a fixed 7-row list (never empty); assert Esc-restore after a hover
        // sweep leaves the ORIGINAL caret. open_cursor_picker captures original_shape/blink.
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let orig = e.caret_shape;
        e.open_cursor_picker();
        let (reg, ex, clk, tx, km) = ctx();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let r = crate::chrome_geom::palette_overlay_rect(area, n + 1);
        // Sweep across rows 1,2,3.
        for row in 1..=3u16 { handle(&mut e, moved(r.x + 1, r.y + 1 + row), &reg, &km, &ex, &clk, &tx); }
        // Esc through the intercept restores original + closes.
        crate::app::reduce(crate::app::Msg::Input(crossterm::event::Event::Key(
            crossterm::event::KeyEvent { code: crossterm::event::KeyCode::Esc,
                modifiers: KeyModifiers::NONE, kind: crossterm::event::KeyEventKind::Press,
                state: crossterm::event::KeyEventState::NONE })),
            &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.cursor_picker.is_none(), "Esc closed the picker");
        assert_eq!(e.caret_shape, orig, "Esc after a hover sweep restored the original caret");
    }

    #[test]
    fn theme_picker_wheel_empty_list_no_preview() {
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_theme_picker();
        // Filter the picker to zero rows.
        if let Some(tp) = e.theme_picker.as_mut() {
            tp.query = "zzz_no_theme_zzz".into();
            crate::theme_picker::rebuild_rows(tp);
            assert!(tp.rows.is_empty(), "precondition: zero theme rows");
            tp.selected = 0; tp.scroll_top = 0;
        }
        let theme_before = e.theme.clone();
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, wheel_ev(true, 40, 12), &reg, &km, &ex, &clk, &tx);
        handle(&mut e, wheel_ev(false, 40, 12), &reg, &km, &ex, &clk, &tx);
        let tp = e.theme_picker.as_ref().unwrap();
        assert_eq!((tp.selected, tp.scroll_top), (0, 0), "empty theme list: wheel is a total no-op");
        assert_eq!(e.theme, theme_before, "empty list fired NO preview (theme unchanged)");
    }

    #[test]
    fn cursor_picker_wheel_boundary_notch_fires_no_preview() {
        // I5 dedupe on the WHEEL path — MUTATION-DETECTING. Park `selected` at the BOTTOM row
        // (6 = Underline·steady) and wheel DOWN: a true boundary (wheel_list's `.min(n-1)` keeps
        // it at 6, so after == before). Pre-set the caret to a SENTINEL (Block) that DIFFERS from
        // row 6's action — so a spurious re-preview would overwrite it with Underline and the test
        // would catch it. Pointer (0,0) is off the (centered) overlay, so no re-hover interferes.
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_cursor_picker();
        let last = crate::cursor_picker::ROW_ACTIONS.len() - 1; // 6 (Underline·steady)
        { e.cursor_picker.as_mut().unwrap().selected = last; }
        e.set_caret_shape(crate::config::CaretShape::Block); // sentinel ≠ row 6's Underline
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, wheel_ev(true, 0, 0), &reg, &km, &ex, &clk, &tx); // down at bottom → no move
        assert_eq!(e.cursor_picker.as_ref().unwrap().selected, last, "still at the bottom boundary");
        assert_eq!(e.caret_shape, crate::config::CaretShape::Block,
            "boundary wheel notch did NOT re-fire preview (sentinel Block survives; a spurious \
             re-preview would set row 6's Underline)");
    }

    #[test]
    fn theme_picker_wheel_boundary_notch_fires_no_preview() {
        // I5 dedupe on the WHEEL path for the theme overlay — MUTATION-DETECTING via `previewed`.
        // Park at the TOP row and wheel UP: a true boundary (saturating_sub keeps selected at 0,
        // after == before). Pre-set `previewed` to a SENTINEL distinct from row 0's name — a
        // spurious re-preview would overwrite it with Some(rows[0]). Pointer (0,0) is off the
        // (centered) overlay, so no re-hover interferes.
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_theme_picker();
        assert!(!e.theme_picker.as_ref().unwrap().rows.is_empty(), "precondition: builtin themes present");
        { e.theme_picker.as_mut().unwrap().selected = 0; }
        let row0 = e.theme_picker.as_ref().unwrap().rows[0].clone();
        let sentinel = format!("__sentinel_not_{row0}");
        { e.theme_picker.as_mut().unwrap().previewed = Some(sentinel.clone()); }
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, wheel_ev(false, 0, 0), &reg, &km, &ex, &clk, &tx); // up at 0 → no move
        assert_eq!(e.theme_picker.as_ref().unwrap().selected, 0, "still at the top boundary");
        assert_eq!(e.theme_picker.as_ref().unwrap().previewed.as_deref(), Some(sentinel.as_str()),
            "boundary wheel notch did NOT re-fire preview (sentinel in `previewed` survives; a \
             spurious re-preview would set Some(rows[0]))");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p wordcartel --lib mouse::tests::cursor_picker_hover 2>&1 | tail -20`
Expected: FAIL — the preview slots have no `Moved` arm; hover leaves `selected`/caret unchanged.

- [ ] **Step 3: Rewrite the two preview slots (keep `Down(Left)` arms verbatim)**

Full `mouse_theme_picker`:

```rust
pub(crate) fn mouse_theme_picker(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    _ctx: &crate::overlays::DispatchCtx) {
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.theme_picker.as_ref()
            .and_then(|tp| crate::chrome_geom::theme_picker_row_at(area, tp, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            let changed = editor.theme_picker.as_ref().is_some_and(|tp| tp.selected != idx);
            if changed {
                if let Some(tp) = editor.theme_picker.as_mut() {
                    tp.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, tp.rows.len(), &mut tp.scroll_top);
                }
                crate::theme_cmds::preview_selected_theme(editor); // dedupe: only on row change
            }
        }
        return;
    }
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.theme_picker.as_ref() { Some(tp) => tp.selected, None => return };
        let scrolled = if let Some(tp) = editor.theme_picker.as_mut() {
            let n = tp.rows.len();
            if n == 0 { return; } // I3b: no step/scroll/preview on an empty list
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut tp.selected, &mut tp.scroll_top)
        } else { return };
        if scrolled {
            if let Some(idx) = editor.theme_picker.as_ref()
                .and_then(|tp| crate::chrome_geom::theme_picker_row_at(area, tp, ev.column, ev.row))
            {
                if let Some(tp) = editor.theme_picker.as_mut() { tp.selected = idx; }
            }
        }
        // I5 dedupe: re-window AND fire the preview funnel ONLY when the row actually moved.
        let after = editor.theme_picker.as_ref().map(|tp| tp.selected).unwrap_or(before);
        if after != before {
            let n = editor.theme_picker.as_ref().map(|tp| tp.rows.len()).unwrap_or(0);
            if let Some(tp) = editor.theme_picker.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut tp.scroll_top);
            }
            crate::theme_cmds::preview_selected_theme(editor);
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // ⟨UNCHANGED — the existing Down(Left) arm body verbatim⟩
    }
}
```

Full `mouse_cursor_picker` (row count is the fixed `ROW_ACTIONS.len()`; the `n == 0` guard is defensive-uniform, never hit):

```rust
pub(crate) fn mouse_cursor_picker(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
    _ctx: &crate::overlays::DispatchCtx) {
    if let MouseEventKind::Moved = ev.kind {
        let hit = editor.cursor_picker.as_ref()
            .and_then(|cp| crate::chrome_geom::cursor_picker_row_at(area, cp, ev.column, ev.row));
        if let Some(idx) = hit {
            let ah = editor.active().view.area.1;
            let changed = editor.cursor_picker.as_ref().is_some_and(|cp| cp.selected != idx);
            if changed {
                if let Some(cp) = editor.cursor_picker.as_mut() {
                    cp.selected = idx;
                    crate::app::keep_overlay_visible(ah, idx, crate::cursor_picker::ROW_ACTIONS.len(), &mut cp.scroll_top);
                }
                crate::cursor_picker::preview_selected(editor); // dedupe: only on row change
            }
        }
        return;
    }
    if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
        let ah = editor.active().view.area.1;
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        let before = match editor.cursor_picker.as_ref() { Some(cp) => cp.selected, None => return };
        let scrolled = if let Some(cp) = editor.cursor_picker.as_mut() {
            let n = crate::cursor_picker::ROW_ACTIONS.len();
            if n == 0 { return; }
            let list_h = crate::list_window::list_h_for(n, ah);
            crate::list_window::wheel_list(down, n, list_h, &mut cp.selected, &mut cp.scroll_top)
        } else { return };
        if scrolled {
            if let Some(idx) = editor.cursor_picker.as_ref()
                .and_then(|cp| crate::chrome_geom::cursor_picker_row_at(area, cp, ev.column, ev.row))
            {
                if let Some(cp) = editor.cursor_picker.as_mut() { cp.selected = idx; }
            }
        }
        // I5 dedupe: re-window AND fire the preview funnel ONLY when the row actually moved.
        let after = editor.cursor_picker.as_ref().map(|cp| cp.selected).unwrap_or(before);
        if after != before {
            let n = crate::cursor_picker::ROW_ACTIONS.len();
            if let Some(cp) = editor.cursor_picker.as_mut() {
                crate::app::keep_overlay_visible(ah, after, n, &mut cp.scroll_top);
            }
            crate::cursor_picker::preview_selected(editor);
        }
        return;
    }
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        // ⟨UNCHANGED — the existing Down(Left) arm body verbatim⟩
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p wordcartel --lib mouse:: 2>&1 | tail -25`
Expected: PASS (preview hover/dedupe/restore tests + all pre-existing).

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -20`
Expected: no new warnings.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/mouse.rs
git commit -m "feat(a21): embrace hover-preview on theme_picker + cursor_picker (dedupe-bounded)"
```

---

### Task 5: Cross-cutting — `Moved` completeness sweep + e2e journey

**Files:**
- Modify: `wordcartel/src/overlays.rs` `#[cfg(test)] mod tests` — add a `Moved`-leg sweep
- Modify: `wordcartel/src/e2e.rs` — add a `mouse_wheel` harness helper + a hover→wheel→click journey test

**Interfaces:**
- Consumes: the existing openers table pattern in `overlays.rs` (`every_overlay_is_active_xor_and_consumes_key_and_click`); the e2e `Harness` (`ctrl`, `mouse_move`, `mouse_down`, `.editor.borrow()`).
- Produces: nothing new.

- [ ] **Step 1: Write the completeness GUARDRAIL test (overlays.rs)**

This is a durable no-panic / no-data-loss GUARDRAIL, NOT a red-first TDD step: the slots already ignore `Moved` today, so it passes before AND after T2–T4 — its job is to lock in that EVERY slot (the seven list overlays plus the four OUT overlays plus splash) consumes a hover without panicking or mutating the buffer, forever. The genuinely-red hover-behavior tests (hover SETS the highlight — failing before impl, passing after) live in T2/T3/T4. Add to `#[cfg(test)] mod tests` in `wordcartel/src/overlays.rs`:

```rust
    /// A21: every overlay's mouse slot consumes a `Moved` (hover) without panic and without
    /// mutating the document buffer — the no-data-loss guardrail for hover, across all 11 slots
    /// (the four OUT overlays + splash must no-op it too). Mirrors the Down-leg assertion in
    /// `every_overlay_is_active_xor_and_consumes_key_and_click`, calling the slot directly.
    #[test]
    #[allow(clippy::type_complexity)] // test-local table of (name, opener closure) pairs
    fn every_overlay_consumes_moved_without_panic_or_data_loss() {
        use crossterm::event::{MouseEvent, MouseEventKind, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let diag_fixture = || wordcartel_core::diagnostics::Diagnostic {
            range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "m".into(), suggestions: Vec::new(),
        };
        let openers: Vec<(&str, Box<dyn Fn(&mut Editor)>)> = vec![
            ("search",        Box::new(|e: &mut Editor| e.open_search(crate::search_overlay::Phase::Find, 0))),
            ("minibuffer",    Box::new(|e: &mut Editor| e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter))),
            ("palette",       Box::new(|e: &mut Editor| e.open_palette())),
            ("outline",       Box::new(|e: &mut Editor| e.open_outline())),
            ("theme_picker",  Box::new(|e: &mut Editor| e.open_theme_picker())),
            ("file_browser",  Box::new(|e: &mut Editor| e.open_file_browser(std::path::PathBuf::from(".")))),
            ("prompt",        Box::new(|e: &mut Editor| e.open_prompt(crate::prompt::Prompt::swap_recovery()))),
            ("diag",          Box::new(move |e: &mut Editor| e.open_diag(diag_fixture()))),
            ("cursor_picker", Box::new(|e: &mut Editor| e.open_cursor_picker())),
            ("menu",          Box::new(|e: &mut Editor| { e.menu = Some(crate::menu::empty()); })),
            ("splash",        Box::new(|e: &mut Editor| { e.splash = Some(crate::splash::Splash::new(
                &crate::keymap::KeyTrie::default(), "0.0.0")); })),
        ];
        for (name, open) in openers {
            let mut e = Editor::new_from_text("hello world\nsecond line here\n", None, (40, 12));
            crate::derive::rebuild(&mut e);
            open(&mut e);
            let id = *OverlayId::ALL.iter().find(|id| (id.row().is_active)(&e))
                .unwrap_or_else(|| panic!("{name}: no active overlay"));
            let (w, h) = e.active().view.area;
            let area = ratatui::layout::Rect::new(0, 0, w, h);
            let ctx = DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clock, msg_tx: &tx };
            let v0 = e.active().document.version;
            let moved = MouseEvent { kind: MouseEventKind::Moved, column: 6, row: 9, modifiers: KeyModifiers::NONE };
            (id.row().mouse)(&mut e, moved, area, &ctx);
            assert_eq!(e.active().document.version, v0,
                "{name}: Moved did not mutate the buffer (no data loss)");
        }
    }
```

- [ ] **Step 2: Run the guardrail to confirm it passes green**

Run: `cargo test -p wordcartel --lib overlays::tests::every_overlay_consumes_moved 2>&1 | tail -20`
Expected: PASS (a guardrail, green by construction — the assertion is "no panic, buffer version unchanged" across all 11 slots, which holds both before and after T2–T4). No red-first step here — the red hover-behavior coverage is in T2/T3/T4.

- [ ] **Step 3: Add the `mouse_wheel` harness helper (e2e.rs)**

In `wordcartel/src/e2e.rs`, next to `mouse_move`/`mouse_down`:

```rust
    fn mouse_wheel(&mut self, down: bool, col: u16, row: u16) {
        self.step(Msg::Input(Event::Mouse(crossterm::event::MouseEvent {
            kind: if down { crossterm::event::MouseEventKind::ScrollDown }
                  else { crossterm::event::MouseEventKind::ScrollUp },
            column: col, row, modifiers: KeyModifiers::NONE,
        })));
    }
```

- [ ] **Step 4: Write the e2e journey test (e2e.rs)**

Add a `#[test]` in the e2e test region. It opens the palette via `ctrl('p')` (as the existing plugin-palette tests do), hovers a row, wheels, and clicks — driving the real `reduce → advance → render` loop.

```rust
#[test]
fn hover_wheel_click_palette_journey() {
    let mut h = Harness::new("doc\n", None, (80, 24));
    h.ctrl('p');
    assert!(h.editor.borrow().palette.is_some(), "ctrl-p opened the palette");
    let area = ratatui::layout::Rect::new(0, 0, 80, 24);
    let (n, rect) = {
        let e = h.editor.borrow();
        let p = e.palette.as_ref().unwrap();
        (p.rows.len(), crate::chrome_geom::palette_overlay_rect(area, p.rows.len()))
    };
    assert!(n > 4, "precondition: several palette rows");
    // Hover the 3rd visible row → highlight follows the pointer.
    h.mouse_move(rect.x + 1, rect.y + 2 + 2);
    assert_eq!(h.editor.borrow().palette.as_ref().unwrap().selected, 2, "hover moved the highlight");
    // Wheel down → the viewport scrolls (only if the list overflows) and the highlight follows.
    let list_h = crate::list_window::list_h_for(n, 24);
    if n > list_h {
        h.mouse_wheel(true, rect.x + 1, rect.y + 2);
        let p = h.editor.borrow();
        let p = p.palette.as_ref().unwrap();
        assert!(p.scroll_top > 0, "wheel scrolled the viewport");
        assert_eq!(p.selected, p.scroll_top, "highlight followed the pointer's top row");
    }
    // Click the top visible row → dispatch + close.
    h.mouse_down(rect.x + 1, rect.y + 2);
    assert!(h.editor.borrow().palette.is_none(), "click dispatched a row and closed the palette");
}
```

- [ ] **Step 5: Run the e2e + sweep tests**

Run: `cargo test -p wordcartel --lib overlays::tests::every_overlay_consumes_moved e2e::hover_wheel_click 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: §7 return-normalization confirmation (no separate work)**

The T2/T4 rewrites already give every wheel arm a trailing `return;` (the theme/cursor/file_browser arms that previously fell through now return uniformly). No extra edit — confirm by inspection that no slot's wheel arm falls through into the `Down` arm.

- [ ] **Step 7: Full-suite green + workspace clippy**

Run: `cargo test -p wordcartel-core && cargo test -p wordcartel 2>&1 | tail -15`
Expected: all green.

Run: `cargo clippy --workspace --all-targets 2>&1 | tail -15`
Expected: clean under `deny` (no warnings; `too_many_lines` satisfied).

- [ ] **Step 8: Commit**

```bash
git add wordcartel/src/overlays.rs wordcartel/src/e2e.rs
git commit -m "test(a21): Moved completeness sweep + hover→wheel→click e2e journey"
```

---

## Pre-merge (final-gate) checklist

- `cargo test -p wordcartel-core` and `cargo test -p wordcartel` green.
- `cargo build -p wordcartel` and `cargo test -p wordcartel --no-run` warning-free.
- `cargo clippy --workspace --all-targets` clean under `deny` (incl. `too_many_lines` ≤ 100 per fn and `module_budgets` for `mouse.rs`).
- `scripts/smoke/run.sh` run; quote its one-line summary verbatim in the pre-merge report (advisory-pass).
- Command-surface contract: **N/A — does not touch the command surface** (state explicitly).
- Backlog: mark A21 shipped in `backlog.toml` → `scripts/backlog bless`; move its prose section to `docs/backlog-archive.md` and repoint `doc =` (per CLAUDE.md).

## Self-review notes (author's own pass)

- **Spec coverage:** I1 hover (T2/T3/T4 hover arms) ✓; I2 off-rect (T2 `palette_hover_off_rect_leaves_highlight`, the `if let Some(idx)` guard) ✓; I3 wheel-scroll+drag+re-hover (`wheel_list`+re-hover blocks; T2 `palette_wheel_scrolls_and_re_hovers`) ✓; I3b empty no-op (`if n == 0 { return; }`; T2/T4 empty tests) ✓; I4 short-step (`wheel_list` short branch; T3 `menu_wheel_short_category_steps_by_one`) ✓; I5 dedupe at BOTH hover AND wheel (hover `if … != idx`; wheel `if after != before` at all seven slots — T4 `cursor_picker_hover_same_row_does_not_re_preview` + the MUTATION-DETECTING boundary tests `cursor_picker_wheel_boundary_notch_fires_no_preview` (bottom row 6 sentinel Block ≠ Underline) and `theme_picker_wheel_boundary_notch_fires_no_preview` (`previewed` sentinel ≠ rows[0])) ✓; I6 hover-to-switch (T3 reset-triple + `cat != open` + first-open tests) ✓; I7 dwell-disjoint (T3 no-menu-open test; `mouse::handle` early-return unchanged) ✓; I8 preview embrace + restore (T4) ✓; I9 Drag-not-Moved (slots ignore Drag — unchanged) ✓; I10 no-motion graceful (re-hover uses wheel coords — unchanged plumbing) ✓; I11 no data loss / no click-through (T5 guardrail sweep) ✓; menu effective-budget + indicator-row safety (T3 `menu_wheel_tall_category_scrolls_without_landing_on_indicator_row`) ✓; list_h==0 / row_count==0 saturating (T1) ✓; command-surface N/A (checklist) ✓.
- **I5 wheel-dedupe structure (Codex plan-gate r1 fix):** every wheel arm captures `before = selected/highlighted`, runs `wheel_list` (which moves `scroll_top` every notch and possibly the selection), re-hovers on the scroll path, then fires the selection-derived side effects (`keep_overlay_visible`/`keep_visible` AND, on preview slots, the preview funnel) inside ONE `if after != before { … }` guard — so a clamp-boundary notch neither re-derives `scroll_top` from the (unmoved) selection nor re-previews. Verified present at all seven slots (`grep -c "if after != before"`).
- **Saturating arithmetic (Codex plan-gate r1 fix):** `wheel_scroll` down uses `scroll_top.saturating_add(WHEEL_STEP)`; `wheel_list` short-step uses `selected.saturating_add(1).min(row_count.saturating_sub(1))`; `clamp_into_window`'s `list_h - 1` is guarded by the `list_h == 0` early return and wrapped in `saturating_add`. No bare `+ 1` / `n - 1` remains in T1 or the wheel arms.
- **Placeholder scan:** the only "⟨UNCHANGED⟩" markers are explicit "keep the existing `Down(Left)` arm verbatim" instructions, not missing code — the `Down` arms are pre-existing and must not be edited; every NEW arm is shown in full.
- **Type consistency:** `wheel_list(down, n, list_h, &mut selected|highlighted, &mut scroll_top) -> bool` used identically in T2/T3/T4; `keep_overlay_visible(area_h, sel, n, &mut top)` and `list_window::keep_visible(hl, n, list_h, &mut top)` used per family; hit-tester signatures match `chrome_geom`; `preview_selected_theme(editor)` / `preview_selected(editor)` take `&mut Editor`; `MenuView { groups, open, highlighted, built, scroll_top }` all-pub construction used by `tall_menu`; the `selected`/`highlighted` split honored throughout.
