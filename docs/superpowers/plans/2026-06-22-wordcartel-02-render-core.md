# Wordcartel Render Core — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the **render core** to `wordcartel-core`: an inline-markdown conceal/style analyzer (`md_parse`) and the spike-validated `layout`/`ColMap`/cursor-navigation subsystem — so that, given a logical line of markdown + a cursor + a viewport width, we can produce styled soft-wrapped visual rows and move the cursor correctly amid concealed markers.

**Architecture:** Pure, headless (same crate, no terminal). `md_parse` turns one logical line (with its block role supplied) into conceal **runs** (visible vs hidden source bytes) + inline **style spans**, using `pulldown-cmark`'s offset iterator. `layout` consumes the conceal runs to produce `VisualRow`s + a `ColMap` (source-byte ↔ visual-`(row,col)`), and cursor navigation uses the cursor's explicit visual-row affinity (the spike's key finding). Block-role *determination* is deferred to Plan 3 (`block_tree`); here the role is an input.

**Tech Stack:** Rust 2021; `pulldown-cmark` (new); existing `unicode-segmentation`, `unicode-width`; `proptest`. Builds on Effort 1's `TextBuffer`/`BytePos`.

## Global Constraints

- Same crate `wordcartel-core`; `#![forbid(unsafe_code)]`; pure/headless (no `std::io`, threads, terminal).
- Canonical position = **byte offset** into the logical line (`usize`), matching §16.1.
- New dep: **`pulldown-cmark = "0.12"`** with GFM strikethrough enabled (`Options::ENABLE_STRIKETHROUGH`).
- The `layout`/`ColMap`/`Cursor`/navigation code is **ported from the validated spike** at `~/projects/wordcartel-layout-spike/src/lib.rs` (the implementer reads that file — it is real, property-tested reference code). Port verbatim except where a step says to adapt the signature to consume `md_parse` output. Preserve the spike's documented policies (grapheme atom; concealed bytes absent from `placed`; wide-cell owns `[col,col+w)`; zero-width shares cell with positive-width winning; tab = fixed `TAB_WIDTH`; wrap-boundary resolves toward next row; cursor carries `{offset,row,desired_col}` affinity).
- v1 **inline** construct set (§13.3): emphasis, strong, bold-italic, inline code, strikethrough (GFM), link, escape. **Block-role rendering (headings/lists/quotes) and block-role *determination* are out of scope here** (Plan 3); `BlockRole` is carried as data but only `Paragraph` behavior is exercised.
- TDD; pristine output; `proptest` for the round-trip laws; commit `proptest-regressions/` seeds.

---

## Reuse Posture

Layout/ColMap/cursor is **ported from our own validated spike** (the hard, spike-proven code — maximal reuse of work already done and tested). `md_parse` is new but thin: it depends on `pulldown-cmark` (the parser — reused) and only computes conceal/style spans from its offset events. We write the span-mapping glue, not a parser.

---

## File Structure

- `wordcartel-core/Cargo.toml` — add `pulldown-cmark`.
- `wordcartel-core/src/lib.rs` — declare `style`, `md_parse`, `layout` modules.
- `wordcartel-core/src/style.rs` — `Style`, `BlockRole`, `StyleSpan`, `Run`, `LineAnalysis`.
- `wordcartel-core/src/md_parse.rs` — `analyze(line, role, is_active) -> LineAnalysis`.
- `wordcartel-core/src/layout.rs` — `Placed`, `VisualRow`, `ColMap`, `layout()`, `Cursor`, navigation.
- `wordcartel-core/tests/render_integration.rs` — multi-line doc + cursor navigation.

---

### Task 0: Render scaffold + pulldown-cmark dependency

**Files:** Modify `Cargo.toml`, `src/lib.rs`; Create `src/style.rs`, `src/md_parse.rs`, `src/layout.rs` (stubs).

**Interfaces:** Produces a compiling crate with the new modules declared and `pulldown-cmark` available.

- [ ] **Step 1:** Add to `wordcartel-core/Cargo.toml` under `[dependencies]`: `pulldown-cmark = "0.12"`.
- [ ] **Step 2:** In `src/lib.rs`, add module declarations after the existing ones:
```rust
pub mod layout;
pub mod md_parse;
pub mod style;
```
- [ ] **Step 3:** Create `src/style.rs`, `src/md_parse.rs`, `src/layout.rs` each containing only `// filled in by later tasks`.
- [ ] **Step 4:** Run `cargo build --manifest-path wordcartel-core/Cargo.toml`. Expected: clean build (pulldown-cmark downloads).
- [ ] **Step 5:** Commit: `git add wordcartel-core && git commit -m "chore(core): scaffold render modules + pulldown-cmark dep"`

---

### Task 1: Style / BlockRole / span types

**Files:** Modify `src/style.rs`.

**Interfaces — Produces:**
- `enum Style { Plain, Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link }`
- `enum BlockRole { Paragraph, Heading(u8), BlockQuote, ListItem, CodeBlock, ThematicBreak, FrontMatter }`
- `struct StyleSpan { src: std::ops::Range<usize>, style: Style }`
- `struct Run { src: std::ops::Range<usize>, visible: bool }`
- `struct LineAnalysis { runs: Vec<Run>, styles: Vec<StyleSpan>, role: BlockRole }`
- all `#[derive(Clone, Debug, PartialEq, Eq)]`.

- [ ] **Step 1: Write the failing test** in `src/style.rs`:
```rust
//! Inline style + block-role types shared by md_parse and layout.
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style { Plain, Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockRole { Paragraph, Heading(u8), BlockQuote, ListItem, CodeBlock, ThematicBreak, FrontMatter }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleSpan { pub src: Range<usize>, pub style: Style }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run { pub src: Range<usize>, pub visible: bool }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineAnalysis { pub runs: Vec<Run>, pub styles: Vec<StyleSpan>, pub role: BlockRole }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn types_construct() {
        let a = LineAnalysis {
            runs: vec![Run { src: 0..3, visible: true }],
            styles: vec![StyleSpan { src: 0..3, style: Style::Strong }],
            role: BlockRole::Paragraph,
        };
        assert_eq!(a.runs.len(), 1);
        assert_eq!(a.styles[0].style, Style::Strong);
        assert_eq!(a.role, BlockRole::Paragraph);
    }
}
```
- [ ] **Step 2:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml style` → FAIL (module empty).
- [ ] **Step 3:** The code above IS the implementation (types + test together). Confirm it compiles.
- [ ] **Step 4:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml style` → PASS.
- [ ] **Step 5:** Commit: `feat(core): style/block-role/span types for render core`

---

### Task 2: md_parse — inline conceal + style analysis

**Files:** Modify `src/md_parse.rs`.

**Interfaces:**
- Consumes: `crate::style::{Style, BlockRole, StyleSpan, Run, LineAnalysis}`.
- Produces: `pub fn analyze(line: &str, role: BlockRole, is_active: bool) -> LineAnalysis`.

**Behavior:** If `is_active`, return one visible run `0..line.len()`, empty styles, the given role (raw — active line shows source). Otherwise parse `line` with `pulldown-cmark` (`Options::ENABLE_STRIKETHROUGH`) via `into_offset_iter()` and compute:
- **Conceal** the markers of: Strong (`**`/`__`), Emphasis (`*`/`_`), Strikethrough (`~~`), inline Code (leading/trailing backticks only), and Link (`[`, `](url)` — keep the link text). Build a per-byte `visible` grid (like the spike), conceal marker spans, re-reveal `Event::Text`/`Event::Code` content, then collapse to `Run`s.
- **Style spans** over the *visible content*: Strong→`Style::Strong`, Emphasis→`Style::Emphasis`, both nested→`Style::StrongEmphasis`, Strikethrough→`Style::Strikethrough`, Code content→`Style::Code`, Link text→`Style::Link`. Compute by tracking the active emphasis/strong/strike/link/code nesting between Start/End events and assigning the style of the innermost-with-combining to each `Event::Text`/`Event::Code` text range.

Read the spike's `analyze` (`~/projects/wordcartel-layout-spike/src/lib.rs:172-250`) as the reference for the conceal-grid technique; extend it with strikethrough, escapes, and the style-span capture.

- [ ] **Step 1: Write failing tests** in `src/md_parse.rs`:
```rust
//! Inline markdown conceal + style analysis for one logical line.
//! Conceal-grid technique adapted from the validated layout spike.
use crate::style::{BlockRole, LineAnalysis, Run, Style, StyleSpan};
use std::ops::Range;

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: the concatenated visible source (bytes whose run.visible == true).
    fn visible(a: &LineAnalysis, line: &str) -> String {
        let mut s = String::new();
        for r in &a.runs { if r.visible { s.push_str(&line[r.src.clone()]); } }
        s
    }
    // Helper: the style covering a given visible byte, if any.
    fn style_at(a: &LineAnalysis, b: usize) -> Option<Style> {
        a.styles.iter().find(|s| s.src.contains(&b)).map(|s| s.style)
    }

    #[test]
    fn active_line_is_raw() {
        let line = "a **b** c";
        let a = analyze(line, BlockRole::Paragraph, true);
        assert_eq!(a.runs, vec![Run { src: 0..line.len(), visible: true }]);
        assert!(a.styles.is_empty());
    }

    #[test]
    fn strong_conceals_markers_keeps_text_with_style() {
        let line = "a **bold** c"; // 'b' of bold is at byte 5
        let a = analyze(line, BlockRole::Paragraph, false);
        assert_eq!(visible(&a, line), "a bold c"); // ** hidden
        assert_eq!(style_at(&a, 5), Some(Style::Strong));
    }

    #[test]
    fn emphasis_and_code_and_strike() {
        let line = "*i* `c` ~~s~~";
        let a = analyze(line, BlockRole::Paragraph, false);
        assert_eq!(visible(&a, line), "i c s");
        assert_eq!(style_at(&a, 1), Some(Style::Emphasis));   // 'i'
        assert_eq!(style_at(&a, 5), Some(Style::Code));       // 'c'
        assert_eq!(style_at(&a, 10), Some(Style::Strikethrough)); // 's'
    }

    #[test]
    fn link_hides_target_keeps_text() {
        let line = "see [docs](http://x.io) now";
        let a = analyze(line, BlockRole::Paragraph, false);
        assert_eq!(visible(&a, line), "see docs now");
        // 'd' of docs is at byte 5
        assert_eq!(style_at(&a, 5), Some(Style::Link));
    }

    #[test]
    fn bold_italic_is_strong_emphasis() {
        let line = "***x***"; // 'x' at byte 3
        let a = analyze(line, BlockRole::Paragraph, false);
        assert_eq!(visible(&a, line), "x");
        assert_eq!(style_at(&a, 3), Some(Style::StrongEmphasis));
    }
}
```
- [ ] **Step 2:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml md_parse` → FAIL (no `analyze`).
- [ ] **Step 3: Implement `analyze`.** Port the spike's conceal-grid approach and extend it. Reference structure (write this, adapting the spike):
  - Early return for `is_active` and empty line.
  - `let mut visible = vec![true; line.len()];` and `let mut styles: Vec<StyleSpan> = Vec::new();`
  - Track nesting with counters: `strong`, `em`, `strike`, `link`, and for code use `Event::Code`. Build the parser with strikethrough enabled:
    ```rust
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(line, opts).into_offset_iter();
    ```
  - On `Start(Strong|Emphasis|Strikethrough)` / `Start(Link)`: push the whole event `range` to a `conceal` list and increment the matching counter (link also conceals; its text re-reveals).
  - On `End(...)`: decrement the counter.
  - On `Event::Text(_)` with `range`: push to `reveal`; and push a `StyleSpan { src: range, style }` where `style` is derived from the current counters: `strong>0 && em>0 → StrongEmphasis; strong>0 → Strong; em>0 → Emphasis; strike>0 → Strikethrough; link>0 → Link; else → Plain` (skip pushing for `Plain`).
  - On `Event::Code(_)` with `range`: conceal the leading/trailing backtick fences (as the spike does), reveal the inner content, and push a `StyleSpan` for the inner content with `Style::Code`.
  - Apply `conceal` (set false) then `reveal` (set true) onto the `visible` grid (reveal wins), exactly as the spike does, then collapse to `Run`s.
  - Return `LineAnalysis { runs, styles, role }`.
- [ ] **Step 4:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml md_parse` → PASS (5 tests).
- [ ] **Step 5:** Commit: `feat(core): md_parse inline conceal + style analysis`

---

### Task 3: layout types + layout() soft-wrap

**Files:** Modify `src/layout.rs`.

**Interfaces:**
- Consumes: `crate::md_parse::analyze`, `crate::style::{LineAnalysis, Run, Style, StyleSpan, BlockRole}`, `unicode-segmentation`, `unicode-width`.
- Produces: `pub const TAB_WIDTH: usize = 4;`, `pub struct Placed { src, row, col, width, text, style }`, `pub struct VisualRow { display, width, src_span }`, `pub struct ColMap { placed, rows, eol, row_end_col, is_active }`, and `pub fn layout(line: &str, role: BlockRole, is_active: bool, viewport_width: usize) -> (Vec<VisualRow>, ColMap)`.

**Port source:** `~/projects/wordcartel-layout-spike/src/lib.rs` lines 33–354. **Adaptations:** (a) `layout` calls `crate::md_parse::analyze(line, role, is_active)` and uses its `runs` instead of the spike's inline `analyze`; (b) add a `style: Style` field to `Placed`, looked up per grapheme from the `LineAnalysis.styles` (the style whose `src` contains the grapheme's start byte, else `Style::Plain`); (c) carry `role` through the signature. Keep all soft-wrap and width logic verbatim.

