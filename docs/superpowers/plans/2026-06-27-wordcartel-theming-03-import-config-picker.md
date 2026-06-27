# Theming Plan ③ — Import, Config & Switching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make theming user-selectable — base16/24 palette import, a `[theme]` config section (name/file/depth/styles), env-based color-depth detection, and a theme-picker overlay with relayout-on-switch — so a writer can load Tokyo Night (or any base16 theme) and pick themes at runtime.

**Architecture:** Functional-core/imperative-shell. `wordcartel-core` (pure, no IO) gains `BasePalette` + `from_base16` + a per-element style-override mutator + a config-key→`SemanticElement` map. The shell (`wordcartel`) gains a hand-rolled base16 file parser (NO YAML crate), a `[theme]` config layer that threads through the existing `(Config, Vec<String>)` warning stream, env depth detection, a `resolve_theme` that ties it together at startup, and a `ThemePicker` overlay mirroring the existing command palette. Render already reads `editor.theme`/`editor.depth` live each frame; a switch sets `editor.theme` and calls `derive::rebuild` (because `heading_level_glyph` is a layout input).

**Tech Stack:** Rust, ratatui 0.30, toml 0.8 (already a dep), serde. No new dependencies.

## Global Constraints

- **NO YAML / no new dependency for base16:** base16 files are parsed by a small hand-rolled shell parser (`serde_yml` is deprecated / RUSTSEC-2025-0068). Spec §3.3.
- **Core stays IO-free:** all file/env reading lives in the shell. `wordcartel-core` must not import `std::fs`/`std::env`/`std::io`. Spec §3.2.
- **Depth precedence (locked):** `NO_COLOR` > explicit `[theme] depth` > detection. Effective depth stored separately; when `None`, the picker can't re-enable color. Spec §3.3.
- **Cue mode forces the heading glyph:** when effective `Depth == None` OR `theme.monochrome`, `heading_level_glyph` is forced **on** regardless of theme/config. Spec §4.
- **Resolution/relayout happen once at startup and on switch — never per frame.** Spec §3.6.
- **Configs load unchanged:** every new config struct uses `#[serde(default)]` so pre-theming configs parse with no `[theme]` section. Spec §5.
- **Warnings append to the existing startup stream:** `config::load` returns `(Config, Vec<String>)`; theme resolution warnings join that same `warns` vec (app.rs ~1237/1348). Spec §3.3.
- **Discriminated source:** within the config layers, a layer setting `name` clears accumulated `file` (and vice-versa); both in one layer → `file` wins + warning. `file` paths expand `~` and resolve relative to the **declaring** config file. Spec §5.

**Built-in theme names (verbatim, from `Theme::builtin_names()`):** `default`, `no-color`, `tokyo-night`, `phosphor-green`, `phosphor-green-flat`, `phosphor-amber`, `phosphor-amber-flat`, `phosphor-red`, `phosphor-red-flat`, `phosphor-blue`, `phosphor-blue-flat`, `phosphor-purple`, `phosphor-purple-flat`.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `wordcartel-core/src/theme.rs` | `BasePalette`, `from_base16`, `override_face`, `element_from_key` | 1 |
| `wordcartel/src/base16.rs` (NEW) | hand-rolled base16/24 file parser → `BasePalette`; `parse_hex6` | 2 |
| `wordcartel/src/config.rs` | `RawTheme`/`RawFace`, `ThemeConfig`, layered `[theme]` parse + discrimination + path resolution | 3 |
| `wordcartel/src/theme_resolve.rs` (NEW) | `detect_depth`, `effective_depth`, `parse_depth`, `resolve_theme`, `RawFace→Face`, cue-mode forcing | 4, 5 |
| `wordcartel/src/compose.rs` | `face_to_ratatui` suppress color at `Depth::None`; `base_canvas` helper | 4, 8 |
| `wordcartel/src/lib.rs` | module declarations (`pub mod base16/theme_resolve/theme_picker;`) | 2, 4, 7 |
| `wordcartel/src/app.rs` | startup wiring: build env snapshot, call `resolve_theme`, seed editor, append warnings | 6 |
| `wordcartel/src/theme_picker.rs` (NEW) | `ThemePicker` overlay state + row build | 7 |
| `wordcartel/src/editor.rs` | `theme_picker` field + `open_theme_picker()` (XOR) | 7 |
| `wordcartel/src/registry.rs` | register the `theme` command | 7 |
| `wordcartel/src/render.rs` | paint the theme-picker overlay | 7 |

---

## Task 1: Core — `BasePalette`, `from_base16`, `override_face`, `element_from_key`

**Files:**
- Modify: `wordcartel-core/src/theme.rs` (add after `tokyo_night()`, ~line 285; and methods on `impl Theme`)
- Test: same file `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `pub struct BasePalette { pub base: [Color; 16], pub extra: Option<[Color; 8]> }`
  - `pub fn from_base16(name: &str, p: BasePalette) -> Theme`
  - `impl Theme { pub fn override_face(&mut self, el: SemanticElement, patch: Face) }` — sets each `Some` field of `patch` onto the stored face for `el` (per-field; `None` leaves the existing value).
  - `pub fn element_from_key(key: &str) -> Option<SemanticElement>` — snake-case config key → element.
- Consumes (existing, verbatim): `Color` (`Rgb{r,g,b}`, named, `Indexed(u8)`, `Default`), `Face` (all-`Option` fields), `Theme`, `ThemeFaces` (private named-field struct), `SemanticElement` (26 variants), `theme.face(el)`.

- [ ] **Step 1: Write the failing tests** (in `theme.rs` `#[cfg(test)]`)

