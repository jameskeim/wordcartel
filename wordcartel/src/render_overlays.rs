// render_overlays.rs — overlay and menu painters (moved from render.rs, Task 4).
// All logic is byte-identical to the inline code it replaced; the only changes
// are the module boundary, the added imports, and receiving `&ChromeStyles`
// instead of accessing the six chrome locals that existed in render.rs.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::{
    editor::Editor,
    render::{
        ChromeStyles, menu_bar_layout, menu_bar_layout_cats, menu_dropdown_rect,
        palette_overlay_rect, windowed_indicator,
    },
};

/// Paint all overlay and menu surfaces for one frame.
///
/// Called from `render::render()` after the chrome styles are built.
/// The painters are listed in render order (overlays on top of the editing
/// area, menu drawn first so overlays can cover it):
/// - Command palette
/// - Outline
/// - Theme picker
/// - File browser
/// - Menu bar + dropdown
/// - Diagnostic quick-fix
///
/// `area` and `h` are derived from `frame.area()` to match the values the
/// main render function computes; no state is duplicated.
pub(crate) fn paint(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    let area = frame.area();
    let h = area.height;

    // -----------------------------------------------------------------------
    // Command palette overlay (drawn on top of everything else)
    // -----------------------------------------------------------------------
    // A6 self-heal: the window must respect the LIVE frame's geometry (resize
    // has no overlay hook; render is the one place that always sees the truth).
    if let Some(p) = editor.palette.as_mut() {
        crate::app::keep_overlay_visible(h, p.selected, p.rows.len(), &mut p.scroll_top);
    }
    if let Some(ref palette) = editor.palette {
        // Overlay dimensions — shared with mouse hit-testing via palette_overlay_rect.
        let ov_rect = palette_overlay_rect(area, palette.rows.len());
        let ov_x = ov_rect.x;
        let ov_y = ov_rect.y;
        let ov_w = ov_rect.width;
        let ov_h = ov_rect.height;
        let list_h = crate::list_window::list_h_for(palette.rows.len(), h) as u16;

        // Clear the overlay area; then apply the fill style (T4: no-op default; T5: ChromeOverlay bg).
        frame.render_widget(Clear, ov_rect);
        frame.buffer_mut().set_style(ov_rect, cs.ov_fill);

        // Draw the border (FIX-3: themed with Chrome so the frame matches the panel bg).
        let mut block = Block::default().borders(Borders::ALL).title(" Command Palette ")
            .border_style(cs.overlay_border);
        if let Some(ind) = windowed_indicator(palette.selected, palette.rows.len(), list_h as usize) {
            block = block.title_bottom(ind);
        }
        frame.render_widget(block, ov_rect);

        if ov_h < 3 {
            return; // too small to render query + any rows
        }

        // Query row (just inside top border).
        let query_area = Rect::new(ov_x + 1, ov_y + 1, ov_w.saturating_sub(2), 1);
        let query_display = format!("> {}", palette.query);
        let truncated_q: String = query_display.chars().take(query_area.width as usize).collect();
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(truncated_q, cs.ov_query))),
            query_area,
        );

        if ov_h < 4 || list_h == 0 {
            return;
        }

        // List of rows (below query, inside border) — windowed by scroll_top.
        let list_area = Rect::new(ov_x + 1, ov_y + 2, ov_w.saturating_sub(2), list_h);
        let highlight_style = cs.overlay_selected;
        let end = (palette.scroll_top + list_h as usize).min(palette.rows.len());
        let items: Vec<ListItem> = palette.rows[palette.scroll_top..end].iter().map(|row| {
            // Left: label; right-aligned: chord.
            let chord_w = row.chord.chars().count() as u16;
            let label_w = list_area.width.saturating_sub(chord_w + 1) as usize;
            let label: String = row.label.chars().take(label_w).collect();
            let padding = " ".repeat(list_area.width.saturating_sub(label.chars().count() as u16 + chord_w) as usize);
            let text = format!("{label}{padding}{}", row.chord);
            ListItem::new(Line::from(text))
        }).collect();

        let mut list_state = ListState::default();
        list_state.select(if palette.rows.is_empty() {
            None
        } else {
            Some(palette.selected.saturating_sub(palette.scroll_top))
        });

        frame.render_stateful_widget(
            List::new(items).highlight_style(highlight_style),
            list_area,
            &mut list_state,
        );
    }

    // A6 self-heal: the window must respect the LIVE frame's geometry (resize
    // has no overlay hook; render is the one place that always sees the truth).
    if let Some(o) = editor.outline.as_mut() {
        crate::app::keep_overlay_visible(h, o.selected, o.rows.len(), &mut o.scroll_top);
    }
    if let Some(ref outline) = editor.outline {
        let ov_rect = palette_overlay_rect(area, outline.rows.len());
        let ov_x = ov_rect.x;
        let ov_y = ov_rect.y;
        let ov_w = ov_rect.width;
        let ov_h = ov_rect.height;
        let list_h = crate::list_window::list_h_for(outline.rows.len(), h);

        frame.render_widget(Clear, ov_rect);
        frame.buffer_mut().set_style(ov_rect, cs.ov_fill);
        let mut block = Block::default().borders(Borders::ALL).title(" Outline ")
            .border_style(cs.overlay_border);
        if let Some(ind) = windowed_indicator(outline.selected, outline.rows.len(), list_h) {
            block = block.title_bottom(ind);
        }
        frame.render_widget(block, ov_rect);

        if ov_h >= 3 {
            let query_area = Rect::new(ov_x + 1, ov_y + 1, ov_w.saturating_sub(2), 1);
            let query_display = format!("> {}", outline.query);
            let truncated_q: String = query_display.chars().take(query_area.width as usize).collect();
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(truncated_q, cs.ov_query))),
                query_area,
            );

            if ov_h >= 4 && list_h > 0 {
                let list_h_u16 = list_h as u16;
                let list_area = Rect::new(ov_x + 1, ov_y + 2, ov_w.saturating_sub(2), list_h_u16);
                let highlight_style = cs.overlay_selected;
                let end = (outline.scroll_top + list_h).min(outline.rows.len());
                let items: Vec<ListItem> = outline.rows[outline.scroll_top..end].iter().map(|row| {
                    let mut text = format!("{}{}", " ".repeat(row.indent.saturating_mul(2)), row.text);
                    text = text.chars().take(list_area.width as usize).collect();
                    ListItem::new(Line::from(text))
                }).collect();

                let mut list_state = ListState::default();
                list_state.select(if outline.rows.is_empty() {
                    None
                } else {
                    Some(outline.selected.saturating_sub(outline.scroll_top))
                });

                frame.render_stateful_widget(
                    List::new(items).highlight_style(highlight_style),
                    list_area,
                    &mut list_state,
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Theme picker overlay (drawn on top of everything else)
    // -----------------------------------------------------------------------
    // A6 self-heal: the window must respect the LIVE frame's geometry (resize
    // has no overlay hook; render is the one place that always sees the truth).
    if let Some(tp) = editor.theme_picker.as_mut() {
        crate::app::keep_overlay_visible(h, tp.selected, tp.rows.len(), &mut tp.scroll_top);
    }
    if let Some(ref tp) = editor.theme_picker {
        let ov_rect = palette_overlay_rect(area, tp.rows.len());
        let ov_x = ov_rect.x;
        let ov_y = ov_rect.y;
        let ov_w = ov_rect.width;
        let ov_h = ov_rect.height;
        let list_h = crate::list_window::list_h_for(tp.rows.len(), h);

        frame.render_widget(Clear, ov_rect);
        frame.buffer_mut().set_style(ov_rect, cs.ov_fill);
        let mut block = Block::default().borders(Borders::ALL).title(" Select Theme ")
            .border_style(cs.overlay_border);
        if let Some(ind) = windowed_indicator(tp.selected, tp.rows.len(), list_h) {
            block = block.title_bottom(ind);
        }
        frame.render_widget(block, ov_rect);

        if ov_h >= 3 {
            let query_area = Rect::new(ov_x + 1, ov_y + 1, ov_w.saturating_sub(2), 1);
            let query_display = format!("> {}", tp.query);
            let truncated_q: String = query_display.chars().take(query_area.width as usize).collect();
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(truncated_q, cs.ov_query))),
                query_area,
            );

            if ov_h >= 4 && list_h > 0 {
                let list_h_u16 = list_h as u16;
                let list_area = Rect::new(ov_x + 1, ov_y + 2, ov_w.saturating_sub(2), list_h_u16);
                let highlight_style = cs.overlay_selected;
                let end = (tp.scroll_top + list_h).min(tp.rows.len());
                let items: Vec<ListItem> = tp.rows[tp.scroll_top..end].iter().map(|name| {
                    let truncated: String = name.chars().take(list_area.width as usize).collect();
                    ListItem::new(Line::from(truncated))
                }).collect();

                let mut list_state = ListState::default();
                list_state.select(if tp.rows.is_empty() {
                    None
                } else {
                    Some(tp.selected.saturating_sub(tp.scroll_top))
                });

                frame.render_stateful_widget(
                    List::new(items).highlight_style(highlight_style),
                    list_area,
                    &mut list_state,
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // File browser overlay (drawn on top of everything else)
    // -----------------------------------------------------------------------
    // A6 self-heal: the window must respect the LIVE frame's geometry (resize
    // has no overlay hook; render is the one place that always sees the truth).
    if let Some(fb) = editor.file_browser.as_mut() {
        crate::app::keep_overlay_visible(h, fb.selected, fb.entries.len(), &mut fb.scroll_top);
    }
    if let Some(ref fb) = editor.file_browser {
        let ov_rect = palette_overlay_rect(area, fb.entries.len());
        let ov_x = ov_rect.x;
        let ov_y = ov_rect.y;
        let ov_w = ov_rect.width;
        let ov_h = ov_rect.height;
        let list_h = crate::list_window::list_h_for(fb.entries.len(), h);

        frame.render_widget(Clear, ov_rect);
        frame.buffer_mut().set_style(ov_rect, cs.ov_fill);
        let title = format!(" Open: {} ", fb.dir.display());
        let mut block = Block::default().borders(Borders::ALL).title(title)
            .border_style(cs.overlay_border);
        // Indicator composes with the existing dynamic title (file browser already uses top title).
        if let Some(ind) = windowed_indicator(fb.selected, fb.entries.len(), list_h) {
            block = block.title_bottom(ind);
        }
        frame.render_widget(block, ov_rect);

        if ov_h >= 3 {
            let query_area = Rect::new(ov_x + 1, ov_y + 1, ov_w.saturating_sub(2), 1);
            let query_display = format!("> {}", fb.query);
            let truncated_q: String = query_display.chars().take(query_area.width as usize).collect();
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(truncated_q, cs.ov_query))),
                query_area,
            );

            if ov_h >= 4 && list_h > 0 {
                let list_h_u16 = list_h as u16;
                let list_area = Rect::new(ov_x + 1, ov_y + 2, ov_w.saturating_sub(2), list_h_u16);
                let highlight_style = cs.overlay_selected;
                let end = (fb.scroll_top + list_h).min(fb.entries.len());
                let items: Vec<ListItem> = fb.entries[fb.scroll_top..end].iter().map(|e| {
                    let label = if e.is_dir { format!("{}/", e.name) } else { e.name.clone() };
                    let truncated: String = label.chars().take(list_area.width as usize).collect();
                    ListItem::new(Line::from(truncated))
                }).collect();

                let mut list_state = ListState::default();
                list_state.select(if fb.entries.is_empty() {
                    None
                } else {
                    Some(fb.selected.saturating_sub(fb.scroll_top))
                });

                frame.render_stateful_widget(
                    List::new(items).highlight_style(highlight_style),
                    list_area,
                    &mut list_state,
                );
            }
        }
    }

    if editor.menu_bar_rows() == 1 {
        let menu_area = Rect::new(area.x, area.y, area.width, h.saturating_sub(1));
        // Full-width bar background: gaps between labels + the right side carry the
        // Chrome style; the per-label paints below overwrite their own rects (A2).
        let bar_row = Rect::new(area.x, area.y, area.width, 1);
        frame.buffer_mut().set_style(bar_row, cs.menu_closed);
        match editor.menu {
            Some(ref menu) if !menu.groups.is_empty() => {
                // Paint the menu bar (one label per category)
                let bar = menu_bar_layout(menu_area, &menu.groups);
                for (i, rect) in &bar {
                    let cat = menu.groups[*i].0;
                    let label = crate::menu::category_label_pub(cat);
                    let text = format!(" {label} ");
                    let style = if *i == menu.open {
                        cs.menu_open
                    } else {
                        cs.menu_closed
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
                                cs.menu_sel
                            } else {
                                cs.menu_norm
                            };
                            ListItem::new(format!(" {label} ")).style(style)
                        })
                        .collect();
                    frame.render_widget(List::new(items), drop_rect);
                }
            }
            _ => {
                // Inactive bar (pinned / auto-revealed / unbuilt placeholder): static
                // labels, all closed-style, no dropdown, no highlight.
                for (i, rect) in &menu_bar_layout_cats(menu_area, &crate::registry::MENU_ORDER) {
                    let label = crate::menu::category_label_pub(crate::registry::MENU_ORDER[*i]);
                    frame.render_widget(Paragraph::new(format!(" {label} ")).style(cs.menu_closed), *rect);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Diagnostic quick-fix overlay (drawn on top of everything else)
    // -----------------------------------------------------------------------
    if let Some(ref diag_ov) = editor.diag {
        let row_count = diag_ov.row_count();
        let ov_rect = palette_overlay_rect(area, row_count);
        let ov_x = ov_rect.x;
        let ov_y = ov_rect.y;
        let ov_w = ov_rect.width;
        let ov_h = ov_rect.height;

        frame.render_widget(Clear, ov_rect);
        frame.buffer_mut().set_style(ov_rect, cs.ov_fill);

        let title = format!(" {} ", diag_ov.anchor.message);
        let block = Block::default().borders(Borders::ALL).title(title)
            .border_style(cs.overlay_border);
        frame.render_widget(block, ov_rect);

        if ov_h >= 3 {
            let list_h = (row_count as u16).min(15).min(ov_h.saturating_sub(2));
            let list_area = Rect::new(ov_x + 1, ov_y + 1, ov_w.saturating_sub(2), list_h);
            let highlight_style = cs.overlay_selected;

            let n_sugg = diag_ov.anchor.suggestions.len();
            let items: Vec<ListItem> = (0..row_count).take(list_h as usize).map(|i| {
                let label = if i < n_sugg {
                    crate::diag_overlay::suggestion_label(&diag_ov.anchor.suggestions[i])
                } else if i == n_sugg {
                    "Ignore once".to_string()
                } else {
                    "Add to dictionary".to_string()
                };
                let truncated: String = label.chars().take(list_area.width as usize).collect();
                ListItem::new(Line::from(truncated))
            }).collect();

            let mut list_state = ListState::default();
            list_state.select(if row_count == 0 { None } else { Some(diag_ov.selected) });

            frame.render_stateful_widget(
                List::new(items).highlight_style(highlight_style),
                list_area,
                &mut list_state,
            );
        }
    }
}
