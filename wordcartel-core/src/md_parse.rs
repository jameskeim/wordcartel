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

    // Collapse the visible grid to Run slices.
    let runs = collapse_runs(&visible, n);

    LineAnalysis { runs, styles, role }
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
}
