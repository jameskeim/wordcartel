# Effort 5d — Focus / Writing Experience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add four toggleable, configurable display layers — typewriter scrolling, focus dimming, a centered text measure + wrap-guide, and a live word/char count — atop a shared text-column geometry.

**Architecture:** A single `nav::text_geometry` (`text_left`/`text_width`) is the source of truth for the centered measure, threaded through the layout *producer* (`derive::rebuild`), the on-demand nav layout helpers, render's paint rect + cursor, and `offset_at_cell`. The four features are independent flags on `Editor.view_opts` (seeded from a `[view]` config section), each flipped by a `toggle_*` command.

**Tech Stack:** Rust, `unicode-segmentation` (already in core), `ratatui` styles. No new dependencies.

## Global Constraints

- `#![forbid(unsafe_code)]`; `wordcartel-core` stays IO/thread-free.
- `cargo build --workspace` zero warnings; an item unused until a later task carries a SCOPED per-item `#[allow(dead_code)] // wired in Task N` (never module-level).
- No pre-existing test weakened; `cargo test --workspace` stays green.
- No new dependency.
- The measure's `text_left`/`text_width` come from ONE helper (`nav::text_geometry`) read by every consumer — paint, layout, cursor, and mouse must never disagree.

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `wordcartel-core/src/count.rs` (new) | Pure word/char count | 1 |
| `wordcartel-core/src/lib.rs` | `pub mod count;` | 1 |
| `wordcartel/src/config.rs` | `[view]` → `ViewConfig`/`RawView` + validation | 2 |
| `wordcartel/src/editor.rs` | `Editor.view_opts: ViewConfig` | 2 |
| `wordcartel/src/registry.rs` | 5 `toggle_*` commands + id-uniqueness test | 2 |
| `wordcartel/src/app.rs` | seed `view_opts` from config | 2 |
| `wordcartel/src/nav.rs` | `text_geometry`; measure-aware `screen_pos`/`offset_at_cell`; typewriter `ensure_visible` | 3,5 |
| `wordcartel/src/derive.rs` | `rebuild` uses `text_width` | 3 |
| `wordcartel/src/render.rs` | measure paint rect; wrap-guide; focus dim; word-count segment | 3,4,6,7 |

**Linear order:** 1 (count) → 2 (config/state/toggles) → 3 (measure keystone) → 4 (wrap-guide) → 5 (typewriter) → 6 (focus dim) → 7 (word count).

---

## Task 1: Core `count.rs` — pure word/char count

**Files:**
- Create: `wordcartel-core/src/count.rs`
- Modify: `wordcartel-core/src/lib.rs` (add `pub mod count;`)
- Test: `wordcartel-core/src/count.rs`

**Interfaces:**
- Produces: `pub fn word_count(text: &str) -> usize`; `pub fn char_count(text: &str) -> usize`.

- [ ] **Step 1: Write the failing tests** in `count.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_words_and_chars() {
        assert_eq!(word_count("the quick brown fox"), 4);
        assert_eq!(word_count("don't stop — now"), 3); // contraction = 1 word; em-dash not a word
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("   "), 0);
        assert_eq!(char_count("café"), 4); // 'é' is one char
        assert_eq!(char_count(""), 0);
    }
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel-core --lib count::` → FAIL.

- [ ] **Step 3: Implement `count.rs`:**

```rust
//! Pure word/char counts (UAX-#29 word segments; alphanumeric-first rule
//! matches textobj::is_word for consistency).
use unicode_segmentation::UnicodeSegmentation;

/// Number of word segments whose first char is alphanumeric.
pub fn word_count(text: &str) -> usize {
    text.split_word_bounds()
        .filter(|seg| seg.chars().next().is_some_and(char::is_alphanumeric))
        .count()
}

/// Number of Unicode scalar values.
pub fn char_count(text: &str) -> usize {
    text.chars().count()
}
```

Add `pub mod count;` to `wordcartel-core/src/lib.rs` after the other `pub mod` lines.

