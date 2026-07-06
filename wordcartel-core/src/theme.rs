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
    ChromeOverlay,  // modal interior fill (palette/outline/picker overlay bg)
    ChromeAccent,   // accent fg on panel bg (active-prompt status + future focus marks)
}

/// Controls how aggressively the derived chrome ladder steps away from the canvas.
/// `Full` = calibrated steps; `Zen` = collapsed steps (×0.35) toward the canvas poles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChromeDisposition { Full, Zen }

/// Whether the theme's canvas (`base_bg`) is painted across the editing area.
/// `Opaque` (default) = paint it — RGB themes own the page. `Transparent` = skip it and
/// the modal-interior fill, so a see-through terminal shows through. Render-only; never
/// affects derivation. Non-Rgb `base_bg` (terminal-* themes) has nothing to paint.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CanvasMode { Opaque, Transparent }

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
    chrome_overlay: Face, chrome_accent: Face,
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
            ChromeOverlay => self.faces.chrome_overlay, ChromeAccent => self.faces.chrome_accent,
        }
    }
    pub fn builtin(name: &str) -> Option<Theme> {
        match name {
            "terminal-plain" => Some(default()),
            "terminal-ansi" => Some(terminal_ansi()),
            "no-color" => Some(no_color()),
            "tokyo-night" => Some(tokyo_night()),
            "catppuccin-mocha"  => Some(catppuccin_mocha()),
            "catppuccin-latte"  => Some(catppuccin_latte()),
            "flexoki-dark"      => Some(flexoki_dark()),
            "flexoki-light"     => Some(flexoki_light()),
            "gruvbox-dark"      => Some(gruvbox_dark()),
            "gruvbox-light"     => Some(gruvbox_light()),
            "rosepine-moon"     => Some(rosepine_moon()),
            "rosepine-dawn"     => Some(rosepine_dawn()),
            "solarized-dark"    => Some(solarized_dark()),
            "solarized-light"   => Some(solarized_light()),
            _ => {
                // "phosphor-<hue>" — flat suffix removed (D4); resolve layer maps stale aliases (T3).
                let rest = name.strip_prefix("phosphor-")?;
                let hue = PHOSPHORS.iter().find(|(n, _)| *n == rest)?.1;
                Some(phosphor(name, hue))
            }
        }
    }
    pub fn builtin_names() -> &'static [&'static str] {
        // D5 order: terminal variants → no-color → tokyo → phosphors → 10 E4 themes
        &[
            "terminal-plain", "terminal-ansi", "no-color", "tokyo-night",
            "phosphor-green", "phosphor-amber", "phosphor-red", "phosphor-blue", "phosphor-purple",
            "catppuccin-mocha", "catppuccin-latte",
            "flexoki-dark",  "flexoki-light",
            "gruvbox-dark",  "gruvbox-light",
            "rosepine-moon", "rosepine-dawn",
            "solarized-dark", "solarized-light",
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

    /// Derive the chrome ladder from `base_fg`/`base_bg` under `disp`.
    ///
    /// Fills **only** the five chrome faces whose current value is `Face::default()` (all-None
    /// sentinel): `chrome`, `chrome_selected`, `chrome_muted`, `chrome_overlay`, `chrome_accent`.
    /// `chrome_reverse` is **never** derived — it stays the reverse-modifier default.
    ///
    /// Early-returns without change if either base is not `Color::Rgb`. Callers should call on a
    /// fresh theme instance before applying user overrides (resolve order: base → derive → styles).
    /// A second call on an already-derived theme is a no-op (non-sentinel faces are skipped).
    pub fn derive_chrome(&mut self, disp: ChromeDisposition) {
        let (bgr, bgg, bgb, fgr, fgg, fgb) = match (self.base_bg, self.base_fg) {
            (Color::Rgb { r: bgr, g: bgg, b: bgb }, Color::Rgb { r: fgr, g: fgg, b: fgb }) =>
                (bgr, bgg, bgb, fgr, fgg, fgb),
            _ => return,
        };
        let base_bg = Color::Rgb { r: bgr, g: bgg, b: bgb };
        let base_fg = Color::Rgb { r: fgr, g: fgg, b: fgb };

        let is_dark = rel_lum(bgr, bgg, bgb) < 0.5;
        let z = match disp { ChromeDisposition::Full => 1.0f32, ChromeDisposition::Zen => ZEN_COLLAPSE };

        let bar_pct     = if is_dark { CHROME_BAR_PCT_DARK  } else { CHROME_BAR_PCT_LIGHT  } * z;
        let overlay_pct = if is_dark { CHROME_OVERLAY_PCT_DARK } else { CHROME_OVERLAY_PCT_LIGHT } * z;
        let deep_pct    = if is_dark { CHROME_DEEP_PCT_DARK } else { CHROME_DEEP_PCT_LIGHT } * z;

        let black = (0u8, 0u8, 0u8);
        let white = (255u8, 255u8, 255u8);
        let ov_pole = if is_dark { white } else { black };

        // contrast threshold: WCAG AA 4.5, capped by the theme's own fg-vs-canvas ratio
        let own_cr = contrast_ratio(base_fg, base_bg);
        let threshold = own_cr.min(4.5_f32);

        // clamp_blend: shrink pct by 0.005 per step until contrast >= threshold - 0.001 (tolerance)
        let clamp_blend = |initial_pct: f32, pole: (u8, u8, u8)| -> Color {
            let mut pct = initial_pct;
            loop {
                if pct <= 0.0 { return base_bg; }
                let rung = blend(base_bg, pole, pct);
                if contrast_ratio(base_fg, rung) >= threshold - 0.001 { return rung; }
                pct -= 0.005;
            }
        };

        // ── Chrome ──────────────────────────────────────────────────────────────────
        if self.faces.chrome == Face::default() {
            let bg = clamp_blend(bar_pct, black);
            self.faces.chrome = Face { fg: Some(base_fg), bg: Some(bg), ..Face::default() };
        }

        // ── ChromeOverlay ───────────────────────────────────────────────────────────
        if self.faces.chrome_overlay == Face::default() {
            let bg = clamp_blend(overlay_pct, ov_pole);
            self.faces.chrome_overlay = Face { fg: Some(base_fg), bg: Some(bg), ..Face::default() };
        }

        // ── ChromeSelected ──────────────────────────────────────────────────────────
        if self.faces.chrome_selected == Face::default() {
            self.faces.chrome_selected = Face { fg: Some(base_bg), bg: Some(base_fg), ..Face::default() };
        }

        // ── ChromeMuted ─────────────────────────────────────────────────────────────
        if self.faces.chrome_muted == Face::default() {
            let bg = clamp_blend(deep_pct, black);
            let muted_fg = blend(base_fg, (bgr, bgg, bgb), MUTED_FG_BLEND);
            self.faces.chrome_muted = Face {
                fg: Some(muted_fg), bg: Some(bg), dim: Some(true), ..Face::default()
            };
        }

        // ── ChromeAccent ────────────────────────────────────────────────────────────
        // bg = Chrome.bg (read the now-resolved chrome face); fg = seed desaturated toward equal-lum gray.
        // seed = link fg (colored on every base16 + tokyo); fallback = base_fg.
        if self.faces.chrome_accent == Face::default() {
            let accent_bg = self.faces.chrome.bg.unwrap_or(base_bg);
            let seed = self.faces.link.fg.unwrap_or(base_fg);
            // The gray is derived once from the seed; both the initial desat and the zen
            // extra blend use the SAME gray target (equal_lum_gray of the original seed).
            let gray = equal_lum_gray(seed);
            let mut accent_fg = blend(seed, gray, ACCENT_DESAT);
            // zen: extra 0.40 blend toward the same seed-derived gray (convention 3)
            if disp == ChromeDisposition::Zen {
                accent_fg = blend(accent_fg, gray, 0.40);
            }
            self.faces.chrome_accent = Face {
                fg: Some(accent_fg), bg: Some(accent_bg), bold: Some(true), ..Face::default()
            };
        }
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
            ChromeOverlay => &mut self.faces.chrome_overlay, ChromeAccent => &mut self.faces.chrome_accent,
        }
    }
}

