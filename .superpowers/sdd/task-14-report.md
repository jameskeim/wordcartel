# Task 14 Report: Menu dropdown windowing + n/total indicator + wheel scroll

## Status

DONE

## What was implemented

### menu.rs
- Added `pub scroll_top: usize` field to `MenuView`.
- Set `scroll_top: 0` in `empty`, `empty_at`, and `build` constructors.

### render.rs
- `menu_dropdown_rect`: replaced `height = leaves.len()` with `leaves.len().min(15).min(avail_below)` where `avail_below = area.height.saturating_sub(1)`. Returns `None` when budget is 0 (cramped terminal).
- `menu_dropdown_row_at`: added `scroll_top: usize` parameter; returns `Some((row - r.y) as usize + scroll_top)` for absolute indexing.
- Added `tall_menu_groups(n)` test helper.
- Added `menu_dropdown_windows_a_tall_category` (T14-a).
- Added `dropdown_indicator_row_carries_panel_bg` (T14-b carry-forward from Task 8).

### render_overlays.rs
- Dropdown paint block: calls `keep_visible` against the live `drop_rect.height` every frame (two-layer invariant). Slices `leaves[scroll_top..end]`. When category overflows the window, reserves the bottom row for the `windowed_indicator`. Item list rendered in `item_rect` (top N-1 rows); indicator rendered in a 1-row rect at `drop_rect.y + drop_rect.height - 1`.

### mouse.rs
- Updated `menu_dropdown_row_at` call to pass `scroll_top`.
- Added `ScrollDown/ScrollUp` arm for open menu: moves `highlighted` within `[0, n)`, calls `keep_visible` with `avail_below`-derived `list_h` so the window scrolls at the right boundary.
- `bar_hit` category-switch: resets `m.scroll_top = 0` in addition to `m.highlighted = 0`.
- Added `menu_wheel_scrolls_dropdown` test.

### app.rs
- `KeyCode::Left/Right` arms: added `menu.scroll_top = 0` reset.
- `KeyCode::Up/Down` arms: call `keep_visible(highlighted, n, n.min(15), &mut menu.scroll_top)` after each move.

## Gates

- `cargo test -p wordcartel --lib`: 869 passed, 0 failed.
- `cargo build -p wordcartel`: warning-free.
- `cargo clippy -p wordcartel --all-targets`: clean.

---

## C1 fix + regression test (post-review patch)

### Bug
`keep_visible` was called with `list_h = drop_rect.height` (the full rect height), but when
the category overflows the paint only renders items in `item_rows = list_h - 1` rows —
reserving the bottom row for the n/total indicator.  `keep_visible` guaranteed
`highlighted ∈ [scroll_top, scroll_top + list_h)`, but the highlighted item is only VISIBLE
when `highlighted ∈ [scroll_top, scroll_top + item_rows)`.  When
`highlighted == scroll_top + list_h - 1` the item mapped onto the indicator row and was
never painted; repeated Down kept the highlight stuck hidden.

### Fix (`render_overlays.rs`)
Compute `overflows` BEFORE calling `keep_visible`, then pass the item-row budget `keep_h`
instead of the full `list_h`:

```rust
let overflows = leaves_len > list_h;
let keep_h = if overflows { list_h.saturating_sub(1) } else { list_h };
crate::list_window::keep_visible(m.highlighted, leaves_len, keep_h, &mut m.scroll_top);
```

The downstream `overflows`/`item_rows` computation in the paint block is unchanged — it
still computes `let overflows = leaves_len > list_h` correctly from the same `list_h`.

### Minor consistency comments (`app.rs`, `mouse.rs`)
Added a one-line "coarse follow-the-selection layer — paint re-windows against the true
item-row budget every frame (list_window two-layer invariant)" comment at each handler
`keep_visible` call, per the reviewer's guidance.  No code changes to the handlers.