```rust
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
```

- [ ] **Step 2: Run — fails to compile** (`BasePalette`/`from_base16`/`override_face`/`element_from_key` undefined).
Run: `cargo test -p wordcartel-core from_base16 override_face element_from_key`

- [ ] **Step 3: Implement `BasePalette` + `from_base16`** (add after `tokyo_night()`, ~line 285)

```rust
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
        heading_level_glyph: false,
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
```

- [ ] **Step 4: Implement `override_face` + `element_from_key`.** Add `override_face` to the existing `impl Theme` block (the one with `face`), and `element_from_key` as a free fn near `face`.

```rust
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
            SearchMatch => &mut self.faces.search_match, SearchCurrent => &mut self.faces.search_current,
            DiagSpelling => &mut self.faces.diag_spelling, DiagGrammar => &mut self.faces.diag_grammar,
            FocusDim => &mut self.faces.focus_dim, FoldMarker => &mut self.faces.fold_marker,
            WrapGuide => &mut self.faces.wrap_guide,
            Chrome => &mut self.faces.chrome, ChromeReverse => &mut self.faces.chrome_reverse,
            ChromeSelected => &mut self.faces.chrome_selected, ChromeMuted => &mut self.faces.chrome_muted,
        }
    }
```

```rust
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
        "selection" => Selection, "search_match" => SearchMatch, "search_current" => SearchCurrent,
        "diag_spelling" => DiagSpelling, "diag_grammar" => DiagGrammar, "focus_dim" => FocusDim,
        "fold_marker" => FoldMarker, "wrap_guide" => WrapGuide,
        "chrome" => Chrome, "chrome_reverse" => ChromeReverse,
        "chrome_selected" => ChromeSelected, "chrome_muted" => ChromeMuted,
        _ => return None,
    })
}
```

- [ ] **Step 5: Run** `cargo test -p wordcartel-core from_base16 override_face element_from_key` — PASS. Then `cargo test -p wordcartel-core` — full core green (no regression; the existing `modface`/`Face` totality tests still hold).

- [ ] **Step 6: Commit** `feat(theme): core base16 import (BasePalette/from_base16) + per-field override + element key map`

---

## Task 2: Shell — hand-rolled base16/24 file parser

**Files:**
- Create: `wordcartel/src/base16.rs`
- Modify: `wordcartel/src/lib.rs` (add `pub mod base16;` — the module list is in **lib.rs** at ~line 3, NOT main.rs; Codex I7)
- Test: `wordcartel/src/base16.rs` `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `pub fn parse_hex6(s: &str) -> Option<wordcartel_core::theme::Color>` — `"#rrggbb"`/`"rrggbb"`/quoted → `Color::Rgb`.
  - `pub fn parse_base16(text: &str) -> Result<(BasePalette, Option<String>), String>` — flat `key: "rrggbb"` map → `(palette, scheme_name)`. `Err` if any of `base00..base0F` missing.
- Consumes: `wordcartel_core::theme::{BasePalette, Color}` (Task 1).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_core::theme::Color;

    const GRUVBOX: &str = r#"
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
"#;

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
```

- [ ] **Step 2: Run — fails** (no `base16` module). `cargo test -p wordcartel base16`

- [ ] **Step 3: Implement** `wordcartel/src/base16.rs`

```rust
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
```

- [ ] **Step 4: Run** `cargo test -p wordcartel base16` — PASS. Then `cargo build -p wordcartel` — compiles (module wired in).

- [ ] **Step 5: Commit** `feat(theme): hand-rolled base16/24 file parser (no YAML dep)`

---

## Task 3: Shell — `[theme]` config (RawTheme/RawFace + layered discrimination + path resolution)

**Files:**
- Modify: `wordcartel/src/config.rs` (add structs near the other `Raw*` ~line 125; add a `ThemeConfig` field to `Config` ~line 33; extend `load()` ~line 215)
- Test: `wordcartel/src/config.rs` `#[cfg(test)]`

**Interfaces:**
- Produces (on `Config`): `pub theme: ThemeConfig` where
  ```rust
  #[derive(Debug, Default, Clone)]
  pub struct ThemeConfig {
      pub name: Option<String>,
      pub file: Option<PathBuf>,           // ~-expanded, resolved relative to declaring config
      pub depth: Option<String>,           // "truecolor"|"256"|"16"|"none"
      pub heading_level_glyph: Option<bool>,
      pub styles: BTreeMap<String, RawFace>,
  }
  #[derive(Debug, Default, Clone, Deserialize, PartialEq)]
  #[serde(default)]
  pub struct RawFace {
      pub fg: Option<String>, pub bg: Option<String>, pub underline_color: Option<String>,
      pub bold: Option<bool>, pub italic: Option<bool>, pub underline: Option<bool>,
      pub strike: Option<bool>, pub reverse: Option<bool>, pub dim: Option<bool>,
  }
  ```
- Consumes: the existing `load()` per-layer fold pattern, `config_layer_paths`, the `~` expansion pattern at config.rs:286-300, `toml::from_str`.

- [ ] **Step 1: Write the failing tests** (mirror the existing config tests; each builds a temp file and calls `load`)

