# Theming Plan ① — Theme Model & Render Centralization

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the pure `wordcartel-core::theme` data model + all 13 built-in themes, and route every hardcoded color in `render.rs` through a single `compose` seam — with the **Default** theme reproducing today's look exactly (golden-tested).

**Architecture:** A pure, UI-agnostic `theme` module in core (`Color`/`Face`/`Theme`/`SemanticElement`, `quantize`, built-ins incl. the phosphor family with a hand-rolled HSL shade ramp). A shell `compose.rs` maps a face stack to a `ratatui::Style` (`face_to_ratatui` + `compose`). `render.rs`'s ~25 hardcoded `Color::`/`Modifier::` sites are replaced by `theme.face(element)` → `compose`. The active `Theme`/`Depth` are seeded on `Editor` like `view_opts`.

**Tech Stack:** Rust, `wordcartel-core` (`#![forbid(unsafe_code)]`, IO-free), `wordcartel` shell, ratatui 0.30.2.

## Global Constraints
- `wordcartel-core` is `#![forbid(unsafe_code)]`, IO/thread-free. `theme` is pure data + pure functions; **no new dependency** (HSL is hand-rolled; core has no color crate).
- Core has **no ratatui dependency** — `Color`/`Face` are plain data; the ratatui mapping lives in the shell (`Color::Default` → `ratatui::Color::Reset`).
- **Plan ① must NOT claim §13.2 completion** (Codex): it ships the model + colored themes + centralization. Structural glyphs (blockquote `▎`, thematic-break `───`, heading-level glyph), **document-selection painting**, and the cursor-safe prefix geometry are **plan ②**. The No-color theme here carries *modifier* cues only (no glyphs yet).
- **The Default theme reproduces today's pre-existing color sites exactly.** `default().base_fg/base_bg = Color::Default` so source modes and untouched cells are unchanged. Golden render tests gate every centralization task.
- `Theme::face` is **total** — every theme returns a `Face` for every `SemanticElement` (no missing element at a render site).
- Composition rule: each present `Face` field overrides the accumulator; `None` inherits; `Some(Color::Default)` hard-resets that color to terminal default.
- Cue mode (`Depth::None` OR `theme.monochrome`) is defined in the spec; plan ① sets the `monochrome` flag correctly on built-ins but the **forced-glyph** behavior is plan ② (no glyphs here).

---

## File Structure
- **Create** `wordcartel-core/src/theme.rs` — the whole model + built-ins (grows in plan ③ with `from_base16`).
- **Modify** `wordcartel-core/src/lib.rs` — `pub mod theme;`.
- **Create** `wordcartel/src/compose.rs` — `face_to_ratatui` + `compose` (the seam).
- **Modify** `wordcartel/src/lib.rs` — `pub mod compose;`.
- **Modify** `wordcartel/src/editor.rs` — `Editor.theme: Theme` + `Editor.depth: Depth`, seeded in `new_from_text`.
- **Modify** `wordcartel/src/render.rs` — replace `style_to_ratatui` + the 25 hardcoded color sites with `compose`.

---

## Task 1: Core — `Color`, `Face`, `SemanticElement` types + module export

**Files:**
- Create: `wordcartel-core/src/theme.rs`
- Modify: `wordcartel-core/src/lib.rs` (add `pub mod theme;` after `pub mod style;`, line ~18)
- Test: `wordcartel-core/src/theme.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `Color` enum (`Rgb{r,g,b}` / 16 named ANSI variants `Black..White` / `Indexed(u8)` / `Default`), `Face` struct (all-`Option` fields, incl. `dim`), `SemanticElement` enum (incl. `ChromeReverse`), `Depth` enum.

- [ ] **Step 1: Write the failing test**

Create `wordcartel-core/src/theme.rs`:
```rust
//! Pure, UI-agnostic theme model. No IO, no ratatui. The shell maps `Color`→ratatui.

/// Mirrors ratatui's named-color set 1:1 so the Default theme reproduces today's
/// `Color::Cyan` etc. EXACTLY (ratatui's `Color::Cyan` != `Color::Indexed(6)`).
/// `Indexed(u8)` is ONLY a quantized 256-color result; `Rgb` is truecolor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Rgb { r: u8, g: u8, b: u8 },
    Black, Red, Green, Yellow, Blue, Magenta, Cyan, Gray,
    DarkGray, LightRed, LightGreen, LightYellow, LightBlue, LightMagenta, LightCyan, White,
    Indexed(u8),
    Default,
}

