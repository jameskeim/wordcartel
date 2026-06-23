use std::ops::Range;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

/// Kinds of block we track. Inline-level tags are ignored; we only keep the
/// block skeleton, which is what the renderer's layout depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Document,
    Paragraph,
    Heading,
    FencedCode,
    IndentedCode,
    BlockQuote,
    List,
    ListItem,
    ThematicBreak,
    HtmlBlock,
    Table,
    /// Footnote definitions / metadata blocks / def lists collapsed here.
    Other,
}

/// One block in the tree. `span` is a byte range into the source text.
/// Containers (BlockQuote/List/ListItem/Document) carry children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub span: Range<usize>,
    pub children: Vec<Block>,
}

/// The block tree. Top level is a synthetic Document whose children are the
/// top-level blocks of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockTree {
    pub root: Block,
}

impl BlockTree {
    /// Convenience: the top-level blocks.
    pub fn top_level(&self) -> &[Block] {
        &self.root.children
    }
}

/// GFM-ish options. Tables on; strikethrough is inline so it doesn't affect
/// the block tree but matches the editor's intended config. Footnotes OFF per
/// the spike brief.
fn options() -> Options {
    let mut o = Options::empty();
    o.insert(Options::ENABLE_TABLES);
    o.insert(Options::ENABLE_STRIKETHROUGH);
    o
}

fn tag_to_kind(tag: &Tag) -> Option<BlockKind> {
    Some(match tag {
        Tag::Paragraph => BlockKind::Paragraph,
        Tag::Heading { .. } => BlockKind::Heading,
        Tag::CodeBlock(CodeBlockKind::Fenced(_)) => BlockKind::FencedCode,
        Tag::CodeBlock(CodeBlockKind::Indented) => BlockKind::IndentedCode,
        Tag::BlockQuote(_) => BlockKind::BlockQuote,
        Tag::List(_) => BlockKind::List,
        Tag::Item => BlockKind::ListItem,
        Tag::HtmlBlock => BlockKind::HtmlBlock,
        Tag::Table(_) => BlockKind::Table,
        Tag::FootnoteDefinition(_)
        | Tag::DefinitionList
        | Tag::DefinitionListTitle
        | Tag::DefinitionListDefinition
        | Tag::MetadataBlock(_) => BlockKind::Other,
        // Inline / table-internal tags: not block-level for our skeleton.
        Tag::TableHead
        | Tag::TableRow
        | Tag::TableCell
        | Tag::Emphasis
        | Tag::Strong
        | Tag::Strikethrough
        | Tag::Superscript
        | Tag::Subscript
        | Tag::Link { .. }
        | Tag::Image { .. } => return None,
    })
}

/// pulldown-cmark does not emit Start/End for thematic breaks; it emits a
/// standalone `Event::Rule`. We synthesize a leaf block for it.
fn is_rule(event: &Event) -> bool {
    matches!(event, Event::Rule)
}

/// THE ORACLE. Walk block-level events, building a nested tree with byte spans.
pub fn full_parse(text: &str) -> BlockTree {
    parse_region(text, 0)
}

/// Parse `text`, treating it as living at byte offset `base` in some larger
/// document. All spans are shifted by `base`.
fn parse_region(text: &str, base: usize) -> BlockTree {
    let parser = Parser::new_ext(text, options());

    let mut root = Block {
        kind: BlockKind::Document,
        span: base..base + text.len(),
        children: Vec::new(),
    };
    let mut stack: Vec<Block> = Vec::new();

    for (event, range) in parser.into_offset_iter() {
        let span = (range.start + base)..(range.end + base);
        match event {
            Event::Start(tag) => {
                if let Some(kind) = tag_to_kind(&tag) {
                    stack.push(Block { kind, span, children: Vec::new() });
                }
            }
            Event::End(_) => {
                if let Some(done) = stack.pop() {
                    push_child(&mut root, &mut stack, done);
                }
            }
            _ if is_rule(&event) => {
                let rule = Block { kind: BlockKind::ThematicBreak, span, children: Vec::new() };
                push_child(&mut root, &mut stack, rule);
            }
            _ => {}
        }
    }
    while let Some(done) = stack.pop() {
        push_child(&mut root, &mut stack, done);
    }
    BlockTree { root }
}

fn push_child(root: &mut Block, stack: &mut Vec<Block>, done: Block) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(done),
        None => root.children.push(done),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(t: &BlockTree) -> Vec<BlockKind> {
        // BlockKind is Clone (not Copy) in the spike — clone, don't move out of &Block.
        t.top_level().iter().map(|b| b.kind.clone()).collect()
    }

    #[test]
    fn parses_heading_and_paragraph() {
        let t = full_parse("# Title\n\nbody text\n");
        assert_eq!(kinds(&t), vec![BlockKind::Heading, BlockKind::Paragraph]);
    }

    #[test]
    fn fenced_code_spans_blank_lines_as_one_block() {
        let t = full_parse("```\na\n\nb\n```\n");
        assert_eq!(kinds(&t), vec![BlockKind::FencedCode]); // the blank line is INSIDE the fence
    }

    #[test]
    fn blockquote_is_a_container() {
        let t = full_parse("> quoted\n");
        assert_eq!(t.top_level()[0].kind, BlockKind::BlockQuote);
        assert!(!t.top_level()[0].children.is_empty()); // contains a paragraph
    }
}