```rust
    #[test]
    fn theme_name_parses() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.toml");
        std::fs::write(&p, "[theme]\nname = \"tokyo-night\"\n").unwrap();
        let (cfg, warns) = load(&[p]);
        assert_eq!(cfg.theme.name.as_deref(), Some("tokyo-night"));
        assert!(cfg.theme.file.is_none());
        assert!(warns.is_empty());
    }

    #[test]
    fn theme_file_resolves_relative_to_declaring_config() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.toml");
        std::fs::write(&p, "[theme]\nfile = \"palettes/gruvbox.yaml\"\n").unwrap();
        let (cfg, _w) = load(&[p]);
        // resolved against the config file's directory, not CWD
        assert_eq!(cfg.theme.file, Some(dir.path().join("palettes/gruvbox.yaml")));
        assert!(cfg.theme.name.is_none());
    }

    #[test]
    fn theme_name_then_file_across_layers_is_discriminated() {
        let dir = tempfile::tempdir().unwrap();
        let lo = dir.path().join("lo.toml");
        let hi = dir.path().join("hi.toml");
        std::fs::write(&lo, "[theme]\nname = \"tokyo-night\"\n").unwrap();
        std::fs::write(&hi, "[theme]\nfile = \"g.yaml\"\n").unwrap();
        let (cfg, _w) = load(&[lo, hi]); // hi overrides
        assert!(cfg.theme.name.is_none(), "a later `file` clears an earlier `name`");
        assert_eq!(cfg.theme.file, Some(dir.path().join("g.yaml")));
    }

    #[test]
    fn theme_name_and_file_same_layer_file_wins_with_warning() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.toml");
        std::fs::write(&p, "[theme]\nname = \"tokyo-night\"\nfile = \"g.yaml\"\n").unwrap();
        let (cfg, warns) = load(&[p]);
        assert!(cfg.theme.name.is_none());
        assert!(cfg.theme.file.is_some());
        assert!(warns.iter().any(|w| w.contains("name") && w.contains("file")));
    }

    #[test]
    fn theme_styles_accumulate_across_layers() {
        let dir = tempfile::tempdir().unwrap();
        let lo = dir.path().join("lo.toml");
        let hi = dir.path().join("hi.toml");
        std::fs::write(&lo, "[theme.styles]\nheading1 = { fg = \"#bb9af7\", bold = true }\n").unwrap();
        std::fs::write(&hi, "[theme.styles]\nselection = { bg = \"#283457\" }\n").unwrap();
        let (cfg, _w) = load(&[lo, hi]);
        assert!(cfg.theme.styles.contains_key("heading1"));
        assert!(cfg.theme.styles.contains_key("selection"));
    }

    #[test]
    fn pre_theming_config_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.toml");
        std::fs::write(&p, "[view]\ntypewriter = true\n").unwrap(); // no [theme] at all
        let (cfg, warns) = load(&[p]);
        assert!(cfg.view.typewriter);
        assert!(cfg.theme.name.is_none() && cfg.theme.file.is_none());
        assert!(warns.is_empty());
    }
```

> If `tempfile` is not already a dev-dependency, check `wordcartel/Cargo.toml` `[dev-dependencies]`; the existing config tests already write temp files, so mirror exactly how they create paths (they may use `std::env::temp_dir()` + a unique name rather than `tempfile`). Match the existing test helper.

- [ ] **Step 2: Run — fails** (no `theme` field / `RawFace`). `cargo test -p wordcartel theme_name_parses theme_file_resolves theme_name_then_file theme_name_and_file theme_styles pre_theming`

- [ ] **Step 3: Add the structs.** In `config.rs`: add `RawFace` + `RawTheme` (deserialized) near the other `Raw*` structs (~line 125), `ThemeConfig` (resolved) near `Config` (~line 33), and a `pub theme: ThemeConfig` field on `Config`.

```rust
// ---- resolved (on Config) ----
#[derive(Debug, Default, Clone)]
pub struct ThemeConfig {
    pub name: Option<String>,
    pub file: Option<PathBuf>,
    pub depth: Option<String>,
    pub heading_level_glyph: Option<bool>,
    pub styles: BTreeMap<String, RawFace>,
}

// ---- deserialized (per layer) ----
#[derive(Debug, Default, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct RawFace {
    pub fg: Option<String>, pub bg: Option<String>, pub underline_color: Option<String>,
    pub bold: Option<bool>, pub italic: Option<bool>, pub underline: Option<bool>,
    pub strike: Option<bool>, pub reverse: Option<bool>, pub dim: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawTheme {
    name: Option<String>,
    file: Option<String>,
    depth: Option<String>,
    heading_level_glyph: Option<bool>,
    styles: BTreeMap<String, RawFace>,
}
```
Add `theme: ThemeConfig` to `pub struct Config` (it derives `Default`, so the new field needs no manual default), and `theme: RawTheme` to the private `RawConfig` (~line 125).

- [ ] **Step 4: Extend `load()`** — fold each layer's `RawTheme` into `cfg.theme` with discrimination + path resolution. Inside the per-layer loop in `load()` (after the existing keymap/view folds), `p` is the current layer's config `PathBuf`:

```rust
        // ---- [theme] (discriminated source; file resolved vs the declaring config) ----
        let rt = raw.theme;
        let layer_dir = p.parent().unwrap_or_else(|| std::path::Path::new("."));
        let raw_file = rt.file.clone();
        // Resolve a layer's `file` (~ expand + relative-to-this-config) if present.
        let resolved_file = raw_file.as_ref().map(|s| {
            if s == "~" {
                dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"))
            } else if let Some(rest) = s.strip_prefix("~/") {
                dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(s))
            } else {
                let pb = PathBuf::from(s);
                if pb.is_absolute() { pb } else { layer_dir.join(pb) }
            }
        });
        match (rt.name.clone(), resolved_file) {
            (Some(_), Some(f)) => {
                warns.push(format!(
                    "theme: both `name` and `file` set in {} — `file` wins", p.display()));
                cfg.theme.name = None;
                cfg.theme.file = Some(f);
            }
            (Some(n), None) => { cfg.theme.name = Some(n); cfg.theme.file = None; } // name clears file
            (None, Some(f)) => { cfg.theme.file = Some(f); cfg.theme.name = None; } // file clears name
            (None, None) => {} // neither set this layer → inherit accumulated
        }
        if let Some(d) = rt.depth { cfg.theme.depth = Some(d); }
        if let Some(h) = rt.heading_level_glyph { cfg.theme.heading_level_glyph = Some(h); }
        for (k, v) in rt.styles { cfg.theme.styles.insert(k, v); } // accumulate across layers
```