/// One resolved look. Option None = "inherit accumulated" during composition;
/// Some(Color::Default) = explicitly reset that color to the terminal default.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Face {
    pub fg: Option<Color>, pub bg: Option<Color>,
    pub underline_color: Option<Color>,
    pub bold: Option<bool>, pub italic: Option<bool>, pub underline: Option<bool>,
    pub strike: Option<bool>, pub reverse: Option<bool>,
    /// DIM modifier. The No-color cue for Comment (italic+dim), FocusDim, ChromeMuted —
    /// keeps "italic+dim" (Comment) distinct from "italic" (Emphasis). Maps to Modifier::DIM.
    pub dim: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SemanticElement {
    Text,
    Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link,
    Heading(u8), BlockQuote, CodeBlock, ListMarker, ThematicBreak,
    FrontMatter, Comment, Selection,
    SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar, FocusDim, FoldMarker, WrapGuide,
    Chrome,         // panel/frame base (status/menu bar bg, overlay frames)
    ChromeReverse,  // REVERSED highlight (status line, palette/outline/diag selected row)
    ChromeSelected, // explicit fg/bg selection (menu item — today Black-on-White, NOT reverse)
    ChromeMuted,    // dim secondary chrome (menu dropdown normal item, scrollbar track)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Depth { Truecolor, Indexed256, Ansi16, None }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn face_default_is_all_none() {
        let f = Face::default();
        assert!(f.fg.is_none() && f.bold.is_none() && f.underline_color.is_none());
    }
    #[test]
    fn color_and_element_construct() {
        let _ = Color::Rgb { r: 1, g: 2, b: 3 };
        let _ = SemanticElement::Heading(3);
        let _ = Depth::Truecolor;
    }
}
```
Add `pub mod theme;` to `wordcartel-core/src/lib.rs` (alphabetically, after `pub mod style;`).

- [ ] **Step 2: Run to verify it fails (then compiles + passes)**

Run: `cargo test -p wordcartel-core theme::`
Expected: compiles; `face_default_is_all_none` + `color_and_element_construct` PASS (these are construction smoke tests — they pass once the types exist; the next tasks add behavior).

- [ ] **Step 3: Commit**
```bash
git add wordcartel-core/src/theme.rs wordcartel-core/src/lib.rs
git commit -m "feat(theme): core Color/Face/SemanticElement/Depth types"
```

---

## Task 2: Core — `quantize` (depth downsampling)

**Files:**
- Modify: `wordcartel-core/src/theme.rs`
- Test: `wordcartel-core/src/theme.rs`

**Interfaces:**
- Consumes: `Color`, `Depth`.
- Produces: `pub fn quantize(c: Color, depth: Depth) -> Color` — `Rgb`→`Indexed` (6×6×6 cube + 24-gray ramp) at `Indexed256`; `Rgb`/`Indexed`→the nearest **named** color at `Ansi16`; named colors and `Default` pass through at every depth; `Truecolor` passes everything through. (`Depth::None` is never passed here — callers force the No-color theme upstream.)

- [ ] **Step 1: Write the failing test**
```rust
    #[test]
    fn quantize_truecolor_is_identity() {
        let c = Color::Rgb { r: 10, g: 20, b: 30 };
        assert_eq!(quantize(c, Depth::Truecolor), c);
        assert_eq!(quantize(Color::Default, Depth::Truecolor), Color::Default);
    }
    #[test]
    fn quantize_rgb_to_indexed_cube_and_gray() {
        // pure black/white land on the cube corners (16 + ...); a mid-gray lands on the gray ramp.
        assert_eq!(quantize(Color::Rgb { r: 0, g: 0, b: 0 }, Depth::Indexed256), Color::Indexed(16));
        assert_eq!(quantize(Color::Rgb { r: 255, g: 255, b: 255 }, Depth::Indexed256), Color::Indexed(231));
        // a neutral gray (128,128,128) snaps onto the 232..=255 gray ramp, not the cube.
        match quantize(Color::Rgb { r: 128, g: 128, b: 128 }, Depth::Indexed256) {
            Color::Indexed(i) => assert!((232..=255).contains(&i), "gray ramp, got {i}"),
            other => panic!("expected Indexed gray, got {other:?}"),
        }
    }
    #[test]
    fn quantize_rgb_to_ansi16_nearest() {
        // pure red → a named red (Red or LightRed).
        let r = quantize(Color::Rgb { r: 255, g: 0, b: 0 }, Depth::Ansi16);
        assert!(matches!(r, Color::Red | Color::LightRed), "red, got {r:?}");
        assert_eq!(quantize(Color::Magenta, Depth::Ansi16), Color::Magenta); // named passthrough
        assert_eq!(quantize(Color::Default, Depth::Ansi16), Color::Default);
    }
    #[test]
    fn quantize_is_idempotent_per_depth() {
        let c = Color::Rgb { r: 200, g: 100, b: 50 };
        let q = quantize(c, Depth::Indexed256);
        assert_eq!(quantize(q, Depth::Indexed256), q);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel-core quantize`
Expected: FAIL — `quantize` not defined.

- [ ] **Step 3: Implement `quantize`**

Add to `theme.rs` (the 256-cube/gray algorithm is the standard xterm mapping; the 16-color step maps via the cube's nearest ANSI):
```rust
/// Nearest-color downsample. Pure arithmetic; no allocation. Only `Rgb` (and
/// `Indexed`→ansi16) are converted; named colors and `Default` pass through.
pub fn quantize(c: Color, depth: Depth) -> Color {
    match (c, depth) {
        (_, Depth::Truecolor) | (_, Depth::None) => c, // None never reached; identity safe
        (Color::Rgb { r, g, b }, Depth::Indexed256) => Color::Indexed(rgb_to_xterm256(r, g, b)),
        (Color::Rgb { r, g, b }, Depth::Ansi16) => rgb_to_named16(r, g, b),
        (Color::Indexed(i), Depth::Ansi16) => { let (r, g, b) = xterm256_to_rgb(i); rgb_to_named16(r, g, b) }
        // named colors, Indexed@256, Default → unchanged
        (c, _) => c,
    }
}

/// Nearest of the 16 named ANSI colors by squared RGB distance, returned as the
/// matching named `Color` variant (NOT an index — so it maps to ratatui's named color).
fn rgb_to_named16(r: u8, g: u8, b: u8) -> Color {
    const NAMED: [(Color, (u8,u8,u8)); 16] = [
        (Color::Black,(0,0,0)),(Color::Red,(128,0,0)),(Color::Green,(0,128,0)),(Color::Yellow,(128,128,0)),
        (Color::Blue,(0,0,128)),(Color::Magenta,(128,0,128)),(Color::Cyan,(0,128,128)),(Color::Gray,(192,192,192)),
        (Color::DarkGray,(128,128,128)),(Color::LightRed,(255,0,0)),(Color::LightGreen,(0,255,0)),(Color::LightYellow,(255,255,0)),
        (Color::LightBlue,(0,0,255)),(Color::LightMagenta,(255,0,255)),(Color::LightCyan,(0,255,255)),(Color::White,(255,255,255)),
    ];
    NAMED.iter().min_by_key(|(_, rgb)| dist2((r,g,b), *rgb)).unwrap().0
}

fn rgb_to_xterm256(r: u8, g: u8, b: u8) -> u8 {
    // gray ramp 232..=255 when r==g==b-ish and not a cube gray; else the 6x6x6 cube (16..=231).
    let to6 = |v: u8| -> u8 { // 0,95,135,175,215,255 buckets
        match v { 0..=47 => 0, 48..=114 => 1, 115..=154 => 2, 155..=194 => 3, 195..=234 => 4, _ => 5 }
    };
    let (cr, cg, cb) = (to6(r), to6(g), to6(b));
    let cube = 16 + 36 * cr + 6 * cg + cb;
    // gray ramp candidate
    let avg = ((r as u16 + g as u16 + b as u16) / 3) as u8;
    let gray_idx = if avg < 8 { 232 } else if avg > 238 { 255 } else { 232 + (avg - 8) / 10 };
    // pick whichever is closer to the original
    let cube_rgb = xterm256_to_rgb(cube);
    let gray_rgb = xterm256_to_rgb(gray_idx);
    if dist2((r, g, b), cube_rgb) <= dist2((r, g, b), gray_rgb) { cube } else { gray_idx }
}

fn xterm256_to_rgb(i: u8) -> (u8, u8, u8) {
    if i < 16 {
        // not used for cube/gray; return a rough ANSI-16 rgb (only for distance math)
        const A: [(u8,u8,u8);16] = [(0,0,0),(128,0,0),(0,128,0),(128,128,0),(0,0,128),(128,0,128),(0,128,128),(192,192,192),(128,128,128),(255,0,0),(0,255,0),(255,255,0),(0,0,255),(255,0,255),(0,255,255),(255,255,255)];
        A[i as usize]
    } else if i < 232 {
        let i = i - 16;
        let lv = |n: u8| -> u8 { if n == 0 { 0 } else { 55 + n * 40 } };
        (lv(i / 36), lv((i / 6) % 6), lv(i % 6))
    } else {
        let v = 8 + (i - 232) * 10;
        (v, v, v)
    }
}

fn dist2(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| { let v = x as i32 - y as i32; (v * v) as u32 };
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wordcartel-core quantize`
Expected: PASS. (If the exact `Indexed(16)`/`Indexed(231)` assertions trip on a bucket edge, adjust the `to6` thresholds — the cube corners for 0,0,0 and 255,255,255 must be 16 and 231; verify by computing `16+36*0+6*0+0=16` and `16+36*5+6*5+5=231`.)

- [ ] **Step 5: Commit**
```bash
git add wordcartel-core/src/theme.rs
git commit -m "feat(theme): quantize (Rgb→256 cube/gray + →ansi16 nearest)"
```

---

## Task 3: Core — `Theme` + `face()` + `default()` (the golden anchor)

**Files:**
- Modify: `wordcartel-core/src/theme.rs`
- Test: `wordcartel-core/src/theme.rs`

**Interfaces:**
- Consumes: `Color`, `Face`, `SemanticElement`.
- Produces: `pub struct Theme { name, base_fg, base_bg, heading_level_glyph, monochrome, faces }` (the `faces` field private); `Theme::face(&self, SemanticElement) -> Face` (total; `Heading(n)` clamps `1..=6`); `pub fn default() -> Theme` reproducing today's faces; `Theme::builtin(name)` + `Theme::builtin_names()` (wired to `default`/`no_color`/`tokyo_night`/phosphor in later tasks — for now `builtin("default")` works).

- [ ] **Step 1: Write the failing test** — `default()` reproduces today's inline look + is total
```rust
    fn f(fg: Option<Color>, bold: bool, italic: bool, ul: bool, strike: bool) -> Face {
        Face { fg, bold: bold.then_some(true), italic: italic.then_some(true),
               underline: ul.then_some(true), strike: strike.then_some(true), ..Face::default() }
    }
    #[test]
    fn default_reproduces_todays_inline_faces() {
        let t = default();
        // mirrors style_to_ratatui (render.rs:35-47)
        assert_eq!(t.face(SemanticElement::Strong),         f(None, true,  false, false, false));
        assert_eq!(t.face(SemanticElement::Emphasis),       f(None, false, true,  false, false));
        assert_eq!(t.face(SemanticElement::StrongEmphasis), f(None, true,  true,  false, false));
        assert_eq!(t.face(SemanticElement::Strikethrough),  f(None, false, false, false, true));
        assert_eq!(t.face(SemanticElement::Code), f(Some(Color::Cyan), false, false, false, false));
        assert_eq!(t.face(SemanticElement::Link), f(Some(Color::Yellow), false, false, true,  false));
    }
    #[test]
    fn default_base_is_terminal_default() {
        let t = default();
        assert_eq!(t.base_fg, Color::Default);
        assert_eq!(t.base_bg, Color::Default);
        assert!(!t.monochrome);
        assert!(!t.heading_level_glyph);
        // headings get NO color today → empty face (centralizing roles is a no-op for Default)
        assert_eq!(t.face(SemanticElement::Heading(1)), Face::default());
        assert_eq!(t.face(SemanticElement::Text), Face::default());
    }
    #[test]
    fn face_is_total_and_heading_clamps() {
        let t = default();
        for el in ALL_ELEMENTS { let _ = t.face(el); } // never panics
        assert_eq!(t.face(SemanticElement::Heading(0)), t.face(SemanticElement::Heading(1)));
        assert_eq!(t.face(SemanticElement::Heading(9)), t.face(SemanticElement::Heading(6)));
    }
```
Add a test helper listing every element (used by several tasks):
```rust
    const ALL_ELEMENTS: [SemanticElement; 31] = {
        use SemanticElement::*;
        [Text, Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link,
         Heading(1), Heading(2), Heading(3), Heading(4), Heading(5), Heading(6),
         BlockQuote, CodeBlock, ListMarker, ThematicBreak, FrontMatter, Comment, Selection,
         SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar, FocusDim, FoldMarker, WrapGuide,
         Chrome, ChromeReverse, ChromeSelected, ChromeMuted]
    };
    // 31 = Text + 6 inline + 6 heading + 4 block + 3 (fm/comment/sel) + 7 overlay + 4 chrome.
    // This is the totality proof — the count must equal the SemanticElement variant count
    // (Heading collapsed to its 6 levels). The `face_is_total` loop visits every one.
```
> The implementer: make `ALL_ELEMENTS` cover **every** variant (Text, 7 inline, Heading 1-6, BlockQuote, CodeBlock, ListMarker, ThematicBreak, FrontMatter, Comment, Selection, SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar, FocusDim, FoldMarker, WrapGuide, Chrome, ChromeSelected, ChromeMuted) — count them and fix the array length. The exact count is the proof `face` is total.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel-core default_reproduces`
Expected: FAIL — `default`/`Theme`/`face` not defined.

- [ ] **Step 3: Implement `Theme` + `face` + `default`**

Model `ThemeFaces` as a struct with a named `Face` per element (totality by construction), and `face()` a match. `default()` sets every Face to reproduce today (most are `Face::default()`; only the inline ones carry the mapped modifiers/colors). Use the **named `Color` variants directly** (`Color::Cyan`, `Color::Yellow`, `Color::Red`, `Color::Blue`, `Color::Black`, `Color::White`, `Color::DarkGray`) — they map 1:1 to ratatui's named colors in Task 7, so a golden test asserting `Some(Color::Cyan)` matches today.
```rust
#[derive(Clone, PartialEq, Eq, Debug)]
struct ThemeFaces {
    text: Face, emphasis: Face, strong: Face, strong_emphasis: Face, code: Face, strikethrough: Face, link: Face,
    heading: [Face; 6], block_quote: Face, code_block: Face, list_marker: Face, thematic_break: Face,
    front_matter: Face, comment: Face, selection: Face,
    search_match: Face, search_current: Face, diag_spelling: Face, diag_grammar: Face,
    focus_dim: Face, fold_marker: Face, wrap_guide: Face,
    chrome: Face, chrome_reverse: Face, chrome_selected: Face, chrome_muted: Face,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Theme {
    pub name: String,
    pub base_fg: Color, pub base_bg: Color,
    pub heading_level_glyph: bool,
    pub monochrome: bool,
    faces: ThemeFaces,
}

impl Theme {
    pub fn face(&self, el: SemanticElement) -> Face {
        use SemanticElement::*;
        match el {
            Text => self.faces.text,
            Emphasis => self.faces.emphasis, Strong => self.faces.strong,
            StrongEmphasis => self.faces.strong_emphasis, Code => self.faces.code,
            Strikethrough => self.faces.strikethrough, Link => self.faces.link,
            Heading(n) => self.faces.heading[(n.clamp(1, 6) - 1) as usize],
            BlockQuote => self.faces.block_quote, CodeBlock => self.faces.code_block,
            ListMarker => self.faces.list_marker, ThematicBreak => self.faces.thematic_break,
            FrontMatter => self.faces.front_matter, Comment => self.faces.comment, Selection => self.faces.selection,
            SearchMatch => self.faces.search_match, SearchCurrent => self.faces.search_current,
            DiagSpelling => self.faces.diag_spelling, DiagGrammar => self.faces.diag_grammar,
            FocusDim => self.faces.focus_dim, FoldMarker => self.faces.fold_marker, WrapGuide => self.faces.wrap_guide,
            Chrome => self.faces.chrome, ChromeReverse => self.faces.chrome_reverse,
            ChromeSelected => self.faces.chrome_selected, ChromeMuted => self.faces.chrome_muted,
        }
    }
    pub fn builtin(name: &str) -> Option<Theme> {
        match name {
            "default" => Some(default()),
            _ => None, // no-color/tokyo-night/phosphor wired in later tasks
        }
    }
    pub fn builtin_names() -> &'static [&'static str] { &["default"] } // extended in later tasks
}

// helper for terse face literals
fn modface(fg: Option<Color>, bold: bool, italic: bool, underline: bool, strike: bool, reverse: bool) -> Face {
    Face { fg, bold: bold.then_some(true), italic: italic.then_some(true),
           underline: underline.then_some(true), strike: strike.then_some(true),
           reverse: reverse.then_some(true), ..Face::default() }
}

pub fn default() -> Theme {
    Theme {
        name: "default".into(),
        base_fg: Color::Default, base_bg: Color::Default,
        heading_level_glyph: false, monochrome: false,
        faces: ThemeFaces {
            text: Face::default(),
            emphasis: modface(None, false, true, false, false, false),
            strong: modface(None, true, false, false, false, false),
            strong_emphasis: modface(None, true, true, false, false, false),
            code: modface(Some(Color::Cyan), false, false, false, false, false),
            strikethrough: modface(None, false, false, false, true, false),
            link: modface(Some(Color::Yellow), false, false, true, false, false),
            heading: [Face::default(); 6],          // today: no heading color
            block_quote: Face::default(), code_block: Face::default(),
            list_marker: Face { fg: Some(Color::DarkGray), ..Face::default() }, // prefix glyph normal
            thematic_break: Face::default(), front_matter: Face::default(), comment: Face::default(),
            selection: Face::default(),             // not painted in plan ① (no face needed yet)
            // search: today match = yellow bg + black fg; current = reverse.
            search_match: Face { bg: Some(Color::Yellow), fg: Some(Color::Black), ..Face::default() },
            search_current: modface(None, false, false, false, false, true),
            diag_spelling: Face { underline: Some(true), underline_color: Some(Color::Red), ..Face::default() },
            diag_grammar:  Face { underline: Some(true), underline_color: Some(Color::Blue), ..Face::default() },
            focus_dim: Face { fg: Some(Color::DarkGray), ..Face::default() },   // today: DarkGray
            fold_marker: Face { fg: Some(Color::DarkGray), ..Face::default() },
            wrap_guide: Face { fg: Some(Color::DarkGray), ..Face::default() },
            // chrome today: frame/menu-closed = white/black; status & overlay-selected = REVERSED;
            // menu-selected = explicit Black-on-White (NOT reverse); dropdown-normal = white/dark-gray.
            chrome: Face { fg: Some(Color::White), bg: Some(Color::Black), ..Face::default() },
            chrome_reverse: modface(None, false, false, false, false, true),
            chrome_selected: Face { fg: Some(Color::Black), bg: Some(Color::White), ..Face::default() },
            chrome_muted: Face { fg: Some(Color::White), bg: Some(Color::DarkGray), ..Face::default() },
        },
    }
}
```
> NOTE on exactness: the `default()` faces above are the contract for "reproduce today." The render-centralization tasks (10-12) are what actually prove it via golden cell tests — if a golden test shows a mismatch (e.g. today's diagnostic colored the underline but kept text fg, or the search match used a specific shade), adjust the corresponding Default face here to match the observed today-behavior, not the other way around.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wordcartel-core theme::`
Expected: PASS (all Task-1..3 tests).

- [ ] **Step 5: Commit**
```bash
git add wordcartel-core/src/theme.rs
git commit -m "feat(theme): Theme + total face() + default() reproducing today's faces"
```

---

## Task 4: Core — `no_color()` built-in (monochrome, modifier cues)

**Files:** Modify + Test: `wordcartel-core/src/theme.rs`

**Interfaces:** Produces `pub fn no_color() -> Theme` (`monochrome = true`; all `fg/bg/underline_color = Color::Default`; cues are modifiers only — **no structural glyphs yet**, those are plan ②). Wire `builtin("no-color")` + add to `builtin_names()`.

- [ ] **Step 1: Write the failing test**
```rust
    #[test]
    fn no_color_is_monochrome_with_modifier_cues() {
        let t = no_color();
        assert!(t.monochrome);
        assert_eq!(t.base_fg, Color::Default);
        // no element carries a real color
        for el in ALL_ELEMENTS {
            let f = t.face(el);
            for c in [f.fg, f.bg, f.underline_color].into_iter().flatten() {
                assert_eq!(c, Color::Default, "{el:?} must be color-free in no_color");
            }
        }
        // every Face-cued element has >=1 modifier (the §4-layer-1 invariant; glyph-only
        // elements BlockQuote/ThematicBreak/ListMarker/FoldMarker/WrapGuide/Text/Chrome are exempt here —
        // their cue is a glyph/placement added in plan ②/chrome task).
        let cued = [SemanticElement::Strong, SemanticElement::Emphasis, SemanticElement::Code,
                    SemanticElement::Link, SemanticElement::Strikethrough, SemanticElement::FrontMatter,
                    SemanticElement::Comment, SemanticElement::Selection, SemanticElement::SearchMatch];
        for el in cued {
            let f = t.face(el);
            assert!(f.bold.unwrap_or(false) || f.italic.unwrap_or(false) || f.underline.unwrap_or(false)
                    || f.strike.unwrap_or(false) || f.reverse.unwrap_or(false),
                    "{el:?} needs a modifier cue");
        }
        // pairwise distinctness for the §4 same-context pairs
        assert_ne!(t.face(SemanticElement::Comment), t.face(SemanticElement::Emphasis));
        assert_ne!(t.face(SemanticElement::FrontMatter), t.face(SemanticElement::Code));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel-core no_color`
Expected: FAIL — `no_color` not defined.

- [ ] **Step 3: Implement `no_color()`** (cues from spec §4; compound cues for the collisions)
```rust
pub fn no_color() -> Theme {
    let m = |bold, italic, underline, strike, reverse| modface(None, bold, italic, underline, strike, reverse);
    Theme {
        name: "no-color".into(),
        base_fg: Color::Default, base_bg: Color::Default,
        heading_level_glyph: true, monochrome: true,
        faces: ThemeFaces {
            text: Face::default(),
            emphasis: m(false, true, false, false, false),
            strong: m(true, false, false, false, false),
            strong_emphasis: m(true, true, false, false, false),
            code: m(false, false, false, false, true),                 // reverse
            strikethrough: m(false, false, false, true, false),
            link: m(false, false, true, false, false),                 // underline
            heading: [m(true,false,false,false,false); 6],             // bold; level glyph (plan ②) adds density
            block_quote: Face::default(),                              // glyph cue is plan ②
            code_block: m(false, false, false, false, true),           // reverse (block context)
            list_marker: Face::default(), thematic_break: Face::default(),
            front_matter: m(false, true, false, false, true),          // reverse + italic (distinct from Code)
            comment: Face { italic: Some(true), dim: Some(true), ..Face::default() }, // italic+dim (distinct from Emphasis=italic)
            selection: m(false, false, true, false, true),            // reverse + underline (visible over reverse)
            search_match: m(false, false, false, false, true),        // reverse
            search_current: m(true, false, false, false, true),       // reverse + bold
            diag_spelling: m(true, false, true, false, false),        // bold + underline (distinct from Link)
            diag_grammar:  m(true, false, true, false, false),
            focus_dim: Face { dim: Some(true), ..Face::default() },     // DIM inactive rows
            fold_marker: Face::default(), wrap_guide: Face::default(),
            chrome: Face::default(),
            chrome_reverse: m(false, false, false, false, true),     // REVERSED
            chrome_selected: m(false, false, false, false, true),    // no-color: reverse (can't do black-on-white)
            chrome_muted: Face { dim: Some(true), ..Face::default() },
        },
    }
}
```
> Uses `Face.dim` (defined in Task 1). Comment = `italic+dim` vs Emphasis = `italic` → genuinely distinct (the pairwise test); FocusDim/ChromeMuted use `dim`. `face_to_ratatui` (Task 7) maps `dim`→`Modifier::DIM`.

Wire `builtin`/`builtin_names` to include `"no-color"`. (Task 6 extracts these same
modifier-cue faces into a shared `mono_faces()` helper; when you reach Task 6,
refactor this `no_color()` to `Theme { ..., faces: mono_faces() }` so the cue set
has ONE source of truth — the `no_color` tests stay green since the faces are identical.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wordcartel-core theme::`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add wordcartel-core/src/theme.rs
git commit -m "feat(theme): no_color built-in (monochrome modifier cues) + Face.dim"
```

---

## Task 5: Core — `tokyo_night()` built-in

**Files:** Modify + Test: `wordcartel-core/src/theme.rs`

**Interfaces:** Produces `pub fn tokyo_night() -> Theme` (truecolor palette from `tokyonight.nvim`, MIT). Wire `builtin("tokyo-night")` + names.

- [ ] **Step 1: Write the failing test**
```rust
    #[test]
    fn tokyo_night_is_colored_and_total() {
        let t = tokyo_night();
        assert!(!t.monochrome);
        assert_ne!(t.base_bg, Color::Default);                 // dark bg
        // headings carry color here (unlike Default)
        assert!(matches!(t.face(SemanticElement::Heading(1)).fg, Some(Color::Rgb{..})));
        for el in ALL_ELEMENTS { let _ = t.face(el); }         // total
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel-core tokyo_night`
Expected: FAIL.

- [ ] **Step 3: Implement `tokyo_night()`** using the published Tokyo Night palette (MIT, `folke/tokyonight.nvim`): bg `#1a1b26`, fg `#c0caf5`, blue `#7aa2f7`, cyan `#7dcfff`, green `#9ece6a`, magenta `#bb9af7`, orange `#ff9e64`, red `#f7768e`, yellow `#e0af68`, comment `#565f89`, dark3 `#545c7e`. Map: headings→magenta/blue ramp, code→green, link→blue+underline, emphasis→italic+default fg, strong→bold, comment→comment-gray italic, front-matter→dark3, diag spell→red underline, grammar→yellow underline, search→a selection bg (`#283457`), chrome→a panel (`#16161e` bg / fg). Provide a `rgb(hex)` helper:
```rust
const fn rgb(r: u8, g: u8, b: u8) -> Color { Color::Rgb { r, g, b } }
// ... build ThemeFaces with the palette; heading[0]=magenta, [1]=blue, [2..]=cyan/green ramp, etc.
```
(Fill every face; the test only checks colored + total, so use sensible per-element assignments from the palette.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wordcartel-core tokyo_night`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add wordcartel-core/src/theme.rs
git commit -m "feat(theme): tokyo_night built-in (MIT palette)"
```

---

## Task 6: Core — phosphor family + HSL shade ramp

**Files:** Modify + Test: `wordcartel-core/src/theme.rs`

**Interfaces:** Produces `pub fn phosphor(name: &str, hue: Color, flat: bool) -> Theme` and a private `fn shade(hue: Color, level: u8) -> Color` (HSL lightness scaling, hand-rolled — no dep). Wire `builtin` + `builtin_names` to add the 10 phosphor themes (`phosphor-green`/`-green-flat`/… for green/amber/red/blue/purple). `flat ⇒ monochrome = true` + chrome from the ramp + base = hue/near-black-hue.

- [ ] **Step 1: Write the failing tests**
```rust
    #[test]
    fn phosphor_shade_ramp_varies_lightness() {
        let hue = Color::Rgb { r: 51, g: 255, b: 51 }; // green
        let dark = shade(hue, 0);
        let bright = shade(hue, 5);
        // both share the hue family but differ in lightness (bright is lighter)
        let lum = |c: Color| if let Color::Rgb{r,g,b}=c { r as u32+g as u32+b as u32 } else { 0 };
        assert!(lum(bright) > lum(dark), "ramp must brighten");
    }
    #[test]
    fn phosphor_flat_is_monochrome_single_shade() {
        let amber = Color::Rgb { r: 255, g: 176, b: 0 };
        let t = phosphor("phosphor-amber-flat", amber, true);
        assert!(t.monochrome);
        // every text element shares base_fg (flat); distinctions are modifiers
        for el in [SemanticElement::Strong, SemanticElement::Code, SemanticElement::Link, SemanticElement::Text] {
            assert_eq!(t.face(el).fg.unwrap_or(t.base_fg), t.base_fg, "{el:?} flat = base_fg");
        }
        // chrome is the hue, not gray
        assert!(matches!(t.face(SemanticElement::Chrome).bg, Some(Color::Rgb{..})));
    }
    #[test]
    fn phosphor_shaded_distinguishes_by_shade() {
        let amber = Color::Rgb { r: 255, g: 176, b: 0 };
        let t = phosphor("phosphor-amber", amber, false);
        assert!(!t.monochrome);
        assert_ne!(t.face(SemanticElement::Heading(1)).fg, t.face(SemanticElement::Comment).fg);
    }
    #[test]
    fn all_thirteen_builtins_total() {
        for name in Theme::builtin_names() {
            let t = Theme::builtin(name).unwrap();
            for el in ALL_ELEMENTS { let _ = t.face(el); }
        }
        assert_eq!(Theme::builtin_names().len(), 13); // default,no-color,tokyo-night, + 10 phosphor
    }
    #[test]
    fn phosphor_16color_floor() {
        for name in Theme::builtin_names().iter().filter(|n| n.starts_with("phosphor-")) {
            let t = Theme::builtin(name).unwrap();
            assert_ne!(quantize(t.base_fg, Depth::Ansi16), quantize(t.base_bg, Depth::Ansi16),
                       "{name}: fg/bg collapse at ansi16");
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel-core phosphor`
Expected: FAIL.

- [ ] **Step 3: Implement `shade` (HSL) + `phosphor` + wire builtins**

Hand-rolled RGB→HSL→RGB with lightness scaling:
```rust
fn shade(hue: Color, level: u8) -> Color {
    let Color::Rgb { r, g, b } = hue else { return hue };
    let (h, s, _l) = rgb_to_hsl(r, g, b);
    // map level 0..=5 to lightness 0.18..=0.92
    let l = 0.18 + (level.min(5) as f32 / 5.0) * (0.92 - 0.18);
    let (r, g, b) = hsl_to_rgb(h, s, l);
    Color::Rgb { r, g, b }
}
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) { /* standard conversion */ }
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) { /* standard conversion */ }

/// The monochrome (modifier-cue) face set, shared by `no_color()` and phosphor-flat
/// so the §4 cue discipline lives in one place. ALL text faces have `fg = None`
/// (→ inherit the theme's `base_fg`); distinctions are modifiers only.
fn mono_faces() -> ThemeFaces {
    let m = |bold, italic, underline, strike, reverse| modface(None, bold, italic, underline, strike, reverse);
    ThemeFaces {
        text: Face::default(),
        emphasis: m(false, true, false, false, false),
        strong: m(true, false, false, false, false),
        strong_emphasis: m(true, true, false, false, false),
        code: m(false, false, false, false, true),                // reverse
        strikethrough: m(false, false, false, true, false),
        link: m(false, false, true, false, false),                // underline
        heading: [m(true, false, false, false, false); 6],        // bold (+ level glyph in plan ②)
        block_quote: Face::default(), code_block: m(false, false, false, false, true),
        list_marker: Face::default(), thematic_break: Face::default(),
        front_matter: m(false, true, false, false, true),         // reverse+italic
        comment: Face { italic: Some(true), dim: Some(true), ..Face::default() }, // italic+dim
        selection: m(false, false, true, false, true),            // reverse+underline
        search_match: m(false, false, false, false, true),
        search_current: m(true, false, false, false, true),
        diag_spelling: m(true, false, true, false, false),        // bold+underline
        diag_grammar:  m(true, false, true, false, false),
        focus_dim: Face { dim: Some(true), ..Face::default() },
        fold_marker: Face::default(), wrap_guide: Face::default(),
        chrome: Face::default(),
        chrome_reverse: m(false, false, false, false, true),
        chrome_selected: m(false, false, false, false, true),
        chrome_muted: Face { dim: Some(true), ..Face::default() },
    }
}

pub fn phosphor(name: &str, hue: Color, flat: bool) -> Theme {
    let bg = shade(hue, 0);           // near-black hue
    let fg = shade(hue, 3);           // mid-bright hue
    let faces = if flat {
        // flat: reuse the modifier-only mono cues, but theme the chrome in-hue so the
        // whole screen is monochrome (text inherits base_fg = the hue).
        let mut f = mono_faces();
        f.chrome = Face { fg: Some(shade(hue, 4)), bg: Some(shade(hue, 1)), ..Face::default() };
        f.chrome_muted = Face { fg: Some(shade(hue, 2)), bg: Some(shade(hue, 0)), dim: Some(true), ..Face::default() };
        f
    } else {
        // shaded: distinguish by lightness within the hue.
        let s = |n| Face { fg: Some(shade(hue, n)), ..Face::default() };
        ThemeFaces {
            text: s(3),
            emphasis: Face { fg: Some(shade(hue, 3)), italic: Some(true), ..Face::default() },
            strong:   Face { fg: Some(shade(hue, 4)), bold: Some(true), ..Face::default() },
            strong_emphasis: Face { fg: Some(shade(hue, 4)), bold: Some(true), italic: Some(true), ..Face::default() },
            code: Face { fg: Some(shade(hue, 2)), reverse: Some(true), ..Face::default() },
            strikethrough: Face { fg: Some(shade(hue, 2)), strike: Some(true), ..Face::default() },
            link: Face { fg: Some(shade(hue, 5)), underline: Some(true), ..Face::default() },
            heading: [s(5), s(5), s(4), s(4), s(3), s(3)],
            block_quote: s(2), code_block: Face { fg: Some(shade(hue, 2)), reverse: Some(true), ..Face::default() },
            list_marker: s(2), thematic_break: s(1),
            front_matter: Face { fg: Some(shade(hue, 2)), italic: Some(true), ..Face::default() },
            comment: Face { fg: Some(shade(hue, 1)), italic: Some(true), ..Face::default() },
            selection: Face { fg: Some(shade(hue, 5)), reverse: Some(true), underline: Some(true), ..Face::default() },
            search_match: Face { bg: Some(shade(hue, 2)), fg: Some(shade(hue, 0)), ..Face::default() },
            search_current: Face { reverse: Some(true), bold: Some(true), ..Face::default() },
            diag_spelling: Face { underline: Some(true), underline_color: Some(shade(hue, 5)), ..Face::default() },
            diag_grammar:  Face { underline: Some(true), underline_color: Some(shade(hue, 4)), ..Face::default() },
            focus_dim: Face { fg: Some(shade(hue, 1)), dim: Some(true), ..Face::default() },
            fold_marker: s(1), wrap_guide: s(1),
            chrome: Face { fg: Some(shade(hue, 4)), bg: Some(shade(hue, 1)), ..Face::default() },
            chrome_reverse: Face { reverse: Some(true), ..Face::default() },
            chrome_selected: Face { fg: Some(shade(hue, 0)), bg: Some(shade(hue, 4)), ..Face::default() },
            chrome_muted: Face { fg: Some(shade(hue, 2)), bg: Some(shade(hue, 0)), dim: Some(true), ..Face::default() },
        }
    };
    Theme { name: name.into(), base_fg: fg, base_bg: bg, heading_level_glyph: flat, monochrome: flat, faces }
}

Wire `builtin`/`builtin_names`:
```rust
const PHOSPHORS: [(&str, Color); 5] = [
    ("green",  Color::Rgb{r:0x33,g:0xff,b:0x33}),
    ("amber",  Color::Rgb{r:0xff,g:0xb0,b:0x00}),
    ("red",    Color::Rgb{r:0xff,g:0x55,b:0x55}),
    ("blue",   Color::Rgb{r:0x55,g:0x99,b:0xff}),
    ("purple", Color::Rgb{r:0xcc,g:0x99,b:0xff}),
];
// builtin(name): match "phosphor-<hue>" / "phosphor-<hue>-flat" → phosphor(name, hue, flat).
// builtin_names(): a static slice of all 13 (lazy_static-free: a const &[&str]).
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wordcartel-core theme::`
Expected: PASS — all 13 built-ins total; phosphor flat/shaded/floor green.

- [ ] **Step 5: Commit**
```bash
git add wordcartel-core/src/theme.rs
git commit -m "feat(theme): phosphor family (10 themes) + HSL shade ramp"
```

---

## Task 7: Shell — `compose.rs`: `face_to_ratatui`

**Files:**
- Create: `wordcartel/src/compose.rs`
- Modify: `wordcartel/src/lib.rs` (`pub mod compose;`)
- Test: `wordcartel/src/compose.rs`

**Interfaces:**
- Consumes: `wordcartel_core::theme::{Color, Face, Depth, quantize}`; ratatui `Style as RStyle`, `Color as RColor`, `Modifier`.
- Produces: `pub fn face_to_ratatui(face: &Face, depth: Depth) -> RStyle` — quantize each color; `Color::Default`→unset (ratatui `Reset`/no-op); map `bold/italic/underline/strike/reverse/dim`→`Modifier`; `underline_color`→`RStyle::underline_color`.

- [ ] **Step 1: Write the failing test**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_core::theme::{Color, Face, Depth};
    use ratatui::style::{Color as RColor, Modifier};
    #[test]
    fn maps_rgb_and_modifiers_at_truecolor() {
        let f = Face { fg: Some(Color::Rgb{r:1,g:2,b:3}), bold: Some(true), underline: Some(true),
                       underline_color: Some(Color::Red), ..Face::default() };
        let s = face_to_ratatui(&f, Depth::Truecolor);
        assert_eq!(s.fg, Some(RColor::Rgb(1,2,3)));
        assert!(s.add_modifier.contains(Modifier::BOLD));
        assert!(s.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(s.underline_color, Some(RColor::Red));
    }
    #[test]
    fn default_color_is_reset_not_a_color() {
        let f = Face { fg: Some(Color::Default), ..Face::default() };
        let s = face_to_ratatui(&f, Depth::Truecolor);
        assert_eq!(s.fg, Some(RColor::Reset));
    }
    #[test]
    fn quantizes_at_ansi16() {
        let f = Face { fg: Some(Color::Rgb{r:255,g:0,b:0}), ..Face::default() };
        let s = face_to_ratatui(&f, Depth::Ansi16);
        // Rgb red → named Red/LightRed → ratatui named (NOT Indexed)
        assert!(matches!(s.fg, Some(RColor::Red) | Some(RColor::LightRed)));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel compose::`
Expected: FAIL — module/fn not defined.

- [ ] **Step 3: Implement `face_to_ratatui`** + a `Color`→`RColor` mapper
```rust
use ratatui::style::{Color as RColor, Modifier, Style as RStyle};
use wordcartel_core::theme::{quantize, Color, Depth, Face};

fn to_rcolor(c: Color, depth: Depth) -> RColor {
    match quantize(c, depth) {
        Color::Rgb { r, g, b } => RColor::Rgb(r, g, b),
        Color::Indexed(i) => RColor::Indexed(i),
        Color::Default => RColor::Reset,
        // named → ratatui named (1:1, so the Default theme reproduces today's Color::Cyan etc.)
        Color::Black => RColor::Black, Color::Red => RColor::Red, Color::Green => RColor::Green,
        Color::Yellow => RColor::Yellow, Color::Blue => RColor::Blue, Color::Magenta => RColor::Magenta,
        Color::Cyan => RColor::Cyan, Color::Gray => RColor::Gray, Color::DarkGray => RColor::DarkGray,
        Color::LightRed => RColor::LightRed, Color::LightGreen => RColor::LightGreen,
        Color::LightYellow => RColor::LightYellow, Color::LightBlue => RColor::LightBlue,
        Color::LightMagenta => RColor::LightMagenta, Color::LightCyan => RColor::LightCyan,
        Color::White => RColor::White,
    }
}

pub fn face_to_ratatui(face: &Face, depth: Depth) -> RStyle {
    let mut s = RStyle::default();
    if let Some(c) = face.fg { s = s.fg(to_rcolor(c, depth)); }
    if let Some(c) = face.bg { s = s.bg(to_rcolor(c, depth)); }
    if let Some(c) = face.underline_color { s = s.underline_color(to_rcolor(c, depth)); }
    let mut add = |on: Option<bool>, m: Modifier, s: RStyle| if on == Some(true) { s.add_modifier(m) } else { s };
    s = add(face.bold, Modifier::BOLD, s);
    s = add(face.italic, Modifier::ITALIC, s);
    s = add(face.underline, Modifier::UNDERLINED, s);
    s = add(face.strike, Modifier::CROSSED_OUT, s);
    s = add(face.reverse, Modifier::REVERSED, s);
    s = add(face.dim, Modifier::DIM, s);
    s
}
```
> `face.dim` requires the `dim` field from Task 4's decision — confirm it exists on `Face`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wordcartel compose::`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add wordcartel/src/compose.rs wordcartel/src/lib.rs
git commit -m "feat(theme): face_to_ratatui seam (Color→ratatui, quantize, Default→Reset)"
```

---

## Task 8: Shell — `compose.rs`: `compose(theme, depth, stack)`

**Files:** Modify + Test: `wordcartel/src/compose.rs`

**Interfaces:**
- Consumes: `Theme`, `Depth`, `SemanticElement`, `face_to_ratatui`.
- Produces: `pub fn compose(theme: &Theme, depth: Depth, stack: &[SemanticElement]) -> RStyle` — fold the elements' faces in order (each present field overrides the accumulator; `Some(Color::Default)` resets), then `face_to_ratatui` the merged `Face`.

- [ ] **Step 1: Write the failing test**
```rust
    use wordcartel_core::theme::{SemanticElement as E, Theme};
    #[test]
    fn later_face_field_overrides_earlier() {
        let t = wordcartel_core::theme::tokyo_night();
        // Heading then Code: Code's reverse/fg should override; heading fields not set by Code persist.
        let s = compose(&t, Depth::Truecolor, &[E::Text, E::Heading(1), E::Code]);
        let code = face_to_ratatui(&t.face(E::Code), Depth::Truecolor);
        // the Code fg wins over the heading fg
        assert_eq!(s.fg, code.fg);
    }
    #[test]
    fn empty_stack_is_default_style() {
        let t = wordcartel_core::theme::default();
        assert_eq!(compose(&t, Depth::Truecolor, &[]), RStyle::default());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel compose::compose` (or the test names)
Expected: FAIL.

- [ ] **Step 3: Implement `compose`** by merging `Face`s then mapping once
```rust
fn merge(acc: Face, f: Face) -> Face {
    Face {
        fg: f.fg.or(acc.fg), bg: f.bg.or(acc.bg), underline_color: f.underline_color.or(acc.underline_color),
        bold: f.bold.or(acc.bold), italic: f.italic.or(acc.italic), underline: f.underline.or(acc.underline),
        strike: f.strike.or(acc.strike), reverse: f.reverse.or(acc.reverse), dim: f.dim.or(acc.dim),
    }
}
pub fn compose(theme: &Theme, depth: Depth, stack: &[SemanticElement]) -> RStyle {
    let merged = stack.iter().fold(Face::default(), |acc, &el| merge(acc, theme.face(el)));
    face_to_ratatui(&merged, depth)
}
```
> `merge` uses `f.X.or(acc.X)` — the LATER face (f) wins when it sets a field, else inherits. `Some(Color::Default)` is a real `Some`, so it overrides (hard reset) — correct per the spec.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wordcartel compose::`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add wordcartel/src/compose.rs
git commit -m "feat(theme): compose() face-stack pipeline"
```

---

## Task 9: Shell — seed `Editor.theme` + `Editor.depth`

**Files:** Modify + Test: `wordcartel/src/editor.rs`

**Interfaces:**
- Consumes: `wordcartel_core::theme::{Theme, Depth, default}`.
- Produces: `Editor.theme: Theme`, `Editor.depth: Depth`, seeded in `new_from_text` (default theme; `depth = Depth::Truecolor` — real detection is plan ③). render reads them.

- [ ] **Step 1: Write the failing test** (in editor.rs tests)
```rust
    #[test]
    fn editor_seeds_default_theme_truecolor() {
        let ed = Editor::new_from_text("x", None, (80, 24));
        assert_eq!(ed.theme.name, "default");
        assert_eq!(ed.depth, wordcartel_core::theme::Depth::Truecolor);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel editor_seeds_default_theme_truecolor`
Expected: FAIL — no `theme` field.

- [ ] **Step 3: Add the fields + seed them**

Add to `Editor` (editor.rs ~line 215, after `outline`):
```rust
    /// Active theme + terminal color depth. Seeded at startup (real depth detection: plan ③).
    pub theme: wordcartel_core::theme::Theme,
    pub depth: wordcartel_core::theme::Depth,
```
Seed in `new_from_text` (in the `Editor { ... }` literal):
```rust
            theme: wordcartel_core::theme::default(),
            depth: wordcartel_core::theme::Depth::Truecolor,
```
Update any OTHER `Editor { ... }` construction sites (grep `Editor {` — `new_from_text` is the main one; tests may build others).

- [ ] **Step 4: Run to verify it passes + build the shell**

Run: `cargo test -p wordcartel editor_seeds_default_theme_truecolor && cargo build -p wordcartel`
Expected: PASS + builds (no "missing field `theme`").

- [ ] **Step 5: Commit**
```bash
git add wordcartel/src/editor.rs
git commit -m "feat(theme): seed Editor.theme + Editor.depth (default/truecolor)"
```

---

## Task 10: Render — centralize inline styles + role base color

**Files:** Modify + Test: `wordcartel/src/render.rs`

**Interfaces:**
- Consumes: `compose::compose`, `editor.theme`, `editor.depth`, `SemanticElement`.
- Produces: `style_to_ratatui` replaced by a theme lookup; heading/blockquote/code-block **roles get a base fg** from the theme (NEW for colored themes; no-op for Default). NO glyphs (plan ②).

- [ ] **Step 1: Write the failing tests** (golden no-regression + new coloring)
```rust
    #[test]
    fn default_theme_inline_styles_unchanged() {
        // a strong word renders BOLD, a code span gets cyan fg — exactly as today.
        let mut ed = Editor::new_from_text("**bold** and `code`\n", None, (40, 4));
        let buf = render_to_buffer(&mut ed, 40, 4);
        // find a bold cell and a cyan cell on row 0 (live-preview conceals the markers)
        let row0_has_bold = (0..40).any(|x| buf[(x,0)].style().add_modifier.contains(Modifier::BOLD));
        let row0_has_cyan = (0..40).any(|x| buf[(x,0)].style().fg == Some(Color::Indexed(6)) || buf[(x,0)].style().fg == Some(Color::Cyan));
        assert!(row0_has_bold && row0_has_cyan);
    }
    #[test]
    fn tokyo_night_heading_row_carries_heading_fg() {
        let mut ed = Editor::new_from_text("# Title\n", None, (40, 4));
        ed.theme = wordcartel_core::theme::tokyo_night();
        let buf = render_to_buffer(&mut ed, 40, 4);
        let want = compose::compose(&ed.theme, ed.depth, &[wordcartel_core::theme::SemanticElement::Text, wordcartel_core::theme::SemanticElement::Heading(1)]).fg;
        assert!((0..40).any(|x| buf[(x,0)].style().fg == want && want.is_some()), "heading fg applied");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p wordcartel default_theme_inline_styles_unchanged tokyo_night_heading_row_carries_heading_fg`
Expected: `tokyo_night_heading...` FAILS (roles not colored yet); the inline one may pass (still uses style_to_ratatui).

- [ ] **Step 3: Replace `style_to_ratatui` + apply role base color**

(a) Replace `style_to_ratatui(s: Style)` body with a theme lookup. Map `wordcartel_core::style::Style` → `SemanticElement`:
```rust
fn style_element(s: Style) -> wordcartel_core::theme::SemanticElement {
    use wordcartel_core::{style::Style as S, theme::SemanticElement as E};
    match s { S::Plain => E::Text, S::Emphasis => E::Emphasis, S::Strong => E::Strong,
              S::StrongEmphasis => E::StrongEmphasis, S::Code => E::Code,
              S::Strikethrough => E::Strikethrough, S::Link => E::Link }
}
```
At each inline-span paint site (render.rs ~301, ~347 — where `style_to_ratatui(seg.style)` / `style_to_ratatui(p.style)` are called), replace with `compose(&editor.theme, editor.depth, &[Text, role_element(role), style_element(seg.style)])` — i.e. build the stack `[Text, <block role>, <inline style>]`. (b) Map the block role to an element:
```rust
fn role_element(role: wordcartel_core::style::BlockRole) -> wordcartel_core::theme::SemanticElement {
    use wordcartel_core::{style::BlockRole as R, theme::SemanticElement as E};
    match role { R::Heading(n) => E::Heading(n), R::BlockQuote => E::BlockQuote, R::CodeBlock => E::CodeBlock,
                 R::ListItem => E::ListMarker, R::ThematicBreak => E::ThematicBreak, R::FrontMatter => E::FrontMatter,
                 R::Paragraph => E::Text }
}
```
The row's role is available as `vr.role` at both inline paint sites (layout.rs:51; render.rs ~300 and ~338) — thread it in. For the Default theme, the role/Text faces are empty so this is a no-op; for Tokyo Night/phosphor the heading/quote/code-block fg appears. Keep `style_to_ratatui` as a thin wrapper that calls `compose(&theme, depth, &[Text, style_element(s)])` if any caller still needs the inline-only form, OR inline the compose call.

**(c) Render-mode branch (Codex I4 — source modes get base canvas only, NO roles/inline):**
the renderer already computes `source_mode = editor.active().view.mode != RenderMode::LivePreview` (derive.rs:130-145). Build the stack conditionally:
```rust
let stack: Vec<SemanticElement> = if source_mode {
    vec![E::Text]                                   // base canvas only (Default→terminal default; phosphor→hue tint)
} else {
    vec![E::Text, role_element(role), style_element(seg.style)]  // live-preview: full semantic styling
};
let style = compose(&editor.theme, editor.depth, &stack);
```
(Overlays — search/diag/focus — are layered by Tasks 11/12 in **both** modes; selection painting is plan ②.) Add a test: a `# Heading` in `RenderMode::SourcePlain` under Tokyo Night does **not** carry the heading fg (it's literal source); the same heading in LivePreview does.

- [ ] **Step 4: Run to verify they pass + golden sweep**

Run: `cargo test -p wordcartel render::`
Expected: PASS — the two new tests + all existing render tests (Default = no change).

- [ ] **Step 5: Commit**
```bash
git add wordcartel/src/render.rs
git commit -m "feat(theme): centralize inline styles + role base color via compose"
```

---

## Task 11: Render — centralize chrome (status / menu / overlays)

**Files:** Modify + Test: `wordcartel/src/render.rs`

**Interfaces:** Consumes `compose`, the chrome faces (`Chrome`/`ChromeSelected`/`ChromeMuted`). Replaces the status-line, menu, palette/outline/diag-overlay, and scrollbar hardcoded color sites.

**Site → element mapping (from the spec §3.8 table + the render inventory):**
| render.rs site | replace with element |
|---|---|
| status lines (442,447,452,460 — REVERSED) | `ChromeReverse` |
| palette/outline/diag selected row (558,602,678 — REVERSED) | `ChromeReverse` |
| palette/outline query (548,596 — `RStyle::default()`) | `Text` (Codex C3 — Chrome's white-on-black would break the query row; `Text`=empty for Default → unchanged) |
| menu open category (631 — **Black on White**) | `ChromeSelected` |
| menu closed category (633 — White on Black) | `Chrome` |
| menu dropdown selected (646 — Black on White) | `ChromeSelected` |
| menu dropdown normal (648 — White on DarkGray) | `ChromeMuted` |
| scrollbar track/thumb (397-413) | `ChromeMuted` / `Chrome` |

> Codex C2: REVERSED highlights (status/list/overlay) use **`ChromeReverse`** (a `REVERSED` modifier — adapts to the themed bg); the menu's explicit Black-on-White selection uses **`ChromeSelected`** (a fixed fg/bg). They are different ratatui styles and must not collapse to one face.

- [ ] **Step 1: Write the failing test** (Default reproduces today; a phosphor theme tints chrome)
```rust
    #[test]
    fn default_status_line_still_reversed() {
        let mut ed = Editor::new_from_text("x", None, (40, 4));
        let buf = render_to_buffer(&mut ed, 40, 4);
        let last = 3u16;
        assert!((0..40).any(|x| buf[(x,last)].style().add_modifier.contains(Modifier::REVERSED)));
    }
    #[test]
    fn phosphor_status_line_carries_hue() {
        let mut ed = Editor::new_from_text("x", None, (40, 4));
        ed.theme = wordcartel_core::theme::Theme::builtin("phosphor-amber").unwrap();
        let buf = render_to_buffer(&mut ed, 40, 4);
        let want = compose::compose(&ed.theme, ed.depth, &[wordcartel_core::theme::SemanticElement::ChromeReverse]);
        // the status row picks up the themed chrome-reverse style, not a hardcoded REVERSED
        assert!((0..40).any(|x| buf[(x,3)].style() == want));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel phosphor_status_line_carries_hue`
Expected: FAIL — status still hardcoded reverse.

- [ ] **Step 3: Replace each chrome site** per the mapping table, using `compose(&editor.theme, editor.depth, &[<chrome element>])`. For the scrollbar, set the `Scrollbar` widget's track/thumb styles from `compose(.. &[ChromeMuted])` / `compose(.. &[Chrome])`.

- [ ] **Step 4: Run to verify it passes + golden sweep**

Run: `cargo test -p wordcartel render::`
Expected: PASS — Default chrome unchanged (the Default chrome faces reproduce white/black/reverse), phosphor chrome themed.

- [ ] **Step 5: Commit**
```bash
git add wordcartel/src/render.rs
git commit -m "feat(theme): centralize chrome (status/menu/overlays/scrollbar) via chrome faces"
```

---

## Task 12: Render — centralize search / diagnostics / focus / fold / wrap + final sweep

**Files:** Modify + Test: `wordcartel/src/render.rs`

**Interfaces:** Consumes `compose`, the overlay/structural faces. Replaces the remaining hardcoded sites and proves the full Default golden no-regression.

**Site → element mapping:**
| render.rs site | element |
|---|---|
| search current (350 — REVERSED) | `SearchCurrent` |
| search match (352 — yellow bg/black fg) | `SearchMatch` |
| spelling diag underline (360 — red) | `DiagSpelling` |
| grammar diag underline (361 — blue) | `DiagGrammar` |
| focus dim rows (293,345 — DarkGray) | `FocusDim` |
| prefix glyph, normal (329-330 — DarkGray) | `ListMarker` |
| prefix glyph, active row (297 — DIM) | `ListMarker` **+ `Modifier::DIM`** (Codex I5 — the active-row prefix is dimmer; add DIM on top of the themed face to reproduce today) |
| fold marker glyph (381 — DarkGray) | `FoldMarker` |
| fold marker count (384 — DarkGray + DIM) | `FoldMarker` **+ `Modifier::DIM`** |
| wrap guide (181 — DarkGray) | `WrapGuide` |

> Codex I5: the active-prefix and fold-count are the **base element face + `DIM`**, not a separate face. At those two sites: `compose(&theme, depth, &[ListMarker]).add_modifier(Modifier::DIM)` / `&[FoldMarker]).add_modifier(Modifier::DIM)`. This reproduces today (DarkGray+DIM) and themes correctly (hue+DIM under phosphor).

> The structural glyphs (blockquote/hr/heading glyph) are NOT added here (plan ②). These sites already exist; only their *style* is re-sourced from the theme.

- [ ] **Step 1: Write the failing tests** (Default unchanged for each)
```rust
    #[test]
    fn default_search_and_diag_unchanged() {
        // search highlight still yellow-bg/reverse; diagnostics still underline red/blue.
        // (build a buffer with a search overlay + a seeded diagnostic, assert the existing
        //  row_has_highlight / row_has_underline helpers still hold — mirror the existing
        //  search/diag render tests, which must keep passing.)
    }
    #[test]
    fn no_color_theme_strips_search_color_keeps_reverse() {
        let mut ed = Editor::new_from_text("needle here\n", None, (40, 4));
        ed.theme = wordcartel_core::theme::no_color();
        // open search for "needle" via the real path, then assert the match cell has REVERSED and no yellow bg.
        // (mirror an existing search render test for the open path.)
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel no_color_theme_strips_search_color_keeps_reverse`
Expected: FAIL — search still hardcoded yellow.

- [ ] **Step 3: Replace each remaining site** per the mapping table via `compose(&editor.theme, editor.depth, &[<element>])`. For diagnostics, the element's Face carries `underline + underline_color` → `face_to_ratatui` applies both. For focus-dim, `FocusDim`'s face (Default = DarkGray fg; No-color = `dim`) replaces the hardcoded DarkGray.

- [ ] **Step 4: Run the FULL shell + core suites (golden no-regression gate)**

Run: `XDG_STATE_HOME=/tmp/wc-theme cargo test -p wordcartel && cargo test -p wordcartel-core`
Expected: PASS — every pre-existing render test green with the Default theme (the centralization is invisible), the new theme tests green.

- [ ] **Step 5: Commit**
```bash
git add wordcartel/src/render.rs
git commit -m "feat(theme): centralize search/diag/focus/fold/wrap; full Default golden green"
```

---

## Final Verification
- [ ] Run `cargo test` (whole workspace) — all green.
- [ ] Run `cargo clippy -p wordcartel-core -p wordcartel --lib` — no NEW warnings in `theme.rs`/`compose.rs` or the touched render sites (pre-existing debt in untouched files is out of scope).
- [ ] Manual smoke: launch with each built-in via a temporary hardcode (`ed.theme = Theme::builtin("phosphor-amber").unwrap()`); confirm headings/code/links/chrome take the hue, Default is unchanged, No-color is colorless-but-modifier-cued. (Config-driven selection + the picker are plan ③.)

## Self-Review Notes (coverage vs spec §12 plan ①)
- §3.1 Color/Face/Theme/SemanticElement → Tasks 1,3; `quantize` → 2; built-ins (Default/No-color/Tokyo/phosphor×10) → 3-6; `monochrome`/`heading_level_glyph` flags set → 3-6.
- §3.3 `face_to_ratatui` + `compose` seam → 7,8; depth seed (detection deferred to ③) → 9.
- §3.2 render centralization (25 sites) → 10 (inline+role), 11 (chrome), 12 (search/diag/focus/fold/wrap).
- §9 "Default reproduces today" → golden tests in 10-12 + final sweep.
- **Deferred to plan ②/③ (correctly NOT here):** structural glyphs + heading-level glyph + prefix geometry, document-selection painting (Selection face defined, not painted), `from_base16`/base16 parsing, `[theme]` config, depth detection, the theme picker. Plan ① claims model + centralization only — NOT §13.2 completion.
