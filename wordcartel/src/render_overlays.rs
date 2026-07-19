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
        palette_overlay_rect, windowed_indicator, file_browser_list_h, file_browser_overlay_rect,
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

/// Truncate a footer PATH to `width` columns from the LEFT, preserving the TAIL and marking
/// the elision — the opposite of the `.chars().take(n)` right-truncation used elsewhere in this
/// file for query/list text.
///
/// A path's highest-value component is its filename, which sits at the far RIGHT; the leading
/// directories are the most expendable part. Right-truncating (as this footer briefly did)
/// silently drops the filename — the one piece of the "where will this land" disclosure that
/// matters most, and the whole reason the footer exists. `width == 0` yields nothing; `width ==
/// 1` yields the marker alone, since even one column of real path text plus a marker cannot fit.
fn elide_path_left(line: &str, width: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= width { return line.to_string(); }
    if width == 0 { return String::new(); }
    if width == 1 { return "\u{2026}".to_string(); }
    let keep = width - 1; // reserve one column for the leading "…" marker
    let tail: String = chars[chars.len() - keep..].iter().collect();
    format!("\u{2026}{tail}")
}

/// Paint a prompt's `detail` disclosure box directly above the status row.
///
/// **Not an `OVERLAYS`-table painter.** The prompt's row in `overlays.rs` is, and stays,
/// `RenderSite::StatusRow`: its question and choices are painted on the status row by
/// `render::paint_status`, which calls this at the end of its own pass. The box is body
/// painted *for* that one overlay, not a second render site — `RenderSite` keeps its
/// single-valued axis and H21's render-coverage test is untouched (spec §5.3 as amended by
/// C5 §11.3).
///
/// A prompt with an **empty `detail` paints nothing at all**, so every prompt that has no
/// disclosure renders exactly as it did before this seam existed.
///
/// Degenerate geometry is handled by refusing to paint rather than by clamping into a
/// zero-sized rect: `prompt_detail_rect` returns `None` when fewer than three rows are free
/// above the status row. When the box is shorter than `detail`, the last visible row becomes
/// an `…and N more` count so a truncated disclosure announces itself instead of just
/// stopping.
///
/// **Which end a too-long line loses depends on its indent**, and the rule is the same one
/// the eye already reads off the box: a **flush-left line is a heading** (`Keeping 18 that
/// may hold unsaved work:`) whose meaning is at its START, so it truncates on the right; an
/// **indented line is an item** — path-shaped, with the filename and the trailing age at its
/// END — so it elides from the LEFT via `elide_path_left`. Eliding a heading from the left
/// produced the observed `…ng 18 that may hold unsaved work:` on a 60-column terminal.
pub(crate) fn paint_prompt_detail(frame: &mut Frame, prompt: &crate::prompt::Prompt,
    area: Rect, status_row: u16, cs: &ChromeStyles)
{
    let lines = &prompt.detail;
    let Some(ov_rect) = crate::chrome_geom::prompt_detail_rect(area, status_row, lines.len())
    else { return };

    frame.render_widget(Clear, ov_rect);
    frame.buffer_mut().set_style(ov_rect, cs.ov_fill);
    frame.render_widget(
        Block::default().borders(Borders::ALL).border_style(cs.overlay_border),
        ov_rect,
    );

    let inner_w = ov_rect.width.saturating_sub(2) as usize;
    let body_h = ov_rect.height.saturating_sub(2) as usize;
    for i in 0..body_h {
        // The final visible row turns into a count when the box could not hold the rest.
        // The count is orphan ITEMS, never `detail` lines: line 0 is the heading (never an
        // orphan), and a dropped line may itself already be `clean_recovery`'s own elision
        // (`  …and N more`) — that one line speaks for N orphans, not one, so it is weighed
        // by its own count rather than counted as a single entry. Indented like every other
        // item, matching `clean_recovery`'s own elision line rather than sitting flush like
        // a heading.
        let text = if body_h < lines.len() && i + 1 == body_h {
            let orphans: usize = lines[i.max(1)..].iter().map(|l| elided_weight(l)).sum();
            format!("  \u{2026}and {orphans} more")
        } else {
            lines[i].clone()
        };
        let fitted = if text.starts_with(' ') {
            elide_path_left(&text, inner_w)          // item: keep the filename and the age
        } else {
            text.chars().take(inner_w).collect()     // heading: keep the opening words
        };
        let y = ov_rect.y + 1 + i as u16;
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(fitted, cs.ov_query))),
            Rect::new(ov_rect.x + 1, y, inner_w as u16, 1),
        );
    }
}

