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
}
