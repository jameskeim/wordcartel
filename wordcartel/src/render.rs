// Task 5: ratatui live-preview render + status line.
// Pure: takes &Editor, mutates NOTHING on the editor.

use crate::{editor::Editor, nav};
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Modifier, Style as RStyle},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
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

// Shared geometry — render AND mouse (Task 7) both call these.
pub(crate) fn menu_bar_layout(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)]) -> Vec<(usize, Rect)> {
    let mut out = Vec::new();
    let mut x = area.x;
    for (i, (cat, _)) in groups.iter().enumerate() {
        let label = crate::menu::category_label_pub(*cat);
        let wgt = label.chars().count() as u16 + 2; // 1 space padding each side
        out.push((i, Rect::new(x, area.y, wgt, 1)));
        x = x.saturating_add(wgt);
    }
    out
}

pub(crate) fn menu_dropdown_rect(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)], open: usize) -> Option<Rect> {
    let bar = menu_bar_layout(area, groups);
    let (_, label_rect) = bar.get(open)?;
    let leaves = &groups.get(open)?.1;
    if leaves.is_empty() { return None; }
    let width = leaves.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0) as u16 + 2;
    let height = leaves.len() as u16;
    Some(Rect::new(label_rect.x, area.y + 1, width.min(area.width.saturating_sub(label_rect.x - area.x)), height.min(area.height.saturating_sub(1))))
}

#[allow(dead_code)] // used by Task 7 (mouse hit-testing)
pub(crate) fn menu_dropdown_row_at(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)], open: usize, col: u16, row: u16) -> Option<usize> {
    let r = menu_dropdown_rect(area, groups, open)?;
    if col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height {
        Some((row - r.y) as usize)
    } else { None }
}