- [ ] **Step 5: Run** `cargo test -p wordcartel config::` (or the specific test names) — PASS. Then `cargo test -p wordcartel --lib` — no regression (pre-theming configs unaffected).

- [ ] **Step 6: Commit** `feat(theme): [theme] config layer — name/file/depth/styles, discriminated source, relative paths`

---

## Task 4: Shell — depth detection (`detect_depth` / `effective_depth` / `parse_depth`)

**Files:**
- Create: `wordcartel/src/theme_resolve.rs` (this task adds the depth fns; Task 5 adds `resolve_theme` to the same file)
- Modify: `wordcartel/src/lib.rs` (add `pub mod theme_resolve;` — the module list is in **lib.rs**, NOT main.rs — Codex I7)
- Modify: `wordcartel/src/compose.rs` (`face_to_ratatui` — suppress color at `Depth::None`, Codex C1)
- Test: `wordcartel/src/theme_resolve.rs` + `wordcartel/src/compose.rs` `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `pub fn detect_depth(no_color: bool, colorterm: Option<&str>, term: Option<&str>) -> Depth`
  - `pub fn parse_depth(s: &str) -> Option<Depth>` — `"truecolor"|"256"|"16"|"none"` (case-insensitive).
  - `pub fn effective_depth(no_color: bool, explicit: Option<Depth>, detected: Depth) -> Depth` — precedence `NO_COLOR > explicit > detected`.
- Consumes: `wordcartel_core::theme::Depth`.

- [ ] **Step 1: Write the failing tests**

```rust
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
```

- [ ] **Step 2: Run — fails** (no module). `cargo test -p wordcartel theme_resolve`

- [ ] **Step 3: Implement** `wordcartel/src/theme_resolve.rs` (depth section)

```rust
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
```

- [ ] **Step 4: Run** `cargo test -p wordcartel theme_resolve` — PASS.

- [ ] **Step 4b: CRITICAL (Codex C1) — `face_to_ratatui` must suppress color at `Depth::None`.** Today `compose::face_to_ratatui` (compose.rs) applies `fg`/`bg`/`underline_color` UNCONDITIONALLY; `quantize(_, Depth::None)` is identity. So at `Depth::None` a colored theme renders RGB color — violating "when None, the picker can't re-enable color" (§3.3) and the cue-mode predicate (§4). This never manifested because `depth` was hardcoded `Truecolor`; Task 4 makes `None` reachable (NO_COLOR), so this MUST be fixed now. Add a failing test in `compose.rs`:
```rust
    #[test]
    fn depth_none_suppresses_color_keeps_modifiers() {
        use wordcartel_core::theme::{Face, Color, Depth};
        let f = Face { fg: Some(Color::Rgb { r:0x7a, g:0xa2, b:0xf7 }), bold: Some(true), ..Face::default() };
        let s = face_to_ratatui(&f, Depth::None);
        assert!(s.fg.is_none(), "no color at Depth::None");
        assert!(s.add_modifier.contains(ratatui::style::Modifier::BOLD), "modifiers survive");
    }
