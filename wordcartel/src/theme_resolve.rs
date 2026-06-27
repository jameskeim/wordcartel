//! Shell theme resolution: env depth detection + `resolve_theme` (Task 5).
//! Core stays IO-free; this is where env/file reading happens.

use wordcartel_core::theme::Depth;

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

use wordcartel_core::theme::{self, Color, Face, Theme};
use crate::config::{ThemeConfig, RawFace};

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

pub fn resolve_theme(tc: &ThemeConfig, env: &EnvSnapshot) -> ResolvedTheme {
    let mut warnings = Vec::new();

    let detected = detect_depth(env.no_color, env.colorterm.as_deref(), env.term.as_deref());
    let explicit = tc.depth.as_ref().and_then(|d| match parse_depth(d) {
        Some(x) => Some(x),
        None => { warnings.push(format!("theme: unknown depth `{d}` — using detection")); None }
    });
    let depth = effective_depth(env.no_color, explicit, detected);

    // Base theme: depth==None → no_color(); else file > name > default.
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
        match Theme::builtin(name) {   // ASSOCIATED method (impl Theme), NOT a free fn (Codex C3)
            Some(th) => th,
            None => { warnings.push(format!("theme: unknown name `{name}` — using default")); theme::default() }
        }
    } else {
        theme::default()
    };

    // Per-element style overrides. On a MONOCHROME theme (cue mode by theme), color
    // fields are dropped (modifiers still apply) so an override can't defeat the §4
    // cue discipline (Codex C2); the Depth::None case is already color-stripped at
    // render (Task 4b). A non-monochrome theme keeps colors.
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
    use wordcartel_core::theme::Depth;

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
        let r = resolve_theme(&tc, &env(false));
        assert_eq!(r.theme.name, "tokyo-night");
        assert_eq!(r.depth, Depth::Truecolor);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn resolve_unknown_name_falls_back_with_warning() {
        let tc = ThemeConfig { name: Some("nope".into()), ..Default::default() };
        let r = resolve_theme(&tc, &env(false));
        assert_eq!(r.theme.name, "default");
        assert!(r.warnings.iter().any(|w| w.contains("nope")));
    }

    #[test]
    fn no_color_forces_no_color_theme_and_none_depth() {
        let tc = ThemeConfig { name: Some("tokyo-night".into()), ..Default::default() };
        let r = resolve_theme(&tc, &env(true)); // NO_COLOR set
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
        let r = resolve_theme(&tc, &env(false));
        // good override applied
        assert_eq!(r.theme.face(SemanticElement::Selection).bg, Some(Color::Rgb { r:0x28, g:0x34, b:0x57 }));
        // partial: bold applied even though fg was bad
        assert_eq!(r.theme.face(SemanticElement::Heading(1)).bold, Some(true));
        // warnings for the bad hex AND the unknown key
        assert!(r.warnings.iter().any(|w| w.contains("not-a-color") || w.contains("heading1")));
        assert!(r.warnings.iter().any(|w| w.contains("bogus_key")));
    }
}