- [ ] **Step 1: Write failing tests** in `src/layout.rs`:
```rust
//! Soft-wrap + conceal layout and the source↔visual ColMap.
//! Ported from the validated spike (~/projects/wordcartel-layout-spike).
use crate::md_parse::analyze;
use crate::style::{BlockRole, Style};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_line_identity_and_wrap() {
        // Active: raw, identity-ish. "abcdef" width 4 -> rows ["abcd","ef"].
        let (rows, map) = layout("abcdef", BlockRole::Paragraph, true, 4);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].display, "abcd");
        assert_eq!(rows[1].display, "ef");
        assert_eq!(map.eol, 6);
        assert!(map.is_active);
    }

    #[test]
    fn concealed_bold_drops_markers_in_display() {
        // Inactive: "**bold**" -> visible "bold".
        let (rows, _map) = layout("**bold**", BlockRole::Paragraph, false, 80);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].display, "bold");
    }

    #[test]
    fn cjk_width_two() {
        let (rows, _) = layout("中a", BlockRole::Paragraph, true, 80);
        assert_eq!(rows[0].width, 3); // 中=2, a=1
    }

    #[test]
    fn style_attached_to_placed() {
        // visible 'b' (first of bold) should carry Style::Strong.
        let (_rows, map) = layout("**bold**", BlockRole::Paragraph, false, 80);
        let first = map.placed.iter().find(|p| p.text == "b").unwrap();
        assert_eq!(first.style, Style::Strong);
    }
}
```
- [ ] **Step 2:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml layout` → FAIL.
- [ ] **Step 3: Implement** by reading the spike file and porting lines 33–354 with the three adaptations above. `Placed` gains `pub style: Style`. In the grapheme loop, set `style` by finding the `StyleSpan` in `analysis.styles` whose `src` contains the grapheme's start byte (default `Style::Plain`). `VisualRow` and the soft-wrap loop are unchanged from the spike.
- [ ] **Step 4:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml layout` → PASS (4 tests).
- [ ] **Step 5:** Commit: `feat(core): layout soft-wrap + Placed/ColMap (ported from spike)`