```
Then guard the three color lines (do NOT touch the modifiers):
```rust
pub fn face_to_ratatui(face: &Face, depth: Depth) -> RStyle {
    let mut s = RStyle::default();
    if depth != Depth::None {                              // <-- cue mode (None) carries NO color
        if let Some(c) = face.fg { s = s.fg(to_rcolor(c, depth)); }
        if let Some(c) = face.bg { s = s.bg(to_rcolor(c, depth)); }
        if let Some(c) = face.underline_color { s = s.underline_color(to_rcolor(c, depth)); }
    }
    // …modifiers unchanged (bold/italic/underline/strike/reverse/dim)…
}
```
Run `cargo test -p wordcartel compose:: depth_none` + the existing compose/render tests (the Default theme at `Truecolor` is unaffected — the guard only fires at `None`). NOTE: a render golden that previously rendered at a non-`None` depth is unchanged; only `Depth::None` paths change (they should already be monochrome since `no_color()` has all-`Default` faces — this guard makes color-bearing themes/overrides also monochrome at `None`).

- [ ] **Step 5: Commit** `feat(theme): env color-depth detection + precedence; face_to_ratatui suppresses color at Depth::None`

---

## Task 5: Shell — `resolve_theme` (ties import + config + depth together)

**Files:**
- Modify: `wordcartel/src/theme_resolve.rs` (add `resolve_theme`, `EnvSnapshot`, `ResolvedTheme`, `raw_face_to_face`, `apply_cue_mode_glyph`)
- Test: same file

**Interfaces:**
- Consumes: `config::ThemeConfig`, `config::RawFace`, Task 1 (`from_base16`, `override_face`, `element_from_key`), Task 2 (`base16::parse_base16`, `base16::parse_hex6`), Task 4 (`detect_depth`/`parse_depth`/`effective_depth`), `theme::{builtin, default, no_color, Theme, Depth, Face, Color}`.
- Produces:
  ```rust
  pub struct EnvSnapshot { pub no_color: bool, pub colorterm: Option<String>, pub term: Option<String> }
  impl EnvSnapshot { pub fn from_env() -> EnvSnapshot }   // reads std::env (the only IO entry)
  pub struct ResolvedTheme { pub theme: Theme, pub depth: Depth, pub warnings: Vec<String> }
  pub fn resolve_theme(tc: &config::ThemeConfig, env: &EnvSnapshot) -> ResolvedTheme
  ```

- [ ] **Step 1: Write the failing tests**

```rust
    use crate::config::{ThemeConfig, RawFace};
    use wordcartel_core::theme::{Depth, SemanticElement, Color};

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
```

- [ ] **Step 2: Run — fails** (no `resolve_theme`). `cargo test -p wordcartel resolve_`

- [ ] **Step 3: Implement** (append to `theme_resolve.rs`)

```rust
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

    // Base theme: cue None → no_color; else file > name > default.
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
```

- [ ] **Step 4: Run** `cargo test -p wordcartel resolve_` — PASS. Then `cargo test -p wordcartel --lib` — green.

- [ ] **Step 5: Commit** `feat(theme): resolve_theme — depth→theme(file/name/no_color)+styles+cue-mode glyph`

---

## Task 6: Shell — startup wiring (seed `editor.theme`/`editor.depth` from config)

**Files:**
- Modify: `wordcartel/src/app.rs` (the `run()` startup, after `let mut editor = editor;` ~line 1282, parallel to the `editor.view_opts = cfg.view.clone()` block at ~1286-1288)
- Test: an integration-style assertion in `app.rs` tests if one exists, else a focused `theme_resolve` test already covers the logic; this task is the wiring.

**Interfaces:**
- Consumes: `config::load` output `cfg.theme` (Task 3), `theme_resolve::{EnvSnapshot, resolve_theme}` (Task 5), the existing `warns` vec, `editor.theme`/`editor.depth` fields (editor.rs:262-263).

- [ ] **Step 1: Wire it.** In `app.rs::run`, after the editor is constructed and the other config seeds (~line 1288), add:

```rust
    // Resolve and seed the active theme + color depth (once, at startup — §3.6).
    let env = crate::theme_resolve::EnvSnapshot::from_env();
    let resolved = crate::theme_resolve::resolve_theme(&cfg.theme, &env);
    editor.theme = resolved.theme;
    editor.depth = resolved.depth;
    editor.heading_glyph_cfg = cfg.theme.heading_level_glyph;  // for runtime picker switches (Codex I4)
    warns.extend(resolved.warnings);   // join the existing startup warning stream
```
(`warns` is `let (cfg, mut warns) = config::load(&paths);` at ~1237, already mutable; keymap warnings already `warns.append` at ~1348 — theme warnings join the same stream and surface via the existing startup-warning UI.)

- [ ] **Step 2: Run** `cargo build -p wordcartel` — compiles. Then `cargo test -p wordcartel --lib` — green (no regression).

- [ ] **Step 3: Manual smoke (document, don't automate here):** `TERM=xterm-256color` with a `[theme] name = "tokyo-night"` config → editor starts themed; `NO_COLOR=1` → monochrome with heading glyphs. (The picker task adds the runtime path; this is the startup path.)

- [ ] **Step 4: Commit** `feat(theme): seed editor theme+depth from config at startup (warnings → startup stream)`

---

## Task 7: Shell — theme picker overlay + `theme` command + relayout-on-switch

**Files:**
- Create: `wordcartel/src/theme_picker.rs` (`ThemePicker` state + row build)
- Modify: `wordcartel/src/lib.rs` (add `pub mod theme_picker;` — module list is in lib.rs, Codex I7)
- Modify: `wordcartel/src/editor.rs` (add `pub theme_picker: Option<...>` + `pub heading_glyph_cfg: Option<bool>` fields ~line 218; `open_theme_picker()` mirroring `open_palette` ~line 332; add `theme_picker = None` to EVERY overlay-opening site — see CRITICAL note below)
- Modify: `wordcartel/src/registry.rs` (register `theme` command)
- Modify: `wordcartel/src/app.rs` (key-handling block mirroring the palette block ~line 753-833)
- Modify: `wordcartel/src/render.rs` (paint the overlay, mirroring the palette render)
- Test: `wordcartel/src/theme_picker.rs` + a render test

**Interfaces:**
- Produces:
  - `pub struct ThemePicker { pub query: String, pub selected: usize, pub rows: Vec<String>, pub original: Theme }`
  - `pub fn rebuild_rows(tp: &mut ThemePicker)` — filter `Theme::builtin_names()` (associated method, Codex C3) by `query` (substring, case-insensitive); `selected` clamped.
  - `editor.open_theme_picker()` (XOR), `editor.apply_theme(theme: Theme)` (sets `theme`, re-forces cue-mode glyph for the current depth, `derive::rebuild`, `nav::ensure_visible`).
- Consumes: `theme::{builtin, builtin_names, Theme}`, `derive::rebuild`, `nav::ensure_visible`, the palette key-handling + render patterns.

- [ ] **Step 1: Write the failing tests** (`theme_picker.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    #[test]
    fn rebuild_rows_filters_builtins() {
        let mut tp = ThemePicker { query: String::new(), selected: 0, rows: vec![],
            original: wordcartel_core::theme::default() };
        rebuild_rows(&mut tp);
        assert!(tp.rows.iter().any(|r| r == "tokyo-night"));
        assert!(tp.rows.len() >= 13);
        tp.query = "phosphor-amber".into();
        rebuild_rows(&mut tp);
        assert!(tp.rows.iter().all(|r| r.contains("phosphor-amber")));
        assert!(tp.rows.contains(&"phosphor-amber".to_string()));
        assert!(tp.rows.contains(&"phosphor-amber-flat".to_string()));
    }

    #[test]
    fn apply_theme_swaps_and_relayouts() {
        let mut ed = Editor::new_from_text("# Heading\n\n> quote\n", None, (40, 12));
        crate::derive::rebuild(&mut ed);
        let nc = wordcartel_core::theme::no_color(); // monochrome → cue mode forces heading glyph
        ed.depth = wordcartel_core::theme::Depth::None;
        ed.apply_theme(nc);
        assert_eq!(ed.theme.name, "no-color");
        assert!(ed.theme.heading_level_glyph, "cue mode forces heading glyph on apply");
        // layout cache was rebuilt (line_layouts repopulated for the visible range)
        assert!(!ed.active().view.line_layouts.is_empty());
    }

    #[test]
    fn open_theme_picker_enforces_xor() {
        let mut ed = Editor::new_from_text("x\n", None, (40, 12));
        ed.open_palette();
        ed.open_theme_picker();
        assert!(ed.theme_picker.is_some());
        assert!(ed.palette.is_none(), "opening the theme picker closes the palette (XOR)");
    }
}
```

- [ ] **Step 2: Run — fails** (no `ThemePicker`/`apply_theme`/`open_theme_picker`). `cargo test -p wordcartel theme_picker`

- [ ] **Step 3: Implement `theme_picker.rs`**

```rust
//! Theme-picker overlay: lists built-in theme names, filters by query, applies
//! (with live preview) on selection. Mirrors the command palette (palette.rs).

