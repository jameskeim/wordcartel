//! Inline markdown conceal + style analysis for one logical line.
//! Conceal-grid technique adapted from the validated layout spike.
use crate::style::{BlockRole, LineAnalysis, LineRender, Run, Style, StyleSpan};
use std::ops::Range;

/// Analyze a logical line into visible/concealed runs and style spans.
///
/// The `render` descriptor selects among three modes:
/// - `RawPlain`: show raw source with no styles (cursor/active line, SourcePlain).
/// - `Concealed`: hide markdown markers and style content (LivePreview inactive).
/// - `RawStyled`: show raw source with every construct styled — delimiters,
///   block prefixes, and content all carry their element face (SourceHighlighted).
pub fn analyze(line: &str, role: BlockRole, render: LineRender) -> LineAnalysis {
    // RawPlain: show raw source, no styles (the old is_active=true path).
    if render == LineRender::RawPlain || line.is_empty() {
        return LineAnalysis {
            runs: vec![Run { src: 0..line.len(), visible: true }],
            styles: vec![],
            role,
            prefix_glyph: None,
        };
    }
    let raw_styled = render == LineRender::RawStyled;

    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let n = line.len();
    let mut visible = vec![true; n];
    let mut styles: Vec<StyleSpan> = Vec::new();

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(line, opts).into_offset_iter();

    let mut conceal: Vec<Range<usize>> = Vec::new();
    let mut reveal: Vec<Range<usize>> = Vec::new();

    // Nesting counters for style tracking.
    let mut strong: usize = 0;
    let mut em: usize = 0;
    let mut strike: usize = 0;
    let mut link: usize = 0;

    for (ev, range) in parser {
        match ev {
            Event::Start(Tag::Strong) => {
                strong += 1;
                // RawStyled: push a whole-span style for the delimiter + content;
                // Concealed: add to the conceal list (content revealed by Text event).
                if raw_styled { styles.push(StyleSpan { src: range, style: current_style(strong, em, strike, link) }); }
                else { conceal.push(range); }
            }
            Event::End(TagEnd::Strong) => {
                strong = strong.saturating_sub(1);
            }
            Event::Start(Tag::Emphasis) => {
                em += 1;
                if raw_styled { styles.push(StyleSpan { src: range, style: current_style(strong, em, strike, link) }); }
                else { conceal.push(range); }
            }
            Event::End(TagEnd::Emphasis) => {
                em = em.saturating_sub(1);
            }
            Event::Start(Tag::Strikethrough) => {
                strike += 1;
                if raw_styled { styles.push(StyleSpan { src: range, style: current_style(strong, em, strike, link) }); }
                else { conceal.push(range); }
            }
            Event::End(TagEnd::Strikethrough) => {
                strike = strike.saturating_sub(1);
            }
            Event::Start(Tag::Link { .. }) => {
                link += 1;
                if raw_styled { styles.push(StyleSpan { src: range, style: current_style(strong, em, strike, link) }); }
                else { conceal.push(range); }
            }
            Event::End(TagEnd::Link) => {
                link = link.saturating_sub(1);
            }
            Event::Text(_) => {
                // In Concealed mode the text must be re-revealed (the whole span was
                // concealed by the Start event). In RawStyled every byte is already
                // visible — no reveal needed. Either way, push a content style span.
                if !raw_styled { reveal.push(range.clone()); }
                let style = current_style(strong, em, strike, link);
                if style != Style::Plain {
                    styles.push(StyleSpan { src: range, style });
                }
            }
            Event::Code(_) => {
                if raw_styled {
                    // Style the whole span (backticks + inner) as Code — no fence conceal.
                    styles.push(StyleSpan { src: range.clone(), style: Style::Code });
                } else {
                    // Conceal the leading and trailing backtick fence; reveal the
                    // inner content and mark it as Code.
                    let bytes = &line.as_bytes()[range.clone()];
                    let lead = bytes.iter().take_while(|&&b| b == b'`').count();
                    let trail = bytes.iter().rev().take_while(|&&b| b == b'`').count();
                    conceal.push(range.start..range.start + lead);
                    conceal.push(range.end - trail..range.end);
                    let inner = range.start + lead..range.end - trail;
                    reveal.push(inner.clone());
                    if !inner.is_empty() {
                        styles.push(StyleSpan { src: inner, style: Style::Code });
                    }
                }
            }
            Event::InlineHtml(_) => {
                // Style a `<!-- … -->` inline comment; leave other inline HTML (<span> etc.) Plain.
                let s = &line[range.clone()];
                if s.starts_with("<!--") && s.ends_with("-->") {
                    // In Concealed mode the comment bytes may have been hidden by a wrapping
                    // construct — re-reveal them. In RawStyled all bytes are already visible.
                    if !raw_styled { reveal.push(range.clone()); }
                    styles.push(StyleSpan { src: range, style: Style::Comment });
                }
            }
            _ => {}
        }
    }

    // Apply conceal/reveal, escapes, and block-prefix conceal only for Concealed mode.
    // RawStyled reveals all bytes — no mutations to the visibility grid needed.
    let prefix_glyph = if !raw_styled {
        // Apply conceal then reveal (reveal wins over conceal).
        for r in conceal {
            for b in r {
                if b < n {
                    visible[b] = false;
                }
            }
        }
        for r in reveal {
            for b in r {
                if b < n {
                    visible[b] = true;
                }
            }
        }

        // Escapes: hide the backslash in `\<punctuation>` sequences.
        // Do this after the conceal/reveal pass so it overrides the grid
        // independently of how pulldown-cmark handles escape events.
        let bytes = line.as_bytes();
        let mut i = 0;
        while i + 1 < n {
            if bytes[i] == b'\\' && bytes[i + 1].is_ascii_punctuation() {
                visible[i] = false;
            }
            i += 1;
        }

        // Block-prefix conceal: LAST grid mutation before collapse_runs.
        // This runs AFTER inline conceal/reveal/escape so # markers cannot be
        // re-revealed by the inline reveal pass (e.g. "## **bold**" hides both
        // "## " and "**", leaving "bold" styled Strong).
        apply_block_prefix_conceal(&mut visible, line, &role)
    } else {
        // RawStyled: all bytes remain visible; block prefixes show in raw source.
        None
    };

    // Collapse the visible grid to Run slices.
    let runs = collapse_runs(&visible, n);

    LineAnalysis { runs, styles, role, prefix_glyph }
}

