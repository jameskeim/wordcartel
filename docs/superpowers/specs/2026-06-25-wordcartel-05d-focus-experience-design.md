# Effort 5d — Focus / Writing Experience (design)

**Status:** design / pre-plan
**Date:** 2026-06-25
**Depends on:** 5a (layered config + per-effort `[section]` keys), 5c (`paragraph_range_at`, `sentence_bounds`, `nav::screen_pos`/`offset_at_cell`/`ensure_visible`), 5b (registry/commands), core `layout`/`unicode-segmentation`.

## 1. Goal & scope

A distraction-free writing environment: four independent, toggleable, configurable display layers — **typewriter scrolling**, **focus dimming**, a **centered measure with wrap-guide**, and a **live word/char count**. Each composes with the others and with the existing render modes; each defaults OFF and is flipped by a command + a `[view]` config key.

**In scope (5d):** the four features above, the shared `[view]` config section, the runtime `ViewOpts` state, the toggle commands, and the shared text-column geometry that the centered measure requires.

**Out of scope (noted future work):** typewriter end-of-document "virtual space" padding (v1 clamps at boundaries); per-buffer (vs global) view options; theming of the dim/guide colors beyond a sensible default.

## 2. Crate & widget posture

No new dependencies. Word counting uses the already-present `unicode-segmentation` (UAX-#29) in `wordcartel-core`. All rendering is existing `ratatui` primitives + styles (dim via `Style`/`Modifier`). ratatui remains a pure renderer — the measure offset and dim pass are ours.

## 3. Architecture & modules

| Unit | Responsibility | Depends on |
|------|----------------|-----------|
| **`wordcartel-core/src/count.rs`** (new, pure) | `word_count(&str)`/`char_count(&str)` (UAX-#29). Oracle-tested. | unicode-segmentation |
| `wordcartel/src/config.rs` (extend) | `[view]` section → `ViewConfig` (mirrors `MouseConfig`/`RawMouse` + the `load()` merge). | 5a |
| `wordcartel/src/editor.rs` (extend) | `Editor.view_opts: ViewOpts` (runtime flags, seeded from config). | — |
| **`wordcartel/src/nav.rs`** (extend) | **`text_geometry(editor) -> TextGeometry`** (the keystone); typewriter-aware `ensure_visible`; measure-aware `screen_pos`/`offset_at_cell`; the `layout::layout` call sites use `text_width`. | core layout |
| `wordcartel/src/render.rs` (extend) | Measure paint offset; focus dim pass; wrap-guide line; word-count status segment. | nav geometry |
| `wordcartel/src/registry.rs` / `keymap.rs` (extend) | `toggle_typewriter`/`_focus`/`_measure`/`_wrap_guide`/`_word_count` commands. | 5b |

## 4. The keystone — shared text-column geometry

The centered measure introduces a **horizontal text offset**. Today text paints at `area.x` and wraps at `view.area.0`. Three places must agree on *where the text column is*, or the cursor/mouse will desync (the 5c-m lesson):

```rust
pub struct TextGeometry { pub text_left: u16, pub text_width: u16 }

/// The text column's left edge and width for the current frame.
/// Measure ON and viewport wider than wrap_column → a centered column of
/// width wrap_column; else full width at area.x.
pub fn text_geometry(editor: &Editor) -> TextGeometry {
    let vp = editor.active().view.area.0;
    let opts = &editor.view_opts;
    if opts.measure && vp > opts.wrap_column && opts.wrap_column > 0 {
        let text_width = opts.wrap_column;
        let text_left = (vp - text_width) / 2; // relative to area.x (area.x==0 today)
        TextGeometry { text_left, text_width }
    } else {
        TextGeometry { text_left: 0, text_width: vp.max(1) }
    }
}
```

- **Wrap width — the cache PRODUCER is `derive::rebuild`, not render (Codex Critical):** the visible-line layout cache `view.line_layouts` is populated by `layout::layout` inside **`derive::rebuild` (derive.rs:130/148)**; `render.rs` only *consumes* the cache (render.rs:133-164 reads `view.line_layouts`, it does NOT call `layout::layout`). So `text_width` must be threaded into **`derive::rebuild`** (which currently derives `vp_width` from `area_width` at derive.rs:130) AND the two on-demand `nav` call sites (nav.rs:41, nav.rs:112). If only the nav sites changed, render would paint full-width *cached* rows while `screen_pos`/`offset_at_cell` use measure-width on-demand maps → wrap/cursor/mouse desync. **All three call sites (`derive::rebuild` + the two nav helpers) read `nav::text_geometry(editor).text_width`.**
- **Render paint rect (Codex Important):** each visual row is drawn into **`Rect::new(area.x + text_left, edit_top + screen_row, text_width, 1)`** (was `Rect::new(area.x, …, w, 1)`) — bounded to the text column so text never paints into the margins; the hardware cursor is placed at `area.x + text_left + vcol`.
- **`screen_pos`** returns the visual `(vcol, vrow)` *within the text column* (math unchanged — it already works in text-relative columns via the `ColMap`); render adds `text_left` when placing the cursor.
- **`offset_at_cell(col, row)`** subtracts `text_left` from the incoming mouse `col` **inside the function** (so ALL callers — click, shift-click, AND the 5c-m drag path — benefit without separate adjustment): `text_col = col.saturating_sub(text_left)`; a click left of `text_left` → column 0 (line start), right of `text_left + text_width` → line end. No panic in the margins.
- **Scrollbar (Codex Important):** the 5c-m scrollbar stays at the **viewport's rightmost column** (`area.x + viewport_width - 1`), *outside* the text geometry — it is a viewport scrollbar, intentionally detached from a centered text column. The wrap-guide is NOT drawn on the scrollbar column when the scrollbar is visible (skip `gx == viewport_right`).
- **Cache invalidation:** `view.line_layouts` is keyed by line index, not width, so a width change with a stale cache renders at the OLD width. `derive::rebuild` already *clears* `view.line_layouts` before repopulating (derive.rs:135), and resize already triggers a rebuild (app.rs:743). Toggling `measure` / changing `wrap_column` therefore just needs to **force a `derive::rebuild`** (or clear `view.line_layouts` so the next frame rebuilds) — the toggle command does this, mirroring the resize path.

This `text_geometry` helper is the single source of truth — landed and round-trip-tested (`screen_pos` ↔ `offset_at_cell` with a non-zero `text_left`) before the features that depend on it.

## 5. Centered measure + wrap-guide

- **Measure** (`view_opts.measure`): §4 gives the centered column. Equal margins; odd leftover pixel goes left (`(vp - width)/2` floor). Applies in **all render modes**. When `vp <= wrap_column`, the measure is inert (full width) — small terminals are unaffected.
- **Wrap-guide** (`view_opts.wrap_guide`, a *separate* toggle sharing `wrap_column`): a dim vertical line (`│`, dim/`DarkGray` style) at screen column `gx = area.x + text_left + wrap_column` — i.e. the right edge of the measure when the measure is on, or a guide over full-width text when the measure is off (so the "guide-only" experience is `measure=off, wrap_guide=on`). Drawn per visible editing row, under the text (text cells overwrite it where they coincide). **Guard:** only drawn when `gx < area.x + viewport_width` (a guide column past a narrow viewport is simply not shown — no panic, no wrapped column).

## 6. Typewriter scrolling

`view_opts.typewriter` + `typewriter_anchor: f32` (0.0 top … 1.0 bottom, default 0.5). When ON, `nav::ensure_visible` pins the caret's visual row to `anchor_row = (edit_height as f32 * anchor).round()`.

**Visual-row, not logical-line, coordinates (Codex Important):** the viewport top is two-dimensional — `View.scroll` (logical line) + `View.scroll_row` (visual sub-row) — and `max_scroll` today is logical-only (`total_logical_lines - 1`, nav.rs:360). Typewriter must therefore work in **absolute visual-row space**: compute the caret's absolute visual row `Cabs` (sum of visual rows of all logical lines before the caret line + the caret's visual row within its line — the same accounting `rows_before_caret` already does), set the desired viewport-top absolute visual row to `Cabs - anchor_row`, then convert that target top back to `(scroll, scroll_row)` (walk logical lines accumulating visual-row counts via the existing `rows_of_line`). **Clamp** the target top to `[0, last_valid_top]` where `last_valid_top` is the viewport-top visual row that still fills the screen (so we never scroll past the document's last visual rows). This reuses the existing `rows_before_caret`/`rows_of_line`/`advance_view_top_one_row` machinery rather than the logical `max_scroll`.

Consequences (v1, deliberate): in the document interior the caret rides the anchor; near the very top it sits above the anchor (top clamps at `(0,0)`); near the bottom it drifts below the anchor (no end virtual-space — future enhancement). When OFF, `ensure_visible` keeps today's minimal-scroll behavior verbatim.

**Drag is exempt (Codex Important):** the 5c-m mouse-drag path updates the selection head + `derive::rebuild` but does NOT call `ensure_visible` — its own edge auto-scroll (`scroll_up_one`/`scroll_down_one`) governs scrolling during a drag. Typewriter does not apply mid-drag (the drag autoscroll already keeps the head visible); typewriter resumes on the next keyboard motion (which calls `ensure_visible`). **Focus dimming, by contrast, follows the drag automatically** — it is recomputed from the caret in `render` each frame, independent of `ensure_visible`.

## 7. Focus dimming

`view_opts.focus` + `focus_granularity` (Paragraph default | Sentence). When ON, render computes the **active region** byte-range `[from, to)` at the caret once per frame — `nav::paragraph_range_at(blocks, buf, head)` or (within that paragraph window) `textobj::sentence_bounds`.

**Row-level intersection (Codex Important — per-cell dimming is harder than it looked, simplified for v1):** `VisualRow.src_span` (layout.rs) is **line-relative**; render converts it to a document-global range by adding the line start: `global = derive::line_start(buf, l) + src_span`. A visible row is painted **full style if its global span intersects `[from, to)` at all, dim style (`DarkGray`, no color/bold) otherwise** — whole-row granularity, no per-cell `StyledSeg` splitting (render today paints whole styled segments, not per-cell, so per-cell boundary dimming would need a render refactor — deferred). For **paragraph** granularity this is exact (blocks are line-delimited). For **sentence** granularity the line(s) containing the sentence stay bright (slightly wider than the sentence) — an accepted v1 approximation; per-cell sentence dimming is a noted future refinement. Composes with the measure (dim is a style choice on the already-positioned row) and all render modes.

## 8. Word count

- **Pure core (`count.rs`):** `pub fn word_count(text: &str) -> usize` (count of UAX-#29 word segments whose first char is alphanumeric — reuse the `textobj::is_word` rule for consistency) and `pub fn char_count(text: &str) -> usize` (`text.chars().count()`).
- **Display (left+right composition — Codex Minor):** the status row is a single truncated string today (render.rs:211-234: `path* [MODE] status`, REVERSED, `.chars().take(w)`). When `view_opts.word_count` is ON and no prompt/minibuffer occupies the row, compose the status as **left segment + right segment**: build the right segment `"{words} words · {chars} chars"` first, **reserve its width** (`r = right.chars().count() + 1` padding), truncate the LEFT segment to `w - r`, then pad so the right segment is flush-right. (If `w` is too small to fit both, the right segment wins its reserved width and the left is truncated to whatever remains, ≥ 0.) **Selection-aware:** if the primary selection is non-empty, count over `buffer.slice(from..to)`; else over the whole document. Recomputed each frame (O(n), fine for v1; version-cache is a noted option).

## 9. Config, state & commands

- **`[view]` config** (`ViewConfig`, all fields with defaults; `RawView` with `Option<…>` per-field merge in `load()`, mirroring `[mouse]`):
  `typewriter=false`, `typewriter_anchor=0.5`, `focus=false`, `focus_granularity="paragraph"`, `measure=false`, `wrap_column=80`, `wrap_guide=false`, `word_count=false`.
- **Validation on load (Codex Important — bounded fields need deterministic handling, with a warning into `load()`'s warning vec):**
  - `typewriter_anchor` → **clamp** to `0.0..=1.0` (out-of-range warns + clamps).
  - `focus_granularity` → parsed into an enum `FocusGranularity { Paragraph, Sentence }`; an unrecognized string **warns + falls back to Paragraph**.
  - `wrap_column` → minimum **20** (smaller warns + clamps to 20); `0` is *not* special-cased here (a clamped-to-20 column is harmless and the measure is inert when `vp <= column` anyway).
- **`Editor.view_opts: ViewOpts`** holds the runtime values, seeded from `ViewConfig` at startup (in `app::run`, like `mouse_capture`). It is **global** (not per-buffer).
- **Toggle commands** (registered, `MenuCategory::View`, palette/menu-accessible, **no default keybindings** — saves key space; the plan may add chords after verifying they're free): `toggle_typewriter`, `toggle_focus`, `toggle_measure`, `toggle_wrap_guide`, `toggle_word_count`. The five ids are new and must be unique (`Registry::register` silently overwrites on a duplicate id — a builtin-id-uniqueness test guards this, §11). `toggle_measure` (and any wrap_column change) also forces a `derive::rebuild` / clears `view.line_layouts` (§4).

## 10. Error handling & edge cases

- `wrap_column = 0` or `vp <= wrap_column` → measure inert (full width); no divide-by-zero (`text_geometry` guards).
- Tiny terminal (height ≤ 1) → typewriter/dim/guide degrade gracefully (existing `edit_height` guards); the status row is reserved as today.
- Mouse click in a measure margin → clamps to line start/end (no panic, no offset past EOL).
- Focus region at an empty paragraph / doc start/end → `paragraph_range_at` is total (5c), so the active range is always valid; an empty region dims nothing extra.
- Toggling measure mid-edit → layout cache cleared, lines re-flow; the caret stays on its offset (selection is byte-based, unaffected by re-wrap).
- Word count on a multi-MB doc → O(n) per frame; acceptable for v1 (note the version-cache option).

## 11. Testing strategy

- **Core `count.rs` (oracle/unit):** word/char counts over ASCII, multibyte, punctuation, contractions, empty — pure and deterministic.
- **`nav::text_geometry`:** measure on/off, `vp > / <= / == wrap_column`, even/odd margin; `text_left`/`text_width` exact.
- **The keystone seam:** `screen_pos` ↔ `offset_at_cell` round-trip **with a non-zero `text_left`** (measure on) — a click at the cursor's screen cell returns the cursor's offset; a click in the left/right margin clamps to line start/end.
- **Typewriter:** caret motion with `typewriter=on` pins the caret visual row to `anchor_row` in the doc interior; clamps (caret above anchor) near the top.
- **Focus dimming:** the active-region range at a caret in paragraph mode equals `paragraph_range_at`; in sentence mode equals the sentence; rows outside are styled dim (assert the style decision, not pixels).
- **Word count:** selection-aware count (selection vs whole doc); the status segment string format.
- **Config:** `[view]` parses + merges; `ViewOpts` seeded; validation (anchor clamp, bad `focus_granularity` → warn+Paragraph, `wrap_column` min-20) emits the right warnings; each `toggle_*` flips its flag (and `toggle_measure` forces a rebuild).
- **Command-id uniqueness:** a test asserts all `Registry::builtins()` ids are unique (the five new `toggle_*` ids don't collide / silently overwrite).
- No pre-existing test weakened; full workspace green, zero warnings.

## 12. Module & command summary

- **New file:** `wordcartel-core/src/count.rs`.
- **New state:** `Editor.view_opts: ViewOpts`; `ViewConfig`/`RawView` in config.
- **New helper:** `nav::text_geometry -> TextGeometry { text_left, text_width }` (consumed by `derive::rebuild` + the two nav `layout::layout` sites + render paint + `screen_pos` + `offset_at_cell`); typewriter-aware `ensure_visible` (visual-row space). `derive.rs` is touched (the cache producer reads `text_width`).
- **New commands:** `toggle_typewriter`, `toggle_focus`, `toggle_measure`, `toggle_wrap_guide`, `toggle_word_count`.
- **Reuses:** 5c `paragraph_range_at`/`sentence_bounds`/`screen_pos`/`offset_at_cell`/`ensure_visible`, the `[mouse]` config pattern, the `RenderMode` status-row precedent.

## 13. Deliberate decisions (for review)

1. **One shared `text_geometry`** consumed by the layout PRODUCER `derive::rebuild` + the two nav on-demand sites + render paint + `screen_pos` + `offset_at_cell` (no desync) — §4.
2. **Toggling measure / changing wrap_column forces a `derive::rebuild`** (re-wrap like a resize, which already clears `line_layouts`) — §4.
3. **Typewriter works in absolute visual-row space** (not logical `max_scroll`), v1 clamps at doc boundaries (no end virtual-space); **drag is exempt** (its autoscroll governs) — §6.
4. **`measure` and `wrap_guide` are independent toggles sharing `wrap_column`** (guide-only reachable; guide skips the scrollbar column / past-viewport) — §5.
5. **Focus dimming is ROW-LEVEL** (a row is bright if its global span intersects the active region) — per-cell deferred; default = paragraph, sentence config-selectable — §7.
6. **Word count = selection-aware status segment** via left+right composition, toggleable — §8.
7. **All toggles default OFF, palette/menu-only (no default chords); `view_opts` is global; config validates anchor/granularity/wrap_column** — §9.
8. **Measure applies in all render modes; the scrollbar stays at the viewport edge (detached from a centered column)** — §4/§5.
9. **`offset_at_cell` subtracts `text_left` internally** so all callers (click/shift-click/drag) get the measure adjustment for free — §4.