// helper for terse face literals
fn modface(fg: Option<Color>, bold: bool, italic: bool, underline: bool, strike: bool, reverse: bool) -> Face {
    Face { fg, bold: bold.then_some(true), italic: italic.then_some(true),
           underline: underline.then_some(true), strike: strike.then_some(true),
           reverse: reverse.then_some(true), ..Face::default() }
}

// ── Chrome derivation — fraction constants ────────────────────────────────────────────────────
// Calibrated against: tokyo #16161e≈18% toward black, mocha mantle ≈20%, latte mantle ≈3.2%,
// mocha crust ≈43%, latte crust ≈11%. One probe-solved set; see grounding §B.1.
const CHROME_BAR_PCT_DARK:     f32 = 0.18;
const CHROME_BAR_PCT_LIGHT:    f32 = 0.035;
const CHROME_OVERLAY_PCT_DARK: f32 = 0.09;
const CHROME_OVERLAY_PCT_LIGHT: f32 = 0.075;
const CHROME_DEEP_PCT_DARK:    f32 = 0.43;
const CHROME_DEEP_PCT_LIGHT:   f32 = 0.11;
const MUTED_FG_BLEND: f32 = 0.35;   // muted fg = blend(base_fg, base_bg, 0.35)
const ACCENT_DESAT:   f32 = 0.50;   // accent fg = blend(seed, equal_lum_gray(seed), 0.50)
const ZEN_COLLAPSE:   f32 = 0.35;   // zen multiplies each bg step; accent gets +0.40 extra

/// Per-channel linear interpolation toward `pole` at fraction `pct`.
/// `blend(base, pole, 0.0) == base`; `blend(base, pole, 1.0) == pole (rgb)`.
/// Non-Rgb `base` passes through unchanged.
fn blend(base: Color, pole: (u8, u8, u8), pct: f32) -> Color {
    let Color::Rgb { r, g, b } = base else { return base };
    let ch = |c: u8, p: u8| -> u8 {
        (c as f32 + (p as f32 - c as f32) * pct).round().clamp(0.0, 255.0) as u8
    };
    Color::Rgb { r: ch(r, pole.0), g: ch(g, pole.1), b: ch(b, pole.2) }
}