### Regression test (`render.rs` — `dropdown_highlight_never_hidden_in_overflow`)
80×8 terminal, 20-leaf category (`drop_rect.height = 6`, `item_rows = 5`).  Sets
`highlighted = 6`, `scroll_top = 0` — the exact position hidden under the old code.
After render:
- Arithmetic invariant: `highlighted - scroll_top < item_rows` (5 < 5 was FALSE under
  old code; 4 < 5 is TRUE under fix, because `keep_h=5` drives scroll_top to 2).
- Visual invariant: the highlighted row carries `cs.menu_sel.fg`; the indicator row does not.

**Confirmed FAILS under old code** (revert + run):
```
FAILED: highlighted=6, scroll_top=1, item_rows=5 — highlighted-scroll_top=5 must be < 5
```
**PASSES under fix**: `ok` (870 passed, 0 failed).

### Final gates (post-fix)
- `cargo test -p wordcartel --lib`: 870 passed, 0 failed.
- `cargo build -p wordcartel`: warning-free.
- `cargo clippy -p wordcartel --all-targets`: clean.

---

## T14-e fix: hit-test excludes reserved indicator row (Codex pre-merge gate finding)

### Bug (Codex cross-task finding)
`menu_dropdown_row_at` mapped EVERY row inside `drop_rect` to `scroll_top + (row - r.y)`,
including the bottom row reserved for the n/total indicator when a category overflows the
window.  A mouse click on that indicator row returned `Some(scroll_top + (list_h - 1))` —
dispatching the item at that index, which the user could not see.

### Fix (`render.rs` — `menu_dropdown_row_at`)
Added the same `overflows`/`item_rows` computation used by the paint, guarding the hit-test
so clicks outside the actual item rows return `None`:

```rust
let leaves_len = groups.get(open).map(|g| g.1.len()).unwrap_or(0);
let list_h = r.height as usize;
let overflows = leaves_len > list_h;   // identical to render_overlays.rs line 342
let item_rows = if overflows { list_h.saturating_sub(1) } else { list_h };
if col >= r.x && col < r.x + r.width && row >= r.y {
    let row_in_window = (row - r.y) as usize;
    if row_in_window < item_rows {
        let abs = scroll_top + row_in_window;
        if abs < leaves_len { Some(abs) } else { None }
    } else { None }
} else { None }
```

The non-overflow case is unchanged: `item_rows == list_h`, every rendered row is clickable.

### Call-site sanity-check (`mouse.rs` ~173)
When `menu_dropdown_row_at` returns `None` for an indicator-row click, `row_id` is `None`.
Neither `bar_hit` nor `row_id` is Some, so the else arm fires: `editor.menu = None` (close
menu). This is acceptable — clicking the indicator (a scroll position display, not a button)
closes the menu, which is the same as clicking anywhere outside.

### Regression test added (`render.rs` — `dropdown_indicator_row_hit_test_returns_none`)
80×8 terminal, 20-leaf category (`drop_rect.height = 7`, `item_rows = 6`).  Three assertions:
1. Click on indicator row (`drop_rect.y + 6`) → `None` (was `Some(6)` under old code — FAIL).
2. Click on last real item row (`drop_rect.y + 5`) → `Some(5)` — PASS.
3. Click on first item row (`drop_rect.y`) → `Some(0)` — PASS.

**Confirmed FAILS under old code**: the indicator-row click returned `Some(6)` instead of `None`.
**PASSES under fix**: `test render::tests::dropdown_indicator_row_hit_test_returns_none ... ok`.

### Final gates (post T14-e fix)
- `cargo test -p wordcartel --lib`: 871 passed, 0 failed.
- `cargo build -p wordcartel`: warning-free.
- `cargo clippy -p wordcartel --all-targets`: clean.
- All named prior menu tests pass: `menu_dropdown_windows_a_tall_category`,
  `menu_wheel_scrolls_dropdown`, `dropdown_indicator_row_carries_panel_bg`,
  `dropdown_highlight_never_hidden_in_overflow`, `click_on_inactive_bar_opens_that_category`.

