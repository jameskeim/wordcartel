//! Inline style + block-role types shared by md_parse and layout.
use std::ops::Range;

/// Inline style applied to a run of source bytes within a line's rendered text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// No inline styling — the default/fallback face.
    Plain,
    /// Markdown emphasis (`_x_` / `*x*`), rendered italic.
    Emphasis,
    /// Markdown strong emphasis (`**x**`), rendered bold.
    Strong,
    /// Nested bold-and-italic emphasis (e.g. `**_x_**`).
    StrongEmphasis,
    /// Inline code span (`` `x` ``) — the backtick fence is concealed, the inner text is styled.
    Code,
    /// Strikethrough text (`~~x~~`).
    Strikethrough,
    /// Link text inside `[x](url)`.
    Link,
    /// An inline HTML comment span (`<!-- ... -->`).
    Comment,
}

/// The markdown block-level kind of the line being analyzed; drives which block-prefix
/// markers are concealed and how the line is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockRole {
    /// A plain paragraph line — no prefix conceal.
    Paragraph,
    /// An ATX or setext heading; the payload is the heading level, 1–6.
    Heading(u8),
    /// A `>`-prefixed block-quote line; the marker and following space are concealed.
    BlockQuote,
    /// A list item (bulleted or ordered); the indent and marker are concealed.
    ListItem,
    /// A fenced or indented code-block line; fence lines are fully concealed and content
    /// lines are exempt from word-wrap.
    CodeBlock,
    /// A thematic break (`---`) rule; the whole line is concealed.
    ThematicBreak,
    /// A line inside a YAML front-matter block.
    FrontMatter,
    /// A block-level HTML comment (`<!-- ... -->`), distinct from a list item.
    Comment,
}

/// A byte range of a logical line tagged with the [`Style`] to render over it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleSpan {
    /// Byte offsets into the line's source text (not grapheme or column indices).
    pub src: Range<usize>,
    /// The style applying to that byte range.
    pub style: Style,
}

/// A maximal same-visibility byte slice of a line, produced by collapsing the line's
/// per-byte visibility grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// Byte offsets into the line's source text.
    pub src: Range<usize>,
    /// Whether this slice is shown in the rendered output (`true`), or is concealed markdown
    /// syntax that occupies the source but is hidden from view (`false`).
    pub visible: bool,
}

/// The result of analyzing one logical source line: how it decomposes into visible/concealed
/// runs, inline styles, and its block role, ready for layout to turn into visual rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineAnalysis {
    /// Ordered runs covering the whole line — both visible and concealed spans.
    pub runs: Vec<Run>,
    /// Inline style spans over (typically visible) sub-ranges of the line.
    pub styles: Vec<StyleSpan>,
    /// The line's block-level role.
    pub role: BlockRole,
    /// Synthetic replacement text painted in place of a concealed block prefix (e.g. a
    /// block-quote bar, list bullet/ordinal, or thematic-break rule); `None` when the line
    /// has no prefix glyph.
    pub prefix_glyph: Option<String>,
}

/// How one logical line is rendered into visual rows. Replaces the old
/// `is_active: bool`. `Concealed` hides markdown markers and styles content
/// (LivePreview inactive lines). `RawPlain` shows raw source with no styles
/// (the LivePreview active/caret line and all SourcePlain lines). `RawStyled`
/// shows raw source with every construct — delimiters, block prefixes, and
/// content — styled in its element face (SourceHighlighted). Concealment
/// (hence geometry) is identical for `RawPlain` and `RawStyled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineRender {
    /// Markdown markers are hidden and content is styled (LivePreview inactive lines).
    Concealed,
    /// Raw source is shown with no styles (the LivePreview active/caret line and all
    /// SourcePlain lines).
    RawPlain,
    /// Raw source is shown with every construct — delimiters, block prefixes, and content —
    /// styled in its element face (SourceHighlighted).
    RawStyled,
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
        // compile-guard: every BlockRole variant must be named
        fn _exhaustive_block_role(r: super::BlockRole) -> u8 { match r {
            super::BlockRole::Paragraph=>0, super::BlockRole::Heading(_)=>1,
            super::BlockRole::BlockQuote=>2, super::BlockRole::ListItem=>3,
            super::BlockRole::CodeBlock=>4, super::BlockRole::ThematicBreak=>5,
            super::BlockRole::FrontMatter=>6, super::BlockRole::Comment=>7 } }
        // compile-guard: every LineRender variant must be named (exhaustive match).
        fn _exhaustive_line_render(r: super::LineRender) -> u8 { match r {
            super::LineRender::Concealed=>0, super::LineRender::RawPlain=>1,
            super::LineRender::RawStyled=>2 } }
    }
}