- [ ] **Step 4: Run tests + build.** `cargo test -p wordcartel-core --lib count::` → PASS; `cargo build -p wordcartel-core` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel-core/src/count.rs wordcartel-core/src/lib.rs
git commit -m "feat(core): count — pure word/char count (UAX-29)"
```

---

## Task 2: `[view]` config + `view_opts` state + toggle commands

**Files:**
- Modify: `wordcartel/src/config.rs` (`ViewConfig`/`FocusGranularity`/`RawView` + merge), `wordcartel/src/editor.rs` (`Editor.view_opts`), `wordcartel/src/registry.rs` (commands + id-uniqueness test), `wordcartel/src/app.rs` (seed)
- Test: `wordcartel/src/config.rs`, `wordcartel/src/registry.rs`

**Interfaces:**
- Produces: `config::ViewConfig { typewriter: bool, typewriter_anchor: f32, focus: bool, focus_granularity: FocusGranularity, measure: bool, wrap_column: u16, wrap_guide: bool, word_count: bool }` (`Clone`, `Default`); `config::FocusGranularity { Paragraph, Sentence }` (`Clone, Copy, PartialEq`); `Editor.view_opts: ViewConfig`. Command ids `toggle_typewriter`, `toggle_focus`, `toggle_measure`, `toggle_wrap_guide`, `toggle_word_count`.

- [ ] **Step 1: Write the failing tests.** In `config.rs`:

```rust
    #[test]
    fn view_config_parses_and_validates() {
        // NOTE: there is NO `tempfile` dependency. Feed `load()` the SAME way the
        // existing config tests at config.rs:202+ do (they already write a temp
        // file and call `load(&[path])`, or build input directly — mirror that
        // exact pattern; do NOT add a tempfile dep). The toml under test:
        let toml = r#"
            [view]
            measure = true
            wrap_column = 5
            typewriter_anchor = 1.5
            focus_granularity = "bogus"
        "#;
        // ... write `toml` to a path the way the existing tests do, then: ...
        let (cfg, warnings) = load(&[path]);
        assert!(cfg.view.measure);
        assert_eq!(cfg.view.wrap_column, 20, "wrap_column clamped to min 20");
        assert_eq!(cfg.view.typewriter_anchor, 1.0, "anchor clamped to <=1.0");
        assert_eq!(cfg.view.focus_granularity, FocusGranularity::Paragraph, "bad granularity -> default");
        assert!(warnings.iter().any(|w| w.contains("wrap_column")));
        assert!(warnings.iter().any(|w| w.contains("focus_granularity")));
    }
