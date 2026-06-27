//! Hand-rolled base16/base24 palette parser (NO YAML dependency — serde_yml is
//! deprecated / RUSTSEC-2025-0068). base16 files are a flat `key: "rrggbb"` map.
//! Core stays IO-free; the caller reads the file and passes its text here.

use wordcartel_core::theme::{BasePalette, Color};

/// Parse a 6-hex-digit color, tolerant of a leading `#` and surrounding quotes.
pub fn parse_hex6(s: &str) -> Option<Color> {
    let s = s.trim().trim_matches('"').trim_matches('\'').trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb { r, g, b })
}

/// Parse a base16 (or base24) flat map into a `BasePalette` + optional scheme name.
/// `Err(msg)` if any of `base00..base0F` is missing or unparseable. If all of
/// `base10..base17` are present too, the palette is base24 (`extra` populated).
pub fn parse_base16(text: &str) -> Result<(BasePalette, Option<String>), String> {
    let mut base: [Option<Color>; 16] = [None; 16];
    let mut extra: [Option<Color>; 8] = [None; 8];
    let mut scheme: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some((key, val)) = line.split_once(':') else { continue; };
        let key = key.trim();
        let val = val.trim();
        if key == "scheme" {
            scheme = Some(val.trim_matches('"').trim_matches('\'').to_string());
            continue;
        }
        // base00..base0F → base[0..16]; base10..base17 → extra[0..8]
        if let Some(hex) = key.strip_prefix("base") {
            if let Ok(idx) = u8::from_str_radix(hex, 16) {
                // tolerate a trailing inline comment: take through the closing quote
                // if quoted, else the first whitespace-delimited token (Codex M1).
                let val = if let Some(rest) = val.strip_prefix('"') {
                    rest.split('"').next().unwrap_or(rest)
                } else if let Some(rest) = val.strip_prefix('\'') {
                    rest.split('\'').next().unwrap_or(rest)
                } else {
                    val.split_whitespace().next().unwrap_or(val)
                };
                if let Some(c) = parse_hex6(val) {
                    if (idx as usize) < 16 { base[idx as usize] = Some(c); }
                    else if (0x10..=0x17).contains(&idx) { extra[(idx - 0x10) as usize] = Some(c); }
                }
            }
        }
    }

    let mut out = [Color::Default; 16];
    for (i, slot) in base.iter().enumerate() {
        out[i] = slot.ok_or_else(|| format!("base16: missing slot base{:02X}", i))?;
    }
    let extra_out = if extra.iter().all(|e| e.is_some()) {
        let mut e = [Color::Default; 8];
        for (i, slot) in extra.iter().enumerate() { e[i] = slot.unwrap(); }
        Some(e)
    } else {
        None
    };
    Ok((BasePalette { base: out, extra: extra_out }, scheme))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_core::theme::Color;

    const GRUVBOX: &str = r##"
scheme: "Gruvbox dark, medium"
author: "Dawid Kurek"
base00: "282828"
base01: "3c3836"
base02: "504945"
base03: "665c54"
base04: "bdae93"
base05: "d5c4a1"
base06: "ebdbb2"
base07: "fbf1c7"
base08: "fb4934"
base09: "#fe8019"
base0A: fabd2f
base0B: "b8bb26"
base0C: "8ec07c"
base0D: "83a598"
base0E: "d3869b"
base0F: "d65d0e"
"##;

    #[test]
    fn parse_hex6_tolerant() {
        assert_eq!(parse_hex6("282828"), Some(Color::Rgb { r:0x28, g:0x28, b:0x28 }));
        assert_eq!(parse_hex6("#fe8019"), Some(Color::Rgb { r:0xfe, g:0x80, b:0x19 }));
        assert_eq!(parse_hex6("\"fabd2f\""), Some(Color::Rgb { r:0xfa, g:0xbd, b:0x2f }));
        assert_eq!(parse_hex6("zzz"), None);
        assert_eq!(parse_hex6("12345"), None); // too short
    }

    #[test]
    fn parse_base16_reads_all_slots_and_scheme() {
        let (p, name) = parse_base16(GRUVBOX).expect("valid base16");
        assert_eq!(name.as_deref(), Some("Gruvbox dark, medium"));
        assert_eq!(p.base[0], Color::Rgb { r:0x28, g:0x28, b:0x28 }); // base00
        assert_eq!(p.base[0xF], Color::Rgb { r:0xd6, g:0x5d, b:0x0e }); // base0F
        assert!(p.extra.is_none()); // base16, not base24
    }

    #[test]
    fn parse_base16_tolerates_inline_comments() {
        let with_comments = GRUVBOX.replace("base00: \"282828\"", "base00: \"282828\" # bg");
        let (p, _n) = parse_base16(&with_comments).expect("valid base16 w/ comment");
        assert_eq!(p.base[0], Color::Rgb { r:0x28, g:0x28, b:0x28 });
    }

    #[test]
    fn parse_base16_missing_slot_errors() {
        let bad = "base00: \"282828\"\nbase05: \"d5c4a1\"\n"; // missing most slots
        assert!(parse_base16(bad).is_err());
    }
}
