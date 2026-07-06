//! Shell theme resolution: env depth detection + `resolve_theme` (Task 5).
//! Core stays IO-free; this is where env/file reading happens.

use wordcartel_core::theme::{self, Color, Depth, Face, Theme, ChromeDisposition, SemanticElement};
use crate::config::{ThemeConfig, RawFace};

/// Detect color depth from environment values. Case-insensitive.
/// `NO_COLOR` set → None; `TERM` empty/`dumb` → None; `COLORTERM`∈{truecolor,24bit}
/// → Truecolor; `TERM` `*-direct*` → Truecolor; `*256color*` → Indexed256; else Ansi16.
pub fn detect_depth(no_color: bool, colorterm: Option<&str>, term: Option<&str>) -> Depth {
    if no_color { return Depth::None; }
    let term_l = term.map(|t| t.to_ascii_lowercase());
    match term_l.as_deref() {
        None | Some("") | Some("dumb") => return Depth::None,
        _ => {}
    }
    if let Some(ct) = colorterm {
        let ct = ct.to_ascii_lowercase();
        if ct == "truecolor" || ct == "24bit" { return Depth::Truecolor; }
    }
    let term_l = term_l.unwrap(); // not None per the match above
    if term_l.contains("-direct") { return Depth::Truecolor; }
    if term_l.contains("256color") { return Depth::Indexed256; }
    Depth::Ansi16
}

/// Parse an explicit `[theme] depth` string. Case-insensitive.
pub fn parse_depth(s: &str) -> Option<Depth> {
    match s.trim().to_ascii_lowercase().as_str() {
        "truecolor" => Some(Depth::Truecolor),
        "256" => Some(Depth::Indexed256),
        "16" => Some(Depth::Ansi16),
        "none" => Some(Depth::None),
        _ => None,
    }
}

/// Effective depth precedence: NO_COLOR > explicit `[theme] depth` > detection.
pub fn effective_depth(no_color: bool, explicit: Option<Depth>, detected: Depth) -> Depth {
    if no_color { return Depth::None; }
    explicit.unwrap_or(detected)
}

pub struct EnvSnapshot { pub no_color: bool, pub colorterm: Option<String>, pub term: Option<String> }

impl EnvSnapshot {
    pub fn from_env() -> EnvSnapshot {
        EnvSnapshot {
            no_color: std::env::var_os("NO_COLOR").is_some(),
            colorterm: std::env::var("COLORTERM").ok(),
            term: std::env::var("TERM").ok(),
        }
    }
}

pub struct ResolvedTheme { pub theme: Theme, pub depth: Depth, pub warnings: Vec<String> }

/// Parse a `[theme] chrome` config string into a `ChromeDisposition`.
///
/// Returns the disposition and an optional warning string.
/// `"full"` or `None` → `Full` (silent). `"zen"` → `Zen`. Unknown value → `Full` + warning.
pub fn parse_chrome(s: &Option<String>) -> (ChromeDisposition, Option<String>) {
    match s.as_deref() {
        None | Some("full") => (ChromeDisposition::Full, None),
        Some("zen") => (ChromeDisposition::Zen, None),
        Some(other) => (ChromeDisposition::Full,
            Some(format!("theme.chrome: unknown value `{other}` — using full"))),
    }
}

/// Convert a config `RawFace` (hex strings) to a core `Face`; push a warning per bad hex.
fn raw_face_to_face(key: &str, rf: &RawFace, warnings: &mut Vec<String>) -> Face {
    let mut hex = |s: &Option<String>, field: &str| -> Option<Color> {
        let s = s.as_ref()?;
        match crate::base16::parse_hex6(s) {
            Some(c) => Some(c),
            None => { warnings.push(format!("theme.styles.{key}.{field}: invalid color `{s}`")); None }
        }
    };
    Face {
        fg: hex(&rf.fg, "fg"),
        bg: hex(&rf.bg, "bg"),
        underline_color: hex(&rf.underline_color, "underline_color"),
        bold: rf.bold, italic: rf.italic, underline: rf.underline,
        strike: rf.strike, reverse: rf.reverse, dim: rf.dim,
    }
}