```

(Read the existing config tests (config.rs:202+) FIRST and reuse their exact file-feeding helper — they already exercise `load(&[path])` without a tempfile crate, e.g. via `std::env::temp_dir()` + a unique name. The assertions on clamp/fallback/warnings are the point.) In `registry.rs`:

```rust
    #[test]
    fn builtin_command_ids_are_unique() {
        let reg = Registry::builtins();
        let mut seen = std::collections::HashSet::new();
        for (id, _) in reg.commands() {
            assert!(seen.insert(id.0), "duplicate command id: {}", id.0);
        }
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib config::tests::view_config registry::tests::builtin_command_ids` → FAIL.

- [ ] **Step 3: Implement.**
  - `config.rs` — types + merge (mirror `MouseConfig`/`RawMouse`, config.rs:40-48/108):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusGranularity { Paragraph, Sentence }

#[derive(Debug, Clone)]
pub struct ViewConfig {
    pub typewriter: bool,
    pub typewriter_anchor: f32,
    pub focus: bool,
    pub focus_granularity: FocusGranularity,
    pub measure: bool,
    pub wrap_column: u16,
    pub wrap_guide: bool,
    pub word_count: bool,
}
impl Default for ViewConfig {
    fn default() -> Self {
        ViewConfig { typewriter: false, typewriter_anchor: 0.5, focus: false,
            focus_granularity: FocusGranularity::Paragraph, measure: false,
            wrap_column: 80, wrap_guide: false, word_count: false }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawView {
    typewriter: Option<bool>,
    typewriter_anchor: Option<f32>,
    focus: Option<bool>,
    focus_granularity: Option<String>,
    measure: Option<bool>,
    wrap_column: Option<u16>,
    wrap_guide: Option<bool>,
    word_count: Option<bool>,
}
```

   Add `pub view: ViewConfig` to `Config` (config.rs:34) and `view: RawView` to `RawConfig` (config.rs:88). In `load()` (after the `[mouse]` merge ~config.rs:185), per-field merge + validation:

```rust
        if let Some(v) = raw.view.typewriter { cfg.view.typewriter = v; }
        if let Some(v) = raw.view.focus { cfg.view.focus = v; }
        if let Some(v) = raw.view.measure { cfg.view.measure = v; }
        if let Some(v) = raw.view.wrap_guide { cfg.view.wrap_guide = v; }
        if let Some(v) = raw.view.word_count { cfg.view.word_count = v; }
        if let Some(a) = raw.view.typewriter_anchor {
            if (0.0..=1.0).contains(&a) { cfg.view.typewriter_anchor = a; }
            else { cfg.view.typewriter_anchor = a.clamp(0.0, 1.0);
                   warnings.push(format!("view.typewriter_anchor {a} out of 0.0..=1.0; clamped")); }
        }
        if let Some(c) = raw.view.wrap_column {
            if c >= 20 { cfg.view.wrap_column = c; }
            else { cfg.view.wrap_column = 20;
                   warnings.push(format!("view.wrap_column {c} below min 20; clamped to 20")); }
        }
        if let Some(g) = raw.view.focus_granularity {
            match g.as_str() {
                "paragraph" => cfg.view.focus_granularity = FocusGranularity::Paragraph,
                "sentence"  => cfg.view.focus_granularity = FocusGranularity::Sentence,
                other => warnings.push(format!("view.focus_granularity \"{other}\" invalid; using paragraph")),
            }
        }
```

   (Match the real `warnings` variable name in `load()` — it returns `(Config, Vec<String>)`.)
  - `editor.rs` — `pub view_opts: crate::config::ViewConfig` on `Editor` (init `crate::config::ViewConfig::default()` in `new_from_text`).
  - `app.rs` — seed after the mouse seed (app.rs:886): `editor.view_opts = cfg.view.clone();`.
  - `registry.rs` — register (after the existing View-category commands):

```rust
        r.register("toggle_typewriter", "Toggle Typewriter", Some(MenuCategory::View), |c| { c.editor.view_opts.typewriter = !c.editor.view_opts.typewriter; CommandResult::Handled });
        r.register("toggle_focus",      "Toggle Focus Mode", Some(MenuCategory::View), |c| { c.editor.view_opts.focus = !c.editor.view_opts.focus; CommandResult::Handled });
        r.register("toggle_measure",    "Toggle Centered Measure", Some(MenuCategory::View), |c| { c.editor.view_opts.measure = !c.editor.view_opts.measure; c.editor.active_mut().view.line_layouts.clear(); CommandResult::Handled });
        r.register("toggle_wrap_guide", "Toggle Wrap Guide", Some(MenuCategory::View), |c| { c.editor.view_opts.wrap_guide = !c.editor.view_opts.wrap_guide; CommandResult::Handled });
        r.register("toggle_word_count", "Toggle Word Count", Some(MenuCategory::View), |c| { c.editor.view_opts.word_count = !c.editor.view_opts.word_count; CommandResult::Handled });
```

   (`toggle_measure` clears `line_layouts` so the next `derive::rebuild` re-wraps at the new width — Task 3 makes the width depend on `measure`.)

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib config:: registry::` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings. (Several `view_opts` fields are read only in Tasks 3-7 → no `#[allow(dead_code)]` needed since the struct is public + constructed.)

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/config.rs wordcartel/src/editor.rs wordcartel/src/registry.rs wordcartel/src/app.rs
git commit -m "feat(view): [view] config + ViewConfig validation + view_opts + 5 toggle commands"
```

---

## Task 3: The keystone — centered measure (`text_geometry` threaded everywhere)

**Files:**
- Modify: `wordcartel/src/nav.rs` (`text_geometry`, `offset_at_cell`), `wordcartel/src/derive.rs` (`rebuild` width), `wordcartel/src/render.rs` (paint rect + cursor)
- Test: `wordcartel/src/nav.rs`

**Interfaces:**
- Consumes: `Editor.view_opts` (Task 2).
- Produces: `pub struct nav::TextGeometry { pub text_left: u16, pub text_width: u16 }`; `pub fn nav::text_geometry(editor: &Editor) -> TextGeometry`.

- [ ] **Step 1: Write the failing tests** in `nav.rs`:

```rust
    #[test]
    fn text_geometry_centers_when_measure_on() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        let g = super::text_geometry(&e);
        assert_eq!((g.text_left, g.text_width), (0, 80), "measure off → full width");
        e.view_opts.measure = true; e.view_opts.wrap_column = 40;
        let g = super::text_geometry(&e);
        assert_eq!((g.text_left, g.text_width), (20, 40), "centered 40-wide column");
        // narrow terminal: measure inert
        e.active_mut().view.area = (30, 24);
        let g = super::text_geometry(&e);
        assert_eq!((g.text_left, g.text_width), (0, 30), "vp <= column → full width");
    }

    #[test]
    fn screen_pos_and_offset_at_cell_round_trip_with_measure() {
        let mut e = Editor::new_from_text("abc\ndef\n", None, (80, 24));
        e.view_opts.measure = true; e.view_opts.wrap_column = 40; // text_left = 20
        set_caret(&mut e, 5); // 'e' in "def" (line 1, text-col 1)
        derive::rebuild(&mut e);
        let (vcol, vrow) = screen_pos(&e).unwrap();
        // the actual SCREEN cell is (text_left + vcol, vrow)
        assert_eq!(super::offset_at_cell(&e, 20 + vcol, vrow), Some(5));
        // a click in the LEFT margin clamps to line start of that row
        assert_eq!(super::offset_at_cell(&e, 3, vrow), Some(3)); // "def" line start = offset 4? -> see note
    }
