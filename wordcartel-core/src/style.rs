//! Inline style + block-role types shared by md_parse and layout.
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style { Plain, Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link, Comment }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockRole { Paragraph, Heading(u8), BlockQuote, ListItem, CodeBlock, ThematicBreak, FrontMatter }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleSpan { pub src: Range<usize>, pub style: Style }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run { pub src: Range<usize>, pub visible: bool }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineAnalysis {
    pub runs: Vec<Run>,
    pub styles: Vec<StyleSpan>,
    pub role: BlockRole,
    pub prefix_glyph: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn types_construct() {
        let a = LineAnalysis {
            runs: vec![Run { src: 0..3, visible: true }],
            styles: vec![StyleSpan { src: 0..3, style: Style::Strong }],
            role: BlockRole::Paragraph,
            prefix_glyph: None,
        };
        assert_eq!(a.runs.len(), 1);
        assert_eq!(a.styles[0].style, Style::Strong);
        assert_eq!(a.role, BlockRole::Paragraph);
    }

    #[test]
    fn style_comment_exists() {
        let _ = Style::Comment;
        // total: a match over Style must be able to name Comment (compile-guard).
        fn _exhaustive(s: Style) -> u8 { match s {
            Style::Plain=>0, Style::Emphasis=>1, Style::Strong=>2, Style::StrongEmphasis=>3,
            Style::Code=>4, Style::Strikethrough=>5, Style::Link=>6, Style::Comment=>7 } }
    }
}