/// Paint the viewport + status line to `frame` using `editor` state.
///
/// The editor is borrowed mutably so stateful overlay widgets can update their
/// internal render state.
///
/// Layout:
/// - Editing area = full frame area minus the bottom row.
/// - Status line = the bottom row.
///
/// §15.6 tiny-terminal guard: if width < 4 or height < 2, paint a clamped
/// "too small" notice and return without indexing out of bounds.
pub fn render(frame: &mut Frame, editor: &mut Editor) {
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

    let menu_rows = u16::from(editor.menu.is_some());
    let edit_height = h.saturating_sub(1 + menu_rows); // rows available for editing content
    let edit_top = area.y + menu_rows;
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
            let row_area = Rect::new(area.x, edit_top + screen_row, w, 1);
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
        // When the minibuffer is open, render <prompt><text> on the status row.
        let (status_text, status_style) = if let Some(ref mb) = editor.minibuffer {
            (
                format!("{}{}", mb.prompt, mb.text),
                RStyle::default().add_modifier(Modifier::REVERSED),
            )
        } else if let Some(ref prompt) = editor.prompt {
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
    if let Some(ref mb) = editor.minibuffer {
        // Minibuffer is open: place caret on the status row at prompt.len() + cursor.
        // cursor is a byte offset; for display we want the char count so the terminal
        // column is correct even for multi-byte prompts/text (small strings, safe).
        let prompt_cols = mb.prompt.chars().count() as u16;
        let text_cols = mb.text[..mb.cursor].chars().count() as u16;
        let caret_col = prompt_cols + text_cols;
        if caret_col < w {
            frame.set_cursor_position(Position { x: area.x + caret_col, y: status_row });
        }
    } else if let Some((col, row)) = nav::screen_pos(editor) {
        // Guard: only set if within the editing area (not into the status line).
        if row < edit_height && col < w {
            frame.set_cursor_position(Position { x: area.x + col, y: edit_top + row });
        }
    }

    // -----------------------------------------------------------------------
    // Command palette overlay (drawn on top of everything else)
    // -----------------------------------------------------------------------
    if let Some(ref palette) = editor.palette {
        // Overlay dimensions: width = 60% of terminal (min 30, max 80), height = up to 20 rows.
        let ov_w = (w * 3 / 5).max(30).min(80).min(w);
        let max_rows = palette.rows.len() as u16;
        let list_h = max_rows.min(15).min(h.saturating_sub(4));
        let ov_h = (list_h + 3).min(h); // 1 border top + 1 query row + list_h list rows + 1 border bottom = list_h + 3; clamp to h

        // Center the overlay.
        let ov_x = area.x + (w.saturating_sub(ov_w)) / 2;
        let ov_y = area.y + (h.saturating_sub(ov_h)) / 4; // slightly above center

        let ov_rect = Rect::new(ov_x, ov_y, ov_w, ov_h);

        // Clear the overlay area.
        frame.render_widget(Clear, ov_rect);

        // Draw the border.
        let block = Block::default().borders(Borders::ALL).title(" Command Palette ");
        frame.render_widget(block, ov_rect);

        if ov_h < 3 {
            return; // too small to render query + any rows
        }

        // Query row (just inside top border).
        let query_area = Rect::new(ov_x + 1, ov_y + 1, ov_w.saturating_sub(2), 1);
        let query_display = format!("> {}", palette.query);
        let truncated_q: String = query_display.chars().take(query_area.width as usize).collect();
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(truncated_q, RStyle::default()))),
            query_area,
        );

        if ov_h < 4 || list_h == 0 {
            return;
        }

        // List of rows (below query, inside border).
        let list_area = Rect::new(ov_x + 1, ov_y + 2, ov_w.saturating_sub(2), list_h);
        let highlight_style = RStyle::default().add_modifier(Modifier::REVERSED);
        let items: Vec<ListItem> = palette.rows.iter().take(list_h as usize).map(|row| {
            // Left: label; right-aligned: chord.
            let chord_w = row.chord.chars().count() as u16;
            let label_w = list_area.width.saturating_sub(chord_w + 1) as usize;
            let label: String = row.label.chars().take(label_w).collect();
            let padding = " ".repeat(list_area.width.saturating_sub(label.chars().count() as u16 + chord_w) as usize);
            let text = format!("{label}{padding}{}", row.chord);
            ListItem::new(Line::from(text))
        }).collect();

        let mut list_state = ListState::default();
        list_state.select(if palette.rows.is_empty() { None } else { Some(palette.selected) });

        frame.render_stateful_widget(
            List::new(items).highlight_style(highlight_style),
            list_area,
            &mut list_state,
        );
    }

    if let Some(ref menu) = editor.menu {
        if !menu.groups.is_empty() {
            let menu_area = Rect::new(area.x, area.y, w, h.saturating_sub(1));
            // Paint the menu bar (one label per category)
            let bar = menu_bar_layout(menu_area, &menu.groups);
            for (i, rect) in &bar {
                let cat = menu.groups[*i].0;
                let label = crate::menu::category_label_pub(cat);
                let text = format!(" {label} ");
                let style = if *i == menu.open {
                    RStyle::default().fg(Color::Black).bg(Color::White)
                } else {
                    RStyle::default().fg(Color::White).bg(Color::Black)
                };
                frame.render_widget(Paragraph::new(text).style(style), *rect);
            }
            // Paint the dropdown for the open category
            if let Some(drop_rect) = menu_dropdown_rect(menu_area, &menu.groups, menu.open) {
                frame.render_widget(Clear, drop_rect);
                let leaves = &menu.groups[menu.open].1;
                let items: Vec<ListItem> = leaves
                    .iter()
                    .enumerate()
                    .map(|(row, (label, _))| {
                        let style = if row == menu.highlighted {
                            RStyle::default().fg(Color::Black).bg(Color::White)
                        } else {
                            RStyle::default().fg(Color::White).bg(Color::DarkGray)
                        };
                        ListItem::new(format!(" {label} ")).style(style)
                    })
                    .collect();
                frame.render_widget(List::new(items), drop_rect);
            }
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
        term.draw(|f| render(f, &mut e)).unwrap();
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
            term.draw(|f| render(f, &mut e)).unwrap(); // must not panic at any tiny size
        }
    }

    /// When a modal prompt is active, the status row must show the prompt message.
    #[test]
    fn renders_active_prompt_on_status_row() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 6));
        e.active_mut().document.version = 1; // dirty so quit_confirm is realistic
        e.open_prompt(crate::prompt::Prompt::quit_confirm());
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(40, 6)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
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

    /// When the minibuffer is open, the status row must show <prompt><text>.
    #[test]
    fn renders_active_minibuffer_on_status_row() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 6));
        e.open_minibuffer("> ");
        // Simulate typing "cat" into the minibuffer
        e.minibuffer.as_mut().unwrap().insert('c');
        e.minibuffer.as_mut().unwrap().insert('a');
        e.minibuffer.as_mut().unwrap().insert('t');
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(40, 6)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();
        // Bottom row (row 5) must show "> cat"
        let status_row: String = (0u16..40)
            .map(|x| buf[(x, 5u16)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            status_row.starts_with("> cat"),
            "status row must show minibuffer prompt+text, got: {:?}",
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
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();
        let row0: String = (0u16..4)
            .map(|x| buf[(x, 0u16)].symbol().chars().next().unwrap_or(' '))
            .collect();

        assert_eq!(row0, "mnop");
        assert!(crate::nav::screen_pos(&e).is_some());
    }
}