```

(Note: the left-margin assertion's expected offset is the start of the clicked row; compute the real value from the doc and adjust — the POINT is "margin click clamps to line start, no panic", not the literal number. The round-trip assertion `offset_at_cell(20+vcol, vrow) == Some(5)` is the load-bearing one.)

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib nav::tests::text_geometry nav::tests::screen_pos_and_offset_at_cell_round_trip` → FAIL.

- [ ] **Step 3: Implement.**
  - `nav.rs` — the helper:

```rust
pub struct TextGeometry { pub text_left: u16, pub text_width: u16 }

/// The text column's left edge (relative to area.x) and width for this frame.
pub fn text_geometry(editor: &Editor) -> TextGeometry {
    let vp = editor.active().view.area.0;
    let o = &editor.view_opts;
    if o.measure && vp > o.wrap_column && o.wrap_column > 0 {
        let text_width = o.wrap_column;
        TextGeometry { text_left: (vp - text_width) / 2, text_width }
    } else {
        TextGeometry { text_left: 0, text_width: vp.max(1) }
    }
}
```

  - `nav.rs` — the two on-demand layout sites (nav.rs:40 and nav.rs:111) change `let vp_width = (editor.active().view.area.0 as usize).max(1);` → `let vp_width = text_geometry(editor).text_width as usize;`.
  - `nav.rs` — `offset_at_cell(editor, col, row)` (nav.rs:672): subtract `text_left` from the incoming `col` as the FIRST step, so the rest of the function works in text-relative columns: `let text_left = text_geometry(editor).text_left; let col = col.saturating_sub(text_left);` (a click left of the column → 0 = line start; the existing visual_to_source clamps the right side to the line end).
  - `derive.rs` — `rebuild`: replace `let vp_width = area_width.max(1);` (derive.rs:130) with `let vp_width = crate::nav::text_geometry(editor).text_width as usize;` (compute it before the cache-clear/loop; it returns an owned value so no borrow conflict with the later `active_mut()`).
  - `render.rs` — compute `let tg = crate::nav::text_geometry(editor);` once; change each visible-row paint rect from `Rect::new(area.x, edit_top + screen_row, w, 1)` (render.rs:167) to `Rect::new(area.x + tg.text_left, edit_top + screen_row, tg.text_width, 1)`; and the hardware-cursor placement (render.rs:252-253) from `area.x + col` to `area.x + tg.text_left + col`.

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib nav:: render::` → PASS; `cargo test --workspace` → green (the existing screen_pos/offset_at_cell/render tests still pass with measure OFF — the default); `cargo build --workspace` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/nav.rs wordcartel/src/derive.rs wordcartel/src/render.rs
git commit -m "feat(view): centered measure — text_geometry threaded through derive/render/screen_pos/offset_at_cell"
```

---