use wordcartel_core::theme::{self, Theme};

#[derive(Debug, Clone)]
pub struct ThemePicker {
    pub query: String,
    pub selected: usize,
    pub rows: Vec<String>,
    /// The theme active when the picker opened — restored on Esc (preview cancel).
    pub original: Theme,
}

/// Rebuild rows from the built-in theme list, filtered by `query` (case-insensitive
/// substring). Empty query → all built-ins in registration order. `selected` clamped.
pub fn rebuild_rows(tp: &mut ThemePicker) {
    let q = tp.query.to_ascii_lowercase();
    tp.rows = Theme::builtin_names().iter()   // associated method (Codex C3)
        .filter(|n| q.is_empty() || n.to_ascii_lowercase().contains(&q))
        .map(|n| n.to_string())
        .collect();
    if tp.selected >= tp.rows.len() { tp.selected = tp.rows.len().saturating_sub(1); }
}
```

- [ ] **Step 4: Implement editor methods** (`editor.rs`). Add two fields (~line 218, beside `outline`/`theme`/`depth`):

```rust
    pub theme_picker: Option<crate::theme_picker::ThemePicker>,
    /// The config's `[theme] heading_level_glyph` override (Codex I4): apply_theme
    /// uses it for non-cue themes so a runtime switch preserves the user's setting.
    pub heading_glyph_cfg: Option<bool>,
```
Initialize `theme_picker: None,` and `heading_glyph_cfg: None,` in `new_from_text` (beside the other overlay inits; Task 6 seeds `heading_glyph_cfg` from config at startup). Add `open_theme_picker` (mirror `open_palette` exactly — clear ALL other overlays):

```rust
    /// Open the theme picker, enforcing the single-overlay XOR invariant.
    pub fn open_theme_picker(&mut self) {
        self.prompt = None; self.minibuffer = None; self.menu = None;
        self.pending_keys.clear(); self.pending_mark = None;
        self.search = None; self.diag = None; self.outline = None; self.palette = None;
        self.theme_picker = Some(crate::theme_picker::ThemePicker {
            query: String::new(), selected: 0, rows: Vec::new(),
            original: self.theme.clone(),
        });
        if let Some(tp) = self.theme_picker.as_mut() { crate::theme_picker::rebuild_rows(tp); }
    }

    /// Apply a theme: swap, re-derive the heading-glyph flag (cue mode forces ON;
    /// else the CONFIG override `heading_glyph_cfg` wins, else the theme's own flag —
    /// Codex I4, so a picker switch doesn't drop a configured override), relayout
    /// (heading_level_glyph is a layout input — §3.6/§3.7), keep caret visible.
    pub fn apply_theme(&mut self, mut theme: wordcartel_core::theme::Theme) {
        let cue = self.depth == wordcartel_core::theme::Depth::None || theme.monochrome;
        theme.heading_level_glyph = if cue { true }
            else { self.heading_glyph_cfg.unwrap_or(theme.heading_level_glyph) };
        self.theme = theme;
        crate::derive::rebuild(self);
        crate::nav::ensure_visible(self);
    }
```
**CRITICAL (Codex I5) — the invariant must hold BOTH directions; add `theme_picker = None` to EVERY site that opens a different overlay, not just the `open_*` methods.** The real sites (verified):
   - `editor.rs`: `open_minibuffer` (~:295), `open_prompt` (~:316), `open_palette` (~:332), `open_search` (~:348), `open_diag` (~:362), `open_outline` (~:372) — add `self.theme_picker = None;` to each (beside the existing `self.palette = None;`).
   - `registry.rs` (~:177): the **menu** command opens the menu directly (not via an `open_*` method) — add `c.editor.theme_picker = None;` there.
   - `app.rs::dispatch_overlay_command` (~:441): currently clears only `palette`/`menu` — add `editor.theme_picker = None;`.
   A missed site means another overlay can open OVER the theme picker (broken XOR). Add the `open_theme_picker` XOR-test (Step 1) plus one asserting `open_outline()` clears `theme_picker`.

- [ ] **Step 5: Register the `theme` command** (`registry.rs`, beside the `palette` registration ~line 173):

```rust
    r.register("theme", "Select Theme\u{2026}", Some(MenuCategory::View), |c| {
        c.editor.open_theme_picker();
        CommandResult::Handled
    });