/// sRGB linearisation (IEC 61966-2-1).
fn srgb_lin(c: u8) -> f32 {
    let v = c as f32 / 255.0;
    if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Relative luminance per WCAG 2.1.
fn rel_lum(r: u8, g: u8, b: u8) -> f32 {
    0.2126 * srgb_lin(r) + 0.7152 * srgb_lin(g) + 0.0722 * srgb_lin(b)
}

/// WCAG 2.1 contrast ratio. Returns 1.0 if either color is non-Rgb.
pub(crate) fn contrast_ratio(a: Color, b: Color) -> f32 {
    let lum = |c: Color| -> f32 {
        if let Color::Rgb { r, g, b } = c { rel_lum(r, g, b) } else { 0.0 }
    };
    let la = lum(a); let lb = lum(b);
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Smallest integer gray `g` such that `rel_lum(g,g,g) >= rel_lum(seed)` (lower-bound search,
/// convention 2). Ensures the gray is AT LEAST as bright as the seed, not nearest.
fn equal_lum_gray(seed: Color) -> (u8, u8, u8) {
    let (r, g, b) = match seed { Color::Rgb { r, g, b } => (r, g, b), _ => return (128, 128, 128) };
    let target = rel_lum(r, g, b);
    for gv in 0u8..=255 {
        if rel_lum(gv, gv, gv) >= target { return (gv, gv, gv); }
    }
    (255, 255, 255)
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
        name: "terminal-plain".into(),
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
            // terminal-plain: non-Rgb bases → derive_chrome skips. ChromeOverlay exempt (D2/I5).
            // ChromeAccent explicit reverse+bold — a sentinel accent would compose empty forever (I4).
            chrome_overlay: Face::default(),
            chrome_accent: modface(None, true, false, false, false, true),
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
            // ChromeOverlay + ChromeAccent: all-None sentinels — derive_chrome fills them.
            // Chrome (PANEL_BG) is kept; the sentinel rule skips any non-all-None face.
            chrome_overlay: Face::default(),
            chrome_accent: Face::default(),
        },
    }
}

/// ANSI-named theme — explicit named-color chrome ladder (§C terminal-ansi table).
/// `base_fg/bg = Color::Default`; NOT monochrome; chrome faces fully explicit (unlike
/// terminal-plain whose overlay stays exempt). [verify at implementation: named-hue choices]
pub fn terminal_ansi() -> Theme {
    let m = |fg: Option<Color>, bold: bool, italic: bool, underline: bool, strike: bool, reverse: bool| Face {
        fg, bold: bold.then_some(true), italic: italic.then_some(true),
        underline: underline.then_some(true), strike: strike.then_some(true),
        reverse: reverse.then_some(true), ..Face::default()
    };
    Theme {
        name: "terminal-ansi".into(),
        base_fg: Color::Default, base_bg: Color::Default,
        heading_level_glyph: true, monochrome: false,
        faces: ThemeFaces {
            text: Face::default(),
            emphasis: m(None, false, true, false, false, false),
            strong: m(None, true, false, false, false, false),
            strong_emphasis: m(None, true, true, false, false, false),
            code: m(Some(Color::Green), false, false, false, false, false),
            strikethrough: m(None, false, false, false, true, false),
            link: m(Some(Color::Blue), false, false, true, false, false),
            heading: [
                m(Some(Color::Cyan),    true, false, false, false, false), // h1
                m(Some(Color::Blue),    true, false, false, false, false), // h2
                m(Some(Color::Green),   true, false, false, false, false), // h3
                m(Some(Color::Yellow),  true, false, false, false, false), // h4
                m(Some(Color::Magenta), true, false, false, false, false), // h5
                m(Some(Color::Red),     true, false, false, false, false), // h6
            ],
            block_quote: Face { fg: Some(Color::Cyan), italic: Some(true), ..Face::default() },
            code_block: m(Some(Color::Green), false, false, false, false, false),
            list_marker: m(Some(Color::Yellow), false, false, false, false, false),
            thematic_break: Face { fg: Some(Color::DarkGray), ..Face::default() },
            front_matter: Face { fg: Some(Color::Magenta), italic: Some(true), ..Face::default() },
            comment: Face { fg: Some(Color::DarkGray), italic: Some(true), dim: Some(true), ..Face::default() },
            selection: Face { reverse: Some(true), ..Face::default() },
            marked_block: Face { bg: Some(Color::DarkGray), reverse: Some(true), bold: Some(true), underline: Some(true), ..Face::default() },
            search_match: Face { bg: Some(Color::Yellow), fg: Some(Color::Black), ..Face::default() },
            search_current: Face { reverse: Some(true), bold: Some(true), ..Face::default() },
            diag_spelling: Face { underline: Some(true), underline_color: Some(Color::Red), ..Face::default() },
            diag_grammar:  Face { underline: Some(true), underline_color: Some(Color::Blue), ..Face::default() },
            focus_dim: Face { fg: Some(Color::DarkGray), ..Face::default() },
            fold_marker: Face { fg: Some(Color::DarkGray), ..Face::default() },
            wrap_guide: Face { fg: Some(Color::DarkGray), ..Face::default() },
            // Explicit named-ANSI chrome ladder (§C terminal-ansi table; D2 — unlike terminal-plain
            // whose overlay is exempt, terminal-ansi makes ChromeOverlay explicit).
            chrome:          Face { fg: Some(Color::White),    bg: Some(Color::Black),   ..Face::default() },
            chrome_reverse:  Face { reverse: Some(true), ..Face::default() },
            chrome_overlay:  Face { fg: Some(Color::White),    bg: Some(Color::DarkGray), ..Face::default() },
            chrome_selected: Face { fg: Some(Color::Black),    bg: Some(Color::White),   ..Face::default() },
            chrome_muted:    Face { fg: Some(Color::Gray),     bg: Some(Color::Black), dim: Some(true), ..Face::default() },
            chrome_accent:   Face { fg: Some(Color::LightCyan), bg: Some(Color::Black), bold: Some(true), ..Face::default() },
        },
    }
}

// ── E4 bundled themes — ten base16 palettes, chrome all-sentinel (derive_chrome fills them) ──

/// Catppuccin Mocha — src: tinted-theming/schemes base16/catppuccin-mocha.yaml; catppuccin.com/palette
pub fn catppuccin_mocha() -> Theme {
    from_base16("catppuccin-mocha", BasePalette { base: [
        rgb(0x1e,0x1e,0x2e), rgb(0x18,0x18,0x25), rgb(0x31,0x32,0x44), rgb(0x45,0x47,0x5a), // 00-03
        rgb(0x58,0x5b,0x70), rgb(0xcd,0xd6,0xf4), rgb(0xf5,0xe0,0xdc), rgb(0xb4,0xbe,0xfe), // 04-07
        rgb(0xf3,0x8b,0xa8), rgb(0xfa,0xb3,0x87), rgb(0xf9,0xe2,0xaf), rgb(0xa6,0xe3,0xa1), // 08-0B
        rgb(0x94,0xe2,0xd5), rgb(0x89,0xb4,0xfa), rgb(0xcb,0xa6,0xf7), rgb(0xf2,0xcd,0xcd), // 0C-0F
    ], extra: None })
}

/// Catppuccin Latte — src: tinted-theming/schemes base16/catppuccin-latte.yaml; catppuccin.com/palette
pub fn catppuccin_latte() -> Theme {
    from_base16("catppuccin-latte", BasePalette { base: [
        rgb(0xef,0xf1,0xf5), rgb(0xe6,0xe9,0xef), rgb(0xcc,0xd0,0xda), rgb(0xbc,0xc0,0xcc), // 00-03
        rgb(0xac,0xb0,0xbe), rgb(0x4c,0x4f,0x69), rgb(0xdc,0x8a,0x78), rgb(0x72,0x87,0xfd), // 04-07
        rgb(0xd2,0x0f,0x39), rgb(0xfe,0x64,0x0b), rgb(0xdf,0x8e,0x1d), rgb(0x40,0xa0,0x2b), // 08-0B
        rgb(0x17,0x92,0x99), rgb(0x1e,0x66,0xf5), rgb(0x88,0x39,0xef), rgb(0xdd,0x78,0x78), // 0C-0F
    ], extra: None })
}

/// Flexoki Dark — src: kepano/flexoki, stephango.com/flexoki — tones CONFIRMED; base16 = derived mapping
pub fn flexoki_dark() -> Theme {
    from_base16("flexoki-dark", BasePalette { base: [
        rgb(0x10,0x0f,0x0f), rgb(0x1c,0x1b,0x1a), rgb(0x28,0x27,0x26), rgb(0x57,0x56,0x53), // 00-03
        rgb(0x87,0x85,0x80), rgb(0xce,0xcd,0xc3), rgb(0xda,0xd8,0xce), rgb(0xe6,0xe4,0xd9), // 04-07
        rgb(0xd1,0x4d,0x41), rgb(0xda,0x70,0x2c), rgb(0xd0,0xa2,0x15), rgb(0x87,0x9a,0x39), // 08-0B
        rgb(0x3a,0xa9,0x9f), rgb(0x43,0x85,0xbe), rgb(0x8b,0x7e,0xc8), rgb(0xce,0x5d,0x97), // 0C-0F
    ], extra: None })
}

/// Flexoki Light — src: kepano/flexoki, stephango.com/flexoki — 600-step accents for on-light
pub fn flexoki_light() -> Theme {
    from_base16("flexoki-light", BasePalette { base: [
        rgb(0xff,0xfc,0xf0), rgb(0xf2,0xf0,0xe5), rgb(0xe6,0xe4,0xd9), rgb(0xb7,0xb5,0xac), // 00-03
        rgb(0x6f,0x6e,0x69), rgb(0x10,0x0f,0x0f), rgb(0x1c,0x1b,0x1a), rgb(0x28,0x27,0x26), // 04-07
        rgb(0xaf,0x30,0x29), rgb(0xbc,0x52,0x15), rgb(0xad,0x83,0x01), rgb(0x66,0x80,0x0b), // 08-0B
        rgb(0x24,0x83,0x7b), rgb(0x20,0x5e,0xa6), rgb(0x5e,0x40,0x9d), rgb(0xa0,0x2f,0x6f), // 0C-0F
    ], extra: None })
}

/// Gruvbox Dark (medium) — src: tinted-theming base16/gruvbox-dark-medium.yaml; morhetz/gruvbox
pub fn gruvbox_dark() -> Theme {
    from_base16("gruvbox-dark", BasePalette { base: [
        rgb(0x28,0x28,0x28), rgb(0x3c,0x38,0x36), rgb(0x50,0x49,0x45), rgb(0x66,0x5c,0x54), // 00-03
        rgb(0xbd,0xae,0x93), rgb(0xd5,0xc4,0xa1), rgb(0xeb,0xdb,0xb2), rgb(0xfb,0xf1,0xc7), // 04-07
        rgb(0xfb,0x49,0x34), rgb(0xfe,0x80,0x19), rgb(0xfa,0xbd,0x2f), rgb(0xb8,0xbb,0x26), // 08-0B
        rgb(0x8e,0xc0,0x7c), rgb(0x83,0xa5,0x98), rgb(0xd3,0x86,0x9b), rgb(0xd6,0x5d,0x0e), // 0C-0F
    ], extra: None })
}

/// Gruvbox Light (medium) — src: tinted-theming base16/gruvbox-light-medium.yaml; morhetz/gruvbox
pub fn gruvbox_light() -> Theme {
    from_base16("gruvbox-light", BasePalette { base: [
        rgb(0xfb,0xf1,0xc7), rgb(0xeb,0xdb,0xb2), rgb(0xd5,0xc4,0xa1), rgb(0xbd,0xae,0x93), // 00-03
        rgb(0x66,0x5c,0x54), rgb(0x50,0x49,0x45), rgb(0x3c,0x38,0x36), rgb(0x28,0x28,0x28), // 04-07
        rgb(0x9d,0x00,0x06), rgb(0xaf,0x3a,0x03), rgb(0xb5,0x76,0x14), rgb(0x79,0x74,0x0e), // 08-0B
        rgb(0x42,0x7b,0x58), rgb(0x07,0x66,0x78), rgb(0x8f,0x3f,0x71), rgb(0xd6,0x5d,0x0e), // 0C-0F
    ], extra: None })
}

/// Rosé Pine Moon — src: base16/rose-pine-moon.yaml; rosepinetheme.com/palette
pub fn rosepine_moon() -> Theme {
    from_base16("rosepine-moon", BasePalette { base: [
        rgb(0x23,0x21,0x36), rgb(0x2a,0x27,0x3f), rgb(0x39,0x35,0x52), rgb(0x6e,0x6a,0x86), // 00-03
        rgb(0x90,0x8c,0xaa), rgb(0xe0,0xde,0xf4), rgb(0xe0,0xde,0xf4), rgb(0x56,0x52,0x6e), // 04-07
        rgb(0xeb,0x6f,0x92), rgb(0xf6,0xc1,0x77), rgb(0xea,0x9a,0x97), rgb(0x3e,0x8f,0xb0), // 08-0B
        rgb(0x9c,0xcf,0xd8), rgb(0xc4,0xa7,0xe7), rgb(0xf6,0xc1,0x77), rgb(0x56,0x52,0x6e), // 0C-0F
    ], extra: None })
}

/// Rosé Pine Dawn — src: base16/rose-pine-dawn.yaml; rosepinetheme.com
pub fn rosepine_dawn() -> Theme {
    from_base16("rosepine-dawn", BasePalette { base: [
        rgb(0xfa,0xf4,0xed), rgb(0xff,0xfa,0xf3), rgb(0xf2,0xe9,0xde), rgb(0x98,0x93,0xa5), // 00-03
        rgb(0x79,0x75,0x93), rgb(0x57,0x52,0x79), rgb(0x57,0x52,0x79), rgb(0xce,0xca,0xcd), // 04-07
        rgb(0xb4,0x63,0x7a), rgb(0xea,0x9d,0x34), rgb(0xd7,0x82,0x7e), rgb(0x28,0x69,0x83), // 08-0B
        rgb(0x56,0x94,0x9f), rgb(0x90,0x7a,0xa9), rgb(0xea,0x9d,0x34), rgb(0xce,0xca,0xcd), // 0C-0F
    ], extra: None })
}

/// Solarized Dark — src: base16/solarized-dark.yaml; ethanschoonover.com/solarized
pub fn solarized_dark() -> Theme {
    from_base16("solarized-dark", BasePalette { base: [
        rgb(0x00,0x2b,0x36), rgb(0x07,0x36,0x42), rgb(0x58,0x6e,0x75), rgb(0x65,0x7b,0x83), // 00-03
        rgb(0x83,0x94,0x96), rgb(0x93,0xa1,0xa1), rgb(0xee,0xe8,0xd5), rgb(0xfd,0xf6,0xe3), // 04-07
        rgb(0xdc,0x32,0x2f), rgb(0xcb,0x4b,0x16), rgb(0xb5,0x89,0x00), rgb(0x85,0x99,0x00), // 08-0B
        rgb(0x2a,0xa1,0x98), rgb(0x26,0x8b,0xd2), rgb(0x6c,0x71,0xc4), rgb(0xd3,0x36,0x82), // 0C-0F
    ], extra: None })
}

/// Solarized Light — src: base16/solarized-light.yaml; ethanschoonover.com/solarized
pub fn solarized_light() -> Theme {
    from_base16("solarized-light", BasePalette { base: [
        rgb(0xfd,0xf6,0xe3), rgb(0xee,0xe8,0xd5), rgb(0x93,0xa1,0xa1), rgb(0x83,0x94,0x96), // 00-03
        rgb(0x65,0x7b,0x83), rgb(0x58,0x6e,0x75), rgb(0x07,0x36,0x42), rgb(0x00,0x2b,0x36), // 04-07
        rgb(0xdc,0x32,0x2f), rgb(0xcb,0x4b,0x16), rgb(0xb5,0x89,0x00), rgb(0x85,0x99,0x00), // 08-0B
        rgb(0x2a,0xa1,0x98), rgb(0x26,0x8b,0xd2), rgb(0x6c,0x71,0xc4), rgb(0xd3,0x36,0x82), // 0C-0F
    ], extra: None })
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
    let _panel = p.extra.map(|e| e[0]).unwrap_or(b[1]); // base10 if base24, else base01 — unused after I3 sentinel migration
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
            // chrome/selected/muted: all-None sentinels — derive_chrome fills them (I3).
            // chrome_reverse: keep reverse default (never derived — D1 contract).
            chrome: Face::default(),
            chrome_reverse: Face { reverse: Some(true), ..Face::default() },
            chrome_selected: Face::default(),
            chrome_muted: Face::default(),
            chrome_overlay: Face::default(),
            chrome_accent: Face::default(),
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
        "chrome_overlay" => ChromeOverlay, "chrome_accent" => ChromeAccent,
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

/// The monochrome (modifier-cue) face set for `no_color()`.
/// §4-layer-1 discipline: every Face-cued element carries ≥1 non-color modifier.
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
        // ChromeOverlay: exempt from cue requirement (fill face, no glyph — M4 a11y).
        // ChromeAccent: reverse+bold — glyph-bearing, testable under no-color.
        chrome_overlay: Face::default(),
        chrome_accent: m(true, false, false, false, true),
    }
}