---

### Task 4: ColMap mapping methods

**Files:** Modify `src/layout.rs`.

**Interfaces — Produces** (on `impl ColMap`): `source_to_visual(offset) -> (usize,usize)`, `visual_to_source(row,col) -> usize`, `is_cursor_stop(offset) -> bool`, `cursor_stops() -> Vec<usize>`, `col_on_row(offset,row) -> usize`.

**Port source:** spike lines 79–162 and 398–411 — **verbatim** (these are the property-validated mapping methods).

- [ ] **Step 1: Write failing tests** in the `tests` module:
```rust
    #[test]
    fn roundtrip_bijection_on_visible_cells() {
        let (_rows, map) = layout("a中b", BlockRole::Paragraph, true, 80);
        for p in &map.placed {
            let (r, c) = map.source_to_visual(p.src.start);
            assert_eq!(map.visual_to_source(r, c), p.src.start);
        }
    }
    #[test]
    fn cursor_never_inside_concealed_marker() {
        // "**a**": only 'a' (byte 2) and EOL(6) are stops; the * bytes are not.
        let (_rows, map) = layout("**a**", BlockRole::Paragraph, false, 80);
        let stops = map.cursor_stops();
        assert!(stops.contains(&2));
        assert!(stops.contains(&map.eol));
        assert!(!stops.contains(&0)); // leading * concealed
        assert!(!stops.contains(&1));
    }
    #[test]
    fn end_of_row_clamps_not_teleports() {
        // width 2: "abcd" -> rows ["ab","cd"]. col 9 on row 0 clamps to end of row 0 (byte 2).
        let (_rows, map) = layout("abcd", BlockRole::Paragraph, true, 2);
        assert_eq!(map.visual_to_source(0, 9), 2);
    }
```
- [ ] **Step 2:** Run the layout tests → the three new ones FAIL.
- [ ] **Step 3: Implement** by porting spike lines 79–162 (the `impl ColMap` block: `source_to_visual`, `visual_to_source`, `is_cursor_stop`, `cursor_stops`) and 398–411 (`col_on_row`) verbatim.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(core): ColMap source↔visual mapping (ported from spike)`

---

### Task 5: Cursor + navigation

**Files:** Modify `src/layout.rs`.

**Interfaces — Produces:** `pub struct Cursor { offset, row, desired_col }` + `Cursor::new`; free fns `cursor_at`, `move_right`, `move_left`, `move_home`, `move_end`, `move_down_within`, `move_up_within`, `enter_from_top`, `enter_from_bottom`.

**Port source:** spike lines 360–497 — **verbatim**.

- [ ] **Step 1: Write failing tests:**
```rust
    #[test]
    fn right_skips_concealed_link_url() {
        // "ab[cd](http://x.io)ef": visible "abcdef"; moving right from start
        // visits only visible grapheme starts, never inside the hidden URL.
        let line = "ab[cd](http://x.io)ef";
        let (_r, map) = layout(line, BlockRole::Paragraph, false, 80);
        let mut cur = cursor_at(&map, 0);
        let mut visited = vec![cur.offset];
        for _ in 0..6 { cur = move_right(&map, cur); visited.push(cur.offset); }
        // none of the visited offsets fall inside the URL byte range [7,18)
        assert!(visited.iter().all(|&o| !(7..18).contains(&o)));
    }
    #[test]
    fn move_end_snaps_off_concealed_trailing_marker() {
        // "**a**" width 1: end-of-row raw position is a concealed '*'; move_end
        // must snap to a real stop (the 'a' start or EOL), never a '*'.
        let (_r, map) = layout("**a**", BlockRole::Paragraph, false, 1);
        let cur = cursor_at(&map, 2); // on 'a'
        let e = move_end(&map, cur);
        assert!(map.is_cursor_stop(e.offset));
    }
    #[test]
    fn down_then_up_preserves_desired_col() {
        // "aaaaa" width 3 -> rows ["aaa","aa"]. start at col 2 row 0, down then up.
        let (_r, map) = layout("aaaaa", BlockRole::Paragraph, true, 3);
        let start = Cursor { offset: 2, row: 0, desired_col: 2 };
        let down = move_down_within(&map, start).unwrap();
        let up = move_up_within(&map, down).unwrap();
        assert_eq!(up.offset, start.offset);
    }