## Task 4: Wrap-guide line

**Files:** Modify `wordcartel/src/render.rs`. Test: `wordcartel/src/render.rs`.

**Interfaces:** Consumes `nav::text_geometry` (Task 3), `view_opts.wrap_guide`/`wrap_column`, `mouse.scrollbar_visible`.

- [ ] **Step 1: Write the failing test** in `render.rs` (a render-buffer test if the file has them, else a geometry assertion):

```rust
    #[test]
    fn wrap_guide_column_position() {
        // measure off, wrap_guide on, column 40, viewport 80 → guide at screen col 40
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.view_opts.wrap_guide = true; e.view_opts.wrap_column = 40;
        let tg = crate::nav::text_geometry(&e);
        let gx = tg.text_left + e.view_opts.wrap_column;
        assert_eq!(gx, 40);
        assert!(gx < e.active().view.area.0, "guide within viewport");
        // measure on, column 40, viewport 80 → text_left 20, guide at 60 (right edge)
        e.view_opts.measure = true;
        let tg = crate::nav::text_geometry(&e);
        assert_eq!(tg.text_left + e.view_opts.wrap_column, 60);
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib render::tests::wrap_guide` → FAIL.

- [ ] **Step 3: Implement** in `render.rs` (after the text rows are painted, before/under the cursor): when `editor.view_opts.wrap_guide`, compute `gx = area.x + tg.text_left + editor.view_opts.wrap_column`; if `gx < area.x + w` AND (scrollbar hidden OR `gx != area.x + w - 1`), paint a dim `│` at column `gx` for each editing row (`Rect::new(gx, edit_top + r, 1, 1)` with a `DarkGray` styled `│`, for `r in 0..edit_height`). Text already painted at those cells (Task 3) overwrites the guide where they coincide — paint the guide BEFORE the text rows, or only on cells the text doesn't occupy. Simplest: paint the guide line FIRST (before the row loop) so text overwrites it; a guide cell with no text shows through.

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib render::` → PASS; `cargo test --workspace` → green; zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/render.rs
git commit -m "feat(view): wrap-guide line (shares wrap_column; skips scrollbar/past-viewport)"
```

---

## Task 5: Typewriter scrolling

**Files:** Modify `wordcartel/src/nav.rs` (`ensure_visible`). Test: `wordcartel/src/nav.rs`.

**Interfaces:** Consumes `view_opts.typewriter`/`typewriter_anchor`; existing `rows_before_caret`/`rows_of_line`/`caret_visual_row`/`advance_view_top_one_row`.

- [ ] **Step 1: Write the failing test** in `nav.rs`:

```rust
    #[test]
    fn typewriter_pins_caret_to_anchor_row() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 21)); // edit_height = 20
        e.view_opts.typewriter = true; e.view_opts.typewriter_anchor = 0.5; // anchor_row = 10
        let l50 = derive::line_start(&e.active().document.buffer, 50);
        set_caret(&mut e, l50);
        ensure_visible(&mut e);
        derive::rebuild(&mut e);
        let (_c, row) = screen_pos(&e).unwrap();
        assert_eq!(row, 10, "caret pinned to anchor row 10");
        // near the top, caret sits ABOVE the anchor (can't scroll past 0)
        set_caret(&mut e, derive::line_start(&e.active().document.buffer, 2));
        ensure_visible(&mut e);
        derive::rebuild(&mut e);
        assert_eq!(e.active().view.scroll, 0, "top clamps; no scroll past 0");
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib nav::tests::typewriter_pins` → FAIL.

- [ ] **Step 3: Implement** in `ensure_visible` (nav.rs:352): at the top, branch on `editor.view_opts.typewriter`:

