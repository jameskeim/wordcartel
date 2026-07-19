// render_overlays.rs — overlay and menu painters (moved from render.rs, Task 4).
// All logic is byte-identical to the inline code it replaced; the only changes
// are the module boundary, the added imports, and receiving `&ChromeStyles`
// instead of accessing the six chrome locals that existed in render.rs.

use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::{
    editor::Editor,
    render::ChromeStyles,
    chrome_geom::{
        menu_bar_layout, menu_bar_layout_cats, menu_dropdown_rect,
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
///
/// Width of the `"> "` query prefix — the SINGLE SOURCE shared by the query painter
/// (the `format!("> {}", …)` display strings below) and the caret placements (B11),
/// so painter and caret can never drift.
const OV_QUERY_PREFIX_COLS: u16 = 2;

pub(crate) fn paint(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    // Splash owns the frame (RENDER_ORDER[0]) — paint it and return, exactly as before.
    if editor.splash.is_some() {
        crate::render_overlays::paint_splash(frame, editor, cs);
        return;
    }
    // Walk the remaining Frame overlays in paint order. Each painter self-gates on its own
    // `if let Some(..)`, so an inactive overlay is a no-op (byte-identical to the old
    // sequential blocks). The always-on menu BAR chrome is NOT a table row — it is painted as
    // a standalone step pinned at the `Menu` slot, before the menu-dropdown painter, so it
    // sits at the same z-position it held today (after file_browser, before diag): palette/
    // outline/theme_picker/cursor_picker/file_browser paint UNDER the bar, diag OVER it.
    for id in &crate::overlays::RENDER_ORDER[1..] {
        if *id == crate::overlays::OverlayId::Menu {
            paint_menu_bar(frame, editor, cs); // chrome (out of table), pinned here
        }
        if let crate::overlays::RenderSite::Frame(f) = id.row().render {
            f(frame, editor, cs);
        }
    }
}

/// Splash painter (RENDER_ORDER[0]). Owns the whole frame; `paint` early-returns after this
/// so no other overlay paints while the splash is up.
pub(crate) fn paint_splash(frame: &mut Frame, editor: &mut Editor, _cs: &ChromeStyles) {
    crate::splash::paint(frame, editor);
}

#[allow(clippy::too_many_lines)] // single overlay's paint block, extracted verbatim
pub(crate) fn paint_palette(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
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

        // B11: place the caret mid-string at `palette.cursor` (a byte offset), not just at
        // the end of the query — the palette query is the only overlay with an interior cursor.
        // H7: sum in usize and guard BEFORE narrowing — an unbounded-paste query must hide
        // the caret, not overflow the `+` or truncate to a small column that passes `< width`.
        let caret_col = query_area.x as usize + OV_QUERY_PREFIX_COLS as usize
            + palette.query[..palette.cursor].chars().count();
        if caret_col < (query_area.x + query_area.width) as usize {
            frame.set_cursor_position(Position { x: caret_col as u16, y: query_area.y });
        }

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
}

pub(crate) fn paint_outline(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    let area = frame.area();
    let h = area.height;
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

            // B11: end-of-query caret (outline's `cursor` field is pinned to the end anyway).
            // H7: sum in usize and guard BEFORE narrowing (see the palette arm above).
            let caret_col = query_area.x as usize + OV_QUERY_PREFIX_COLS as usize
                + outline.query.chars().count();
            if caret_col < (query_area.x + query_area.width) as usize {
                frame.set_cursor_position(Position { x: caret_col as u16, y: query_area.y });
            }

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
}

pub(crate) fn paint_theme_picker(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    let area = frame.area();
    let h = area.height;
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

            // B11: end-of-query caret.
            // H7: sum in usize and guard BEFORE narrowing (see the palette arm above).
            let caret_col = query_area.x as usize + OV_QUERY_PREFIX_COLS as usize
                + tp.query.chars().count();
            if caret_col < (query_area.x + query_area.width) as usize {
                frame.set_cursor_position(Position { x: caret_col as u16, y: query_area.y });
            }

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
}

pub(crate) fn paint_cursor_picker(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    let area = frame.area();
    let h = area.height;
    // -----------------------------------------------------------------------
    // Cursor (caret-shape) picker overlay
    // -----------------------------------------------------------------------
    // A FIXED 7-row list, WINDOWED like every sibling overlay (Finding 1 — mirrors
    // theme_picker's A6 self-heal: re-window against the LIVE frame geometry every
    // render since resize has no overlay hook). The list sits between the top border and
    // a dedicated "Preview:" sample row on the second-to-last inner line; the sample-cell
    // caret is the SOLE on-screen caret while the picker is open (place_cursor suppresses
    // the editor caret via has_active_input_overlay), so `reconcile_cursor_style` morphs
    // THIS caret live as the selection changes (Fork 5-C). The overlay box is sized via
    // `n + 1` rows (palette_overlay_rect) to reserve room for the sample row below the
    // list; the resulting visible-list height equals `list_h_for(n, h)` exactly (the
    // `+1`/`+3`/`-3`/`-2` terms cancel — see `chrome_geom::cursor_picker_row_at`), so
    // windowing reuses the SAME list_h_for/keep_overlay_visible machinery as every
    // sibling. This geometry (list_top = ov_y + 1, sample_row = ov_y + ov_h - 2) is
    // shared with `chrome_geom::cursor_picker_row_at` — keep them in step.
    if let Some(cp) = editor.cursor_picker.as_mut() {
        crate::app::keep_overlay_visible(h, cp.selected, crate::cursor_picker::ROW_ACTIONS.len(), &mut cp.scroll_top);
    }
    if let Some(ref cp) = editor.cursor_picker {
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        let ov_rect = palette_overlay_rect(area, n + 1);
        let ov_x = ov_rect.x;
        let ov_y = ov_rect.y;
        let ov_w = ov_rect.width;
        let ov_h = ov_rect.height;
        let list_h = crate::list_window::list_h_for(n, h);

        frame.render_widget(Clear, ov_rect);
        frame.buffer_mut().set_style(ov_rect, cs.ov_fill);
        let mut block = Block::default().borders(Borders::ALL).title(" Caret ")
            .border_style(cs.overlay_border);
        if let Some(ind) = windowed_indicator(cp.selected, n, list_h) {
            block = block.title_bottom(ind);
        }
        frame.render_widget(block, ov_rect);

        if ov_h >= 3 {
            let list_top = ov_y + 1;
            let sample_row = ov_y + ov_h.saturating_sub(2);
            if list_h > 0 {
                let list_h_u16 = list_h as u16;
                let list_area = Rect::new(ov_x + 1, list_top, ov_w.saturating_sub(2), list_h_u16);
                let end = (cp.scroll_top + list_h).min(n);
                let items: Vec<ListItem> = crate::cursor_picker::ROW_ACTIONS[cp.scroll_top..end].iter()
                    .map(|(label, glyph, _, _)| {
                        let text = format!("{glyph}  {label}");
                        let truncated: String = text.chars().take(list_area.width as usize).collect();
                        ListItem::new(Line::from(truncated))
                    }).collect();
                let mut list_state = ListState::default();
                // Window-relative selection (the highlight-correctness fix — Finding 1):
                // an absolute `cp.selected` past the visible window must never clamp onto
                // a wrong rendered row.
                list_state.select(Some(cp.selected.saturating_sub(cp.scroll_top)));
                frame.render_stateful_widget(
                    List::new(items).highlight_style(cs.overlay_selected),
                    list_area,
                    &mut list_state,
                );
            }

            // Sample cell: a "Preview: <glyph>" line with the live caret parked on the glyph.
            // Placement is independent of scroll_top — it always sits right below the
            // windowed list, above the bottom border.
            let sample_area = Rect::new(ov_x + 1, sample_row, ov_w.saturating_sub(2), 1);
            let glyph = crate::cursor_picker::ROW_ACTIONS[cp.selected.min(n - 1)].1;
            let sample_label = format!("Preview: {glyph}");
            let truncated: String = sample_label.chars().take(sample_area.width as usize).collect();
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(truncated, cs.ov_query))),
                sample_area,
            );
            let caret_x = ov_x + 1 + "Preview: ".chars().count() as u16;
            if caret_x < ov_x + ov_w {
                frame.set_cursor_position(Position { x: caret_x, y: sample_row });
            }
        }
    }
}

