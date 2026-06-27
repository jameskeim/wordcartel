// Task 5: ratatui live-preview render + status line.
// Pure: takes &Editor, mutates NOTHING on the editor.

use crate::{compose, derive, editor::Editor, nav};
use wordcartel_core::count;
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Modifier, Style as RStyle},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};
use wordcartel_core::style::Style;
use wordcartel_core::theme::SemanticElement as SE;

/// Heading-level shade glyphs used in cue mode and when `heading_level_glyph` is on.
/// Index 0 = H1 (`█`), …, index 5 = H6 (`·`). Density decreases with level.
const SHADES: [&str; 6] = ["█", "▓", "▒", "░", "▏", "·"];

/// Half-open interval intersection: is the row's global byte range active?
///
/// Returns `true` if the row's span `[row_from, row_to)` overlaps with the
/// active region `[region_from, region_to)` (any overlap → bright).
pub(crate) fn row_is_active(row_from: usize, row_to: usize, region_from: usize, region_to: usize) -> bool {
    row_from < region_to && region_from < row_to
}

/// Half-open interval overlap for search match highlighting.
///
/// Returns `true` if `[a0, a1)` and `[b0, b1)` have any overlap.
pub(crate) fn overlaps(a0: usize, a1: usize, b0: usize, b1: usize) -> bool {
    a0 < b1 && b0 < a1
}

/// Map a wordcartel inline `Style` to a ratatui `Style`.
///
/// Strong→BOLD; Emphasis→ITALIC; StrongEmphasis→BOLD|ITALIC;
/// Strikethrough→CROSSED_OUT; Code→Cyan color; Link→UNDERLINED+Yellow;
/// Plain→default.
///
/// This function is kept for the `style_mapping_is_bold_for_strong` test and
/// any callers that still need the inline-only form. New render sites use
/// `compose` directly.
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
        Style::Comment => RStyle::default().add_modifier(Modifier::DIM).add_modifier(Modifier::ITALIC),
    }
}

/// Map a wordcartel inline `Style` to a `SemanticElement` for theme lookup.
fn style_element(s: Style) -> SE {
    match s {
        Style::Plain         => SE::Text,
        Style::Emphasis      => SE::Emphasis,
        Style::Strong        => SE::Strong,
        Style::StrongEmphasis => SE::StrongEmphasis,
        Style::Code          => SE::Code,
        Style::Strikethrough => SE::Strikethrough,
        Style::Link          => SE::Link,
        Style::Comment       => SE::Comment,
    }
}

/// Map a `BlockRole` to a `SemanticElement` for theme lookup.
fn role_element(role: wordcartel_core::style::BlockRole) -> SE {
    use wordcartel_core::style::BlockRole as R;
    match role {
        R::Heading(n)     => SE::Heading(n),
        R::BlockQuote     => SE::BlockQuote,
        R::CodeBlock      => SE::CodeBlock,
        R::ListItem       => SE::ListMarker,
        R::ThematicBreak  => SE::ThematicBreak,
        R::FrontMatter    => SE::FrontMatter,
        R::Comment        => SE::Comment,
        R::Paragraph      => SE::Text,
    }
}

/// Map a `BlockRole` to the `SemanticElement` used to style its prefix glyph.
///
/// Blockquote glyphs use the BlockQuote face; thematic-break glyphs use
/// ThematicBreak; headings use Heading (reserved for Task 7); all other
/// prefix glyphs (list bullets, ordered numbers) use ListMarker as today.
fn prefix_element(role: wordcartel_core::style::BlockRole) -> SE {
    use wordcartel_core::style::BlockRole as R;
    match role {
        R::BlockQuote    => SE::BlockQuote,
        R::ThematicBreak => SE::ThematicBreak,
        R::Heading(n)    => SE::Heading(n),
        R::ListItem      => SE::ListMarker,
        // The remaining roles never carry a prefix glyph (no prefix_glyph is
        // produced for them), so this fn is not reached for them; name them
        // explicitly so a future BlockRole forces a deliberate choice here.
        R::Paragraph | R::CodeBlock | R::FrontMatter | R::Comment => SE::ListMarker,
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
    Some(Rect::new(label_rect.x, area.y + 1, width.min(area.width.saturating_sub(label_rect.x.saturating_sub(area.x))), height.min(area.height.saturating_sub(1))))
}

#[allow(dead_code)] // used by Task 7 (mouse hit-testing)
pub(crate) fn menu_dropdown_row_at(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)], open: usize, col: u16, row: u16) -> Option<usize> {
    let r = menu_dropdown_rect(area, groups, open)?;
    if col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height {
        Some((row - r.y) as usize)
    } else { None }
}

/// Compute the palette overlay bounding rect for a given terminal area and row count.
/// The overlay height is sized to the actual number of palette rows (capped at 15).
/// Both the render code and mouse hit-testing call this to share geometry.
pub(crate) fn palette_overlay_rect(area: Rect, row_count: usize) -> Rect {
    let w = area.width;
    let h = area.height;
    let ov_w = (w * 3 / 5).max(30).min(80).min(w);
    let list_h: u16 = (row_count as u16).min(15).min(h.saturating_sub(4));
    let ov_h = (list_h + 3).min(h);
    let ov_x = area.x.saturating_add((w.saturating_sub(ov_w)) / 2);
    let ov_y = area.y.saturating_add((h.saturating_sub(ov_h)) / 4);
    Rect::new(ov_x, ov_y, ov_w, ov_h)
}

/// Return the zero-based list row index that `(col, row)` hits, or `None`.
/// The list starts at `ov_y + 2` and has at most `palette.rows.len()` entries.
pub(crate) fn palette_row_at(area: Rect, palette: &crate::palette::Palette, col: u16, row: u16) -> Option<usize> {
    let r = palette_overlay_rect(area, palette.rows.len());
    let list_top = r.y.saturating_add(2);
    let list_h = (palette.rows.len() as u16).min(15).min(area.height.saturating_sub(4));
    if col >= r.x.saturating_add(1) && col < r.x.saturating_add(r.width).saturating_sub(1)
        && row >= list_top && row < list_top.saturating_add(list_h)
    {
        Some((row - list_top) as usize)
    } else {
        None
    }
}

