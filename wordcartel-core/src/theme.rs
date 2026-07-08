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
            "forever-blue-jeans-dark"  => Some(blue_jeans_dark()),
            "forever-blue-jeans-dusk"  => Some(blue_jeans_dusk()),
            "forever-blue-jeans-paper" => Some(blue_jeans_paper()),
            _ => {
                // "phosphor-<hue>" — flat suffix removed (D4); resolve layer maps stale aliases (T3).
                let rest = name.strip_prefix("phosphor-")?;
                let hue = PHOSPHORS.iter().find(|(n, _)| *n == rest)?.1;
                Some(phosphor(name, hue))
            }
        }
    }
    pub fn builtin_names() -> &'static [&'static str] {
        // Registration order (NOT display order — the theme picker sorts rows alphabetically):
        // D5 order → 10 E4 themes → the Blue Jeans family. Kept append-only so index-based
        // callers/tests stay stable; user-facing ordering lives in theme_picker::rebuild_rows.
        &[
            "terminal-plain", "terminal-ansi", "no-color", "tokyo-night",
            "phosphor-green", "phosphor-amber", "phosphor-red", "phosphor-blue", "phosphor-purple",
            "catppuccin-mocha", "catppuccin-latte",
            "flexoki-dark",  "flexoki-light",
            "gruvbox-dark",  "gruvbox-light",
            "rosepine-moon", "rosepine-dawn",
            "solarized-dark", "solarized-light",
            "forever-blue-jeans-dark", "forever-blue-jeans-dusk", "forever-blue-jeans-paper",
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
    /// The bg stack is a unified HSL-lightness **elevation** ladder of **three tones**: each panel
    /// keeps the canvas hue (and saturation, capped only on light canvases) and grows its lightness
    /// from the layer beneath toward the headroom pole (white on dark themes, black on light) until
    /// it clears the adjacent-layer WCAG contrast target — `FULL_STEP_CR` at full, `SEP_FLOOR_CR` at
    /// zen. The persistent bar (Chrome) and the dropdown (ChromeMuted) are two elevated tones;
    /// the modal overlay SHARES the dropdown tone (`ChromeOverlay.bg == ChromeMuted.bg`) rather than
    /// growing a third layer, so the stack is `canvas < bar < dropdown == overlay` by construction.
    /// Each chrome fg is re-derived via a legibility floor (`FG_FLOOR`, pole-capped): a seed color is
    /// kept when it already clears the floor, else nudged toward the headroom pole. The bar (Chrome) fg
    /// additionally recedes below body text — its seed is `blend(base_fg, canvas, CHROME_BAR_FG_BLEND)`
    /// plus a DIM modifier (E5) — so the bars read as chrome, not content. The overlay fg is derived
    /// straight from `base_fg` (readable primary text), distinct from the dropdown's dim
    /// `MUTED_FG_BLEND` fg, so the two shared-bg panels still differ in foreground.
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
        // Elevate toward the pole with headroom: white on dark canvases, black on light.
        let pole = if is_dark { (255u8, 255u8, 255u8) } else { (0u8, 0u8, 0u8) };
        let headroom = Color::Rgb { r: pole.0, g: pole.1, b: pole.2 };
        // Panels preserve the canvas hue; saturation is capped on LIGHT canvases ONLY — a
        // uniform cap would wash out phosphor/solarized-dark tint (grounding §II.7).
        let (canvas_h, canvas_s, _canvas_l) = rgb_to_hsl(bgr, bgg, bgb);
        let panel_s = if is_dark { canvas_s } else { canvas_s.min(CHROME_PANEL_S_CAP) };
        // full vs zen = the same algorithm, different adjacent-layer CR target.
        let target = match disp {
            ChromeDisposition::Full => FULL_STEP_CR,
            ChromeDisposition::Zen  => SEP_FLOOR_CR,
        };

        // next_layer — grow a panel from the LIGHTNESS of the layer beneath toward the
        // headroom pole (preserving canvas H and the possibly-capped panel S) until it
        // clears `target` WCAG contrast against that layer. Any step finer than one u8 of
        // lightness lands on the first u8-quantized panel clearing the target (§II.5 pins).
        let next_layer = |beneath: Color, target: f32| -> Color {
            let start_l = match beneath {
                Color::Rgb { r, g, b } => rgb_to_hsl(r, g, b).2,
                _ => return beneath,
            };
            let mut extra = 0.0f32;
            loop {
                let l = if is_dark { (start_l + extra).min(1.0) } else { (start_l - extra).max(0.0) };
                let (r, g, b) = hsl_to_rgb(canvas_h, panel_s, l);
                let cand = Color::Rgb { r, g, b };
                if contrast_ratio(cand, beneath) >= target - CR_TOL { return cand; }
                if (is_dark && l >= 1.0) || (!is_dark && l <= 0.0) { return cand; }
                extra += LAYER_L_STEP;
            }
        };

        // derive_fg — legibility floor (A-D3). Returns `seed` unchanged when it already
        // clears the floor (the common case — chrome text keeps body-text identity); else
        // nudges toward the headroom pole. The floor is capped by the pole-vs-panel max CR
        // so a mid-luminance panel always terminates (at the pole in the worst case).
        let derive_fg = |seed: Color, panel: Color| -> Color {
            let floor = FG_FLOOR.min(contrast_ratio(headroom, panel));
            if contrast_ratio(seed, panel) >= floor - CR_TOL { return seed; }
            let mut pct = 0.0f32;
            loop {
                pct += FG_NUDGE_STEP;
                let cand = blend(seed, pole, pct);
                if contrast_ratio(cand, panel) >= floor - CR_TOL || pct >= 1.0 { return cand; }
            }
        };

        // ── Chrome (bar — elevated from the canvas) ──────────────────────────────────
        if self.faces.chrome == Face::default() {
            let bg = next_layer(base_bg, target);
            // E5: recede the bar/status fg toward its panel (still floor-guarded by derive_fg) + DIM,
            // so the bars read as chrome, not body text. Only Chrome changes; bg and every other face
            // are untouched, so the elevation ladder and all other pins are stable.
            let recede = blend(base_fg, (bgr, bgg, bgb), CHROME_BAR_FG_BLEND);
            self.faces.chrome = Face { fg: Some(derive_fg(recede, bg)), bg: Some(bg), dim: Some(true), ..Face::default() };
        }
        let bar_bg = self.faces.chrome.bg.unwrap_or(base_bg);

        // ── ChromeMuted (dropdown — elevated from the bar) ───────────────────────────
        if self.faces.chrome_muted == Face::default() {
            let bg = next_layer(bar_bg, target);
            let muted_seed = blend(base_fg, (bgr, bgg, bgb), MUTED_FG_BLEND);
            self.faces.chrome_muted = Face {
                fg: Some(derive_fg(muted_seed, bg)), bg: Some(bg), dim: Some(true), ..Face::default()
            };
        }
        let drop_bg = self.faces.chrome_muted.bg.unwrap_or(bar_bg);

        // ── ChromeOverlay (modal — shares the dropdown's level-2 bg; 3-tone ladder) ──
        // User decision 2026-07-06: the modal and the dropdown are co-transient "work"
        // surfaces (essentially never co-visible), so the overlay ADOPTS the dropdown bg
        // rather than growing a 3rd elevation layer — `ChromeOverlay.bg == ChromeMuted.bg`
        // (a testable invariant). Its fg stays its OWN readable base_fg-derived contrast
        // (NOT the dropdown's dim MUTED_FG_BLEND fg — modal content is primary text). The
        // overlay is set apart from the document by its border + the `Clear` beneath it.
        if self.faces.chrome_overlay == Face::default() {
            let bg = drop_bg;
            self.faces.chrome_overlay = Face { fg: Some(derive_fg(base_fg, bg)), bg: Some(bg), ..Face::default() };
        }

        // ── ChromeSelected (inverted highlight — unchanged) ──────────────────────────
        if self.faces.chrome_selected == Face::default() {
            self.faces.chrome_selected = Face { fg: Some(base_bg), bg: Some(base_fg), ..Face::default() };
        }

        // ── ChromeAccent (accent fg on the elevated bar bg — fg path unchanged from E3) ─
        if self.faces.chrome_accent == Face::default() {
            let accent_bg = self.faces.chrome.bg.unwrap_or(base_bg);
            let seed = self.faces.link.fg.unwrap_or(base_fg);
            let gray = equal_lum_gray(seed);
            let mut accent_fg = blend(seed, gray, ACCENT_DESAT);
            if disp == ChromeDisposition::Zen {
                accent_fg = blend(accent_fg, gray, ZEN_ACCENT_EXTRA);
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

// ── Chrome derivation — elevation constants (grounding §II.2, probe-calibrated) ──────
// full and zen are the SAME elevation algorithm with different adjacent-layer CR targets —
// guaranteeing full ≠ zen on every theme.
const SEP_FLOOR_CR:  f32 = 1.12;  // zen  — each layer clears CR ≥ 1.12 vs the layer beneath
const FULL_STEP_CR:  f32 = 1.30;  // full — each layer clears CR ≥ 1.30 vs the layer beneath
const FG_FLOOR:      f32 = 4.5;   // each chrome fg clears 4.5 vs its own panel (pole-capped)
const CHROME_PANEL_S_CAP: f32 = 0.35; // elevated-panel S = min(canvas_S, 0.35); LIGHT canvases only
const LAYER_L_STEP:  f32 = 0.002; // panel-lightness search granularity (matches the §II calibration probe)
const FG_NUDGE_STEP: f32 = 0.01;  // fg legibility-nudge granularity (matches the §II calibration probe)
// Acceptance slack for the adjacent-layer / fg-floor contrast searches. The CR targets are
// evaluated in f32 against candidates that will be quantized to u8 channels; a bare `>= target`
// would reject a candidate whose true CR sits a hair below the target only through f32 rounding at
// the quantization boundary, over-shooting to the next (visibly identical) rung. 0.0005 absorbs
// that jitter — calibrated against the §II.5 probe so every pin reproduces byte-exact.
const CR_TOL:        f32 = 0.0005;
const MUTED_FG_BLEND: f32 = 0.35;   // muted fg seed = blend(base_fg, base_bg, 0.35), then nudged
const CHROME_BAR_FG_BLEND: f32 = 0.18;  // bar/status fg recedes toward its panel bg — gentler than the
                                        // dropdown's 0.35 so the ladder stays Text > bar > dropdown.
const ACCENT_DESAT:   f32 = 0.50;   // accent fg = blend(seed, equal_lum_gray(seed), 0.50)
const ZEN_ACCENT_EXTRA: f32 = 0.40; // zen: extra blend of the accent fg toward the same gray

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
            chrome: Face { fg: Some(Color::White), bg: Some(Color::Black), dim: Some(true), ..Face::default() },
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
    const SEL_BG:    Color = rgb(0x29, 0x2e, 0x42); // #292e42 (Folke bg_highlight)

    Theme {
        name: "tokyo-night".into(),
        base_fg: FG,
        base_bg: BG,
        heading_level_glyph: true,
        monochrome: false,
        faces: ThemeFaces {
            text: Face::default(),
            emphasis: Face { fg: Some(MAGENTA), italic: Some(true), ..Face::default() },
            strong: Face { fg: Some(YELLOW), bold: Some(true), ..Face::default() },
            strong_emphasis: Face { fg: Some(ORANGE), bold: Some(true), italic: Some(true), ..Face::default() },
            code: Face { fg: Some(GREEN), ..Face::default() },
            strikethrough: Face { fg: Some(COMMENT), strike: Some(true), ..Face::default() },
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
            front_matter: Face { fg: Some(ORANGE), italic: Some(true), ..Face::default() },
            comment: Face { fg: Some(COMMENT), italic: Some(true), dim: Some(true), ..Face::default() },
            selection: Face { bg: Some(SEL_BG), ..Face::default() },
            // §13.2 marked block: lighter-than-selection bg + reverse+bold+underline.
            marked_block: Face { bg: Some(DARK3), reverse: Some(true), bold: Some(true), underline: Some(true), ..Face::default() },
            search_match: Face { bg: Some(YELLOW), fg: Some(BG), ..Face::default() },
            search_current: Face { reverse: Some(true), ..Face::default() },
            diag_spelling: Face { underline: Some(true), underline_color: Some(RED), ..Face::default() },
            diag_grammar:  Face { underline: Some(true), underline_color: Some(BLUE), ..Face::default() },
            focus_dim: Face { fg: Some(COMMENT), dim: Some(true), ..Face::default() },
            fold_marker: Face { fg: Some(DARK3), ..Face::default() },
            wrap_guide: Face { fg: Some(SEL_BG), ..Face::default() },
            // All five chrome derived faces are sentinels — derive_chrome fills all from the
            // elevation ladder (unified model, T1). chrome_reverse is kept (never derived).
            chrome: Face::default(),
            chrome_reverse: Face { reverse: Some(true), ..Face::default() },
            chrome_selected: Face::default(),
            chrome_muted: Face::default(),
            chrome_overlay: Face::default(),
            chrome_accent: Face::default(),
        },
    }
}

/// ANSI-named theme — the user's terminal palette with the FULL markdown colorization and a
/// polished chrome ladder. `base_fg/bg = Color::Default` (defers to the terminal bg); NOT
/// monochrome; every fg-required role carries a named ANSI hue (see the completeness test), and
/// chrome faces are fully explicit (unlike terminal-plain, whose overlay stays exempt). This is
/// the "keep my terminal colors, lose nothing else" theme.
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
            // Inline emphasis carries a named hue AND its modifier — the same full colorization the
            // RGB themes give (tokyo: MAGENTA/YELLOW/ORANGE/COMMENT), mapped to the ANSI-16 palette.
            emphasis: m(Some(Color::Magenta), false, true, false, false, false),
            strong: m(Some(Color::Yellow), true, false, false, false, false),
            strong_emphasis: m(Some(Color::LightRed), true, true, false, false, false),
            code: m(Some(Color::Green), false, false, false, false, false),
            strikethrough: m(Some(Color::DarkGray), false, false, false, true, false),
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
            // Explicit named-ANSI chrome ladder, mirroring the Ansi16 dark-arm policy: DarkGray
            // elevated over the unknown terminal canvas (visible on a black OR white terminal),
            // bar/dropdown/modal sharing that tone with the dropdown set apart by `dim`. This makes
            // the modal share the dropdown tone (Overlay.bg == Muted.bg) and renders the menus as
            // nicely as the RGB themes do at 16-color depth. (terminal-plain keeps its frameless
            // Black bar — its scrollbar reuses Chrome vs ChromeMuted for thumb-vs-track contrast.)
            chrome:          Face { fg: Some(Color::White),    bg: Some(Color::DarkGray), dim: Some(true), ..Face::default() },
            chrome_reverse:  Face { reverse: Some(true), ..Face::default() },
            chrome_overlay:  Face { fg: Some(Color::White),    bg: Some(Color::DarkGray), ..Face::default() },
            chrome_selected: Face { fg: Some(Color::Black),    bg: Some(Color::White),   ..Face::default() },
            chrome_muted:    Face { fg: Some(Color::White),    bg: Some(Color::DarkGray), dim: Some(true), ..Face::default() },
            chrome_accent:   Face { fg: Some(Color::LightCyan), bg: Some(Color::DarkGray), bold: Some(true), ..Face::default() },
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

// ── Blue Jeans family — three hand-authored variants (faded denim / brass / leather / cotton) ──
// Source: James Keim's "Blue Jeans" theme package (palette + markdown role map). Unlike the ten
// base16 bundles, these honor the designer's EXACT markdown role→color map (brass H1, leather H2,
// denim links, brass list markers, boxed code) rather than the base16 hue rainbow. Chrome is
// all-sentinel — `derive_chrome` builds the panel/menu ladder from `base_bg`/`base_fg` as usual.

/// Final per-role colors for one Blue Jeans variant. Fields are the resolved foreground (or
/// highlight-bg) each semantic role paints — already ADAPTED per variant, so the builder below is
/// purely mechanical. The paper (light) variant substitutes readable dark accents for the roles the
/// dark CSS assigned to a light neutral (canvas): `emphasis`, `heading[3]` (H4), and `block_quote`.
struct BjRoles {
    bg: Color, fg: Color,
    heading: [Color; 6],
    emphasis: Color, strong: Color, strong_emphasis: Color,
    code_fg: Color, code_bg: Color, code_block_fg: Color, code_block_bg: Color,
    link: Color, strike: Color, block_quote: Color,
    list_marker: Color, thematic_break: Color, front_matter: Color, comment: Color,
    selection_bg: Color, mark_bg: Color, search_bg: Color,
    diag_spell: Color, diag_grammar: Color, focus_dim: Color, fold_marker: Color, wrap_guide: Color,
}

/// Build a Blue Jeans variant from its resolved role colors. Every fg-required role carries an
/// explicit fg (Part B completeness); Text stays empty; chrome faces are sentinels for derivation.
fn blue_jeans(name: &str, r: BjRoles) -> Theme {
    let h = |c: Color| Face { fg: Some(c), bold: Some(true), ..Face::default() };
    Theme {
        name: name.into(),
        base_fg: r.fg, base_bg: r.bg,
        heading_level_glyph: true, monochrome: false,
        faces: ThemeFaces {
            text: Face::default(),
            emphasis: Face { fg: Some(r.emphasis), italic: Some(true), ..Face::default() },
            strong: Face { fg: Some(r.strong), bold: Some(true), ..Face::default() },
            strong_emphasis: Face { fg: Some(r.strong_emphasis), bold: Some(true), italic: Some(true), ..Face::default() },
            // Inline code keeps the designed "boxed" look: cotton/denim fg on a raised-surface bg.
            code: Face { fg: Some(r.code_fg), bg: Some(r.code_bg), ..Face::default() },
            strikethrough: Face { fg: Some(r.strike), strike: Some(true), ..Face::default() },
            link: Face { fg: Some(r.link), underline: Some(true), ..Face::default() },
            heading: [h(r.heading[0]), h(r.heading[1]), h(r.heading[2]), h(r.heading[3]), h(r.heading[4]), h(r.heading[5])],
            block_quote: Face { fg: Some(r.block_quote), italic: Some(true), ..Face::default() },
            code_block: Face { fg: Some(r.code_block_fg), bg: Some(r.code_block_bg), ..Face::default() },
            list_marker: Face { fg: Some(r.list_marker), ..Face::default() },
            thematic_break: Face { fg: Some(r.thematic_break), ..Face::default() },
            front_matter: Face { fg: Some(r.front_matter), italic: Some(true), ..Face::default() },
            comment: Face { fg: Some(r.comment), italic: Some(true), dim: Some(true), ..Face::default() },
            selection: Face { bg: Some(r.selection_bg), ..Face::default() },
            // §13.2 marked block: warm mark-bg + reverse+bold+underline (distinct from selection).
            marked_block: Face { bg: Some(r.mark_bg), reverse: Some(true), bold: Some(true), underline: Some(true), ..Face::default() },
            search_match: Face { bg: Some(r.search_bg), fg: Some(r.bg), ..Face::default() },
            search_current: Face { reverse: Some(true), ..Face::default() },
            diag_spelling: Face { underline: Some(true), underline_color: Some(r.diag_spell), ..Face::default() },
            diag_grammar:  Face { underline: Some(true), underline_color: Some(r.diag_grammar), ..Face::default() },
            focus_dim: Face { fg: Some(r.focus_dim), dim: Some(true), ..Face::default() },
            fold_marker: Face { fg: Some(r.fold_marker), ..Face::default() },
            wrap_guide: Face { fg: Some(r.wrap_guide), ..Face::default() },
            // chrome/selected/muted/overlay/accent: all-None sentinels — derive_chrome fills them.
            chrome: Face::default(),
            chrome_reverse: Face { reverse: Some(true), ..Face::default() },
            chrome_selected: Face::default(),
            chrome_muted: Face::default(),
            chrome_overlay: Face::default(),
            chrome_accent: Face::default(),
        },
    }
}

/// Blue Jeans Dark — deep worn indigo, warm cotton text, brass/leather accents.
pub fn blue_jeans_dark() -> Theme {
    blue_jeans("forever-blue-jeans-dark", BjRoles {
        bg: rgb(0x20,0x28,0x33), fg: rgb(0xF2,0xE9,0xDA),
        heading: [rgb(0xC7,0x9A,0x44), rgb(0xA1,0x75,0x4E), rgb(0x8C,0xA4,0xBF), rgb(0xD5,0xCA,0xB7), rgb(0xD5,0xB0,0x5C), rgb(0xB3,0xAA,0xA0)],
        emphasis: rgb(0xD5,0xCA,0xB7), strong: rgb(0xFA,0xF7,0xF2), strong_emphasis: rgb(0xFA,0xF7,0xF2),
        code_fg: rgb(0xFA,0xF7,0xF2), code_bg: rgb(0x2B,0x35,0x42), code_block_fg: rgb(0xF2,0xE9,0xDA), code_block_bg: rgb(0x24,0x2D,0x38),
        link: rgb(0x8C,0xA4,0xBF), strike: rgb(0x87,0x91,0x9D), block_quote: rgb(0xD5,0xCA,0xB7),
        list_marker: rgb(0xC7,0x9A,0x44), thematic_break: rgb(0x3A,0x46,0x54), front_matter: rgb(0xA0,0x8A,0xAD), comment: rgb(0x87,0x91,0x9D),
        selection_bg: rgb(0x3A,0x4D,0x62), mark_bg: rgb(0xD5,0xB0,0x5C), search_bg: rgb(0xD5,0xB0,0x5C),
        diag_spell: rgb(0xE0,0x6C,0x63), diag_grammar: rgb(0x7A,0xA9,0xBA), focus_dim: rgb(0x87,0x91,0x9D), fold_marker: rgb(0x87,0x91,0x9D), wrap_guide: rgb(0x3A,0x46,0x54),
    })
}

/// Blue Jeans Dusk — softer evening denim, lower-glare surfaces for long sessions.
pub fn blue_jeans_dusk() -> Theme {
    blue_jeans("forever-blue-jeans-dusk", BjRoles {
        bg: rgb(0x29,0x32,0x40), fg: rgb(0xEF,0xE4,0xD3),
        heading: [rgb(0xD0,0xA2,0x53), rgb(0xB1,0x80,0x58), rgb(0x9C,0xB3,0xCB), rgb(0xDC,0xCF,0xB9), rgb(0xDD,0xBB,0x6B), rgb(0xBE,0xB2,0xA2)],
        emphasis: rgb(0xDC,0xCF,0xB9), strong: rgb(0xFA,0xF4,0xEA), strong_emphasis: rgb(0xFA,0xF4,0xEA),
        code_fg: rgb(0xFA,0xF4,0xEA), code_bg: rgb(0x37,0x43,0x53), code_block_fg: rgb(0xEF,0xE4,0xD3), code_block_bg: rgb(0x30,0x3A,0x49),
        link: rgb(0x9C,0xB3,0xCB), strike: rgb(0x96,0xA0,0xAA), block_quote: rgb(0xDC,0xCF,0xB9),
        list_marker: rgb(0xD0,0xA2,0x53), thematic_break: rgb(0x47,0x54,0x66), front_matter: rgb(0xAB,0x95,0xB8), comment: rgb(0x96,0xA0,0xAA),
        selection_bg: rgb(0x48,0x5D,0x72), mark_bg: rgb(0xDD,0xBB,0x6B), search_bg: rgb(0xDD,0xBB,0x6B),
        diag_spell: rgb(0xE8,0x83,0x79), diag_grammar: rgb(0x88,0xB3,0xC3), focus_dim: rgb(0x96,0xA0,0xAA), fold_marker: rgb(0x96,0xA0,0xAA), wrap_guide: rgb(0x47,0x54,0x66),
    })
}

/// Blue Jeans Paper — cream paper, denim text, warm ledger borders, brass headings.
/// Light variant: `emphasis`/H4/`block_quote` use readable dark accents (leather/river/denim)
/// where the dark CSS used the light `canvas` neutral — which would be near-invisible on cream.
pub fn blue_jeans_paper() -> Theme {
    blue_jeans("forever-blue-jeans-paper", BjRoles {
        bg: rgb(0xF2,0xE9,0xDA), fg: rgb(0x26,0x32,0x40),
        heading: [rgb(0x7E,0x57,0x0F), rgb(0x67,0x47,0x2D), rgb(0x36,0x5F,0x85), rgb(0x39,0x70,0x83), rgb(0x8F,0x66,0x1E), rgb(0x5F,0x5A,0x54)],
        emphasis: rgb(0x67,0x47,0x2D), strong: rgb(0x17,0x21,0x2D), strong_emphasis: rgb(0x17,0x21,0x2D),
        code_fg: rgb(0x22,0x3B,0x56), code_bg: rgb(0xDD,0xD0,0xBB), code_block_fg: rgb(0x26,0x32,0x40), code_block_bg: rgb(0xE8,0xDD,0xCB),
        link: rgb(0x36,0x5F,0x85), strike: rgb(0x65,0x5F,0x57), block_quote: rgb(0x2F,0x57,0x7D),
        list_marker: rgb(0x7E,0x57,0x0F), thematic_break: rgb(0xC9,0xBD,0xAA), front_matter: rgb(0x66,0x50,0x7B), comment: rgb(0x65,0x5F,0x57),
        selection_bg: rgb(0xD1,0xDB,0xE5), mark_bg: rgb(0x8F,0x66,0x1E), search_bg: rgb(0x8F,0x66,0x1E),
        diag_spell: rgb(0x8F,0x2F,0x2B), diag_grammar: rgb(0x39,0x70,0x83), focus_dim: rgb(0x65,0x5F,0x57), fold_marker: rgb(0x65,0x5F,0x57), wrap_guide: rgb(0xC9,0xBD,0xAA),
    })
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
    // map level 0..=5 to lightness 0.08..=0.78 (ceiling lowered from 0.92 — Part E, §II.6)
    let l = 0.08 + (level.min(5) as f32 / 5.0) * (0.78 - 0.08);
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
        chrome: Face { dim: Some(true), ..Face::default() },
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
        text: Face::default(),   // Part C: empty Text so heading role fg is not clobbered
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

    // Test-only type aliases (keep the chrome-pin tables readable; satisfy clippy::type_complexity).
    type Rgb8 = (u8, u8, u8);
    // One row of the §II.5 chrome-pin table: constructor, disposition, then bg/fg for
    // Chrome / ChromeMuted / ChromeOverlay / ChromeSelected / ChromeAccent, plus a label.
    type ChromePinRow = (fn() -> Theme, ChromeDisposition,
                         Rgb8, Rgb8, Rgb8, Rgb8, Rgb8, Rgb8, Rgb8, Rgb8, Rgb8, Rgb8, &'static str);
    // One row of the Indexed256 rung table: constructor, label, canvas/chrome/muted/overlay indices.
    type RungRow = (fn() -> Theme, &'static str, u8, u8, u8, u8);

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
    fn phosphor_shade_ceiling_keeps_bright_shades_hued() {
        // §II.6: after the 0.78 ceiling, s(5) has a wide channel spread (hued), not near-white.
        let hues = [
            ("green",  Color::Rgb{r:0x33,g:0xff,b:0x33}, Color::Rgb{r:0x8f,g:0xff,b:0x8f}),
            ("amber",  Color::Rgb{r:0xff,g:0xb0,b:0x00}, Color::Rgb{r:0xff,g:0xdc,b:0x8f}),
            ("red",    Color::Rgb{r:0xff,g:0x55,b:0x55}, Color::Rgb{r:0xff,g:0x8f,b:0x8f}),
            ("blue",   Color::Rgb{r:0x55,g:0x99,b:0xff}, Color::Rgb{r:0x8f,g:0xbc,b:0xff}),
            ("purple", Color::Rgb{r:0xcc,g:0x99,b:0xff}, Color::Rgb{r:0xc7,g:0x8f,b:0xff}),
        ];
        for (name, hue, s5) in hues {
            let actual = shade(hue, 5);
            assert_eq!(actual, s5, "{name} s(5) = §II.6 pin");   // [verify]
            // "hued": the max-min channel spread is wide (≥ 96), i.e. NOT washed to near-white.
            if let Color::Rgb { r, g, b } = actual {
                let spread = r.max(g).max(b) - r.min(g).min(b);
                assert!(spread >= 96, "{name} s(5) must stay hued: spread={spread}");
            } else { panic!("non-Rgb"); }
        }
        // base_bg (s0) unchanged by the ceiling; base_fg (s3) shifts to §II.6.
        let green = Color::Rgb{r:0x33,g:0xff,b:0x33};
        assert_eq!(shade(green, 0), Color::Rgb{r:0x00,g:0x29,b:0x00}, "green s0 unchanged");
        assert_eq!(shade(green, 3), Color::Rgb{r:0x00,g:0xff,b:0x00}, "green s3 (base_fg) shifts");
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
    fn builtin_names_registry_total() {
        // Every name resolves; every face is total; count is exactly 22 (19 + Blue Jeans ×3);
        // no -flat names. Registration head is stable (index-based callers rely on it).
        for name in Theme::builtin_names() {
            let t = Theme::builtin(name).expect(name);
            for el in ALL_ELEMENTS { let _ = t.face(el); }
        }
        assert_eq!(Theme::builtin_names().len(), 22);
        assert_eq!(Theme::builtin_names()[0], "terminal-plain"); // D5 first entry
        assert_eq!(Theme::builtin_names()[1], "terminal-ansi");  // D5 second entry
        assert!(!Theme::builtin_names().iter().any(|n| n.contains("-flat")), "no flat variants");
        // Blue Jeans family present and resolvable.
        for n in ["forever-blue-jeans-dark", "forever-blue-jeans-dusk", "forever-blue-jeans-paper"] {
            assert!(Theme::builtin_names().contains(&n), "missing builtin {n}");
        }
    }

    #[test]
    fn exemplar_spot_pins_blue_jeans() {
        // Blue Jeans honors the designer's markdown role map (NOT the base16 rainbow):
        // dark H1 = brass, H2 = leather, links = denim-soft; list marker = brass. Inline code is
        // "boxed" (fg + raised-surface bg). Paper (light) adapts the canvas-neutral roles.
        let d = blue_jeans_dark();
        assert_eq!(d.base_bg, rgb(0x20, 0x28, 0x33), "dark base_bg = background");
        assert_eq!(d.base_fg, rgb(0xF2, 0xE9, 0xDA), "dark base_fg = foreground");
        assert_eq!(d.face(SemanticElement::Heading(1)).fg, Some(rgb(0xC7, 0x9A, 0x44)), "dark H1 = brass");
        assert_eq!(d.face(SemanticElement::Heading(1)).bold, Some(true), "dark H1 bold");
        assert_eq!(d.face(SemanticElement::Heading(2)).fg, Some(rgb(0xA1, 0x75, 0x4E)), "dark H2 = leather");
        assert_eq!(d.face(SemanticElement::Link).fg, Some(rgb(0x8C, 0xA4, 0xBF)), "dark link = denim-soft");
        assert_eq!(d.face(SemanticElement::ListMarker).fg, Some(rgb(0xC7, 0x9A, 0x44)), "dark list marker = brass");
        let code = d.face(SemanticElement::Code);
        assert_eq!(code.fg, Some(rgb(0xFA, 0xF7, 0xF2)), "dark inline code fg = cotton");
        assert_eq!(code.bg, Some(rgb(0x2B, 0x35, 0x42)), "dark inline code bg = raised surface (boxed)");
        // Paper is a light theme; its canvas-neutral roles are adapted to readable dark accents.
        let p = blue_jeans_paper();
        assert_eq!(p.base_bg, rgb(0xF2, 0xE9, 0xDA), "paper base_bg = cream");
        assert_eq!(p.face(SemanticElement::Emphasis).fg, Some(rgb(0x67, 0x47, 0x2D)), "paper em adapted → leather");
        assert_eq!(p.face(SemanticElement::Heading(4)).fg, Some(rgb(0x39, 0x70, 0x83)), "paper H4 adapted → river");
    }

    // ── Part B — completeness conformance ───────────────────────────────────────────────────────

    /// Per-face requirement type for the Part B completeness contract.
    /// Derived from the `from_base16` template — each semantic role has exactly one
    /// requirement class; any new SemanticElement variant forces an update here (exhaustive match).
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum FaceReq { FgRequired, UnderlineColorRequired, Highlight, Modifier, Empty, Exempt }

    fn face_requirement(el: SemanticElement) -> FaceReq {
        use SemanticElement::*;
        use FaceReq::*;
        match el {
            Text                                                               => Empty,
            Emphasis | Strong | StrongEmphasis | Code | Strikethrough | Link
            | Heading(_) | BlockQuote | CodeBlock | ListMarker | ThematicBreak
            | FrontMatter | Comment | FocusDim | FoldMarker | WrapGuide      => FgRequired,
            DiagSpelling | DiagGrammar                                        => UnderlineColorRequired,
            Selection | MarkedBlock | SearchMatch                             => Highlight,
            SearchCurrent                                                     => Modifier,
            Chrome | ChromeReverse | ChromeSelected | ChromeMuted | ChromeOverlay
            | ChromeAccent                                                     => Exempt,
        }
    }

    #[test]
    fn every_rgb_builtin_satisfies_the_completeness_contract() {
        // Part B — over the 16 RGB builtins (terminal-plain/terminal-ansi/no-color are non-Rgb
        // and exempt), every face satisfies ITS requirement type from the spec Part B contract.
        // Retrospective guardrail: T2/T3/T4 already brought every theme to standard, so this
        // test passes immediately — but it prevents future themes from silently omitting a
        // required role. Confirmed REAL (RED-then-GREEN): with tokyo emphasis.fg temporarily
        // cleared the test failed as "tokyo-night Emphasis: fg-required face has no explicit fg".
        for name in Theme::builtin_names() {
            let t = Theme::builtin(name).unwrap();
            if !matches!(t.base_bg, Color::Rgb { .. }) { continue; }   // skip the 3 non-Rgb themes
            for el in ALL_ELEMENTS {
                let f = t.face(el);
                match face_requirement(el) {
                    FaceReq::FgRequired => assert!(
                        f.fg.is_some() && f.fg != Some(Color::Default),
                        "{name} {el:?}: fg-required face has no explicit fg"),
                    FaceReq::UnderlineColorRequired => assert!(
                        f.underline_color.is_some() && f.underline == Some(true),
                        "{name} {el:?}: diagnostic face needs BOTH underline_color AND the \
                         underline modifier — a colored underline with no underline draws nothing \
                         (a11y cue would be invisible)"),
                    FaceReq::Highlight => assert!(
                        f.bg.is_some() || f.reverse == Some(true),
                        "{name} {el:?}: highlight face needs a bg OR reverse"),
                    FaceReq::Modifier => assert!(
                        [f.bold, f.italic, f.underline, f.strike, f.reverse, f.dim]
                            .contains(&Some(true)),
                        "{name} {el:?}: modifier-required face has no modifier"),
                    FaceReq::Empty => assert_eq!(
                        f, Face::default(), "{name} {el:?}: SE::Text must be empty (Part C)"),
                    FaceReq::Exempt => {} // chrome — supplied by the elevation ladder
                }
            }
        }
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
        // 3-tone ladder: the elevated bar/dropdown rungs do NOT collapse onto the canvas index —
        // each is distinct and ordered toward the headroom pole. The modal overlay SHARES the
        // dropdown bg (user decision 2026-07-06), so at 256 the overlay index EQUALS the dropdown
        // index. Indices regenerated by quantizing the shipped truecolor rungs at Depth::Indexed256.
        // Uses the REAL constructors so the pinned indices match §II.5's themes.
        let expect: &[RungRow] = &[
            (flexoki_dark,     "flexoki-dark", 232, 235, 237, 237),
            (catppuccin_mocha, "mocha",        234, 236, 238, 238),
            (gruvbox_dark,     "gruvbox-dark", 235, 237, 238, 238),
        ];
        for &(ctor, label, ci, chi, mi, oi) in expect {
            let mut t = ctor();
            t.derive_chrome(ChromeDisposition::Full);
            let q = |c: Color| quantize(c, Depth::Indexed256);
            let canvas  = q(t.base_bg);
            let chrome  = q(t.face(SemanticElement::Chrome).bg.unwrap());
            let muted   = q(t.face(SemanticElement::ChromeMuted).bg.unwrap());
            let overlay = q(t.face(SemanticElement::ChromeOverlay).bg.unwrap());
            assert_ne!(chrome,  canvas,  "{label}: chrome distinct from canvas at 256 (elevated)");
            assert_ne!(muted,   chrome,  "{label}: dropdown distinct from bar at 256");
            assert_eq!(overlay, muted,   "{label}: overlay shares the dropdown bg (3-tone ladder)");
            // exact regenerated indices (probe output):
            assert_eq!(canvas,  Color::Indexed(ci),  "{label} canvas index");
            assert_eq!(chrome,  Color::Indexed(chi), "{label} chrome index");
            assert_eq!(muted,   Color::Indexed(mi),  "{label} muted index");
            assert_eq!(overlay, Color::Indexed(oi),  "{label} overlay index");
        }
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
        // Chrome faces must use named ANSI colors (not Rgb) — spot check. The ladder mirrors the
        // Ansi16 dark-arm policy (DarkGray elevated over the unknown terminal canvas) so the menus
        // render as nicely as the RGB themes do at 16-color depth.
        let chrome = t.face(SemanticElement::Chrome);
        assert_eq!(chrome.fg, Some(Color::White),    "chrome fg White");
        assert_eq!(chrome.bg, Some(Color::DarkGray), "chrome bg DarkGray");
        let ov = t.face(SemanticElement::ChromeOverlay);
        assert_eq!(ov.fg, Some(Color::White),    "overlay fg White");
        assert_eq!(ov.bg, Some(Color::DarkGray), "overlay bg DarkGray");
        let sel = t.face(SemanticElement::ChromeSelected);
        assert_eq!(sel.fg, Some(Color::Black), "selected fg Black");
        assert_eq!(sel.bg, Some(Color::White), "selected bg White");
        let acc = t.face(SemanticElement::ChromeAccent);
        assert_eq!(acc.fg, Some(Color::LightCyan), "accent fg LightCyan");
        assert_eq!(acc.bg, Some(Color::DarkGray),  "accent bg DarkGray");
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
    fn terminal_ansi_is_fully_colored_and_chrome_coherent() {
        // terminal-ansi honors the user's ANSI palette (named colors) while providing the SAME
        // completeness as the RGB builtins: every fg-required role carries a real named color, and
        // the chrome ladder honors the modal-shares-dropdown invariant (Overlay.bg == Muted.bg).
        let t = terminal_ansi();
        // (1) The four inline faces that used to be modifier-only now carry a hue AND keep their
        // modifier — the flat-emphasis look that read as half-baked is gone.
        let emph = t.face(SemanticElement::Emphasis);
        assert_eq!(emph.fg, Some(Color::Magenta), "emphasis fg Magenta");
        assert_eq!(emph.italic, Some(true), "emphasis keeps italic");
        let strong = t.face(SemanticElement::Strong);
        assert_eq!(strong.fg, Some(Color::Yellow), "strong fg Yellow");
        assert_eq!(strong.bold, Some(true), "strong keeps bold");
        let se = t.face(SemanticElement::StrongEmphasis);
        assert_eq!(se.fg, Some(Color::LightRed), "strong_emphasis fg LightRed (warm orange proxy)");
        assert_eq!(se.bold, Some(true), "strong_emphasis keeps bold");
        assert_eq!(se.italic, Some(true), "strong_emphasis keeps italic");
        let strike = t.face(SemanticElement::Strikethrough);
        assert_eq!(strike.fg, Some(Color::DarkGray), "strikethrough fg DarkGray (matches comment tone)");
        assert_eq!(strike.strike, Some(true), "strikethrough keeps strike");
        // (2) Full completeness — every fg-required role has a real (non-Default) color, exactly the
        // contract the 16 RGB builtins satisfy. Reuses the Part B `face_requirement` classifier.
        for el in ALL_ELEMENTS {
            if face_requirement(el) == FaceReq::FgRequired {
                let f = t.face(el);
                assert!(f.fg.is_some() && f.fg != Some(Color::Default),
                    "terminal-ansi {el:?}: fg-required face has no explicit color");
            }
        }
        // (3) Chrome coherence: the ladder mirrors the Ansi16 dark-arm policy (DarkGray elevated
        // over the unknown terminal canvas), and the modal shares the dropdown tone.
        let bar   = t.face(SemanticElement::Chrome);
        let drop  = t.face(SemanticElement::ChromeMuted);
        let modal = t.face(SemanticElement::ChromeOverlay);
        assert_eq!(bar.bg, Some(Color::DarkGray),
            "bar bg DarkGray — elevated, visible on a black OR white terminal");
        assert_eq!(modal.bg, drop.bg,
            "Overlay.bg == Muted.bg (modal shares the dropdown tone)");
        assert_eq!(drop.bg, Some(Color::DarkGray), "dropdown bg DarkGray");
        assert_eq!(drop.dim, Some(true),
            "dropdown distinguished from the bar by its dim fg");
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

    // True when the three ladder-panel faces (Chrome/Muted/Overlay) all start as sentinels,
    // i.e. the theme's whole chrome bg stack is derived. Every RGB builtin (Tokyo included, since
    // Part D made its chrome all-sentinel) satisfies this — the ladder invariants cover them all.
    fn chrome_ladder_is_sentinel(t: &Theme) -> bool {
        t.face(SemanticElement::Chrome) == Face::default()
            && t.face(SemanticElement::ChromeMuted) == Face::default()
            && t.face(SemanticElement::ChromeOverlay) == Face::default()
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

    // phosphor-green bases: canvas=shade(hue,0), fg=shade(hue,3), link=shade(hue,5)
    fn phosphor_green_theme() -> Theme {
        let hue = Color::Rgb { r:0x33, g:0xff, b:0x33 };
        phosphor("phosphor-green", hue)
    }

    #[test]
    fn derive_chrome_base16_pins() {
        // §II.5 — the base16 sentinel-derived chrome, FULL + ZEN, byte-exact.
        // Bases are stable across the branch, so these pins are final.
        // Modal=dropdown tone (user decision 2026-07-06): ChromeOverlay bg == ChromeMuted bg on
        // every row; ChromeOverlay fg is base_fg-derived (readable), so it differs from the
        // dropdown's dim muted fg.
        // Columns: Chrome bg,fg · ChromeMuted bg,fg · ChromeOverlay bg,fg · ChromeSelected bg,fg
        //          · ChromeAccent bg,fg · label.
        let cases: &[ChromePinRow] = &[
            (flexoki_dark, ChromeDisposition::Full,
             (0x2a,0x28,0x28),(0xac,0xab,0xa3), (0x3e,0x3a,0x3a),(0xa5,0xa5,0x9f),
             (0x3e,0x3a,0x3a),(0xce,0xcd,0xc3), (0xce,0xcd,0xc3),(0x10,0x0f,0x0f),
             (0x2a,0x28,0x28),(0x62,0x83,0xa0), "flexoki-dark FULL"),
            (flexoki_dark, ChromeDisposition::Zen,
             (0x1e,0x1c,0x1c),(0xac,0xab,0xa3), (0x28,0x26,0x26),(0x8e,0x8d,0x86),
             (0x28,0x26,0x26),(0xce,0xcd,0xc3), (0xce,0xcd,0xc3),(0x10,0x0f,0x0f),
             (0x1e,0x1c,0x1c),(0x6e,0x82,0x94), "flexoki-dark ZEN"),
            (flexoki_light, ChromeDisposition::Full,
             (0xe5,0xdf,0xc8),(0x3b,0x3a,0x38), (0xcf,0xc4,0x9b),(0x53,0x51,0x4e),
             (0xcf,0xc4,0x9b),(0x10,0x0f,0x0f), (0x10,0x0f,0x0f),(0xff,0xfc,0xf0),
             (0xe5,0xdf,0xc8),(0x3f,0x5e,0x82), "flexoki-light FULL"),
            (flexoki_light, ChromeDisposition::Zen,
             (0xf2,0xef,0xe4),(0x3b,0x3a,0x38), (0xe7,0xe2,0xce),(0x64,0x62,0x5e),
             (0xe7,0xe2,0xce),(0x10,0x0f,0x0f), (0x10,0x0f,0x0f),(0xff,0xfc,0xf0),
             (0xf2,0xef,0xe4),(0x4b,0x5e,0x74), "flexoki-light ZEN"),
            (catppuccin_mocha, ChromeDisposition::Full,
             (0x31,0x31,0x4a),(0xae,0xb5,0xd0), (0x42,0x42,0x65),(0xae,0xb2,0xc5),
             (0x42,0x42,0x65),(0xcd,0xd6,0xf4), (0xcd,0xd6,0xf4),(0x1e,0x1e,0x2e),
             (0x31,0x31,0x4a),(0x9e,0xb4,0xd7), "mocha FULL"),
            (catppuccin_mocha, ChromeDisposition::Zen,
             (0x27,0x27,0x3c),(0xae,0xb5,0xd0), (0x2f,0x2f,0x48),(0x92,0x98,0xb1),
             (0x2f,0x2f,0x48),(0xcd,0xd6,0xf4), (0xcd,0xd6,0xf4),(0x1e,0x1e,0x2e),
             (0x27,0x27,0x3c),(0xa6,0xb4,0xc9), "mocha ZEN"),
            (gruvbox_dark, ChromeDisposition::Full,
             (0x3b,0x3b,0x3b),(0xb6,0xa8,0x8b), (0x4c,0x4c,0x4c),(0xc2,0xbc,0xaf),
             (0x4c,0x4c,0x4c),(0xd5,0xc4,0xa1), (0xd5,0xc4,0xa1),(0x28,0x28,0x28),
             (0x3b,0x3b,0x3b),(0x91,0xa2,0x9b), "gruvbox-dark FULL"),
            (gruvbox_dark, ChromeDisposition::Zen,
             (0x31,0x31,0x31),(0xb6,0xa8,0x8b), (0x39,0x39,0x39),(0xab,0xa2,0x8f),
             (0x39,0x39,0x39),(0xd5,0xc4,0xa1), (0xd5,0xc4,0xa1),(0x28,0x28,0x28),
             (0x31,0x31,0x31),(0x96,0xa0,0x9c), "gruvbox-dark ZEN"),
            (solarized_dark, ChromeDisposition::Full,
             (0x00,0x3f,0x50),(0x96,0xa5,0xa7), (0x00,0x52,0x66),(0xb1,0xbd,0xbf),
             (0x00,0x52,0x66),(0xb2,0xbc,0xbc), (0x93,0xa1,0xa1),(0x00,0x2b,0x36),
             (0x00,0x3f,0x50),(0x56,0x89,0xac), "solarized-dark FULL"),
            (solarized_dark, ChromeDisposition::Zen,
             (0x00,0x34,0x41),(0x88,0x99,0x9a), (0x00,0x3d,0x4c),(0x93,0xa3,0xa6),
             (0x00,0x3d,0x4c),(0x95,0xa3,0xa3), (0x93,0xa1,0xa1),(0x00,0x2b,0x36),
             (0x00,0x34,0x41),(0x69,0x88,0x9d), "solarized-dark ZEN"),
            (solarized_light, ChromeDisposition::Full,
             (0xe2,0xd9,0xc2),(0x55,0x60,0x63), (0xcd,0xbe,0x97),(0x49,0x4f,0x4e),
             (0xcd,0xbe,0x97),(0x40,0x50,0x55), (0x58,0x6e,0x75),(0xfd,0xf6,0xe3),
             (0xe2,0xd9,0xc2),(0x56,0x89,0xac), "solarized-light FULL"),
            (solarized_light, ChromeDisposition::Zen,
             (0xee,0xe9,0xdc),(0x5e,0x6b,0x6e), (0xe4,0xdc,0xc7),(0x5b,0x62,0x61),
             (0xe4,0xdc,0xc7),(0x50,0x64,0x6a), (0x58,0x6e,0x75),(0xfd,0xf6,0xe3),
             (0xee,0xe9,0xdc),(0x69,0x88,0x9d), "solarized-light ZEN"),
            // gruvbox-light + rosepine-dawn (the other two S-capped light themes; §II.5).
            (gruvbox_light, ChromeDisposition::Full,
             (0xdc,0xd5,0xb6),(0x62,0x5b,0x51), (0xc7,0xbb,0x8b),(0x4e,0x4a,0x40),
             (0xc7,0xbb,0x8b),(0x50,0x49,0x45), (0x50,0x49,0x45),(0xfb,0xf1,0xc7),
             (0xdc,0xd5,0xb6),(0x32,0x62,0x6b), "gruvbox-light FULL"),
            (gruvbox_light, ChromeDisposition::Zen,
             (0xe9,0xe4,0xd1),(0x6d,0x65,0x5a), (0xdf,0xd8,0xbc),(0x63,0x5e,0x52),
             (0xdf,0xd8,0xbc),(0x50,0x49,0x45), (0x50,0x49,0x45),(0xfb,0xf1,0xc7),
             (0xe9,0xe4,0xd1),(0x43,0x60,0x65), "gruvbox-light ZEN"),
            (rosepine_dawn, ChromeDisposition::Full,
             (0xe4,0xd6,0xc6),(0x5f,0x5b,0x74), (0xd1,0xbb,0xa0),(0x4f,0x4c,0x59),
             (0xd1,0xbb,0xa0),(0x4d,0x49,0x6c), (0x57,0x52,0x79),(0xfa,0xf4,0xed),
             (0xe4,0xd6,0xc6),(0x8a,0x7f,0x97), "rosepine-dawn FULL"),
            (rosepine_dawn, ChromeDisposition::Zen,
             (0xef,0xe7,0xde),(0x6a,0x65,0x81), (0xe6,0xda,0xcc),(0x62,0x5f,0x6e),
             (0xe6,0xda,0xcc),(0x57,0x52,0x79), (0x57,0x52,0x79),(0xfa,0xf4,0xed),
             (0xef,0xe7,0xde),(0x88,0x81,0x8f), "rosepine-dawn ZEN"),
        ];
        for &(ctor, disp,
               c_bg, c_fg, m_bg, m_fg, o_bg, o_fg, s_bg, s_fg, a_bg, a_fg, label) in cases {
            let mut t = ctor();
            t.derive_chrome(disp);
            assert_face_bg_fg(t.face(SemanticElement::Chrome),         c_bg, c_fg, label);
            assert_face_bg_fg(t.face(SemanticElement::ChromeMuted),    m_bg, m_fg, label);
            assert_face_bg_fg(t.face(SemanticElement::ChromeOverlay),  o_bg, o_fg, label);
            assert_face_bg_fg(t.face(SemanticElement::ChromeSelected), s_bg, s_fg, label);
            assert_face_bg_fg(t.face(SemanticElement::ChromeAccent),   a_bg, a_fg, label);
            assert_eq!(t.face(SemanticElement::ChromeMuted).dim, Some(true), "{label} muted dim");
            assert_eq!(t.face(SemanticElement::Chrome).dim, Some(true), "{label} chrome dim");
            assert_eq!(t.face(SemanticElement::ChromeAccent).bold, Some(true), "{label} accent bold");
        }
    }

    #[test]
    fn derive_stack_ordered_and_floored_all_rgb_builtins() {
        // (a)+(b): for every RGB builtin, at FULL and ZEN, the 3-tone stack is strictly ordered
        // toward the headroom pole AND each elevated adjacent pair clears its CR target. The modal
        // overlay SHARES the dropdown tone (user decision 2026-07-06), so it is NOT a separately
        // floored layer — instead the overlay bg is asserted EQUAL to the dropdown bg.
        for name in Theme::builtin_names() {
            let base = Theme::builtin(name).unwrap();
            if !matches!(base.base_bg, Color::Rgb { .. }) { continue; } // skip terminal-*/no-color
            if !chrome_ladder_is_sentinel(&base) { continue; } // defensive: skip any explicit-chrome theme (all current RGB builtins are all-sentinel)
            for (disp, target) in [(ChromeDisposition::Full, 1.30_f32), (ChromeDisposition::Zen, 1.12)] {
                let mut t = Theme::builtin(name).unwrap();
                t.derive_chrome(disp);
                let canvas = t.base_bg;
                let bar  = t.face(SemanticElement::Chrome).bg.unwrap();
                let drop = t.face(SemanticElement::ChromeMuted).bg.unwrap();
                let ov   = t.face(SemanticElement::ChromeOverlay).bg.unwrap();
                for (below, above, lbl) in [(canvas, bar, "canvas→bar"), (bar, drop, "bar→dropdown")] {
                    assert!(contrast_ratio(above, below) >= target - 0.01,
                        "{name} {disp:?} {lbl}: CR {} < target {target}",
                        contrast_ratio(above, below));
                }
                assert_eq!(ov, drop, "{name} {disp:?}: overlay bg shares the dropdown bg (3-tone ladder)");
            }
        }
    }

    #[test]
    fn derive_full_distinct_from_zen_all_rgb_builtins() {
        // (d): the FULL bar tone and the ZEN bar tone are perceptibly distinct (CR ≥ ~1.14).
        for name in Theme::builtin_names() {
            let base = Theme::builtin(name).unwrap();
            if !matches!(base.base_bg, Color::Rgb { .. }) { continue; }
            if !chrome_ladder_is_sentinel(&base) { continue; } // defensive: skip any explicit-chrome theme (all current RGB builtins are all-sentinel)
            let mut f = Theme::builtin(name).unwrap(); f.derive_chrome(ChromeDisposition::Full);
            let mut z = Theme::builtin(name).unwrap(); z.derive_chrome(ChromeDisposition::Zen);
            let fb = f.face(SemanticElement::Chrome).bg.unwrap();
            let zb = z.face(SemanticElement::Chrome).bg.unwrap();
            assert!(contrast_ratio(fb, zb) >= 1.14,
                "{name}: full≠zen bar CR {} too small", contrast_ratio(fb, zb));
        }
    }

    #[test]
    fn derive_every_chrome_fg_clears_legibility_floor() {
        // (c): every derived chrome fg clears 4.5 vs its own panel, on all RGB builtins.
        for name in Theme::builtin_names() {
            let base = Theme::builtin(name).unwrap();
            if !matches!(base.base_bg, Color::Rgb { .. }) { continue; }
            if !chrome_ladder_is_sentinel(&base) { continue; } // defensive: skip any explicit-chrome theme (all current RGB builtins are all-sentinel)
            for disp in [ChromeDisposition::Full, ChromeDisposition::Zen] {
                let mut t = Theme::builtin(name).unwrap();
                t.derive_chrome(disp);
                for el in [SemanticElement::Chrome, SemanticElement::ChromeMuted, SemanticElement::ChromeOverlay] {
                    let f = t.face(el);
                    assert!(contrast_ratio(f.fg.unwrap(), f.bg.unwrap()) >= 4.5 - 0.05,
                        "{name} {disp:?} {el:?} fg CR {} < 4.5", contrast_ratio(f.fg.unwrap(), f.bg.unwrap()));
                }
            }
        }
    }

    #[test]
    fn derive_overlay_shares_dropdown_tone_all_rgb_builtins() {
        // Load-bearing invariant (user decision 2026-07-06 — the 3-tone ladder): for EVERY RGB
        // builtin at BOTH dispositions the modal overlay SHARES the dropdown's level-2 background
        // (`ChromeOverlay.bg == ChromeMuted.bg`), yet keeps its OWN readable foreground
        // (`ChromeOverlay.fg != ChromeMuted.fg` — primary text, not the dropdown's dim muted fg),
        // and that overlay fg stays legible on the shared panel (CR ≥ 4.5). The exact hex pins are
        // secondary to this invariant.
        for name in Theme::builtin_names() {
            let base = Theme::builtin(name).unwrap();
            if !matches!(base.base_bg, Color::Rgb { .. }) { continue; } // skip terminal-*/no-color
            for disp in [ChromeDisposition::Full, ChromeDisposition::Zen] {
                let mut t = Theme::builtin(name).unwrap();
                t.derive_chrome(disp);
                let overlay = t.face(SemanticElement::ChromeOverlay);
                let muted   = t.face(SemanticElement::ChromeMuted);
                assert_eq!(overlay.bg, muted.bg,
                    "{name} {disp:?}: overlay bg must equal dropdown (ChromeMuted) bg");
                assert_ne!(overlay.fg, muted.fg,
                    "{name} {disp:?}: overlay fg (readable) must differ from dropdown dim fg");
                let cr = contrast_ratio(overlay.fg.unwrap(), overlay.bg.unwrap());
                assert!(cr >= 4.5 - 0.05,
                    "{name} {disp:?}: overlay fg CR {cr} < 4.5 on the shared panel");
            }
        }
    }

    #[test]
    fn derive_fills_only_unset_faces() {
        // Part D: tokyo-night is now ALL-sentinel on chrome/chrome_selected/chrome_muted/
        // chrome_overlay/chrome_accent — all five derive. chrome_reverse is never derived.
        let mut t = tokyo_night();
        // confirm all five are sentinels pre-derive
        for el in [
            SemanticElement::Chrome, SemanticElement::ChromeSelected,
            SemanticElement::ChromeMuted, SemanticElement::ChromeOverlay,
            SemanticElement::ChromeAccent,
        ] {
            assert_eq!(t.face(el), Face::default(), "{el:?} must be sentinel pre-derive");
        }
        let reverse_before = t.face(SemanticElement::ChromeReverse);

        t.derive_chrome(ChromeDisposition::Full);

        // chrome_reverse is never derived — kept as-is
        assert_eq!(t.face(SemanticElement::ChromeReverse), reverse_before, "reverse kept — never derived");
        // all five chrome faces are now non-sentinel (derived)
        for el in [
            SemanticElement::Chrome, SemanticElement::ChromeSelected,
            SemanticElement::ChromeMuted, SemanticElement::ChromeOverlay,
            SemanticElement::ChromeAccent,
        ] {
            assert_ne!(t.face(el), Face::default(), "{el:?} must be derived (non-sentinel)");
        }
        // §II.5 tokyo FULL pins (byte-exact from the probe) — all five chrome faces
        assert_face_bg_fg(t.face(SemanticElement::Chrome),
            (0x2d,0x2f,0x42), (0xa2,0xab,0xd0), "tokyo Chrome FULL (§II.5)");
        assert_face_bg_fg(t.face(SemanticElement::ChromeMuted),
            (0x3d,0x40,0x5a), (0xa8,0xad,0xc4), "tokyo ChromeMuted FULL (§II.5)");
        assert_face_bg_fg(t.face(SemanticElement::ChromeOverlay),
            (0x3d,0x40,0x5a), (0xc0,0xca,0xf5), "tokyo ChromeOverlay FULL (= ChromeMuted bg, §II.5)");
        assert_face_bg_fg(t.face(SemanticElement::ChromeSelected),
            (0xc0,0xca,0xf5), (0x1a,0x1b,0x26), "tokyo ChromeSelected FULL (§II.5)");
        assert_face_bg_fg(t.face(SemanticElement::ChromeAccent),
            (0x2d,0x2f,0x42), (0x8f,0xa3,0xce), "tokyo ChromeAccent FULL (§II.5)");
        assert_eq!(t.face(SemanticElement::ChromeAccent).bold, Some(true), "accent bold");

        // second call is a no-op (idempotency — sentinel rule)
        let snap = t.clone();
        t.derive_chrome(ChromeDisposition::Full);
        assert_eq!(t.faces, snap.faces, "second derive is no-op");
    }

    #[test]
    fn tokyo_standardized_faces() {
        use SemanticElement::*;
        let t = tokyo_night();
        let magenta = Color::Rgb{r:0xbb,g:0x9a,b:0xf7};
        let yellow  = Color::Rgb{r:0xe0,g:0xaf,b:0x68};
        let orange  = Color::Rgb{r:0xff,g:0x9e,b:0x64};
        let comment = Color::Rgb{r:0x56,g:0x5f,b:0x89};
        let blue    = Color::Rgb{r:0x7a,g:0xa2,b:0xf7};
        let bg      = Color::Rgb{r:0x1a,g:0x1b,b:0x26};
        let sel_bg  = Color::Rgb{r:0x29,g:0x2e,b:0x42};   // aligned #292e42
        assert_eq!(t.face(Emphasis).fg, Some(magenta));   assert_eq!(t.face(Emphasis).italic, Some(true));
        assert_eq!(t.face(Strong).fg, Some(yellow));      assert_eq!(t.face(Strong).bold, Some(true));
        assert_eq!(t.face(StrongEmphasis).fg, Some(orange));
        assert_eq!(t.face(StrongEmphasis).bold, Some(true)); assert_eq!(t.face(StrongEmphasis).italic, Some(true));
        assert_eq!(t.face(Strikethrough).fg, Some(comment)); assert_eq!(t.face(Strikethrough).strike, Some(true));
        assert_eq!(t.face(SearchMatch).bg, Some(yellow));  assert_eq!(t.face(SearchMatch).fg, Some(bg));
        assert_eq!(t.face(FrontMatter).fg, Some(orange));  assert_eq!(t.face(FrontMatter).italic, Some(true));
        assert_eq!(t.face(DiagGrammar).underline_color, Some(blue));
        assert_eq!(t.face(WrapGuide).fg, Some(sel_bg));
        assert_eq!(t.face(Selection).bg, Some(sel_bg));
        // chrome faces are now all-None sentinels (pre-derive).
        for el in [Chrome, ChromeSelected, ChromeMuted, ChromeOverlay, ChromeAccent] {
            assert_eq!(t.face(el), Face::default(), "{el:?} sentinel");
        }
        assert_eq!(t.face(ChromeReverse).reverse, Some(true), "chrome_reverse kept");
    }

    #[test]
    fn derive_elevation_ladder_directions() {
        // 3-tone elevation: bar (Chrome) and dropdown (ChromeMuted) each elevate from the canvas
        // toward the headroom pole (LIGHTER on dark themes, DARKER on light), strictly ordered
        // canvas < bar < dropdown by luminance-toward-pole. The modal overlay SHARES the dropdown
        // tone (user decision 2026-07-06): overlay bg == dropdown bg, so no third elevation. §II.5.
        let mut td = flexoki_dark();
        td.derive_chrome(ChromeDisposition::Full);
        let lum = |c: Color| { if let Color::Rgb{r,g,b} = c { rel_lum(r,g,b) } else { 0.0 } };
        let canvas = lum(Color::Rgb{r:0x10,g:0x0f,b:0x0f});
        let bar  = lum(td.face(SemanticElement::Chrome).bg.unwrap());
        let drop = lum(td.face(SemanticElement::ChromeMuted).bg.unwrap());
        assert!(canvas < bar && bar < drop,
            "dark theme: canvas < bar < dropdown by luminance; canvas={canvas} bar={bar} drop={drop}");
        assert_eq!(td.face(SemanticElement::ChromeOverlay).bg, td.face(SemanticElement::ChromeMuted).bg,
            "dark theme: overlay bg shares the dropdown bg (3-tone ladder)");
        // exact §II.5 pins (redundant with derive_chrome_base16_pins but keeps this self-contained)
        assert_face_bg_fg(td.face(SemanticElement::Chrome),
            (0x2a,0x28,0x28), (0xac,0xab,0xa3), "fd chrome");

        // light polarity: elevation goes DARKER (toward black), still strictly ordered.
        let mut tl = flexoki_light();
        tl.derive_chrome(ChromeDisposition::Full);
        let canvas_l = lum(Color::Rgb{r:0xff,g:0xfc,b:0xf0});
        let bar_l  = lum(tl.face(SemanticElement::Chrome).bg.unwrap());
        let drop_l = lum(tl.face(SemanticElement::ChromeMuted).bg.unwrap());
        assert!(canvas_l > bar_l && bar_l > drop_l,
            "light theme: canvas > bar > dropdown by luminance");
        assert_eq!(tl.face(SemanticElement::ChromeOverlay).bg, tl.face(SemanticElement::ChromeMuted).bg,
            "light theme: overlay bg shares the dropdown bg (3-tone ladder)");
        assert_face_bg_fg(tl.face(SemanticElement::Chrome),
            (0xe5,0xdf,0xc8), (0x3b,0x3a,0x38), "fl chrome");  // §II.5 (S-capped)
    }

    #[test]
    fn derive_zen_floored_but_distinct_on_pole_side() {
        let mut td_full = flexoki_dark();
        td_full.derive_chrome(ChromeDisposition::Full);
        let mut td_zen = flexoki_dark();
        td_zen.derive_chrome(ChromeDisposition::Zen);

        // Zen §II.5 pins (flexoki-dark ZEN)
        assert_face_bg_fg(td_zen.face(SemanticElement::Chrome),
            (0x1e,0x1c,0x1c), (0xac,0xab,0xa3), "fd zen chrome");
        assert_face_bg_fg(td_zen.face(SemanticElement::ChromeOverlay),
            (0x28,0x26,0x26), (0xce,0xcd,0xc3), "fd zen overlay (= zen dropdown bg)");
        assert_face_bg_fg(td_zen.face(SemanticElement::ChromeMuted),
            (0x28,0x26,0x26), (0x8e,0x8d,0x86), "fd zen muted");
        assert_face_bg_fg(td_zen.face(SemanticElement::ChromeAccent),
            (0x1e,0x1c,0x1c), (0x6e,0x82,0x94), "fd zen accent");

        // zen bar is strictly between canvas and the full bar, on the pole side (dark → white).
        let lum = |c: Color| { if let Color::Rgb{r,g,b} = c { rel_lum(r,g,b) } else { 0.0 } };
        let full_bar = lum(td_full.face(SemanticElement::Chrome).bg.unwrap());
        let zen_bar  = lum(td_zen.face(SemanticElement::Chrome).bg.unwrap());
        let canvas   = lum(Color::Rgb{r:0x10,g:0x0f,b:0x0f});
        assert!(canvas < zen_bar && zen_bar < full_bar,
            "dark: canvas < zen bar < full bar; canvas={canvas} zen={zen_bar} full={full_bar}");
        // overlay is likewise elevated above the canvas at zen, and below the full overlay.
        let full_ov = lum(td_full.face(SemanticElement::ChromeOverlay).bg.unwrap());
        let zen_ov  = lum(td_zen.face(SemanticElement::ChromeOverlay).bg.unwrap());
        assert!(canvas < zen_ov && zen_ov < full_ov,
            "dark: canvas < zen overlay < full overlay");
    }

    #[test]
    fn derive_rungs_preserve_canvas_saturation() {
        // Unified elevation: every rung moves toward the SAME pole and keeps the canvas H,S.
        // No sunken/raised split — each rung's HSL-S ≈ canvas S on dark/uncapped themes.
        let mut t = catppuccin_mocha();          // dark, uncapped, canvas S ≈ 0.21
        t.derive_chrome(ChromeDisposition::Full);
        let (_, canvas_s, _) = rgb_to_hsl(0x1e, 0x1e, 0x2e);
        for el in [SemanticElement::Chrome, SemanticElement::ChromeMuted, SemanticElement::ChromeOverlay] {
            if let Color::Rgb { r, g, b } = t.face(el).bg.unwrap() {
                let (_, s, _) = rgb_to_hsl(r, g, b);
                assert!((s - canvas_s).abs() < 0.02,
                    "{el:?} preserves canvas S: rung_s={s:.4} canvas_s={canvas_s:.4}");
            } else { panic!("non-Rgb rung"); }
        }
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
        // §II.5 phosphor-green FULL pins (T4-finalized: fg pins now exact — base_fg = shade(hue,3)
        // settled at 0.78 ceiling, so Chrome/Muted/Overlay fgs are stable).
        let mut t = phosphor_green_theme();
        t.derive_chrome(ChromeDisposition::Full);
        assert_eq!(t.face(SemanticElement::Chrome).bg,        Some(rgb(0x00,0x40,0x00)), "phosphor chrome bg");
        assert_eq!(t.face(SemanticElement::Chrome).fg,        Some(rgb(0x00,0xd8,0x00)), "phosphor chrome fg");
        assert_eq!(t.face(SemanticElement::ChromeMuted).bg,   Some(rgb(0x00,0x54,0x00)), "phosphor muted bg");
        assert_eq!(t.face(SemanticElement::ChromeMuted).fg,   Some(rgb(0x54,0xcd,0x54)), "phosphor muted fg");
        assert_eq!(t.face(SemanticElement::ChromeOverlay).bg, Some(rgb(0x00,0x54,0x00)), "phosphor overlay bg (= dropdown bg)");
        assert_eq!(t.face(SemanticElement::ChromeOverlay).fg, Some(rgb(0x00,0xff,0x00)), "phosphor overlay fg");
        assert_eq!(t.face(SemanticElement::ChromeAccent).bg,  Some(rgb(0x00,0x40,0x00)), "phosphor accent bg");
        assert_eq!(t.face(SemanticElement::ChromeAccent).fg,  Some(rgb(0xbb,0xf3,0xbb)), "phosphor accent fg");
        // hue angle: all bg rungs share the green hue family (r ≈ b, g dominates); fgs likewise green.
        for el in [SemanticElement::Chrome, SemanticElement::ChromeOverlay, SemanticElement::ChromeMuted] {
            let bg = t.face(el).bg.unwrap();
            if let Color::Rgb { r, g, b } = bg {
                assert!(g >= r && g >= b, "{el:?} bg must be green-dominant r={r} g={g} b={b}");
            } else { panic!("{el:?} non-Rgb bg"); }
            let fg = t.face(el).fg;
            assert!(fg.is_some(), "{el:?} fg is Some");
            if let Some(Color::Rgb { r, g, b }) = fg {
                assert!(g >= r && g >= b, "{el:?} fg green-dominant r={r} g={g} b={b}");
            } else { panic!("{el:?} non-Rgb fg"); }
        }
    }

    #[test]
    fn derive_separation_floor_grows_low_contrast_theme() {
        // Synthetic LIGHT-polarity theme with a near-white fg (bg #f8f8f8, fg/link #e0e0e0) —
        // originally fg-vs-canvas contrast is far below 4.5. Under unified elevation there is no
        // shrink-to-canvas: the separation floor GROWS each rung (toward black, ordered) and
        // derive_fg re-derives every chrome fg to clear 4.5. §II.5a pins.
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
        t.derive_chrome(ChromeDisposition::Full);
        // §II.5a FULL pins (synthetic bg #f8f8f8, fg/link #e0e0e0):
        assert_face_bg_fg(t.face(SemanticElement::Chrome),
            (0xdb,0xdb,0xdb), (0x60,0x60,0x60), "synthetic chrome");
        assert_face_bg_fg(t.face(SemanticElement::ChromeMuted),
            (0xc1,0xc1,0xc1), (0x4f,0x4f,0x4f), "synthetic muted");
        assert_face_bg_fg(t.face(SemanticElement::ChromeOverlay),
            (0xc1,0xc1,0xc1), (0x4e,0x4e,0x4e), "synthetic overlay (= dropdown bg)");
        // rungs are DISTINCT from canvas (elevated toward black), and every fg clears 4.5.
        let canvas = Color::Rgb{r:0xf8,g:0xf8,b:0xf8};
        for el in [SemanticElement::Chrome, SemanticElement::ChromeMuted, SemanticElement::ChromeOverlay] {
            let f = t.face(el);
            assert_ne!(f.bg.unwrap(), canvas, "{el:?} must be distinct from canvas");
            assert!(contrast_ratio(f.fg.unwrap(), f.bg.unwrap()) >= 4.5 - 0.01,
                "{el:?} fg clears the legibility floor");
        }
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

    #[test]
    fn e5_chrome_bar_fg_recedes_and_dims() {
        use SemanticElement::*;
        // FLOOR-AWARE. DIM is the always-present recede signal on every RGB theme; the recede CR check
        // only holds where body text itself clears the legibility floor on the bar panel — on the
        // low-contrast themes (phosphor-red/blue/purple, solarized-dark/light) the pre-existing FG_FLOOR
        // guard legitimately dominates and DIM carries the recede.
        for name in Theme::builtin_names() {
            let mut t = Theme::builtin(name).expect("builtin name resolves");
            let base_fg = t.base_fg;
            if !matches!(base_fg, Color::Rgb { .. }) { continue; } // non-RGB → derive_chrome skips
            t.derive_chrome(ChromeDisposition::Full);
            let chrome = t.face(Chrome);
            let cbg = chrome.bg.expect("derived chrome bg");
            let cfg = chrome.fg.expect("derived chrome fg");
            let cr = |a: Color, b: Color| contrast_ratio(a, b);
            assert_eq!(chrome.dim, Some(true), "{name}: chrome must carry DIM");
            let body_on_bar = cr(base_fg, cbg);
            if body_on_bar >= FG_FLOOR {
                assert!(cr(cfg, cbg) <= body_on_bar + CR_TOL,
                    "{name}: chrome fg must not out-contrast body text on the bar panel (recede)");
            }
        }
    }

    #[test]
    fn e5_non_rgb_chrome_carries_dim() {
        use SemanticElement::*;
        // default() = terminal-plain; terminal_ansi(); no_color() are the three non-RGB builtins.
        for t in [default(), terminal_ansi(), no_color()] {
            assert_eq!(t.face(Chrome).dim, Some(true), "{} chrome must carry DIM", t.name);
        }
    }
}