pub(crate) fn paint_file_browser(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    let area = frame.area();
    let h = area.height;
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

            // B11: end-of-query caret.
            // H7: sum in usize and guard BEFORE narrowing (see the palette arm above).
            let caret_col = query_area.x as usize + OV_QUERY_PREFIX_COLS as usize
                + fb.query.chars().count();
            if caret_col < (query_area.x + query_area.width) as usize {
                frame.set_cursor_position(Position { x: caret_col as u16, y: query_area.y });
            }

            if ov_h >= 4 && list_h > 0 {
                let list_h_u16 = list_h as u16;
                let list_area = Rect::new(ov_x + 1, ov_y + 2, ov_w.saturating_sub(2), list_h_u16);
                let highlight_style = cs.overlay_selected;
                let end = (fb.scroll_top + list_h).min(fb.entries.len());
                let items: Vec<ListItem> = fb.entries[fb.scroll_top..end].iter().map(|e| {
                    let label = crate::file_browser::entry_label(e);
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

}

/// Always-on menu BAR chrome (out of the overlay table — painted whether or not `menu` is
/// `Some`). Pinned at the `Menu` slot of the RENDER_ORDER walk (spec §2.3.1). The DROPDOWN is
/// painted separately by `paint_menu_dropdown`.
pub(crate) fn paint_menu_bar(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    if editor.menu_bar_rows() != 1 { return; }
    let area = frame.area();
    let menu_area = crate::chrome_geom::menu_area(area);
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

/// The `Menu` row's Frame painter — the DROPDOWN only (self-gated on an open, non-empty menu).
/// Painted AFTER `paint_menu_bar` at the `Menu` slot so it sits over the bar chrome, exactly as
/// the fused block did today.
#[allow(clippy::too_many_lines)] // the menu dropdown paint block, extracted verbatim
pub(crate) fn paint_menu_dropdown(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    if editor.menu_bar_rows() != 1 { return; }
    // Guard: only an OPEN, non-empty menu has a dropdown (the outer `match editor.menu`
    // Some-arm condition in today's block).
    let open_nonempty = matches!(editor.menu, Some(ref m) if !m.groups.is_empty());
    if !open_nonempty { return; }
    let area = frame.area();
    let menu_area = crate::chrome_geom::menu_area(area);
    // The outer `menu` binding's only role: compute the dropdown rect.
    let drop = {
        let menu = editor.menu.as_ref().unwrap();
        menu_dropdown_rect(menu_area, &menu.groups, menu.open)
    };
    let Some(drop_rect) = drop else { return; };
    // Paint the dropdown for the open category
    // Two-layer windowing invariant: re-window against the live frame geometry
    // every render so a resize without an event hook is self-correcting.
    let scroll_top = {
        let m = editor.menu.as_mut().unwrap();
        let leaves_len = m.groups[m.open].1.len();
        let list_h = drop_rect.height as usize;
        // Reserve the bottom row for the n/total indicator when the category
        // overflows, so keep_visible guarantees the highlight is within the
        // rendered item rows — not hidden behind the indicator row.
        let overflows = leaves_len > list_h;
        let keep_h = if overflows { list_h.saturating_sub(1) } else { list_h };
        crate::list_window::keep_visible(m.highlighted, leaves_len, keep_h, &mut m.scroll_top);
        m.scroll_top
    };
    frame.render_widget(Clear, drop_rect);
    // Attached filled panel: fill the whole rect with the Muted panel bg so
    // the dropdown reads as one elevated surface extending from the bar (no box).
    frame.buffer_mut().set_style(drop_rect, cs.menu_norm);
    let (highlighted, leaves_len) = {
        let m = editor.menu.as_ref().unwrap();
        (m.highlighted, m.groups[m.open].1.len())
    };
    let list_h = drop_rect.height as usize;
    // Determine how many rows are available for items: if the dropdown overflows,
    // reserve the bottom row for the n/total indicator.
    let overflows = leaves_len > list_h;
    let item_rows = if overflows { list_h.saturating_sub(1) } else { list_h };
    let end = (scroll_top + item_rows).min(leaves_len);
    let leaves = &editor.menu.as_ref().unwrap().groups[editor.menu.as_ref().unwrap().open].1;
    let items: Vec<ListItem> = leaves[scroll_top..end]
        .iter()
        .enumerate()
        .map(|(row_in_window, (label, _))| {
            let abs_row = scroll_top + row_in_window;
            let style = if abs_row == highlighted {
                cs.menu_sel
            } else {
                cs.menu_norm
            };
            ListItem::new(format!(" {label} ")).style(style)
        })
        .collect();
    // Render items in a sub-rect (leaving the bottom row for the indicator when needed).
    let item_rect = if overflows && list_h > 0 {
        Rect::new(drop_rect.x, drop_rect.y, drop_rect.width, item_rows as u16)
    } else {
        drop_rect
    };
    frame.render_widget(List::new(items), item_rect);
    // Render n/total indicator on the bottom row of the dropdown when it overflows.
    if overflows && list_h > 0 {
        if let Some(ind) = windowed_indicator(highlighted, leaves_len, list_h) {
            let ind_y = drop_rect.y + drop_rect.height - 1;
            let ind_rect = Rect::new(drop_rect.x, ind_y, drop_rect.width, 1);
            frame.render_widget(
                Paragraph::new(ind).style(cs.menu_norm),
                ind_rect,
            );
        }
    }
}

#[allow(clippy::too_many_lines)] // single overlay's paint block, extracted verbatim
pub(crate) fn paint_diag(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    let area = frame.area();
    let h = area.height;
    // -----------------------------------------------------------------------
    // Diagnostic quick-fix overlay (drawn on top of everything else)
    // -----------------------------------------------------------------------
    // A6 self-heal: the window must respect the LIVE frame's geometry (resize
    // has no overlay hook; render is the one place that always sees the truth).
    if let Some(d) = editor.diag.as_mut() {
        crate::app::keep_overlay_visible(h, d.selected, d.row_count(), &mut d.scroll_top);
    }
    if let Some(ref diag_ov) = editor.diag {
        let row_count = diag_ov.row_count();
        let ov_rect = palette_overlay_rect(area, row_count);
        let ov_x = ov_rect.x;
        let ov_y = ov_rect.y;
        let ov_w = ov_rect.width;
        let ov_h = ov_rect.height;
        let list_h = crate::list_window::list_h_for(row_count, h);

        frame.render_widget(Clear, ov_rect);
        frame.buffer_mut().set_style(ov_rect, cs.ov_fill);

        let title = format!(" {} ", diag_ov.anchor.message);
        let mut block = Block::default().borders(Borders::ALL).title(title)
            .border_style(cs.overlay_border);
        if let Some(ind) = windowed_indicator(diag_ov.selected, row_count, list_h) {
            block = block.title_bottom(ind);
        }
        frame.render_widget(block, ov_rect);

        if ov_h >= 3 && list_h > 0 {
            let list_h_u16 = list_h as u16;
            let list_area = Rect::new(ov_x + 1, ov_y + 1, ov_w.saturating_sub(2), list_h_u16);
            let highlight_style = cs.overlay_selected;
            let scroll_top = diag_ov.scroll_top;
            let end = (scroll_top + list_h).min(row_count);

            let n_sugg = diag_ov.anchor.suggestions.len();
            let items: Vec<ListItem> = (scroll_top..end).map(|i| {
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
            list_state.select(if row_count == 0 {
                None
            } else {
                Some(diag_ov.selected.saturating_sub(scroll_top))
            });

            frame.render_stateful_widget(
                List::new(items).highlight_style(highlight_style),
                list_area,
                &mut list_state,
            );
        }
    }
}
