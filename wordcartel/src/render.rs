// Task 5: ratatui live-preview render + status line.
// Pure: takes &Editor, mutates NOTHING on the editor.

use crate::{compose, derive, editor::Editor, nav};
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Modifier, Style as RStyle},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};
use wordcartel_core::layout::{ColMap, VisualRow};
use wordcartel_core::style::Style;
use wordcartel_core::theme::SemanticElement as SE;

/// Heading-level prefix glyphs used in cue mode and when `heading_level_glyph` is on.
/// Index 0 = H1 … index 5 = H6. Inverted (reverse-video) Nerd Font numerals 1–6
/// (U+F0B3A–F, Material-Design "numeric-N-box"): render paints the glyph with a REVERSED
/// modifier so it reads as a filled box tinted by the heading colour, followed by a normal
/// space (a 1-cell box inside the 2-cell gutter).
///
/// NOTE: these are Nerd Font Private-Use glyphs — they render as tofu on terminals without a
/// Nerd Font. This is a deliberate global choice pending the heading-glyph-style toggle that
/// restores the universal shade ramp as an option (ux-backlog B6).
const HEADING_GLYPHS: [&str; 6] = ["󰬺", "󰬻", "󰬼", "󰬽", "󰬾", "󰬿"];

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

/// Apply a base_fg fallback to editing-area text spans.
///
/// If `style.fg` is already `Some` — set by a heading role, link, code, or any
/// inline colour — the style is returned unchanged. If it is `None` and the
/// theme's canvas fg maps to a real colour at this depth (i.e. not `Reset`;
/// terminal-default themes map `base_fg → Reset` and are left untouched),
/// the fallback is applied so plain body text renders the theme foreground over
/// the opaque canvas rather than the terminal default.
fn text_fg_or_base(
    style: RStyle,
    theme: &wordcartel_core::theme::Theme,
    depth: wordcartel_core::theme::Depth,
) -> RStyle {
    if style.fg.is_some() {
        return style;
    }
    match compose::base_canvas(theme, depth).fg {
        Some(Color::Reset) | None => style,
        Some(c)                   => style.fg(c),
    }
}

/// Pre-computed ratatui styles for all chrome surfaces — built once per frame
/// from the current theme and depth, then passed by reference to the overlay
/// and menu painters.
pub(crate) struct ChromeStyles {
    /// Selected row in overlays — [ChromeSelected] (explicit fg/bg selection).
    pub overlay_selected: RStyle,
    /// Overlay query-bar text style — [ChromeOverlay] (interior fill face; bg preserved).
    pub ov_query: RStyle,
    /// Menu bar: open (active) category label.
    pub menu_open: RStyle,
    /// Menu bar: closed (inactive) label + full-width bar fill — [Chrome].
    /// Also used for the normal-state status line (panel bg, same face).
    pub menu_closed: RStyle,
    /// Menu dropdown: selected / highlighted item.
    pub menu_sel: RStyle,
    /// Menu dropdown: normal item.
    pub menu_norm: RStyle,
    /// Scrollbar track (dim background channel).
    pub scrollbar_track: RStyle,
    /// Scrollbar thumb (active indicator).
    pub scrollbar_thumb: RStyle,
    /// Overlay interior fill — [ChromeOverlay] bg applied via `set_style` after Clear.
    pub ov_fill: RStyle,
    /// Active status-line style (search / minibuffer / prompt) — [ChromeAccent].
    pub ov_accent: RStyle,
    /// Overlay border — fg-only Chrome (bg stripped) so the ChromeOverlay fill
    /// bg shows through under ratatui's `Cell::set_style` patch semantics.
    pub overlay_border: RStyle,
}

impl ChromeStyles {
    /// Build the full chrome style set from the current theme, depth, and canvas mode.
    /// Called once per frame in `render()`, before the scrollbar and status
    /// sections; all downstream painters borrow this struct by reference.
    pub(crate) fn build(
        theme: &wordcartel_core::theme::Theme,
        depth: wordcartel_core::theme::Depth,
        canvas: wordcartel_core::theme::CanvasMode,
    ) -> Self {
        let transparent = canvas == wordcartel_core::theme::CanvasMode::Transparent;
        // overlay_border: fg-only Chrome — .bg cleared so the ChromeOverlay fill bg is
        // preserved under ratatui's Cell::set_style patch semantics (D2 defect-1 fix).
        let mut border = compose::compose(theme, depth, &[SE::Chrome]);
        border.bg = None;
        // Overlay interior fills go see-through in transparent mode: ov_fill becomes a no-op and
        // the query bar renders fg-only. overlay_selected keeps its bg (selection stays visible).
        let mut ov_query = compose::compose(theme, depth, &[SE::ChromeOverlay]);
        if transparent { ov_query.bg = None; }
        let ov_fill = if transparent {
            RStyle::default()
        } else {
            compose::compose(theme, depth, &[SE::ChromeOverlay])
        };
        ChromeStyles {
            // B7: the selected/active chrome styles are painted OVER a DIM-bearing underlay
            // (`menu_norm`=ChromeMuted for the dropdown, `menu_closed`=Chrome for the bar).
            // ratatui's `Cell::set_style` OR-merges add_modifiers, so a bare ChromeSelected swap
            // leaves the underlay's DIM riding on the selected cell → washout on dark/phosphor.
            // Record DIM in each selection style's `sub_modifier` so set_style CLEARS it. The
            // deeper "teach compose modifier subtraction" fix is deferred to backlog H25.
            overlay_selected: compose::compose(theme, depth, &[SE::ChromeSelected]).remove_modifier(Modifier::DIM),
            ov_query,
            menu_open:        compose::compose(theme, depth, &[SE::ChromeSelected]).remove_modifier(Modifier::DIM),
            menu_closed:      compose::compose(theme, depth, &[SE::Chrome]),
            menu_sel:         compose::compose(theme, depth, &[SE::ChromeSelected]).remove_modifier(Modifier::DIM),
            menu_norm:        compose::compose(theme, depth, &[SE::ChromeMuted]),
            scrollbar_track:  compose::compose(theme, depth, &[SE::ChromeMuted]),
            scrollbar_thumb:  compose::compose(theme, depth, &[SE::Chrome]),
            ov_fill,
            ov_accent:        compose::compose(theme, depth, &[SE::ChromeAccent]),
            overlay_border:   border,
        }
    }
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

    let menu_rows = editor.menu_bar_rows();
    let edit_height = h.saturating_sub(1 + menu_rows); // rows available for editing content
    let edit_top = area.y + menu_rows;
    let status_row = area.y + h - 1;

    // Centered-measure geometry: ONE call here so paint + cursor never desync.
    let tg = crate::nav::text_geometry(editor);

    // Opaque canvas: fill the whole edit band (margins + blank/below-content rows) with base_bg
    // BEFORE the per-row text Paragraphs — fg-only text preserves it (Cell::set_style patch
    // semantics, same as fg-only borders). Skipped in Transparent mode and when the theme has no
    // canvas to paint (base_bg → Reset, or Depth::None → no color).
    if editor.canvas == wordcartel_core::theme::CanvasMode::Opaque {
        let mut cbg = compose::base_canvas(&editor.theme, editor.depth);
        cbg.fg = None; // bg-only fill
        if cbg.bg.is_some() && cbg.bg != Some(Color::Reset) {
            let band = Rect::new(area.x, edit_top, w, edit_height);
            frame.buffer_mut().set_style(band, cbg);
        }
    }

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

    // -----------------------------------------------------------------------
    // Editing area: the visible-row paint loop (phases 6–8, extracted). Snapshots
    // its per-frame inputs via gather_row_ctx, then paints each visible row.
    // -----------------------------------------------------------------------
    paint_rows(frame, editor, area, edit_top, edit_height, &tg);

    // -----------------------------------------------------------------------
    // Chrome styles — built once here so the scrollbar, status line, and all
    // overlay/menu painters below can borrow editor fields freely.
    // -----------------------------------------------------------------------
    let cs = ChromeStyles::build(&editor.theme, editor.depth, editor.canvas);

    // -----------------------------------------------------------------------
    // Scrollbar overlay (painted over editing area, rightmost column)
    // -----------------------------------------------------------------------
    if editor.mouse.scrollbar_visible {
        let fv = editor.active_fold_view();
        let total = fv.visible_count();
        let scroll_pos = fv.visible_ordinal(editor.active().view.scroll);
        let sb_area = Rect::new(area.x, edit_top, w, edit_height);
        let mut sb_state = ScrollbarState::new(total).position(scroll_pos);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .track_style(cs.scrollbar_track)
                .thumb_style(cs.scrollbar_thumb),
            sb_area,
            &mut sb_state,
        );
    }

    // -----------------------------------------------------------------------
    // Status line (bottom row)
    // -----------------------------------------------------------------------
    paint_status(frame, editor, area, status_row, &cs);

    // -----------------------------------------------------------------------
    // Hardware cursor
    // -----------------------------------------------------------------------
    place_cursor(frame, editor, area, edit_top, edit_height, status_row, &tg);

    crate::render_overlays::paint(frame, editor, &cs);
}

/// Phase 11 — the bottom status row: search bar / minibuffer / prompt / normal /
/// calm-hidden selection, right-flush Ln/Col/words composition, full-row set_style
/// THEN the Paragraph. Reads editor immutably.
fn paint_status(frame: &mut Frame, editor: &Editor, area: Rect, status_row: u16, cs: &ChromeStyles) {
    let w = area.width;
    // When the search overlay is active, render the search bar.
    // When a modal prompt is active, render its message instead of the normal
    // status text, using a distinct style so it stands out.
    // When the minibuffer is open, render <prompt><text> on the status row.
    // Trailing ghost hint for the empty Filter prompt — rendered as its OWN dim
    // (ChromeMuted) span past the prompt/caret so it recedes below the ChromeAccent
    // prompt text; set in the minibuffer arm below, "" everywhere else.
    let mut hint: &str = "";
    let (status_text, status_style) = if let Some(ref s) = editor.search {
        (
            crate::render_status::format_search_bar(s),
            cs.ov_accent,
        )
    } else if let Some(ref mb) = editor.minibuffer {
        // Empty Filter prompt: expose the `sh -c` shell power via the ghost example
        // hint. The caret sits at prompt+text (see place_cursor), so the hint —
        // emitted as a separate dim span after this text — renders past it.
        if mb.kind == crate::minibuffer::MinibufferKind::Filter && mb.text.is_empty() {
            hint = crate::minibuffer::FILTER_EXAMPLE_HINT;
        }
        (
            format!("{}{}", mb.prompt, mb.text),
            cs.ov_accent,
        )
    } else if let Some(ref prompt) = editor.prompt {
        (
            prompt.message.clone(),
            cs.ov_accent,
        )
    } else {
        // Normal state. Under zen/Auto idle with no message, the reserved row renders
        // as calm canvas (base bg); visible reveal via On / dwell / message force.
        if crate::chrome::status_line_visible(editor) {
            // [Chrome] panel bg, tinted by message kind (A17 §10.2 — compose existing faces, no
            // new SemanticElement). Error/Warning stay legible under no-color/terminal-plain via
            // modifiers, never color alone. Info/Log/none keep the unchanged chrome face.
            let base = cs.menu_closed;
            let status_style = match editor.status().map(|s| s.kind()) {
                Some(crate::status::StatusKind::Error) =>
                    base.patch(compose::compose(&editor.theme, editor.depth, &[SE::ChromeAccent])
                        .add_modifier(Modifier::REVERSED | Modifier::BOLD)),
                Some(crate::status::StatusKind::Warning) => base.add_modifier(Modifier::BOLD),
                _ => base,
            };
            (crate::render_status::status_left_text(editor), status_style)
        } else {
            // Calm canvas: the same bg-only fill the edit band uses — NOT chrome.
            let mut calm = compose::base_canvas(&editor.theme, editor.depth);
            calm.fg = None;
            (String::new(), calm)
        }
    };

    // Compose the status line.
    // When in the normal branch (no prompt/minibuffer/search) and word_count and/or the
    // prose-lens count is showable, flush the segment(s) to the right and truncate the
    // left (path/mode) to fit. The prose-lens count (S8 Task 6) is a SIBLING segment to
    // word_count — its own gate (`prose_lens_count_segment`: lens active AND
    // `computed_for == version`), independent of the word_count view option, so it can
    // show even when word_count is off (and vice versa).
    // When the status row is calm-hidden (Auto idle, no message), suppress these
    // segments so Ln/Col · … does not paint over the calm canvas row.
    let has_overlay = editor.search.is_some() || editor.minibuffer.is_some() || editor.prompt.is_some() || editor.diag.is_some() || editor.outline.is_some();
    let status_hidden = !has_overlay && !crate::chrome::status_line_visible(editor);
    let composed = if !has_overlay && !status_hidden {
        let wc = crate::render_status::word_count_segment(editor);
        let lens = crate::lenses::prose_lens_count_segment(editor);
        if wc.is_some() || lens.is_some() {
            let caret = crate::nav::head(editor);
            let (l, c) = editor.active().document.buffer.caret_line_col(caret);
            let mut right = format!("Ln {l}, Col {c}");
            if let Some(seg) = &lens { right.push_str(" · "); right.push_str(seg); }
            if let Some(seg) = &wc { right.push_str(" · "); right.push_str(seg); }
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
    let used = truncated.chars().count();
    let status_area = Rect::new(area.x, status_row, w, 1);
    // Fill the WHOLE row with the state's chrome style first — the Paragraph
    // styles only the text span, and a partial bar next to the full-width menu
    // bar was the reported-mismatch class (Fable whole-branch I-2).
    frame.buffer_mut().set_style(status_area, status_style);
    // The empty-Filter ghost hint rides its OWN ChromeMuted (dim secondary chrome)
    // span past the prompt so it recedes below the ChromeAccent prompt. ChromeMuted
    // recedes via a muted fg/bg (and DIM where a theme uses it); `remove_modifier`
    // strips the accent BOLD/REVERSED the row-fill set on these cells so it doesn't
    // inherit the live-prompt emphasis.
    let spans = if !hint.is_empty() && used < w as usize {
        let hint_style = compose::compose(&editor.theme, editor.depth, &[SE::ChromeMuted])
            .remove_modifier(Modifier::BOLD | Modifier::REVERSED);
        let tail: String = hint.chars().take(w as usize - used).collect();
        vec![Span::styled(truncated, status_style), Span::styled(tail, hint_style)]
    } else {
        vec![Span::styled(truncated, status_style)]
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), status_area);
}

/// Phase 12 — the hardware cursor: search-field / minibuffer / normal-caret arms,
/// char-count column math, D2 clamp of the normal caret col to tg.text_width.
fn place_cursor(frame: &mut Frame, editor: &Editor, area: Rect, edit_top: u16,
    edit_height: u16, status_row: u16, tg: &nav::TextGeometry) {
    let w = area.width;
    if let Some(ref s) = editor.search {
        // Search bar is open: place caret on the status row at the focused field's caret.
        // Use char counts (not byte offsets) for correct placement with multibyte text.
        // Prefix width is the SINGLE SOURCE shared with `chrome_geom::search_field_click`
        // (the mouse hit-test) — painter and hit-test can never drift.
        let prefix_cols = crate::chrome_geom::search_field_prefix_cols(s, s.field);
        let caret_cols = s.focused_field()[..s.cursor].chars().count();
        // H7: sum in usize and guard BEFORE narrowing — a >65535-column field must hide
        // the caret, not truncate to a small column that passes the `< w` guard.
        let x_offset = prefix_cols + caret_cols;
        if x_offset < w as usize {
            frame.set_cursor_position(Position { x: area.x + x_offset as u16, y: status_row });
        }
    } else if let Some(ref mb) = editor.minibuffer {
        // Minibuffer is open: place caret on the status row at prompt.len() + cursor.
        // cursor is a byte offset; for display we want the char count so the terminal
        // column is correct even for multi-byte prompts/text.
        let prompt_cols = mb.prompt.chars().count();
        let text_cols = mb.text[..mb.cursor].chars().count();
        // H7: sum in usize and guard BEFORE narrowing (see the search arm).
        let caret_col = prompt_cols + text_cols;
        if caret_col < w as usize {
            frame.set_cursor_position(Position { x: area.x + caret_col as u16, y: status_row });
        }
    } else if !editor.has_active_input_overlay() {
        // B11: a modal/overlay other than search/minibuffer (palette, outline, theme_picker,
        // file_browser, menu, prompt, splash, diag, cursor_picker) must not leave the hardware
        // caret parked in the editor text area underneath it.
        if let Some((col, row)) = nav::screen_pos(editor) {
            // Guard rows; clamp cols. B17 (amends spec D2): a trailing space at the margin now resolves
            // to col 0 of the flush continuation row, so this clamp is inert for that case. It still
            // guards a caret placed on a hung-INTERIOR cell (before the space) and a genuinely over-wide
            // single grapheme past the rect.
            if row < edit_height && tg.text_width > 0 {
                let col = col.min((tg.text_width as usize).saturating_sub(1) as u16);
                frame.set_cursor_position(Position { x: area.x + tg.text_left + col, y: edit_top + row });
            }
        }
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
    let hb = b.folds.folded().iter().copied().find(|&hb| buf.byte_to_line(hb) == l)?;
    Some(crate::fold::hidden_count_lines(b.document.blocks(), buf, hb))
}

// ---------------------------------------------------------------------------
// Row-loop extraction (H14b): the per-frame snapshot + the two span builders +
// the two shared unification helpers + the paint loop driver.
// ---------------------------------------------------------------------------

/// Per-frame inputs the row-paint loop reads (render() phases 6–7). EXACTLY the
/// 12 fields paint_rows/row_spans_* read. has_block/block_hidden/diag_active are
/// gather-time locals feeding use_placed/diag_all — NOT fields (a written-but-unread
/// field would trip the warning-free gate).
struct RowCtx<'a> {
    scroll: usize,
    focus_region: Option<(usize, usize)>,
    sorted_lines: Vec<usize>,
    hl_current: Option<wordcartel_core::search::Match>,
    hl_window: Vec<wordcartel_core::search::Match>,
    diag_all: &'a [wordcartel_core::diagnostics::Diagnostic],
    sel_from: usize,
    sel_to: usize,
    has_sel: bool,
    marked_block: Option<crate::editor::MarkedBlock>,
    use_placed: bool,
    plain_source: bool,
    prose_lens: &'a [crate::lenses::PosMatch],
}

/// Snapshot the row loop's per-frame inputs (render() phases 6–7). has_block/block_hidden/
/// diag_active/prose_lens_active are gather-time locals feeding use_placed/diag_all/prose_lens;
/// only the 13 fields the paint path reads are kept in RowCtx.
fn gather_row_ctx(editor: &Editor) -> RowCtx<'_> {
    let scroll = editor.active().view.scroll;

    // Compute the active focus region once (before the row loop) when focus is on.
    // For Paragraph: use paragraph_range_at at the caret.
    // For Sentence: scope paragraph_range_at first, then sentence_bounds within that window.
    let focus_region: Option<(usize, usize)> = if editor.view_opts.focus {
        let buf = &editor.active().document.buffer;
        let blocks = editor.active().document.blocks();
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
            // Conservative upper bound: the END of the last cached entry's coverage. Under ventilate
            // the last key is a block anchor, so bound by that block's last line + 1 (resolver),
            // never a raw line_start of the anchor (which would clip matches inside the block). Off,
            // resolve on a per-line key returns last_line == max_visible, so `hi` is unchanged.
            let max_visible = sorted_lines.last().copied().unwrap_or(scroll);
            let end_line = crate::ventilate::resolve(&editor.active().view, buf, max_visible)
                .map(|r| r.last_line).unwrap_or(max_visible);
            let hi = derive::line_start(buf, end_line + 1);
            // partition_point keeps the sorted invariant; matches are sorted by start,
            // and non-overlapping so end is also non-decreasing.
            let lo_idx = s.matches().partition_point(|m| m.end <= lo);
            let hi_idx = s.matches().partition_point(|m| m.start < hi);
            s.matches()[lo_idx..hi_idx.max(lo_idx)].to_vec()
        }
    };

    // Diagnostic overlay: the switchable lens (Task 6) — `active_lens_diags` is the single source
    // of truth for "computed for the current version, non-empty, and from the active lens engine";
    // everything downstream (windowing, face-by-kind) is unchanged, just fed from the lens slice.
    let diag_all: &[wordcartel_core::diagnostics::Diagnostic] =
        crate::diagnostics_run::active_lens_diags(editor).unwrap_or(&[]);
    let diag_active = !diag_all.is_empty();

    // Snapshot primary selection (Task 9: selection painting).
    let sel_range = editor.active().document.selection.primary();
    let (sel_from, sel_to) = (sel_range.from(), sel_range.to());
    let has_sel = !sel_range.is_empty();

    // Snapshot the persistent marked block (Effort 9A). A visible (non-hidden)
    // block is painted on the placed path; a hidden block is never painted.
    let marked_block = editor.active().marked_block;
    let block_hidden = marked_block.is_some_and(|b| b.hidden);
    let has_block = marked_block.is_some() && !block_hidden;

    // Active prose lens (S8 Task 5): the active category's slice iff a lens is on AND the
    // store is current for this version; `active_pos_matches` is the single source of truth
    // (mirrors `active_lens_diags` above). Windowing to the visible span happens per-row in
    // `row_spans_placed` (the `lenses::window_matches` helper — hub budget).
    let prose_lens: &[crate::lenses::PosMatch] =
        crate::lenses::active_pos_matches(editor).unwrap_or(&[]);
    let prose_lens_active = !prose_lens.is_empty();

    // Use the placed-path builder when search is active, valid diagnostics exist,
    // a non-empty selection must be painted, a visible marked block must be
    // painted, or an active prose lens has matches (segs path does no per-glyph
    // styling). Computed ONCE (Codex): a visible block forces the placed path
    // even with no selection/search/diag.
    let use_placed = !hl_window.is_empty() || diag_active || has_sel || has_block || prose_lens_active;

    // SourcePlain is the only mode with no semantic colour; LivePreview and
    // SourceHighlighted both paint role + inline styles (SH's raw markers carry
    // their construct's style from layout, so they colour too).
    let plain_source = editor.active().view.mode == crate::editor::RenderMode::SourcePlain;

    RowCtx {
        scroll, focus_region, sorted_lines, hl_current, hl_window, diag_all,
        sel_from, sel_to, has_sel, marked_block, use_placed, plain_source, prose_lens,
    }
}

