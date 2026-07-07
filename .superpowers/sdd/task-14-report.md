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
