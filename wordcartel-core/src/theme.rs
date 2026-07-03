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
    FrontMatter, Comment, Selection, MarkedBlock,
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
    front_matter: Face, comment: Face, selection: Face, marked_block: Face,
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
            MarkedBlock => self.faces.marked_block,
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
            "no-color" => Some(no_color()),
            "tokyo-night" => Some(tokyo_night()),
            _ => {
                // "phosphor-<hue>" or "phosphor-<hue>-flat"
                let rest = name.strip_prefix("phosphor-")?;
                let (hue_name, flat) = if let Some(h) = rest.strip_suffix("-flat") {
                    (h, true)
                } else {
                    (rest, false)
                };
                let hue = PHOSPHORS.iter().find(|(n, _)| *n == hue_name)?.1;
                Some(phosphor(name, hue, flat))
            }
        }
    }
    pub fn builtin_names() -> &'static [&'static str] {
        &[
            "default", "no-color", "tokyo-night",
            "phosphor-green", "phosphor-green-flat",
            "phosphor-amber", "phosphor-amber-flat",
            "phosphor-red",   "phosphor-red-flat",
            "phosphor-blue",  "phosphor-blue-flat",
            "phosphor-purple","phosphor-purple-flat",
        ]
    }

    /// Override a face PER FIELD: each `Some` field of `patch` replaces the stored
    /// value; `None` fields leave the existing value untouched. Used by `[theme.styles]`.
    pub fn override_face(&mut self, el: SemanticElement, patch: Face) {
        let f = self.face_mut(el);
        if patch.fg.is_some() { f.fg = patch.fg; }
        if patch.bg.is_some() { f.bg = patch.bg; }
        if patch.underline_color.is_some() { f.underline_color = patch.underline_color; }
        if patch.bold.is_some() { f.bold = patch.bold; }
        if patch.italic.is_some() { f.italic = patch.italic; }
        if patch.underline.is_some() { f.underline = patch.underline; }
        if patch.strike.is_some() { f.strike = patch.strike; }
        if patch.reverse.is_some() { f.reverse = patch.reverse; }
        if patch.dim.is_some() { f.dim = patch.dim; }
    }

    /// Mutable accessor mirroring `face()` (same match arms). Private.
    fn face_mut(&mut self, el: SemanticElement) -> &mut Face {
        use SemanticElement::*;
        match el {
            Text => &mut self.faces.text,
            Emphasis => &mut self.faces.emphasis, Strong => &mut self.faces.strong,
            StrongEmphasis => &mut self.faces.strong_emphasis, Code => &mut self.faces.code,
            Strikethrough => &mut self.faces.strikethrough, Link => &mut self.faces.link,
            Heading(n) => &mut self.faces.heading[(n.clamp(1, 6) - 1) as usize],
            BlockQuote => &mut self.faces.block_quote, CodeBlock => &mut self.faces.code_block,
            ListMarker => &mut self.faces.list_marker, ThematicBreak => &mut self.faces.thematic_break,
            FrontMatter => &mut self.faces.front_matter, Comment => &mut self.faces.comment,
            Selection => &mut self.faces.selection,
            MarkedBlock => &mut self.faces.marked_block,
            SearchMatch => &mut self.faces.search_match, SearchCurrent => &mut self.faces.search_current,
            DiagSpelling => &mut self.faces.diag_spelling, DiagGrammar => &mut self.faces.diag_grammar,
            FocusDim => &mut self.faces.focus_dim, FoldMarker => &mut self.faces.fold_marker,
            WrapGuide => &mut self.faces.wrap_guide,
            Chrome => &mut self.faces.chrome, ChromeReverse => &mut self.faces.chrome_reverse,
            ChromeSelected => &mut self.faces.chrome_selected, ChromeMuted => &mut self.faces.chrome_muted,
        }
    }
}

// helper for terse face literals
fn modface(fg: Option<Color>, bold: bool, italic: bool, underline: bool, strike: bool, reverse: bool) -> Face {
    Face { fg, bold: bold.then_some(true), italic: italic.then_some(true),
           underline: underline.then_some(true), strike: strike.then_some(true),
           reverse: reverse.then_some(true), ..Face::default() }
}