```

- [ ] **Step 6: Key handling** (`app.rs`). Add a block mirroring the palette block (~line 753-833), placed beside it. Live-preview on Up/Down (apply as you move); Enter keeps; Esc restores `original`:

```rust
    if editor.theme_picker.is_some() {
        // Paste intercept FIRST (mirror the palette, app.rs:753) — else paste leaks
        // into the document while the picker is open (Codex I6).
        if let Msg::Input(Event::Paste(text)) = &msg {
            if let Some(tp) = editor.theme_picker.as_mut() {
                tp.query.push_str(text);
                crate::theme_picker::rebuild_rows(tp);
            }
            preview_selected_theme(editor);
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::KeyCode;
                match k.code {
                    KeyCode::Esc => {
                        // cancel preview → restore the theme active when we opened.
                        if let Some(tp) = editor.theme_picker.take() { editor.apply_theme(tp.original); }
                    }
                    KeyCode::Enter => { editor.theme_picker = None; } // keep current preview
                    KeyCode::Up => {
                        if let Some(tp) = editor.theme_picker.as_mut() { tp.selected = tp.selected.saturating_sub(1); }
                        preview_selected_theme(editor);
                    }
                    KeyCode::Down => {
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            let max = tp.rows.len().saturating_sub(1);
                            tp.selected = (tp.selected + 1).min(max);
                        }
                        preview_selected_theme(editor);
                    }
                    KeyCode::Backspace => {
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            tp.query.pop();
                            crate::theme_picker::rebuild_rows(tp);
                        }
                        preview_selected_theme(editor);
                    }
                    KeyCode::Char(c)
                        if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                            && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            tp.query.push(c);
                            crate::theme_picker::rebuild_rows(tp);
                        }
                        preview_selected_theme(editor);
                    }
                    _ => {}
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
    }
```
Add the helper near the other `app.rs` free fns:
```rust
/// Apply the theme-picker's currently-selected built-in as a live preview.
fn preview_selected_theme(editor: &mut crate::editor::Editor) {
    let name = editor.theme_picker.as_ref().and_then(|tp| tp.rows.get(tp.selected).cloned());
    if let Some(name) = name {
        if let Some(theme) = wordcartel_core::theme::Theme::builtin(&name) { editor.apply_theme(theme); }
    }
}
```
> Match the REAL local names in `app.rs`'s message loop (`msg`, `editor`, `ex`, the early-return shape) — mirror the palette block verbatim for `Paste`/drain/return handling. The palette block is the template.

- [ ] **Step 7: Render the overlay** (`render.rs`). Mirror the palette overlay render (grep `palette` in render.rs for the exact widget/rect pattern): a centered list of `tp.rows`, the `selected` row highlighted via `compose(&editor.theme, editor.depth, &[SE::ChromeReverse])`, the query line via `SE::ChromeMuted`, frame via `SE::Chrome`. Add the render test **inside `render.rs`'s `#[cfg(test)]` module** (the `render_to_buffer`/`row_string` helpers are private there — Codex M2; the logic tests in Step 1 live in `theme_picker.rs` and don't need them):

```rust
    #[test]
    fn theme_picker_paints_rows_and_selection() {
        let mut ed = Editor::new_from_text("x\n", None, (60, 16));
        ed.open_theme_picker();
        let buf = render_to_buffer(&mut ed, 60, 16);
        let text: String = (0..16).map(|r| row_string(&buf, r)).collect();
        assert!(text.contains("tokyo-night"), "picker lists built-in themes");
    }
```

- [ ] **Step 8: Run** `cargo test -p wordcartel theme_picker theme_picker_paints` + FULL `cargo test -p wordcartel --lib` + `cargo test -p wordcartel-core`. ALL green. Fix any other `open_*` XOR site flagged (compile-driven if a test asserts XOR).

- [ ] **Step 9: Commit** `feat(theme): theme-picker overlay + `theme` command + live preview + relayout-on-switch`

---

## Task 8: Shell — source-mode base canvas (§3.5; phosphor/base16 tint the source text)

**Files:**
- Modify: `wordcartel/src/render.rs` (the source-mode style path — `compose([Text])` sites at ~:391 and ~:462)
- Modify: `wordcartel/src/compose.rs` (add a `base_canvas` helper)
- Test: `wordcartel/src/render.rs` `#[cfg(test)]`