```rust
    if editor.view_opts.typewriter {
        let edit_height = (editor.active().view.area.1 as usize).saturating_sub(1);
        if edit_height == 0 { return; }
        let anchor = editor.view_opts.typewriter_anchor.clamp(0.0, 1.0);
        let anchor_row = ((edit_height as f32 * anchor).round() as usize).min(edit_height - 1);
        // caret's absolute visual row = (visual rows of all logical lines before its line) + its vrow
        let l = caret_line(editor);
        let cvr = caret_visual_row(editor, l);
        let mut caret_abs = cvr;
        for li in 0..l { caret_abs += rows_of_line(editor, li); }
        // desired viewport-top absolute visual row
        let target_top = caret_abs.saturating_sub(anchor_row);
        // convert target_top → (scroll, scroll_row), walking logical lines
        let mut acc = 0usize; let mut scroll = 0usize; let mut scroll_row = 0usize;
        let total = derive::total_logical_lines(&editor.active().document.buffer);
        'outer: for li in 0..total {
            let rows = rows_of_line(editor, li);
            if acc + rows > target_top { scroll = li; scroll_row = target_top - acc; break 'outer; }
            acc += rows; scroll = li; scroll_row = rows.saturating_sub(1);
        }
        editor.active_mut().view.scroll = scroll;
        editor.active_mut().view.scroll_row = scroll_row;
        return;
    }
    // ... existing minimal-scroll body unchanged ...
```

(Walking all logical lines for `caret_abs` is O(lines-before-caret); acceptable for v1. If a huge-doc perf issue ever appears, cap the walk — not needed now. The `target_top` naturally clamps at 0 via `saturating_sub`; near the bottom the caret drifts below the anchor because `target_top` can exceed the last valid top, which the `for` loop bounds to the last line — the deliberate v1 boundary behavior.)

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib nav::` → PASS; `cargo test --workspace` → green (typewriter OFF default → existing ensure_visible tests unchanged); zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/nav.rs
git commit -m "feat(view): typewriter scrolling — pin caret to anchor row in visual-row space"
```

---

## Task 6: Focus dimming (row-level)

**Files:** Modify `wordcartel/src/render.rs`. Test: `wordcartel/src/render.rs`.

**Interfaces:** Consumes `view_opts.focus`/`focus_granularity`; `nav::paragraph_range_at`, `textobj::sentence_bounds`; `derive::line_start`; `VisualRow.src_span`.

- [ ] **Step 1: Write the failing test** in `render.rs`:

```rust
    #[test]
    fn focus_active_region_is_paragraph_at_caret() {
        let mut e = Editor::new_from_text("Para one.\n\nPara two.\n\nThree.\n", None, (80, 24));
        e.view_opts.focus = true; // paragraph default
        set_caret(&mut e, 12); // inside "Para two."
        derive::rebuild(&mut e);
        // the active region used by render = paragraph_range_at at the caret
        let buf = &e.active().document.buffer; let blocks = &e.active().document.blocks;
        let (from, to) = crate::nav::paragraph_range_at(blocks, buf, 12);
        assert_eq!(buf.slice(from..to).trim(), "Para two.");
        // a row whose global src span is outside [from,to) is dimmed; inside is bright.
        // (assert the helper render uses to decide, not pixels — see Step 3 for the fn)
        assert!(!crate::render::row_is_active(0, "Para one.".len(), from, to), "para one dimmed");
        assert!(crate::render::row_is_active(from, to, from, to), "active row bright");
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib render::tests::focus_active_region` → FAIL.

- [ ] **Step 3: Implement** in `render.rs`:
  - A small pure helper (testable): `pub(crate) fn row_is_active(row_from: usize, row_to: usize, region_from: usize, region_to: usize) -> bool { row_from < region_to && region_from < row_to }` (half-open intersection).
  - Before the row loop, when `editor.view_opts.focus`, compute the active region once: `head = nav::head(editor)`; `let (from, to) = match editor.view_opts.focus_granularity { Paragraph => nav::paragraph_range_at(blocks, buf, head), Sentence => { let (ps, pe) = nav::paragraph_range_at(blocks, buf, head); let win = buf.slice(ps..pe); let (sf, st) = wordcartel_core::textobj::sentence_bounds(&win, head - ps); (ps + sf, ps + st) } };`
  - In the row loop, for visible logical line `l` and its `VisualRow` `vr`: global span = `derive::line_start(buf, l) + vr.src_span.start .. derive::line_start(buf, l) + vr.src_span.end`. If focus is on and `!row_is_active(g_from, g_to, from, to)`, paint that row's `Paragraph` with a dim style (`RStyle::default().fg(Color::DarkGray)`) instead of its normal style. (Apply the dim as a style override on the row's rendered line — wrap the existing styled spans' style with DarkGray, or render the row text in DarkGray.)

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib render::` → PASS; `cargo test --workspace` → green (focus OFF default); zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/render.rs
git commit -m "feat(view): focus dimming — dim rows outside the active paragraph/sentence (row-level)"
```