pub fn no_color() -> Theme {
    Theme {
        name: "no-color".into(),
        base_fg: Color::Default, base_bg: Color::Default,
        heading_level_glyph: true, monochrome: true,
        faces: mono_faces(),
    }
}

pub fn default() -> Theme {
    Theme {
        name: "default".into(),
        base_fg: Color::Default, base_bg: Color::Default,
        heading_level_glyph: true, monochrome: false,
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
            selection: Face { reverse: Some(true), ..Face::default() },
            // §13.2 marked block: tinted bg + reverse+bold+underline (distinct from selection=reverse).
            marked_block: Face { bg: Some(Color::DarkGray), reverse: Some(true), bold: Some(true), underline: Some(true), ..Face::default() },
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

const fn rgb(r: u8, g: u8, b: u8) -> Color { Color::Rgb { r, g, b } }

pub fn tokyo_night() -> Theme {
    // Tokyo Night palette — MIT license, folke/tokyonight.nvim
    const BG:        Color = rgb(0x1a, 0x1b, 0x26); // #1a1b26
    const FG:        Color = rgb(0xc0, 0xca, 0xf5); // #c0caf5
    const BLUE:      Color = rgb(0x7a, 0xa2, 0xf7); // #7aa2f7
    const CYAN:      Color = rgb(0x7d, 0xcf, 0xff); // #7dcfff
    const GREEN:     Color = rgb(0x9e, 0xce, 0x6a); // #9ece6a
    const MAGENTA:   Color = rgb(0xbb, 0x9a, 0xf7); // #bb9af7
    const ORANGE:    Color = rgb(0xff, 0x9e, 0x64); // #ff9e64
    const RED:       Color = rgb(0xf7, 0x76, 0x8e); // #f7768e
    const YELLOW:    Color = rgb(0xe0, 0xaf, 0x68); // #e0af68
    const COMMENT:   Color = rgb(0x56, 0x5f, 0x89); // #565f89
    const DARK3:     Color = rgb(0x54, 0x5c, 0x7e); // #545c7e
    const SEL_BG:    Color = rgb(0x28, 0x34, 0x57); // #283457
    const PANEL_BG:  Color = rgb(0x16, 0x16, 0x1e); // #16161e

    Theme {
        name: "tokyo-night".into(),
        base_fg: FG,
        base_bg: BG,
        heading_level_glyph: true,
        monochrome: false,
        faces: ThemeFaces {
            text: Face::default(),
            emphasis: Face { italic: Some(true), ..Face::default() },
            strong: Face { bold: Some(true), ..Face::default() },
            strong_emphasis: Face { bold: Some(true), italic: Some(true), ..Face::default() },
            code: Face { fg: Some(GREEN), ..Face::default() },
            strikethrough: Face { strike: Some(true), ..Face::default() },
            link: Face { fg: Some(BLUE), underline: Some(true), ..Face::default() },
            heading: [
                Face { fg: Some(MAGENTA), bold: Some(true), ..Face::default() }, // h1
                Face { fg: Some(BLUE),    bold: Some(true), ..Face::default() }, // h2
                Face { fg: Some(CYAN),    bold: Some(true), ..Face::default() }, // h3
                Face { fg: Some(GREEN),   bold: Some(true), ..Face::default() }, // h4
                Face { fg: Some(YELLOW),  bold: Some(true), ..Face::default() }, // h5
                Face { fg: Some(ORANGE),  bold: Some(true), ..Face::default() }, // h6
            ],
            block_quote: Face { fg: Some(DARK3), italic: Some(true), ..Face::default() },
            code_block: Face { fg: Some(GREEN), ..Face::default() },
            list_marker: Face { fg: Some(BLUE), ..Face::default() },
            thematic_break: Face { fg: Some(DARK3), ..Face::default() },
            front_matter: Face { fg: Some(DARK3), ..Face::default() },
            comment: Face { fg: Some(COMMENT), italic: Some(true), dim: Some(true), ..Face::default() },
            selection: Face { bg: Some(SEL_BG), ..Face::default() },
            // §13.2 marked block: lighter-than-selection bg + reverse+bold+underline.
            marked_block: Face { bg: Some(DARK3), reverse: Some(true), bold: Some(true), underline: Some(true), ..Face::default() },
            search_match: Face { bg: Some(SEL_BG), ..Face::default() },
            search_current: Face { reverse: Some(true), ..Face::default() },
            diag_spelling: Face { underline: Some(true), underline_color: Some(RED), ..Face::default() },
            diag_grammar:  Face { underline: Some(true), underline_color: Some(YELLOW), ..Face::default() },
            focus_dim: Face { fg: Some(COMMENT), dim: Some(true), ..Face::default() },
            fold_marker: Face { fg: Some(DARK3), ..Face::default() },
            wrap_guide: Face { fg: Some(DARK3), ..Face::default() },
            chrome: Face { fg: Some(FG), bg: Some(PANEL_BG), ..Face::default() },
            chrome_reverse: Face { reverse: Some(true), ..Face::default() },
            chrome_selected: Face { fg: Some(BG), bg: Some(FG), ..Face::default() },
            chrome_muted: Face { fg: Some(DARK3), dim: Some(true), ..Face::default() },
        },
    }
}

/// A base16 (or base24) palette: 16 canonical slots, optional 8 extra (base10..base17).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BasePalette {
    pub base: [Color; 16],
    pub extra: Option<[Color; 8]>,
}

/// Build a Theme from a base16 palette using the canonical base16 markdown mapping.
/// Total even from the 16 core slots; `extra` (base24) only refines the chrome panel bg.
pub fn from_base16(name: &str, p: BasePalette) -> Theme {
    let b = p.base;
    // base16 slot conventions:
    // 00 bg · 01 panel · 02 sel-bg · 03 comment/dim · 04 dark-fg · 05 fg · 06 light-fg · 07 light-bg
    // 08 red · 09 orange · 0A yellow · 0B green · 0C cyan · 0D blue · 0E magenta · 0F brown
    let panel = p.extra.map(|e| e[0]).unwrap_or(b[1]); // base10 if base24, else base01
    Theme {
        name: name.into(),
        base_fg: b[5],
        base_bg: b[0],
        heading_level_glyph: true,
        monochrome: false,
        faces: ThemeFaces {
            text: Face::default(),
            emphasis: Face { italic: Some(true), fg: Some(b[0xE]), ..Face::default() },
            strong: Face { bold: Some(true), fg: Some(b[0xA]), ..Face::default() },
            strong_emphasis: Face { bold: Some(true), italic: Some(true), fg: Some(b[0x9]), ..Face::default() },
            code: Face { fg: Some(b[0xB]), ..Face::default() },
            strikethrough: Face { strike: Some(true), fg: Some(b[0x3]), ..Face::default() },
            link: Face { fg: Some(b[0xD]), underline: Some(true), ..Face::default() },
            heading: [
                Face { fg: Some(b[0xD]), bold: Some(true), ..Face::default() }, // h1 blue
                Face { fg: Some(b[0xC]), bold: Some(true), ..Face::default() }, // h2 cyan
                Face { fg: Some(b[0xB]), bold: Some(true), ..Face::default() }, // h3 green
                Face { fg: Some(b[0xA]), bold: Some(true), ..Face::default() }, // h4 yellow
                Face { fg: Some(b[0xE]), bold: Some(true), ..Face::default() }, // h5 magenta
                Face { fg: Some(b[0x8]), bold: Some(true), ..Face::default() }, // h6 red
            ],
            block_quote: Face { fg: Some(b[0xC]), italic: Some(true), ..Face::default() },
            code_block: Face { fg: Some(b[0xB]), ..Face::default() },
            list_marker: Face { fg: Some(b[0x8]), ..Face::default() },
            thematic_break: Face { fg: Some(b[0x3]), ..Face::default() },
            front_matter: Face { fg: Some(b[0xF]), italic: Some(true), ..Face::default() },
            comment: Face { fg: Some(b[0x3]), italic: Some(true), dim: Some(true), ..Face::default() },
            selection: Face { bg: Some(b[0x2]), ..Face::default() },
            // §13.2 marked block: distinct (comment-slot) bg + reverse+bold+underline.
            marked_block: Face { bg: Some(b[0x3]), reverse: Some(true), bold: Some(true), underline: Some(true), ..Face::default() },
            search_match: Face { bg: Some(b[0xA]), fg: Some(b[0x0]), ..Face::default() },
            search_current: Face { reverse: Some(true), ..Face::default() },
            diag_spelling: Face { underline: Some(true), underline_color: Some(b[0x8]), ..Face::default() },
            diag_grammar:  Face { underline: Some(true), underline_color: Some(b[0xD]), ..Face::default() },
            focus_dim: Face { fg: Some(b[0x3]), dim: Some(true), ..Face::default() },
            fold_marker: Face { fg: Some(b[0x3]), ..Face::default() },
            wrap_guide: Face { fg: Some(b[0x2]), ..Face::default() },
            chrome: Face { fg: Some(b[0x5]), bg: Some(panel), ..Face::default() },
            chrome_reverse: Face { reverse: Some(true), ..Face::default() },
            chrome_selected: Face { fg: Some(b[0x0]), bg: Some(b[0x5]), ..Face::default() },
            chrome_muted: Face { fg: Some(b[0x4]), dim: Some(true), ..Face::default() },
        },
    }
}

/// Map a snake-case config key (`[theme.styles]`) to a SemanticElement.
/// `heading1`..`heading6` map to `Heading(n)`. Unknown → None (caller warns).
pub fn element_from_key(key: &str) -> Option<SemanticElement> {
    use SemanticElement::*;
    Some(match key {
        "text" => Text,
        "emphasis" => Emphasis, "strong" => Strong, "strong_emphasis" => StrongEmphasis,
        "code" => Code, "strikethrough" => Strikethrough, "link" => Link,
        "heading1" => Heading(1), "heading2" => Heading(2), "heading3" => Heading(3),
        "heading4" => Heading(4), "heading5" => Heading(5), "heading6" => Heading(6),
        "block_quote" => BlockQuote, "code_block" => CodeBlock, "list_marker" => ListMarker,
        "thematic_break" => ThematicBreak, "front_matter" => FrontMatter, "comment" => Comment,
        "selection" => Selection, "marked_block" => MarkedBlock,
        "search_match" => SearchMatch, "search_current" => SearchCurrent,
        "diag_spelling" => DiagSpelling, "diag_grammar" => DiagGrammar, "focus_dim" => FocusDim,
        "fold_marker" => FoldMarker, "wrap_guide" => WrapGuide,
        "chrome" => Chrome, "chrome_reverse" => ChromeReverse,
        "chrome_selected" => ChromeSelected, "chrome_muted" => ChromeMuted,
        _ => return None,
    })
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        let mut h = (g - b) / d;
        if g < b { h += 6.0; }
        h / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    if s.abs() < f32::EPSILON {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue_to_rgb = |p: f32, q: f32, mut t: f32| -> f32 {
        if t < 0.0 { t += 1.0; }
        if t > 1.0 { t -= 1.0; }
        if t < 1.0/6.0 { return p + (q - p) * 6.0 * t; }
        if t < 1.0/2.0 { return q; }
        if t < 2.0/3.0 { return p + (q - p) * (2.0/3.0 - t) * 6.0; }
        p
    };
    let r = (hue_to_rgb(p, q, h + 1.0/3.0) * 255.0).round() as u8;
    let g = (hue_to_rgb(p, q, h) * 255.0).round() as u8;
    let b = (hue_to_rgb(p, q, h - 1.0/3.0) * 255.0).round() as u8;
    (r, g, b)
}

fn shade(hue: Color, level: u8) -> Color {
    let Color::Rgb { r, g, b } = hue else { return hue };
    let (h, s, _l) = rgb_to_hsl(r, g, b);
    // map level 0..=5 to lightness 0.08..=0.92 (widened from 0.18..=0.92 for floor test)
    let l = 0.08 + (level.min(5) as f32 / 5.0) * (0.92 - 0.08);
    let (r, g, b) = hsl_to_rgb(h, s, l);
    Color::Rgb { r, g, b }
}

/// The monochrome (modifier-cue) face set, shared by `no_color()` and phosphor-flat
/// so the §4 cue discipline lives in one place.
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
        heading: [m(true, false, false, false, false); 6],        // bold
        block_quote: Face::default(), code_block: m(false, false, false, false, true),
        list_marker: Face::default(), thematic_break: Face::default(),
        front_matter: m(false, true, false, false, true),         // reverse+italic
        comment: Face { italic: Some(true), dim: Some(true), ..Face::default() }, // italic+dim
        selection: m(false, false, true, false, true),            // reverse+underline
        marked_block: m(true, false, true, false, true),          // reverse+bold+underline (§13.2 distinct)
        search_match: m(false, false, false, false, true),
        search_current: m(true, false, false, false, true),
        diag_spelling: m(true, false, true, false, false),        // bold+underline
        diag_grammar:  m(false, true, true, false, false),        // italic+underline (I7: distinct from spelling)
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
        let mut f = mono_faces();
        f.chrome = Face { fg: Some(shade(hue, 4)), bg: Some(shade(hue, 1)), ..Face::default() };
        f.chrome_muted = Face { fg: Some(shade(hue, 2)), bg: Some(shade(hue, 0)), dim: Some(true), ..Face::default() };
        f
    } else {
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
            marked_block: Face { bg: Some(shade(hue, 2)), reverse: Some(true), bold: Some(true), underline: Some(true), ..Face::default() },
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
    Theme { name: name.into(), base_fg: fg, base_bg: bg, heading_level_glyph: true, monochrome: flat, faces }
}

const PHOSPHORS: [(&str, Color); 5] = [
    ("green",  Color::Rgb{r:0x33,g:0xff,b:0x33}),
    ("amber",  Color::Rgb{r:0xff,g:0xb0,b:0x00}),
    ("red",    Color::Rgb{r:0xff,g:0x55,b:0x55}),
    ("blue",   Color::Rgb{r:0x55,g:0x99,b:0xff}),
    ("purple", Color::Rgb{r:0xcc,g:0x99,b:0xff}),
];

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
        assert!(t.heading_level_glyph);
        // headings get NO color today → empty face (centralizing roles is a no-op for Default)
        assert_eq!(t.face(SemanticElement::Heading(1)), Face::default());
        assert_eq!(t.face(SemanticElement::Text), Face::default());
    }
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

    // a11y: MarkedBlock has a distinct mono modifier (reverse+bold+underline) and is in ALL_ELEMENTS.
    #[test]
    fn marked_block_mono_modifier_is_distinct() {
        let t = no_color();
        let mb = t.face(SemanticElement::MarkedBlock);
        assert_eq!((mb.reverse, mb.bold, mb.underline), (Some(true), Some(true), Some(true)));
        // distinct from selection (reverse+underline), search_current (bold+reverse), diag_spelling (bold+underline)
        assert_ne!(mb, t.face(SemanticElement::Selection));
        assert_ne!(mb, t.face(SemanticElement::SearchCurrent));
        assert_ne!(mb, t.face(SemanticElement::DiagSpelling));
        // present in the totality set
        assert!(ALL_ELEMENTS.contains(&SemanticElement::MarkedBlock));
    }

    const ALL_ELEMENTS: [SemanticElement; 32] = {
        use SemanticElement::*;
        [Text, Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link,
         Heading(1), Heading(2), Heading(3), Heading(4), Heading(5), Heading(6),
         BlockQuote, CodeBlock, ListMarker, ThematicBreak, FrontMatter, Comment, Selection, MarkedBlock,
         SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar, FocusDim, FoldMarker, WrapGuide,
         Chrome, ChromeReverse, ChromeSelected, ChromeMuted]
    };
    // 32 = Text + 6 inline + 6 heading + 4 block + 4 (fm/comment/sel/marked-block) + 7 overlay + 4 chrome.
    // This is the totality proof — the count must equal the SemanticElement variant count
    // (Heading collapsed to its 6 levels). The `face_is_total` loop visits every one.
    #[test]
    fn face_is_total_and_heading_clamps() {
        let t = default();
        for el in ALL_ELEMENTS { let _ = t.face(el); } // never panics
        assert_eq!(t.face(SemanticElement::Heading(0)), t.face(SemanticElement::Heading(1)));
        assert_eq!(t.face(SemanticElement::Heading(9)), t.face(SemanticElement::Heading(6)));
    }

    #[test]
    fn tokyo_night_is_colored_and_total() {
        let t = tokyo_night();
        assert!(!t.monochrome);
        assert_ne!(t.base_bg, Color::Default);                 // dark bg
        // headings carry color here (unlike Default)
        assert!(matches!(t.face(SemanticElement::Heading(1)).fg, Some(Color::Rgb{..})));
        for el in ALL_ELEMENTS { let _ = t.face(el); }         // total
    }

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

    // A minimal but realistic base16 palette (Gruvbox-dark-ish), 16 RGB slots.
    fn sample_base16() -> BasePalette {
        let c = |r, g, b| Color::Rgb { r, g, b };
        BasePalette {
            base: [
                c(0x28,0x28,0x28), c(0x3c,0x38,0x36), c(0x50,0x49,0x45), c(0x66,0x5c,0x54), // 00..03
                c(0xbd,0xae,0x93), c(0xd5,0xc4,0xa1), c(0xeb,0xdb,0xb2), c(0xfb,0xf1,0xc7), // 04..07
                c(0xfb,0x49,0x34), c(0xfe,0x80,0x19), c(0xfa,0xbd,0x2f), c(0xb8,0xbb,0x26), // 08..0B
                c(0x8e,0xc0,0x7c), c(0x83,0xa5,0x98), c(0xd3,0x86,0x9b), c(0xd6,0x5d,0x0e), // 0C..0F
            ],
            extra: None,
        }
    }

    #[test]
    fn from_base16_is_total_and_uses_canonical_slots() {
        let t = from_base16("base16-gruvbox", sample_base16());
        assert_eq!(t.name, "base16-gruvbox");
        assert!(!t.monochrome);
        // base_bg = base00, base_fg = base05 (base16 convention)
        assert_eq!(t.base_bg, Color::Rgb { r:0x28, g:0x28, b:0x28 });
        assert_eq!(t.base_fg, Color::Rgb { r:0xd5, g:0xc4, b:0xa1 });
        // headings are bold + colored (base0D = blue slot for h1)
        let h1 = t.face(SemanticElement::Heading(1));
        assert_eq!(h1.bold, Some(true));
        assert_eq!(h1.fg, Some(Color::Rgb { r:0x83, g:0xa5, b:0x98 })); // base0D
        // code colored from base0B (green); link underlined from base0D
        assert_eq!(t.face(SemanticElement::Code).fg, Some(Color::Rgb { r:0xb8, g:0xbb, b:0x26 }));
        assert_eq!(t.face(SemanticElement::Link).underline, Some(true));
        // EVERY element resolves to *some* face without panicking (totality)
        for el in [SemanticElement::Text, SemanticElement::Comment, SemanticElement::FrontMatter,
                   SemanticElement::Chrome, SemanticElement::ChromeSelected, SemanticElement::WrapGuide] {
            let _ = t.face(el);
        }
    }

    #[test]
    fn override_face_is_per_field() {
        let mut t = default();
        // override only the bg of Selection; reverse (existing) must remain.
        t.override_face(SemanticElement::Selection,
            Face { bg: Some(Color::Rgb { r:0x28, g:0x34, b:0x57 }), ..Face::default() });
        let sel = t.face(SemanticElement::Selection);
        assert_eq!(sel.bg, Some(Color::Rgb { r:0x28, g:0x34, b:0x57 })); // set
        assert_eq!(sel.reverse, Some(true));                            // preserved from default()
    }

    #[test]
    fn element_from_key_maps_snake_case_names() {
        use SemanticElement::*;
        assert_eq!(element_from_key("heading1"), Some(Heading(1)));
        assert_eq!(element_from_key("heading6"), Some(Heading(6)));
        assert_eq!(element_from_key("selection"), Some(Selection));
        assert_eq!(element_from_key("strong_emphasis"), Some(StrongEmphasis));
        assert_eq!(element_from_key("chrome_selected"), Some(ChromeSelected));
        assert_eq!(element_from_key("nope"), None);
        assert_eq!(element_from_key("heading0"), None); // out of range
        assert_eq!(element_from_key("heading7"), None);
    }
}