/// True iff `b` is a space or tab (CommonMark marker whitespace).
#[inline]
fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Apply block-prefix concealment based on the block role.
/// Mutates the per-byte `visible` grid (sets matching prefix bytes to false).
/// Returns the prefix glyph string for list items, None otherwise.
fn apply_block_prefix_conceal(
    visible: &mut [bool],
    line: &str,
    role: &BlockRole,
) -> Option<String> {
    let bytes = line.as_bytes();
    let n = bytes.len();

    match role {
        BlockRole::Heading(_) => {
            // Skip optional leading spaces.
            let start = bytes.iter().take_while(|&&b| b == b' ').count();

            // Check for ATX heading: `#{1,6}` followed by space/tab or EOL.
            if start < n && bytes[start] == b'#' {
                let hash_end = bytes[start..]
                    .iter()
                    .take_while(|&&b| b == b'#')
                    .count()
                    + start;
                let level = hash_end - start;
                // Valid level: 1..=6
                if (1..=6).contains(&level) {
                    // Valid opener: EOL or followed by space/tab
                    let valid_opener =
                        hash_end == n || is_ws(bytes[hash_end]);
                    if valid_opener {
                        // content_start: first non-ws byte after hash_end
                        let mut content_start = hash_end;
                        while content_start < n && is_ws(bytes[content_start]) {
                            content_start += 1;
                        }
                        // Conceal opening: indent + hashes + following ws
                        visible[..content_start].fill(false);
                        // Empty heading: whole line concealed
                        if content_start >= n {
                            return None;
                        }
                        // Detect optional closing sequence.
                        // te = index after last non-ws char (trim trailing ws)
                        let mut te = n;
                        while te > content_start && is_ws(bytes[te - 1]) {
                            te -= 1;
                        }
                        // cs = start of trailing '#' run
                        let mut cs = te;
                        while cs > content_start && bytes[cs - 1] == b'#' {
                            cs -= 1;
                        }
                        let hashes = te - cs;
                        // Valid closing sequence: at least one '#', preceded by
                        // ws or the '#' run starts at content_start (entire
                        // visible content is hashes).
                        if hashes > 0
                            && (cs == content_start || is_ws(bytes[cs - 1]))
                        {
                            // Back cs over the preceding ws
                            while cs > content_start && is_ws(bytes[cs - 1]) {
                                cs -= 1;
                            }
                            // Conceal cs..n (trailing ws + closing hashes + any
                            // trailing ws already captured in te..n)
                            visible[cs..n].fill(false);
                        }
                        return None;
                    }
                }
            }

            // Check for setext underline: whole line is `[=-]+` with optional spaces.
            // If so, conceal the whole line.
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let first = trimmed.as_bytes()[0];
                if (first == b'=' || first == b'-')
                    && trimmed.bytes().all(|b| b == first)
                {
                    visible[..n].fill(false);
                    return None;
                }
            }

            // Setext text line: show as-is (role provides heading style via Task 4).
            None
        }

        BlockRole::BlockQuote => {
            // Conceal one leading `>` and optional following space or tab.
            let start = bytes.iter().take_while(|&&b| b == b' ').count();
            if start < n && bytes[start] == b'>' {
                visible[start] = false;
                // Also hide the space/tab after `>` if present.
                if start + 1 < n && is_ws(bytes[start + 1]) {
                    visible[start + 1] = false;
                }
            }
            Some("▎ ".to_string())
        }

        BlockRole::ListItem => {
            // Skip leading indent: spaces AND tabs (spec D3 — the scan is tab-aware).
            let start = bytes.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
            if start >= n {
                return None;
            }
            // The glyph reproduces the indent's display width: space as-is, tab as
            // TAB_WIDTH spaces (matches layout's tab policy) — so the bullet paints
            // at its indent level and continuation rows hang under the item text.
            // TAB_WIDTH = 4 (layout.rs tab policy)
            let indent_str: String = bytes[..start]
                .iter()
                .map(|&b| if b == b'\t' { "    " } else { " " })
                .collect();
            let b0 = bytes[start];

            // Unordered marker: `[-*+]` followed by space or tab
            if (b0 == b'-' || b0 == b'*' || b0 == b'+')
                && start + 1 < n
                && is_ws(bytes[start + 1])
            {
                // Conceal indent + marker + its whitespace (marker-conditional: the
                // no-marker path below conceals NOTHING — spec I4).
                visible[..start].fill(false);
                visible[start] = false;
                visible[start + 1] = false;
                return Some(format!("{indent_str}• "));
            }

            // Ordered marker: `<digits>[.)]` followed by space or tab
            if b0.is_ascii_digit() {
                let digit_end = bytes[start..]
                    .iter()
                    .take_while(|&&b| b.is_ascii_digit())
                    .count()
                    + start;
                if digit_end < n
                    && (bytes[digit_end] == b'.' || bytes[digit_end] == b')')
                    && digit_end + 1 < n
                    && is_ws(bytes[digit_end + 1])
                {
                    let ordinal: &str = &line[start..digit_end];
                    let glyph = format!("{indent_str}{ordinal}. ");
                    visible[..start].fill(false);
                    visible[start..=digit_end + 1].fill(false);
                    return Some(glyph);
                }
            }

            None
        }

        BlockRole::CodeBlock => {
            // If the line (after optional spaces) starts with ``` or ~~~,
            // it's a fence line — conceal the whole line.
            let start = bytes.iter().take_while(|&&b| b == b' ').count();
            if start < n {
                let b0 = bytes[start];
                if b0 == b'`' || b0 == b'~' {
                    let fence_end = bytes[start..]
                        .iter()
                        .take_while(|&&b| b == b0)
                        .count()
                        + start;
                    if fence_end - start >= 3 {
                        // It's a valid fence opener/closer.
                        visible[..n].fill(false);
                    }
                }
            }
            None
        }

        BlockRole::ThematicBreak => {
            // Conceal the whole line.
            visible[..n].fill(false);
            Some("─── ".to_string())
        }

        // Block HTML comments: no prefix glyph (the full source is shown as-is).
        BlockRole::Comment => None,

        // Paragraph and others: no prefix conceal.
        _ => None,
    }
}

