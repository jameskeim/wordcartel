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

    // Task 3 tests
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
    #[test]
    fn face_is_total_and_heading_clamps() {
        let t = default();
        for el in ALL_ELEMENTS { let _ = t.face(el); } // never panics
        assert_eq!(t.face(SemanticElement::Heading(0)), t.face(SemanticElement::Heading(1)));
        assert_eq!(t.face(SemanticElement::Heading(9)), t.face(SemanticElement::Heading(6)));
    }
}