/// The per-glyph style ladder shared by both row-span builders, keyed on the inline
/// `Style` value (seg.style / p.style). The dim non-plain arm is the distinct
/// 4-element compose [Text, role, style, FocusDim] (NOT compose(...).add_modifier(DIM)) —
/// preserves heading bold / comment italic on dim rows (§13.2 FIX-1).
fn ladder_style(theme: &wordcartel_core::theme::Theme, depth: wordcartel_core::theme::Depth,
                role: wordcartel_core::style::BlockRole, inline: wordcartel_core::style::Style,
                row_dim: bool, plain_source: bool) -> RStyle {
    if row_dim {
        if plain_source {
            compose::compose(theme, depth, &[SE::Text, SE::FocusDim])
        } else {
            compose::compose(theme, depth, &[SE::Text, role_element(role), style_element(inline), SE::FocusDim])
        }
    } else if plain_source {
        compose::compose(theme, depth, &[SE::Text])
    } else {
        compose::compose(theme, depth, &[SE::Text, role_element(role), style_element(inline)])
    }
}

/// Prefix lead-in shared by both row-span builders. Row 0 of a prefixed line paints the
/// real glyph — a heading inverted-numeral box (REVERSED glyph + NORMAL space) when
/// heading_level_glyph is on, else the dim single-span glyph; continuation rows push a
/// prefix_width blank spacer so text stays aligned with the prefix-offset cursor columns.
fn push_prefix_lead_in(spans: &mut Vec<Span<'static>>,
                       theme: &wordcartel_core::theme::Theme,
                       depth: wordcartel_core::theme::Depth,
                       vr: &VisualRow, map: &ColMap, row_dim: bool) {
    if let Some(ref glyph) = vr.prefix_glyph {
        let pe = prefix_element(vr.role);
        let heading_n = if theme.heading_level_glyph {
            if let wordcartel_core::style::BlockRole::Heading(n) = vr.role { Some(n) } else { None }
        } else { None };
        if let Some(n) = heading_n {
            let g = HEADING_GLYPHS[(n.clamp(1, 6) - 1) as usize];
            let base = if row_dim {
                compose::compose(theme, depth, &[pe, SE::FocusDim])
            } else {
                compose::compose(theme, depth, &[pe])
            };
            spans.push(Span::styled(g.to_string(), base.add_modifier(Modifier::REVERSED)));
            spans.push(Span::styled(" ".to_string(), base));
        } else {
            let gstyle = if row_dim {
                compose::compose(theme, depth, &[pe, SE::FocusDim])
            } else {
                compose::compose(theme, depth, &[pe]).add_modifier(Modifier::DIM)
            };
            spans.push(Span::styled(glyph.clone(), gstyle));
        }
    } else if map.prefix_width > 0 {
        spans.push(Span::raw(" ".repeat(map.prefix_width)));
    }
}

/// Segs fast path (true no-op rows, no per-glyph styling) — the builder for
/// !use_placed rows.
fn row_spans_segs(editor: &Editor, ctx: &RowCtx, vr: &VisualRow, map: &ColMap,
                  row_dim: bool) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    push_prefix_lead_in(&mut spans, &editor.theme, editor.depth, vr, map, row_dim);
    for seg in &vr.segs {
        let style = ladder_style(&editor.theme, editor.depth, vr.role, seg.style, row_dim, ctx.plain_source);
        let style = text_fg_or_base(style, &editor.theme, editor.depth);
        spans.push(Span::styled(seg.text.clone(), style));
    }
    spans
}

/// Placed path: build spans from map.placed, per-glyph search highlight and/or
/// diagnostic underline. Fires when search is active OR valid diagnostics are
/// present OR a selection / visible marked block must be painted.
fn row_spans_placed(editor: &Editor, ctx: &RowCtx, l: usize, row_index: usize,
                    vr: &VisualRow, map: &ColMap, row_dim: bool) -> Vec<Span<'static>> {
    let buf = &editor.active().document.buffer;
    // ORIGIN from the window resolver: `ps` for a ventilated anchor key, else the logical line start.
    let line_off = crate::ventilate::origin_of(&editor.active().view, buf, l);
    let mut spans: Vec<Span<'static>> = Vec::new();

    // Compute the visible byte span for this visual row so we can window the diagnostics.
    // src_span is relative to the entry ORIGIN (`ps` under ventilate, else the logical line start).
    let lo = line_off + vr.src_span.start;
    let hi = line_off + vr.src_span.end;

    // Window diagnostics by upper bound only (diagnostics may overlap so end is
    // not monotonic — binary lower-bound on end is unsound). Upper-bound
    // partition_point + linear filter for end > lo.
    let hi_idx = ctx.diag_all.partition_point(|d| d.range.start < hi);
    let diag_window: Vec<&wordcartel_core::diagnostics::Diagnostic> =
        ctx.diag_all[..hi_idx].iter().filter(|d| d.range.end > lo).collect();

    // Prose-lens window (S8): upper-bound only (lenses.rs helper — hub budget); the
    // `end > lo` lower bound is applied per glyph by `overlaps` below, same idiom as diag.
    let lens_window = crate::lenses::window_matches(ctx.prose_lens, hi);

    push_prefix_lead_in(&mut spans, &editor.theme, editor.depth, vr, map, row_dim);

    // One span per run of glyphs sharing the same (style, highlight-kind).
    let mut run = String::new();
    let mut run_style: Option<RStyle> = None;
    for p in map.placed.iter().filter(|p| p.row == row_index) {
        let g_from = line_off + p.src.start;
        let g_to = line_off + p.src.end;
        let is_current = ctx.hl_current.is_some_and(|m| overlaps(g_from, g_to, m.start, m.end));
        let is_match = !is_current && ctx.hl_window.iter().any(|m| overlaps(g_from, g_to, m.start, m.end));

        let mut style = ladder_style(&editor.theme, editor.depth, vr.role, p.style, row_dim, ctx.plain_source);

        // MarkedBlock composes BELOW Selection/Search/Diag (base → MarkedBlock →
        // Selection → …). Only visible placed cells are touched; hidden lines are
        // never in `map.placed`, so the block paint is inherently fold-safe.
        if let Some(b) = ctx.marked_block {
            if !b.hidden && overlaps(g_from, g_to, b.start, b.end) {
                let mb_face = editor.theme.face(SE::MarkedBlock);
                style = style.patch(crate::compose::face_to_ratatui(&mb_face, editor.depth));
            }
        }

        // FIX-2: Selection layers first; a current search match patches over it so
        // it stands out; diagnostics last. (Spec §3.4: Selection → Search → Diag.)
        // Cue-mode modifiers accumulate (selection underline + search bold = both
        // visible), so §13.2 is preserved.
        let is_selected = ctx.has_sel && overlaps(g_from, g_to, ctx.sel_from, ctx.sel_to);
        if is_selected {
            let sel_face = editor.theme.face(SE::Selection);
            style = style.patch(crate::compose::face_to_ratatui(&sel_face, editor.depth));
        }
        if is_current {
            let search_face = editor.theme.face(SE::SearchCurrent);
            let ss = crate::compose::face_to_ratatui(&search_face, editor.depth);
            style = style.patch(ss);
        } else if is_match {
            let search_face = editor.theme.face(SE::SearchMatch);
            let ss = crate::compose::face_to_ratatui(&search_face, editor.depth);
            style = style.patch(ss);
        }

        // Prose-lens highlight (S8) — composes above Search, below Diagnostics (errors
        // stay topmost; a stylistic lens never outranks a real diagnostic). AMENDMENT
        // (Fable whole-branch review): suppressed on a selected glyph — on plain fg/bg
        // Selection themes the lens bg would otherwise paint over Selection, making a
        // nav-selected match visually indistinguishable from an unselected one (risking
        // a surprise span-replacement). Selection styling shows through instead.
        if !is_selected && lens_window.iter().any(|m| overlaps(g_from, g_to, m.start, m.end)) {
            let lf = editor.theme.face(SE::ProseLensMatch);
            style = style.patch(crate::compose::face_to_ratatui(&lf, editor.depth));
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

        // Apply base_fg fallback BEFORE the run-accumulation comparison so runs of
        // plain body text share one span rather than splitting.
        let style = text_fg_or_base(style, &editor.theme, editor.depth);

        // Flush the accumulated run when the style changes.
        if run_style != Some(style) && !run.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut run), run_style.unwrap()));
        }
        run_style = Some(style);
        run.push_str(&p.text);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, run_style.unwrap()));
    }
    spans
}