---

## Task 7: Word-count status segment

**Files:** Modify `wordcartel/src/render.rs`. Test: `wordcartel/src/render.rs`.

**Interfaces:** Consumes `wordcartel_core::count::{word_count, char_count}` (Task 1), `view_opts.word_count`, the primary selection.

- [ ] **Step 1: Write the failing test** in `render.rs`:

```rust
    #[test]
    fn word_count_segment_selection_aware() {
        let mut e = Editor::new_from_text("alpha beta gamma\n", None, (80, 24));
        e.view_opts.word_count = true;
        // whole doc: 3 words
        assert_eq!(crate::render::word_count_segment(&e), Some("3 words · 17 chars".to_string()));
        // select "alpha" → 1 word, 5 chars
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5);
        assert_eq!(crate::render::word_count_segment(&e), Some("1 words · 5 chars".to_string()));
        e.view_opts.word_count = false;
        assert_eq!(crate::render::word_count_segment(&e), None);
    }
```

(Adjust the whole-doc char count to the real value of `"alpha beta gamma\n"` — 17 including the newline; if the count should exclude the trailing newline, count over `buffer.to_string()` as-is and match. Pick one and make the test assert the real value.)

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib render::tests::word_count_segment` → FAIL.

- [ ] **Step 3: Implement** in `render.rs`:
  - `pub(crate) fn word_count_segment(editor: &Editor) -> Option<String>`: if `!editor.view_opts.word_count` → `None`; else let `sel = editor.active().document.selection.primary()`; `let text = if !sel.is_empty() { editor.active().document.buffer.slice(sel.from()..sel.to()) } else { editor.active().document.buffer.to_string() };` → `Some(format!("{} words · {} chars", count::word_count(&text), count::char_count(&text)))`.
  - In the status-row composition (render.rs:211-234), when not showing a prompt/minibuffer and `word_count_segment(editor)` is `Some(right)`: compose left+right — `let reserve = right.chars().count() + 1; let left: String = status_text.chars().take(w.saturating_sub(reserve as u16) as usize).collect(); let pad = (w as usize).saturating_sub(left.chars().count() + right.chars().count()); let composed = format!("{left}{}{right}", " ".repeat(pad));` then render `composed` (still REVERSED, truncated to `w`).

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib render::` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/render.rs
git commit -m "feat(view): live selection-aware word/char count status segment"
```

---

## Self-Review

**Spec coverage:** §2 deps (no new) ✅; §3 modules (count T1, config/view_opts/toggles T2, nav/derive/render keystone T3, render T4/6/7, nav T5) ✅; §4 keystone — text_geometry in derive::rebuild + nav sites + render paint rect + screen_pos/offset_at_cell + cache-clear on toggle (T3, clear in T2's toggle_measure) ✅; §5 measure + wrap-guide (T3/T4, guide skips scrollbar/past-viewport) ✅; §6 typewriter visual-row + drag-exempt (T5; drag already doesn't call ensure_visible) ✅; §7 focus row-level dim + global-span via line_start (T6) ✅; §8 word count selection-aware + left/right status composition (T1/T7) ✅; §9 [view] config + validation + view_opts global + toggles no-chords + id-uniqueness (T2) ✅; §10 edges (wrap_column<vp inert, margin-click clamp, guide guard) covered in T3/T4/config ✅; §11 tests per task ✅.

**Placeholder scan:** the focus-dim "apply dim as a style override on the row's rendered line" (T6 Step 3) and the word-count "adjust char count to real value" (T7) are concrete instructions with the helper fns fully specified (`row_is_active`, `word_count_segment`); the implementer fills the exact `RStyle`/count by reading the real row-paint + buffer. No TBD/empty steps.

**Type consistency:** `ViewConfig`/`FocusGranularity` fields, `Editor.view_opts`, `nav::text_geometry`/`TextGeometry{text_left,text_width}`, the five `toggle_*` ids, `render::{row_is_active, word_count_segment}`, `count::{word_count,char_count}`, and the `derive::rebuild` width source are used identically across the tasks that define and consume them.
