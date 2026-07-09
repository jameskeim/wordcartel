//! Theme-picker overlay: lists built-in theme names, filters by query, applies
//! (with live preview) on selection. Mirrors the command palette (palette.rs).

use wordcartel_core::theme::Theme;

#[derive(Debug, Clone)]
pub struct ThemePicker {
    pub query: String,
    pub selected: usize,
    pub rows: Vec<String>,
    /// First visible row index — drives the windowed painter (A6).
    pub scroll_top: usize,
    /// The theme active when the picker opened — restored on Esc (preview cancel).
    pub original: Theme,
    /// The builtin name most recently applied via preview_selected_theme — the
    /// single funnel for the identity threading. `None` until a preview fires;
    /// consumed (take()) by the Enter arm to commit the identity; drops with the
    /// picker struct on Esc (restoring the original already clears it implicitly).
    pub previewed: Option<String>,
}

/// Rebuild rows from the built-in theme list, filtered by `query` (case-insensitive
/// substring) and sorted alphabetically for display (the selector is A→Z regardless of
/// registration order). `selected` clamped.
pub fn rebuild_rows(tp: &mut ThemePicker) {
    let q = tp.query.to_ascii_lowercase();
    tp.rows = Theme::builtin_names().iter()   // associated method (Codex C3)
        .filter(|n| q.is_empty() || n.to_ascii_lowercase().contains(&q))
        .map(|n| n.to_string())
        .collect();
    tp.rows.sort();                            // alphabetical display order (case-sensitive ASCII)
    if tp.selected >= tp.rows.len() { tp.selected = tp.rows.len().saturating_sub(1); }
    tp.scroll_top = tp.scroll_top.min(tp.rows.len().saturating_sub(1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    #[test]
    fn rebuild_rows_filters_builtins() {
        let mut tp = ThemePicker { query: String::new(), selected: 0, rows: vec![],
            scroll_top: 0, original: wordcartel_core::theme::default(), previewed: None };
        rebuild_rows(&mut tp);
        assert!(tp.rows.iter().any(|r| r == "tokyo-night"));
        assert!(tp.rows.len() >= 22, "expected >= 22 builtins, got {}", tp.rows.len());
        // Selector rows are displayed alphabetically (rebuild_rows sorts).
        let mut sorted = tp.rows.clone(); sorted.sort();
        assert_eq!(tp.rows, sorted, "rows must be alphabetized for display");
        assert!(tp.rows.iter().any(|r| r == "forever-blue-jeans-dark"), "Blue Jeans dark listed");
        tp.query = "phosphor-amber".into();
        rebuild_rows(&mut tp);
        assert!(tp.rows.iter().all(|r| r.contains("phosphor-amber")));
        assert!(tp.rows.contains(&"phosphor-amber".to_string()));
        // phosphor-amber-flat removed in D4 — no flat variants in builtin_names()
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

    /// Esc-restore round-trip: open picker (captures original theme A), preview a
    /// different theme B, then restore original (simulating Esc → app.rs calls
    /// `apply_theme(tp.original)`). Asserts the active theme returns to A.
    #[test]
    fn esc_restore_returns_to_original_theme() {
        let mut ed = Editor::new_from_text("hello\n", None, (40, 12));
        crate::derive::rebuild(&mut ed);
        // default theme is A
        let default_name = ed.theme.name.clone();
        assert_eq!(default_name, "terminal-plain");
        ed.open_theme_picker();
        let original = ed.theme_picker.as_ref().unwrap().original.clone();
        assert_eq!(original.name, default_name, "picker must capture the original theme");
        // simulate live preview: apply B (tokyo-night)
        ed.apply_theme(wordcartel_core::theme::tokyo_night());
        assert_eq!(ed.theme.name, "tokyo-night", "preview must be active");
        // simulate Esc: restore A from picker.original
        ed.apply_theme(original);
        assert_eq!(ed.theme.name, default_name, "Esc restore must return to the original theme");
    }

    #[test]
    fn open_outline_clears_theme_picker() {
        let mut ed = Editor::new_from_text("# Heading\n\nbody\n", None, (40, 12));
        crate::derive::rebuild(&mut ed);
        ed.open_theme_picker();
        assert!(ed.theme_picker.is_some());
        ed.open_outline();
        assert!(ed.theme_picker.is_none(), "opening the outline must clear the theme picker (XOR)");
        assert!(ed.outline.is_some());
    }

    /// T7 pin: `preview_selected_theme` derives chrome using `editor.chrome_disposition`
    /// before `apply_theme` (grounding A.9 / D3). Under Zen disposition, flexoki-dark's
    /// Chrome bg must equal the §II.5 probe-generated ZEN Chrome bg (#1e1c1c).
    ///
    /// The >= 19 count pin lives in `rebuild_rows_filters_builtins` (T2 — not duplicated here).
    #[test]
    fn preview_derives_zen_chrome_bg_on_flexoki_dark() {
        use wordcartel_core::theme::{ChromeDisposition, Color, SemanticElement};
        let mut ed = Editor::new_from_text("x\n", None, (40, 12));
        ed.chrome_disposition = ChromeDisposition::Zen;
        ed.open_theme_picker();
        // Rows are alphabetized (rebuild_rows sorts), so locate flexoki-dark by position
        // rather than a fixed index.
        let idx = ed.theme_picker.as_ref().unwrap().rows.iter()
            .position(|r| r == "flexoki-dark")
            .unwrap_or_else(|| panic!("flexoki-dark must be listed; rows: {:?}",
                ed.theme_picker.as_ref().unwrap().rows));
        ed.theme_picker.as_mut().unwrap().selected = idx;
        // preview_selected_theme is the single funnel (A.9): derives before apply_theme.
        crate::theme_cmds::preview_selected_theme(&mut ed);
        // §II.5 probe-generated: flexoki-dark ZEN Chrome bg = #1e1c1c.
        let chrome_bg = ed.theme.face(SemanticElement::Chrome).bg;
        assert_eq!(
            chrome_bg,
            Some(Color::Rgb { r: 0x1e, g: 0x1c, b: 0x1c }),
            "preview under Zen must install ZEN Chrome bg #1e1c1c for flexoki-dark (§II.5); got {:?}",
            chrome_bg,
        );
    }
}