```
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3: Implement** by porting spike lines 360–497 verbatim (`Cursor`, `cursor_at`, `move_*`, `enter_from_*`). `cursor_at`/`move_*` build a fresh `Cursor` with `style` not involved.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(core): Cursor + navigation with row affinity (ported from spike)`

---

### Task 6: Layout invariant property tests

**Files:** Modify `src/layout.rs`.

**Port source:** the spike's `tests/invariants.rs` (the 5 laws). Adapt the alphabet to include multi-byte graphemes (ASCII + `é 中 🙂`) plus the concealed inline constructs, snapping cut points to char boundaries.

**Interfaces — Produces:** five `proptest`s pinning: (1) round-trip bijection on visible cells; (2) every reachable cursor stop is a visible grapheme start or EOL (no cursor inside concealed markers); (3) soft-wrap fidelity (concatenated visible row spans reconstruct the visible source; no grapheme split); (4) active-line identity (is_active ⇒ visible == raw, placed gapless); (5) down→up preserves desired_col when row lengths allow.

- [ ] **Step 1:** Read the spike's `tests/invariants.rs`; write the five `proptest`s into a new `#[cfg(test)] mod props` in `src/layout.rs`, using a generator that mixes ASCII, `é`, `中`, `🙂`, and the constructs `**x** *y* ~~z~~ \`c\` [t](u)`, with widths capped at ~512 cases. Use the crate's `visible_source`/`visible_width` helpers (port spike lines 500–524) for the fidelity checks.
- [ ] **Step 2:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml layout` → the props should pass (porting validated code). If any FAILS, proptest prints a counterexample — fix the ported code (do not weaken the law), since a failure means the port diverged from the spike.
- [ ] **Step 3:** Commit (include any `proptest-regressions/`): `test(core): layout invariant property tests (5 laws)`

---

### Task 7: Style-segmented visual rows

**Files:** Modify `src/layout.rs`.

**Interfaces — Produces:** `pub struct StyledSeg { text: String, style: Style, width: usize }` and `VisualRow` gains `pub segs: Vec<StyledSeg>` — contiguous runs of same-style cells on that row (so a terminal renderer in Effort 3 can emit one SGR span per seg). The plain `display` string is retained (concatenation of seg texts).

- [ ] **Step 1: Write failing test:**
```rust
    #[test]
    fn styled_segments_split_by_style() {
        // "a **b**" inactive -> visible "a b": 'a',' ' Plain then 'b' Strong.
        let (rows, _map) = layout("a **b**", BlockRole::Paragraph, false, 80);
        let segs = &rows[0].segs;
        assert_eq!(segs.last().unwrap().style, Style::Strong);
        assert_eq!(segs.last().unwrap().text, "b");
        // concatenated segs equal display
        let joined: String = segs.iter().map(|s| s.text.clone()).collect();
        assert_eq!(joined, rows[0].display);
    }