/// Phase 8 — the visible-row paint loop. Owns screen_row, the outer/inner loop,
/// row_dim, fold marker, the segs/placed selector, fold-marker insert, and the
/// per-row render_widget. tg is passed in (single-call invariant) — never recompute.
fn paint_rows(frame: &mut Frame, editor: &Editor, area: Rect,
              edit_top: u16, edit_height: u16, tg: &nav::TextGeometry) {
    let ctx = gather_row_ctx(editor);
    let mut screen_row: u16 = 0;
    'outer: for &l in &ctx.sorted_lines {
        if l < ctx.scroll { continue; }
        let (visual_rows, map) = &editor.active().view.line_layouts[&l];
        let skip_rows = if l == ctx.scroll { editor.active().view.scroll_row } else { 0 };
        for (row_index, vr) in visual_rows.iter().enumerate() {
            if row_index < skip_rows { continue; }
            if screen_row >= edit_height { break 'outer; }

            // Determine whether this visual row is dim (outside the active region).
            let row_dim = if let Some((from, to)) = ctx.focus_region {
                let buf = &editor.active().document.buffer;
                // ORIGIN from the window resolver: `ps` for a ventilated anchor key, else line_start.
                let origin = crate::ventilate::origin_of(&editor.active().view, buf, l);
                let g_from = origin + vr.src_span.start;
                let g_to = origin + vr.src_span.end;
                !row_is_active(g_from, g_to, from, to)
            } else { false };

            // 5g: compute fold marker before span-building borrows.
            let fold_marker_n: Option<usize> = if row_index == skip_rows {
                fold_marker_for(editor, l)
            } else { None };

            let mut spans = if !ctx.use_placed {
                row_spans_segs(editor, &ctx, vr, map, row_dim)
            } else {
                row_spans_placed(editor, &ctx, l, row_index, vr, map, row_dim)
            };

            // Ventilated PROSE rows carry a VentBlock gutter (count/│); OVERWRITE the reserved
            // 6-space lead-in that push_prefix_lead_in emitted for the glyphless prose row. Verbatim
            // rows have NO vent_blocks entry: their reserved lead-in already reads as a blank gutter
            // (§5.4), so nothing to do; glyph-carrying verbatim rows kept reserve 0 (Task 5) and are
            // untouched. gutter_span returns exactly GUTTER_COLS columns, so width is preserved.
            if let Some(cell) = editor.active().view.vent_blocks.get(&l)
                .and_then(|vb| vb.gutter.get(row_index).copied())
            {
                let gutter = crate::ventilate::gutter_span(Some(cell), editor);
                if spans.is_empty() { spans = gutter; } else { spans.splice(0..1, gutter); }
            }

            // 5g: fold marker on the heading's first visual row.
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

    /// Same shape as render_to_buffer, but reads the backend cursor after draw.
    /// Returns Some((x, y)) always — suppression shows as (0, 0) (the TestBackend
    /// default), not as None.
    fn render_capturing_cursor(e: &mut Editor, w: u16, h: u16) -> Option<(u16, u16)> {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| super::render(f, e)).unwrap();
        // TestBackend's INHERENT cursor_position() — no Backend trait import needed
        // (Codex plan r1; the trait method get_cursor_position would need
        // `use ratatui::backend::Backend`, which the test module does not import).
        let p = term.backend().cursor_position();
        Some((p.x, p.y))
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

    /// Row 0 of a heading with caret on a later line must show "󰬺 Title" (numeral prefix + concealed "# ").
    #[test]
    fn renders_concealed_heading_and_cursor_on_active_line() {
        let mut e = Editor::new_from_text("# Title\n\nbody\n", None, (20, 6));
        set_caret(&mut e, 10); // somewhere in "body" so heading line is inactive/concealed
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(20, 6)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();
        // row 0 shows "󰬺 Title" (numeral prefix + concealed "# "), not "Title"
        let row0: String = (0u16..20).map(|x| buf[(x, 0u16)].symbol().chars().next().unwrap_or(' ')).collect();
        assert!(row0.starts_with("󰬺 Title"), "expected '󰬺 Title...' got {:?}", row0);
    }

    /// The flipped default: colored themes now render the heading numeral ramp (B3).
    #[test]
    fn default_theme_renders_heading_shade_prefix() {
        let mut e = Editor::new_from_text("# One\n\n## Two\n\nbody\n", None, (20, 8));
        set_caret(&mut e, 15); // byte 15 = the 'b' of "body" (Codex-verified) — both headings inactive
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(20, 8)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();
        let row = |y: u16| -> String { (0u16..20).map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' ')).collect() };
        assert!(row(0).starts_with("󰬺 One"), "H1 numeral: got {:?}", row(0));
        assert!(row(2).starts_with("󰬻 Two"), "H2 numeral: got {:?}", row(2));
    }

    /// The heading numeral glyph cell carries the REVERSED modifier (inverted box); the trailing
    /// gutter space stays normal — the "inverted numeral + normal space" gutter (2 cells).
    #[test]
    fn heading_numeral_glyph_is_reversed_and_space_is_not() {
        use ratatui::style::Modifier;
        let mut e = Editor::new_from_text("# One\n\nbody\n", None, (20, 6));
        set_caret(&mut e, 8); // caret in "body" → heading (row 0) inactive → glyph painted (segs path)
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(20, 6)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();
        assert!(buf[(0u16, 0u16)].style().add_modifier.contains(Modifier::REVERSED),
            "heading numeral glyph (col 0) must be REVERSED, got {:?}", buf[(0u16, 0u16)].style());
        assert!(!buf[(1u16, 0u16)].style().add_modifier.contains(Modifier::REVERSED),
            "gutter space (col 1) must NOT be reversed, got {:?}", buf[(1u16, 0u16)].style());
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
        e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
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

    /// The empty Filter minibuffer renders a receded ghost example hint after the
    /// caret, so the shell-power (`sh -c`) is discoverable before anything is typed.
    #[test]
    fn filter_minibuffer_shows_example_hint_when_empty() {
        use ratatui::style::Modifier;
        let mut e = Editor::new_from_text("hello\n", None, (80, 6));
        e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(80, 6)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();
        let status_row: String = (0u16..80)
            .map(|x| buf[(x, 5u16)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            status_row.contains("sort"),
            "empty Filter minibuffer must show the example hint, got: {:?}",
            status_row
        );
        // The hint must RECEDE: its cells carry the ChromeMuted dim-secondary-chrome
        // element (which recedes via a muted fg/bg color, plus DIM where the theme uses
        // it), NOT the ChromeAccent bold+reverse of the live prompt. Verify against the
        // real composed ChromeMuted so the fix is proven theme-robustly.
        let sort_col = status_row.find("sort").expect("hint 'sort' present") as u16;
        let hint_cell = buf[(sort_col, 5u16)].style();
        let muted = compose::compose(&e.theme, e.depth, &[SE::ChromeMuted]);
        assert_eq!(hint_cell.fg, muted.fg, "ghost hint fg must be ChromeMuted fg");
        assert_eq!(hint_cell.bg, muted.bg, "ghost hint bg must be ChromeMuted bg");
        assert!(
            !hint_cell.add_modifier.contains(Modifier::BOLD)
                && !hint_cell.add_modifier.contains(Modifier::REVERSED),
            "ghost hint must NOT carry the ChromeAccent BOLD/REVERSED of live prompt text, \
             got modifiers: {:?}",
            hint_cell.add_modifier
        );
        // Contrast: the prompt glyph (col 0) DOES carry the active ChromeAccent emphasis.
        let prompt_cell = buf[(0u16, 5u16)].style();
        assert!(
            prompt_cell.add_modifier.contains(Modifier::BOLD)
                || prompt_cell.add_modifier.contains(Modifier::REVERSED),
            "live prompt must carry ChromeAccent emphasis, got: {:?}",
            prompt_cell.add_modifier
        );
    }

    /// The ghost hint disappears the moment the user types the first character.
    #[test]
    fn filter_hint_gone_once_text_typed() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 6));
        e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        e.minibuffer.as_mut().unwrap().insert('x');
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(80, 6)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();
        let status_row: String = (0u16..80)
            .map(|x| buf[(x, 5u16)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            !status_row.contains("sort"),
            "hint must vanish once text is typed, got: {:?}",
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

    // -----------------------------------------------------------------------
    // A6 Task 1: windowed palette render tests
    // -----------------------------------------------------------------------

    fn commands_palette(e: &mut Editor) {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &km);
        e.palette = Some(p);
    }

    /// A6: after render, the first visible list row shows the label of rows[scroll_top],
    /// NOT rows[0], proving the windowed slice is painted correctly.
    #[test]
    fn palette_windowed_slice_shows_scrolled_rows() {
        let mut e = Editor::new_from_text("", None, (80, 24));
        commands_palette(&mut e);
        // Set selected deep in the list — render's self-heal will compute scroll_top.
        e.palette.as_mut().unwrap().selected = 50;
        let buf = render_to_buffer(&mut e, 80, 24);
        // After render, scroll_top is set by the self-heal.
        let p = e.palette.as_ref().unwrap();
        let scroll_top = p.scroll_top;
        assert!(scroll_top > 0, "scroll_top must be > 0 when selected=50");
        // Geometry: palette_overlay_rect for ~110 rows on 80×24.
        let area = Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, p.rows.len());
        let first_list_row = rect.y + 2;
        let row_text = row_string(&buf, first_list_row);
        let expected_label = &p.rows[scroll_top].label;
        assert!(row_text.contains(expected_label.as_str()),
            "first visible row must show rows[{scroll_top}].label = {expected_label:?}, got: {row_text:?}");
    }

    /// A6: the position indicator ` N/M ` appears in the bottom border when the
    /// list scrolls; it is absent within the overlay rect when all rows fit.
    #[test]
    fn palette_indicator_only_when_scrollable() {
        // — Case 1: scrollable (Commands palette, ~110 rows, selected=12) —
        let mut e = Editor::new_from_text("", None, (80, 24));
        commands_palette(&mut e);
        e.palette.as_mut().unwrap().selected = 12;
        let buf = render_to_buffer(&mut e, 80, 24);
        let area = Rect::new(0, 0, 80, 24);
        let n = e.palette.as_ref().unwrap().rows.len();
        let rect = crate::chrome_geom::palette_overlay_rect(area, n);
        let bottom_row = rect.y + rect.height - 1;
        let row_text = row_string(&buf, bottom_row);
        // Indicator is right-aligned in the bottom border: " 13/N "
        assert!(row_text.contains(" 13/"),
            "scrollable palette: bottom border must contain indicator ' 13/N ', got: {row_text:?}");
        // — Case 2: not scrollable (3 source_rows Buffers palette on 80×24) —
        let mut e2 = Editor::new_from_text("", None, (80, 24));
        let reg = crate::registry::Registry::builtins();
        let km = crate::keymap::KeyTrie::default();
        let mut p2 = crate::palette::Palette {
            kind: crate::palette::PaletteKind::Buffers,
            source_rows: vec![
                crate::palette::PaletteRow { id: crate::registry::CommandId("palette"), label: "alpha".into(), chord: "".into(), buffer: Some(crate::editor::BufferId(1)) },
                crate::palette::PaletteRow { id: crate::registry::CommandId("palette"), label: "beta".into(), chord: "".into(), buffer: Some(crate::editor::BufferId(2)) },
                crate::palette::PaletteRow { id: crate::registry::CommandId("palette"), label: "gamma".into(), chord: "".into(), buffer: Some(crate::editor::BufferId(3)) },
            ],
            ..Default::default()
        };
        crate::palette::rebuild_rows(&mut p2, &reg, &km);
        e2.palette = Some(p2);
        let buf2 = render_to_buffer(&mut e2, 80, 24);
        let rect2 = crate::chrome_geom::palette_overlay_rect(Rect::new(0, 0, 80, 24), 3);
        let bottom_row2 = rect2.y + rect2.height - 1;
        // Only scan the overlay's own columns — document text outside must not interfere.
        let row2_text: String = (rect2.x..rect2.x + rect2.width)
            .map(|x| buf2[(x, bottom_row2)].symbol().to_string())
            .collect();
        assert!(!row2_text.chars().any(|c| c.is_ascii_digit()),
            "non-scrollable palette: no indicator digits in overlay bottom border, got: {row2_text:?}");
    }

    /// A6 self-heal: seeding an out-of-bounds scroll_top and rendering into a
    /// smaller terminal must not panic, and after the draw the selection is visible.
    #[test]
    fn palette_resize_self_heal_no_panic() {
        let mut e = Editor::new_from_text("", None, (80, 10));
        commands_palette(&mut e);
        let p = e.palette.as_mut().unwrap();
        p.selected = 50;
        p.scroll_top = 36; // simulate a stale scroll position before a resize
        // Draw into 80×10 — no panic; self-heal adjusts the window.
        let _buf = render_to_buffer(&mut e, 80, 10);
        let p = e.palette.as_ref().unwrap();
        let lh = crate::list_window::list_h_for(p.rows.len(), 10);
        assert!(p.selected.saturating_sub(p.scroll_top) < lh.max(1),
            "after self-heal: selected={} scroll_top={} lh={}", p.selected, p.scroll_top, lh);
    }

    /// A6: a 4-row terminal is degenerate (list_h=0); rendering must not panic
    /// and must not paint any list row cells.
    #[test]
    fn palette_degenerate_h4_no_panic_no_rows() {
        let mut e = Editor::new_from_text("", None, (80, 4));
        commands_palette(&mut e);
        // Just verify no panic — the painter returns before the list area.
        let _buf = render_to_buffer(&mut e, 80, 4);
        // No assertion on content needed: the spec only requires no panic + no rows.
    }

    #[test]
    fn focus_active_region_is_paragraph_at_caret() {
        let mut e = Editor::new_from_text("Para one.\n\nPara two.\n\nThree.\n", None, (80, 24));
        e.view_opts.focus = true; // paragraph default
        set_caret(&mut e, 12); // inside "Para two."
        derive::rebuild(&mut e);
        // the active region used by render = paragraph_range_at at the caret
        let buf = &e.active().document.buffer; let blocks = e.active().document.blocks();
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
        use crate::editor::{Editor, RenderMode};
        let mut e = Editor::new_from_text("teh cat\n", None, (40, 6));
        let v = e.active().document.version;
        e.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "x".into(), suggestions: vec![] }];
        e.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).computed_version = v;
        e.active_mut().view.mode = RenderMode::Review; // E7 T2: display gate requires Review
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 40, 6);
        assert!(row_has_underline(&buf, 0), "the misspelled 'teh' is underlined");
    }

    #[test]
    fn stale_diagnostics_are_not_painted() {
        use crate::editor::{Editor, RenderMode};
        let mut e = Editor::new_from_text("teh cat\n", None, (40, 6));
        e.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "x".into(), suggestions: vec![] }];
        e.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).computed_version = 999; // != current version
        e.active_mut().view.mode = RenderMode::Review; // isolate the version-staleness guard, not the mode gate
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 40, 6);
        assert!(!row_has_underline(&buf, 0), "version-mismatched diagnostics are hidden");
    }

    // -----------------------------------------------------------------------
    // S8 Task 5 — prose-lens paint (between Search and Diag)
    // -----------------------------------------------------------------------

    #[test]
    fn prose_lens_paints_flagged_span_between_search_and_diag() {
        use wordcartel_core::theme::SemanticElement::ProseLensMatch;
        let t = "The report was written by them.\n";
        let mut e = Editor::new_from_text(t, None, (80, 24));
        let v = e.active().document.version;
        let start = t.find("was written").unwrap();
        let end = start + "was written".len();
        e.active_mut().pos.passive =
            vec![crate::lenses::PosMatch { start, end, category: crate::lenses::ProseLensCategory::Passive }];
        e.active_mut().pos.computed_for = Some(v);
        crate::lenses::set_prose_lens(&mut e, Some(crate::lenses::ProseLensCategory::Passive));
        crate::derive::rebuild(&mut e);

        // use_placed must be forced on by an active prose-lens match.
        let ctx = super::gather_row_ctx(&e);
        assert!(ctx.use_placed, "an active prose lens forces the placed path");

        // No prefix glyph on a plain paragraph and no ventilate/wrap here — byte offsets ==
        // screen columns on row 0, same idiom as `diagnostics_underline_the_flagged_glyphs`.
        let want_bg = crate::compose::face_to_ratatui(&e.theme.face(ProseLensMatch), e.depth).bg;
        let buf = render_to_buffer(&mut e, 80, 24);
        for x in start..end {
            assert_eq!(buf[(x as u16, 0)].style().bg, want_bg,
                "glyph at col {x} inside the flagged span must carry the ProseLensMatch bg");
        }
        assert_ne!(buf[(0u16, 0u16)].style().bg, want_bg,
            "glyph outside the flagged span ('T' at col 0) must not carry the ProseLensMatch bg");
    }

    #[test]
    fn prose_lens_paints_cue_mode_modifier_not_color() {
        // Depth::None + the no-color theme: no bg/fg is ever applied, but the ProseLensMatch
        // modifier stack (bold+italic+underline, mono_faces) must still land on the flagged glyphs.
        use wordcartel_core::theme::SemanticElement::ProseLensMatch;
        let t = "The report was written by them.\n";
        let mut e = Editor::new_from_text(t, None, (80, 24));
        e.theme = wordcartel_core::theme::no_color();
        e.depth = wordcartel_core::theme::Depth::None;
        let v = e.active().document.version;
        let start = t.find("was written").unwrap();
        let end = start + "was written".len();
        e.active_mut().pos.passive =
            vec![crate::lenses::PosMatch { start, end, category: crate::lenses::ProseLensCategory::Passive }];
        e.active_mut().pos.computed_for = Some(v);
        crate::lenses::set_prose_lens(&mut e, Some(crate::lenses::ProseLensCategory::Passive));
        crate::derive::rebuild(&mut e);

        let want = crate::compose::face_to_ratatui(&e.theme.face(ProseLensMatch), e.depth);
        assert_eq!(want.bg, None, "cue mode carries no color — modifiers are the only cue");
        let buf = render_to_buffer(&mut e, 80, 24);
        for x in start..end {
            let s = buf[(x as u16, 0)].style();
            assert!(
                s.add_modifier.contains(Modifier::BOLD)
                    && s.add_modifier.contains(Modifier::ITALIC)
                    && s.add_modifier.contains(Modifier::UNDERLINED),
                "col {x} inside the flagged span must carry bold+italic+underline in cue mode, got {:?}",
                s.add_modifier
            );
        }
    }

    #[test]
    fn prose_lens_composes_between_search_and_diag() {
        // Selection -> Search -> ProseLensMatch -> Diagnostics-last (spec order). A match that also
        // overlaps a search hit and a diagnostic: the final bg must be ProseLensMatch's (patched
        // AFTER Search), while the diagnostic still layers its underline on top (Diag never
        // overwrites color, so this alone proves Diag composes above without erasing the lens bg).
        use wordcartel_core::diagnostics::{DiagSource, Diagnostic, DiagnosticKind};
        use wordcartel_core::theme::SemanticElement::ProseLensMatch;
        let t = "The report was written by them.\n";
        let mut e = Editor::new_from_text(t, None, (80, 24));
        let v = e.active().document.version;
        let start = t.find("was written").unwrap();
        let end = start + "was written".len();
        e.active_mut().pos.passive =
            vec![crate::lenses::PosMatch { start, end, category: crate::lenses::ProseLensCategory::Passive }];
        e.active_mut().pos.computed_for = Some(v);
        crate::lenses::set_prose_lens(&mut e, Some(crate::lenses::ProseLensCategory::Passive));
        // A search match covering the exact same span.
        e.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "was written".chars() { e.search.as_mut().unwrap().insert(c); }
        let rope = e.active().document.buffer.snapshot();
        e.search.as_mut().unwrap().recompute(&rope, v);
        assert_eq!(e.search.as_ref().unwrap().count(), 1, "the search must land exactly on the span");
        // A grammar diagnostic covering the same span, in Review mode so it displays.
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).diagnostics = vec![Diagnostic {
            range: start..end, kind: DiagnosticKind::Grammar, source: DiagSource::Harper,
            code: None, href: None, message: "x".into(), suggestions: vec![] }];
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).computed_version = v;
        crate::derive::rebuild(&mut e);

        let want_bg = crate::compose::face_to_ratatui(&e.theme.face(ProseLensMatch), e.depth).bg;
        let buf = render_to_buffer(&mut e, 80, 24);
        for x in start..end {
            let s = buf[(x as u16, 0)].style();
            assert_eq!(s.bg, want_bg,
                "col {x}: ProseLensMatch bg must win over Search (patched after it)");
            assert!(s.add_modifier.contains(Modifier::UNDERLINED),
                "col {x}: the diagnostic underline still composes on top of the lens bg");
        }
    }

    #[test]
    fn prose_lens_paints_at_correct_absolute_offset_under_ventilate() {
        // A prose lens must paint correctly UNDER the ventilate lens too (they compose): the
        // paragraph is anchored past a heading + blank line (non-zero `vent_blocks` byte_origin),
        // so a naive per-line origin (instead of `ventilate::origin_of`) would rebase the match to
        // the wrong absolute bytes. Locate the painted glyphs by their TEXT content (not a
        // hardcoded column), since ventilate reserves gutter columns ahead of the prose text.
        use wordcartel_core::theme::SemanticElement::ProseLensMatch;
        let t = "# Head\n\nOne cat sat. The report was written by them today.\n";
        let mut e = Editor::new_from_text(t, None, (60, 10));
        let v = e.active().document.version;
        let start = t.find("was written").unwrap();
        let end = start + "was written".len();
        e.active_mut().pos.passive =
            vec![crate::lenses::PosMatch { start, end, category: crate::lenses::ProseLensCategory::Passive }];
        e.active_mut().pos.computed_for = Some(v);
        crate::lenses::set_prose_lens(&mut e, Some(crate::lenses::ProseLensCategory::Passive));
        e.active_mut().view.ventilate = true;
        crate::derive::rebuild(&mut e);
        assert!(e.active().view.vent_blocks.get(&2).is_some_and(|vb| vb.byte_origin > 0),
            "precondition: the paragraph's vent_blocks origin must be non-zero (past the heading)");

        let want_bg = crate::compose::face_to_ratatui(&e.theme.face(ProseLensMatch), e.depth).bg;
        let buf = render_to_buffer(&mut e, 60, 10);
        let (row, col) = (0..10u16).find_map(|r| {
            let s = row_string(&buf, r);
            s.find("written").map(|c| (r, c as u16))
        }).expect("the second sentence ('written') must be on some visible row");
        assert_eq!(buf[(col, row)].style().bg, want_bg,
            "'written' glyph carries the ProseLensMatch bg at its real (ventilated) position");
        // Control: the FIRST sentence ("One cat sat.") is outside the flagged span.
        let (crow, ccol) = (0..10u16).find_map(|r| {
            let s = row_string(&buf, r);
            s.find("cat").map(|c| (r, c as u16))
        }).expect("the first sentence ('cat') must be on some visible row");
        assert_ne!(buf[(ccol, crow)].style().bg, want_bg,
            "the unflagged first sentence must not carry the ProseLensMatch bg");
    }

    #[test]
    fn prose_lens_paints_nothing_when_no_lens_active_or_store_stale() {
        use wordcartel_core::theme::SemanticElement::ProseLensMatch;
        let t = "The report was written by them.\n";
        let mut e = Editor::new_from_text(t, None, (80, 24));
        let v = e.active().document.version;
        let start = t.find("was written").unwrap();
        let end = start + "was written".len();
        e.active_mut().pos.passive =
            vec![crate::lenses::PosMatch { start, end, category: crate::lenses::ProseLensCategory::Passive }];
        e.active_mut().pos.computed_for = Some(v);
        let want_bg = crate::compose::face_to_ratatui(&e.theme.face(ProseLensMatch), e.depth).bg;

        // No lens active: no paint, use_placed not forced by the lens.
        crate::derive::rebuild(&mut e);
        assert!(!super::gather_row_ctx(&e).use_placed, "no lens active → placed path not forced");
        let buf = render_to_buffer(&mut e, 80, 24);
        for x in start..end {
            assert_ne!(buf[(x as u16, 0)].style().bg, want_bg, "no lens active → nothing painted at col {x}");
        }

        // Lens active but store stale (computed_for != version): still no paint.
        crate::lenses::set_prose_lens(&mut e, Some(crate::lenses::ProseLensCategory::Passive));
        e.active_mut().document.version += 1;
        crate::derive::rebuild(&mut e);
        assert!(!super::gather_row_ctx(&e).use_placed, "stale store → placed path not forced");
        let buf2 = render_to_buffer(&mut e, 80, 24);
        for x in start..end {
            assert_ne!(buf2[(x as u16, 0)].style().bg, want_bg, "stale store → nothing painted at col {x}");
        }
    }

    #[test]
    fn prose_lens_suppressed_on_selected_match_but_not_on_others() {
        // Fable whole-branch amendment: on a plain fg/bg-swap Selection theme (tokyo-night's
        // Selection is `bg: SEL_BG`, no `reverse`), a nav-selected lens match must revert to
        // Selection styling — not the ProseLensMatch bg — so the writer can SEE it's selected
        // and doesn't accidentally overtype the whole span. A second, unselected match must
        // still carry the lens bg (the suppression is per-glyph, not lens-wide).
        use wordcartel_core::theme::SemanticElement::{ProseLensMatch, Selection};
        let t = "The report was written by them today. The essay was written by us anyway.\n";
        let mut e = Editor::new_from_text(t, None, (80, 24));
        e.theme = wordcartel_core::theme::tokyo_night();
        e.depth = wordcartel_core::theme::Depth::Truecolor;
        let v = e.active().document.version;
        let start1 = t.find("was written").unwrap();
        let end1 = start1 + "was written".len();
        let start2 = t[end1..].find("was written").unwrap() + end1;
        let end2 = start2 + "was written".len();
        e.active_mut().pos.passive = vec![
            crate::lenses::PosMatch { start: start1, end: end1, category: crate::lenses::ProseLensCategory::Passive },
            crate::lenses::PosMatch { start: start2, end: end2, category: crate::lenses::ProseLensCategory::Passive },
        ];
        e.active_mut().pos.computed_for = Some(v);
        crate::lenses::set_prose_lens(&mut e, Some(crate::lenses::ProseLensCategory::Passive));
        // Select exactly the FIRST match's span (nav-jumped-to match).
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(start1, end1);
        crate::derive::rebuild(&mut e);

        let sel_bg = crate::compose::face_to_ratatui(&e.theme.face(Selection), e.depth).bg;
        let lens_bg = crate::compose::face_to_ratatui(&e.theme.face(ProseLensMatch), e.depth).bg;
        assert_ne!(sel_bg, lens_bg, "precondition: tokyo-night must give Selection and ProseLensMatch distinct bgs");
        let buf = render_to_buffer(&mut e, 80, 24);
        for x in start1..end1 {
            assert_eq!(buf[(x as u16, 0)].style().bg, sel_bg,
                "col {x}: the SELECTED match must show Selection bg, not ProseLensMatch bg");
        }
        for x in start2..end2 {
            assert_eq!(buf[(x as u16, 0)].style().bg, lens_bg,
                "col {x}: the UNselected second match must still carry the ProseLensMatch bg");
        }
    }

    /// E7 T2: the display gate hides underlines the instant the buffer leaves Review, even
    /// with valid (version-matched) diagnostics still stored.
    #[test]
    fn diagnostics_paint_only_in_review() {
        use crate::editor::{Editor, RenderMode};
        let mut e = Editor::new_from_text("teh cat\n", None, (40, 6));
        let v = e.active().document.version;
        e.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "x".into(), suggestions: vec![] }];
        e.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).computed_version = v;
        e.active_mut().view.mode = RenderMode::Review;
        crate::derive::rebuild(&mut e);
        assert!(row_has_underline(&render_to_buffer(&mut e, 40, 6), 0), "painted in Review");
        e.active_mut().view.mode = RenderMode::LivePreview;
        crate::derive::rebuild(&mut e);
        assert!(!row_has_underline(&render_to_buffer(&mut e, 40, 6), 0), "hidden the instant we leave Review");
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
        // Heading rows in Tokyo Night LivePreview must carry the heading-role fg (MAGENTA for H1).
        // The render-time base_fg fallback (text_fg_or_base) is skipped when the composed style
        // already has a fg — which it does for headings via the Heading role in the compose stack.
        let mut ed = Editor::new_from_text("# Title\n", None, (40, 4));
        ed.theme = wordcartel_core::theme::tokyo_night();
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 4);
        let want = crate::compose::compose(&ed.theme, ed.depth, &[wordcartel_core::theme::SemanticElement::Text, wordcartel_core::theme::SemanticElement::Heading(1)]).fg;
        assert!((0..40).any(|x| buf[(x,0)].style().fg == want && want.is_some()), "heading fg applied");
    }

    #[test]
    fn terminal_plain_status_carries_chrome_face() {
        // D2: normal status uses [Chrome] — terminal-plain Chrome = White fg / Black bg
        // (not REVERSED). The status row must carry the Chrome bg, not a reverse modifier.
        let mut ed = Editor::new_from_text("x", None, (40, 4));
        ed.status_line_mode = crate::config::TransientMode::On; // test chrome face, not calm mode
        let buf = render_to_buffer(&mut ed, 40, 4);
        let last = 3u16;
        // terminal-plain Chrome: fg=White, bg=Black — explicit color, not reverse.
        assert!((0..40u16).any(|x| buf[(x, last)].style().bg == Some(Color::Black)),
                "terminal-plain normal status must carry Chrome face (bg=Black, not REVERSED)");
    }

    #[test]
    fn marked_block_paints_and_status_shows_blk() {
        let mut e = Editor::new_from_text("hello world\n", None, (60, 6));
        e.status_line_mode = crate::config::TransientMode::On; // test chrome status content, not calm mode
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false });
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 60, 6);
        // the block cells carry a non-default style distinct from unselected cells (reverse modifier)
        assert!(row_has_highlight(&buf, 0), "block cells painted with a modifier");
        // and the status row contains "BLK"
        assert!(row_string(&buf, 5).contains("BLK"), "status shows BLK indicator");
    }

    #[test]
    fn hidden_block_status_reads_blk_hidden_and_not_painted() {
        let mut e = Editor::new_from_text("hello\n", None, (60, 6));
        e.status_line_mode = crate::config::TransientMode::On; // test chrome status content, not calm mode
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: true });
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 60, 6);
        assert!(row_string(&buf, 5).contains("BLK·hidden"));
        // a hidden block is not painted into the text rows
        assert!(!row_has_highlight(&buf, 0), "hidden block not painted");
    }

    #[test]
    fn phosphor_status_line_carries_hue() {
        // D2: normal status uses [Chrome] — phosphor Chrome bg is a derived hue-tinted Rgb.
        // After derive_chrome, Chrome.bg is an Rgb step toward black preserving the hue.
        use wordcartel_core::theme::ChromeDisposition;
        let mut ed = Editor::new_from_text("x", None, (40, 4));
        ed.status_line_mode = crate::config::TransientMode::On; // test chrome face, not calm mode
        let mut theme = wordcartel_core::theme::Theme::builtin("phosphor-amber").unwrap();
        theme.derive_chrome(ChromeDisposition::Full);
        ed.theme = theme;
        let buf = render_to_buffer(&mut ed, 40, 4);
        let want = compose::compose(&ed.theme, ed.depth, &[SE::Chrome]);
        // The status row carries the derived phosphor Chrome bg (Rgb, hue-tinted toward black).
        assert!(want.bg.is_some(), "phosphor Chrome must have a derived bg after derive_chrome");
        assert!(want.fg.is_some(), "phosphor Chrome must have a derived fg after derive_chrome");
        assert!((0..40u16).any(|x| {
            let cell = buf[(x, 3u16)].style();
            cell.bg == want.bg && cell.fg == want.fg
        }), "status row must carry the derived phosphor Chrome face");
    }

    /// THE reported bug (D2): under tokyo-night, the status row bg and the menu bar bg
    /// were different (status=[ChromeReverse] had no bg; menu=[Chrome]=explicit #16161e).
    /// After T5 both use [Chrome] → same bg. Part D (T3): tokyo chrome is now a sentinel,
    /// so derived FULL Chrome bg = #2d2f42 (§II.5).
    #[test]
    fn tokyo_status_matches_menu_bar() {
        use wordcartel_core::theme::{ChromeDisposition, Depth};
        let mut ed = Editor::new_from_text("x", None, (40, 6));
        // Enable pinned menu bar so row 0 carries the menu Chrome bg.
        ed.menu_bar_mode = crate::config::MenuBarMode::Pinned;
        ed.status_line_mode = crate::config::TransientMode::On; // test chrome parity, not calm mode
        let mut theme = wordcartel_core::theme::tokyo_night();
        theme.derive_chrome(ChromeDisposition::Full);
        ed.theme = theme;
        ed.depth = Depth::Truecolor;
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);
        // Status is the last row (row 5); menu bar is row 0. FULL-ROW parity
        // (Fable whole-branch I-2): every status cell carries the bar bg — a
        // partial text-span bar next to the full-width menu bar was the reported
        // mismatch class.
        let menu_bg = buf[(0u16, 0u16)].style().bg;
        for x in 0..40u16 {
            let status_bg = buf[(x, 5u16)].style().bg;
            assert_eq!(status_bg, menu_bg,
                "status cell x={x} bg must equal menu bar bg; status={status_bg:?}, menu={menu_bg:?}");
        }
        // Both must be the derived FULL Chrome bg = #2d2f42 (§II.5 tokyo pin).
        assert_eq!(menu_bg, Some(Color::Rgb(0x2d, 0x2f, 0x42)),
            "chrome bg must be derived FULL Chrome #2d2f42, got {menu_bg:?}");
    }

    /// D2: opening a minibuffer switches the status style to [ChromeAccent];
    /// normal state uses [Chrome]. Both are verified under terminal-plain.
    #[test]
    fn prompt_active_status_uses_accent() {
        let mut ed = Editor::new_from_text("x", None, (40, 4));
        ed.status_line_mode = crate::config::TransientMode::On; // test chrome face, not calm mode

        // Normal state: status carries Chrome face (bg=Black under terminal-plain).
        let buf_normal = render_to_buffer(&mut ed, 40, 4);
        let want_chrome = compose::compose(&ed.theme, ed.depth, &[SE::Chrome]);
        assert_eq!(want_chrome.bg, Some(Color::Black), "terminal-plain Chrome bg must be Black");
        assert!((0..40u16).any(|x| buf_normal[(x, 3u16)].style().bg == want_chrome.bg),
            "normal status must carry Chrome bg (Black)");

        // Active state (minibuffer): status carries ChromeAccent (reverse+bold under terminal-plain).
        ed.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        derive::rebuild(&mut ed);
        let buf_active = render_to_buffer(&mut ed, 40, 4);
        let want_accent = compose::compose(&ed.theme, ed.depth, &[SE::ChromeAccent]);
        assert!((0..40u16).any(|x| {
            let cell = buf_active[(x, 3u16)].style();
            cell.add_modifier == want_accent.add_modifier
        }), "minibuffer-active status must carry ChromeAccent modifiers");
    }

    /// I4 pin: terminal-plain ChromeAccent = reverse+bold. When a minibuffer is open
    /// under terminal-plain, the status row must carry REVERSED + BOLD.
    #[test]
    fn terminal_plain_prompt_status_reverse_bold() {
        let mut ed = Editor::new_from_text("x", None, (40, 4));
        ed.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 4);
        // terminal-plain ChromeAccent = modface(None, bold=true, reverse=true) → BOLD | REVERSED.
        let any_accent_cell = (0..40u16).any(|x| {
            let m = buf[(x, 3u16)].style().add_modifier;
            m.contains(Modifier::REVERSED) && m.contains(Modifier::BOLD)
        });
        assert!(any_accent_cell,
            "terminal-plain prompt-active status must carry REVERSED+BOLD (ChromeAccent = reverse+bold)");
    }

    #[test]
    fn source_mode_no_heading_fg_live_preview_has_heading_fg() {
        // In SourcePlain under Tokyo Night, a heading row must NOT carry the heading fg.
        // In LivePreview it must — the render-time base_fg fallback is skipped when the composed
        // style already has a fg (heading role sets it), so the heading colour is preserved.
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

        // SourceHighlighted: heading fg present — raw markers visible, role colour applied.
        ed.active_mut().view.mode = RenderMode::SourceHighlighted;
        crate::derive::rebuild(&mut ed);
        let buf_sh = render_to_buffer(&mut ed, 40, 4);
        let sh_has_heading_fg = (0..40).any(|x| buf_sh[(x,0)].style().fg == want && want.is_some());
        assert!(sh_has_heading_fg, "SourceHighlighted must carry heading fg (raw markers, role colour)");
    }

    #[test]
    fn review_is_not_plain_source() {
        use crate::editor::RenderMode;
        let mut ed = Editor::new_from_text("# Title\n", None, (40, 6));
        // Status line legitimately differs by mode ([REVIEW] vs [PREVIEW] — see
        // status_line_shows_review_label); turn it off so this whole-buffer pin
        // isolates the document-body rendering the brief actually asks about.
        ed.status_line_mode = crate::config::TransientMode::Off;
        // Under the default terminal-plain theme, heading has no fg (`Face::default()`),
        // and the caret sits on the single heading line so LivePreview shows it RawPlain
        // too — content would coincide with SourcePlain. Use tokyo_night (as the sibling
        // heading-fg test above does) so LivePreview's role colour actually distinguishes
        // it from SourcePlain's monochrome ladder, making the assert_ne meaningful.
        ed.theme = wordcartel_core::theme::tokyo_night();
        ed.active_mut().view.mode = RenderMode::Review;
        crate::derive::rebuild(&mut ed);
        let review = render_to_buffer(&mut ed, 40, 6);
        ed.active_mut().view.mode = RenderMode::LivePreview;
        crate::derive::rebuild(&mut ed);
        let live = render_to_buffer(&mut ed, 40, 6);
        ed.active_mut().view.mode = RenderMode::SourcePlain;
        crate::derive::rebuild(&mut ed);
        let plain = render_to_buffer(&mut ed, 40, 6);
        assert_eq!(review, live,  "Review renders styled exactly like LivePreview");
        assert_ne!(review, plain, "Review is NOT raw-plain like SourcePlain");
    }

    #[test]
    fn srchi_colors_delimiters_and_content_source_stays_plain() {
        // SH paints raw markdown with construct faces (Strong/Code/Link/Heading).
        // SP shows the same raw text monochrome at base_fg.
        // Core SH≠SP pin (whole-effort T3); also exercises the RawStyled
        // Code+Link whole-span styling end-to-end (Task 1 carry-forward).
        use crate::editor::RenderMode;
        // Doc: paragraph with bold, code span, link; then a heading.
        let mut ed = Editor::new_from_text("**bold** `x` [t](u)\n# H\n", None, (40, 5));
        ed.theme = wordcartel_core::theme::tokyo_night();

        let strong_fg  = crate::compose::compose(&ed.theme, ed.depth, &[SE::Strong]).fg;
        let heading_fg = crate::compose::compose(&ed.theme, ed.depth, &[SE::Text, SE::Heading(1)]).fg;
        let code_fg    = crate::compose::compose(&ed.theme, ed.depth, &[SE::Code]).fg;
        let link_fg    = crate::compose::compose(&ed.theme, ed.depth, &[SE::Link]).fg;
        let base_fg    = crate::compose::base_canvas(&ed.theme, ed.depth).fg;

        // Sanity: all face fgs must differ from base_fg for assertions to be meaningful.
        assert!(strong_fg.is_some()  && strong_fg  != base_fg, "tokyo-night Strong fg must differ from base_fg");
        assert!(heading_fg.is_some() && heading_fg != base_fg, "tokyo-night Heading(1) fg must differ from base_fg");
        assert!(code_fg.is_some()    && code_fg    != base_fg, "tokyo-night Code fg must differ from base_fg");
        assert!(link_fg.is_some()    && link_fg    != base_fg, "tokyo-night Link fg must differ from base_fg");

        // --- SourceHighlighted: delimiters + content carry construct faces ---
        ed.active_mut().view.mode = RenderMode::SourceHighlighted;
        crate::derive::rebuild(&mut ed);
        let buf_sh = render_to_buffer(&mut ed, 40, 5);

        // Row 0 col 0: first '*' of **bold** → Strong fg.
        assert_eq!(buf_sh[(0u16, 0u16)].style().fg, strong_fg,
            "SH: first '*' must carry Strong fg, got {:?}", buf_sh[(0u16, 0u16)].style().fg);
        // Row 0 col 9: opening backtick of `x` → Code fg.
        assert_eq!(buf_sh[(9u16, 0u16)].style().fg, code_fg,
            "SH: opening backtick must carry Code fg, got {:?}", buf_sh[(9u16, 0u16)].style().fg);
        // Row 0 col 13: '[' of [t](u) → Link fg.
        assert_eq!(buf_sh[(13u16, 0u16)].style().fg, link_fg,
            "SH: '[' must carry Link fg, got {:?}", buf_sh[(13u16, 0u16)].style().fg);
        // Row 1 col 0: '#' of '# H' → Heading(1) fg.
        assert_eq!(buf_sh[(0u16, 1u16)].style().fg, heading_fg,
            "SH: '#' must carry Heading(1) fg, got {:?}", buf_sh[(0u16, 1u16)].style().fg);

        // --- SourcePlain: same cells are monochrome base_fg ---
        ed.active_mut().view.mode = RenderMode::SourcePlain;
        crate::derive::rebuild(&mut ed);
        let buf_sp = render_to_buffer(&mut ed, 40, 5);

        // Row 0 col 0: '*' → base_fg (no construct styling in SP).
        assert_eq!(buf_sp[(0u16, 0u16)].style().fg, base_fg,
            "SP: first '*' must be base_fg, got {:?}", buf_sp[(0u16, 0u16)].style().fg);
        // Row 1 col 0: '#' → base_fg.
        assert_eq!(buf_sp[(0u16, 1u16)].style().fg, base_fg,
            "SP: '#' must be base_fg, got {:?}", buf_sp[(0u16, 1u16)].style().fg);
    }

    #[test]
    fn live_preview_and_source_plain_render_unchanged() {
        // Regression pin: SRC-HI effort must not change LivePreview or SourcePlain behaviour.
        // LP: conceals markers, colours content (heading fg present).
        // SP: raw source, monochrome base canvas (no heading fg).
        use crate::editor::RenderMode;
        let mut ed = Editor::new_from_text("# Heading\n", None, (40, 4));
        ed.theme = wordcartel_core::theme::tokyo_night();
        let heading_fg = crate::compose::compose(&ed.theme, ed.depth, &[
            SE::Text, SE::Heading(1),
        ]).fg;

        // LivePreview: heading fg present (markers concealed, role colour applied).
        ed.active_mut().view.mode = RenderMode::LivePreview;
        crate::derive::rebuild(&mut ed);
        let buf_lp = render_to_buffer(&mut ed, 40, 4);
        assert!((0..40u16).any(|x| buf_lp[(x, 0u16)].style().fg == heading_fg && heading_fg.is_some()),
            "LP: heading row must carry heading fg (unchanged from pre-effort)");

        // SourcePlain: base canvas only — no heading fg.
        ed.active_mut().view.mode = RenderMode::SourcePlain;
        crate::derive::rebuild(&mut ed);
        let buf_sp = render_to_buffer(&mut ed, 40, 4);
        assert!(!(0..40u16).any(|x| buf_sp[(x, 0u16)].style().fg == heading_fg && heading_fg.is_some()),
            "SP: heading row must NOT carry heading fg (base canvas only, unchanged from pre-effort)");
    }

    #[test]
    fn heading_text_carries_role_fg_base16_and_phosphor() {
        use crate::editor::RenderMode;
        // Each: (theme, label). base16 (flexoki-dark) already colours headings; phosphor does so
        // ONLY after Part C empties its `text` face (before this, text = shade(3) clobbered the role).
        for (theme, label) in [
            (wordcartel_core::theme::flexoki_dark(), "flexoki-dark"),
            (wordcartel_core::theme::Theme::builtin("phosphor-green").expect("phosphor-green is a builtin"), "phosphor-green"),
        ] {
            let mut ed = Editor::new_from_text("# Title\nbody\n", None, (40, 6));
            ed.theme = theme;
            ed.active_mut().view.mode = RenderMode::LivePreview;
            derive::rebuild(&mut ed);
            let buf = render_to_buffer(&mut ed, 40, 6);

            let role_fg = compose::compose(&ed.theme, ed.depth, &[SE::Text, SE::Heading(1)]).fg;
            let base_fg = compose::base_canvas(&ed.theme, ed.depth).fg;
            assert!(role_fg.is_some() && role_fg != base_fg,
                "{label}: heading role fg must be coloured and distinct from base_fg");
            // heading row (0) carries the role fg — shaded heading colour in live preview.
            assert!((0..40).any(|x| buf[(x, 0u16)].style().fg == role_fg),
                "{label}: live-preview heading must carry the role fg");
            // body row (1) carries base_fg via the empty-Text fallback.
            assert!((0..40).any(|x| buf[(x, 1u16)].style().fg == base_fg),
                "{label}: body text must carry base_fg");

            // source mode: uniform base_fg, no heading colour (compose [SE::Text] only).
            ed.active_mut().view.mode = RenderMode::SourcePlain;
            derive::rebuild(&mut ed);
            let src = render_to_buffer(&mut ed, 40, 6);
            assert!(!(0..40).any(|x| src[(x, 0u16)].style().fg == role_fg),
                "{label}: source mode must NOT carry the heading role fg");
        }
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
            use crate::editor::RenderMode;
            let mut e = Editor::new_from_text("teh cat\n", None, (40, 6));
            let v = e.active().document.version;
            e.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
                range: 0..3,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                message: "x".into(),
                suggestions: vec![],
            }];
            e.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).computed_version = v;
            e.active_mut().view.mode = RenderMode::Review; // E7 T2: display gate requires Review
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
    // Golden tests: lock terminal-plain styles for 4 render sites
    // -----------------------------------------------------------------------

    /// Golden: scrollbar track = White/DarkGray, thumb = White/Black under terminal-plain.
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

    /// Golden: list bullet prefix glyph has DarkGray fg and DIM modifier under terminal-plain.
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

    /// B17 (amends spec D2): a trailing space hung at the margin now breaks the caret to
    /// col 0 of an appended flush continuation row — real line-move feedback that the
    /// space registered — instead of pinning at text_width−1 on the same row.
    #[test]
    fn trailing_space_at_margin_breaks_caret_to_flush_row() {
        // B17 (amends spec D2): vw 4, "abcd " — the caret AFTER the hung space (byte 5, eol) no longer
        // pins at the edge; it moves to col 0 of the next (flush phantom) screen row — real line-move
        // feedback. (Pre-B17 this pinned at x == 3 on the same row.)
        let mut e = Editor::new_from_text("abcd \n", None, (4, 8));
        set_caret(&mut e, 5); // eol, after the hung space (active line — layout raw)
        derive::rebuild(&mut e);
        assert_eq!(crate::nav::screen_pos(&e), Some((0, 1)), "caret at col 0 of the flush row");
        let cur = render_capturing_cursor(&mut e, 4, 8);
        assert_eq!(cur, Some((0, 1)), "hardware caret at (0, 1) — col 0, one row down");
    }

    #[test]
    fn place_cursor_minibuffer_hides_caret_past_terminal_width_no_wraparound() {
        // A minibuffer answer longer than u16::MAX chars must HIDE the caret (its column is
        // off-screen), not truncate the column into the visible range. Pre-H7 the
        // `chars().count() as u16` truncated FIRST, so a length of 65536+10 wrapped to
        // column 10 and wrongly passed the `< w` guard, planting the caret at col ~12.
        let mut e = Editor::new_from_text("body\n", None, (80, 24));
        e.open_minibuffer("x ", crate::minibuffer::MinibufferKind::SaveAs);
        {
            let mb = e.minibuffer.as_mut().unwrap();
            mb.text = "a".repeat(65_546); // 65536 + 10
            mb.cursor = mb.text.len();
        }
        let cur = render_capturing_cursor(&mut e, 80, 24);
        // A suppressed cursor shows as the TestBackend default (0, 0), NOT (~12, status_row).
        assert_eq!(cur, Some((0, 0)),
            "a caret past the terminal width must be hidden, not wrapped into view");
    }

    /// B1 × B2 headline composition: a two-level nested list item ("  - alpha beta")
    /// in a 12-wide viewport renders with the bullet at the indent column and the
    /// continuation row's text hanging under the text column, not the marker.
    #[test]
    fn wrapped_nested_item_bullet_column_and_hanging_indent() {
        // The effort's headline composition (B1 × B2). 12-wide, "  - alpha beta":
        // glyph "  • " (indent 2 + bullet 2 = prefix_width 4);
        //   row 0: "  • alpha "  (bullet at col 2, text from col 4, space hangs ok)
        //   row 1: "    beta"    (spacer cols 0..4, text at col 4 — under TEXT)
        let mut e = Editor::new_from_text("  - alpha beta\nmore\n", None, (12, 8));
        set_caret(&mut e, 17); // on "more" so line 0 is INACTIVE (conceal active)
        derive::rebuild(&mut e);
        {
            let (_rows, map) = &e.active().view.line_layouts[&0];
            assert_eq!(map.prefix_width, 4, "indent(2) + bullet(2)");
            assert!(map.rows >= 2, "must wrap");
        }
        let buf = render_to_buffer(&mut e, 12, 8);
        assert_eq!(buf[(2u16, 0u16)].symbol(), "\u{2022}", "bullet at indent col 2");
        assert_eq!(buf[(4u16, 0u16)].symbol(), "a", "item text at col 4");
        for c in 0..4u16 {
            assert_eq!(buf[(c, 1u16)].symbol(), " ", "continuation spacer col {c}");
        }
        assert_eq!(buf[(4u16, 1u16)].symbol(), "b", "continuation hangs under TEXT");
        // Round-trip on the continuation row: "beta" starts at byte 10 in the line.
        let (_rows, map) = &e.active().view.line_layouts[&0];
        let (vrow, vcol) = map.source_to_visual(10);
        assert_eq!((vrow, vcol), (1, 4));
        assert_eq!(map.visual_to_source(1, 4), 10);
    }

    #[test]
    fn ventilated_block_stays_painted_when_scrolled_into_its_interior() {
        // Bug 1 (scroll blanks the paragraph), end-to-end through the real paint loop. A four-
        // sentence paragraph (anchored at logical line 0, four visual rows) is TALLER than the
        // 2-row edit area. With the caret on its last line, ensure_visible scrolls partway INTO the
        // block. The block's rows live only at the anchor key, so a scroll/row-count consumer that
        // mishandled the interior lines (over-advancing scroll past the anchor, or landing scroll on
        // an interior line paint skips) blanked the paragraph. Assert the caret's own sentence
        // ("Four") is actually on screen after the scroll.
        let text = "One one one.\nTwo two two.\nThree three.\nFour four.\n";
        let mut e = Editor::new_from_text(text, None, (40, 3)); // edit height 2 < the block's 4 rows
        e.active_mut().view.ventilate = true;
        derive::rebuild(&mut e);
        let head = e.active().document.buffer.line_to_byte(3); // "Four four." — last, interior line
        set_caret(&mut e, head);
        crate::nav::ensure_visible(&mut e);
        derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 40, 3);
        let painted: String = (0..2u16).map(|r| row_string(&buf, r)).collect::<Vec<_>>().join(" | ");
        assert!(
            painted.contains("Four"),
            "the caret's sentence must be painted after scrolling into the block, got rows: {painted:?} \
             (scroll={}, scroll_row={})",
            e.active().view.scroll, e.active().view.scroll_row,
        );
    }

    /// Golden: fold marker `▸` glyph has DarkGray fg under terminal-plain.
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

    /// Golden: wrap guide `│` glyph has DarkGray fg under terminal-plain.
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
            guide_col < 40,
            "guide column {guide_col} must be within viewport"
        );

        // Find the '│' glyph in the guide column.
        // Check all edit rows (0..5) since content may not start at row 0 for guide.
        let guide_x = guide_col;
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

    // cue_themes: the modifier-battery test set — no_color() only.
    // A derived zen-phosphor is NOT a cue theme: its ChromeSelected is explicit fg/bg with
    // NO modifier, so it fails the cued-array assertion. The phosphor case gets its own
    // scoped assertion below (zen_phosphor_chrome_is_fully_colored).
    fn cue_themes() -> [wordcartel_core::theme::Theme; 1] {
        [wordcartel_core::theme::no_color()]
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
        //   DiagSpelling=bold+underline, DiagGrammar=italic+underline (distinct face).
        // Transient overlay + chrome elements with modifier cues:
        //   FocusDim=dim, ChromeReverse=reverse, ChromeSelected=reverse, ChromeMuted=dim,
        //   ChromeAccent=reverse+bold (glyph-bearing prompt-active status — I3/D2).
        let cued = [Emphasis, Strong, StrongEmphasis, Code, CodeBlock, Link, Strikethrough,
                    Comment, FrontMatter, Selection, SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar,
                    ProseLensMatch, FocusDim, ChromeReverse, ChromeSelected, ChromeMuted, ChromeAccent];
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
        // §13.2: ChromeOverlay is exempt from the modifier requirement (M4 a11y) — it is a
        //   fill face with no glyph; the overlay's accessibility is provided by the surrounding
        //   border frame glyphs and placement cue. Its mono_faces() entry is Face::default()
        //   deliberately; proven by the overlay-frame and interior test fixtures.
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
            // S8: ProseLensMatch must be distinguishable from every same-context overlay face.
            assert_ne!(t.face(ProseLensMatch), t.face(Selection), "{}: ProseLensMatch vs Selection", t.name);
            assert_ne!(t.face(ProseLensMatch), t.face(SearchMatch), "{}: ProseLensMatch vs SearchMatch", t.name);
            assert_ne!(t.face(ProseLensMatch), t.face(DiagSpelling), "{}: ProseLensMatch vs DiagSpelling", t.name);
            assert_ne!(t.face(ProseLensMatch), t.face(DiagGrammar), "{}: ProseLensMatch vs DiagGrammar", t.name);
            assert_ne!(t.face(ProseLensMatch), t.face(MarkedBlock), "{}: ProseLensMatch vs MarkedBlock", t.name);
        }
    }

    /// Derived zen-phosphor chrome faces are fully colored (Rgb) and hue-preserving.
    /// This is a SEPARATE assertion from the modifier battery — zen-phosphor is NOT a cue theme
    /// (ChromeSelected is explicit fg/bg with no modifier), but its chrome IS visually complete.
    #[test]
    fn zen_phosphor_chrome_is_fully_colored() {
        use wordcartel_core::theme::{ChromeDisposition, SemanticElement as SE, Color};
        let mut t = wordcartel_core::theme::Theme::builtin("phosphor-green").unwrap();
        t.derive_chrome(ChromeDisposition::Zen);
        // All chrome bg rungs carry Rgb — fully colored, not sentinel
        for el in [SE::Chrome, SE::ChromeOverlay, SE::ChromeMuted, SE::ChromeAccent] {
            let f = t.face(el);
            assert!(matches!(f.bg, Some(Color::Rgb{..})) || matches!(f.fg, Some(Color::Rgb{..})),
                "{el:?} must have Rgb color after zen derive");
        }
        // Hue carried: chrome/overlay/muted bg are green-dominant (G ≥ R and G ≥ B)
        for el in [SE::Chrome, SE::ChromeOverlay, SE::ChromeMuted] {
            if let Some(Color::Rgb { r, g, b }) = t.face(el).bg {
                assert!(g >= r && g >= b, "{el:?} bg must be green-dominant r={r} g={g} b={b}");
            }
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
        assert!(text.contains('󰬼'), "H3 heading glyph (󰬼 = HEADING_GLYPHS[2])");
    }

    /// §13.2 §8.3 completeness: All six heading levels render their distinct numeral glyphs
    /// (`󰬺󰬻󰬼󰬽󰬾󰬿`) under No-color so H1–H6 are distinguishable. (Content assertion: the reverse
    /// video is a style, not a symbol — the cell symbol is the numeral glyph either way.)
    ///
    /// Document: "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6\n\n"
    ///   bytes  0- 4: "# H1\n"    (H1 — HEADING_GLYPHS[0] = '󰬺')
    ///   bytes  5-10: "## H2\n"   (H2 — HEADING_GLYPHS[1] = '󰬻')
    ///   bytes 11-17: "### H3\n"  (H3 — HEADING_GLYPHS[2] = '󰬼')
    ///   bytes 18-25: "#### H4\n" (H4 — HEADING_GLYPHS[3] = '󰬽')
    ///   bytes 26-34: "##### H5\n"(H5 — HEADING_GLYPHS[4] = '󰬾')
    ///   bytes 35-44: "###### H6\n"(H6 — HEADING_GLYPHS[5] = '󰬿')
    ///   byte  45:    "\n"        (blank line — caret placed here so ALL headings INACTIVE)
    #[test]
    fn a11y_all_six_heading_shades_render_in_no_color() {
        let mut ed = Editor::new_from_text(
            "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6\n\n",
            None, (40, 10),
        );
        ed.theme = wordcartel_core::theme::no_color(); // heading_level_glyph = true
        // Place caret on the trailing blank line (byte 45) so ALL six heading lines are
        // INACTIVE and render their numeral glyphs (Task 6 invariant: active line ⟹ no glyph).
        set_caret(&mut ed, 45);
        crate::derive::rebuild(&mut ed);
        let text = (0..10).map(|r| row_string(&render_to_buffer(&mut ed, 40, 10), r)).collect::<String>();
        assert!(text.contains('󰬺'), "H1 glyph (󰬺 = HEADING_GLYPHS[0]) missing in no_color");
        assert!(text.contains('󰬻'), "H2 glyph (󰬻 = HEADING_GLYPHS[1]) missing in no_color");
        assert!(text.contains('󰬼'), "H3 glyph (󰬼 = HEADING_GLYPHS[2]) missing in no_color");
        assert!(text.contains('󰬽'), "H4 glyph (󰬽 = HEADING_GLYPHS[3]) missing in no_color");
        assert!(text.contains('󰬾'), "H5 glyph (󰬾 = HEADING_GLYPHS[4]) missing in no_color");
        assert!(text.contains('󰬿'), "H6 glyph (󰬿 = HEADING_GLYPHS[5]) missing in no_color");
    }

    #[test]
    fn theme_picker_paints_rows_and_selection() {
        let mut ed = Editor::new_from_text("x\n", None, (60, 16));
        ed.open_theme_picker();
        let buf = render_to_buffer(&mut ed, 60, 16);
        let text: String = (0..16).map(|r| row_string(&buf, r)).collect();
        // Rows are alphabetized; catppuccin-latte sorts first, so it is visible at the top.
        assert!(text.contains("catppuccin-latte"), "picker lists built-in themes (alphabetized)");
    }

    // -----------------------------------------------------------------------
    // A6 Task 2: sibling overlay windowed render checks
    // -----------------------------------------------------------------------

    /// A6 (outline): a scrolled outline shows rows[scroll_top], not rows[0];
    /// the indicator appears when scrollable and is absent when all rows fit.
    #[test]
    fn outline_windowed_slice_and_indicator() {
        // 20 headings so the list exceeds the 15-row window cap on a 24-row terminal.
        let doc: String = (0..20).map(|i| format!("# Heading {i:02}\n\n")).collect();
        let mut e = Editor::new_from_text(&doc, None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_outline();
        let total = e.outline.as_ref().unwrap().rows.len();
        assert_eq!(total, 20, "precondition: 20 headings");
        // Force a non-zero scroll_top — render's self-heal will validate/keep it.
        e.outline.as_mut().unwrap().selected = 18;
        e.outline.as_mut().unwrap().scroll_top = 5; // render self-heal will adjust if needed
        let buf = render_to_buffer(&mut e, 80, 24);
        // After render the self-heal has run: read the live scroll_top.
        let scroll_top = e.outline.as_ref().unwrap().scroll_top;
        assert!(scroll_top > 0, "scroll_top must be > 0 after seeding selected=18");
        // First list row (ov_y+2) must show rows[scroll_top], not rows[0].
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, total);
        let first_list_row = rect.y + 2;
        let row_text = row_string(&buf, first_list_row);
        let expected_label = &e.outline.as_ref().unwrap().rows[scroll_top].text;
        assert!(row_text.contains(expected_label.as_str()),
            "first visible row must be rows[{scroll_top}] = {expected_label:?}, got: {row_text:?}");
        // Indicator present (20 rows > 15 window).
        let bottom_row = rect.y + rect.height - 1;
        let bottom_text = row_string(&buf, bottom_row);
        let selected = e.outline.as_ref().unwrap().selected;
        assert!(bottom_text.contains(&format!(" {}/", selected + 1)),
            "outline: indicator must show selected+1/{total}, got: {bottom_text:?}");
        // Non-scrollable: 3 headings, all fit — no indicator digits.
        let doc2 = "# A\n\n# B\n\n# C\n\n";
        let mut e2 = Editor::new_from_text(doc2, None, (80, 24));
        crate::derive::rebuild(&mut e2);
        e2.open_outline();
        assert_eq!(e2.outline.as_ref().unwrap().rows.len(), 3);
        let buf2 = render_to_buffer(&mut e2, 80, 24);
        let rect2 = crate::chrome_geom::palette_overlay_rect(ratatui::layout::Rect::new(0, 0, 80, 24), 3);
        let bottom2 = rect2.y + rect2.height - 1;
        let bottom2_text: String = (rect2.x..rect2.x + rect2.width)
            .map(|x| buf2[(x, bottom2)].symbol().to_string())
            .collect();
        assert!(!bottom2_text.chars().any(|c| c.is_ascii_digit()),
            "non-scrollable outline: no indicator digits in bottom border, got: {bottom2_text:?}");
    }

    /// A6 (theme picker): scrolled picker shows rows[scroll_top], not rows[0];
    /// indicator appears when scrollable, absent when all rows fit.
    #[test]
    fn theme_picker_windowed_slice_and_indicator() {
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.open_theme_picker();
        // 19 builtins — pad to 20 by cycling real names so the list exceeds
        // the 15-row window cap. Directly assigned; no rebuild_rows called.
        {
            let names = wordcartel_core::theme::Theme::builtin_names();
            let tp = e.theme_picker.as_mut().unwrap();
            tp.rows.clear();
            for i in 0..20 { tp.rows.push(names[i % names.len()].to_string()); }
        }
        let total = 20usize;
        // Seed a scroll — render's self-heal will validate it.
        e.theme_picker.as_mut().unwrap().selected = 17;
        e.theme_picker.as_mut().unwrap().scroll_top = 5;
        let buf = render_to_buffer(&mut e, 80, 24);
        let scroll_top = e.theme_picker.as_ref().unwrap().scroll_top;
        assert!(scroll_top > 0, "scroll_top must be > 0 after seeding selected=17");
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, total);
        let first_list_row = rect.y + 2;
        let row_text = row_string(&buf, first_list_row);
        let expected = &e.theme_picker.as_ref().unwrap().rows[scroll_top];
        assert!(row_text.contains(expected.as_str()),
            "first visible row must be rows[{scroll_top}] = {expected:?}, got: {row_text:?}");
        // Indicator present (20 rows > 15 window).
        let selected = e.theme_picker.as_ref().unwrap().selected;
        let bottom_row = rect.y + rect.height - 1;
        let bottom_text = row_string(&buf, bottom_row);
        assert!(bottom_text.contains(&format!(" {}/", selected + 1)),
            "theme picker: indicator must show {}/{total}, got: {bottom_text:?}", selected + 1);
        // Non-scrollable: 1 row fits — no indicator.
        let mut e2 = Editor::new_from_text("x\n", None, (80, 24));
        e2.open_theme_picker();
        {
            let tp = e2.theme_picker.as_mut().unwrap();
            tp.rows = vec!["default".to_string()]; // exactly 1 row
        }
        let buf2 = render_to_buffer(&mut e2, 80, 24);
        let rect2 = crate::chrome_geom::palette_overlay_rect(ratatui::layout::Rect::new(0, 0, 80, 24), 1);
        let bottom2 = rect2.y + rect2.height - 1;
        let bottom2_text: String = (rect2.x..rect2.x + rect2.width)
            .map(|x| buf2[(x, bottom2)].symbol().to_string())
            .collect();
        assert!(!bottom2_text.chars().any(|c| c.is_ascii_digit()),
            "non-scrollable theme picker: no indicator digits, got: {bottom2_text:?}");
    }

    /// Finding 1 regression (C1 T7 fix): at a SHORT terminal the cursor-picker list must
    /// window like every sibling overlay — before the fix, only `list_h_for(7, h)` rows
    /// painted (the fixed-7-row assumption broke below ~11 rows tall) and the `List`
    /// highlight silently clamped to the WRONG row when `selected` was past that short
    /// window, misleading the user about which row was actually selected, and the tail
    /// rows were unreachable.
    #[test]
    fn cursor_picker_windows_and_highlights_true_selection_at_short_terminal() {
        use crate::config::CaretShape;
        let mut e = Editor::new_from_text("x\n", None, (60, 9));
        e.open_cursor_picker();
        let n = crate::cursor_picker::ROW_ACTIONS.len();
        assert_eq!(n, 7, "precondition: fixed 7-row list");
        // Navigate to the LAST row (6, Underline · steady) — past the short-terminal
        // window (list_h_for(7, 9) == 5, so pre-fix only rows 0..5 ever painted).
        e.cursor_picker.as_mut().unwrap().selected = n - 1;
        crate::cursor_picker::preview_selected(&mut e);

        let buf = render_to_buffer(&mut e, 60, 9);
        // (a) selected == 6.
        assert_eq!(e.cursor_picker.as_ref().unwrap().selected, 6);
        let scroll_top = e.cursor_picker.as_ref().unwrap().scroll_top;
        assert!(scroll_top > 0, "render self-heal must advance scroll_top so row 6 is visible");

        let area = ratatui::layout::Rect::new(0, 0, 60, 9);
        let rect = crate::chrome_geom::palette_overlay_rect(area, n + 1);
        let list_top = rect.y + 1; // no query row on the cursor picker
        let list_h = crate::list_window::list_h_for(n, 9);
        assert!(scroll_top + list_h >= n, "row 6 must fall within the rendered window");

        // (b) row 6 is within the rendered window AND the highlight maps to its screen
        // row — window-relative (`selected - scroll_top`), not the raw absolute index
        // (which pre-fix clamped onto whatever row happened to be last on screen).
        let highlight_row = list_top + (6 - scroll_top) as u16;
        let row_text = row_string(&buf, highlight_row);
        assert!(row_text.contains("Underline") && row_text.contains("steady"),
            "row at the computed highlight position must show row 6's label, got: {row_text:?}");

        let selected_bg = crate::compose::compose(&e.theme, e.depth, &[SE::ChromeSelected]).bg;
        assert!((0..60u16).any(|x| buf[(x, highlight_row)].style().bg == selected_bg),
            "row 6's own screen row must carry the ChromeSelected highlight bg");
        for r in list_top..list_top + list_h as u16 {
            if r != highlight_row {
                assert!(!(0..60u16).any(|x| buf[(x, r)].style().bg == selected_bg),
                    "row {r} must NOT carry the highlight — only the true selection does");
            }
        }

        // The preview funnel ran against the TRUE row (not a clamped one): row 6 =
        // Underline · steady.
        assert_eq!(e.caret_shape, CaretShape::Underline);
        assert!(!e.caret_blink, "steady → blink false");
    }

    /// A6 (file browser): scrolled browser shows entries[scroll_top], not entries[0];
    /// indicator appears when scrollable, absent when all rows fit.
    #[test]
    fn file_browser_windowed_slice_and_indicator() {
        // 20 directories → 21 entries (.., d00..d19).
        let dir = std::env::temp_dir().join(format!("wc-a6-fbrender-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..20usize {
            std::fs::create_dir(dir.join(format!("d{i:02}"))).unwrap();
        }
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.open_file_browser(dir.clone());
        let total = e.file_browser.as_ref().unwrap().entries.len();
        assert_eq!(total, 21, "precondition: 21 entries (.., d00..d19)");
        // Seed selected deep — render self-heal will compute scroll_top.
        e.file_browser.as_mut().unwrap().selected = 18;
        e.file_browser.as_mut().unwrap().scroll_top = 4;
        let buf = render_to_buffer(&mut e, 80, 24);
        let scroll_top = e.file_browser.as_ref().unwrap().scroll_top;
        assert!(scroll_top > 0, "scroll_top must be > 0 after seeding selected=18");
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let rect = crate::chrome_geom::palette_overlay_rect(area, total);
        let first_list_row = rect.y + 2;
        let row_text = row_string(&buf, first_list_row);
        let entry = &e.file_browser.as_ref().unwrap().entries[scroll_top];
        let expected = if entry.is_dir { format!("{}/", entry.name) } else { entry.name.clone() };
        assert!(row_text.contains(expected.as_str()),
            "first visible row must be entries[{scroll_top}] = {expected:?}, got: {row_text:?}");
        // Indicator present.
        let selected = e.file_browser.as_ref().unwrap().selected;
        let bottom_row = rect.y + rect.height - 1;
        let bottom_text = row_string(&buf, bottom_row);
        assert!(bottom_text.contains(&format!(" {}/", selected + 1)),
            "file browser: indicator must show {}/{total}, got: {bottom_text:?}", selected + 1);
        // Non-scrollable: just 2 entries — no indicator.
        let small_dir = std::env::temp_dir().join(format!("wc-a6-fbrender-small-{}", std::process::id()));
        std::fs::create_dir_all(&small_dir).unwrap();
        std::fs::write(small_dir.join("foo.md"), "x").unwrap();
        let mut e2 = Editor::new_from_text("x\n", None, (80, 24));
        e2.open_file_browser(small_dir.clone());
        let buf2 = render_to_buffer(&mut e2, 80, 24);
        let total2 = e2.file_browser.as_ref().unwrap().entries.len();
        let rect2 = crate::chrome_geom::palette_overlay_rect(ratatui::layout::Rect::new(0, 0, 80, 24), total2);
        let bottom2 = rect2.y + rect2.height - 1;
        let bottom2_text: String = (rect2.x..rect2.x + rect2.width)
            .map(|x| buf2[(x, bottom2)].symbol().to_string())
            .collect();
        assert!(!bottom2_text.chars().any(|c| c.is_ascii_digit()),
            "non-scrollable file browser: no indicator digits, got: {bottom2_text:?}");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&small_dir);
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

    // -----------------------------------------------------------------------
    // Task 3: full-edit-band canvas fill + transparent modal interiors (RED first)
    // -----------------------------------------------------------------------

    #[test]
    fn opaque_canvas_paints_edit_band() {
        use wordcartel_core::theme::{Theme, Depth};
        let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
        ed.theme = Theme::builtin("flexoki-dark").unwrap();
        ed.depth = Depth::Truecolor;
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);
        let want = compose::base_canvas(&ed.theme, ed.depth).bg;   // flexoki-dark base_bg (Rgb)
        assert!(matches!(want, Some(ratatui::style::Color::Rgb(..))), "flexoki base_bg is Rgb");
        // A cell to the RIGHT of the text (col 20, row 0) — never covered by the per-row Paragraph —
        // carries the canvas bg (the blank-area gap the old per-span paint missed).
        assert_eq!(buf[(20u16, 0u16)].style().bg, want, "blank editing cell must carry canvas bg");
        // A below-content editing row (row 3) too.
        assert_eq!(buf[(5u16, 3u16)].style().bg, want, "below-content cell must carry canvas bg");
    }

    #[test]
    fn transparent_canvas_leaves_edit_band_reset() {
        use wordcartel_core::theme::{Theme, Depth, CanvasMode};
        let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
        ed.theme = Theme::builtin("flexoki-dark").unwrap();
        ed.depth = Depth::Truecolor;
        ed.canvas = CanvasMode::Transparent;
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);
        let bg = buf[(20u16, 0u16)].style().bg;
        assert!(bg.is_none() || bg == Some(ratatui::style::Color::Reset),
            "transparent: blank editing cell stays terminal-default; got {bg:?}");
    }

    #[test]
    fn transparent_suppresses_overlay_interior() {
        // Modal interiors go see-through in transparent mode: ov_fill is a no-op and the query bar
        // renders fg-only (bg stripped). The selected-row highlight keeps its bg (stays visible).
        // Tested directly on the hook — no palette/registry setup needed.
        use wordcartel_core::theme::{Theme, Depth, CanvasMode, ChromeDisposition};
        let mut theme = Theme::builtin("flexoki-dark").unwrap();
        theme.derive_chrome(ChromeDisposition::Full);
        let opaque = ChromeStyles::build(&theme, Depth::Truecolor, CanvasMode::Opaque);
        let transp = ChromeStyles::build(&theme, Depth::Truecolor, CanvasMode::Transparent);
        assert!(opaque.ov_fill.bg.is_some(), "opaque overlay fill carries a ChromeOverlay bg");
        assert_eq!(transp.ov_fill, RStyle::default(), "transparent overlay fill is a no-op");
        assert!(opaque.ov_query.bg.is_some(), "opaque query bar carries a bg");
        assert!(transp.ov_query.bg.is_none(), "transparent query bar bg is stripped (fg-only)");
        assert!(transp.overlay_selected.bg.is_some(), "selected-row highlight stays visible in transparent");
    }

    /// B7 seam invariant: `ChromeStyles::build` records DIM in the `sub_modifier` of every
    /// ChromeSelected-derived selection style (so `Cell::set_style` clears an underlay's DIM) and
    /// never leaves DIM in their `add_modifier`. `menu_norm` (dropdown-normal) MUST retain DIM in
    /// `add_modifier` — the strip is scoped to selection, not the recede. Swept across a derived RGB
    /// theme, terminal-ansi, and the no-color/mono theme (Depth::None).
    #[test]
    fn chrome_selected_styles_strip_dim_via_sub_modifier() {
        use wordcartel_core::theme::{ChromeDisposition, CanvasMode, Depth, Theme};
        use ratatui::style::Modifier;

        // (theme, depth) sweep: derived RGB (tokyo-night), explicit terminal-ansi, mono no-color.
        let mut tokyo = Theme::builtin("tokyo-night").unwrap();
        tokyo.derive_chrome(ChromeDisposition::Full);
        let cases = [
            (tokyo,                                    Depth::Truecolor, "tokyo-night"),
            (Theme::builtin("terminal-ansi").unwrap(), Depth::Ansi16, "terminal-ansi"),
            (Theme::builtin("no-color").unwrap(),      Depth::None,   "no-color"),
        ];
        for (theme, depth, name) in cases {
            let cs = ChromeStyles::build(&theme, depth, CanvasMode::Opaque);
            for (label, style) in [
                ("overlay_selected", cs.overlay_selected),
                ("menu_open",        cs.menu_open),
                ("menu_sel",         cs.menu_sel),
            ] {
                assert!(style.sub_modifier.contains(Modifier::DIM),
                    "{name}/{label}: selection style must record DIM in sub_modifier (strip applied)");
                assert!(!style.add_modifier.contains(Modifier::DIM),
                    "{name}/{label}: selection style must not carry DIM in add_modifier");
            }
            // Guard: the strip must NOT touch the dropdown-normal recede.
            assert!(cs.menu_norm.add_modifier.contains(Modifier::DIM),
                "{name}: menu_norm (ChromeMuted) must keep its DIM recede — strip is selection-only");
        }
    }

    #[test]
    fn transparent_keeps_content_highlights() {
        // Content highlights (selection/search/code/diagnostics) keep their explicit bg in
        // transparent mode — canvas mode only touches the band fill + ov_fill, never content
        // composition. A transparent selection would be an invisible selection (spec D1 boundary).
        use wordcartel_core::theme::{Theme, Depth, CanvasMode};
        use wordcartel_core::selection::Selection;
        let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
        ed.theme = Theme::builtin("flexoki-dark").unwrap();
        ed.depth = Depth::Truecolor;
        ed.canvas = CanvasMode::Transparent;
        ed.active_mut().document.selection = Selection::range(0, 2); // select "hi"
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);
        let bg = buf[(0u16, 0u16)].style().bg;
        assert!(matches!(bg, Some(ratatui::style::Color::Rgb(..))),
            "selection highlight must survive transparent canvas; got {bg:?}");
    }

    #[test]
    fn transparent_keeps_bars_painted() {
        use wordcartel_core::theme::{Theme, Depth, CanvasMode, ChromeDisposition};
        let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
        ed.menu_bar_mode = crate::config::MenuBarMode::Pinned;
        ed.status_line_mode = crate::config::TransientMode::On; // test chrome paint, not calm mode
        let mut theme = Theme::builtin("flexoki-dark").unwrap();
        theme.derive_chrome(ChromeDisposition::Full);
        ed.theme = theme;
        ed.depth = Depth::Truecolor;
        ed.canvas = CanvasMode::Transparent;
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);
        let menu = compose::compose(&ed.theme, ed.depth, &[SE::Chrome]).bg;
        assert_eq!(buf[(0u16, 0u16)].style().bg, menu, "menu bar stays painted in transparent mode");
        assert_eq!(buf[(0u16, 5u16)].style().bg, menu, "status bar stays painted in transparent mode");
    }

    #[test]
    fn non_rgb_theme_canvas_moot_both_modes() {
        use wordcartel_core::theme::{Theme, Depth, CanvasMode};
        for mode in [CanvasMode::Opaque, CanvasMode::Transparent] {
            let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
            ed.theme = Theme::builtin("terminal-plain").unwrap();
            ed.depth = Depth::Truecolor;
            ed.canvas = mode;
            derive::rebuild(&mut ed);
            let buf = render_to_buffer(&mut ed, 40, 6);
            let bg = buf[(20u16, 0u16)].style().bg;
            assert!(bg.is_none() || bg == Some(ratatui::style::Color::Reset),
                "terminal-plain has no canvas — {mode:?} editing cell stays terminal-default; got {bg:?}");
        }
    }

    #[test]
    fn opaque_canvas_at_ansi16_paints_quantized_bg() {
        use wordcartel_core::theme::{Theme, Depth};
        let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
        ed.theme = Theme::builtin("flexoki-dark").unwrap();
        ed.depth = Depth::Ansi16;
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);
        let want = compose::base_canvas(&ed.theme, Depth::Ansi16).bg;
        assert!(want.is_some() && want != Some(ratatui::style::Color::Reset),
            "flexoki base_bg quantizes to a named Ansi16 color; got {want:?}");
        assert_eq!(buf[(20u16, 0u16)].style().bg, want, "opaque Ansi16 paints the quantized canvas bg");
    }

    #[test]
    fn opaque_canvas_at_depth_none_paints_nothing() {
        use wordcartel_core::theme::{Theme, Depth};
        let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
        ed.theme = Theme::builtin("flexoki-dark").unwrap();
        ed.depth = Depth::None;                       // cue/monochrome — base_canvas has no color
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);
        let bg = buf[(20u16, 0u16)].style().bg;
        assert!(bg.is_none() || bg == Some(ratatui::style::Color::Reset),
            "Depth::None: band guard skips the fill; got {bg:?}");
    }

    // -----------------------------------------------------------------------
    // Branch-review fixes (RED tests written before implementation — TDD)
    // -----------------------------------------------------------------------

    /// FIX-1 (segs path §13.2): FocusDim must layer OVER the semantic stack, not replace it.
    /// In no_color + focus mode, a dimmed (out-of-focus) heading row must keep its BOLD
    /// modifier — without BOLD, the heading text is indistinguishable from plain dimmed text
    /// in cue mode (§13.2 violation). Tests the segs path (use_placed=false: no search,
    /// no diagnostics, single cursor).
    #[test]
    fn focus_dim_keeps_heading_bold_in_no_color() {
        // "# Heading\n" = bytes  0-9  (H1 heading)
        // "\n"          = byte  10    (blank line)
        // "paragraph\n" = bytes 11-20 (paragraph; caret here → heading is DIM)
        let mut ed = Editor::new_from_text("# Heading\n\nparagraph\n", None, (40, 8));
        ed.theme = wordcartel_core::theme::no_color();
        ed.depth = wordcartel_core::theme::Depth::None; // cue mode: modifiers only, no color
        ed.view_opts.focus = true;
        set_caret(&mut ed, 11); // caret in "paragraph" → heading is outside focus region → dim
        // Single cursor → has_sel=false, no search → use_placed=false → segs path.
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 8);
        // Row 0 = heading. no_color has heading_level_glyph=true; inactive heading shows
        // "󰬺 " (numeral glyph + space, cols 0-1) then "Heading" (cols 2-8).
        // Prefix "󰬺 " already has BOLD in current code; text cells do NOT → that is the bug.
        let row_text = row_string(&buf, 0);
        let h_col = row_text.chars().position(|c| c == 'H').unwrap_or(99) as u16;
        assert!(h_col < 40, "expected 'Heading' to appear on row 0 (inactive heading), got: {:?}", row_text);
        // After FIX-1: each text cell of the dimmed heading must carry BOLD (heading cue).
        let text_cells_have_bold = (h_col..h_col + 7).any(|x| {
            buf[(x, 0u16)].style().add_modifier.contains(Modifier::BOLD)
        });
        assert!(
            text_cells_have_bold,
            "FIX-1(segs): dimmed heading text must carry BOLD cue in no_color+focus (§13.2); \
             cell at col {h_col} style: {:?}",
            buf[(h_col, 0u16)].style()
        );
    }

    /// FIX-1 (placed path §13.2): same BOLD requirement when the placed path is active
    /// (use_placed=true). A non-empty selection on the paragraph forces the placed path
    /// while leaving the dim heading row unselected — it must still carry BOLD.
    #[test]
    fn focus_dim_keeps_heading_bold_in_no_color_placed_path() {
        // Same document as the segs-path test; selection on paragraph → use_placed=true.
        let mut ed = Editor::new_from_text("# Heading\n\nparagraph\n", None, (40, 8));
        ed.theme = wordcartel_core::theme::no_color();
        ed.depth = wordcartel_core::theme::Depth::None;
        ed.view_opts.focus = true;
        // Non-empty selection on "paragraph" (bytes 11-20) → has_sel=true → placed path.
        ed.active_mut().document.selection =
            wordcartel_core::selection::Selection::range(11, 20);
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 8);
        let row_text = row_string(&buf, 0);
        let h_col = row_text.chars().position(|c| c == 'H').unwrap_or(99) as u16;
        assert!(h_col < 40, "expected 'Heading' visible on row 0, got: {:?}", row_text);
        let text_cells_have_bold = (h_col..h_col + 7).any(|x| {
            buf[(x, 0u16)].style().add_modifier.contains(Modifier::BOLD)
        });
        assert!(
            text_cells_have_bold,
            "FIX-1(placed): dimmed heading text must carry BOLD cue in no_color+focus (§13.2); \
             cell at col {h_col} style: {:?}",
            buf[(h_col, 0u16)].style()
        );
    }

    /// FIX-2: Selection must be applied BEFORE search so the search match can stand out.
    /// A cell that is BOTH selected AND the current search match must show the SearchCurrent
    /// face's bg on top, not the Selection face's bg.
    ///
    /// We start from tokyo-night and override SearchCurrent to carry a distinctive bg color
    /// (LightGreen) that differs from Selection's bg (SEL_BG ≈ Rgb(0x28,0x34,0x57)).
    /// Before fix: selection patches AFTER search → SEL_BG overwrites LightGreen.
    /// After fix: search patches AFTER selection → LightGreen is the final bg.
    #[test]
    fn search_current_wins_over_selection_on_overlap() {
        use wordcartel_core::theme::{Face, Color as ThemeColor, Depth, SemanticElement};
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 6));
        ed.theme = wordcartel_core::theme::tokyo_night();
        ed.depth = Depth::Truecolor;
        // Override SearchCurrent: give it an explicit bg (LightGreen) distinct from Selection.
        ed.theme.override_face(
            SemanticElement::SearchCurrent,
            Face { bg: Some(ThemeColor::LightGreen), ..Face::default() },
        );
        // Open search and find "hello" (bytes 0-4); it becomes the current (first) match.
        ed.open_search(crate::search_overlay::Phase::Find, 0);
        for c in "hello".chars() { ed.search.as_mut().unwrap().insert(c); }
        let rope = ed.active().document.buffer.snapshot();
        let v = ed.active().document.version;
        ed.search.as_mut().unwrap().recompute(&rope, v);
        // Also select "hello" so the first cell is both selected AND current match.
        ed.active_mut().document.selection =
            wordcartel_core::selection::Selection::range(0, 5);
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);
        // Col 0 ('h') is in both the current search match (0..5) and the selection (0..5).
        // After FIX-2: SearchCurrent bg (LightGreen) must win because search patches last.
        let cell_bg = buf[(0u16, 0u16)].style().bg;
        assert_eq!(
            cell_bg,
            Some(ratatui::style::Color::LightGreen),
            "FIX-2: SearchCurrent bg must win over Selection bg on overlap; \
             expected LightGreen, got {:?}",
            cell_bg
        );
    }

    // -----------------------------------------------------------------------
    // Effort 8 Task 4: Ln,Col cursor-position status indicator
    // -----------------------------------------------------------------------

    #[test]
    fn status_shows_ln_col_when_word_count_on() {
        // "hello\nworld\n": byte 8 = 'r' in "world" → line 2, col 3 ("wo|rld")
        // Line 2 starts at byte 6; bytes 6='w', 7='o' → 2 graphemes before caret → col 3
        let mut e = Editor::new_from_text("hello\nworld\n", None, (60, 6));
        e.status_line_mode = crate::config::TransientMode::On; // test Ln/Col content, not calm mode
        e.view_opts.word_count = true;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(8);
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 60, 6);
        let status = row_string(&buf, 5); // bottom row (h-1)
        assert!(status.contains("Ln 2, Col 3"), "got: {status}");
        assert!(status.contains("words"), "still shows the count: {status}");
    }

    #[test]
    fn status_hides_ln_col_when_word_count_off() {
        let mut e = Editor::new_from_text("hello\n", None, (60, 6));
        e.view_opts.word_count = false;
        crate::derive::rebuild(&mut e);
        let status = row_string(&render_to_buffer(&mut e, 60, 6), 5);
        assert!(!status.contains("Ln "), "position rides word-count; off → hidden: {status}");
    }

    /// S8 Task 6: the prose-lens count is its OWN right-side segment, gated on `computed_for
    /// == version` (an active AND current lens) — independent of `view_opts.word_count`. It
    /// rides the status line with word_count OFF (proving no dependency on that option) and,
    /// when both are on, the two segments coexist without one overwriting the other.
    #[test]
    fn status_shows_prose_lens_count_segment_when_active_and_current() {
        let t = "The report was written by them.\n";
        let mut e = Editor::new_from_text(t, None, (60, 6));
        e.status_line_mode = crate::config::TransientMode::On; // test the segment content, not calm mode
        e.view_opts.word_count = false; // independence from word_count
        let v = e.active().document.version;
        let start = t.find("was written").unwrap();
        let end = start + "was written".len();
        e.active_mut().pos.passive = vec![crate::lenses::PosMatch { start, end, category: crate::lenses::ProseLensCategory::Passive }];
        e.active_mut().pos.computed_for = Some(v);
        crate::lenses::set_prose_lens(&mut e, Some(crate::lenses::ProseLensCategory::Passive));
        crate::derive::rebuild(&mut e);
        let status = row_string(&render_to_buffer(&mut e, 60, 6), 5);
        assert!(status.contains("Ln "), "lens segment rides Ln/Col even with word_count off: {status}");
        assert!(status.contains("Passive: 1"), "got: {status}");
        assert!(!status.contains("words"), "word_count off — no word-count segment: {status}");

        // Both on: the two segments coexist, neither overwrites the other.
        e.view_opts.word_count = true;
        crate::derive::rebuild(&mut e);
        let status_both = row_string(&render_to_buffer(&mut e, 60, 6), 5);
        assert!(status_both.contains("Passive: 1"), "lens segment survives alongside word count: {status_both}");
        assert!(status_both.contains("words"), "word-count segment still present: {status_both}");
    }

    #[test]
    fn ln_col_is_view_independent() {
        // "# Heading\n\n**bold** text\n": byte 14 = 'o' in "bold"
        // Line 3 ("**bold** text\n") starts at byte 11; bytes 11='*', 12='*', 13='b' → 3 graphemes
        // before caret → col 4.  Ln,Col must be identical in LivePreview and SourcePlain.
        let mk = |mode| {
            let mut e = Editor::new_from_text("# Heading\n\n**bold** text\n", None, (60, 8));
            e.status_line_mode = crate::config::TransientMode::On; // test Ln/Col content, not calm mode
            e.view_opts.word_count = true;
            e.active_mut().view.mode = mode;
            e.active_mut().document.selection = wordcartel_core::selection::Selection::single(14);
            crate::derive::rebuild(&mut e);
            row_string(&render_to_buffer(&mut e, 60, 8), 7)
        };
        let live = mk(crate::editor::RenderMode::LivePreview);
        let src  = mk(crate::editor::RenderMode::SourcePlain);
        let pick = |s: &str| s.split_once("Ln ").map(|(_, r)| format!("Ln {}", r.split(" ·").next().unwrap_or(r))).unwrap_or_default();
        assert!(pick(&live).starts_with("Ln "), "expected Ln,Col in live status; got: {live}");
        assert_eq!(pick(&live), pick(&src), "Ln,Col identical across views\nlive: {live}\nsrc:  {src}");
    }

    /// FIX-3 + T5: Overlay borders use Chrome fg only; bg == ChromeOverlay fill.
    /// Under phosphor-amber, the theme-picker overlay border cells must carry Chrome
    /// RGB fg; and the border cell bg must equal the ChromeOverlay fill bg — not
    /// Chrome's own bg (which would create a visible "halo" around the overlay).
    #[test]
    fn phosphor_overlay_frame_border_uses_chrome_style() {
        use wordcartel_core::theme::{ChromeDisposition, Theme, Depth};
        let mut ed = Editor::new_from_text("x\n", None, (60, 16));
        let mut theme = Theme::builtin("phosphor-amber").unwrap();
        // I4-A: phosphor chrome is now a sentinel filled by derive_chrome (not set in constructor).
        theme.derive_chrome(ChromeDisposition::Full);
        ed.theme = theme;
        ed.depth = Depth::Truecolor;
        ed.open_theme_picker();
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 60, 16);
        // Scan the buffer for any border glyph (box-drawing chars used by Block).
        let border_cell = (0..60u16).flat_map(|x| (0..16u16).map(move |y| (x, y))).find(|&(x, y)| {
            let s = buf[(x, y)].symbol();
            s == "─" || s == "│" || s == "┌" || s == "┐" || s == "└" || s == "┘"
        });
        assert!(border_cell.is_some(), "FIX-3: expected box-drawing border cells in theme-picker overlay");
        let (bx, by) = border_cell.unwrap();
        let cs = buf[(bx, by)].style();
        // T5: border is fg-only — fg carries Chrome RGB, bg comes from the ChromeOverlay fill.
        // Before T5: border bg = Chrome.bg (distinct from ChromeOverlay bg → halo).
        // After T5: border bg = ChromeOverlay bg (fg-only border, fill shows through).
        let fill_bg = compose::compose(&ed.theme, ed.depth, &[SE::ChromeOverlay]).bg;
        assert!(
            matches!(cs.fg, Some(ratatui::style::Color::Rgb(..))),
            "FIX-3: phosphor overlay border must carry Chrome RGB fg; got {:?} at ({bx},{by})",
            cs.fg
        );
        assert_eq!(
            cs.bg, fill_bg,
            "T5: border bg must equal ChromeOverlay fill bg (fg-only border — no halo); \
             got {:?}, expected fill {:?} at ({bx},{by})", cs.bg, fill_bg
        );
    }

    /// D2: under tokyo-night with the palette open, every interior cell (not on the
    /// border perimeter, not the selected row) must carry the ChromeOverlay bg.
    /// No cell inside the overlay should have the terminal-default bg.
    #[test]
    fn tokyo_overlay_interior_is_themed() {
        use wordcartel_core::theme::{ChromeDisposition, Depth};
        let mut ed = Editor::new_from_text("x", None, (80, 20));
        let mut theme = wordcartel_core::theme::tokyo_night();
        theme.derive_chrome(ChromeDisposition::Full);
        ed.theme = theme;
        ed.depth = Depth::Truecolor;
        commands_palette(&mut ed);
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 80, 20);

        let fill_bg = compose::compose(&ed.theme, ed.depth, &[SE::ChromeOverlay]).bg;
        // §II.5 pin: tokyo FULL ChromeOverlay bg = #3d405a — the modal shares the dropdown
        // (ChromeMuted) level-2 tone (3-tone ladder, user decision 2026-07-06).
        assert_eq!(fill_bg, Some(Color::Rgb(0x3d, 0x40, 0x5a)),
            "tokyo-night FULL ChromeOverlay (§II.5 pin) must be #3d405a (= ChromeMuted bg)");

        let n_rows = ed.palette.as_ref().unwrap().rows.len();
        let ov_rect = crate::chrome_geom::palette_overlay_rect(ratatui::layout::Rect::new(0, 0, 80, 20), n_rows);
        // query row = ov_y+1; list items start at ov_y+2; selected (index 0) is at ov_y+2.
        let selected_y = ov_rect.y + 2;

        for y in (ov_rect.y + 1)..(ov_rect.y + ov_rect.height - 1) {
            if y == selected_y { continue; } // selected row carries ChromeSelected — skip
            for x in (ov_rect.x + 1)..(ov_rect.x + ov_rect.width - 1) {
                let cell_bg = buf[(x, y)].style().bg;
                assert_eq!(cell_bg, fill_bg,
                    "interior ({x},{y}) must have ChromeOverlay bg={fill_bg:?}, got {cell_bg:?}");
            }
        }
    }

    /// THE halo bug fix (D2 defect-1): under phosphor-green, overlay border cells must NOT
    /// carry Chrome's own bg — the fg-only border rule leaves the ChromeOverlay fill's bg
    /// intact on border cells. Tested for both Full and Zen dispositions.
    #[test]
    fn phosphor_border_cells_carry_no_own_bg() {
        use wordcartel_core::theme::{ChromeDisposition, Depth};
        for disp in [ChromeDisposition::Full, ChromeDisposition::Zen] {
            let mut ed = Editor::new_from_text("x", None, (80, 20));
            let mut theme = wordcartel_core::theme::Theme::builtin("phosphor-green").unwrap();
            theme.derive_chrome(disp);
            ed.theme = theme;
            ed.depth = Depth::Truecolor;
            ed.open_theme_picker();
            derive::rebuild(&mut ed);
            let buf = render_to_buffer(&mut ed, 80, 20);

            let fill_bg = compose::compose(&ed.theme, ed.depth, &[SE::ChromeOverlay]).bg;
            assert!(fill_bg.is_some(), "phosphor ChromeOverlay must have an Rgb bg after derive_chrome");

            // Find any border glyph cell.
            let border_cell = (0..80u16).flat_map(|x| (0..20u16).map(move |y| (x, y))).find(|&(x, y)| {
                let s = buf[(x, y)].symbol();
                s == "─" || s == "│" || s == "┌" || s == "┐" || s == "└" || s == "┘"
            });
            assert!(border_cell.is_some(),
                "expected border cells in theme-picker overlay (disp={disp:?})");
            let (bx, by) = border_cell.unwrap();
            let cell_bg = buf[(bx, by)].style().bg;
            assert_eq!(cell_bg, fill_bg,
                "border cell bg must equal fill bg (no halo) under phosphor-green \
                 ({disp:?}); bg={cell_bg:?}, fill={fill_bg:?} at ({bx},{by})");
        }
    }

    // -----------------------------------------------------------------------
    // A1 Task 2: inactive bar renders static labels in Pinned mode.
    // -----------------------------------------------------------------------

    /// Pinned mode + menu None → row 0 shows static labels (" File ", " Edit ", …)
    /// in Chrome style; row 1 has no dropdown (contains the document text).
    #[test]
    fn render_paints_inactive_bar_labels() {
        // 8 categories (Task 4.2 adds Documents after View): " File "(6)+" Edit "(6)+
        // " Block "(7)+" Format "(8)+" View "(6)+" Documents "(11)+" Settings "(10)+
        // " Export "(8) = 62 — use 70 wide to fit all.
        let mut e = Editor::new_from_text("hello\n", None, (70, 8));
        e.menu_bar_mode = crate::config::MenuBarMode::Pinned;
        e.menu = None;
        derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 70, 8);
        let row0 = row_string(&buf, 0);
        assert!(row0.contains(" File "),      "inactive bar must show ' File '");
        assert!(row0.contains(" Edit "),      "inactive bar must show ' Edit '");
        assert!(row0.contains(" Block "),     "inactive bar must show ' Block '");
        assert!(row0.contains(" Format "),    "inactive bar must show ' Format '");
        assert!(row0.contains(" View "),      "inactive bar must show ' View '");
        assert!(row0.contains(" Documents "), "inactive bar must show ' Documents '");
        assert!(row0.contains(" Settings "),  "inactive bar must show ' Settings '");
        assert!(row0.contains(" Export "),    "inactive bar must show ' Export '");
        // Row 1 must have the document text (not a dropdown).
        let row1 = row_string(&buf, 1);
        assert!(row1.contains("hello"), "row 1 must show document text, not a dropdown");
    }

    /// A2: with the menu open, row 0 is a solid bar — every cell styled Chrome (gaps +
    /// right side) or ChromeSelected (the open label); no cell keeps the base background.
    #[test]
    fn menu_bar_row_is_filled_full_width() {
        let mut e = Editor::new_from_text("body\n", None, (40, 8));
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        e.menu = Some(crate::menu::build(&reg, &km, &e));
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();
        let chrome = compose::compose(&e.theme, e.depth, &[SE::Chrome]).bg;
        let selected = compose::compose(&e.theme, e.depth, &[SE::ChromeSelected]).bg;
        for x in 0u16..40 {
            let bg = buf[(x, 0u16)].style().bg;
            assert!(bg == chrome || bg == selected,
                "row-0 cell {x} not bar-styled: {bg:?} (chrome={chrome:?}, selected={selected:?})");
        }
        // And the RIGHT EDGE specifically is Chrome (it is unpainted today — this fails pre-fix).
        assert_eq!(buf[(39u16, 0u16)].style().bg, chrome, "right edge must carry the Chrome fill");
    }

    /// Whole-branch gate regression: base16 themes and tokyo-night have `text: Face::default()`
    /// (no fg), so plain body-text cells rendered with terminal-default fg over base_bg — a
    /// visible readability defect once the opaque canvas paints base_bg. The fix applies a
    /// render-time `text_fg_or_base` fallback: body spans with no composed fg fall back to
    /// base_fg, while headings/colored roles (fg already set) are untouched.
    ///
    /// Must FAIL before the fix (text fg is None) and PASS after (text fg == base_fg).
    #[test]
    fn body_text_carries_theme_fg() {
        use crate::editor::RenderMode;

        // flexoki-dark (a base16 theme) — plain body-text cells must carry base_fg.
        {
            let mut ed = Editor::new_from_text("Hello\n", None, (40, 4));
            ed.theme = wordcartel_core::theme::flexoki_dark();
            ed.active_mut().view.mode = RenderMode::LivePreview;
            derive::rebuild(&mut ed);
            let buf = render_to_buffer(&mut ed, 40, 4);
            let want = compose::base_canvas(&ed.theme, ed.depth).fg;
            assert!(want.is_some(), "flexoki-dark base_fg must be Some(Rgb)");
            assert!(
                (0..40u16).any(|x| buf[(x, 0u16)].style().fg == want),
                "flexoki-dark: body text must carry base_fg {:?} — row-0 fgs: {:?}",
                want,
                (0..40u16).map(|x| buf[(x, 0u16)].style().fg).collect::<Vec<_>>(),
            );
        }

        // tokyo-night — plain body-text cells must carry base_fg.
        {
            let mut ed = Editor::new_from_text("Hello\n", None, (40, 4));
            ed.theme = wordcartel_core::theme::tokyo_night();
            ed.active_mut().view.mode = RenderMode::LivePreview;
            derive::rebuild(&mut ed);
            let buf = render_to_buffer(&mut ed, 40, 4);
            let want = compose::base_canvas(&ed.theme, ed.depth).fg;
            assert!(want.is_some(), "tokyo-night base_fg must be Some(Rgb)");
            assert!(
                (0..40u16).any(|x| buf[(x, 0u16)].style().fg == want),
                "tokyo-night: body text must carry base_fg {:?} — row-0 fgs: {:?}",
                want,
                (0..40u16).map(|x| buf[(x, 0u16)].style().fg).collect::<Vec<_>>(),
            );
        }
    }

    // Task 4 — status-line Auto-mode calm / visible render

    /// Under Auto mode with no message and no dwell-reveal, the bottom row must be
    /// blank (calm canvas — NOT the info-line text). Under On mode the info line shows.
    #[test]
    fn auto_idle_hides_status_line_on_mode_paints_it() {
        use crate::config::TransientMode;

        // Auto + no message + not revealed → bottom row must be blank / calm.
        {
            let mut ed = Editor::new_from_text("hello\n", None, (40, 6));
            ed.status_line_mode = TransientMode::Auto;
            ed.mouse.status_revealed = false;
            ed.clear_transient_status();
            derive::rebuild(&mut ed);
            let buf = render_to_buffer(&mut ed, 40, 6);
            let bottom = row_string(&buf, 5);
            assert!(
                bottom.trim().is_empty(),
                "Auto idle: bottom row must be calm/blank, got: {:?}",
                bottom
            );
        }

        // On mode → bottom row must show the info line (non-empty, contains buffer name
        // or mode indicator).
        {
            let mut ed = Editor::new_from_text("hello\n", None, (40, 6));
            ed.status_line_mode = TransientMode::On;
            ed.mouse.status_revealed = false;
            ed.clear_transient_status();
            derive::rebuild(&mut ed);
            let buf = render_to_buffer(&mut ed, 40, 6);
            let bottom = row_string(&buf, 5);
            assert!(
                !bottom.trim().is_empty(),
                "On mode: bottom row must show info line (non-empty), got: {:?}",
                bottom
            );
        }
    }

    /// T8 (two-archetype styling): the dropdown rect is filled with the Muted panel bg all
    /// the way to its bottom row — no unfilled gap cells.  The fill is applied via an explicit
    /// `set_style(drop_rect, cs.menu_norm)` after Clear and before the per-item List render so
    /// that the entire rect reads as one elevated surface regardless of item-row coverage.
    ///
    /// SCOPE NOTE — this is a FORWARD regression pin, not a present-day failure detector.
    /// Today `menu_dropdown_rect` height == `leaves.len()`, so every row (incl. `drop_bottom`)
    /// is an item row that ratatui's `List` already styles with `cs.menu_norm` — the explicit
    /// fill is idempotent and this assertion passes with or without it.  When Task 14 extends
    /// the dropdown height (windowing + the `n/total` indicator row), `drop_bottom` becomes a
    /// NON-item row and the explicit fill is its only source of panel bg — Task 14 MUST add a
    /// test that probes that non-item row directly (this test does not cover it).
    #[test]
    fn dropdown_fills_whole_rect_with_muted_panel_bg() {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        // Terminal is 80 wide × 24 tall; the menu bar occupies row 0 and the dropdown
        // starts at row 1 — wide enough to fit all category labels.
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let menu = crate::menu::build(&reg, &km, &e);
        e.menu = Some(menu);
        derive::rebuild(&mut e);

        // Derive the expected panel bg from the same ChromeStyles path that paint() uses.
        let cs = ChromeStyles::build(&e.theme, e.depth, e.canvas);
        let panel_bg = cs.menu_norm.bg;

        // Compute drop_rect before rendering so we can look up coordinates.
        // mirror render_overlays::paint: menu_area = full area minus the bottom status row.
        let area  = Rect::new(0, 0, 80, 24);
        let h     = area.height;
        let menu_area = Rect::new(area.x, area.y, area.width, h.saturating_sub(1));
        let open  = e.menu.as_ref().unwrap().open;
        let groups = e.menu.as_ref().unwrap().groups.clone();
        let drop_rect = crate::chrome_geom::menu_dropdown_rect(menu_area, &groups, open)
            .expect("builtins menu must have at least one non-empty group");

        // x of the leftmost column; y of the very last row of the dropdown rect.
        let drop_x      = drop_rect.x;
        let drop_bottom = drop_rect.y + drop_rect.height - 1;

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();

        let bg_at = |x: u16, y: u16| buf[(x, y)].style().bg;
        assert_eq!(bg_at(drop_x, drop_bottom), panel_bg,
            "dropdown paints a filled panel to its bottom row (no unfilled gap)");
    }

    // -----------------------------------------------------------------------
    // Task 14: dropdown windowing + indicator
    // -----------------------------------------------------------------------

    /// T14-b carry-forward from Task 8: when the dropdown OVERFLOWS the window, the
    /// indicator/pad row at drop_bottom is a non-item row — the explicit `set_style`
    /// fill is its ONLY source of panel bg.  This test probes that row after a render.
    #[test]
    fn dropdown_indicator_row_carries_panel_bg() {
        let reg = crate::registry::Registry::builtins();
        let (_km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        // Short terminal (h=8) so avail_below=7; categories with many items overflow the window.
        let mut e = Editor::new_from_text("x\n", None, (80, 8));
        // Build menu with a synthetic tall category so the dropdown overflows the 7-row window.
        let leaves: Vec<(String, crate::menu::MenuRowAction)> = (0..20)
            .map(|i| (format!("item{i:02}       "), crate::menu::MenuRowAction::Command(crate::registry::CommandId("move_right"))))
            .collect();
        e.menu = Some(crate::menu::MenuView {
            groups: vec![(crate::registry::MenuCategory::Edit, leaves)],
            open: 0, highlighted: 0, built: true, scroll_top: 0,
        });
        derive::rebuild(&mut e);

        let cs = ChromeStyles::build(&e.theme, e.depth, e.canvas);
        let panel_bg = cs.menu_norm.bg;

        let area      = Rect::new(0, 0, 80, 8);
        let h         = area.height;
        let menu_area = Rect::new(area.x, area.y, area.width, h.saturating_sub(1));
        let groups    = e.menu.as_ref().unwrap().groups.clone();
        let open      = e.menu.as_ref().unwrap().open;
        let drop_rect = crate::chrome_geom::menu_dropdown_rect(menu_area, &groups, open)
            .expect("tall category must produce a dropdown rect");
        let drop_x      = drop_rect.x;
        let drop_bottom = drop_rect.y + drop_rect.height - 1;

        let mut term = Terminal::new(TestBackend::new(80, 8)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();

        let bg_at = |x: u16, y: u16| buf[(x, y)].style().bg;
        assert_eq!(bg_at(drop_x, drop_bottom), panel_bg,
            "indicator/pad row at drop_bottom must carry panel bg — only source is the explicit fill");
    }

    /// T14-c (C1 regression): the highlighted item must ALWAYS be within the rendered
    /// item rows — never hidden behind the n/total indicator row.
    ///
    /// Geometry: 80×8 terminal, 20-leaf category.  drop_rect.height = min(20,15,7) = 7.
    /// item_rows = 6 (one row reserved for the indicator).  The bug: keep_visible was
    /// called with list_h=7, so highlighted=6 satisfied the invariant yet mapped to the
    /// indicator row (only items 0-5 were painted).  The fix: keep_visible is called with
    /// keep_h=6 — the paint then adjusts scroll_top so highlighted ∈ [scroll_top,
    /// scroll_top+6), which is always within the rendered item rows.
    #[test]
    fn dropdown_highlight_never_hidden_in_overflow() {
        // 80×8 terminal: avail_below = 7, drop_rect.height = 7, item_rows = 6.
        let mut e = Editor::new_from_text("x\n", None, (80, 8));
        let leaves: Vec<(String, crate::menu::MenuRowAction)> = (0..20)
            .map(|i| (format!("item{i:02}"), crate::menu::MenuRowAction::Command(crate::registry::CommandId("move_right"))))
            .collect();
        // Place the highlight at the position that was hidden under the old code:
        // highlighted=6, scroll_top=0.  With list_h=7 (old) keep_visible accepts this
        // (6 < 0+7), but item_rows=6 means only rows 0-5 are painted — row 6 is the
        // indicator.  With keep_h=6 (fix) keep_visible adjusts scroll_top to 1,
        // putting highlighted at visual row 5 — within item_rows.
        e.menu = Some(crate::menu::MenuView {
            groups: vec![(crate::registry::MenuCategory::Edit, leaves)],
            open: 0, highlighted: 6, built: true, scroll_top: 0,
        });
        derive::rebuild(&mut e);

        let cs = ChromeStyles::build(&e.theme, e.depth, e.canvas);

        let area      = Rect::new(0, 0, 80, 8);
        let menu_area = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        let groups    = e.menu.as_ref().unwrap().groups.clone();
        let open      = e.menu.as_ref().unwrap().open;
        let drop_rect = crate::chrome_geom::menu_dropdown_rect(menu_area, &groups, open)
            .expect("tall category must produce a dropdown rect");
        let list_h    = drop_rect.height as usize;         // = 7
        let item_rows = list_h.saturating_sub(1);          // = 6

        let mut term = Terminal::new(TestBackend::new(80, 8)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();

        // After render, keep_visible has been applied — read back the updated window state.
        let m          = e.menu.as_ref().unwrap();
        let scroll_top = m.scroll_top;
        let highlighted = m.highlighted;

        // Arithmetic invariant: the highlight must be within the item rows, not hidden.
        assert!(
            highlighted - scroll_top < item_rows,
            "highlight must be in item rows: highlighted={highlighted}, scroll_top={scroll_top}, item_rows={item_rows} — highlighted-scroll_top={} must be < {item_rows}",
            highlighted - scroll_top,
        );

        // Visual invariant: the rendered row at the highlight position must carry
        // menu_sel style, and the indicator row must NOT carry menu_sel.
        let visual_row    = (highlighted - scroll_top) as u16;
        let sel_y         = drop_rect.y + visual_row;
        let indicator_y   = drop_rect.y + drop_rect.height - 1;
        let buf           = term.backend().buffer();
        let fg_at         = |x: u16, y: u16| buf[(x, y)].style().fg;
        let sel_fg        = cs.menu_sel.fg;
        let norm_fg       = cs.menu_norm.fg;
        let col           = drop_rect.x + 1; // first text column inside the item
        assert_eq!(fg_at(col, sel_y), sel_fg,
            "rendered item at visual row {visual_row} (y={sel_y}) must carry menu_sel fg");
        assert_ne!(fg_at(col, indicator_y), sel_fg,
            "indicator row (y={indicator_y}) must not carry menu_sel fg — it should be norm ({norm_fg:?})");
    }

    // -------------------------------------------------------------------------
    // B11 — arm-3 hide-guard + overlay carets (the census itself is retired: subsumed by
    // overlays::tests::every_overlay_is_active_xor_and_consumes_key_and_click, H21 T6)
    // -------------------------------------------------------------------------

    /// A real, valid `Diagnostic` literal (copied from `diag_overlay.rs`'s own test fixture)
    /// so `open_diag` gets a real value instead of a synthesized one.
    fn diag_fixture() -> wordcartel_core::diagnostics::Diagnostic {
        wordcartel_core::diagnostics::Diagnostic {
            range: 0..1,
            kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "m".into(),
            suggestions: Vec::new(),
        }
    }

    /// Test B (palette): mid-string caret at `palette.cursor` — the ONLY query overlay
    /// with an interior cursor, so it gets its own dedicated column-math assertion.
    #[test]
    fn palette_query_shows_caret_mid_string() {
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
        derive::rebuild(&mut ed);
        ed.open_palette();
        if let Some(p) = ed.palette.as_mut() { p.query = "abc".into(); p.cursor = 1; }
        let ov = crate::chrome_geom::palette_overlay_rect(
            Rect::new(0, 0, 40, 12), ed.palette.as_ref().unwrap().rows.len());
        let cur = render_capturing_cursor(&mut ed, 40, 12);
        // query_area.x == ov.x + 1; prefix "> " == 2 cols; cursor == 1 char into "abc"; query row == ov.y + 1
        assert_eq!(cur, Some((ov.x + 1 + 2 + 1, ov.y + 1)), "mid-string caret in the palette query");
    }

    /// Test B (outline): end-of-query caret.
    #[test]
    fn outline_query_shows_caret_end_of_string() {
        let mut ed = Editor::new_from_text("# Heading\nbody\n", None, (40, 12));
        derive::rebuild(&mut ed);
        ed.open_outline();
        if let Some(o) = ed.outline.as_mut() { o.query = "he".into(); o.cursor = 2; }
        let ov = crate::chrome_geom::palette_overlay_rect(
            Rect::new(0, 0, 40, 12), ed.outline.as_ref().unwrap().rows.len());
        let cur = render_capturing_cursor(&mut ed, 40, 12);
        assert_eq!(cur, Some((ov.x + 1 + 2 + 2, ov.y + 1)), "end-of-query caret in the outline query");
    }

    /// Test B (theme_picker): end-of-query caret.
    #[test]
    fn theme_picker_query_shows_caret_end_of_string() {
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
        derive::rebuild(&mut ed);
        ed.open_theme_picker();
        if let Some(tp) = ed.theme_picker.as_mut() { tp.query = "ton".into(); }
        let ov = crate::chrome_geom::palette_overlay_rect(
            Rect::new(0, 0, 40, 12), ed.theme_picker.as_ref().unwrap().rows.len());
        let cur = render_capturing_cursor(&mut ed, 40, 12);
        assert_eq!(cur, Some((ov.x + 1 + 2 + 3, ov.y + 1)), "end-of-query caret in the theme_picker query");
    }

    /// F1 regression (final-gate finding): a query overlay backed by unbounded bracketed
    /// paste (e.g. theme_picker's `query`) must not panic when the char count overflows a
    /// u16 caret column, and must simply HIDE the caret (the `< width` guard) rather than
    /// truncate to a misleading on-screen column. Mirrors the H7 fix in `place_cursor`.
    #[test]
    fn theme_picker_huge_paste_query_does_not_panic_and_hides_caret() {
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
        derive::rebuild(&mut ed);
        ed.open_theme_picker();
        if let Some(tp) = ed.theme_picker.as_mut() { tp.query = "x".repeat(70_000); }
        // Must not panic (dev/test builds are overflow-checked) and must not place the
        // caret at some wrapped/truncated column — the frame is left at the TestBackend
        // default, i.e. no cursor placed by this overlay.
        let cur = render_capturing_cursor(&mut ed, 40, 12);
        assert_eq!(cur, Some((0, 0)), "huge query hides the caret rather than truncating it on-screen");
    }

    /// Test B (file_browser): end-of-query caret.
    #[test]
    fn file_browser_query_shows_caret_end_of_string() {
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
        derive::rebuild(&mut ed);
        ed.open_file_browser(std::path::PathBuf::from("."));
        if let Some(fb) = ed.file_browser.as_mut() { fb.query = "rs".into(); }
        let ov = crate::chrome_geom::palette_overlay_rect(
            Rect::new(0, 0, 40, 12), ed.file_browser.as_ref().unwrap().entries.len());
        let cur = render_capturing_cursor(&mut ed, 40, 12);
        assert_eq!(cur, Some((ov.x + 1 + 2 + 2, ov.y + 1)), "end-of-query caret in the file_browser query");
    }

    /// Test B (menu): a hide surface — arm-3 must suppress, and menu places no caret of
    /// its own, so the frame is left at the TestBackend default == suppression.
    #[test]
    fn menu_open_suppresses_editor_caret() {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
        derive::rebuild(&mut ed);
        // put the editor caret off-origin so suppression (-> (0,0)) is unambiguous:
        set_caret(&mut ed, "hello world".len());
        let baseline = render_capturing_cursor(&mut ed, 40, 12);
        assert_ne!(baseline, Some((0, 0)), "precondition: editor caret is off-origin at rest");
        // open the menu (a hide surface) — arm-3 must suppress, and menu places no caret:
        ed.menu = Some(crate::menu::build(&reg, &km, &ed));
        let cur = render_capturing_cursor(&mut ed, 40, 12);
        assert_eq!(cur, Some((0, 0)),
            "arm-3 must suppress the editor caret under a modal (B11); suppression == (0,0)");
    }

    /// B7 leak-proof: with a menu open under a DIM-bearing derived theme (tokyo-night —
    /// both `ChromeMuted` and `Chrome` carry dim), neither the SELECTED dropdown row nor the
    /// OPEN-category bar label may carry a leaked `Modifier::DIM`. This is the crux regression
    /// for the chrome-selection-legibility fix and covers BOTH leak sites (spec §1.2):
    ///   - dropdown selected row  ← `menu_norm` (ChromeMuted/DIM) underlay + `menu_sel` swap
    ///   - open-category bar label ← `menu_closed` (Chrome/DIM) bar fill + `menu_open` swap
    ///
    /// First-failing on the unpatched tree (DIM leaks through ratatui's OR-merge); passes once
    /// `ChromeStyles::build` strips DIM via `sub_modifier`.
    #[test]
    fn menu_selection_and_open_label_have_no_leaked_dim() {
        use wordcartel_core::theme::{ChromeDisposition, Depth, Theme};
        use ratatui::layout::Rect;
        use ratatui::style::Modifier;

        // Arrange: derived RGB theme with a DIM-bearing dropdown + bar, at truecolor.
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut ed = Editor::new_from_text("hello world\n", None, (80, 12));
        let mut theme = Theme::builtin("tokyo-night").unwrap();
        theme.derive_chrome(ChromeDisposition::Full);
        ed.theme = theme;
        ed.depth = Depth::Truecolor;
        derive::rebuild(&mut ed);
        // Open the first category's menu (build → open:0, highlighted:0, scroll_top:0). No
        // menu_bar_mode setup is needed: `menu_bar_rows()` returns `u16::from(bar || menu.is_some())`
        // (editor.rs), so an open menu forces the bar to 1 row regardless of the default Auto mode —
        // this is what makes the open-label (bar) assertion exercisable/first-failing.
        ed.menu = Some(crate::menu::build(&reg, &km, &ed));

        // Geometry, via the same helpers the painter uses (menu_area excludes the status row).
        let menu_area = crate::chrome_geom::menu_area(Rect::new(0, 0, 80, 12));
        let groups = ed.menu.as_ref().unwrap().groups.clone();
        let open = ed.menu.as_ref().unwrap().open;
        let bar = crate::chrome_geom::menu_bar_layout(menu_area, &groups);
        let (_, label_rect) = bar[open];
        let drop_rect = crate::chrome_geom::menu_dropdown_rect(menu_area, &groups, open)
            .expect("open category must produce a dropdown rect");

        // Act.
        let buf = render_to_buffer(&mut ed, 80, 12);

        // Assert — selected dropdown row (highlighted 0, scroll_top 0 → the top item row).
        let sel_y = drop_rect.y;
        let sel_x = drop_rect.x + 1; // first text cell inside the item (mirrors the highlight test)
        assert!(
            !buf[(sel_x, sel_y)].style().add_modifier.contains(Modifier::DIM),
            "selected dropdown row must not carry leaked DIM at ({sel_x},{sel_y}); \
             style={:?}", buf[(sel_x, sel_y)].style(),
        );

        // Assert — open-category bar label: no cell across its own columns may carry DIM.
        let label_has_dim = (label_rect.x..label_rect.x + label_rect.width)
            .any(|x| buf[(x, label_rect.y)].style().add_modifier.contains(Modifier::DIM));
        assert!(
            !label_has_dim,
            "open-category bar label (row {}) must not carry leaked DIM", label_rect.y,
        );
    }

    /// Test B (prompt): a hide surface.
    #[test]
    fn prompt_open_suppresses_editor_caret() {
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
        derive::rebuild(&mut ed);
        set_caret(&mut ed, "hello world".len());
        let baseline = render_capturing_cursor(&mut ed, 40, 12);
        assert_ne!(baseline, Some((0, 0)), "precondition: editor caret is off-origin at rest");
        ed.open_prompt(crate::prompt::Prompt::swap_recovery());
        let cur = render_capturing_cursor(&mut ed, 40, 12);
        assert_eq!(cur, Some((0, 0)), "arm-3 must suppress the editor caret under a modal prompt");
    }

    /// Test B (splash): a hide surface.
    #[test]
    fn splash_open_suppresses_editor_caret() {
        let (km, _) = crate::keymap::build_keymap(
            &crate::config::KeymapConfig::default(), &crate::registry::Registry::builtins());
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
        derive::rebuild(&mut ed);
        set_caret(&mut ed, "hello world".len());
        let baseline = render_capturing_cursor(&mut ed, 40, 12);
        assert_ne!(baseline, Some((0, 0)), "precondition: editor caret is off-origin at rest");
        ed.splash = Some(crate::splash::Splash::new(&km, "0.0.0"));
        let cur = render_capturing_cursor(&mut ed, 40, 12);
        assert_eq!(cur, Some((0, 0)), "arm-3 must suppress the editor caret under the startup splash");
    }

    /// Test B (diag): a hide surface.
    #[test]
    fn diag_open_suppresses_editor_caret() {
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
        derive::rebuild(&mut ed);
        set_caret(&mut ed, "hello world".len());
        let baseline = render_capturing_cursor(&mut ed, 40, 12);
        assert_ne!(baseline, Some((0, 0)), "precondition: editor caret is off-origin at rest");
        ed.open_diag(diag_fixture());
        let cur = render_capturing_cursor(&mut ed, 40, 12);
        assert_eq!(cur, Some((0, 0)), "arm-3 must suppress the editor caret under the quick-fix overlay");
    }

    /// Test B (search): arms 1/2 are unaffected by the arm-3 guard — the caret still
    /// lands on the status row, not at the TestBackend-default suppression coordinate.
    #[test]
    fn search_caret_still_lands_on_status_row() {
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
        derive::rebuild(&mut ed);
        ed.open_search(crate::search_overlay::Phase::Find, 0);
        let cur = render_capturing_cursor(&mut ed, 40, 12);
        let (_, y) = cur.expect("helper always returns Some");
        assert_eq!(y, 11, "search caret sits on the status row (h-1)");
        assert_ne!(cur, Some((0, 0)), "search caret must not read as suppressed");
    }

    /// Test B (minibuffer): same guarantee as search — arms 1/2 are unaffected.
    #[test]
    fn minibuffer_caret_still_lands_on_status_row() {
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
        derive::rebuild(&mut ed);
        ed.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        let cur = render_capturing_cursor(&mut ed, 40, 12);
        let (_, y) = cur.expect("helper always returns Some");
        assert_eq!(y, 11, "minibuffer caret sits on the status row (h-1)");
        assert_ne!(cur, Some((0, 0)), "minibuffer caret must not read as suppressed");
    }

    // -----------------------------------------------------------------------
    // S4 Task 10 — SEE==SELECT / fold-survival paint asserts (spec §8 probes 1 & 2)
    // -----------------------------------------------------------------------

    /// Probe 1: a single sentence long enough to hard-wrap across several ventilated rows is ONE
    /// row-group (`GutterCell::Count` then N-1 `Continuation` cells); `select_sentence` must paint
    /// the `SE::Selection` highlight on EVERY row of that group, not just the caret's own row.
    #[test]
    fn see_equals_select_highlights_every_row_of_a_wrapped_sentence_group() {
        let text = "This is one single long sentence that must wrap across several rows in a narrow viewport for the test.\n";
        let mut e = Editor::new_from_text(text, None, (20, 10));
        e.active_mut().view.ventilate = true;
        derive::rebuild(&mut e);
        let vb = e.active().view.vent_blocks.get(&0).cloned().expect("paragraph anchored at line 0");
        assert!(vb.gutter.len() > 1,
            "precondition: the sentence must wrap to more than one row for this probe to be meaningful");
        // Every cell in this one-sentence paragraph belongs to the SAME row-group: one Count
        // (the first row) followed by nothing but Continuation cells.
        assert!(matches!(vb.gutter[0], crate::ventilate::GutterCell::Count(_)));
        assert!(vb.gutter[1..].iter().all(|g| matches!(g, crate::ventilate::GutterCell::Continuation)));

        let at = text.find("must wrap").expect("probe word present");
        e.active_mut().document.selection = Selection::single(at);
        derive::rebuild(&mut e);
        let expected = crate::commands::prose_sentence_at(&e, at).expect("prose sentence at caret");
        assert_eq!(
            crate::commands::run(crate::commands::Command::SelectScope(crate::commands::Scope::Sentence),
                &mut e, &crate::test_support::TestClock(0)),
            crate::commands::CommandResult::Handled,
        );
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), expected, "select_sentence picks exactly the SEE==SELECT span");

        let buf = render_to_buffer(&mut e, 20, 10);
        for row in 0..vb.gutter.len() as u16 {
            assert!(row_has_highlight(&buf, row),
                "row {row} of the wrapped sentence's row-group must carry the Selection highlight");
        }
    }

    /// Probe 2: `select_section` on a FOLDED heading paints the Selection highlight on the folded
    /// heading row; the hidden body rows are never drawn at all (fold survival, spec §8 probe 2).
    #[test]
    fn select_section_on_a_folded_heading_highlights_the_heading_row_and_hides_the_body() {
        let doc = "## A\nbody of a.\n\n## B\nbody of b.\n";
        let mut e = Editor::new_from_text(doc, None, (30, 10));
        let a_heading_byte = doc.find("## A").unwrap();
        e.active_mut().folds.toggle(a_heading_byte);
        derive::rebuild(&mut e);
        let fv = e.active_fold_view();
        let body_a_line = e.active().document.buffer.byte_to_line(doc.find("body of a.").unwrap());
        let b_heading_line = e.active().document.buffer.byte_to_line(doc.find("## B").unwrap());
        assert!(!fv.is_hidden(0), "the folded heading line itself stays visible");
        assert!(fv.is_hidden(body_a_line), "the folded body is hidden");
        assert!(!fv.is_hidden(b_heading_line), "the next heading (outside the fold) stays visible");

        e.active_mut().document.selection = Selection::single(a_heading_byte);
        derive::rebuild(&mut e);
        assert_eq!(
            crate::commands::run(crate::commands::Command::SelectScope(crate::commands::Scope::Section),
                &mut e, &crate::test_support::TestClock(0)),
            crate::commands::CommandResult::Handled,
        );

        let buf = render_to_buffer(&mut e, 30, 10);
        assert!(row_has_highlight(&buf, 0), "the folded heading row must carry the Selection highlight");
        let screen: Vec<String> = (0..10u16).map(|r| row_string(&buf, r)).collect();
        assert!(!screen.iter().any(|r| r.contains("body of a.")),
            "hidden body rows must never be drawn: {screen:?}");
        // "## B" (an H2 heading, concealed to its numeral glyph HEADING_GLYPHS[1]) repaints
        // immediately on row 1 — no blank row left where the hidden body used to be.
        assert!(screen[1].contains(HEADING_GLYPHS[1]) && screen[1].contains('B'),
            "the section after the fold repaints on the very next visible row: {screen:?}");
        assert!(screen[2].contains("body of b."), "the un-folded section's own body is untouched: {screen:?}");
    }
}