/// How many orphan items one `detail` line accounts for. An elision line — `…and N more`,
/// however indented — already speaks for `N` orphans; every other line names exactly one.
/// Lets the box-clamp count in `paint_prompt_detail` re-total the true unnamed remainder
/// when it clamps past `clean_recovery`'s own `KEPT_SHOWN` elision line, instead of
/// undercounting it as a single dropped entry.
fn elided_weight(line: &str) -> usize {
    line.trim_start()
        .strip_prefix("\u{2026}and ")
        .and_then(|rest| rest.strip_suffix(" more"))
        .and_then(|n| n.parse().ok())
        .unwrap_or(1)
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
        // Sizes the box to CONTENT — but reserves a row for the resolved-target footer when
        // one is showing, even for a listing too small to otherwise need it (an empty or
        // freshly created directory). Single-sourced with `chrome_geom::file_browser_row_at`
        // so hit-testing can never disagree with what gets painted here (the A21 hazard).
        let ov_rect = file_browser_overlay_rect(area, fb);
        let ov_x = ov_rect.x;
        let ov_y = ov_rect.y;
        let ov_w = ov_rect.width;
        let ov_h = ov_rect.height;
        // The list's ACTUAL row budget — single-sourced with `chrome_geom::file_browser_row_at`
        // so hit-testing can never disagree with what gets painted here (the A21 hazard).
        let list_h = file_browser_list_h(area, fb) as usize;

        // The resolved-target footer: the post-policy, post-resolution absolute write target,
        // shown live so a writer never saves not knowing where it went (§ the reason this task
        // exists). `None` in select mode or with an empty field. Rendered against the real
        // filesystem — this is a read-only display probe, not a fault-injectable write path.
        let footer = crate::file_browser::footer_target(&crate::fsx::RealFs, fb);
        // How many dedicated footer rows the BOX was actually sized for — read from the same
        // ledger that sized it, never re-derived from a height guard here, or the painter and
        // the geometry could disagree about the row the mouse hit-test skips. Gating on the
        // TERMINAL'S available height rather than on how many entries the directory happens to
        // contain is the point: an empty directory on an ordinary terminal still gets its own
        // row, not the cramped border-title fallback. Only a genuinely tiny terminal falls
        // back to the title.
        let footer_rows = crate::chrome_geom::file_browser_footer_rows_shown(area, fb);
        let dedicated_footer_row = footer.is_some() && footer_rows >= 1;
        let footer_takes_title = footer.is_some() && !dedicated_footer_row;

        frame.render_widget(Clear, ov_rect);
        frame.buffer_mut().set_style(ov_rect, cs.ov_fill);
        let title = format!(" Open: {} ", fb.dir.display());
        let mut block = Block::default().borders(Borders::ALL).title(title)
            .border_style(cs.overlay_border);
        if footer_takes_title {
            // No spare interior row: the footer — the safety disclosure that prevents
            // save-to-nowhere — wins the block's bottom edge over the n/total indicator,
            // which is mere navigational polish.
            if let Some(ref line) = footer {
                let truncated = elide_path_left(line, ov_w.saturating_sub(2) as usize);
                block = block.title_bottom(Line::from(truncated));
            }
        } else if let Some(ind) = windowed_indicator(fb.selected, fb.entries.len(), list_h) {
            // Indicator composes with the existing dynamic title (file browser already uses top title).
            block = block.title_bottom(ind);
        }
        frame.render_widget(block, ov_rect);

        if ov_h >= 3 {
            let query_area = Rect::new(ov_x + 1, ov_y + 1, ov_w.saturating_sub(2), 1);
            // The MODE's own text source, never `fb.query` unconditionally: destination mode
            // types into `fb.mode`'s `field`, so painting the query row from `fb.query` left a
            // writer naming a Save-As/Write-Block/Export target typing into an invisible
            // string — the field, the caret and the whole `field_cursor` machinery had no
            // visual representation at all (C5 review finding C1). `filter_text` is the same
            // accessor the listing filter reads, so the row can never show text the filter is
            // not using. An empty field paints as a bare `> `, which §7.2's Row 2 safety
            // rationale requires to be VISIBLY empty.
            let field_text = fb.mode.filter_text(&fb.query);
            let query_display = format!("> {field_text}");
            let truncated_q: String = query_display.chars().take(query_area.width as usize).collect();
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(truncated_q, cs.ov_query))),
                query_area,
            );

            // B11: the text caret — at the end of a select-mode query, at `field_cursor` in
            // destination mode (where Left/Right/Home actually move it).
            // H7: sum in usize and guard BEFORE narrowing (see the palette arm above).
            let caret_col = query_area.x as usize + OV_QUERY_PREFIX_COLS as usize
                + fb.mode.caret_chars(&fb.query);
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

            // The footer's dedicated row: the last interior row, immediately above the bottom
            // border — full box width, so a long absolute path is not truncated to whatever a
            // border title's corners leave (unlike `footer_takes_title`'s cramped fallback
            // above). TEXT carries the meaning here (the arrow, the "exists" note), never
            // colour alone — the terminal-plain / no-color constraint.
            if dedicated_footer_row {
                if let Some(ref line) = footer {
                    let footer_row = ov_y + 2 + list_h as u16;
                    let footer_area = Rect::new(ov_x + 1, footer_row, ov_w.saturating_sub(2), 1);
                    let truncated = elide_path_left(line, footer_area.width as usize);
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(truncated, cs.ov_query))),
                        footer_area,
                    );
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::file_browser::{BrowseMode, DestinationPurpose, FileBrowser};
    use ratatui::{Terminal, backend::TestBackend};

    fn row_text(term: &Terminal<TestBackend>, y: u16) -> String {
        let buf = term.backend().buffer();
        let w = buf.area().width;
        (0..w).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// The rectangle the painter ACTUALLY drew, recovered from the cell grid by finding the
    /// box-drawing corners. Nothing else paints on these fresh single-widget backends, so the
    /// four corner glyphs occur exactly once each. Panics with the screen dump if a corner is
    /// missing — a silent `None` here would turn a real geometry regression into a skipped
    /// assertion, which is the failure mode this helper exists to close.
    fn drawn_box_rect(term: &Terminal<TestBackend>) -> Rect {
        let buf = term.backend().buffer();
        let (w, h) = (buf.area().width, buf.area().height);
        let find = |glyph: &str| -> (u16, u16) {
            for y in 0..h {
                for x in 0..w {
                    if buf[(x, y)].symbol() == glyph { return (x, y); }
                }
            }
            panic!("the painter drew no {glyph} corner; screen:\n{}",
                (0..h).map(|y| row_text(term, y)).collect::<Vec<_>>().join("\n"));
        };
        let (x0, y0) = find("\u{250c}");                 // ┌
        let (x1, _)  = find("\u{2510}");                 // ┐
        let (_, y1)  = find("\u{2514}");                 // └
        Rect::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1)
    }

    fn empty_destination_fb(dir: std::path::PathBuf, field: &str) -> FileBrowser {
        FileBrowser {
            dir, query: String::new(),
            mode: BrowseMode::Destination {
                purpose: DestinationPurpose::SaveAs,
                field: field.into(), field_cursor: field.len(),
            },
            listing: vec![], total_seen: 0, unreadable: 0, entries: vec![],
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None, navigated_name: None,
        }
    }

    // ---- `elide_path_left` ---------------------------------------------------------

    #[test]
    fn elide_path_left_keeps_the_tail_and_marks_the_elision() {
        let long = "/home/writer/projects/my-book/drafts-v2/chapter one.md";
        let got = elide_path_left(long, 24);
        assert_eq!(got.chars().count(), 24, "fits exactly in the given width: {got}");
        assert!(got.starts_with('\u{2026}'), "the elision is marked with a leading ellipsis: {got}");
        assert!(got.ends_with("chapter one.md"),
            "the FILENAME — the highest-value part of the path — survives: {got}");
        assert!(!got.contains("home"),
            "the leading, most-expendable directories are what's dropped, not the tail: {got}");
    }

    #[test]
    fn elide_path_left_is_a_no_op_when_it_already_fits() {
        assert_eq!(elide_path_left("short.md", 40), "short.md");
    }

    #[test]
    fn elide_path_left_degenerate_widths_never_panic() {
        assert_eq!(elide_path_left("anything", 0), "", "zero columns show nothing");
        assert_eq!(elide_path_left("anything", 1), "\u{2026}", "one column shows only the marker");
    }

    // ---- the destination field reaches the SCREEN ---------------------------------

    /// C5 review finding C1. The intercept routes destination-mode typing into
    /// `BrowseMode::Destination`'s `field`, but the painter unconditionally drew
    /// `format!("> {}", fb.query)` — which stays empty in destination mode. A writer naming a
    /// Save-As target therefore typed into a string with NO visual representation: the query
    /// row showed a bare `> `, and the hardware caret stayed pinned at the prefix while
    /// Backspace/Left/Right edited text nobody could see. Spec §7.2 requires the writer to
    /// "see the name land in the field", and Row 2's safety rationale requires a VISIBLY
    /// empty field to distinguish itself from Row 4.
    ///
    /// Twenty-six tasks missed it because every field guard asserted `field` on the STRUCT or
    /// read the resolved-target footer. This one scrapes the drawn cell grid, drives every
    /// keystroke through the real intercept, and pumps the real async listing first — so it
    /// fails the moment the painter stops rendering the mode's own text source.
    ///
    /// FAIL-VERIFY (mutation): restore the painter's `format!("> {}", fb.query)` — the
    /// "chapter" assertion fails on a bare `> ` row. Separately restore the caret's
    /// `fb.query.chars().count()` — the caret assertion fails, reporting column 3 (the
    /// prefix) instead of 7. Both confirmed, then reverted.
    #[test]
    fn the_destination_field_and_its_caret_are_painted_in_the_query_row() {
        let d = std::env::temp_dir().join(format!("wc-render-field-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("fixture dir");
        let mut e = Editor::new_from_text("body\n", None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        // Opened with an EMPTY field — exactly what a writer sees invoking Save-As fresh.
        e.open_destination_picker(&fs, &tx, DestinationPurpose::SaveAs, d.clone(), String::new());
        crate::test_support::pump_listing(&mut e, &rx);

        let cs = ChromeStyles::build(&e.theme, e.depth, e.canvas);
        let area = Rect::new(0, 0, 80, 24);
        // The box's origin is re-read after every render, never cached: a non-empty field
        // brings the resolved-target footer with it, and the box grows for that row.
        let query_row = |e: &Editor| crate::chrome_geom::file_browser_overlay_rect(
            area, e.file_browser.as_ref().expect("picker open")).y + 1;

        // (a) An empty field paints a VISIBLY empty row — §7.2 Row 2's safety rationale.
        crate::derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        term.draw(|f| paint_file_browser(f, &mut e, &cs)).expect("draw");
        let empty_row = row_text(&term, query_row(&e));
        assert_eq!(empty_row.trim_matches(|c| c == ' ' || c == '\u{2502}'), ">",
            "an empty field must paint as a bare prompt, nothing more: {empty_row:?}");

        // (b) Type through the REAL intercept — no handler call, no struct poke.
        for c in ['c', 'h', 'a', 'p', 't', 'e', 'r'] {
            crate::test_support::press_char_fb(&mut e, &fs, &tx, c);
        }
        crate::derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        term.draw(|f| paint_file_browser(f, &mut e, &cs)).expect("draw");
        let row = row_text(&term, query_row(&e));
        assert!(row.contains("chapter"),
            "the typed filename must reach the SCREEN, not just `fb.mode`'s field: {row:?}");

        // (c) The caret tracks `field_cursor`, not the (empty) query. Two Lefts put it
        //     between `chapt` and `er`; the painter must place it five columns past the
        //     `> ` prefix, not at the prefix itself.
        for _ in 0..2 { crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Left); }
        crate::derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        term.draw(|f| paint_file_browser(f, &mut e, &cs)).expect("draw");
        let ov_x = {
            let fb = e.file_browser.as_ref().expect("picker open");
            crate::chrome_geom::file_browser_overlay_rect(area, fb).x
        };
        let expected = ov_x + 1 + OV_QUERY_PREFIX_COLS + 5;
        assert_eq!(term.get_cursor_position().expect("caret").x, expected,
            "the caret must sit at `field_cursor`, not pinned to the empty query");

        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- the footer GROWS the box; it does not confiscate a list row ---------------

    /// C5 review finding M5, found live at 100x30: a filtered destination listing with two
    /// entries painted only ONE of them plus a `1/2` windowed indicator. The footer's row was
    /// taken out of the list's budget rather than added to the box, so a picker whose whole
    /// job is to show a writer what they might clobber hid half of it — on a terminal with
    /// twenty spare rows. Superset of T20's deferred "a 1-row listing loses its only visible
    /// row" Minor, which is the degenerate case of the same arithmetic.
    ///
    /// FAIL-VERIFY (mutation): restore `file_browser_rows`'s pre-fix budget
    /// (`box_rows = raw.max(reserved)`, i.e. grow only when the content leaves nothing) —
    /// `chapter-two.md` vanishes from the screen and the `1/2` indicator appears. Confirmed,
    /// then reverted.
    #[test]
    fn a_two_entry_destination_listing_shows_both_entries_alongside_the_footer() {
        let dir = std::env::temp_dir().join(format!("wc-render-m5-{}", std::process::id()));
        let mut e = Editor::new_from_text("x\n", None, (100, 30));
        let mut fb = empty_destination_fb(dir, "chapter");
        fb.entries = ["chapter-one.md", "chapter-two.md"].iter().map(|n| {
            crate::file_browser::FileEntry {
                name: (*n).into(), kind: crate::fsx::EntryKind::File,
                is_symlink: false, broken: false,
            }
        }).collect();
        e.file_browser = Some(fb);
        crate::derive::rebuild(&mut e);

        let area = Rect::new(0, 0, 100, 30);
        assert_eq!(crate::chrome_geom::file_browser_list_h(area, e.file_browser.as_ref().expect("open")), 2,
            "the list keeps a row per entry — the footer's row comes from the box, not the list");

        let cs = ChromeStyles::build(&e.theme, e.depth, e.canvas);
        let mut term = Terminal::new(TestBackend::new(100, 30)).expect("test terminal");
        term.draw(|f| paint_file_browser(f, &mut e, &cs)).expect("draw");
        let screen = (0..30).map(|y| row_text(&term, y)).collect::<Vec<_>>().join("\n");

        assert!(screen.contains("chapter-one.md"), "the first entry is drawn:\n{screen}");
        assert!(screen.contains("chapter-two.md"),
            "and so is the second — the footer must not cost the writer an entry:\n{screen}");
        assert!(!screen.contains("1/2"),
            "and nothing is windowed away, so no n/total indicator is drawn:\n{screen}");
    }

    // ---- the height, not the listing size, gates the cramped fallback -------------

    #[test]
    fn an_empty_listing_still_gets_a_dedicated_footer_row_on_an_ordinary_terminal() {
        // IMPORTANT 2 — `footer_takes_title` used to be gated on the LISTING's size
        // (`list_h_for(fb.entries.len(), h)`), so a listing with zero entries — a freshly
        // created, still-empty project folder; a listing that has not arrived yet — forced
        // the footer into the cramped border-title path REGARDLESS of how spacious the
        // terminal actually was. An 80x24 terminal has ample room for a dedicated row.
        //
        // FAIL-VERIFY: revert `dedicated_footer_row` to `footer.is_some() && raw_list_h > 0`
        // where `raw_list_h = list_window::list_h_for(fb.entries.len(), h)` (the pre-fix
        // form), watch this fail — the footer text lands in the title row instead of its own
        // dedicated row. Confirmed, then restored.
        let dir = std::env::temp_dir().join(format!("wc-render-empty-{}", std::process::id()));
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.file_browser = Some(empty_destination_fb(dir.clone(), "new-chapter"));
        crate::derive::rebuild(&mut e);

        let area = Rect::new(0, 0, 80, 24);
        let ov_rect = {
            let fb = e.file_browser.as_ref().expect("open");
            crate::chrome_geom::file_browser_overlay_rect(area, fb)
        };
        assert!(ov_rect.height >= 4,
            "precondition: the box must grow to hold a dedicated footer row on this terminal");
        let title_row = ov_rect.y + ov_rect.height - 1;
        // list_h is 0 for an empty listing, so the dedicated row sits right after the query row.
        let footer_row = ov_rect.y + 2;

        let cs = ChromeStyles::build(&e.theme, e.depth, e.canvas);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| paint_file_browser(f, &mut e, &cs)).unwrap();

        assert!(row_text(&term, footer_row).contains("new-chapter.md"),
            "the footer text must land in its OWN dedicated row: {:?}", row_text(&term, footer_row));
        assert!(!row_text(&term, title_row).contains("new-chapter"),
            "and must NOT be squeezed into the border title instead: {:?}", row_text(&term, title_row));
    }

    // ---- the DRAWN box is the one chrome_geom describes ---------------------------

    #[test]
    fn the_painted_box_occupies_exactly_file_browser_overlay_rect() {
        // `file_browser_overlay_rect` is the single source the footer reservation, the mouse
        // hit-test (`file_browser_row_at`) and its inverse (`file_browser_row_origin`) all read
        // the box geometry from. That contract is worth only as much as the painter's actual
        // obedience to it: every other test here scrapes CONTENT at a row derived from the same
        // function, so a painter that computed its own — even a differently-WRONG own — box
        // could still line those assertions up. This one scrapes the drawn border itself.
        //
        // Both fixtures are checked because they discriminate different mistakes. The empty
        // destination listing is where `file_browser_overlay_rect` and the plain
        // `palette_overlay_rect(area, entries.len())` it wraps genuinely DISAGREE (the footer
        // reservation grows the box), so it catches a painter that dropped the reservation. The
        // populated select listing pins x/width/height for the ordinary case, where the two
        // agree on height and only a hand-rolled inset would show up.
        //
        // FAIL-VERIFY: replace the painter's `file_browser_overlay_rect(area, fb)` with
        // `palette_overlay_rect(area, fb.entries.len())` — the empty case fails on height.
        // Then instead inset it by one column (`ov_rect.x + 1`) — both cases fail on x.
        // Confirmed for both, then restored.
        let dir = std::env::temp_dir().join(format!("wc-render-rect-{}", std::process::id()));
        let area = Rect::new(0, 0, 80, 24);

        let mut empty = Editor::new_from_text("x\n", None, (80, 24));
        empty.file_browser = Some(empty_destination_fb(dir.clone(), "new-chapter"));

        let mut populated = Editor::new_from_text("x\n", None, (80, 24));
        let mut fb = empty_destination_fb(dir.clone(), "");
        fb.mode = BrowseMode::Select;
        fb.entries = (0..7).map(|i| crate::file_browser::FileEntry {
            name: format!("chapter-{i}.md"), kind: crate::fsx::EntryKind::File,
            is_symlink: false, broken: false,
        }).collect();
        populated.file_browser = Some(fb);

        for (label, e) in [("empty destination", &mut empty), ("populated select", &mut populated)] {
            crate::derive::rebuild(e);
            let expected = {
                let fb = e.file_browser.as_ref().expect("open");
                crate::chrome_geom::file_browser_overlay_rect(area, fb)
            };
            let cs = ChromeStyles::build(&e.theme, e.depth, e.canvas);
            let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
            term.draw(|f| paint_file_browser(f, e, &cs)).expect("draw");

            assert_eq!(drawn_box_rect(&term), expected,
                "{label}: the border the painter actually drew must BE \
                 file_browser_overlay_rect — the rect the hit-test and footer both trust");
        }
    }
}