/// Cue mode = effective depth None OR a monochrome theme. In cue mode the heading
/// glyph is forced ON; otherwise the config override (if any) applies, else the
/// theme's own flag. Spec §4.
fn apply_cue_mode_glyph(theme: &mut Theme, depth: Depth, cfg_override: Option<bool>) {
    let cue = depth == Depth::None || theme.monochrome;
    theme.heading_level_glyph = if cue { true } else { cfg_override.unwrap_or(theme.heading_level_glyph) };
}

/// Resolve order (D1+D5): base pick/construct → derive_chrome(disp) → Ansi16 policy →
/// user styles → cue glyph. User chrome overrides land LAST, over the depth policy.
pub fn resolve_theme(tc: &ThemeConfig, env: &EnvSnapshot, disp: ChromeDisposition) -> ResolvedTheme {
    let mut warnings = Vec::new();

    let detected = detect_depth(env.no_color, env.colorterm.as_deref(), env.term.as_deref());
    let explicit = tc.depth.as_ref().and_then(|d| match parse_depth(d) {
        Some(x) => Some(x),
        None => { warnings.push(format!("theme: unknown depth `{d}` — using detection")); None }
    });
    let depth = effective_depth(env.no_color, explicit, detected);

    // Base theme: depth==None → no_color(); else file > name > launch-default (flexoki-dark).
    // Aliases resolved HERE (not in builtin): "default" → terminal-plain (warn);
    // "phosphor-X-flat" → "phosphor-X" (warn). Error fallbacks → theme::default().
    let mut t = if depth == Depth::None {
        theme::no_color()
    } else if let Some(path) = &tc.file {
        match std::fs::read_to_string(path) {
            Ok(text) => match crate::base16::parse_base16(&text) {
                Ok((pal, scheme)) => {
                    let name = scheme.unwrap_or_else(|| format!("base16:{}", path.display()));
                    theme::from_base16(&name, pal)
                }
                Err(e) => { warnings.push(format!("theme file {}: {e} — using default", path.display())); theme::default() }
            },
            Err(e) => { warnings.push(format!("theme file {}: {e} — using default", path.display())); theme::default() }
        }
    } else if let Some(name) = &tc.name {
        // Resolve "default" alias and "-flat" fallbacks at the resolve layer (with warnings),
        // keeping builtin() itself clean (plan §D5, T2 review Important).
        let resolved_name = if name == "default" {
            warnings.push("theme 'default' renamed 'terminal-plain' — update your config".to_string());
            "terminal-plain".to_string()
        } else if let Some(base) = name.strip_suffix("-flat") {
            warnings.push(format!("theme '{name}' removed; using '{base}'"));
            base.to_string()
        } else {
            name.clone()
        };
        match Theme::builtin(&resolved_name) {   // ASSOCIATED method (impl Theme), NOT a free fn
            Some(th) => th,
            None => { warnings.push(format!("theme: unknown name `{name}` — using default")); theme::default() }
        }
    } else {
        // No name, no file: launch default is flexoki-dark (D5). Depth::None already handled above.
        Theme::builtin("flexoki-dark").expect("flexoki-dark is a bundled builtin")
    };

    // D1 resolve order step 1: derive chrome ladder from Rgb bases.
    // No-op for non-Rgb themes (terminal-plain, terminal-ansi, no-color, error fallbacks).
    t.derive_chrome(disp);

    // D1 resolve order step 2: Ansi16 fixed chrome policy.
    // At Depth::Ansi16 on an Rgb-based theme, overwrite the five color faces with the
    // named-ANSI table keyed on the binary predicate: quantize(canvas) == Black → DarkGray
    // arm (dark themes); else → Black arm (light themes). ChromeReverse is EXCLUDED
    // (never derived; reverse-modifier default stands at all depths). §C, B.4.
    if depth == Depth::Ansi16 {
        if let Color::Rgb { .. } = t.base_bg {
            let canvas_q = theme::quantize(t.base_bg, Depth::Ansi16);
            if canvas_q == Color::Black {
                // Dark canvas arm: Chrome/Overlay → DarkGray bg White fg; Selected → Black/White;
                // Muted → White dim; Accent → White bold.
                t.override_face(SemanticElement::Chrome,
                    Face { fg: Some(Color::White), bg: Some(Color::DarkGray), ..Face::default() });
                t.override_face(SemanticElement::ChromeOverlay,
                    Face { fg: Some(Color::White), bg: Some(Color::DarkGray), ..Face::default() });
                t.override_face(SemanticElement::ChromeSelected,
                    Face { fg: Some(Color::Black), bg: Some(Color::White), ..Face::default() });
                t.override_face(SemanticElement::ChromeMuted,
                    Face { fg: Some(Color::White), dim: Some(true), ..Face::default() });
                t.override_face(SemanticElement::ChromeAccent,
                    Face { fg: Some(Color::White), bold: Some(true), ..Face::default() });
            } else {
                // Light canvas arm: Chrome/Overlay → Black bg White fg; Selected → White/Black;
                // Muted → White dim; Accent → White bold.
                t.override_face(SemanticElement::Chrome,
                    Face { fg: Some(Color::White), bg: Some(Color::Black), ..Face::default() });
                t.override_face(SemanticElement::ChromeOverlay,
                    Face { fg: Some(Color::White), bg: Some(Color::Black), ..Face::default() });
                t.override_face(SemanticElement::ChromeSelected,
                    Face { fg: Some(Color::Black), bg: Some(Color::White), ..Face::default() });
                t.override_face(SemanticElement::ChromeMuted,
                    Face { fg: Some(Color::White), dim: Some(true), ..Face::default() });
                t.override_face(SemanticElement::ChromeAccent,
                    Face { fg: Some(Color::White), bold: Some(true), ..Face::default() });
            }
        }
    }

    // D1 resolve order step 3: per-element style overrides. On a MONOCHROME theme (cue mode
    // by theme), color fields are dropped (modifiers still apply) so an override can't defeat
    // the §4 cue discipline (Codex C2). Note: depth==None always yields a monochrome theme
    // via no_color(), so the C2 scrub ALSO covers that path (doubly protected) —
    // do NOT remove this check. A non-monochrome theme keeps colors.
    // User chrome overrides land LAST — over the Ansi16 depth policy (Codex plan r1 I1).
    for (key, rf) in &tc.styles {
        match theme::element_from_key(key) {
            Some(el) => {
                let mut patch = raw_face_to_face(key, rf, &mut warnings);
                if t.monochrome && (patch.fg.is_some() || patch.bg.is_some() || patch.underline_color.is_some()) {
                    warnings.push(format!("theme.styles.{key}: color ignored on a monochrome theme (cue mode)"));
                    patch.fg = None; patch.bg = None; patch.underline_color = None;
                }
                t.override_face(el, patch);
            }
            None => warnings.push(format!("theme.styles: unknown element key `{key}`")),
        }
    }

    apply_cue_mode_glyph(&mut t, depth, tc.heading_level_glyph);
    ResolvedTheme { theme: t, depth, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_core::theme::{Depth, ChromeDisposition};

    #[test]
    fn detect_depth_rules() {
        // NO_COLOR wins → None
        assert_eq!(detect_depth(true, Some("truecolor"), Some("xterm-256color")), Depth::None);
        // dumb / empty TERM → None
        assert_eq!(detect_depth(false, None, Some("dumb")), Depth::None);
        assert_eq!(detect_depth(false, None, Some("")), Depth::None);
        // COLORTERM truecolor/24bit → Truecolor (case-insensitive)
        assert_eq!(detect_depth(false, Some("TrueColor"), Some("xterm")), Depth::Truecolor);
        assert_eq!(detect_depth(false, Some("24bit"), Some("xterm")), Depth::Truecolor);
        // TERM *-direct* → Truecolor
        assert_eq!(detect_depth(false, None, Some("xterm-direct")), Depth::Truecolor);
        // *256color* → Indexed256
        assert_eq!(detect_depth(false, None, Some("screen-256color")), Depth::Indexed256);
        // else → Ansi16
        assert_eq!(detect_depth(false, None, Some("xterm")), Depth::Ansi16);
    }

    #[test]
    fn parse_depth_values() {
        assert_eq!(parse_depth("truecolor"), Some(Depth::Truecolor));
        assert_eq!(parse_depth("256"), Some(Depth::Indexed256));
        assert_eq!(parse_depth("16"), Some(Depth::Ansi16));
        assert_eq!(parse_depth("NONE"), Some(Depth::None));
        assert_eq!(parse_depth("nonsense"), None);
    }

    #[test]
    fn effective_depth_precedence() {
        // NO_COLOR forces None even with an explicit override
        assert_eq!(effective_depth(true, Some(Depth::Truecolor), Depth::Ansi16), Depth::None);
        // explicit beats detection
        assert_eq!(effective_depth(false, Some(Depth::Indexed256), Depth::Truecolor), Depth::Indexed256);
        // no explicit → detection
        assert_eq!(effective_depth(false, None, Depth::Truecolor), Depth::Truecolor);
    }

    // -----------------------------------------------------------------------
    // Task 5: resolve_theme
    // -----------------------------------------------------------------------

    use crate::config::{ThemeConfig, RawFace};
    use wordcartel_core::theme::{SemanticElement, Color};

    fn env(no_color: bool) -> EnvSnapshot {
        EnvSnapshot { no_color, colorterm: Some("truecolor".into()), term: Some("xterm-256color".into()) }
    }

    #[test]
    fn resolve_builtin_name() {
        let tc = ThemeConfig { name: Some("tokyo-night".into()), ..Default::default() };
        let r = resolve_theme(&tc, &env(false), ChromeDisposition::Full);
        assert_eq!(r.theme.name, "tokyo-night");
        assert_eq!(r.depth, Depth::Truecolor);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn resolve_unknown_name_falls_back_with_warning() {
        let tc = ThemeConfig { name: Some("nope".into()), ..Default::default() };
        let r = resolve_theme(&tc, &env(false), ChromeDisposition::Full);
        // fallback calls theme::default() whose name is "terminal-plain" (D5)
        assert_eq!(r.theme.name, "terminal-plain");
        assert!(r.warnings.iter().any(|w| w.contains("nope")));
    }

    #[test]
    fn no_color_forces_no_color_theme_and_none_depth() {
        let tc = ThemeConfig { name: Some("tokyo-night".into()), ..Default::default() };
        let r = resolve_theme(&tc, &env(true), ChromeDisposition::Full); // NO_COLOR set
        assert_eq!(r.depth, Depth::None);
        assert_eq!(r.theme.name, "no-color");
        assert!(r.theme.monochrome);
        assert!(r.theme.heading_level_glyph, "cue mode forces the heading glyph on");
    }

    #[test]
    fn styles_override_per_field_with_bad_hex_warning() {
        let mut styles = std::collections::BTreeMap::new();
        styles.insert("selection".to_string(), RawFace { bg: Some("#283457".into()), ..Default::default() });
        styles.insert("heading1".to_string(), RawFace { fg: Some("not-a-color".into()), bold: Some(true), ..Default::default() });
        styles.insert("bogus_key".to_string(), RawFace { fg: Some("#ffffff".into()), ..Default::default() });
        let tc = ThemeConfig { name: Some("default".into()), styles, ..Default::default() };
        let r = resolve_theme(&tc, &env(false), ChromeDisposition::Full);
        // good override applied
        assert_eq!(r.theme.face(SemanticElement::Selection).bg, Some(Color::Rgb { r:0x28, g:0x34, b:0x57 }));
        // partial: bold applied even though fg was bad
        assert_eq!(r.theme.face(SemanticElement::Heading(1)).bold, Some(true));
        // warnings for the bad hex AND the unknown key (and the "default" alias)
        assert!(r.warnings.iter().any(|w| w.contains("not-a-color") || w.contains("heading1")));
        assert!(r.warnings.iter().any(|w| w.contains("bogus_key")));
    }

    // C2 invariant: monochrome theme must strip color overrides and keep modifiers.
    // This test FAILS if the scrub block in resolve_theme is removed or the condition is inverted.
    #[test]
    fn monochrome_theme_strips_color_overrides_but_keeps_modifiers() {
        let mut styles = std::collections::BTreeMap::new();
        styles.insert("heading1".to_string(),
            RawFace { fg: Some("#ff0000".into()), bold: Some(true), ..Default::default() });
        let tc = ThemeConfig { styles, ..Default::default() };
        let r = resolve_theme(&tc, &env(true), ChromeDisposition::Full); // NO_COLOR → no_color() → monochrome
        assert!(r.theme.monochrome);
        assert_eq!(r.theme.face(SemanticElement::Heading(1)).fg, None, "color stripped in cue mode");
        assert_eq!(r.theme.face(SemanticElement::Heading(1)).bold, Some(true), "modifier preserved");
        assert!(r.warnings.iter().any(|w| w.contains("heading1")), "cue-mode strip warning emitted");
    }

    // -----------------------------------------------------------------------
    // Task 3: chrome axis — disposition, Ansi16 policy, aliases, launch default
    // -----------------------------------------------------------------------

    #[test]
    fn no_config_resolves_flexoki_dark() {
        // Empty ThemeConfig (no name, no file) → launch default = flexoki-dark (D5).
        let r = resolve_theme(&ThemeConfig::default(), &env(false), ChromeDisposition::Full);
        assert_eq!(r.theme.name, "flexoki-dark");
    }

    #[test]
    fn no_color_env_still_wins() {
        // NO_COLOR + empty config → no-color theme (Depth::None wins over the launch default).
        let r = resolve_theme(&ThemeConfig::default(), &env(true), ChromeDisposition::Full);
        assert_eq!(r.theme.name, "no-color");
        assert_eq!(r.depth, Depth::None);
    }

    #[test]
    fn default_name_aliases_with_warning() {
        // name="default" → alias to "terminal-plain" + warning containing "default".
        let tc = ThemeConfig { name: Some("default".into()), ..Default::default() };
        let r = resolve_theme(&tc, &env(false), ChromeDisposition::Full);
        assert_eq!(r.theme.name, "terminal-plain");
        assert!(r.warnings.iter().any(|w| w.contains("default")),
            "warning about 'default' alias must be emitted");
    }

    #[test]
    fn flat_name_falls_back_with_warning() {
        // name="phosphor-amber-flat" → removed; falls back to "phosphor-amber" + warning.
        let tc = ThemeConfig { name: Some("phosphor-amber-flat".into()), ..Default::default() };
        let r = resolve_theme(&tc, &env(false), ChromeDisposition::Full);
        assert_eq!(r.theme.name, "phosphor-amber",
            "phosphor-amber-flat must resolve to phosphor-amber base");
        assert!(r.warnings.iter().any(|w| w.contains("flat")),
            "warning about removed flat name must be emitted");
    }

    #[test]
    fn chrome_key_parses_and_derives() {
        // parse_chrome: "zen" → Zen; the Zen flexoki-dark Chrome bg matches §B.3.
        let (disp, warn) = parse_chrome(&Some("zen".into()));
        assert_eq!(disp, ChromeDisposition::Zen);
        assert!(warn.is_none(), "known key 'zen' must not warn");

        let tc = ThemeConfig { name: Some("flexoki-dark".into()), ..Default::default() };
        let r = resolve_theme(&tc, &env(false), disp);
        // §B.3 ZEN flexoki-dark Chrome bg = #0f0e0e
        assert_eq!(r.theme.face(SemanticElement::Chrome).bg,
            Some(Color::Rgb { r:0x0f, g:0x0e, b:0x0e }),
            "flexoki-dark Zen Chrome bg must match §B.3 probe value");
    }

    #[test]
    fn unknown_chrome_warns_full() {
        // Unknown chrome string → Full disposition + warning.
        let (disp, warn) = parse_chrome(&Some("invalid".into()));
        assert_eq!(disp, ChromeDisposition::Full);
        let w = warn.expect("unknown chrome value must produce a warning");
        assert!(w.contains("invalid"), "warning must name the unknown value");
    }

    #[test]
    fn ansi16_policy_replaces_derived_chrome() {
        // flexoki-dark @ Ansi16: canvas #100f0f → Black → DarkGray arm.
        // Chrome bg = DarkGray, fg = White (≠ canvas Black).
        let tc = ThemeConfig {
            name: Some("flexoki-dark".into()),
            depth: Some("16".into()),
            ..Default::default()
        };
        let r = resolve_theme(&tc, &env(false), ChromeDisposition::Full);
        assert_eq!(r.depth, Depth::Ansi16);
        assert_eq!(r.theme.face(SemanticElement::Chrome).bg, Some(Color::DarkGray),
            "Ansi16 dark-canvas policy: Chrome bg must be DarkGray");
        assert_eq!(r.theme.face(SemanticElement::Chrome).fg, Some(Color::White),
            "Ansi16 dark-canvas policy: Chrome fg must be White");
        // Canvas itself is Black (not DarkGray) — the separation is the policy's purpose.
        assert_eq!(theme::quantize(r.theme.base_bg, Depth::Ansi16), Color::Black,
            "flexoki-dark canvas quantizes to Black at Ansi16");

        // flexoki-light @ Ansi16: canvas #fffcf0 → White → Black arm.
        let tc2 = ThemeConfig {
            name: Some("flexoki-light".into()),
            depth: Some("16".into()),
            ..Default::default()
        };
        let r2 = resolve_theme(&tc2, &env(false), ChromeDisposition::Full);
        assert_eq!(r2.theme.face(SemanticElement::Chrome).bg, Some(Color::Black),
            "Ansi16 light-canvas policy: Chrome bg must be Black");
        assert_eq!(r2.theme.face(SemanticElement::Chrome).fg, Some(Color::White),
            "Ansi16 light-canvas policy: Chrome fg must be White");

        // tokyo-night @ Ansi16: canvas #1a1b26 → Black → DarkGray arm.
        // The explicit PANEL_BG #16161e is overwritten by the Ansi16 policy.
        let tc3 = ThemeConfig {
            name: Some("tokyo-night".into()),
            depth: Some("16".into()),
            ..Default::default()
        };
        let r3 = resolve_theme(&tc3, &env(false), ChromeDisposition::Full);
        assert_eq!(r3.theme.face(SemanticElement::Chrome).bg, Some(Color::DarkGray),
            "Ansi16 policy overwrites tokyo's explicit PANEL_BG with DarkGray");
    }

    #[test]
    fn user_styles_override_ansi16_policy() {
        // User [theme.styles] chrome override lands AFTER the Ansi16 policy — order pin.
        let mut styles = std::collections::BTreeMap::new();
        styles.insert("chrome".to_string(),
            RawFace { bg: Some("#ff0000".into()), ..Default::default() });
        let tc = ThemeConfig {
            name: Some("flexoki-dark".into()),
            depth: Some("16".into()),
            styles,
            ..Default::default()
        };
        let r = resolve_theme(&tc, &env(false), ChromeDisposition::Full);
        // Policy would set Chrome bg to DarkGray, but user override replaces it with #ff0000.
        assert_eq!(r.theme.face(SemanticElement::Chrome).bg,
            Some(Color::Rgb { r:0xff, g:0x00, b:0x00 }),
            "user chrome style override must land after the Ansi16 policy");
    }
}
