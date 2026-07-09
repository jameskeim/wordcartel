//! Theme-picker overlay: lists built-in theme names, filters by query, applies
//! (with live preview) on selection. Mirrors the command palette (palette.rs).

use wordcartel_core::theme::Theme;
use crate::app::Msg;
use crossterm::event::Event;

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

/// Theme picker overlay intercepts KEY INPUT and PASTE. Non-key, non-paste messages
/// fall through to normal handling while the picker stays open (mirrors palette block).
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled {
    if editor.theme_picker.is_none() { return crate::app::Handled::Pass(msg); }
    // Paste intercept FIRST (mirror the palette, app.rs palette block) — else paste leaks
    // into the document while the picker is open (Codex I6).
    if matches!(&msg, Msg::ClipboardPaste { .. }) {
        // Drop an async clipboard-paste result that arrives while the theme picker is
        // open — it must not land in the document behind the overlay.
        return crate::app::Handled::Done({ for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); } !editor.quit });
    }
    if let Msg::Input(Event::Paste(text)) = &msg {
        let ah = editor.active().view.area.1;
        if let Some(tp) = editor.theme_picker.as_mut() {
            tp.query.push_str(text);
            crate::theme_picker::rebuild_rows(tp);
            crate::app::keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
        }
        crate::theme_cmds::preview_selected_theme(editor);
        return crate::app::Handled::Done({ for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); } !editor.quit });
    }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            use crossterm::event::KeyCode;
            match k.code {
                KeyCode::Esc => {
                    // cancel preview → restore the theme active when we opened.
                    if let Some(tp) = editor.theme_picker.take() { editor.apply_theme(tp.original); }
                }
                KeyCode::Enter => { crate::theme_cmds::commit_theme_picker(editor); }
                KeyCode::Up => {
                    let ah = editor.active().view.area.1;
                    if let Some(tp) = editor.theme_picker.as_mut() {
                        tp.selected = tp.selected.saturating_sub(1);
                        crate::app::keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                    }
                    crate::theme_cmds::preview_selected_theme(editor);
                }
                KeyCode::Down => {
                    let ah = editor.active().view.area.1;
                    if let Some(tp) = editor.theme_picker.as_mut() {
                        let max = tp.rows.len().saturating_sub(1);
                        tp.selected = (tp.selected + 1).min(max);
                        crate::app::keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                    }
                    crate::theme_cmds::preview_selected_theme(editor);
                }
                KeyCode::PageDown => {
                    let ah = editor.active().view.area.1;
                    if let Some(tp) = editor.theme_picker.as_mut() {
                        let lh = crate::list_window::list_h_for(tp.rows.len(), ah);
                        tp.selected = (tp.selected + lh.max(1)).min(tp.rows.len().saturating_sub(1));
                        crate::app::keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                    }
                    crate::theme_cmds::preview_selected_theme(editor);
                }
                KeyCode::PageUp => {
                    let ah = editor.active().view.area.1;
                    if let Some(tp) = editor.theme_picker.as_mut() {
                        let lh = crate::list_window::list_h_for(tp.rows.len(), ah);
                        tp.selected = tp.selected.saturating_sub(lh.max(1));
                        crate::app::keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                    }
                    crate::theme_cmds::preview_selected_theme(editor);
                }
                KeyCode::Home => {
                    let ah = editor.active().view.area.1;
                    if let Some(tp) = editor.theme_picker.as_mut() {
                        tp.selected = 0;
                        crate::app::keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                    }
                    crate::theme_cmds::preview_selected_theme(editor);
                }
                KeyCode::End => {
                    let ah = editor.active().view.area.1;
                    if let Some(tp) = editor.theme_picker.as_mut() {
                        tp.selected = tp.rows.len().saturating_sub(1);
                        crate::app::keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                    }
                    crate::theme_cmds::preview_selected_theme(editor);
                }
                KeyCode::Backspace => {
                    let ah = editor.active().view.area.1;
                    if let Some(tp) = editor.theme_picker.as_mut() {
                        tp.query.pop();
                        crate::theme_picker::rebuild_rows(tp);
                        crate::app::keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                    }
                    crate::theme_cmds::preview_selected_theme(editor);
                }
                KeyCode::Char(c)
                    if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                {
                    let ah = editor.active().view.area.1;
                    if let Some(tp) = editor.theme_picker.as_mut() {
                        tp.query.push(c);
                        crate::theme_picker::rebuild_rows(tp);
                        crate::app::keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                    }
                    crate::theme_cmds::preview_selected_theme(editor);
                }
                _ => {}
            }
        }
        return crate::app::Handled::Done({ for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); } !editor.quit });
    }
    // Non-key msg falls through to normal handling while picker stays open.
    crate::app::Handled::Pass(msg)
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
