//! Inline markdown conceal + style analysis for one logical line.
//! Conceal-grid technique adapted from the validated layout spike.
use crate::style::{BlockRole, LineAnalysis, Run, Style, StyleSpan};
use std::ops::Range;

/// Analyze a logical line into visible/concealed runs and style spans.
///
/// If `is_active` is true (the cursor line), return the full source as one
/// visible run with no styles — the editor shows raw markdown for the active
/// line.  Otherwise, parse with pulldown-cmark and compute conceal + styles.
pub fn analyze(line: &str, role: BlockRole, is_active: bool) -> LineAnalysis {
    // Active line: show raw source.
    if is_active || line.is_empty() {
        return LineAnalysis {
            runs: vec![Run { src: 0..line.len(), visible: true }],
            styles: vec![],
            role,
            prefix_glyph: None,
        };
    }

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
                conceal.push(range);
                strong += 1;
            }
            Event::End(TagEnd::Strong) => {
                strong = strong.saturating_sub(1);
            }
            Event::Start(Tag::Emphasis) => {
                conceal.push(range);
                em += 1;
            }
            Event::End(TagEnd::Emphasis) => {
                em = em.saturating_sub(1);
            }
            Event::Start(Tag::Strikethrough) => {
                conceal.push(range);
                strike += 1;
            }
            Event::End(TagEnd::Strikethrough) => {
                strike = strike.saturating_sub(1);
            }
            Event::Start(Tag::Link { .. }) => {
                conceal.push(range);
                link += 1;
            }
            Event::End(TagEnd::Link) => {
                link = link.saturating_sub(1);
            }
            Event::Text(_) => {
                reveal.push(range.clone());
                let style = current_style(strong, em, strike, link);
                if style != Style::Plain {
                    styles.push(StyleSpan { src: range, style });
                }
            }
            Event::Code(_) => {
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
            Event::InlineHtml(_) => {
                // Style a `<!-- … -->` inline comment; leave other inline HTML (<span> etc.) Plain.
                let s = &line[range.clone()];
                if s.starts_with("<!--") && s.ends_with("-->") {
                    reveal.push(range.clone());          // keep the comment visible
                    styles.push(StyleSpan { src: range, style: Style::Comment });
                }
            }
            _ => {}
        }
    }

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
    let prefix_glyph = apply_block_prefix_conceal(&mut visible, line, &role);

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
    visible: &mut Vec<bool>,
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
                if level >= 1 && level <= 6 {
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
                        for b in 0..content_start {
                            visible[b] = false;
                        }
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
                            for b in cs..n {
                                visible[b] = false;
                            }
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
                    for b in 0..n {
                        visible[b] = false;
                    }
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
            None
        }

        BlockRole::ListItem => {
            // Skip optional leading spaces.
            let start = bytes.iter().take_while(|&&b| b == b' ').count();
            if start >= n {
                return None;
            }
            let b0 = bytes[start];

            // Unordered marker: `[-*+]` followed by space or tab
            if (b0 == b'-' || b0 == b'*' || b0 == b'+')
                && start + 1 < n
                && is_ws(bytes[start + 1])
            {
                visible[start] = false;
                visible[start + 1] = false;
                return Some("• ".to_string());
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
                    // Parse the ordinal number.
                    let ordinal: &str = &line[start..digit_end];
                    let glyph = format!("{}. ", ordinal);
                    // Conceal start..=digit_end+1 (digits + punctuation + space/tab).
                    for i in start..=digit_end + 1 {
                        visible[i] = false;
                    }
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
                        for b in 0..n {
                            visible[b] = false;
                        }
                    }
                }
            }
            None
        }

        BlockRole::ThematicBreak => {
            // Conceal the whole line.
            for b in 0..n {
                visible[b] = false;
            }
            None
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
        let a = analyze(line, BlockRole::Paragraph, true);
        assert_eq!(a.runs, vec![Run { src: 0..line.len(), visible: true }]);
        assert!(a.styles.is_empty());
    }

    #[test]
    fn strong_conceals_markers_keeps_text_with_style() {
        // bytes: a=0 ' '=1 *=2 *=3 b=4 o=5 l=6 d=7 *=8 *=9 ' '=10 c=11
        let line = "a **bold** c";
        let a = analyze(line, BlockRole::Paragraph, false);
        assert_eq!(visible(&a, line), "a bold c"); // ** hidden
        assert_eq!(style_at(&a, 4), Some(Style::Strong)); // 'b' of bold at byte 4
        assert_eq!(style_at(&a, 7), Some(Style::Strong)); // 'd' of bold, still Strong
    }

    #[test]
    fn escaped_marker_shows_literal() {
        // backslash escapes the asterisk: the '*' is literal text, the '\' is hidden.
        let line = r"a \* b"; // bytes: a=0 ' '=1 \=2 *=3 ' '=4 b=5
        let a = analyze(line, BlockRole::Paragraph, false);
        assert_eq!(visible(&a, line), "a * b"); // backslash concealed, * literal
    }

    #[test]
    fn emphasis_and_code_and_strike() {
        let line = "*i* `c` ~~s~~";
        let a = analyze(line, BlockRole::Paragraph, false);
        assert_eq!(visible(&a, line), "i c s");
        assert_eq!(style_at(&a, 1), Some(Style::Emphasis));   // 'i'
        assert_eq!(style_at(&a, 5), Some(Style::Code));       // 'c'
        assert_eq!(style_at(&a, 10), Some(Style::Strikethrough)); // 's'
    }

    #[test]
    fn link_hides_target_keeps_text() {
        let line = "see [docs](http://x.io) now";
        let a = analyze(line, BlockRole::Paragraph, false);
        assert_eq!(visible(&a, line), "see docs now");
        // 'd' of docs is at byte 5
        assert_eq!(style_at(&a, 5), Some(Style::Link));
    }

    #[test]
    fn bold_italic_is_strong_emphasis() {
        let line = "***x***"; // 'x' at byte 3
        let a = analyze(line, BlockRole::Paragraph, false);
        assert_eq!(visible(&a, line), "x");
        assert_eq!(style_at(&a, 3), Some(Style::StrongEmphasis));
    }

    // --- Task 3: block-prefix conceal tests ---

    #[test]
    fn heading_prefix_concealed() {
        let a = analyze("## Title", BlockRole::Heading(2), false);
        assert_eq!(visible(&a, "## Title"), "Title"); // "## " hidden
    }

    #[test]
    fn blockquote_prefix_concealed() {
        let a = analyze("> quoted", BlockRole::BlockQuote, false);
        assert_eq!(visible(&a, "> quoted"), "quoted");
    }

    #[test]
    fn list_marker_becomes_bullet_glyph() {
        let a = analyze("- item", BlockRole::ListItem, false);
        assert_eq!(visible(&a, "- item"), "item");
        assert_eq!(a.prefix_glyph.as_deref(), Some("• "));
    }

    #[test]
    fn fence_line_concealed() {
        let a = analyze("```rust", BlockRole::CodeBlock, false);
        assert_eq!(visible(&a, "```rust"), ""); // fence line hidden
    }

    #[test]
    fn active_line_keeps_block_prefix_raw() {
        let a = analyze("## Title", BlockRole::Heading(2), true);
        assert_eq!(visible(&a, "## Title"), "## Title"); // raw on cursor line
        assert!(a.prefix_glyph.is_none());
    }

    #[test]
    fn heading_prefix_composes_with_inline_style() {
        // "## **bold**" -> "## " AND "**" hidden, leaving "bold" as Strong.
        let line = "## **bold**";
        let a = analyze(line, BlockRole::Heading(2), false);
        assert_eq!(visible(&a, line), "bold");
        // 'b' is at byte 5 in "## **bold**"
        assert_eq!(style_at(&a, 5), Some(Style::Strong));
    }

    #[test]
    fn list_marker_composes_with_inline_style() {
        let line = "- **item**";
        let a = analyze(line, BlockRole::ListItem, false);
        assert_eq!(visible(&a, line), "item");
        assert_eq!(a.prefix_glyph.as_deref(), Some("• "));
    }

    #[test]
    fn setext_underline_concealed() {
        // role_at returns Heading for the underline line; conceal the whole "---".
        let a = analyze("---", BlockRole::Heading(1), false);
        assert_eq!(visible(&a, "---"), "");
    }

    #[test]
    fn setext_underline_rejects_embedded_spaces() {
        // "- - -" is NOT a setext underline (embedded spaces); with role Heading it
        // must NOT be concealed as an underline — its content stays visible.
        let a = analyze("- - -", BlockRole::Heading(1), false);
        assert_eq!(visible(&a, "- - -"), "- - -");
    }

    // --- Fix 1: TAB as marker whitespace ---

    #[test]
    fn heading_tab_after_hash() {
        let line = "#\tTitle";
        let a = analyze(line, BlockRole::Heading(1), false);
        assert_eq!(visible(&a, line), "Title");
    }

    #[test]
    fn list_unordered_tab_after_marker() {
        let line = "-\titem";
        let a = analyze(line, BlockRole::ListItem, false);
        assert_eq!(visible(&a, line), "item");
        assert_eq!(a.prefix_glyph.as_deref(), Some("• "));
    }

    #[test]
    fn list_ordered_tab_after_marker() {
        let line = "1.\titem";
        let a = analyze(line, BlockRole::ListItem, false);
        assert_eq!(visible(&a, line), "item");
        assert_eq!(a.prefix_glyph.as_deref(), Some("1. "));
    }

    #[test]
    fn blockquote_tab_after_angle() {
        let line = ">\tq";
        let a = analyze(line, BlockRole::BlockQuote, false);
        assert_eq!(visible(&a, line), "q");
    }

    // --- Fix 2: empty ATX heading ---

    #[test]
    fn heading_empty_atx_single_hash() {
        let line = "#";
        let a = analyze(line, BlockRole::Heading(1), false);
        assert_eq!(visible(&a, line), "");
    }

    #[test]
    fn heading_empty_atx_triple_hash() {
        let line = "###";
        let a = analyze(line, BlockRole::Heading(3), false);
        assert_eq!(visible(&a, line), "");
    }

    #[test]
    fn heading_empty_atx_trailing_spaces() {
        let line = "#  ";
        let a = analyze(line, BlockRole::Heading(1), false);
        assert_eq!(visible(&a, line), "");
    }

    // --- Fix 3: closing ATX sequence ---

    #[test]
    fn heading_closing_atx_single_hash() {
        let line = "# Title #";
        let a = analyze(line, BlockRole::Heading(1), false);
        assert_eq!(visible(&a, line), "Title");
    }

    #[test]
    fn heading_closing_atx_multiple_hashes() {
        let line = "## Title ###";
        let a = analyze(line, BlockRole::Heading(2), false);
        assert_eq!(visible(&a, line), "Title");
    }

    #[test]
    fn heading_closing_atx_trailing_spaces() {
        let line = "# Title #  ";
        let a = analyze(line, BlockRole::Heading(1), false);
        assert_eq!(visible(&a, line), "Title");
    }

    #[test]
    fn heading_closing_atx_content_is_just_hashes() {
        // "## #" -> content is empty after the closing hash is stripped
        let line = "## #";
        let a = analyze(line, BlockRole::Heading(2), false);
        assert_eq!(visible(&a, line), "");
    }

    #[test]
    fn heading_closing_not_sequence_no_preceding_ws() {
        // "# Title#" — trailing # not preceded by ws, so it's literal content
        let line = "# Title#";
        let a = analyze(line, BlockRole::Heading(1), false);
        assert_eq!(visible(&a, line), "Title#");
    }

    #[test]
    fn heading_closing_not_sequence_mid_content_hash() {
        // "# a #b" — the "#b" run is interrupted by 'b', so not a closing sequence.
        let line = "# a #b";
        let a = analyze(line, BlockRole::Heading(1), false);
        assert_eq!(visible(&a, line), "a #b");
    }

    #[test]
    fn heading_closing_sequence_multibyte_content() {
        // "# 中 #" — closing # stripped, multibyte content survives intact
        // (the right-to-left # scan must not split or misfire on UTF-8 bytes).
        let line = "# 中 #";
        let a = analyze(line, BlockRole::Heading(1), false);
        assert_eq!(visible(&a, line), "中");
    }

    #[test]
    fn heading_empty_atx_trailing_tab() {
        // "#\t" — hash followed by a tab then nothing → empty heading, whole line hidden.
        let line = "#\t";
        let a = analyze(line, BlockRole::Heading(1), false);
        assert_eq!(visible(&a, line), "");
    }

    // --- Task 2: inline <!-- --> → Style::Comment ---

    #[test]
    fn inline_html_comment_is_styled_comment() {
        let a = analyze("text <!-- note --> more", BlockRole::Paragraph, false);
        let cmt = a.styles.iter().find(|s| s.style == Style::Comment).expect("comment span");
        // span covers the `<!-- note -->`
        assert_eq!(&"text <!-- note --> more"[cmt.src.clone()], "<!-- note -->");
    }

    #[test]
    fn inline_html_non_comment_tag_is_not_comment() {
        let a = analyze("a <span>x</span> b", BlockRole::Paragraph, false);
        assert!(a.styles.iter().all(|s| s.style != Style::Comment), "a <span> is not a comment");
    }

    // --- Regressions ---

    #[test]
    fn heading_regression_no_closing() {
        let line = "## Title";
        let a = analyze(line, BlockRole::Heading(2), false);
        assert_eq!(visible(&a, line), "Title");
    }

    #[test]
    fn heading_regression_compose_bold() {
        let line = "## **bold**";
        let a = analyze(line, BlockRole::Heading(2), false);
        assert_eq!(visible(&a, line), "bold");
        // 'b' is at byte 5 in "## **bold**"
        assert_eq!(style_at(&a, 5), Some(Style::Strong));
    }

    #[test]
    fn heading_regression_active_line_raw() {
        let line = "## Title";
        let a = analyze(line, BlockRole::Heading(2), true);
        assert_eq!(visible(&a, line), "## Title");
        assert!(a.prefix_glyph.is_none());
    }
}
