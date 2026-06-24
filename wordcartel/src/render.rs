// Task 5: ratatui live-preview render + status line.
// Pure: takes &Editor, mutates NOTHING on the editor.

use crate::{editor::Editor, nav};
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Modifier, Style as RStyle},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use wordcartel_core::style::Style;

/// Map a wordcartel inline `Style` to a ratatui `Style`.
///
/// Strong→BOLD; Emphasis→ITALIC; StrongEmphasis→BOLD|ITALIC;
/// Strikethrough→CROSSED_OUT; Code→Cyan color; Link→UNDERLINED+Yellow;
/// Plain→default.
pub fn style_to_ratatui(s: Style) -> RStyle {
    match s {
        Style::Plain => RStyle::default(),
        Style::Strong => RStyle::default().add_modifier(Modifier::BOLD),
        Style::Emphasis => RStyle::default().add_modifier(Modifier::ITALIC),
        Style::StrongEmphasis => {
            RStyle::default().add_modifier(Modifier::BOLD | Modifier::ITALIC)
        }
        Style::Strikethrough => RStyle::default().add_modifier(Modifier::CROSSED_OUT),
        Style::Code => RStyle::default().fg(Color::Cyan),
        Style::Link => RStyle::default().add_modifier(Modifier::UNDERLINED).fg(Color::Yellow),
    }
}