**Why (Codex I8 + the user's explicit requirement):** in Source modes (Highlighted/Plain) the renderer applies `compose([SemanticElement::Text])` only — `Text` is `Face::default()` for every theme, so the source canvas is the **terminal default**, ignoring `theme.base_fg`/`base_bg`. Spec §3.5: source modes apply the **base canvas (`base_fg`/`base_bg`) + overlays** so the **phosphor base hue tints the source** (the authentic green/amber-monitor look the user asked for) while **Default** (base = `Default`) leaves the terminal untouched.

**Interfaces:**
- Produces: `compose::base_canvas(theme: &Theme, depth: Depth) -> RStyle` — `face_to_ratatui` of a face carrying only `base_fg`/`base_bg`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn source_mode_tints_canvas_for_phosphor_but_not_default() {
        use wordcartel_core::theme::{Theme, Depth};
        // Phosphor-amber: source cells carry the amber base bg/fg.
        let mut ed = Editor::new_from_text("# raw markdown\n", None, (40, 6));
        ed.theme = Theme::builtin("phosphor-amber").unwrap();
        ed.depth = Depth::Truecolor;
        ed.active_mut().view.mode = crate::editor::RenderMode::SourcePlain;
        crate::derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);
        let cell = &buf[(0u16, 0u16)];
        assert!(cell.style().bg.is_some(), "phosphor source canvas sets a bg");
        // Default theme: source canvas stays terminal-default (no themed bg).
        let mut ed2 = Editor::new_from_text("# raw markdown\n", None, (40, 6));
        ed2.active_mut().view.mode = crate::editor::RenderMode::SourcePlain;
        crate::derive::rebuild(&mut ed2);
        let buf2 = render_to_buffer(&mut ed2, 40, 6);
        let bg = buf2[(0u16, 0u16)].style().bg;
        assert!(bg.is_none() || bg == Some(ratatui::style::Color::Reset), "Default source = terminal default");
    }
```

- [ ] **Step 2: Run — fails** (source canvas is currently unthemed for phosphor). `cargo test -p wordcartel source_mode_tints`

- [ ] **Step 3: Add the helper** (`compose.rs`):

```rust
/// The source-mode canvas: base_fg/base_bg only (Spec §3.5). Default theme's
/// `base = Default` → `Reset` → terminal default (no themed canvas).
pub fn base_canvas(theme: &Theme, depth: Depth) -> RStyle {
    face_to_ratatui(&Face { fg: Some(theme.base_fg), bg: Some(theme.base_bg), ..Face::default() }, depth)
}
```

- [ ] **Step 4: Apply it in render's source-mode path.** Where the source branch builds its base style as `compose(&editor.theme, editor.depth, &[SE::Text])` (~:391, ~:462), use `compose::base_canvas(&editor.theme, editor.depth)` as the base instead, then layer the SAME overlays (Selection/search/diag/focus) on top exactly as today. Do NOT add per-element/role/inline faces and NO heading glyph in source mode (the literal `#` shows) — only the canvas changes. (The `Depth::None` color suppression from Task 4b applies here too: phosphor source under `NO_COLOR` → monochrome.)

- [ ] **Step 5: Run** `cargo test -p wordcartel source_mode_tints render::` — PASS. Then full `cargo test -p wordcartel --lib`. If a Default-theme source-mode golden changed `None`→`Some(Reset)`, that is the accepted Default canvas (Reset == terminal default) — update the golden to match; if any OTHER (live-preview, or non-Default) golden changed, STOP and investigate (the canvas must not touch live-preview).

- [ ] **Step 6: Commit** `feat(theme): source-mode base canvas — phosphor/base16 tint source text (§3.5)`

---

## Final Verification
- [ ] `cargo test` (workspace) — all green.
- [ ] `cargo clippy -p wordcartel-core -p wordcartel --lib` — no NEW warnings in the touched files.
- [ ] Manual smoke (document): (1) `[theme] name = "tokyo-night"` config → colored on startup. (2) `[theme] file = "…/base16-gruvbox.yaml"` → imported palette. (3) `NO_COLOR=1` → monochrome + heading glyphs (cue mode), and the picker can't re-enable color. (4) Run the `theme` command → picker lists built-ins, arrow keys live-preview, Enter keeps, Esc restores. (5) `[theme.styles] selection = { bg = "#283457" }` overrides only the selection bg.

## Self-Review Notes (coverage vs spec §12 plan ③)
- §3.2 base16 import (BasePalette/from_base16) → Task 1; **§3.2 depth/quantize/Depth already existed (plan ①) — consumed, not rebuilt.**
- §3.3 base16 file parse (no YAML) → Task 2; `detect_depth`/precedence → Task 4; `resolve_theme` → Task 5.
- §5 `[theme]` config (RawThemeConfig/RawFace, discriminated source, ~/relative paths, serde default) → Task 3; per-element `[theme.styles]` → Task 1 (`override_face`/`element_from_key`) + Task 5 (apply).
- §3.6 active theme location + relayout-on-switch → Task 6 (startup seed) + Task 7 (`apply_theme` + picker).
- §4 cue-mode forced heading glyph → Task 5 (`apply_cue_mode_glyph`) + Task 7 (`apply_theme`); cue-mode color suppression → Task 4b (`face_to_ratatui` at `Depth::None`) + Task 5 (monochrome style scrub).
- §3.5 source-mode base canvas (phosphor/base16 tint source text) → **Task 8** (Codex I8 — the user explicitly asked for the green/amber-monitor source look).
- **Out of scope (correctly NOT here):** §3.7 prefix geometry (plan ②), §3.9 producers (plan ②), §13.2 proof (plan ②), §3.4 compose merge / §3.8 chrome faces (plan ①).
- **Codex plan-review folded:** C1 (`Depth::None` color suppression, Task 4b), C2 (monochrome style scrub, Task 5), C3 (`Theme::builtin`/`builtin_names` associated methods), I4 (`heading_glyph_cfg` survives picker switch), I5 (XOR clears all 6 openers + menu cmd + dispatch_overlay_command), I6 (picker Paste intercept), I7 (modules in lib.rs), I8 (Task 8 canvas), M1 (base16 inline comments), M2 (picker render test in render.rs).
</content>