pub fn phosphor(name: &str, hue: Color) -> Theme {
    let bg = shade(hue, 0);           // near-black hue
    let fg = shade(hue, 3);           // mid-bright hue
    let s = |n| Face { fg: Some(shade(hue, n)), ..Face::default() };
    let faces = ThemeFaces {
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
        // chrome/selected/muted/overlay/accent: all-None sentinels — derive_chrome fills them (I4-A).
        // chrome_reverse: kept reverse-modifier default (never derived — D1 contract).
        chrome: Face::default(),
        chrome_reverse: Face { reverse: Some(true), ..Face::default() },
        chrome_selected: Face::default(),
        chrome_muted: Face::default(),
        chrome_overlay: Face::default(),
        chrome_accent: Face::default(),
    };
    Theme { name: name.into(), base_fg: fg, base_bg: bg, heading_level_glyph: true, monochrome: false, faces }
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

    const ALL_ELEMENTS: [SemanticElement; 34] = {
        use SemanticElement::*;
        [Text, Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link,
         Heading(1), Heading(2), Heading(3), Heading(4), Heading(5), Heading(6),
         BlockQuote, CodeBlock, ListMarker, ThematicBreak, FrontMatter, Comment, Selection, MarkedBlock,
         SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar, FocusDim, FoldMarker, WrapGuide,
         Chrome, ChromeReverse, ChromeSelected, ChromeMuted, ChromeOverlay, ChromeAccent]
    };
    // 34 = Text + 6 inline + 6 heading + 4 block + 4 (fm/comment/sel/marked-block) + 7 overlay + 6 chrome.
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
    fn phosphor_shaded_distinguishes_by_shade() {
        let amber = Color::Rgb { r: 255, g: 176, b: 0 };
        let t = phosphor("phosphor-amber", amber);
        assert!(!t.monochrome);
        assert_ne!(t.face(SemanticElement::Heading(1)).fg, t.face(SemanticElement::Comment).fg);
        // chrome faces are all-None sentinels (I4-A) — derive_chrome fills them
        assert_eq!(t.face(SemanticElement::Chrome), Face::default(), "chrome sentinel pre-derive");
        assert_eq!(t.face(SemanticElement::ChromeMuted), Face::default(), "muted sentinel pre-derive");
    }
    #[test]
    fn builtin_names_final_nineteen() {
        // Every name resolves; every face is total; count is exactly 19; no -flat names.
        for name in Theme::builtin_names() {
            let t = Theme::builtin(name).expect(name);
            for el in ALL_ELEMENTS { let _ = t.face(el); }
        }
        assert_eq!(Theme::builtin_names().len(), 19);
        assert_eq!(Theme::builtin_names()[0], "terminal-plain"); // D5 first entry
        assert_eq!(Theme::builtin_names()[1], "terminal-ansi");  // D5 second entry
        assert!(!Theme::builtin_names().iter().any(|n| n.contains("-flat")), "no flat variants");
    }

    #[test]
    fn every_builtin_resolves_at_all_depths() {
        // 19 builtins × 3 depths — every quantize call completes without panic.
        for name in Theme::builtin_names() {
            let t = Theme::builtin(name).expect(name);
            for depth in [Depth::Truecolor, Depth::Indexed256, Depth::Ansi16] {
                for el in ALL_ELEMENTS {
                    let f = t.face(el);
                    let _ = quantize(f.fg.unwrap_or(Color::Default), depth);
                    let _ = quantize(f.bg.unwrap_or(Color::Default), depth);
                }
            }
        }
    }

    #[test]
    fn derived_rungs_distinct_at_256() {
        // §B.5 corrected indices. flexoki-dark: Chrome+Muted collapse on canvas; only Overlay distinct.
        let mut t = from_base16("flexoki-dark", flexoki_dark_palette());
        t.derive_chrome(ChromeDisposition::Full);
        let canvas  = quantize(t.base_bg, Depth::Indexed256);
        let chrome  = quantize(t.face(SemanticElement::Chrome).bg.unwrap(),        Depth::Indexed256);
        let overlay = quantize(t.face(SemanticElement::ChromeOverlay).bg.unwrap(), Depth::Indexed256);
        let muted   = quantize(t.face(SemanticElement::ChromeMuted).bg.unwrap(),   Depth::Indexed256);
        assert_eq!(canvas,  Color::Indexed(232), "flexoki-dark canvas → 232");
        assert_eq!(chrome,  Color::Indexed(232), "flexoki-dark chrome collapses onto canvas at 256");
        assert_eq!(overlay, Color::Indexed(234), "flexoki-dark overlay distinct at 256");
        assert_eq!(muted,   Color::Indexed(232), "flexoki-dark muted collapses onto canvas at 256");

        // catppuccin-mocha: Chrome collapses onto canvas; Overlay distinct.
        let mut t2 = from_base16("mocha", mocha_palette());
        t2.derive_chrome(ChromeDisposition::Full);
        let canvas2  = quantize(t2.base_bg, Depth::Indexed256);
        let chrome2  = quantize(t2.face(SemanticElement::Chrome).bg.unwrap(),        Depth::Indexed256);
        let overlay2 = quantize(t2.face(SemanticElement::ChromeOverlay).bg.unwrap(), Depth::Indexed256);
        assert_eq!(canvas2,  Color::Indexed(234), "mocha canvas → 234");
        assert_eq!(chrome2,  Color::Indexed(234), "mocha chrome collapses onto canvas at 256");
        assert_eq!(overlay2, Color::Indexed(236), "mocha overlay distinct at 256");

        // gruvbox-dark: Chrome distinct from canvas at 256 (grounding §B.5 row 3).
        let mut t3 = from_base16("gruvbox-dark", sample_base16());
        t3.derive_chrome(ChromeDisposition::Full);
        let canvas3  = quantize(t3.base_bg, Depth::Indexed256);
        let chrome3  = quantize(t3.face(SemanticElement::Chrome).bg.unwrap(),        Depth::Indexed256);
        let overlay3 = quantize(t3.face(SemanticElement::ChromeOverlay).bg.unwrap(), Depth::Indexed256);
        assert_eq!(canvas3,  Color::Indexed(235), "gruvbox-dark canvas → 235");
        assert_eq!(chrome3,  Color::Indexed(234), "gruvbox-dark chrome → 234 (distinct from canvas)");
        assert_eq!(overlay3, Color::Indexed(237), "gruvbox-dark overlay → 237");
    }

    #[test]
    fn all_color_themes_fully_explicit_after_derive() {
        // Every Rgb-based builtin: after derive_chrome(Full) no chrome face is all-None.
        // terminal-plain, terminal-ansi, no-color: Color::Default bases → derive skips → excluded.
        let color_chrome = [
            SemanticElement::Chrome, SemanticElement::ChromeSelected,
            SemanticElement::ChromeMuted, SemanticElement::ChromeOverlay, SemanticElement::ChromeAccent,
        ];
        for name in Theme::builtin_names() {
            let mut t = Theme::builtin(name).expect(name);
            if t.base_bg == Color::Default { continue; } // skip non-Rgb themes
            t.derive_chrome(ChromeDisposition::Full);
            for el in color_chrome {
                let f = t.face(el);
                assert!(f.fg.is_some() || f.bg.is_some(),
                    "{name}/{el:?}: chrome face must not be all-None after derive_chrome(Full)");
            }
        }
    }

    #[test]
    fn exemplar_spot_pins_mocha_and_flexoki_light() {
        // catppuccin-mocha §C — base_bg, base_fg, h1 (base0D blue, bold)
        let mocha = catppuccin_mocha();
        assert_eq!(mocha.base_bg, rgb(0x1e, 0x1e, 0x2e), "mocha base_bg");
        assert_eq!(mocha.base_fg, rgb(0xcd, 0xd6, 0xf4), "mocha base_fg");
        let h1m = mocha.face(SemanticElement::Heading(1));
        assert_eq!(h1m.fg, Some(rgb(0x89, 0xb4, 0xfa)), "mocha h1 = blue base0D");
        assert_eq!(h1m.bold, Some(true), "mocha h1 bold");

        // flexoki-light §C — base_bg, base_fg, h1 (base0D blue 205ea6, bold)
        let fl = flexoki_light();
        assert_eq!(fl.base_bg, rgb(0xff, 0xfc, 0xf0), "flexoki-light base_bg");
        assert_eq!(fl.base_fg, rgb(0x10, 0x0f, 0x0f), "flexoki-light base_fg");
        let h1f = fl.face(SemanticElement::Heading(1));
        assert_eq!(h1f.fg, Some(rgb(0x20, 0x5e, 0xa6)), "flexoki-light h1 = blue base0D");
        assert_eq!(h1f.bold, Some(true), "flexoki-light h1 bold");
    }

    #[test]
    fn terminal_ansi_all_named_colors() {
        // terminal-ansi: base Default, NOT monochrome, every chrome face named-ANSI or modifier-only.
        let t = terminal_ansi();
        assert_eq!(t.base_fg, Color::Default, "terminal-ansi base_fg = Default");
        assert_eq!(t.base_bg, Color::Default, "terminal-ansi base_bg = Default");
        assert!(!t.monochrome, "terminal-ansi NOT monochrome");
        // Chrome faces must use named ANSI colors (not Rgb) — spot check
        let chrome = t.face(SemanticElement::Chrome);
        assert_eq!(chrome.fg, Some(Color::White),  "chrome fg White");
        assert_eq!(chrome.bg, Some(Color::Black),  "chrome bg Black");
        let ov = t.face(SemanticElement::ChromeOverlay);
        assert_eq!(ov.fg, Some(Color::White),    "overlay fg White");
        assert_eq!(ov.bg, Some(Color::DarkGray), "overlay bg DarkGray");
        let sel = t.face(SemanticElement::ChromeSelected);
        assert_eq!(sel.fg, Some(Color::Black), "selected fg Black");
        assert_eq!(sel.bg, Some(Color::White), "selected bg White");
        let acc = t.face(SemanticElement::ChromeAccent);
        assert_eq!(acc.fg, Some(Color::LightCyan), "accent fg LightCyan");
        assert_eq!(acc.bg, Some(Color::Black),     "accent bg Black");
        assert_eq!(acc.bold, Some(true),            "accent bold");
        // All text/chrome elements: no Rgb face values (named ANSI or modifier-only or Default)
        for el in ALL_ELEMENTS {
            let f = t.face(el);
            for c in [f.fg, f.bg, f.underline_color].into_iter().flatten() {
                assert!(!matches!(c, Color::Rgb{..}), "terminal-ansi/{el:?} must not use Rgb; got {c:?}");
            }
        }
    }

    #[test]
    fn terminal_plain_name_and_faces() {
        // default() returns name "terminal-plain"; chrome explicitly non-Rgb; derive is no-op.
        let t = default();
        assert_eq!(t.name, "terminal-plain", "name field");
        assert_eq!(t.base_fg, Color::Default);
        assert_eq!(t.base_bg, Color::Default);
        assert!(!t.monochrome);
        assert_eq!(t.face(SemanticElement::Chrome).fg, Some(Color::White));
        assert_eq!(t.face(SemanticElement::Chrome).bg, Some(Color::Black));
        // builtin("terminal-plain") returns terminal-plain; "default" alias lives in resolve_theme (T3).
        let b1 = Theme::builtin("terminal-plain").unwrap();
        assert_eq!(b1.name, "terminal-plain");
        // non-Rgb bases → derive is a no-op (derive_skips_non_rgb_bases also covers this)
        let before = t.faces.clone();
        let mut t2 = t;
        t2.derive_chrome(ChromeDisposition::Full);
        assert_eq!(t2.faces, before, "terminal-plain: derive no-op on Default bases");
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
        assert_eq!(element_from_key("chrome_overlay"), Some(ChromeOverlay));
        assert_eq!(element_from_key("chrome_accent"), Some(ChromeAccent));
        assert_eq!(element_from_key("nope"), None);
        assert_eq!(element_from_key("heading0"), None); // out of range
        assert_eq!(element_from_key("heading7"), None);
    }

    // ── Derivation test battery ──────────────────────────────────────────────────────────────────

    // Convenience: build a BasePalette from named-color sections of the grounding §C tables.
    fn base16_palette(slots: [(u8,u8,u8);16]) -> BasePalette {
        let c = |(r,g,b)| Color::Rgb { r, g, b };
        BasePalette { base: slots.map(c), extra: None }
    }

    // Assert a face has the given bg/fg in #rrggbb form, optional bold/dim.
    fn assert_face_bg_fg(face: Face, bg: (u8,u8,u8), fg: (u8,u8,u8), label: &str) {
        assert_eq!(face.bg, Some(Color::Rgb { r:bg.0, g:bg.1, b:bg.2 }), "{label} bg");
        assert_eq!(face.fg, Some(Color::Rgb { r:fg.0, g:fg.1, b:fg.2 }), "{label} fg");
    }

    // flexoki-dark palette (grounding §C)
    fn flexoki_dark_palette() -> BasePalette {
        base16_palette([
            (0x10,0x0f,0x0f),(0x1c,0x1b,0x1a),(0x28,0x27,0x26),(0x57,0x56,0x53),
            (0x87,0x85,0x80),(0xce,0xcd,0xc3),(0xda,0xd8,0xce),(0xe6,0xe4,0xd9),
            (0xd1,0x4d,0x41),(0xda,0x70,0x2c),(0xd0,0xa2,0x15),(0x87,0x9a,0x39),
            (0x3a,0xa9,0x9f),(0x43,0x85,0xbe),(0x8b,0x7e,0xc8),(0xce,0x5d,0x97),
        ])
    }

    // flexoki-light palette (grounding §C)
    fn flexoki_light_palette() -> BasePalette {
        base16_palette([
            (0xff,0xfc,0xf0),(0xf2,0xf0,0xe5),(0xe6,0xe4,0xd9),(0xb7,0xb5,0xac),
            (0x6f,0x6e,0x69),(0x10,0x0f,0x0f),(0x1c,0x1b,0x1a),(0x28,0x27,0x26),
            (0xaf,0x30,0x29),(0xbc,0x52,0x15),(0xad,0x83,0x01),(0x66,0x80,0x0b),
            (0x24,0x83,0x7b),(0x20,0x5e,0xa6),(0x5e,0x40,0x9d),(0xa0,0x2f,0x6f),
        ])
    }

    // catppuccin-mocha palette (grounding §C)
    fn mocha_palette() -> BasePalette {
        base16_palette([
            (0x1e,0x1e,0x2e),(0x18,0x18,0x25),(0x31,0x32,0x44),(0x45,0x47,0x5a),
            (0x58,0x5b,0x70),(0xcd,0xd6,0xf4),(0xf5,0xe0,0xdc),(0xb4,0xbe,0xfe),
            (0xf3,0x8b,0xa8),(0xfa,0xb3,0x87),(0xf9,0xe2,0xaf),(0xa6,0xe3,0xa1),
            (0x94,0xe2,0xd5),(0x89,0xb4,0xfa),(0xcb,0xa6,0xf7),(0xf2,0xcd,0xcd),
        ])
    }

    // phosphor-green bases: canvas=shade(hue,0), fg=shade(hue,3), link=shade(hue,5)
    fn phosphor_green_theme() -> Theme {
        let hue = Color::Rgb { r:0x33, g:0xff, b:0x33 };
        phosphor("phosphor-green", hue)
    }

    #[test]
    fn derive_fills_only_unset_faces() {
        // tokyo-night: chrome/chrome_selected/chrome_muted are explicitly set (non-sentinel).
        // only chrome_overlay + chrome_accent (all-None sentinels) should be derived.
        let mut t = tokyo_night();
        let chrome_before    = t.face(SemanticElement::Chrome);
        let selected_before  = t.face(SemanticElement::ChromeSelected);
        let muted_before     = t.face(SemanticElement::ChromeMuted);
        let reverse_before   = t.face(SemanticElement::ChromeReverse);
        let overlay_before   = t.face(SemanticElement::ChromeOverlay);
        let accent_before    = t.face(SemanticElement::ChromeAccent);
        assert!(overlay_before == Face::default(), "overlay must start as sentinel");
        assert!(accent_before == Face::default(), "accent must start as sentinel");

        t.derive_chrome(ChromeDisposition::Full);

        // the four explicitly-set faces survive unchanged
        assert_eq!(t.face(SemanticElement::Chrome),         chrome_before,   "chrome kept");
        assert_eq!(t.face(SemanticElement::ChromeSelected), selected_before, "selected kept");
        assert_eq!(t.face(SemanticElement::ChromeMuted),    muted_before,    "muted kept");
        assert_eq!(t.face(SemanticElement::ChromeReverse),  reverse_before,  "reverse kept — never derived");
        // the two new faces are now derived (non-sentinel)
        assert_ne!(t.face(SemanticElement::ChromeOverlay), Face::default(), "overlay derived");
        assert_ne!(t.face(SemanticElement::ChromeAccent),  Face::default(), "accent derived");
        // tokyo §B.3 pins
        assert_face_bg_fg(t.face(SemanticElement::ChromeOverlay),
            (0x2f,0x30,0x3a), (0xc0,0xca,0xf5), "tokyo overlay FULL");
        assert_face_bg_fg(t.face(SemanticElement::ChromeAccent),
            (0x16,0x16,0x1e), (0x8f,0xa3,0xce), "tokyo accent FULL");
        assert_eq!(t.face(SemanticElement::ChromeAccent).bold, Some(true), "accent bold");

        // second call is a no-op (idempotency — sentinel rule)
        let snap = t.clone();
        t.derive_chrome(ChromeDisposition::Full);
        assert_eq!(t.faces, snap.faces, "second derive is no-op");
    }

    #[test]
    fn derive_split_ladder_directions() {
        // flexoki-dark: bar darker than canvas (toward black), overlay LIGHTER (toward white)
        let mut td = from_base16("flexoki-dark", flexoki_dark_palette());
        td.derive_chrome(ChromeDisposition::Full);
        // §B.3 flexoki-dark FULL pins
        assert_face_bg_fg(td.face(SemanticElement::Chrome),
            (0x0d,0x0c,0x0c), (0xce,0xcd,0xc3), "fd chrome bg/fg");
        assert_face_bg_fg(td.face(SemanticElement::ChromeOverlay),
            (0x26,0x25,0x25), (0xce,0xcd,0xc3), "fd overlay bg/fg");
        assert_face_bg_fg(td.face(SemanticElement::ChromeSelected),
            (0xce,0xcd,0xc3), (0x10,0x0f,0x0f), "fd selected bg/fg");
        assert_face_bg_fg(td.face(SemanticElement::ChromeMuted),
            (0x09,0x09,0x09), (0x8c,0x8b,0x84), "fd muted bg/fg");
        assert_eq!(td.face(SemanticElement::ChromeMuted).dim, Some(true), "fd muted dim");
        assert_face_bg_fg(td.face(SemanticElement::ChromeAccent),
            (0x0d,0x0c,0x0c), (0x62,0x83,0xa0), "fd accent bg/fg");
        assert_eq!(td.face(SemanticElement::ChromeAccent).bold, Some(true), "fd accent bold");

        // flexoki-light: bar darker than canvas (toward black), overlay DEEPER black
        let mut tl = from_base16("flexoki-light", flexoki_light_palette());
        tl.derive_chrome(ChromeDisposition::Full);
        // §B.3 flexoki-light FULL pins
        assert_face_bg_fg(tl.face(SemanticElement::Chrome),
            (0xf6,0xf3,0xe8), (0x10,0x0f,0x0f), "fl chrome bg/fg");
        assert_face_bg_fg(tl.face(SemanticElement::ChromeOverlay),
            (0xec,0xe9,0xde), (0x10,0x0f,0x0f), "fl overlay bg/fg");
        assert_face_bg_fg(tl.face(SemanticElement::ChromeSelected),
            (0x10,0x0f,0x0f), (0xff,0xfc,0xf0), "fl selected bg/fg");
        assert_face_bg_fg(tl.face(SemanticElement::ChromeMuted),
            (0xe3,0xe0,0xd6), (0x64,0x62,0x5e), "fl muted bg/fg");
        assert_face_bg_fg(tl.face(SemanticElement::ChromeAccent),
            (0xf6,0xf3,0xe8), (0x3f,0x5e,0x82), "fl accent bg/fg");
    }

    #[test]
    fn derive_zen_collapses_toward_poles() {
        let mut td_full = from_base16("flexoki-dark", flexoki_dark_palette());
        td_full.derive_chrome(ChromeDisposition::Full);
        let mut td_zen = from_base16("flexoki-dark", flexoki_dark_palette());
        td_zen.derive_chrome(ChromeDisposition::Zen);

        // Zen §B.3 pins (flexoki-dark ZEN)
        assert_face_bg_fg(td_zen.face(SemanticElement::Chrome),
            (0x0f,0x0e,0x0e), (0xce,0xcd,0xc3), "fd zen chrome");
        assert_face_bg_fg(td_zen.face(SemanticElement::ChromeOverlay),
            (0x18,0x17,0x17), (0xce,0xcd,0xc3), "fd zen overlay");
        assert_face_bg_fg(td_zen.face(SemanticElement::ChromeMuted),
            (0x0e,0x0d,0x0d), (0x8c,0x8b,0x84), "fd zen muted");
        assert_face_bg_fg(td_zen.face(SemanticElement::ChromeAccent),
            (0x0f,0x0e,0x0e), (0x6e,0x82,0x94), "fd zen accent");

        // zen bg rungs are strictly between canvas and the full rungs (each toward its own pole)
        // canvas = #100f0f; chrome full bg = #0d0c0c (darker); zen bg = #0f0e0e (less dark → closer to canvas)
        let canvas_r = 0x10u8;
        let full_chr_r  = td_full.face(SemanticElement::Chrome).bg.unwrap();
        let zen_chr_r   = td_zen.face(SemanticElement::Chrome).bg.unwrap();
        if let (Color::Rgb{r:fr,..}, Color::Rgb{r:zr,..}) = (full_chr_r, zen_chr_r) {
            // full goes darker (fr ≤ canvas), zen is less dark (zr ≥ fr)
            assert!(zr >= fr, "zen chrome closer to canvas than full: zen={zr} full={fr}");
            assert!(zr <= canvas_r, "zen ≤ canvas for bar (dark→black)");
        } else { panic!("non-Rgb chrome bg"); }
        // overlay: full goes lighter (#262525, overlay_r=0x26), zen = #181717 (less light, still above canvas)
        let full_ov = td_full.face(SemanticElement::ChromeOverlay).bg.unwrap();
        let zen_ov  = td_zen.face(SemanticElement::ChromeOverlay).bg.unwrap();
        if let (Color::Rgb{r:fr,..}, Color::Rgb{r:zr,..}) = (full_ov, zen_ov) {
            assert!(fr >= canvas_r, "full overlay lighter than canvas for dark theme");
            assert!(zr >= canvas_r && zr <= fr, "zen overlay between canvas and full");
        } else { panic!("non-Rgb overlay bg"); }
    }

    #[test]
    fn derive_saturation_split() {
        // Dark themes only (polarity-scoped per Fable r2). Mocha §B.3 pins.
        // sunken rung (Chrome/Muted, toward black) S ≈ canvas S within rounding epsilon.
        // raised rung (Overlay, toward white) S strictly < canvas S.
        let mut t = from_base16("mocha", mocha_palette());
        t.derive_chrome(ChromeDisposition::Full);
        let canvas_s = { let (_,s,_) = rgb_to_hsl(0x1e, 0x1e, 0x2e); s };
        let overlay = t.face(SemanticElement::ChromeOverlay).bg.unwrap();
        if let Color::Rgb { r, g, b } = overlay {
            let (_,s,_) = rgb_to_hsl(r, g, b);
            assert!(s < canvas_s, "raised overlay S < canvas S (mocha): overlay_s={s:.4} canvas_s={canvas_s:.4}");
        } else { panic!("non-Rgb overlay"); }
        // Chrome bg for mocha = #191926; S should be close to canvas S
        let chrome_bg = t.face(SemanticElement::Chrome).bg.unwrap();
        if let Color::Rgb { r, g, b } = chrome_bg {
            let (_,s,_) = rgb_to_hsl(r, g, b);
            // sunken rung S preserved: within ~0.01 of canvas S
            assert!((s - canvas_s).abs() < 0.012,
                "sunken chrome S ≈ canvas S (mocha): chrome_s={s:.4} canvas_s={canvas_s:.4}");
        } else { panic!("non-Rgb chrome"); }
    }

    #[test]
    fn derive_accent_desaturation_bound() {
        // Accent S must be strictly less than seed S (desaturation is a strict decrease).
        let mut t = from_base16("flexoki-dark", flexoki_dark_palette());
        t.derive_chrome(ChromeDisposition::Full);
        let seed = t.face(SemanticElement::Link).fg.unwrap(); // base0D = #4385be
        let accent_fg = t.face(SemanticElement::ChromeAccent).fg.unwrap();
        if let (Color::Rgb{r:sr,g:sg,b:sb}, Color::Rgb{r:ar,g:ag,b:ab}) = (seed, accent_fg) {
            let (_,seed_s,_)   = rgb_to_hsl(sr, sg, sb);
            let (_,accent_s,_) = rgb_to_hsl(ar, ag, ab);
            assert!(accent_s < seed_s,
                "accent S must be < seed S: accent_s={accent_s:.4} seed_s={seed_s:.4}");
        } else { panic!("non-Rgb seed or accent"); }
    }

    #[test]
    fn derive_preserves_hue_angle() {
        // phosphor-green post-derivation: every derived bg rung has the same hue family as the canvas.
        // §B.3 phosphor-green FULL pins.
        let mut t = phosphor_green_theme();
        t.derive_chrome(ChromeDisposition::Full);
        assert_face_bg_fg(t.face(SemanticElement::Chrome),
            (0x00,0x22,0x00), (0x2b,0xff,0x2b), "phosphor chrome bg/fg");
        assert_face_bg_fg(t.face(SemanticElement::ChromeOverlay),
            (0x17,0x3c,0x17), (0x2b,0xff,0x2b), "phosphor overlay bg/fg");
        assert_face_bg_fg(t.face(SemanticElement::ChromeMuted),
            (0x00,0x17,0x00), (0x1c,0xb4,0x1c), "phosphor muted bg/fg");
        assert_face_bg_fg(t.face(SemanticElement::ChromeAccent),
            (0x00,0x22,0x00), (0xe6,0xfa,0xe6), "phosphor accent bg/fg");
        // hue angle: all bg rungs share the green hue family (r ≈ b, g dominates)
        for el in [SemanticElement::Chrome, SemanticElement::ChromeOverlay, SemanticElement::ChromeMuted] {
            let bg = t.face(el).bg.unwrap();
            if let Color::Rgb { r, g, b } = bg {
                assert!(g >= r && g >= b, "{el:?} bg must be green-dominant r={r} g={g} b={b}");
            } else { panic!("{el:?} non-Rgb bg"); }
        }
    }

    #[test]
    fn derive_contrast_clamp_floors_at_zero() {
        // Synthetic LIGHT-polarity theme where fg-vs-canvas contrast is below 4.5,
        // so all three bg rungs (toward black) shrink until contrast ≥ threshold - 0.001.
        // Using a very-low-contrast LIGHT palette: white BG, near-white FG → contrast ≈ 1.24.
        // All rungs will floor at pct=0 (= canvas), no panic, no infinite loop.
        let white_bg = Color::Rgb { r:0xf8, g:0xf8, b:0xf8 };
        let near_white_fg = Color::Rgb { r:0xe0, g:0xe0, b:0xe0 };
        let mut t = Theme {
            name: "synthetic-low-contrast".into(),
            base_fg: near_white_fg,
            base_bg: white_bg,
            heading_level_glyph: false,
            monochrome: false,
            faces: ThemeFaces {
                text: Face::default(), emphasis: Face::default(), strong: Face::default(),
                strong_emphasis: Face::default(), code: Face::default(), strikethrough: Face::default(),
                link: Face { fg: Some(near_white_fg), ..Face::default() },
                heading: [Face::default(); 6],
                block_quote: Face::default(), code_block: Face::default(), list_marker: Face::default(),
                thematic_break: Face::default(), front_matter: Face::default(), comment: Face::default(),
                selection: Face::default(), marked_block: Face::default(),
                search_match: Face::default(), search_current: Face::default(),
                diag_spelling: Face::default(), diag_grammar: Face::default(),
                focus_dim: Face::default(), fold_marker: Face::default(), wrap_guide: Face::default(),
                chrome: Face::default(), chrome_reverse: Face { reverse: Some(true), ..Face::default() },
                chrome_selected: Face::default(), chrome_muted: Face::default(),
                chrome_overlay: Face::default(), chrome_accent: Face::default(),
            },
        };
        // should not panic; all rungs floor to canvas
        t.derive_chrome(ChromeDisposition::Full);
        // derived chrome bg = canvas (floored) or very close
        let chrome_bg = t.face(SemanticElement::Chrome).bg.unwrap_or(white_bg);
        assert_eq!(chrome_bg, white_bg, "low-contrast chrome floors to canvas");
        let overlay_bg = t.face(SemanticElement::ChromeOverlay).bg.unwrap_or(white_bg);
        assert_eq!(overlay_bg, white_bg, "low-contrast overlay floors to canvas");
        let muted_bg = t.face(SemanticElement::ChromeMuted).bg.unwrap_or(white_bg);
        assert_eq!(muted_bg, white_bg, "low-contrast muted floors to canvas");
    }

    #[test]
    fn derive_skips_non_rgb_bases() {
        // A theme with Color::Default bases must be byte-untouched by derive_chrome.
        let t_before = default();
        let mut t = default();
        t.derive_chrome(ChromeDisposition::Full);
        assert_eq!(t.faces, t_before.faces, "non-Rgb bases: derive is a no-op");
    }

    #[test]
    fn contrast_ratio_matches_wcag() {
        // white on black = 21.0 exactly
        let white = Color::Rgb { r:255, g:255, b:255 };
        let black = Color::Rgb { r:0,   g:0,   b:0   };
        let cr = contrast_ratio(white, black);
        assert!((cr - 21.0).abs() < 0.01, "white/black contrast should be 21.0, got {cr}");

        // solarized-light: base05 #586e75 on base00 #fdf6e3 — §B.3 pins 4.99
        let fg = Color::Rgb { r:0x58, g:0x6e, b:0x75 };
        let bg = Color::Rgb { r:0xfd, g:0xf6, b:0xe3 };
        let cr2 = contrast_ratio(fg, bg);
        assert!((cr2 - 4.99).abs() < 0.02, "solarized-light fg/canvas CR should be ≈4.99, got {cr2}");
    }
}