/// Paint the viewport + status line to `frame` using `editor` state.
///
/// Pure: the `editor` is borrowed immutably; nothing is mutated.
///
/// Layout:
/// - Editing area = full frame area minus the bottom row.
/// - Status line = the bottom row.
///
/// §15.6 tiny-terminal guard: if width < 4 or height < 2, paint a clamped
/// "too small" notice and return without indexing out of bounds.
pub fn render(frame: &mut Frame, editor: &Editor) {
    let area = frame.area();
    let w = area.width;
    let h = area.height;

    // §15.6: too small to render properly.
    if w < 4 || h < 2 {
        if w > 0 && h > 0 {
            let notice = "...";
            let truncated: String = notice.chars().take(w as usize).collect();
            let line = Line::from(truncated);
            let para = Paragraph::new(line);
            frame.render_widget(para, Rect::new(area.x, area.y, w, 1));
        }
        return;
    }

    let edit_height = h - 1; // rows available for editing content
    let status_row = area.y + h - 1;

    // -----------------------------------------------------------------------
    // Editing area: walk visible logical lines from view.scroll
    // -----------------------------------------------------------------------
    let scroll = editor.active().view.scroll;
    let mut screen_row: u16 = 0;

    // Collect sorted logical line indices from the layout cache.
    let mut sorted_lines: Vec<usize> = editor.active().view.line_layouts.keys().copied().collect();
    sorted_lines.sort_unstable();

    'outer: for &l in &sorted_lines {
        if l < scroll {
            continue;
        }
        let (visual_rows, _map) = &editor.active().view.line_layouts[&l];
        let skip_rows = if l == scroll {
            editor.active().view.scroll_row
        } else {
            0
        };
        for vr in visual_rows.iter().skip(skip_rows) {
            if screen_row >= edit_height {
                break 'outer;
            }
            // Build spans for this visual row.
            let mut spans: Vec<Span<'_>> = Vec::new();

            // Prepend prefix_glyph as a dim span (first visual row only).
            if let Some(ref glyph) = vr.prefix_glyph {
                spans.push(Span::styled(
                    glyph.clone(),
                    RStyle::default().add_modifier(Modifier::DIM),
                ));
            }

            // One span per StyledSeg.
            for seg in &vr.segs {
                spans.push(Span::styled(seg.text.clone(), style_to_ratatui(seg.style)));
            }

            let line_widget = Line::from(spans);
            let row_area = Rect::new(area.x, area.y + screen_row, w, 1);
            frame.render_widget(Paragraph::new(line_widget), row_area);

            screen_row += 1;
        }
    }

    // -----------------------------------------------------------------------
    // Status line (bottom row)
    // -----------------------------------------------------------------------
    {
        let path_str = editor
            .active()
            .document
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "[no name]".to_string());

        let dirty_marker = if editor.active().document.dirty() { "*" } else { "" };
        let mode_text = match editor.active().view.mode {
            crate::editor::RenderMode::LivePreview => "PREVIEW",
            crate::editor::RenderMode::SourceHighlighted => "SRC-HI",
            crate::editor::RenderMode::SourcePlain => "SOURCE",
        };

        // When a modal prompt is active, render its message instead of the normal
        // status text, using a distinct style so it stands out.
        let (status_text, status_style) = if let Some(ref prompt) = editor.prompt {
            (
                prompt.message.clone(),
                RStyle::default().add_modifier(Modifier::REVERSED),
            )
        } else {
            let text = if editor.status.is_empty() {
                format!("{}{} [{}]", path_str, dirty_marker, mode_text)
            } else {
                format!("{}{} [{}] {}", path_str, dirty_marker, mode_text, editor.status)
            };
            (text, RStyle::default().add_modifier(Modifier::REVERSED))
        };

        // Truncate to fit the terminal width.
        let truncated: String = status_text.chars().take(w as usize).collect();
        let status_line = Line::from(Span::styled(truncated, status_style));
        let status_area = Rect::new(area.x, status_row, w, 1);
        frame.render_widget(Paragraph::new(status_line), status_area);
    }

    // -----------------------------------------------------------------------
    // Hardware cursor
    // -----------------------------------------------------------------------
    if let Some((col, row)) = nav::screen_pos(editor) {
        // Guard: only set if within the editing area (not into the status line).
        if row < edit_height && col < w {
            frame.set_cursor_position(Position { x: area.x + col, y: area.y + row });
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (RED first — write before implementing)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{derive, editor::Editor};
    use ratatui::{backend::TestBackend, Terminal};
    use wordcartel_core::selection::Selection;

    fn set_caret(e: &mut Editor, off: usize) {
        e.active_mut().document.selection = Selection::single(off);
    }

    /// Row 0 of a heading with caret on a later line must show "Title" (concealed "# ").
    #[test]
    fn renders_concealed_heading_and_cursor_on_active_line() {
        let mut e = Editor::new_from_text("# Title\n\nbody\n", None, (20, 6));
        set_caret(&mut e, 10); // somewhere in "body" so heading line is inactive/concealed
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(20, 6)).unwrap();
        term.draw(|f| render(f, &e)).unwrap();
        let buf = term.backend().buffer();
        // row 0 shows "Title" (concealed "# "), not "# Title"
        let row0: String = (0u16..20).map(|x| buf[(x, 0u16)].symbol().chars().next().unwrap_or(' ')).collect();
        assert!(row0.starts_with("Title"), "expected 'Title...' got {:?}", row0);
    }

    /// `style_to_ratatui(Style::Strong)` must have BOLD modifier.
    #[test]
    fn style_mapping_is_bold_for_strong() {
        assert!(
            style_to_ratatui(Style::Strong)
                .add_modifier
                .contains(Modifier::BOLD),
            "Strong style must map to BOLD"
        );
    }

    /// Tiny terminals must not panic — §15.6.
    #[test]
    fn tiny_terminal_shows_notice_not_panic() {
        for (w, h) in [(1u16, 1u16), (2, 1), (3, 2)] {
            let mut e = Editor::new_from_text("# Title\n\nbody\n", None, (w, h));
            derive::rebuild(&mut e);
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| render(f, &e)).unwrap(); // must not panic at any tiny size
        }
    }

    /// When a modal prompt is active, the status row must show the prompt message.
    #[test]
    fn renders_active_prompt_on_status_row() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 6));
        e.active_mut().document.version = 1; // dirty so quit_confirm is realistic
        e.prompt = Some(crate::prompt::Prompt::quit_confirm());
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(40, 6)).unwrap();
        term.draw(|f| render(f, &e)).unwrap();
        let buf = term.backend().buffer();
        // Bottom row (row 5) must show the prompt message, not the normal status.
        let status_row: String = (0u16..40)
            .map(|x| buf[(x, 5u16)].symbol().chars().next().unwrap_or(' '))
            .collect();
        // The quit_confirm message starts with "Unsaved changes: [S]ave & quit …"
        // At terminal width 40 the truncation leaves "Unsaved changes: [S]ave & quit · [Q]uit "
        assert!(
            status_row.contains("Unsaved changes") || status_row.contains("[S]ave"),
            "status row must show prompt message, got: {:?}",
            status_row
        );
    }

    #[test]
    fn render_skips_scroll_row_for_top_logical_line() {
        let mut e = Editor::new_from_text("abcdefghijklmnopqrstuvwxyz123456", None, (4, 5));
        set_caret(&mut e, 25);
        crate::nav::ensure_visible(&mut e);
        derive::rebuild(&mut e);

        assert_eq!(e.active().view.scroll, 0);
        assert_eq!(e.active().view.scroll_row, 3);

        let mut term = Terminal::new(TestBackend::new(4, 5)).unwrap();
        term.draw(|f| render(f, &e)).unwrap();
        let buf = term.backend().buffer();
        let row0: String = (0u16..4)
            .map(|x| buf[(x, 0u16)].symbol().chars().next().unwrap_or(' '))
            .collect();

        assert_eq!(row0, "mnop");
        assert!(crate::nav::screen_pos(&e).is_some());
    }
}