```
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3: Implement.** Add `segs: Vec<StyledSeg>` to `VisualRow` (default empty). In the row-assembly loop (where `placed` are written into `visual_rows`), accumulate per-row segments: append each placed grapheme to the current seg if its `style` matches the seg's style, else start a new seg. Expand a tab grapheme to `TAB_WIDTH` spaces in the seg text (matching `display`). Keep `display`/`width`/`src_span` as-is.
- [ ] **Step 4:** Run → PASS. Re-run the full `layout` suite to confirm no regression.
- [ ] **Step 5:** Commit: `feat(core): style-segmented visual rows for rendering`

---

### Task 8: Multi-line render integration

**Files:** Create `wordcartel-core/tests/render_integration.rs`.

**Interfaces — Produces:** the cross-module law that wiring `TextBuffer` + `md_parse` + `layout` produces correct styled rows and that vertical cursor motion crosses logical-line boundaries via `desired_col`.

- [ ] **Step 1: Write the failing test:**
```rust
//! Render-core integration: split a buffer into logical lines, lay each out,
//! and verify cross-line vertical cursor motion preserves desired column.
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::layout::{enter_from_top, layout, Cursor, move_down_within};
use wordcartel_core::style::BlockRole;

#[test]
fn cursor_crosses_logical_lines_at_desired_col() {
    // Two logical lines (paragraphs separated by \n). Treat each \n-delimited
    // line as one logical line (block role Paragraph for this test).
    let buf = TextBuffer::from_str("hello world\ngoodbye");
    let text = buf.to_string();
    let lines: Vec<&str> = text.split('\n').collect();

    let (_r0, map0) = layout(lines[0], BlockRole::Paragraph, true, 80);
    let (_r1, map1) = layout(lines[1], BlockRole::Paragraph, false, 80);

    // Cursor on line 0 at col 7 ("w" of world). Move down off the end of line 0
    // (single visual row) -> enter line 1 from the top at desired_col 7.
    let on0 = Cursor { offset: 6, row: 0, desired_col: 7 };
    assert!(move_down_within(&map0, on0).is_none()); // line 0 is one visual row
    let on1 = enter_from_top(&map1, on0.desired_col);
    // col 7 of "goodbye" is 'e' (byte 7 == EOL since "goodbye" is 7 bytes) -> clamps to EOL
    assert_eq!(on1.offset, map1.eol.min(7));
    assert_eq!(on1.row, 0);
}

