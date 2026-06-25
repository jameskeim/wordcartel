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

- **Wrap width:** every `layout::layout(text, role, is_active, vp_width)` call site (nav.rs:41, nav.rs:112, and render's per-row paint) passes `text_geometry(editor).text_width` instead of `view.area.0`.
- **Render paint:** each text row is drawn at `x = area.x + text_left` (was `area.x`); the hardware cursor is placed at `area.x + text_left + vcol`.
- **`screen_pos`** returns the visual `(vcol, vrow)` *within the text column* (unchanged math, since it already works in text-relative columns via the `ColMap`); render adds `text_left` when placing the cursor.
- **`offset_at_cell(col, row)`** subtracts `text_left` from the incoming mouse `col` before mapping: a click left of `text_left` clamps to column 0 (line start) and right of `text_left + text_width` clamps to the line end. Clicks in the margins never panic.
- **Cache invalidation:** `text_width` feeds the layout, and `view.line_layouts` is cached per line at a given width. Toggling `measure` or changing `wrap_column` **clears `view.line_layouts`** (same as a resize) so lines re-layout at the new width. The toggle command and the config-change path do this clear.

This `text_geometry` helper is the single source of truth — landed and round-trip-tested (`screen_pos` ↔ `offset_at_cell` with a non-zero `text_left`) before the features that depend on it.

## 5. Centered measure + wrap-guide

- **Measure** (`view_opts.measure`): §4 gives the centered column. Equal margins; odd leftover pixel goes left (`(vp - width)/2` floor). Applies in **all render modes**. When `vp <= wrap_column`, the measure is inert (full width) — small terminals are unaffected.
- **Wrap-guide** (`view_opts.wrap_guide`, a *separate* toggle sharing `wrap_column`): a dim vertical line (`│`, dim/`DarkGray` style) at screen column `gx = area.x + text_left + wrap_column` — i.e. the right edge of the measure when the measure is on, or a guide over full-width text when the measure is off (so the "guide-only" experience is `measure=off, wrap_guide=on`). Drawn per visible editing row, under the text (text cells overwrite it where they coincide). **Guard:** only drawn when `gx < area.x + viewport_width` (a guide column past a narrow viewport is simply not shown — no panic, no wrapped column).

## 6. Typewriter scrolling

`view_opts.typewriter` + `typewriter_anchor: f32` (0.0 top … 1.0 bottom, default 0.5). When ON, `nav::ensure_visible` pins the caret's visual row to `anchor_row = (edit_height as f32 * anchor).round()` by choosing `scroll`/`scroll_row` so the caret line sits at `anchor_row`, **clamped** to the valid scroll range (`[0, max_scroll]`). Consequences (v1, deliberate): in the document interior the caret rides the anchor; near the very top the caret sits above the anchor (can't scroll past 0); near the bottom it drifts below (no end virtual-space — future enhancement). When OFF, `ensure_visible` keeps today's minimal-scroll behavior verbatim. Typewriter re-runs on every caret motion (same call sites that already invoke `ensure_visible`).

## 7. Focus dimming

`view_opts.focus` + `focus_granularity` (Paragraph default | Sentence). When ON, render computes the **active region** byte-range at the caret once per frame — `nav::paragraph_range_at(blocks, buf, head)` or (within that paragraph window) `textobj::sentence_bounds` — and paints every editing row whose **source span** falls entirely outside `[from, to)` with a **dim style** (`DarkGray`, no bold/color), full style inside. A row straddling the boundary is dimmed per-cell by source offset (cells whose source offset is outside the region are dim). Composes with the measure (dim is a style decision layered onto the already-positioned row) and with all render modes.

## 8. Word count

- **Pure core (`count.rs`):** `pub fn word_count(text: &str) -> usize` (count of UAX-#29 word segments whose first char is alphanumeric — reuse the `textobj::is_word` rule for consistency) and `pub fn char_count(text: &str) -> usize` (`text.chars().count()`).
- **Display:** when `view_opts.word_count` is ON and no prompt/minibuffer occupies the status row, render appends/right-aligns a `"{words} words · {chars} chars"` segment to the status line. **Selection-aware:** if the primary selection is non-empty, count over `buffer.slice(from..to)`; else over the whole document. Recomputed each frame (cheap for typical docs; if profiling ever shows cost, cache on version — not needed for v1).

## 9. Config, state & commands

- **`[view]` config** (`ViewConfig`, all fields with defaults; `RawView` with `Option<…>` per-field merge in `load()`, mirroring `[mouse]`):
  `typewriter=false`, `typewriter_anchor=0.5`, `focus=false`, `focus_granularity="paragraph"`, `measure=false`, `wrap_column=80`, `wrap_guide=false`, `word_count=false`.
- **`Editor.view_opts: ViewOpts`** holds the runtime values, seeded from `ViewConfig` at startup (in `app::run`, like `mouse_capture`). It is **global** (not per-buffer).
- **Toggle commands** (registered, `MenuCategory::View`, palette/menu-accessible, **no default keybindings** — saves key space; the plan may add chords after verifying they're free): `toggle_typewriter`, `toggle_focus`, `toggle_measure`, `toggle_wrap_guide`, `toggle_word_count`. `toggle_measure` (and any wrap_column change) also clears `view.line_layouts` (§4).

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
- **Config:** `[view]` parses + merges; `ViewOpts` seeded; each `toggle_*` flips its flag (and `toggle_measure` clears `line_layouts`).
- No pre-existing test weakened; full workspace green, zero warnings.

## 12. Module & command summary

- **New file:** `wordcartel-core/src/count.rs`.
- **New state:** `Editor.view_opts: ViewOpts`; `ViewConfig`/`RawView` in config.
- **New helper:** `nav::text_geometry -> TextGeometry { text_left, text_width }` (consumed by render + `screen_pos` + `offset_at_cell`); typewriter-aware `ensure_visible`.
- **New commands:** `toggle_typewriter`, `toggle_focus`, `toggle_measure`, `toggle_wrap_guide`, `toggle_word_count`.
- **Reuses:** 5c `paragraph_range_at`/`sentence_bounds`/`screen_pos`/`offset_at_cell`/`ensure_visible`, the `[mouse]` config pattern, the `RenderMode` status-row precedent.

## 13. Deliberate decisions (for review)

1. **One shared `text_geometry`** consumed by render + `screen_pos` + `offset_at_cell` (no desync) — §4.
2. **Toggling measure / changing wrap_column clears the layout cache** (re-wrap like a resize) — §4.
3. **Typewriter v1 clamps at doc boundaries** (no end virtual-space) — §6.
4. **`measure` and `wrap_guide` are independent toggles sharing `wrap_column`** (guide-only reachable) — §5.
5. **Focus default = paragraph**, sentence config-selectable — §7.
6. **Word count = selection-aware status segment**, toggleable — §8.
7. **All toggles default OFF, palette/menu-only (no default chords); `view_opts` is global** — §9.
8. **Measure applies in all render modes** — §5.