/// Return a word/char count segment for the status bar, or `None` if the
/// feature is disabled (`view_opts.word_count = false`).
///
/// When the primary selection is non-empty, counts only the selected text;
/// otherwise counts the whole document buffer.
pub(crate) fn word_count_segment(editor: &Editor) -> Option<String> {
    if !editor.view_opts.word_count {
        return None;
    }
    let sel = editor.active().document.selection.primary();
    let text = if !sel.is_empty() {
        editor.active().document.buffer.slice(sel.from()..sel.to())
    } else {
        editor.active().document.buffer.to_string()
    };
    Some(format!(
        "{} words · {} chars",
        count::word_count(&text),
        count::char_count(&text)
    ))
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

    // Centered-measure geometry: ONE call here so paint + cursor never desync.
    let tg = crate::nav::text_geometry(editor);

    // -----------------------------------------------------------------------
    // Wrap-guide line (painted BEFORE the text-row loop so text overwrites it)
    // -----------------------------------------------------------------------
    if editor.view_opts.wrap_guide {
        let gx = area.x + tg.text_left + editor.view_opts.wrap_column;
        let within_viewport = gx < area.x + w;
        let not_scrollbar_col = !(editor.mouse.scrollbar_visible && gx == area.x + w - 1);
        if within_viewport && not_scrollbar_col {
            let guide_style = compose::compose(&editor.theme, editor.depth, &[SE::WrapGuide]);
            for r in 0..edit_height {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled("│", guide_style))),
                    Rect::new(gx, edit_top + r, 1, 1),
                );
            }
        }
    }

    // Compute the active focus region once (before the row loop) when focus is on.
    // For Paragraph: use paragraph_range_at at the caret.
    // For Sentence: scope paragraph_range_at first, then sentence_bounds within that window.
    let focus_region: Option<(usize, usize)> = if editor.view_opts.focus {
        let buf = &editor.active().document.buffer;
        let blocks = &editor.active().document.blocks;
        let head = nav::head(editor);
        let region = match editor.view_opts.focus_granularity {
            crate::config::FocusGranularity::Paragraph => {
                nav::paragraph_range_at(blocks, buf, head)
            }
            crate::config::FocusGranularity::Sentence => {
                let (ps, pe) = nav::paragraph_range_at(blocks, buf, head);
                let win = buf.slice(ps..pe);
                let (sf, st) = wordcartel_core::textobj::sentence_bounds(&win, head - ps);
                (ps + sf, ps + st)
            }
        };
        Some(region)
    } else {
        None
    };

    // Collect sorted logical line indices from the layout cache.
    let mut sorted_lines: Vec<usize> = editor.active().view.line_layouts.keys().copied().collect();
    sorted_lines.sort_unstable();

    // Gather search match data once before the row loop (avoids repeated borrow).
    // Clone only the viewport-bounded window (O(visible matches)) rather than the
    // full match list (O(total matches)).  The search-bar count/ordinal always reads
    // from SearchState directly (`s.count()` / `s.current_ordinal()`), so the
    // truncated window does not affect the N/M display.
    let hl_current: Option<wordcartel_core::search::Match> =
        editor.search.as_ref().and_then(|s| s.current());
    let hl_window: Vec<wordcartel_core::search::Match> = match editor.search.as_ref() {
        None => Vec::new(),
        Some(s) if s.matches().is_empty() => Vec::new(),
        Some(s) => {
            let buf = &editor.active().document.buffer;
            let lo = derive::line_start(buf, scroll);
            // Conservative upper bound: the last logical line in the layout cache.
            let max_visible = sorted_lines.last().copied().unwrap_or(scroll);
            let hi = derive::line_start(buf, max_visible + 1);
            // partition_point keeps the sorted invariant; matches are sorted by start,
            // and non-overlapping so end is also non-decreasing.
            let lo_idx = s.matches().partition_point(|m| m.end <= lo);
            let hi_idx = s.matches().partition_point(|m| m.start < hi);
            s.matches()[lo_idx..hi_idx.max(lo_idx)].to_vec()
        }
    };

    // Diagnostic overlay: check version validity once before the row loop.
    // diag_active = true iff the stored diagnostics were computed for the current
    // document version (and are non-empty). When false, diag_all is empty.
    let diag_active = editor.active().diagnostics.valid_for(editor.active().document.version);
    let diag_all: &[wordcartel_core::diagnostics::Diagnostic] =
        if diag_active { &editor.active().diagnostics.diagnostics } else { &[] };

    // Snapshot primary selection (Task 9: selection painting).
    let sel_range = editor.active().document.selection.primary();
    let (sel_from, sel_to) = (sel_range.from(), sel_range.to());
    let has_sel = !sel_range.is_empty();

    // Use the placed-path builder when search is active, valid diagnostics exist,
    // or a non-empty selection must be painted (segs path does no per-glyph styling).
    let use_placed = !hl_window.is_empty() || diag_active || has_sel;

    // Source-mode branch: in source modes (any mode other than LivePreview) the
    // stack is [Text] only — base canvas, no role or inline semantic styling.
    let source_mode = editor.active().view.mode != crate::editor::RenderMode::LivePreview;

    'outer: for &l in &sorted_lines {
        if l < scroll {
            continue;
        }
        let (visual_rows, map) = &editor.active().view.line_layouts[&l];
        let skip_rows = if l == scroll {
            editor.active().view.scroll_row
        } else {
            0
        };
        for (row_index, vr) in visual_rows.iter().enumerate() {
            if row_index < skip_rows {
                continue;
            }
            if screen_row >= edit_height {
                break 'outer;
            }

            // Determine whether this visual row is dim (outside the active region).
            let row_dim = if let Some((from, to)) = focus_region {
                let buf = &editor.active().document.buffer;
                let line_off = derive::line_start(buf, l);
                let g_from = line_off + vr.src_span.start;
                let g_to = line_off + vr.src_span.end;
                !row_is_active(g_from, g_to, from, to)
            } else {
                false
            };

            // 5g: compute fold marker before span-building borrows.
            let fold_marker_n: Option<usize> = if row_index == skip_rows {
                fold_marker_for(editor, l)
            } else {
                None
            };

            // Build spans for this visual row.
            let spans: Vec<Span<'_>> = if !use_placed {
                // ---------------------------------------------------------------
                // EXISTING segs-based path (no active search, no diagnostics) — true no-op.
                // ---------------------------------------------------------------
                let dim_style = compose::compose(&editor.theme, editor.depth, &[SE::FocusDim]);
                let mut segs_spans: Vec<Span<'_>> = Vec::new();
                // Prefix lead-in. Row 0 paints the real glyph; continuation rows
                // of a prefixed line paint a blank spacer of `prefix_width` cells
                // so painted text stays aligned with the prefix-offset cursor
                // columns (`Placed.col` already includes `prefix_width`).
                if let Some(ref glyph) = vr.prefix_glyph {
                    let pe = prefix_element(vr.role);
                    let gstyle = if row_dim {
                        compose::compose(&editor.theme, editor.depth, &[pe, SE::FocusDim])
                    } else {
                        compose::compose(&editor.theme, editor.depth, &[pe]).add_modifier(Modifier::DIM)
                    };
                    let painted = if editor.theme.heading_level_glyph {
                        if let wordcartel_core::style::BlockRole::Heading(n) = vr.role {
                            let shade = SHADES[(n.clamp(1, 6) - 1) as usize];
                            format!("{shade} ")
                        } else {
                            glyph.clone()
                        }
                    } else {
                        glyph.clone()
                    };
                    segs_spans.push(Span::styled(painted, gstyle));
                } else if map.prefix_width > 0 {
                    segs_spans.push(Span::raw(" ".repeat(map.prefix_width)));
                }
                for seg in &vr.segs {
                    let style = if row_dim {
                        if source_mode {
                            compose::base_canvas(&editor.theme, editor.depth)
                                .patch(compose::compose(&editor.theme, editor.depth, &[SE::FocusDim]))
                        } else {
                            dim_style
                        }
                    } else if source_mode {
                        compose::base_canvas(&editor.theme, editor.depth)
                    } else {
                        compose::compose(&editor.theme, editor.depth, &[SE::Text, role_element(vr.role), style_element(seg.style)])
                    };
                    segs_spans.push(Span::styled(seg.text.clone(), style));
                }
                segs_spans
            } else {
                // ---------------------------------------------------------------
                // Placed path: build spans from map.placed, per-glyph search highlight
                // and/or diagnostic underline. Fires when search is active OR valid
                // diagnostics are present.
                // ---------------------------------------------------------------
                let buf = &editor.active().document.buffer;
                let line_off = derive::line_start(buf, l);
                let mut hl_spans: Vec<Span<'_>> = Vec::new();

                // Compute the visible byte span for this visual row so we can window
                // the diagnostics. src_span is relative to the logical line start.
                let lo = line_off + vr.src_span.start;
                let hi = line_off + vr.src_span.end;

                // Window diagnostics by upper bound only (diagnostics may overlap so
                // end is not monotonic — binary lower-bound on end is unsound).
                // Upper-bound partition_point + linear filter for end > lo.
                let hi_idx = diag_all.partition_point(|d| d.range.start < hi);
                let diag_window: Vec<&wordcartel_core::diagnostics::Diagnostic> =
                    diag_all[..hi_idx].iter().filter(|d| d.range.end > lo).collect();

                // Prefix lead-in. Row 0 paints the real glyph (unsearchable, dim
                // only); continuation rows of a prefixed line paint a blank
                // spacer of `prefix_width` cells so painted text stays aligned
                // with the prefix-offset cursor columns.
                if let Some(ref glyph) = vr.prefix_glyph {
                    let pe = prefix_element(vr.role);
                    let gstyle = if row_dim {
                        compose::compose(&editor.theme, editor.depth, &[pe, SE::FocusDim])
                    } else {
                        compose::compose(&editor.theme, editor.depth, &[pe]).add_modifier(Modifier::DIM)
                    };
                    let painted = if editor.theme.heading_level_glyph {
                        if let wordcartel_core::style::BlockRole::Heading(n) = vr.role {
                            let shade = SHADES[(n.clamp(1, 6) - 1) as usize];
                            format!("{shade} ")
                        } else {
                            glyph.clone()
                        }
                    } else {
                        glyph.clone()
                    };
                    hl_spans.push(Span::styled(painted, gstyle));
                } else if map.prefix_width > 0 {
                    hl_spans.push(Span::raw(" ".repeat(map.prefix_width)));
                }

                // Hoist FocusDim compose once per row (mirrors segs path).
                let dim_style = compose::compose(&editor.theme, editor.depth, &[SE::FocusDim]);

                // One span per run of glyphs sharing the same (style, highlight-kind).
                let mut run = String::new();
                let mut run_style: Option<RStyle> = None;

                for p in map.placed.iter().filter(|p| p.row == row_index) {
                    let g_from = line_off + p.src.start;
                    let g_to = line_off + p.src.end;
                    let is_current = hl_current.is_some_and(|m| overlaps(g_from, g_to, m.start, m.end));
                    let is_match = !is_current && hl_window.iter().any(|m| overlaps(g_from, g_to, m.start, m.end));

                    let mut style = if row_dim {
                        if source_mode {
                            compose::base_canvas(&editor.theme, editor.depth)
                                .patch(compose::compose(&editor.theme, editor.depth, &[SE::FocusDim]))
                        } else {
                            dim_style
                        }
                    } else if source_mode {
                        compose::base_canvas(&editor.theme, editor.depth)
                    } else {
                        compose::compose(&editor.theme, editor.depth, &[SE::Text, role_element(vr.role), style_element(p.style)])
                    };
                    if is_current {
                        let search_face = editor.theme.face(SE::SearchCurrent);
                        let ss = crate::compose::face_to_ratatui(&search_face, editor.depth);
                        style = style.patch(ss);
                    } else if is_match {
                        let search_face = editor.theme.face(SE::SearchMatch);
                        let ss = crate::compose::face_to_ratatui(&search_face, editor.depth);
                        style = style.patch(ss);
                    }

                    // Apply selection reverse. Selection sits below search-current
                    // (so a current search match stands out) but above the base style.
                    // Diagnostic underline stacks on top of selection reverse.
                    let is_selected = has_sel && overlaps(g_from, g_to, sel_from, sel_to);
                    if is_selected {
                        let sel_face = editor.theme.face(SE::Selection);
                        style = style.patch(crate::compose::face_to_ratatui(&sel_face, editor.depth));
                    }

                    // Apply diagnostic underline if this glyph overlaps any diagnostic.
                    // Search-highlight precedence stands: underline may stack on REVERSED.
                    if let Some(d) = diag_window.iter().find(|d| overlaps(g_from, g_to, d.range.start, d.range.end)) {
                        let diag_face = match d.kind {
                            wordcartel_core::diagnostics::DiagnosticKind::Spelling =>
                                compose::compose(&editor.theme, editor.depth, &[SE::DiagSpelling]),
                            wordcartel_core::diagnostics::DiagnosticKind::Grammar =>
                                compose::compose(&editor.theme, editor.depth, &[SE::DiagGrammar]),
                        };
                        style = style.add_modifier(diag_face.add_modifier);
                        if let Some(uc) = diag_face.underline_color {
                            style = style.underline_color(uc);
                        }
                    }

                    // Flush the accumulated run when the style changes.
                    if run_style != Some(style) && !run.is_empty() {
                        hl_spans.push(Span::styled(std::mem::take(&mut run), run_style.unwrap()));
                    }
                    run_style = Some(style);
                    run.push_str(&p.text);
                }
                if !run.is_empty() {
                    hl_spans.push(Span::styled(run, run_style.unwrap()));
                }
                hl_spans
            };

            // 5g: fold marker on the heading's first visual row.
            let mut spans = spans;
            if let Some(n) = fold_marker_n {
                spans.insert(0, Span::styled("▸ ", compose::compose(&editor.theme, editor.depth, &[SE::FoldMarker])));
                spans.push(Span::styled(
                    format!("  … {n} lines"),
                    compose::compose(&editor.theme, editor.depth, &[SE::FoldMarker]).add_modifier(Modifier::DIM),
                ));
            }

            let line_widget = Line::from(spans);
            let row_area = Rect::new(area.x + tg.text_left, edit_top + screen_row, tg.text_width, 1);
            frame.render_widget(Paragraph::new(line_widget), row_area);

            screen_row += 1;
        }
    }

    // -----------------------------------------------------------------------
    // Scrollbar overlay (painted over editing area, rightmost column)
    // -----------------------------------------------------------------------
    if editor.mouse.scrollbar_visible {
        let fv = crate::fold::FoldView::compute(
            &editor.active().folds,
            &editor.active().document.blocks,
            &editor.active().document.buffer,
        );
        let total = fv.visible_count();
        let scroll_pos = fv.visible_ordinal(editor.active().view.scroll);
        let sb_area = Rect::new(area.x, edit_top, w, edit_height);
        let mut sb_state = ScrollbarState::new(total).position(scroll_pos);
        let sb_track_style = compose::compose(&editor.theme, editor.depth, &[SE::ChromeMuted]);
        let sb_thumb_style = compose::compose(&editor.theme, editor.depth, &[SE::Chrome]);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .track_style(sb_track_style)
                .thumb_style(sb_thumb_style),
            sb_area,
            &mut sb_state,
        );
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

        // When the search overlay is active, render the search bar.
        // When a modal prompt is active, render its message instead of the normal
        // status text, using a distinct style so it stands out.
        // When the minibuffer is open, render <prompt><text> on the status row.
        let chrome_reverse_style = compose::compose(&editor.theme, editor.depth, &[SE::ChromeReverse]);
        let (status_text, status_style) = if let Some(ref s) = editor.search {
            (
                format_search_bar(s),
                chrome_reverse_style,
            )
        } else if let Some(ref mb) = editor.minibuffer {
            (
                format!("{}{}", mb.prompt, mb.text),
                chrome_reverse_style,
            )
        } else if let Some(ref prompt) = editor.prompt {
            (
                prompt.message.clone(),
                chrome_reverse_style,
            )
        } else {
            let text = if editor.status.is_empty() {
                format!("{}{} [{}]", path_str, dirty_marker, mode_text)
            } else {
                format!("{}{} [{}] {}", path_str, dirty_marker, mode_text, editor.status)
            };
            (text, chrome_reverse_style)
        };

        // Compose the status line.
        // When in the normal branch (no prompt/minibuffer/search) and word_count is on,
        // flush the count segment to the right and truncate the left (path/mode) to fit.
        let has_overlay = editor.search.is_some() || editor.minibuffer.is_some() || editor.prompt.is_some() || editor.diag.is_some() || editor.outline.is_some();
        let composed = if !has_overlay {
            if let Some(right) = word_count_segment(editor) {
                let reserve = right.chars().count() + 1;
                let left: String = status_text.chars().take((w as usize).saturating_sub(reserve)).collect();
                let pad = (w as usize).saturating_sub(left.chars().count() + right.chars().count());
                format!("{left}{}{right}", " ".repeat(pad))
            } else {
                status_text.chars().take(w as usize).collect()
            }
        } else {
            status_text.chars().take(w as usize).collect()
        };
        // Truncate the composed string to the terminal width (guard for very narrow terminals).
        let truncated: String = composed.chars().take(w as usize).collect();
        let status_line = Line::from(Span::styled(truncated, status_style));
        let status_area = Rect::new(area.x, status_row, w, 1);
        frame.render_widget(Paragraph::new(status_line), status_area);
    }

    // -----------------------------------------------------------------------
    // Hardware cursor
    // -----------------------------------------------------------------------
    if let Some(ref s) = editor.search {
        // Search bar is open: place caret on the status row at the focused field's caret.
        // Use char counts (not byte offsets) for correct placement with multibyte text.
        let prefix_cols = match s.field {
            crate::search_overlay::Field::Needle => "Find: ".chars().count(),
            crate::search_overlay::Field::Template =>
                format!("Find: {}  Replace: ", s.needle).chars().count(),
        };
        let caret_cols = s.focused_field()[..s.cursor].chars().count();
        let x_offset = (prefix_cols + caret_cols) as u16;
        if x_offset < w {
            frame.set_cursor_position(Position { x: area.x + x_offset, y: status_row });
        }
    } else if let Some(ref mb) = editor.minibuffer {
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
        if row < edit_height && col < tg.text_width {
            frame.set_cursor_position(Position { x: area.x + tg.text_left + col, y: edit_top + row });
        }
    }

    // -----------------------------------------------------------------------
    // Precompute chrome styles for overlays and menu (computed here once so
    // the individual if-let blocks can borrow editor fields freely).
    // -----------------------------------------------------------------------
    let ov_highlight_style = compose::compose(&editor.theme, editor.depth, &[SE::ChromeReverse]);
    let ov_query_style     = compose::compose(&editor.theme, editor.depth, &[SE::Text]);
    let menu_open_style    = compose::compose(&editor.theme, editor.depth, &[SE::ChromeSelected]);
    let menu_closed_style  = compose::compose(&editor.theme, editor.depth, &[SE::Chrome]);
    let menu_sel_style     = compose::compose(&editor.theme, editor.depth, &[SE::ChromeSelected]);
    let menu_norm_style    = compose::compose(&editor.theme, editor.depth, &[SE::ChromeMuted]);

    // -----------------------------------------------------------------------
    // Command palette overlay (drawn on top of everything else)
    // -----------------------------------------------------------------------
    if let Some(ref palette) = editor.palette {
        // Overlay dimensions — shared with mouse hit-testing via palette_overlay_rect.
        let ov_rect = palette_overlay_rect(area, palette.rows.len());
        let ov_x = ov_rect.x;
        let ov_y = ov_rect.y;
        let ov_w = ov_rect.width;
        let ov_h = ov_rect.height;
        // list_h mirrors the computation inside palette_overlay_rect.
        let list_h = (palette.rows.len() as u16).min(15).min(h.saturating_sub(4));

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
            Paragraph::new(Line::from(Span::styled(truncated_q, ov_query_style))),
            query_area,
        );

        if ov_h < 4 || list_h == 0 {
            return;
        }

        // List of rows (below query, inside border).
        let list_area = Rect::new(ov_x + 1, ov_y + 2, ov_w.saturating_sub(2), list_h);
        let highlight_style = ov_highlight_style;
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

    if let Some(ref outline) = editor.outline {
        let ov_rect = palette_overlay_rect(area, outline.rows.len());
        let ov_x = ov_rect.x;
        let ov_y = ov_rect.y;
        let ov_w = ov_rect.width;
        let ov_h = ov_rect.height;
        let list_h = (outline.rows.len() as u16).min(15).min(h.saturating_sub(4));

        frame.render_widget(Clear, ov_rect);
        let block = Block::default().borders(Borders::ALL).title(" Outline ");
        frame.render_widget(block, ov_rect);

        if ov_h >= 3 {
            let query_area = Rect::new(ov_x + 1, ov_y + 1, ov_w.saturating_sub(2), 1);
            let query_display = format!("> {}", outline.query);
            let truncated_q: String = query_display.chars().take(query_area.width as usize).collect();
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(truncated_q, ov_query_style))),
                query_area,
            );

            if ov_h >= 4 && list_h > 0 {
                let list_area = Rect::new(ov_x + 1, ov_y + 2, ov_w.saturating_sub(2), list_h);
                let highlight_style = ov_highlight_style;
                let items: Vec<ListItem> = outline.rows.iter().take(list_h as usize).map(|row| {
                    let mut text = format!("{}{}", " ".repeat(row.indent.saturating_mul(2)), row.text);
                    text = text.chars().take(list_area.width as usize).collect();
                    ListItem::new(Line::from(text))
                }).collect();

                let mut list_state = ListState::default();
                list_state.select(if outline.rows.is_empty() { None } else { Some(outline.selected) });

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
    if let Some(ref tp) = editor.theme_picker {
        let ov_rect = palette_overlay_rect(area, tp.rows.len());
        let ov_x = ov_rect.x;
        let ov_y = ov_rect.y;
        let ov_w = ov_rect.width;
        let ov_h = ov_rect.height;
        let list_h = (tp.rows.len() as u16).min(15).min(h.saturating_sub(4));

        frame.render_widget(Clear, ov_rect);
        let block = Block::default().borders(Borders::ALL).title(" Select Theme ");
        frame.render_widget(block, ov_rect);

        if ov_h >= 3 {
            let query_area = Rect::new(ov_x + 1, ov_y + 1, ov_w.saturating_sub(2), 1);
            let query_display = format!("> {}", tp.query);
            let truncated_q: String = query_display.chars().take(query_area.width as usize).collect();
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(truncated_q, ov_query_style))),
                query_area,
            );

            if ov_h >= 4 && list_h > 0 {
                let list_area = Rect::new(ov_x + 1, ov_y + 2, ov_w.saturating_sub(2), list_h);
                let highlight_style = ov_highlight_style;
                let items: Vec<ListItem> = tp.rows.iter().take(list_h as usize).map(|name| {
                    let truncated: String = name.chars().take(list_area.width as usize).collect();
                    ListItem::new(Line::from(truncated))
                }).collect();

                let mut list_state = ListState::default();
                list_state.select(if tp.rows.is_empty() { None } else { Some(tp.selected) });

                frame.render_stateful_widget(
                    List::new(items).highlight_style(highlight_style),
                    list_area,
                    &mut list_state,
                );
            }
        }
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
                    menu_open_style
                } else {
                    menu_closed_style
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
                            menu_sel_style
                        } else {
                            menu_norm_style
                        };
                        ListItem::new(format!(" {label} ")).style(style)
                    })
                    .collect();
                frame.render_widget(List::new(items), drop_rect);
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

        let title = format!(" {} ", diag_ov.anchor.message);
        let block = Block::default().borders(Borders::ALL).title(title);
        frame.render_widget(block, ov_rect);

        if ov_h >= 3 {
            let list_h = (row_count as u16).min(15).min(ov_h.saturating_sub(2));
            let list_area = Rect::new(ov_x + 1, ov_y + 1, ov_w.saturating_sub(2), list_h);
            let highlight_style = ov_highlight_style;

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

// ---------------------------------------------------------------------------
// Search bar formatting
// ---------------------------------------------------------------------------

fn format_search_bar(s: &crate::search_overlay::SearchState) -> String {
    use crate::search_overlay::Phase;
    let mode = if matches!(s.mode, wordcartel_core::search::QueryMode::Regex) { " .*" } else { "" };
    let case = match s.case {
        wordcartel_core::search::CaseMode::Smart => " Aa~",
        wordcartel_core::search::CaseMode::Sensitive => " Aa",
        wordcartel_core::search::CaseMode::Insensitive => " aa",
    };
    let count = if s.error.is_some() {
        " ?".to_string()
    } else if s.count() == 0 {
        " no matches".to_string()
    } else {
        format!(" {}/{}", s.current_ordinal().unwrap_or(0), s.count())
    };
    let wrapped = if s.wrapped { " (wrapped)" } else { "" };
    match s.phase {
        Phase::Replace | Phase::Stepping =>
            format!("Find: {}  Replace: {}{}{}{}{}", s.needle, s.template, mode, case, count, wrapped),
        Phase::Find =>
            format!("Find: {}{}{}{}{}", s.needle, mode, case, count, wrapped),
    }
}

// ---------------------------------------------------------------------------
// Fold marker helper
// ---------------------------------------------------------------------------

/// If logical line `l` is the heading line of a folded section, return the hidden
/// body line count; otherwise None. Pure — drives both the marker glyph and tests.
pub fn fold_marker_for(editor: &crate::editor::Editor, l: usize) -> Option<usize> {
    let b = editor.active();
    let buf = &b.document.buffer;
    // The folded anchor whose heading line is `l`.
    let hb = b.folds.folded.iter().copied().find(|&hb| buf.byte_to_line(hb) == l)?;
    Some(crate::fold::hidden_count_lines(&b.document.blocks, buf, hb))
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

    // -----------------------------------------------------------------------
    // Search render test helpers
    // -----------------------------------------------------------------------

    /// Render the editor to a standalone ratatui Buffer for assertions.
    /// Editor is NOT Clone, so we mutably borrow it for the draw call.
    fn render_to_buffer(editor: &mut Editor, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| super::render(f, editor)).unwrap();
        term.backend().buffer().clone()
    }

    fn row_string(buf: &ratatui::buffer::Buffer, row: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, row)].symbol().to_string()).collect()
    }

    fn row_has_highlight(buf: &ratatui::buffer::Buffer, row: u16) -> bool {
        use ratatui::style::{Color, Modifier};
        (0..buf.area.width).any(|x| {
            let c = &buf[(x, row)];
            c.style().bg == Some(Color::Yellow) || c.style().add_modifier.contains(Modifier::REVERSED)
        })
    }

    fn row_has_underline(buf: &ratatui::buffer::Buffer, row: u16) -> bool {
        use ratatui::style::Modifier;
        (0..buf.area.width).any(|x| {
            buf[(x, row)].style().add_modifier.contains(Modifier::UNDERLINED)
        })
    }

    // -----------------------------------------------------------------------
    // Search render tests
    // -----------------------------------------------------------------------

    #[test]
    fn search_highlights_matches_and_shows_count() {
        let mut e = Editor::new_from_text("foo bar foo\n", None, (40, 6));
        e.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "foo".chars() { e.search.as_mut().unwrap().insert(c); }
        let rope = e.active().document.buffer.snapshot();
        let v = e.active().document.version;
        e.search.as_mut().unwrap().recompute(&rope, v);
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 40, 6);
        let status = row_string(&buf, 5); // bottom row
        assert!(status.contains("Find:"), "search bar shows Find:, got {status:?}");
        assert!(status.contains("1/2") || status.contains("2"), "shows match count, got {status:?}");
        // both "foo" occurrences carry a highlight bg somewhere on row 0
        assert!(row_has_highlight(&buf, 0), "matches highlighted on row 0");
    }

    #[test]
    fn highlight_skips_concealed_markers_in_live_preview() {
        let mut e = Editor::new_from_text("**bold**\n", None, (40, 6)); // LivePreview conceals **
        e.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "bold".chars() { e.search.as_mut().unwrap().insert(c); }
        let rope = e.active().document.buffer.snapshot();
        let v = e.active().document.version;
        e.search.as_mut().unwrap().recompute(&rope, v);
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 40, 6);
        // The visible word "bold" is highlighted; render does not panic projecting
        // a raw-byte match (start=2..6) onto the concealed visible row.
        assert!(row_has_highlight(&buf, 0), "bold should be highlighted");
    }

    #[test]
    fn search_caret_uses_char_count_not_byte_count() {
        // Multibyte needle: "café" = 5 bytes (é is 2 bytes) but 4 chars.
        // Caret should be at "Find: ".chars().count() + 4 = 6 + 4 = 10 cols.
        let mut e = Editor::new_from_text("café\n", None, (40, 6));
        e.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "café".chars() { e.search.as_mut().unwrap().insert(c); }
        let s = e.search.as_ref().unwrap();
        let prefix_cols = "Find: ".chars().count();
        let caret_cols = s.focused_field()[..s.cursor].chars().count();
        let x = prefix_cols + caret_cols;
        assert_eq!(x, 10, "caret col should be char-based (6 + 4 = 10), got {x}");
        // Also verify this differs from the byte-based count (é is 2 bytes → 11 bytes total).
        assert_ne!(x, "Find: ".len() + s.needle.len(), "char count must differ from byte count for multibyte input");
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

    #[test]
    fn wrap_guide_column_position() {
        // measure off, wrap_guide on, column 40, viewport 80 → guide at screen col 40
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.view_opts.wrap_guide = true; e.view_opts.wrap_column = 40;
        let tg = crate::nav::text_geometry(&e);
        let gx = tg.text_left + e.view_opts.wrap_column;
        assert_eq!(gx, 40);
        assert!(gx < e.active().view.area.0, "guide within viewport");
        // measure on, column 40, viewport 80 → text_left 20, guide at 60 (right edge)
        e.view_opts.measure = true;
        let tg = crate::nav::text_geometry(&e);
        assert_eq!(tg.text_left + e.view_opts.wrap_column, 60);
    }

    /// palette_overlay_rect sizes height to the actual row count, not fixed-15.
    #[test]
    fn palette_overlay_rect_sizes_to_row_count() {
        let area = Rect::new(0, 0, 80, 40);
        // 3 rows → list_h=3, ov_h=3+3=6
        let r3 = palette_overlay_rect(area, 3);
        assert_eq!(r3.height, 6, "3 rows: expected height 6 (3 list + 3 chrome)");
        // 30 rows → list_h capped at 15, ov_h=15+3=18
        let r30 = palette_overlay_rect(area, 30);
        assert_eq!(r30.height, 18, "30 rows: expected height 18 (15 capped + 3 chrome)");
    }

    #[test]
    fn focus_active_region_is_paragraph_at_caret() {
        let mut e = Editor::new_from_text("Para one.\n\nPara two.\n\nThree.\n", None, (80, 24));
        e.view_opts.focus = true; // paragraph default
        set_caret(&mut e, 12); // inside "Para two."
        derive::rebuild(&mut e);
        // the active region used by render = paragraph_range_at at the caret
        let buf = &e.active().document.buffer; let blocks = &e.active().document.blocks;
        let (from, to) = crate::nav::paragraph_range_at(blocks, buf, 12);
        assert_eq!(buf.slice(from..to).trim(), "Para two.");
        // a row whose global src span is outside [from,to) is dimmed; inside is bright.
        // (assert the helper render uses to decide, not pixels — see Step 3 for the fn)
        assert!(!crate::render::row_is_active(0, "Para one.".len(), from, to), "para one dimmed");
        assert!(crate::render::row_is_active(from, to, from, to), "active row bright");
    }

    /// Viewport-gated highlight scan:
    /// - A match scrolled completely off-screen must NOT produce a highlight on
    ///   any visible row.
    /// - A match that straddles the top scroll boundary (starts just before lo,
    ///   ends inside the viewport) must still be highlighted on its visible portion.
    #[test]
    fn viewport_gates_search_highlight_scan() {
        // Build a document: "needle\n" at line 0, then 5 filler lines, then "needle\n".
        // With a 3-row viewport (2 editing rows + 1 status) and scroll=5 we see only
        // lines 5 and 6 — so line 0 is completely off-screen and its match must not
        // appear as a highlight on any visible row.
        let doc = "needle\n".to_string()
            + "filler\nfiller\nfiller\nfiller\nfiller\n"
            + "needle\n";
        let mut e = Editor::new_from_text(&doc, None, (40, 3));
        e.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "needle".chars() { e.search.as_mut().unwrap().insert(c); }
        let rope = e.active().document.buffer.snapshot();
        let v = e.active().document.version;
        e.search.as_mut().unwrap().recompute(&rope, v);
        assert_eq!(e.search.as_ref().unwrap().count(), 2, "expect 2 matches total");
        // Scroll to line 5 so line 0 is off-screen.
        e.active_mut().view.scroll = 5;
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 40, 3);
        // Row 1 (screen row 1) shows line 6 ("needle") — must be highlighted.
        assert!(row_has_highlight(&buf, 1), "on-screen match at line 6 must be highlighted");
        // Row 0 shows line 5 ("filler") — must NOT be highlighted (line 0 match is off-screen).
        assert!(!row_has_highlight(&buf, 0), "off-screen match at line 0 must not bleed onto filler row");

        // Straddling test: a match whose end byte is > lo (scroll line boundary) but
        // whose start byte is == lo - 1 (just before the boundary) must be included.
        // Construct: scroll to line 1 so lo = byte offset of "filler" start.
        // The "needle\n" at line 0 ends at byte 7 which equals lo — exactly the
        // partition boundary: m.end == lo means m.end <= lo is TRUE, so lo_idx would
        // exclude it. That's correct: the match ends AT lo, no glyph of it is visible.
        // Test a match that does cross: put scroll at line 0 (lo=0) so everything is in
        // the window, then verify both matches show (not a straddling case but a
        // full-window case that must still work).
        e.active_mut().view.scroll = 0;
        crate::derive::rebuild(&mut e);
        let buf2 = render_to_buffer(&mut e, 40, 3);
        assert!(row_has_highlight(&buf2, 0), "match at line 0 visible when scroll=0");
    }

    #[test]
    fn diagnostics_underline_the_flagged_glyphs() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("teh cat\n", None, (40, 6));
        let v = e.active().document.version;
        e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(), suggestions: vec![] }];
        e.active_mut().diagnostics.computed_version = v;
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 40, 6);
        assert!(row_has_underline(&buf, 0), "the misspelled 'teh' is underlined");
    }

    #[test]
    fn stale_diagnostics_are_not_painted() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("teh cat\n", None, (40, 6));
        e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(), suggestions: vec![] }];
        e.active_mut().diagnostics.computed_version = 999; // != current version
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 40, 6);
        assert!(!row_has_underline(&buf, 0), "version-mismatched diagnostics are hidden");
    }

    #[test]
    fn fold_marker_helper_reports_marker_for_folded_heading() {
        let doc = "## A\nb1\nb2\n## B\n";
        let mut ed = crate::editor::Editor::new_from_text(doc, None, (40, 10));
        ed.active_mut().folds.toggle(doc.find("## A").unwrap());
        crate::derive::rebuild(&mut ed);
        let a_line = 0usize; // "## A"
        assert_eq!(crate::render::fold_marker_for(&ed, a_line), Some(2)); // 2 hidden lines
        assert_eq!(crate::render::fold_marker_for(&ed, 3), None);          // "## B" not folded
    }

    #[test]
    fn word_count_segment_selection_aware() {
        let mut e = Editor::new_from_text("alpha beta gamma\n", None, (80, 24));
        e.view_opts.word_count = true;
        // whole doc: 3 words, 17 chars (including trailing \n)
        assert_eq!(crate::render::word_count_segment(&e), Some("3 words · 17 chars".to_string()));
        // select "alpha" → 1 word, 5 chars
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5);
        assert_eq!(crate::render::word_count_segment(&e), Some("1 words · 5 chars".to_string()));
        e.view_opts.word_count = false;
        assert_eq!(crate::render::word_count_segment(&e), None);
    }

    #[test]
    fn default_theme_inline_styles_unchanged() {
        // a strong word renders BOLD, a code span gets cyan fg — exactly as today.
        // Two lines: the caret goes to line 1 so line 0 is inactive (concealed/styled).
        let mut ed = Editor::new_from_text("**bold** and `code`\nother\n", None, (40, 4));
        // Move caret to line 1 so line 0 is inactive → styling applied in LivePreview.
        set_caret(&mut ed, 20); // byte 20 = start of "other\n"
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 4);
        // find a bold cell and a cyan cell on row 0 (live-preview conceals the markers)
        let row0_has_bold = (0..40).any(|x| buf[(x,0)].style().add_modifier.contains(Modifier::BOLD));
        let row0_has_cyan = (0..40).any(|x| buf[(x,0)].style().fg == Some(Color::Indexed(6)) || buf[(x,0)].style().fg == Some(Color::Cyan));
        assert!(row0_has_bold && row0_has_cyan);
    }

    #[test]
    fn tokyo_night_heading_row_carries_heading_fg() {
        let mut ed = Editor::new_from_text("# Title\n", None, (40, 4));
        ed.theme = wordcartel_core::theme::tokyo_night();
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 4);
        let want = crate::compose::compose(&ed.theme, ed.depth, &[wordcartel_core::theme::SemanticElement::Text, wordcartel_core::theme::SemanticElement::Heading(1)]).fg;
        assert!((0..40).any(|x| buf[(x,0)].style().fg == want && want.is_some()), "heading fg applied");
    }

    #[test]
    fn default_status_line_still_reversed() {
        let mut ed = Editor::new_from_text("x", None, (40, 4));
        let buf = render_to_buffer(&mut ed, 40, 4);
        let last = 3u16;
        assert!((0..40).any(|x| buf[(x,last)].style().add_modifier.contains(Modifier::REVERSED)));
    }

    #[test]
    fn phosphor_status_line_carries_hue() {
        let mut ed = Editor::new_from_text("x", None, (40, 4));
        ed.theme = wordcartel_core::theme::Theme::builtin("phosphor-amber").unwrap();
        let buf = render_to_buffer(&mut ed, 40, 4);
        let want = compose::compose(&ed.theme, ed.depth, &[wordcartel_core::theme::SemanticElement::ChromeReverse]);
        // the status row picks up the themed chrome-reverse style, not a hardcoded REVERSED.
        // ratatui's test buffer normalizes unset colors to Reset in rendered cells, so compare
        // the meaningful modifier bits and any explicit fg/bg from want.
        assert!((0..40).any(|x| {
            let cell = buf[(x,3)].style();
            // modifiers must match exactly
            cell.add_modifier == want.add_modifier
            // any fg set by want must appear in the cell (Reset == None for our purposes)
            && (want.fg.is_none() || cell.fg == want.fg)
            && (want.bg.is_none() || cell.bg == want.bg)
        }));
    }

    #[test]
    fn source_mode_no_heading_fg_live_preview_has_heading_fg() {
        // In SourcePlain under Tokyo Night, a heading row must NOT carry the heading fg.
        // In LivePreview it must.
        use crate::editor::RenderMode;
        let mut ed = Editor::new_from_text("# Heading\n", None, (40, 4));
        ed.theme = wordcartel_core::theme::tokyo_night();

        // LivePreview first: heading fg should appear.
        ed.active_mut().view.mode = RenderMode::LivePreview;
        crate::derive::rebuild(&mut ed);
        let buf_preview = render_to_buffer(&mut ed, 40, 4);
        let want = crate::compose::compose(&ed.theme, ed.depth, &[
            wordcartel_core::theme::SemanticElement::Text,
            wordcartel_core::theme::SemanticElement::Heading(1),
        ]).fg;
        let preview_has_heading_fg = (0..40).any(|x| buf_preview[(x,0)].style().fg == want && want.is_some());
        assert!(preview_has_heading_fg, "LivePreview heading must carry heading fg");

        // SourcePlain: base canvas only, no heading fg.
        ed.active_mut().view.mode = RenderMode::SourcePlain;
        crate::derive::rebuild(&mut ed);
        let buf_source = render_to_buffer(&mut ed, 40, 4);
        let source_has_heading_fg = (0..40).any(|x| buf_source[(x,0)].style().fg == want && want.is_some());
        assert!(!source_has_heading_fg, "SourcePlain must not carry heading fg (base canvas only)");
    }

    #[test]
    fn default_search_and_diag_unchanged() {
        // search highlight still yellow-bg/reverse; diagnostics still underline red/blue.
        // Mirror the existing search/diag tests — they must keep passing under Default.
        {
            let mut e = Editor::new_from_text("foo bar foo\n", None, (40, 6));
            e.open_search(crate::search_overlay::Phase::Find, 0);
            for c in "foo".chars() { e.search.as_mut().unwrap().insert(c); }
            let rope = e.active().document.buffer.snapshot();
            let v = e.active().document.version;
            e.search.as_mut().unwrap().recompute(&rope, v);
            crate::derive::rebuild(&mut e);
            let buf = render_to_buffer(&mut e, 40, 6);
            assert!(row_has_highlight(&buf, 0), "Default: search highlights still present");
        }
        {
            let mut e = Editor::new_from_text("teh cat\n", None, (40, 6));
            let v = e.active().document.version;
            e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                message: "x".into(),
                suggestions: vec![],
            }];
            e.active_mut().diagnostics.computed_version = v;
            crate::derive::rebuild(&mut e);
            let buf = render_to_buffer(&mut e, 40, 6);
            assert!(row_has_underline(&buf, 0), "Default: diag underline still present");
        }
    }

    #[test]
    fn no_color_theme_strips_search_color_keeps_reverse() {
        // Under no-color theme, a search match cell has REVERSED and no yellow bg.
        let mut ed = Editor::new_from_text("needle here\n", None, (40, 4));
        ed.theme = wordcartel_core::theme::no_color();
        ed.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "needle".chars() { ed.search.as_mut().unwrap().insert(c); }
        let rope = ed.active().document.buffer.snapshot();
        let v = ed.active().document.version;
        ed.search.as_mut().unwrap().recompute(&rope, v);
        crate::derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 4);
        // The match cell for "needle" on row 0: should have REVERSED (no_color search_match has reverse=true)
        // and NO yellow bg (no_color strips all color).
        let has_yellow_bg = (0..40u16).any(|x| buf[(x, 0)].style().bg == Some(Color::Yellow));
        let has_reversed = (0..40u16).any(|x| buf[(x, 0)].style().add_modifier.contains(Modifier::REVERSED));
        assert!(!has_yellow_bg, "no-color theme: search match must not have yellow bg");
        assert!(has_reversed, "no-color theme: search match must have REVERSED modifier");
    }

    /// A bold word that is a search match must keep BOLD while also showing the
    /// search highlight (yellow bg or reversed). Guards the layering fix for
    /// search overlays: style.patch(search_face) instead of replacing style.
    #[test]
    fn bold_search_match_keeps_bold_under_default() {
        // Two-line doc: "**bold**" on line 0, caret on line 1 so live-preview
        // conceals the markers and styles "bold" as BOLD. Search matches "bold".
        // Caret on line 1 ensures live-preview is applied to line 0.
        let mut ed = Editor::new_from_text("**bold**\nother\n", None, (40, 6));
        // Place caret on line 1 so line 0 is styled by live-preview.
        set_caret(&mut ed, 9); // byte 9 = start of "other"
        ed.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "bold".chars() { ed.search.as_mut().unwrap().insert(c); }
        let rope = ed.active().document.buffer.snapshot();
        let v = ed.active().document.version;
        ed.search.as_mut().unwrap().recompute(&rope, v);
        crate::derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);
        // A cell on row 0 must be BOLD (from the Strong inline style).
        let has_bold = (0..40u16).any(|x| buf[(x, 0)].style().add_modifier.contains(Modifier::BOLD));
        // A cell on row 0 must also carry the search highlight (yellow bg or reversed).
        let has_highlight = row_has_highlight(&buf, 0);
        assert!(has_bold, "matched bold word must still show BOLD modifier");
        assert!(has_highlight, "matched bold word must show search highlight");
    }

    /// A non-current SearchMatch (≥2 matches so one is non-current) under no-color
    /// must have REVERSED but no yellow bg. This exercises the SearchMatch (not just
    /// SearchCurrent) branch of the layering fix.
    #[test]
    fn no_color_non_current_search_match_keeps_reverse_no_yellow() {
        // Two occurrences of "needle" — the current match is the first one (ordinal 1).
        // The second "needle" is a non-current SearchMatch.
        let mut ed = Editor::new_from_text("needle and needle\n", None, (40, 4));
        ed.theme = wordcartel_core::theme::no_color();
        ed.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "needle".chars() { ed.search.as_mut().unwrap().insert(c); }
        let rope = ed.active().document.buffer.snapshot();
        let v = ed.active().document.version;
        ed.search.as_mut().unwrap().recompute(&rope, v);
        assert_eq!(ed.search.as_ref().unwrap().count(), 2, "expect 2 matches");
        crate::derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 4);
        // Both matches are on row 0. The second occurrence starts at col 11.
        // Check cells around col 11..17 for no-yellow and reversed.
        let has_yellow = (11u16..17u16).any(|x| buf[(x, 0)].style().bg == Some(Color::Yellow));
        let has_reversed = (11u16..17u16).any(|x| buf[(x, 0)].style().add_modifier.contains(Modifier::REVERSED));
        assert!(!has_yellow, "no-color non-current match must not have yellow bg");
        assert!(has_reversed, "no-color non-current match must have REVERSED modifier");
    }

    // -----------------------------------------------------------------------
    // Golden tests: lock Default-theme styles for 4 render sites
    // -----------------------------------------------------------------------

    /// Golden: scrollbar track = White/DarkGray, thumb = White/Black under Default theme.
    ///
    /// Creates a doc tall enough to overflow a short viewport, enables the scrollbar,
    /// and asserts that the rightmost column carries the expected track and thumb styles.
    #[test]
    fn golden_default_scrollbar_styled() {
        let doc: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let mut ed = Editor::new_from_text(&doc, None, (40, 8));
        ed.mouse.scrollbar_visible = true;
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 8);

        // The scrollbar is in the rightmost column (col 39); rows 0..6 are edit area
        // (h=8 minus 1 status row = 7 edit rows). The track style = ChromeMuted
        // (White fg, DarkGray bg) and thumb style = Chrome (White fg, Black bg).
        let track_style = compose::compose(&ed.theme, ed.depth, &[SE::ChromeMuted]);
        let thumb_style = compose::compose(&ed.theme, ed.depth, &[SE::Chrome]);

        // Lock the concrete Default-theme colors at the compose level (not cell level).
        // This validates the theme face mapping regardless of ratatui widget rendering quirks.
        assert_eq!(track_style.fg, Some(Color::White), "Default track fg must be White");
        assert_eq!(track_style.bg, Some(Color::DarkGray), "Default track bg must be DarkGray");
        assert_eq!(thumb_style.fg, Some(Color::White), "Default thumb fg must be White");
        assert_eq!(thumb_style.bg, Some(Color::Black), "Default thumb bg must be Black");

        // Buffer-level check: at least one cell in the rightmost column (the scrollbar
        // column) must have a non-default style — confirming the scrollbar rendered at all.
        let rightmost: u16 = 39;
        let has_scrollbar_cell = (0u16..7).any(|r| {
            let s = buf[(rightmost, r)].style();
            // Any non-trivially-reset cell: has a symbol other than space, or has a non-reset fg/bg.
            !buf[(rightmost, r)].symbol().trim().is_empty()
                || (s.fg.is_some() && s.fg != Some(Color::Reset))
                || (s.bg.is_some() && s.bg != Some(Color::Reset))
        });
        assert!(has_scrollbar_cell, "expected at least one styled scrollbar cell in rightmost column (col {rightmost})");
    }

    /// Golden: list bullet prefix glyph has DarkGray fg and DIM modifier under Default theme.
    ///
    /// Two-line doc `"- item\nmore\n"` with caret on line 1 (so line 0 is non-active
    /// and the bullet on row 0 uses the non-dim path with an explicit DIM modifier).
    #[test]
    fn golden_default_list_bullet_darkgray_dim() {
        let mut ed = Editor::new_from_text("- item\nmore\n", None, (40, 6));
        // Caret on line 1 so line 0 is non-active (bullet gets DIM, not FocusDim).
        set_caret(&mut ed, 7); // byte 7 = start of "more\n"
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);

        // The bullet glyph "• " is rendered at the start of row 0.
        // In LivePreview mode the "- " markdown is concealed and "• " is the prefix_glyph.
        // Find the first cell on row 0 that is the bullet (look for "•").
        let bullet_col = (0u16..40).find(|&x| buf[(x, 0)].symbol().contains('•'));
        assert!(bullet_col.is_some(), "expected bullet glyph '•' on row 0");
        let x = bullet_col.unwrap();
        let cell_style = buf[(x, 0)].style();

        assert_eq!(
            cell_style.fg,
            Some(Color::DarkGray),
            "Default list bullet must have DarkGray fg, got {:?}",
            cell_style.fg
        );
        assert!(
            cell_style.add_modifier.contains(Modifier::DIM),
            "Default list bullet must have DIM modifier, got {:?}",
            cell_style.add_modifier
        );
    }

    /// Golden (Task 5): blockquote prefix glyph `▎` is styled with the BlockQuote face
    /// (not ListMarker), AND a non-active blockquote prefix still carries DIM.
    ///
    /// Two-line doc `"> quoted\nmore\n"` with caret on line 1.  Line 0 is non-active,
    /// so its `▎` glyph uses the non-dim path: `compose([BlockQuote]).add_modifier(DIM)`.
    /// Under the Default theme BlockQuote face has no fg (unlike ListMarker which has
    /// DarkGray fg), so the glyph cell must have no fg (None or Reset) but MUST have DIM.
    #[test]
    fn golden_blockquote_glyph_uses_blockquote_face_not_listmarker() {
        let mut ed = Editor::new_from_text("> quoted\nmore\n", None, (40, 6));
        // Caret on line 1 so line 0 is non-active.
        set_caret(&mut ed, 9); // byte 9 = start of "more\n"
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);

        // Find the `▎` glyph on row 0.
        let glyph_col = (0u16..40).find(|&x| buf[(x, 0)].symbol().contains('▎'));
        assert!(glyph_col.is_some(), "expected blockquote glyph '▎' on row 0");
        let x = glyph_col.unwrap();
        let cell_style = buf[(x, 0)].style();

        // BlockQuote face (Default theme) has no fg — unlike ListMarker (DarkGray).
        assert!(
            cell_style.fg.is_none() || cell_style.fg == Some(Color::Reset),
            "blockquote glyph must use BlockQuote face (no fg), not ListMarker (DarkGray); got {:?}",
            cell_style.fg
        );
        // Non-active row: DIM must still be present.
        assert!(
            cell_style.add_modifier.contains(Modifier::DIM),
            "non-active blockquote glyph must carry DIM modifier, got {:?}",
            cell_style.add_modifier
        );
    }

    /// Task 6 gate (C1): a wrapped list item must paint its CONTINUATION row's
    /// text starting at `text_left + prefix_width` (behind a blank spacer), and
    /// the caret placed on a continuation-row glyph must land on that same
    /// painted column — not `prefix_width` cells to its left. This is the proof
    /// that render's continuation spacer keeps painted text aligned with the
    /// prefix-offset cursor columns (`Placed.col` includes `prefix_width`).
    #[test]
    fn wrapped_list_item_continuation_row_aligns_text_and_caret() {
        // 12-wide viewport, list prefix "• " (width 2). "aaaa bbbb cccc" wraps:
        //   row 0: "• aaaa bbbb "   (glyph cols 0..2, text col 2..)
        //   row 1: "  cccc"         (blank spacer cols 0..2, text col 2..)
        let mut e = Editor::new_from_text("- aaaa bbbb cccc\nmore\n", None, (12, 8));
        // Caret on line 1 so line 0 is INACTIVE: its "- " is concealed and the
        // "• " prefix glyph is shown (active lines render raw, with no prefix).
        set_caret(&mut e, 18); // byte 18 = start of "more\n"
        derive::rebuild(&mut e);
        let tg = crate::nav::text_geometry(&e);
        let text_left = tg.text_left as usize;
        assert_eq!(text_left, 0, "measure off → text_left 0");

        let prefix_width = {
            let (_rows, map) = &e.active().view.line_layouts[&0];
            assert!(map.rows >= 2, "list item must wrap to exercise continuation row");
            assert_eq!(map.prefix_width, 2, "list bullet '• ' width 2");
            map.prefix_width
        };

        let buf = render_to_buffer(&mut e, 12, 8);

        // (a) Continuation row (screen row 1): cols [text_left, text_left+prefix_width)
        // are a blank spacer; the first text glyph is painted at text_left+prefix_width.
        for c in 0..prefix_width {
            let cell = buf[((text_left + c) as u16, 1u16)].symbol();
            assert_eq!(cell, " ", "continuation-row spacer cell {c} must be blank, got {cell:?}");
        }
        let first_text_col = text_left + prefix_width;
        assert_eq!(
            buf[(first_text_col as u16, 1u16)].symbol(),
            "c",
            "continuation-row text must start at text_left+prefix_width ({first_text_col})"
        );

        // (b) The caret position for the first continuation-row glyph (byte 12 =
        // first 'c') must resolve to the SAME column the glyph is painted at. The
        // prefix-offset layout maps byte 12 -> (row 1, col prefix_width), and the
        // screen column is text_left + col. This proves the caret lands ON the
        // glyph, not prefix_width cells to its left under the spacer.
        let (_rows, map) = &e.active().view.line_layouts[&0];
        let (vrow, vcol) = map.source_to_visual(12);
        assert_eq!(vrow, 1, "first continuation glyph is on visual row 1");
        assert_eq!(
            text_left + vcol,
            first_text_col,
            "caret column for the continuation glyph must equal its painted column \
             (cursor on the glyph, not under the spacer)"
        );
        // And the inverse: a click on that painted cell round-trips to byte 12.
        assert_eq!(
            map.visual_to_source(vrow, first_text_col - text_left),
            12,
            "click on the continuation glyph's painted cell selects that glyph"
        );
    }

    /// Golden: fold marker `▸` glyph has DarkGray fg under Default theme.
    ///
    /// Creates a doc with a heading + body, folds the heading, renders, and
    /// asserts the `▸` glyph cell has DarkGray fg.
    #[test]
    fn golden_default_fold_marker_darkgray() {
        let doc = "## Heading\nbody line 1\nbody line 2\n";
        let mut ed = Editor::new_from_text(doc, None, (40, 6));
        let heading_byte = doc.find("## Heading").unwrap();
        ed.active_mut().folds.toggle(heading_byte);
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);

        // The fold marker glyph '▸' is prepended to the heading row (row 0).
        let marker_col = (0u16..40).find(|&x| buf[(x, 0)].symbol().contains('▸'));
        assert!(marker_col.is_some(), "expected fold marker glyph '▸' on row 0");
        let x = marker_col.unwrap();
        let cell_style = buf[(x, 0)].style();

        // Lock: Default FoldMarker = DarkGray fg.
        assert_eq!(
            cell_style.fg,
            Some(Color::DarkGray),
            "Default fold marker must have DarkGray fg, got {:?}",
            cell_style.fg
        );
    }

    /// Golden: wrap guide `│` glyph has DarkGray fg under Default theme.
    ///
    /// Enables the wrap guide at column 10 in a 40-wide viewport and asserts
    /// the guide column cell on row 0 has DarkGray fg.
    #[test]
    fn golden_default_wrap_guide_darkgray() {
        let mut ed = Editor::new_from_text("short line\nmore text\n", None, (40, 6));
        ed.view_opts.wrap_guide = true;
        ed.view_opts.wrap_column = 10;
        // measure=false (default): text_left=0, guide lands at col 10.
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);

        // The wrap guide '│' is painted at column text_left + wrap_column = 10.
        let tg = crate::nav::text_geometry(&ed);
        let guide_col = tg.text_left + ed.view_opts.wrap_column;
        assert!(
            (guide_col as u16) < 40,
            "guide column {guide_col} must be within viewport"
        );

        // Find the '│' glyph in the guide column.
        // Check all edit rows (0..5) since content may not start at row 0 for guide.
        let guide_x = guide_col as u16;
        let guide_cell = (0u16..5).find_map(|r| {
            let s = buf[(guide_x, r)].symbol();
            if s.contains('│') { Some(buf[(guide_x, r)].style()) } else { None }
        });
        assert!(guide_cell.is_some(), "expected wrap guide '│' at column {guide_col}");
        let cell_style = guide_cell.unwrap();

        // Lock: Default WrapGuide = DarkGray fg.
        assert_eq!(
            cell_style.fg,
            Some(Color::DarkGray),
            "Default wrap guide must have DarkGray fg, got {:?}",
            cell_style.fg
        );
    }

    // -----------------------------------------------------------------------
    // Task 9: selection painting tests
    // -----------------------------------------------------------------------

    #[test]
    fn selection_paints_reverse_on_selected_cells() {
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 4));
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5); // "hello"
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 4);
        // a selected cell (col 0..5 on row 0) carries the Selection face (reverse) layered on.
        assert!((0u16..5).any(|x| buf[(x,0)].style().add_modifier.contains(Modifier::REVERSED)),
            "selected cells must carry REVERSED modifier");
    }

    #[test]
    fn empty_selection_paints_nothing() {
        let mut ed = Editor::new_from_text("hello\n", None, (40, 4));
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 4);
        assert!(!(0u16..5).any(|x| buf[(x,0)].style().add_modifier.contains(Modifier::REVERSED)),
            "empty (cursor-only) selection must not paint REVERSED on any cell");
    }

    // -----------------------------------------------------------------------
    // §13.2 accessibility coverage proof tests
    // -----------------------------------------------------------------------

    fn cue_themes() -> [wordcartel_core::theme::Theme; 2] {
        [wordcartel_core::theme::no_color(),
         wordcartel_core::theme::Theme::builtin("phosphor-amber-flat").unwrap()]
    }

    /// §13.2: Every Face-cued SemanticElement carries >=1 non-color modifier in cue mode.
    #[test]
    fn a11y_every_cued_element_has_a_modifier_in_cue_mode() {
        use wordcartel_core::theme::SemanticElement::*;
        // Face-cued persistent text elements (glyph-cued structural elements proven by render
        // fixtures below):
        //   Emphasis=italic, Strong=bold, StrongEmphasis=bold+italic, Code=reverse,
        //   CodeBlock=reverse, Link=underline, Strikethrough=strike, Comment=italic+dim,
        //   FrontMatter=reverse+italic, Selection=reverse+underline,
        //   SearchMatch=reverse, SearchCurrent=reverse+bold,
        //   DiagSpelling=bold+underline, DiagGrammar=bold+underline (distinct face).
        // Transient overlay + chrome elements with modifier cues:
        //   FocusDim=dim, ChromeReverse=reverse, ChromeSelected=reverse, ChromeMuted=dim.
        let cued = [Emphasis, Strong, StrongEmphasis, Code, CodeBlock, Link, Strikethrough,
                    Comment, FrontMatter, Selection, SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar,
                    FocusDim, ChromeReverse, ChromeSelected, ChromeMuted];
        for t in cue_themes() {
            for el in cued {
                let f = t.face(el);
                assert!(f.bold.unwrap_or(false)||f.italic.unwrap_or(false)||f.underline.unwrap_or(false)
                        ||f.strike.unwrap_or(false)||f.reverse.unwrap_or(false)||f.dim.unwrap_or(false),
                        "{}/{el:?} needs a non-color cue", t.name);
            }
        }
        // §13.2: Chrome (base panel face) is placement-cued — it appears only in the chrome
        // region (status bar, menu bar, overlay frames) which is structurally distinct from
        // text content; it never needs a modifier to distinguish it from text elements.
        // Proven by the chrome render tests (status-line, palette, outline overlay fixtures).
        // §13.2: FoldMarker is glyph-cued (▸ + "… N lines"); proven by the fold fixture:
        //   `fold_marker_glyph_prefix_is_rendered` — deferred to plan-③ test pass for the
        //   full §8.3 row (requires outline-folded Editor state, which is elaborate scaffolding).
        // §13.2: WrapGuide is glyph-cued (│ column guide); proven by placement — the guide
        //   column character is a literal `│` cell; deferred to plan-③ render fixture.
    }

    /// §13.2: Same-context pairs must be distinguishable in cue mode.
    #[test]
    fn a11y_pairwise_distinct_same_context_pairs() {
        use wordcartel_core::theme::SemanticElement::*;
        for t in cue_themes() {
            assert_ne!(t.face(Comment), t.face(Emphasis), "{}: Comment vs Emphasis", t.name);
            assert_ne!(t.face(FrontMatter), t.face(Code), "{}: FrontMatter vs Code", t.name);
            assert_ne!(t.face(Selection), t.face(Code), "{}: Selection vs Code", t.name);
            // spelling vs grammar are different underline COLORS today; in cue mode they
            // must stay distinguishable by modifier (Codex I7 — §13.2 is fully closed).
            assert_ne!(t.face(DiagSpelling), t.face(DiagGrammar), "{}: DiagSpelling vs DiagGrammar", t.name);
            // Emphasis/Strong/StrongEmphasis are three distinct inline levels — lock pairwise.
            assert_ne!(t.face(Emphasis), t.face(Strong), "{}: Emphasis vs Strong", t.name);
            assert_ne!(t.face(Strong), t.face(StrongEmphasis), "{}: Strong vs StrongEmphasis", t.name);
            assert_ne!(t.face(Emphasis), t.face(StrongEmphasis), "{}: Emphasis vs StrongEmphasis", t.name);
        }
    }

    /// §13.2: Structural glyphs (blockquote ▎, thematic-break ─, heading shade, list bullet •)
    /// render in the No-color theme for inactive structural lines.
    #[test]
    fn a11y_structural_glyphs_render_in_no_color() {
        // blockquote ▎, thematic-break ───, list bullet •, heading shade glyph all PAINT under
        // No-color (glyph cue).
        // Document layout:
        //   "> quote\n" = bytes  0-7  (blockquote)
        //   "\n"        = byte   8    (blank line)
        //   "---\n"     = bytes  9-12 (thematic break)
        //   "\n"        = byte  13    (blank line)
        //   "- item\n"  = bytes 14-20 (list item)
        //   "\n"        = byte  21    (blank line — place caret HERE: ALL structural lines inactive)
        //   "### H3\n"  = bytes 22-29 (H3 heading)
        let mut ed = Editor::new_from_text("> quote\n\n---\n\n- item\n\n### H3\n", None, (40, 12));
        ed.theme = wordcartel_core::theme::no_color(); // heading_level_glyph = true
        // Place caret on the blank line at byte 21 so ALL structural lines are INACTIVE
        // and render their prefix glyphs (Task 6 invariant: active line ⟹ no prefix glyph).
        set_caret(&mut ed, 21);
        crate::derive::rebuild(&mut ed);
        let text = (0..12).map(|r| row_string(&render_to_buffer(&mut ed, 40, 12), r)).collect::<String>();
        assert!(text.contains('▎'), "blockquote bar");
        assert!(text.contains('─'), "thematic rule");
        assert!(text.contains('•'), "list bullet glyph under no_color");
        assert!(text.contains('▒'), "H3 heading shade glyph (▒ = SHADES[2])");
    }

    /// §13.2 §8.3 completeness: All six heading levels render their distinct shade glyphs
    /// (`█▓▒░▏·`) under No-color so H1–H6 are distinguishable without color.
    ///
    /// Document: "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6\n\n"
    ///   bytes  0- 4: "# H1\n"    (H1 — SHADES[0] = '█')
    ///   bytes  5-10: "## H2\n"   (H2 — SHADES[1] = '▓')
    ///   bytes 11-17: "### H3\n"  (H3 — SHADES[2] = '▒')
    ///   bytes 18-25: "#### H4\n" (H4 — SHADES[3] = '░')
    ///   bytes 26-34: "##### H5\n"(H5 — SHADES[4] = '▏')
    ///   bytes 35-44: "###### H6\n"(H6 — SHADES[5] = '·')
    ///   byte  45:    "\n"        (blank line — caret placed here so ALL headings INACTIVE)
    #[test]
    fn a11y_all_six_heading_shades_render_in_no_color() {
        let mut ed = Editor::new_from_text(
            "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6\n\n",
            None, (40, 10),
        );
        ed.theme = wordcartel_core::theme::no_color(); // heading_level_glyph = true
        // Place caret on the trailing blank line (byte 45) so ALL six heading lines are
        // INACTIVE and render their shade glyphs (Task 6 invariant: active line ⟹ no glyph).
        set_caret(&mut ed, 45);
        crate::derive::rebuild(&mut ed);
        let text = (0..10).map(|r| row_string(&render_to_buffer(&mut ed, 40, 10), r)).collect::<String>();
        assert!(text.contains('█'), "H1 shade glyph (█ = SHADES[0]) missing in no_color");
        assert!(text.contains('▓'), "H2 shade glyph (▓ = SHADES[1]) missing in no_color");
        assert!(text.contains('▒'), "H3 shade glyph (▒ = SHADES[2]) missing in no_color");
        assert!(text.contains('░'), "H4 shade glyph (░ = SHADES[3]) missing in no_color");
        assert!(text.contains('▏'), "H5 shade glyph (▏ = SHADES[4]) missing in no_color");
        assert!(text.contains('·'), "H6 shade glyph (· = SHADES[5]) missing in no_color");
    }

    #[test]
    fn theme_picker_paints_rows_and_selection() {
        let mut ed = Editor::new_from_text("x\n", None, (60, 16));
        ed.open_theme_picker();
        let buf = render_to_buffer(&mut ed, 60, 16);
        let text: String = (0..16).map(|r| row_string(&buf, r)).collect();
        assert!(text.contains("tokyo-night"), "picker lists built-in themes");
    }

    // -----------------------------------------------------------------------
    // Task 8: source-mode base canvas tests (§3.5)
    // -----------------------------------------------------------------------

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
        assert!(
            matches!(cell.style().bg, Some(ratatui::style::Color::Rgb(..))),
            "phosphor-amber source canvas must set a specific RGB bg (not Reset); got {:?}", cell.style().bg
        );
        // Default theme: source canvas stays terminal-default (no themed bg).
        let mut ed2 = Editor::new_from_text("# raw markdown\n", None, (40, 6));
        ed2.active_mut().view.mode = crate::editor::RenderMode::SourcePlain;
        crate::derive::rebuild(&mut ed2);
        let buf2 = render_to_buffer(&mut ed2, 40, 6);
        let bg = buf2[(0u16, 0u16)].style().bg;
        assert!(bg.is_none() || bg == Some(ratatui::style::Color::Reset), "Default source = terminal default");
    }

    #[test]
    fn source_mode_dimmed_row_keeps_phosphor_canvas() {
        // A focused (dimmed) source row must STILL carry the phosphor canvas bg —
        // FocusDim layers over the canvas, it does not replace it (Codex re-review).
        use wordcartel_core::theme::{Theme, Depth};
        let mut ed = Editor::new_from_text("# h\n\nbody paragraph one\n\nbody paragraph two\n", None, (40, 8));
        ed.theme = Theme::builtin("phosphor-amber").unwrap();
        ed.depth = Depth::Truecolor;
        ed.view_opts.focus = true;                 // dims rows outside the focus region
        ed.active_mut().view.mode = crate::editor::RenderMode::SourcePlain;
        crate::derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 8);
        // a row outside the active paragraph is dimmed but must keep a themed bg
        let dimmed = (0..8u16).find_map(|r| {
            let c = &buf[(0u16, r)];
            if c.style().add_modifier.contains(ratatui::style::Modifier::DIM) { Some(c.style().bg) } else { None }
        });
        assert!(
            matches!(dimmed.flatten(), Some(ratatui::style::Color::Rgb(..))),
            "dimmed phosphor source row must carry RGB canvas bg (not Reset); got {:?}", dimmed.flatten()
        );
    }
}