#[test]
fn concealed_line_renders_styled() {
    let buf = TextBuffer::from_str("a **bold** end");
    let line = buf.to_string();
    let (rows, _map) = layout(&line, BlockRole::Paragraph, false, 80);
    assert_eq!(rows[0].display, "a bold end");
    assert!(rows[0].segs.iter().any(|s| s.style == wordcartel_core::style::Style::Strong));
}
```
- [ ] **Step 2:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml --test render_integration` → it should pass once Tasks 1–7 are in. If it FAILS, fix the offending module (not the test).
- [ ] **Step 3:** Commit: `test(core): render-core multi-line integration`

---

## Self-Review (completed during planning)

- **Spec coverage:** §13.3 inline construct set → Task 2; §16 ColMap/cursor contract → Tasks 3–6 (ported from the spike that validated §16); style for rendering → Tasks 1,7; cross-module wiring → Task 8. Block-role *rendering* and `block_tree` → explicitly Plan 3 (out of scope; `BlockRole` carried as data).
- **Reuse:** layout/ColMap/cursor ported from our own validated spike; `pulldown-cmark` is the parser dependency; only the conceal/style glue is hand-written.
- **Placeholder scan:** port tasks point to exact spike line ranges (real, on-disk validated code) + give full tests; new code (md_parse, style, segs) is fully specified. No "TBD".
- **Type consistency:** `Style`/`BlockRole`/`Run`/`StyleSpan`/`LineAnalysis` defined in Task 1 and consumed unchanged in Tasks 2–8; `Placed` gains `style` in Task 3; `VisualRow` gains `segs` in Task 7.

## Completion
When all task checkboxes are `- [x]` and `cargo test --manifest-path wordcartel-core/Cargo.toml` is green: flip the relevant ledger rows (§4 md_parse, §16 layout/ColMap, §13.3 inline conceal) toward ✅ and mark **Plan 2 (Render Core)** complete. Then proceed to Plan 3 (block_tree spike → incremental invalidation + block-role rendering).