/// Derive the current style from active nesting counters.
fn current_style(strong: usize, em: usize, strike: usize, link: usize) -> Style {
    if strong > 0 && em > 0 {
        Style::StrongEmphasis
    } else if strong > 0 {
        Style::Strong
    } else if em > 0 {
        Style::Emphasis
    } else if strike > 0 {
        Style::Strikethrough
    } else if link > 0 {
        Style::Link
    } else {
        Style::Plain
    }
}

/// Collapse a per-byte visibility grid into `Run`s.
fn collapse_runs(visible: &[bool], n: usize) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut i = 0;
    while i < n {
        let vis = visible[i];
        let start = i;
        i += 1;
        while i < n && visible[i] == vis {
            i += 1;
        }
        runs.push(Run { src: start..i, visible: vis });
    }
    if runs.is_empty() {
        runs.push(Run { src: 0..0, visible: true });
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: the concatenated visible source (bytes whose run.visible == true).
    fn visible(a: &LineAnalysis, line: &str) -> String {
        let mut s = String::new();
        for r in &a.runs { if r.visible { s.push_str(&line[r.src.clone()]); } }
        s
    }
    // Helper: the style covering a given visible byte, if any.
    fn style_at(a: &LineAnalysis, b: usize) -> Option<Style> {
        a.styles.iter().find(|s| s.src.contains(&b)).map(|s| s.style)
    }

    #[test]
    fn active_line_is_raw() {
        let line = "a **b** c";
        let a = analyze(line, BlockRole::Paragraph, LineRender::RawPlain);
        assert_eq!(a.runs, vec![Run { src: 0..line.len(), visible: true }]);
        assert!(a.styles.is_empty());
    }

    #[test]
    fn strong_conceals_markers_keeps_text_with_style() {
        // bytes: a=0 ' '=1 *=2 *=3 b=4 o=5 l=6 d=7 *=8 *=9 ' '=10 c=11
        let line = "a **bold** c";
        let a = analyze(line, BlockRole::Paragraph, LineRender::Concealed);
        assert_eq!(visible(&a, line), "a bold c"); // ** hidden
        assert_eq!(style_at(&a, 4), Some(Style::Strong)); // 'b' of bold at byte 4
        assert_eq!(style_at(&a, 7), Some(Style::Strong)); // 'd' of bold, still Strong
    }

    #[test]
    fn escaped_marker_shows_literal() {
        // backslash escapes the asterisk: the '*' is literal text, the '\' is hidden.
        let line = r"a \* b"; // bytes: a=0 ' '=1 \=2 *=3 ' '=4 b=5
        let a = analyze(line, BlockRole::Paragraph, LineRender::Concealed);
        assert_eq!(visible(&a, line), "a * b"); // backslash concealed, * literal
    }

    #[test]
    fn emphasis_and_code_and_strike() {
        let line = "*i* `c` ~~s~~";
        let a = analyze(line, BlockRole::Paragraph, LineRender::Concealed);
        assert_eq!(visible(&a, line), "i c s");
        assert_eq!(style_at(&a, 1), Some(Style::Emphasis));   // 'i'
        assert_eq!(style_at(&a, 5), Some(Style::Code));       // 'c'
        assert_eq!(style_at(&a, 10), Some(Style::Strikethrough)); // 's'
    }

    #[test]
    fn link_hides_target_keeps_text() {
        let line = "see [docs](http://x.io) now";
        let a = analyze(line, BlockRole::Paragraph, LineRender::Concealed);
        assert_eq!(visible(&a, line), "see docs now");
        // 'd' of docs is at byte 5
        assert_eq!(style_at(&a, 5), Some(Style::Link));
    }

    #[test]
    fn bold_italic_is_strong_emphasis() {
        let line = "***x***"; // 'x' at byte 3
        let a = analyze(line, BlockRole::Paragraph, LineRender::Concealed);
        assert_eq!(visible(&a, line), "x");
        assert_eq!(style_at(&a, 3), Some(Style::StrongEmphasis));
    }

    // --- Task 3: block-prefix conceal tests ---

    #[test]
    fn heading_prefix_concealed() {
        let a = analyze("## Title", BlockRole::Heading(2), LineRender::Concealed);
        assert_eq!(visible(&a, "## Title"), "Title"); // "## " hidden
    }

    #[test]
    fn blockquote_prefix_concealed() {
        let a = analyze("> quoted", BlockRole::BlockQuote, LineRender::Concealed);
        assert_eq!(visible(&a, "> quoted"), "quoted");
    }

    #[test]
    fn blockquote_has_bar_glyph() {
        let a = analyze("> quoted", BlockRole::BlockQuote, LineRender::Concealed);
        assert_eq!(a.prefix_glyph.as_deref(), Some("▎ "));
        assert_eq!(visible(&a, "> quoted"), "quoted"); // existing conceal still holds
    }

    #[test]
    fn thematic_break_has_rule_glyph() {
        let a = analyze("---", BlockRole::ThematicBreak, LineRender::Concealed);
        assert_eq!(a.prefix_glyph.as_deref(), Some("─── "));
    }

    #[test]
    fn list_marker_becomes_bullet_glyph() {
        let a = analyze("- item", BlockRole::ListItem, LineRender::Concealed);
        assert_eq!(visible(&a, "- item"), "item");
        assert_eq!(a.prefix_glyph.as_deref(), Some("• "));
    }

    #[test]
    fn fence_line_concealed() {
        let a = analyze("```rust", BlockRole::CodeBlock, LineRender::Concealed);
        assert_eq!(visible(&a, "```rust"), ""); // fence line hidden
    }

    #[test]
    fn active_line_keeps_block_prefix_raw() {
        let a = analyze("## Title", BlockRole::Heading(2), LineRender::RawPlain);
        assert_eq!(visible(&a, "## Title"), "## Title"); // raw on cursor line
        assert!(a.prefix_glyph.is_none());
    }

    #[test]
    fn heading_prefix_composes_with_inline_style() {
        // "## **bold**" -> "## " AND "**" hidden, leaving "bold" as Strong.
        let line = "## **bold**";
        let a = analyze(line, BlockRole::Heading(2), LineRender::Concealed);
        assert_eq!(visible(&a, line), "bold");
        // 'b' is at byte 5 in "## **bold**"
        assert_eq!(style_at(&a, 5), Some(Style::Strong));
    }

    #[test]
    fn list_marker_composes_with_inline_style() {
        let line = "- **item**";
        let a = analyze(line, BlockRole::ListItem, LineRender::Concealed);
        assert_eq!(visible(&a, line), "item");
        assert_eq!(a.prefix_glyph.as_deref(), Some("• "));
    }

    #[test]
    fn setext_underline_concealed() {
        // role_at returns Heading for the underline line; conceal the whole "---".
        let a = analyze("---", BlockRole::Heading(1), LineRender::Concealed);
        assert_eq!(visible(&a, "---"), "");
    }

    #[test]
    fn setext_underline_rejects_embedded_spaces() {
        // "- - -" is NOT a setext underline (embedded spaces); with role Heading it
        // must NOT be concealed as an underline — its content stays visible.
        let a = analyze("- - -", BlockRole::Heading(1), LineRender::Concealed);
        assert_eq!(visible(&a, "- - -"), "- - -");
    }

    // --- Fix 1: TAB as marker whitespace ---

    #[test]
    fn heading_tab_after_hash() {
        let line = "#\tTitle";
        let a = analyze(line, BlockRole::Heading(1), LineRender::Concealed);
        assert_eq!(visible(&a, line), "Title");
    }

    #[test]
    fn list_unordered_tab_after_marker() {
        let line = "-\titem";
        let a = analyze(line, BlockRole::ListItem, LineRender::Concealed);
        assert_eq!(visible(&a, line), "item");
        assert_eq!(a.prefix_glyph.as_deref(), Some("• "));
    }

    #[test]
    fn list_ordered_tab_after_marker() {
        let line = "1.\titem";
        let a = analyze(line, BlockRole::ListItem, LineRender::Concealed);
        assert_eq!(visible(&a, line), "item");
        assert_eq!(a.prefix_glyph.as_deref(), Some("1. "));
    }

    #[test]
    fn blockquote_tab_after_angle() {
        let line = ">\tq";
        let a = analyze(line, BlockRole::BlockQuote, LineRender::Concealed);
        assert_eq!(visible(&a, line), "q");
    }

    // --- Fix 2: empty ATX heading ---

    #[test]
    fn heading_empty_atx_single_hash() {
        let line = "#";
        let a = analyze(line, BlockRole::Heading(1), LineRender::Concealed);
        assert_eq!(visible(&a, line), "");
    }

    #[test]
    fn heading_empty_atx_triple_hash() {
        let line = "###";
        let a = analyze(line, BlockRole::Heading(3), LineRender::Concealed);
        assert_eq!(visible(&a, line), "");
    }

    #[test]
    fn heading_empty_atx_trailing_spaces() {
        let line = "#  ";
        let a = analyze(line, BlockRole::Heading(1), LineRender::Concealed);
        assert_eq!(visible(&a, line), "");
    }

    // --- Fix 3: closing ATX sequence ---

    #[test]
    fn heading_closing_atx_single_hash() {
        let line = "# Title #";
        let a = analyze(line, BlockRole::Heading(1), LineRender::Concealed);
        assert_eq!(visible(&a, line), "Title");
    }

    #[test]
    fn heading_closing_atx_multiple_hashes() {
        let line = "## Title ###";
        let a = analyze(line, BlockRole::Heading(2), LineRender::Concealed);
        assert_eq!(visible(&a, line), "Title");
    }

    #[test]
    fn heading_closing_atx_trailing_spaces() {
        let line = "# Title #  ";
        let a = analyze(line, BlockRole::Heading(1), LineRender::Concealed);
        assert_eq!(visible(&a, line), "Title");
    }

    #[test]
    fn heading_closing_atx_content_is_just_hashes() {
        // "## #" -> content is empty after the closing hash is stripped
        let line = "## #";
        let a = analyze(line, BlockRole::Heading(2), LineRender::Concealed);
        assert_eq!(visible(&a, line), "");
    }

    #[test]
    fn heading_closing_not_sequence_no_preceding_ws() {
        // "# Title#" — trailing # not preceded by ws, so it's literal content
        let line = "# Title#";
        let a = analyze(line, BlockRole::Heading(1), LineRender::Concealed);
        assert_eq!(visible(&a, line), "Title#");
    }

    #[test]
    fn heading_closing_not_sequence_mid_content_hash() {
        // "# a #b" — the "#b" run is interrupted by 'b', so not a closing sequence.
        let line = "# a #b";
        let a = analyze(line, BlockRole::Heading(1), LineRender::Concealed);
        assert_eq!(visible(&a, line), "a #b");
    }

    #[test]
    fn heading_closing_sequence_multibyte_content() {
        // "# 中 #" — closing # stripped, multibyte content survives intact
        // (the right-to-left # scan must not split or misfire on UTF-8 bytes).
        let line = "# 中 #";
        let a = analyze(line, BlockRole::Heading(1), LineRender::Concealed);
        assert_eq!(visible(&a, line), "中");
    }

    #[test]
    fn heading_empty_atx_trailing_tab() {
        // "#\t" — hash followed by a tab then nothing → empty heading, whole line hidden.
        let line = "#\t";
        let a = analyze(line, BlockRole::Heading(1), LineRender::Concealed);
        assert_eq!(visible(&a, line), "");
    }

    // --- Task 2: inline <!-- --> → Style::Comment ---

    #[test]
    fn inline_html_comment_is_styled_comment() {
        let a = analyze("text <!-- note --> more", BlockRole::Paragraph, LineRender::Concealed);
        let cmt = a.styles.iter().find(|s| s.style == Style::Comment).expect("comment span");
        // span covers the `<!-- note -->`
        assert_eq!(&"text <!-- note --> more"[cmt.src.clone()], "<!-- note -->");
    }

    #[test]
    fn inline_html_non_comment_tag_is_not_comment() {
        let a = analyze("a <span>x</span> b", BlockRole::Paragraph, LineRender::Concealed);
        assert!(a.styles.iter().all(|s| s.style != Style::Comment), "a <span> is not a comment");
    }

    // --- Task B2: nested-list indent conceal ---

    #[test]
    fn nested_unordered_indent_concealed_into_glyph() {
        let a = analyze("  - sub", BlockRole::ListItem, LineRender::Concealed);
        assert_eq!(visible(&a, "  - sub"), "sub");
        assert_eq!(a.prefix_glyph.as_deref(), Some("  • "));
    }

    #[test]
    fn tab_indented_item_recognized_and_expanded() {
        // A leading tab is indent (spec D3: the scan is now tab-aware) and expands
        // to TAB_WIDTH spaces in the glyph so widths match the old visual layout.
        let a = analyze("\t- sub", BlockRole::ListItem, LineRender::Concealed);
        assert_eq!(visible(&a, "\t- sub"), "sub");
        assert_eq!(a.prefix_glyph.as_deref(), Some("    • "));
    }

    #[test]
    fn nested_ordered_indent_concealed_into_glyph() {
        let a = analyze("   2. x", BlockRole::ListItem, LineRender::Concealed);
        assert_eq!(visible(&a, "   2. x"), "x");
        assert_eq!(a.prefix_glyph.as_deref(), Some("   2. "));
    }

    #[test]
    fn markerless_listitem_continuation_keeps_indent_no_glyph() {
        // Continuation lines of a multi-line item carry ListItem role with no marker
        // (spec I4): indent must stay VISIBLE and no glyph appear — else invisible text.
        let a = analyze("  second", BlockRole::ListItem, LineRender::Concealed);
        assert_eq!(visible(&a, "  second"), "  second");
        assert_eq!(a.prefix_glyph, None);
    }

    // --- Regressions ---

    #[test]
    fn heading_regression_no_closing() {
        let line = "## Title";
        let a = analyze(line, BlockRole::Heading(2), LineRender::Concealed);
        assert_eq!(visible(&a, line), "Title");
    }

    #[test]
    fn heading_regression_compose_bold() {
        let line = "## **bold**";
        let a = analyze(line, BlockRole::Heading(2), LineRender::Concealed);
        assert_eq!(visible(&a, line), "bold");
        // 'b' is at byte 5 in "## **bold**"
        assert_eq!(style_at(&a, 5), Some(Style::Strong));
    }

    #[test]
    fn heading_regression_active_line_raw() {
        let line = "## Title";
        let a = analyze(line, BlockRole::Heading(2), LineRender::RawPlain);
        assert_eq!(visible(&a, line), "## Title");
        assert!(a.prefix_glyph.is_none());
    }

    // --- Task 1: RawStyled branch tests ---

    #[test]
    fn raw_styled_reveals_all_markers_and_styles_delimiters_and_content() {
        // "**bold**": RawStyled reveals every byte (no conceal) AND styles the whole
        // construct (delimiters + content) Strong.
        let a = analyze("**bold**", BlockRole::Paragraph, LineRender::RawStyled);
        assert!(a.runs.iter().all(|r| r.visible), "RawStyled conceals nothing");
        // every byte of "**bold**" resolves to Strong (delimiters included)
        for b in 0.."**bold**".len() {
            let s = a.styles.iter().rfind(|s| s.src.contains(&b)).map(|s| s.style);
            assert_eq!(s, Some(Style::Strong), "byte {b} of **bold** must be Strong");
        }
    }

    #[test]
    fn raw_styled_nested_delimiters_take_position_style() {
        // "**_x_**": the outer ** = Strong, the inner _ and x = StrongEmphasis.
        let a = analyze("**_x_**", BlockRole::Paragraph, LineRender::RawStyled);
        let at = |b: usize| a.styles.iter().rfind(|s| s.src.contains(&b)).map(|s| s.style);
        assert_eq!(at(0), Some(Style::Strong), "opening ** is Strong");
        let ux = "**_x_**".find("_x_").unwrap();
        assert_eq!(at(ux), Some(Style::StrongEmphasis), "inner _ is Strong+Em");        // '_'
        assert_eq!(at(ux + 1), Some(Style::StrongEmphasis), "x is Strong+Em");          // 'x'
    }

    #[test]
    fn concealed_and_rawplain_unchanged() {
        // Concealed == old is_active=false; RawPlain == old is_active=true.
        let c = analyze("**bold**", BlockRole::Paragraph, LineRender::Concealed);
        assert!(c.runs.iter().any(|r| !r.visible), "Concealed still hides the ** markers");
        let p = analyze("**bold**", BlockRole::Paragraph, LineRender::RawPlain);
        assert!(p.runs.iter().all(|r| r.visible) && p.styles.is_empty(), "RawPlain = raw, no styles");
    }
}