---

## Fable whole-branch blocker fix: `menu_area` shared helper (cross-caller geometry drift)

### Bug (Fable probe — demonstrated on 30×10, 9-leaf category)

The dropdown painter (`render_overlays.rs`) derived its rect from
`menu_area = Rect::new(area.x, area.y, area.width, h.saturating_sub(1))` — frame minus the
status row.  The mouse hit-test path passed the full-height `area` directly to
`menu_bar_layout` and `menu_dropdown_row_at`.  Since `menu_dropdown_rect`'s
`avail_below = area.height.saturating_sub(1)`, the painter's budget was `h-2` rows and the
hit-test's budget was `h-1` rows — one row taller.

On a short terminal (h ≤ 16) with a category whose leaf count fits in the hit-test window
but overflows the paint window, a click on the painted indicator row (or one row below the
painted dropdown) dispatched a hidden off-screen leaf.  Fable demonstrated this at 30×10,
9-leaf category: painter gave list_h=8, overflows=true, indicator at row 8; mouse gave
list_h=9, overflows=false, all 9 rows dispatachable — click row 8 → dispatched leaf 7 via
`move_right`, caret 0→1.

### Fix

Added a shared helper in `render.rs`:

```rust
/// The area the menu bar and dropdown are laid out against: the frame area with the
/// reserved status row excluded.  Both the painter (`render_overlays::paint`) and the
/// mouse hit-test path (`mouse::route_overlay`) MUST derive dropdown geometry through
/// this helper — so `avail_below` in `menu_dropdown_rect` evaluates against the same
/// height in both call sites and the two windows can never drift (Fable whole-branch fix).
pub(crate) fn menu_area(area: Rect) -> Rect {
    Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1))
}
```

### Every menu-geometry call site routed through `menu_area`

1. `render_overlays.rs:295` — painter's `menu_area` local (replaced inline `Rect::new`)
2. `mouse.rs` scroll arm — `avail_below` for coarse `keep_visible` (was `area.height.saturating_sub(1)`)
3. `mouse.rs` click arm — `hit_area` passed to `menu_bar_layout` (was `area`)
4. `mouse.rs` click arm — `hit_area` passed to `menu_dropdown_row_at` (was `area`)

### Regression tests (Fable probes, mouse.rs)

30×10 terminal, 9-leaf category (overflows the 8-row paint window; indicator at row 8).
Leaves all map to `move_right` so dispatch is detectable via caret position (0→1).

| Test | Old result | New result |
|------|------------|------------|
| `menu_click_on_painted_indicator_row_does_not_dispatch` — click row 8 | FAIL (caret moved 0→1) | PASS |
| `menu_click_below_painted_dropdown_does_not_dispatch` — click row 9 | FAIL (caret moved 0→1) | PASS |

A third paint-truth test (`fable_menu_paint_truth_indicator_on_row8_item7_not_visible`)
verifies the geometric foundation: indicator text appears at row 8, label "item07" absent.

**Confirmed FAIL under old code (temporary revert to `hit_area = area`):**
```
test result: FAILED. 0 passed; 2 failed
    mouse::tests::menu_click_on_painted_indicator_row_does_not_dispatch
    mouse::tests::menu_click_below_painted_dropdown_does_not_dispatch
```

### Final gates (post Fable blocker fix)
- `cargo build -p wordcartel`: warning-free.
- `cargo clippy -p wordcartel --all-targets`: clean.
- `cargo test -p wordcartel --lib`: **874 passed, 0 failed** (3 new tests added).
- All named prior menu tests pass: `click_on_inactive_bar_opens_that_category`,
  `dropdown_indicator_row_hit_test_returns_none`, `menu_dropdown_windows_a_tall_category`,
  `menu_wheel_scrolls_dropdown`, `dropdown_highlight_never_hidden_in_overflow`.
