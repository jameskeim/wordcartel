use std::borrow::Cow;
use std::ops::Range;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

// ---------------------------------------------------------------------------
// TextSource trait
// ---------------------------------------------------------------------------

/// Random-access view over the document text for block parsing.
/// Byte offsets are into the whole document. `slice` returns a CONTIGUOUS &str
/// (borrowed for &str sources, owned/materialized for ropes — O(slice len)).
pub trait TextSource {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn slice(&self, range: Range<usize>) -> Cow<'_, str>;
    /// Byte offset of the start of the line containing `pos` (just after the
    /// previous `\n`, or 0). `\n`-only semantics. `pos` is clamped to `len()`.
    fn line_start(&self, pos: usize) -> usize;
    /// Byte offset of the end of the line containing `pos` (the next `\n` + 1,
    /// or `len()`). `\n`-only semantics. `pos` is clamped to `len()`.
    fn line_end(&self, pos: usize) -> usize;
}

impl TextSource for &str {
    fn len(&self) -> usize {
        str::len(self)
    }

    fn slice(&self, range: Range<usize>) -> Cow<'_, str> {
        Cow::Borrowed(&self[range])
    }

    fn line_start(&self, pos: usize) -> usize {
        let pos = pos.min(self.len());
        match self.as_bytes()[..pos].iter().rposition(|&b| b == b'\n') {
            Some(nl) => nl + 1,
            None => 0,
        }
    }

    fn line_end(&self, pos: usize) -> usize {
        let pos = pos.min(self.len());
        match self.as_bytes()[pos..].iter().position(|&b| b == b'\n') {
            Some(off) => pos + off + 1,
            None => self.len(),
        }
    }
}

impl TextSource for &ropey::Rope {
    fn len(&self) -> usize {
        self.len_bytes()
    }

    fn slice(&self, range: Range<usize>) -> Cow<'_, str> {
        Cow::Owned(self.byte_slice(range).to_string())
    }

    /// Walk backward through rope chunks looking for the last `\n` before
    /// `pos`. Returns one past that `\n`, or 0 if none. LF-only semantics
    /// (does NOT use ropey's line APIs which treat many Unicode separators
    /// as line breaks).
    fn line_start(&self, pos: usize) -> usize {
        let pos = pos.min(self.len_bytes());
        // Special case: pos == 0 means we're at the very beginning.
        if pos == 0 {
            return 0;
        }
        // We scan backward from pos-1 through the rope's chunks.
        // chunk_at_byte(byte_idx) returns (chunk_str, chunk_byte_start, ..)
        // The chunk containing byte_idx is chunk[byte_idx - chunk_byte_start].
        let mut remaining = pos; // how many bytes from the start we still need to cover
        loop {
            // Get the chunk that contains byte index `remaining - 1`.
            let (chunk, chunk_start, _, _) = self.chunk_at_byte(remaining - 1);
            // How many bytes of this chunk are relevant? Only up to `remaining`
            // bytes from document start, so up to (remaining - chunk_start) bytes
            // into the chunk.
            let chunk_bytes = chunk.as_bytes();
            let within_chunk_end = remaining - chunk_start; // exclusive end within chunk
            // Search backward in chunk_bytes[..within_chunk_end] for '\n'.
            if let Some(local_nl) = chunk_bytes[..within_chunk_end]
                .iter()
                .rposition(|&b| b == b'\n')
            {
                // Found a '\n' at global byte offset chunk_start + local_nl.
                return chunk_start + local_nl + 1;
            }
            // No '\n' in this chunk's relevant portion. Continue to the chunk before.
            if chunk_start == 0 {
                // We've scanned back to the start of the rope — no '\n' found.
                return 0;
            }
            remaining = chunk_start;
        }
    }

    /// Walk forward through rope chunks looking for the first `\n` at or
    /// after `pos`. Returns that byte's index + 1, or `len_bytes()` if none.
    /// LF-only semantics (does NOT use ropey's line APIs).
    fn line_end(&self, pos: usize) -> usize {
        let total = self.len_bytes();
        let pos = pos.min(total);
        if pos == total {
            return total;
        }
        let mut offset = pos; // current search position in the rope
        loop {
            let (chunk, chunk_start, _, _) = self.chunk_at_byte(offset);
            let chunk_bytes = chunk.as_bytes();
            // Relevant portion of this chunk starts at (offset - chunk_start).
            let within_chunk_start = offset - chunk_start;
            if let Some(local_nl) = chunk_bytes[within_chunk_start..]
                .iter()
                .position(|&b| b == b'\n')
            {
                return chunk_start + within_chunk_start + local_nl + 1;
            }
            // No '\n' in this chunk. Advance to the next chunk.
            let next = chunk_start + chunk.len();
            if next >= total {
                return total;
            }
            offset = next;
        }
    }
}

/// Kinds of block we track. Inline-level tags are ignored; we only keep the
/// block skeleton, which is what the renderer's layout depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Document,
    Paragraph,
    Heading(u8),
    FencedCode,
    IndentedCode,
    BlockQuote,
    List,
    ListItem,
    ThematicBreak,
    HtmlBlock,
    HtmlComment,
    /// A leading YAML front-matter block (`---\n … \n---`) at byte 0 ONLY.
    FrontMatter,
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

    /// Return the `BlockRole` for the block at `byte`.
    ///
    /// Walks the tree recursively, collecting all blocks whose span contains
    /// `byte`, then reduces them by precedence (most-specific wins):
    ///   FencedCode | IndentedCode → CodeBlock
    ///   Heading(n)               → Heading(n)
    ///   ThematicBreak            → ThematicBreak
    ///   ListItem                 → ListItem
    ///   BlockQuote               → BlockQuote
    ///   anything else            → Paragraph
    ///
    /// Blocks are sparse — blank lines / gaps / bytes past EOF belong to no
    /// block, so `byte` in a gap returns `Paragraph` (the safe default).
    pub fn role_at(&self, byte: usize) -> crate::style::BlockRole {
        let mut best = crate::style::BlockRole::Paragraph;
        collect_role(&self.root, byte, &mut best);
        best
    }
}

/// Assign a numeric precedence to a `BlockRole` (lower = higher priority).
fn role_precedence(r: &crate::style::BlockRole) -> u8 {
    use crate::style::BlockRole::*;
    match r {
        CodeBlock      => 0,
        Heading(_)     => 1,
        Comment        => 2,
        ThematicBreak  => 2,
        ListItem       => 3,
        BlockQuote     => 4,
        // FrontMatter is always a top-level byte-0 leaf (it cannot nest), so a
        // rank just below Paragraph suffices for it to win `role_at` over the
        // default Paragraph. `collect_role` overrides only on strictly-lower
        // precedence, so FrontMatter==Paragraph would let Paragraph win.
        FrontMatter    => 4,
        Paragraph      => 5,
    }
}

/// Map a `BlockKind` to its `BlockRole` contribution (None = no upgrade).
fn kind_to_role(kind: &BlockKind) -> Option<crate::style::BlockRole> {
    use crate::style::BlockRole;
    match kind {
        BlockKind::FencedCode | BlockKind::IndentedCode => Some(BlockRole::CodeBlock),
        BlockKind::Heading(n) => Some(BlockRole::Heading(*n)),
        BlockKind::ThematicBreak => Some(BlockRole::ThematicBreak),
        BlockKind::ListItem => Some(BlockRole::ListItem),
        BlockKind::BlockQuote => Some(BlockRole::BlockQuote),
        BlockKind::HtmlComment => Some(BlockRole::Comment),
        BlockKind::FrontMatter => Some(BlockRole::FrontMatter),
        _ => None,
    }
}

/// Recursively walk `block` and its children; if `byte` is inside the block's
/// span, update `best` with the highest-precedence role found.
fn collect_role(block: &Block, byte: usize, best: &mut crate::style::BlockRole) {
    if !block.span.contains(&byte) {
        return;
    }
    // This block contains `byte` — consider its role.
    if let Some(role) = kind_to_role(&block.kind) {
        if role_precedence(&role) < role_precedence(best) {
            *best = role;
        }
    }
    // Children are in document order and non-overlapping, so at most one can
    // contain `byte`. `partition_point` finds the first child whose span ends
    // AFTER `byte` in O(log N); we recurse only if it also starts at/before
    // `byte` (i.e. it actually contains `byte`, not just succeeds it).
    let idx = block.children.partition_point(|c| c.span.end <= byte);
    if let Some(child) = block.children.get(idx).filter(|c| c.span.start <= byte) {
        collect_role(child, byte, best);
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
        Tag::Heading { level, .. } => BlockKind::Heading(*level as usize as u8),
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

/// Returns true if the `TagEnd` corresponds to a block-level tag that was
/// pushed onto the stack by `tag_to_kind`.  Inline `TagEnd` variants (Link,
/// Image, Emphasis, Strong, Strikethrough, Superscript, Subscript, and the
/// table-internal TableHead/TableRow/TableCell variants) must return `false`
/// here so that `Event::End` does not spuriously pop a block off the stack.
///
/// Invariant: `tag_end_is_block(tag_end)` iff `tag_to_kind(start_tag)` returned
/// `Some(_)` for the matching `Event::Start`.
fn tag_end_is_block(tag_end: &TagEnd) -> bool {
    matches!(
        tag_end,
        TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::CodeBlock
            | TagEnd::BlockQuote(_)
            | TagEnd::HtmlBlock
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::Table
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_)
    )
}

/// pulldown-cmark does not emit Start/End for thematic breaks; it emits a
/// standalone `Event::Rule`. We synthesize a leaf block for it.
fn is_rule(event: &Event) -> bool {
    matches!(event, Event::Rule)
}

/// THE ORACLE. Walk block-level events, building a nested tree with byte spans.
pub fn full_parse(text: &str) -> BlockTree {
    full_parse_src(&text)
}

/// A childless `Document`-root tree spanning `0..len`. The safe fallback when a
/// parse cannot run (M4-rest): it has NO child spans, so span-slicing consumers
/// (fold / outline / nav / transform) have nothing to slice out of range, and
/// `role_at` returns the default `Paragraph` everywhere.
pub fn empty_tree(len: usize) -> BlockTree {
    BlockTree { root: Block { kind: BlockKind::Document, span: 0..len, children: Vec::new() } }
}

/// Generic version of `full_parse` over any `TextSource`.
///
/// THE ONLY true whole-document entry point — `full_parse` and
/// `full_parse_rope` both route through here, and the incremental update path
/// calls this (and ONLY this) when an edit forces a reparse-from-byte-0. Byte-0
/// YAML front-matter detection lives HERE and NOWHERE ELSE: `parse_region` stays
/// front-matter-blind so that the incremental splice — which calls
/// `parse_region` on a localized FRAGMENT whose base offset can be 0 — never runs
/// the byte-0 scanner against a non-document slice (the C3 splice hazard).
pub fn full_parse_src<S: TextSource>(src: &S) -> BlockTree {
    // Byte-0 front matter is detected on the whole-document text only.
    let whole = src.slice(0..src.len());
    if let Some(fm) = front_matter_span(whole.as_ref()) {
        // Emit the FrontMatter block first (span 0..fm.end), then parse the
        // REMAINDER `&src[fm.end..]` with base offset `fm.end` so its spans are
        // shifted into document coordinates, and append those blocks after it.
        let mut root = Block {
            kind: BlockKind::Document,
            span: 0..src.len(),
            children: vec![Block {
                kind: BlockKind::FrontMatter,
                span: fm.clone(),
                children: Vec::new(),
            }],
        };
        let remainder = parse_region(src, fm.end..src.len(), fm.end);
        root.children.extend(remainder.root.children);
        return BlockTree { root };
    }
    parse_region(src, 0..src.len(), 0)
}

/// If `src` begins with a YAML front-matter block (`---\n … \n---`), return its
/// byte range (the whole block incl. both fences); else `None`. Byte-0 ONLY:
/// the opening fence MUST sit at byte 0, so a mid-document `---` is never front
/// matter (`strip_prefix` fails). The closing fence is the first line that is
/// exactly `---` or `...`.
fn front_matter_span(src: &str) -> Option<Range<usize>> {
    let rest = src.strip_prefix("---\n")?; // opening fence MUST be at byte 0
    let mut off = 4; // bytes consumed by the opening "---\n"
    for line in rest.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if trimmed == "---" || trimmed == "..." {
            return Some(0..off + line.len());
        }
        off += line.len();
    }
    None
}

/// Parse the byte range `region` of `src`, treating the region as living at
/// byte offset `base` in some larger document.  All spans are shifted by
/// `base`.  The `Cow<str>` returned by `src.slice(region)` is bound to a
/// local variable so it outlives the pulldown-cmark parser borrow.
fn parse_region<S: TextSource>(src: &S, region: Range<usize>, base: usize) -> BlockTree {
    let text = src.slice(region);
    let parser = Parser::new_ext(text.as_ref(), options());

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
                    // Discriminate HTML comment blocks from generic HTML blocks:
                    // `text` is the region-local slice, `range` is region-local,
                    // so `text[range.clone()]` correctly indexes the region text.
                    let kind = if kind == BlockKind::HtmlBlock
                        && text[range.clone()].trim_start().starts_with("<!--")
                    {
                        BlockKind::HtmlComment
                    } else {
                        kind
                    };
                    stack.push(Block { kind, span, children: Vec::new() });
                }
            }
            Event::End(ref tag_end) => {
                // Only pop when the matching Start actually pushed a block.
                // Inline tags (Link, Image, Emphasis, etc.) return None from
                // tag_to_kind and never push onto the stack, so their End
                // events must be ignored here.  Without this guard, an
                // End(Link) inside a Paragraph would spuriously pop the
                // Paragraph off the stack, corrupting the tree structure.
                if tag_end_is_block(tag_end) {
                    if let Some(done) = stack.pop() {
                        push_child(&mut root, &mut stack, done);
                    }
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

fn push_child(root: &mut Block, stack: &mut [Block], done: Block) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(done),
        None => root.children.push(done),
    }
}

// ---------------------------------------------------------------------------
// Edit model
// ---------------------------------------------------------------------------

/// An edit replaces `range` of the OLD text with `new_len` new bytes.
#[derive(Debug, Clone)]
pub struct Edit {
    /// Byte range in the OLD text that was replaced.
    pub range: Range<usize>,
    /// Number of bytes the replacement occupies in the NEW text.
    pub new_len: usize,
}

impl Edit {
    pub fn delta(&self) -> isize {
        self.new_len as isize - self.range.len() as isize
    }
}

// ---------------------------------------------------------------------------
// Incremental update
// ---------------------------------------------------------------------------

/// Reason the update widened (for instrumentation in tests/benches).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidenReason {
    /// Reparsed only the enclosing top-level block(s) (+1 slack block).
    Local,
    /// Could not localize the edit; full reparse.
    NoOverlapFull,
    /// A hard trigger forced reparsing to end-of-document.
    WidenToEnd,
    /// The widen EXTENSION to EOF would exceed `MAX_SYNC_WIDEN_BYTES` while the base
    /// local region is small (Case A). Reparsed only the base local region and left the
    /// container-wide effect (loose/tight, absorb-to-EOF) STALE; `derive` marks it
    /// `maybe_stale` and the debounced reconcile converges it to `full_parse` at rest.
    BoundedStale,
}

/// Result + instrumentation.
pub struct UpdateOutcome {
    pub tree: BlockTree,
    pub reason: WidenReason,
    /// Number of bytes actually reparsed (the slice length).
    pub reparsed_bytes: usize,
}

/// Synchronous-reparse ceiling for the widen path (F1). When a widen's speculative
/// extension to end-of-document would exceed this many bytes but the base local region
/// is smaller, the reparse is bounded to the base region and tagged `BoundedStale`
/// (the container-wide effect is deferred to the reconcile). A named tunable — validate
/// the real per-MiB `parse_region` cost and lower if a bounded parse exceeds a frame.
pub const MAX_SYNC_WIDEN_BYTES: usize = 1 << 20; // 1 MiB

/// Incrementally update `old_tree` for `edit`, producing the new tree.
/// `&str` wrapper — unchanged public signature; existing callers and the oracle
/// rely on this.
pub fn incremental_update(
    old_tree: &BlockTree,
    old_text: &str,
    edit: &Edit,
    new_text: &str,
) -> BlockTree {
    incremental_update_src(old_tree, &old_text, edit, &new_text)
}

/// As `incremental_update` but returns instrumentation about how wide we went.
/// `&str` wrapper — unchanged public signature; existing callers and the oracle
/// rely on this.
pub fn incremental_update_instrumented(
    old_tree: &BlockTree,
    old_text: &str,
    edit: &Edit,
    new_text: &str,
) -> UpdateOutcome {
    incremental_update_instrumented_src(old_tree, &old_text, edit, &new_text)
}

/// Rope entry point: full parse from a `ropey::Rope`.
pub fn full_parse_rope(rope: &ropey::Rope) -> BlockTree {
    full_parse_src(&rope)
}

/// Rope entry point: incremental update from `ropey::Rope` snapshots.
pub fn incremental_update_rope(
    old_tree: &BlockTree,
    old_rope: &ropey::Rope,
    edit: &Edit,
    new_rope: &ropey::Rope,
) -> BlockTree {
    incremental_update_src(old_tree, &old_rope, edit, &new_rope)
}

/// Generic `incremental_update` over any `TextSource`.
pub fn incremental_update_src<S: TextSource>(
    old_tree: &BlockTree,
    old_src: &S,
    edit: &Edit,
    new_src: &S,
) -> BlockTree {
    incremental_update_instrumented_src(old_tree, old_src, edit, new_src).tree
}

/// The Local-path region extension: `+1` slack block, plus (for gap edits) the upstream
/// blank-delimited-group pull-back. Mutates the candidate region `[start, end)` in place.
/// Extracted from the `else` branch of the widen decision so the F1 copy-prediction can
/// apply the SAME rule to a copy.
fn apply_local_slack<S: TextSource>(
    tops: &[Block],
    old_src: &S,
    start: &mut usize,
    end: &mut usize,
    slack_pos: Option<usize>,
    have_overlap: bool,
) {
    if let Some(slack_idx) = slack_pos {
        if slack_idx + 1 < tops.len() {
            *end = tops[slack_idx + 1].span.start;
        } else {
            *end = old_src.len();
        }
    }
    if !have_overlap {
        if let Some(b) = tops.iter().rev().find(|b| b.span.end <= *start) {
            *start = blank_delimited_group_start(old_src, old_src.line_start(b.span.start));
        }
    }
}

/// Straddle repair + trailing-gap coverage on `[start, end)`. Shared by every path;
/// idempotent (a region already repaired grows no further). Extracted from the code
/// after the widen decision.
fn repair_region<S: TextSource>(tops: &[Block], old_src: &S, start: &mut usize, end: &mut usize) {
    loop {
        let mut grew = false;
        for b in tops.iter() {
            if b.span.start < *start && b.span.end > *start {
                *start = old_src.line_start(b.span.start);
                grew = true;
            }
            if b.span.start < *end && b.span.end > *end {
                *end = old_src.line_end(b.span.end);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let has_after_block = tops.iter().any(|b| b.span.start >= *end && b.span.end > *end);
    if !has_after_block && *end < old_src.len() {
        *end = old_src.len();
    }
}

/// Generic `incremental_update_instrumented` over any `TextSource`.
pub fn incremental_update_instrumented_src<S: TextSource>(
    old_tree: &BlockTree,
    old_src: &S,
    edit: &Edit,
    new_src: &S,
) -> UpdateOutcome {
    let delta = edit.delta();
    let tops = old_tree.top_level();

    let edit_lo = edit.range.start;
    let edit_hi = edit.range.end;

    // Find first/last top-level block overlapping the edit (boundary-inclusive).
    let mut first = tops.len();
    let mut last = 0usize;
    for (i, b) in tops.iter().enumerate() {
        if b.span.end >= edit_lo && b.span.start <= edit_hi {
            first = first.min(i);
            last = last.max(i);
        }
    }

    // The edit may fall partly or wholly in UNBLOCKED gaps: blank lines between
    // blocks, and — critically — link-reference-definition lines, which produce
    // NO block events in pulldown-cmark. We must still build a region that
    // *encloses* the edit. Compute start/end from blocks where they overlap,
    // but clamp outward to the edit endpoints and to line boundaries.
    let have_overlap = first <= last && first < tops.len();
    let (mut region_old_start, mut region_old_end) = if have_overlap {
        (tops[first].span.start, tops[last].span.end)
    } else {
        // No overlapping block: the edit is entirely in a gap. Anchor on the
        // edit itself; the line-boundary snap + widen logic below makes it safe.
        (edit_lo, edit_hi)
    };
    // Ensure enclosure even when the touched block doesn't reach the edit.
    region_old_start = region_old_start.min(edit_lo);
    region_old_end = region_old_end.max(edit_hi);
    // Snap to line boundaries so we never cut a construct mid-line.
    region_old_start = old_src.line_start(region_old_start);
    region_old_end = old_src.line_end(region_old_end);

    // UPSTREAM LINE-GROUP CONTEXT: how a line parses depends on the lines
    // immediately above it within the same blank-line-delimited group: a ref
    // def line changes whether the next line continues a paragraph; a setext
    // underline needs the line above; a `-` after a paragraph is a setext
    // underline, but after a blank line is a list bullet. So pull the region
    // start back to the most recent BLANK line (start of the current line
    // group). This guarantees the reparse sees the whole group.
    region_old_start = blank_delimited_group_start(old_src, region_old_start);
    // The group walk can cross a blank line that lives *inside* a fenced or
    // indented code block (where blanks are content, not delimiters), landing
    // mid-block. Snap back to the start of any top-level block it landed inside.
    for b in tops.iter() {
        if b.span.start < region_old_start && b.span.end > region_old_start {
            region_old_start = old_src.line_start(b.span.start);
        }
    }

    // CONTAINER LAZY-CONTINUATION (upstream): a List or BlockQuote can absorb
    // following lines (lazy continuation, loose-list blank lines, trailing
    // ref-def lines). An edit on lines after a container — even with blank /
    // ref-def lines in between — can RE-EXTEND that container. So if the
    // nearest top-level block *starting before* region_old_start is a
    // container, and only blank-or-ref-def lines separate it from the region,
    // pull the region start back to include the whole container.
    loop {
        // nearest preceding block by start position
        let prev = tops
            .iter()
            .filter(|b| b.span.start < region_old_start)
            .max_by_key(|b| b.span.start);
        if let Some(b) = prev {
            // Constructs whose extent can grow downstream across blank lines:
            //   - List / BlockQuote: lazy continuation, loose-list blanks
            //   - IndentedCode: blank-line-separated indented code merges into
            //     one block
            let absorptive = matches!(
                b.kind,
                BlockKind::List | BlockKind::BlockQuote | BlockKind::IndentedCode
            );
            // Only materialize/scan the gap when absorption is actually in play.
            // Hoisting this behind the `absorptive` guard keeps the local hot
            // path O(region): a non-absorptive preceding block (the common case)
            // never reads the gap, which can otherwise be O(document) when a
            // large blank span sits upstream of the edit.
            if absorptive && b.span.end <= region_old_start {
                // gap between block end and region start must be only blank /
                // ref-def lines (no other block intervenes by construction,
                // since this is the nearest preceding block).
                let gap_lo = b.span.end.min(old_src.len());
                let gap_hi = region_old_start.min(old_src.len());
                let gap = old_src.slice(gap_lo.min(gap_hi)..gap_hi);
                let gap_is_soft = gap
                    .as_ref()
                    .lines()
                    .all(|l| l.trim().is_empty() || is_ref_def_line(l));
                if gap_is_soft {
                    region_old_start = old_src.line_start(b.span.start);
                    continue;
                }
            }
        }
        break;
    }

    // HTML blocks can change where a *preceding* block ends, so a downstream-
    // only widen is not enough — fall back to a full reparse. This is cheap to
    // detect and HTML in prose is rare (see report).
    //
    // Bare CR (a `\r` not part of `\r\n`) is folded into the SAME fallback: it is a
    // line break to pulldown-cmark but invisible to the LF-only region/line
    // machinery here, so it can start a block (HTML, heading, fence) at a position
    // the incremental logic treats as mid-line — and that block can then mis-end,
    // desyncing the localized reparse from a full parse. Bare CR is vanishingly
    // rare in real Markdown, so a full reparse when the edited region contains one
    // is the simplest provably-correct treatment (CRLF is handled fine and is NOT
    // flagged).
    if html_in_play(old_src, new_src, edit, region_old_start, region_old_end)
        || region_has_bare_cr(old_src, new_src, edit, region_old_start, region_old_end)
    {
        let tree = full_parse_src(new_src);
        return UpdateOutcome { tree, reason: WidenReason::NoOverlapFull, reparsed_bytes: new_src.len() };
    }

    // Byte-0 YAML front matter is detected ONLY by the whole-document entry
    // (`full_parse_src`); the localized region reparse below is front-matter-
    // blind (it would parse the `---` fences as thematic breaks — the C3 splice
    // hazard). So any edit that could create, destroy, or resize a leading `---`
    // block must route to a real reparse-from-byte-0 (the SAME mechanism the
    // `html_in_play` branch above uses).
    //
    // BUT a body edit in a STABLE front-matter doc leaves the head block 0..E
    // provably untouched, and forcing a full reparse there is the dominant cost
    // on every keystroke (the head is tiny but the body can be megabytes). So we
    // tighten: confirm front matter is present with the SAME extent E on both
    // sides (a bounded head scan, never materializing the whole doc) and that
    // the edit starts at/after E (strictly in the body). When that holds, the FM
    // block is untouched and we take the localized incremental path with the
    // region FLOORED at E (enforced LATE — see below). In every other case
    // (FM created / destroyed / resized, edit touches the head, or an
    // over-cap/unconfirmed head) we keep today's conservative full reparse.
    let fm_floor: Option<usize> = if starts_with_fm_fence(old_src) || starts_with_fm_fence(new_src) {
        let old_e = fm_end_capped(old_src);
        let new_e = fm_end_capped(new_src);
        match (old_e, new_e) {
            // FM present on BOTH sides, identical extent, and the edit is
            // strictly in the BODY (at/after the FM end): the FrontMatter block
            // 0..E is provably unchanged and untouched -> incremental, floored at E.
            (Some(oe), Some(ne)) if oe == ne && edit_lo >= oe => Some(oe),
            // Anything else: FM created / destroyed / resized, edit touches the
            // head, or an unconfirmed (over-cap) head -> full reparse.
            _ => {
                let tree = full_parse_src(new_src);
                return UpdateOutcome {
                    tree,
                    reason: WidenReason::NoOverlapFull,
                    reparsed_bytes: new_src.len(),
                };
            }
        }
    } else {
        // No `---` head on either side: not a front-matter concern at all; the
        // normal incremental machinery below handles it.
        None
    };

    // DOWNSTREAM ABSORPTION: editing inside / abutting a List, BlockQuote or
    // IndentedCode can change indentation/looseness such that *following* top-
    // level blocks get pulled in (indented code, paragraphs, sub-lists), or
    // (indented code) merge across blank lines. Localizing the new extent is
    // the hard part of incremental Markdown. Conservative rule: if the region
    // overlaps such a block, widen to end of document. (Combined with the
    // upstream pull-back above, these edits are then always safe.)
    //
    // We also widen when the FIRST BLOCK AFTER the region (the "slack" block)
    // is absorptive. The slack block's outer span can include trailing blank
    // lines that are part of its structural context — those blank lines fall
    // in the gap between the slack block's span.end and the next block's
    // span.start, and cannot be reliably included in the reparse region
    // without also including the following block's content. Widening to end
    // is the only correct treatment.
    let slack_pos = tops.iter().position(|b| b.span.start >= region_old_end);
    let slack_block = slack_pos.map(|i| &tops[i]);
    let slack_is_absorptive = slack_block.is_some_and(|b| {
        matches!(
            b.kind,
            BlockKind::List | BlockKind::BlockQuote | BlockKind::IndentedCode
        )
    });
    let absorptive_in_region = tops.iter().any(|b| {
        matches!(
            b.kind,
            BlockKind::List | BlockKind::BlockQuote | BlockKind::IndentedCode
        ) && b.span.start < region_old_end
            && b.span.end > region_old_start
    });
    // FORWARD/DOWNSTREAM CONTAINER MERGE: the safe region's downstream end can
    // land exactly at the span.start of a top-level container (Table, List,
    // BlockQuote) that the edit causes to merge backward into the reparsed
    // region.  That container is then shifted verbatim (stale structure) instead
    // of reparsed.  The existing absorptive gate only inspects the in-region
    // blocks and the slack block, missing:
    //   (a) Table — not in the absorptive set at all (CE1).
    //   (b) The block immediately AFTER the slack block when the slack block
    //       itself is non-absorptive (e.g. a Paragraph) but the block past it
    //       is a List or Table (CE2 / CE1 combined).
    // Fix: also widen-to-full when the slack block OR the block immediately
    // following the slack block is a container (List | ListItem | Table |
    // BlockQuote).  Full reparse is trivially correct (ground truth), so there
    // are no false-negatives.  Plain-prose edits far from any container are
    // unaffected (they still take the Local fast path).
    let post_slack_block = slack_pos.and_then(|i| tops.get(i + 1));
    let is_downstream_container = |b: &Block| {
        matches!(
            b.kind,
            BlockKind::List | BlockKind::ListItem | BlockKind::Table | BlockKind::BlockQuote
        )
    };
    let downstream_container_merge = slack_block.is_some_and(is_downstream_container)
        || post_slack_block.is_some_and(is_downstream_container);

    let widen = absorptive_in_region
        || slack_is_absorptive
        || downstream_container_merge
        || needs_widen_to_end(old_src, new_src, edit, region_old_start, region_old_end);
    let mut reason;
    if widen {
        // F1: predict the base local region on COPIES (what the Local path would produce),
        // then bound the widen only when the EXTENSION is the expense (Case A). The copies
        // apply the same slack/pull-back + straddle/trailing-gap the Local path would; fm_floor
        // is deliberately NOT applied to the copies, and widen_span uses the un-floored
        // region_old_start — both conservative: they can only OVER-count (choose BoundedStale
        // where a floored WidenToEnd would have been ≤ cap), never under-count. A BoundedStale
        // that "should" have been a cheap WidenToEnd is still correct (valid tree, converged at
        // rest), so this is safe; do not "fix" it by flooring the copies.
        let mut base_start = region_old_start;
        let mut base_end = region_old_end;
        apply_local_slack(tops, old_src, &mut base_start, &mut base_end, slack_pos, have_overlap);
        repair_region(tops, old_src, &mut base_start, &mut base_end);
        // Size in NEW-text bytes — `reparsed_bytes` is `new_region.len()` AFTER `delta` (Codex):
        // an insertion can push the actual reparse over the cap even when the old base fits.
        let base_new_end = (base_end as isize + delta) as usize;
        let base_region_size = base_new_end.saturating_sub(base_start);
        let widen_span = new_src.len().saturating_sub(region_old_start);
        if widen_span <= MAX_SYNC_WIDEN_BYTES {
            // cheap extension → widen fully, exactly as today
            region_old_end = old_src.len();
            reason = WidenReason::WidenToEnd;
        } else if base_region_size <= MAX_SYNC_WIDEN_BYTES {
            // Case A: expensive extension, small base → install the copied base bounds, defer
            region_old_start = base_start;
            region_old_end = base_end;
            reason = WidenReason::BoundedStale;
        } else {
            // Case B: the base local region itself exceeds the cap → widen as today
            region_old_end = old_src.len();
            reason = WidenReason::WidenToEnd;
        }
    } else {
        // +1 slack block + (gap-edit) upstream pull-back — see `apply_local_slack`.
        apply_local_slack(tops, old_src, &mut region_old_start, &mut region_old_end, slack_pos, have_overlap);
        reason = WidenReason::Local;
    }

    // Straddle repair + trailing-gap coverage — see `repair_region`.
    repair_region(tops, old_src, &mut region_old_start, &mut region_old_end);

    // Gap 3: machine-check the trailing-gap bound. `region_old_end` is now
    // final; verify it never exceeds the document length before we use it to
    // compute region_new_end and drive the splice.
    debug_assert!(
        region_old_end <= old_src.len(),
        "region_old_end {} past doc len {}",
        region_old_end,
        old_src.len()
    );

    debug_assert!(region_old_start <= edit_lo);
    debug_assert!(region_old_end >= edit_hi);

    // FRONT-MATTER FLOOR: the FrontMatter block 0..E is unchanged and untouched
    // (gated above: FM present with identical extent E on both sides, edit_lo >=
    // E). `parse_region` is front-matter-BLIND, so the localized reparse must
    // never reach the `---` fences. Clamp here, AFTER all widening/straddle
    // repair, so no upstream pull-back can re-expose them — in particular the
    // `!have_overlap` rev-find above would otherwise yank `region_old_start`
    // back onto the FrontMatter(0..E) block (whose `span.end <= region_old_start`
    // when the edit is in a gap), dragging the region to 0 and feeding the
    // FM-blind parser a non-document slice. E is a clean line boundary (the byte
    // after the closing fence's newline) and every body block starts at/after E,
    // so clamping introduces no straddle: FrontMatter(0..E) stays a verbatim
    // "before" block and the reparse sees only body bytes >= E.
    if let Some(floor) = fm_floor {
        region_old_start = region_old_start.max(floor);
    }
    // Holds: the gate guarantees floor E <= edit_lo, so the max never lifts
    // region_old_start past edit_lo.
    debug_assert!(region_old_start <= edit_lo);

    let region_new_start = region_old_start;
    let mut region_new_end = (region_old_end as isize + delta) as usize;

    // Materialize only the edited region from new_src (O(region), not O(doc)).
    let mut new_region = new_src.slice(region_new_start..region_new_end);
    let mut reparsed = parse_region(&new_region.as_ref(), 0..new_region.len(), region_new_start);

    // CREATED-CONTAINER GROWTH: the absorptive-widen gate above only inspects OLD-
    // tree blocks, so an edit that CREATES an absorptive container (List, BlockQuote,
    // IndentedCode — each can absorb following lines) is not caught there. If the
    // reparse's LAST block is such a container and it reached the region boundary
    // (its span runs to region_new_end), it may keep absorbing content that lives
    // PAST the region — the localized reparse cut it short, mis-attributing its tail
    // (e.g. a list item's extent). Document still remaining after the region is the
    // tell; widen to end and reparse once. (When we already widened, region_old_end
    // == len, so this never fires twice.)
    if region_old_end < old_src.len() {
        let tail_absorptive = reparsed.root.children.last().is_some_and(|last| {
            matches!(
                last.kind,
                BlockKind::List | BlockKind::BlockQuote | BlockKind::IndentedCode
            ) && last.span.end >= region_new_end
        });
        if tail_absorptive {
            // F1: gate the second widen the same three-way way as the first.
            let widen_span = new_src.len().saturating_sub(region_new_start);
            let base_new_size = region_new_end.saturating_sub(region_new_start);
            if widen_span <= MAX_SYNC_WIDEN_BYTES || base_new_size > MAX_SYNC_WIDEN_BYTES {
                // cheap extension, or Case B (base already > cap) → extend to EOF, as today
                region_old_end = old_src.len();
                region_new_end = (region_old_end as isize + delta) as usize;
                new_region = new_src.slice(region_new_start..region_new_end);
                reparsed = parse_region(&new_region.as_ref(), 0..new_region.len(), region_new_start);
                reason = WidenReason::WidenToEnd;
            } else {
                // Case A: keep the already-parsed base region (no second parse), defer.
                reason = WidenReason::BoundedStale;
            }
        }
    }
    let reparsed_bytes = new_region.len();

    // Splice driven purely by the final region bounds, so it stays consistent
    // regardless of how the region was widened/snapped.
    //   - "before" blocks: entirely before region_old_start (end <= start).
    //   - "after" blocks: entirely after region_old_end (start >= end).
    //   - anything overlapping the region is replaced by the reparsed blocks.
    let mut result_children: Vec<Block> = Vec::new();
    for b in tops.iter() {
        if b.span.end <= region_old_start {
            result_children.push(b.clone());
        }
    }
    let before_count = result_children.len();
    result_children.extend(reparsed.root.children.iter().cloned());
    let after_seam = result_children.len(); // index of the first "after" block, once pushed
    for b in tops.iter() {
        // "after" blocks lie STRICTLY beyond region_old_end. The extra
        // `span.end > region_old_end` guard excludes a zero-length block sitting
        // exactly at region_old_end (e.g. the synthetic trailing empty Paragraph
        // pulldown-cmark emits at end-of-document after a link-reference def):
        // its span.end == region_old_end means it is already COVERED by the region
        // reparse above, so shifting it here too would emit it twice. Straddle
        // repair guarantees no block crosses region_old_end, so this only ever
        // filters out such zero-length boundary blocks.
        if b.span.start >= region_old_end && b.span.end > region_old_end {
            result_children.push(shift_block(b, delta));
        }
    }

    // SEAM CONSISTENCY: the splice stitches verbatim "before"/"after" blocks onto
    // the freshly reparsed region. At each seam an edit can leave a top-level
    // Paragraph adjacent to a following block that a full parse would FOLD INTO that
    // paragraph (another Paragraph it merges with, or an indented line it absorbs as
    // lazy continuation — neither can interrupt a paragraph) with NO blank line
    // between them. A full parse never produces that arrangement; when the local
    // splice does, recover with a provably-correct full reparse. Only the two splice
    // seams (before|reparse, reparse|after) can introduce it, so this is O(1) —
    // never a document scan. (Both seams collapse to one index when the reparse
    // emitted no blocks.)
    let merge_at = |i: usize| {
        i > 0
            && i < result_children.len()
            && paragraph_absorbs_next(new_src, &result_children[i - 1], &result_children[i])
    };
    if merge_at(before_count) || merge_at(after_seam) {
        let tree = full_parse_src(new_src);
        return UpdateOutcome {
            tree,
            reason: WidenReason::NoOverlapFull,
            reparsed_bytes: new_src.len(),
        };
    }

    let root = Block { kind: BlockKind::Document, span: 0..new_src.len(), children: result_children };
    UpdateOutcome { tree: BlockTree { root }, reason, reparsed_bytes }
}

/// Splice consistency guard: in a full parse, would top-level block `a` (a
/// Paragraph) FOLD IN the immediately following block `b`? A paragraph runs over
/// subsequent lines until a blank line or a paragraph-INTERRUPTING block. `b` is
/// non-interrupting when it is itself a `Paragraph` (they merge into one) or
/// `IndentedCode` (an indented line cannot interrupt a paragraph — it is absorbed
/// as lazy continuation). So if `a` is a Paragraph, `b` is one of those kinds, and
/// the inter-block gap `[a.end, b.start)` in `new_src` holds no blank line (a
/// complete line of only spaces/tabs ending in `\n` or `\r\n`) separating them, a
/// correct full parse would make them ONE paragraph. The
/// localized splice can manufacture this illegal adjacency at a region seam when an
/// edit removes the separation (e.g. turns a blank line into a continuation line,
/// or drops an upstream paragraph-interrupting construct). Detecting it lets the
/// caller fall back to a provably-correct full reparse. O(gap) — the gap between
/// adjacent top-level blocks is whitespace/newlines, never a block interior.
fn paragraph_absorbs_next<S: TextSource>(new_src: &S, a: &Block, b: &Block) -> bool {
    if a.kind != BlockKind::Paragraph
        || !matches!(b.kind, BlockKind::Paragraph | BlockKind::IndentedCode)
    {
        return false;
    }
    let lo = a.span.end.min(new_src.len());
    let hi = b.span.start.min(new_src.len());
    if lo >= hi {
        return true; // directly abutting: no separating line at all
    }
    // They fold into one paragraph UNLESS a markdown BLANK line separates them. A
    // blank line is a COMPLETE line (terminated by a line ending) containing ONLY
    // spaces and tabs — not every '\n' (a `\n` can end a non-blank continuation
    // line), and not lines holding other "whitespace" such as the vertical tab
    // \u{0b} or form feed, which CommonMark does NOT treat as blank. CommonMark
    // treats `\r\n`, `\r`, and `\n` as line endings, so a CRLF blank line ("\r\n")
    // must read as blank too — strip an optional trailing '\r' before the
    // space/tab check. So a lone leading-indent run with no newline (lazy
    // continuation) and a " \u{0b}\n" line both correctly read as "no blank line"
    // -> merge, while a "\r\n" gap correctly reads as a blank line -> no merge.
    let gap = new_src.slice(lo..hi);
    let has_blank_line = gap.as_ref().split_inclusive('\n').any(|seg| {
        if !seg.ends_with('\n') { return false; }
        // Strip the line ending: the '\n', then an optional '\r' before it (CRLF).
        // Only '\r' — a vertical tab / form feed before the '\n' is NOT blank
        // (CommonMark), so the fix-#6 nuance is preserved.
        let line = &seg[..seg.len() - 1];
        let line = line.strip_suffix('\r').unwrap_or(line);
        line.bytes().all(|c| c == b' ' || c == b'\t')
    });
    !has_blank_line
}

/// Conservative triggers that force reparsing to end-of-document.
fn needs_widen_to_end<S: TextSource>(
    old_src: &S,
    new_src: &S,
    edit: &Edit,
    region_old_start: usize,
    region_old_end: usize,
) -> bool {
    let os = region_old_start.min(old_src.len());
    let oe = region_old_end.min(old_src.len());
    let old_region = old_src.slice(os.min(oe)..oe);
    let new_start = region_old_start.min(new_src.len());
    let new_region_end = ((region_old_end as isize + edit.delta()) as usize).min(new_src.len());
    let new_region = new_src.slice(new_start.min(new_region_end)..new_region_end);

    // (a) Link reference definitions are resolved document-wide.
    if contains_ref_def(old_region.as_ref()) || contains_ref_def(new_region.as_ref()) {
        return true;
    }
    // (b) Fence structure is fragile: editing ANY fence-marker line can flip a
    //     close into an opener (e.g. "```" -> "```> "), leaving the fence open
    //     and swallowing the rest of the document — or vice versa. The
    //     marker-COUNT is not enough (the line still *starts* with backticks),
    //     so we widen whenever the edit intersects a fence-marker line in
    //     either the old or new text, or the marker count changes.
    if fence_marker_count(old_region.as_ref()) != fence_marker_count(new_region.as_ref()) {
        return true;
    }
    let new_edit_start = edit.range.start;
    let new_edit_end = edit.range.start + edit.new_len;
    if edit_touches_fence_line(old_src, edit.range.start, edit.range.end)
        || edit_touches_fence_line(new_src, new_edit_start, new_edit_end)
    {
        return true;
    }
    false
}

/// HTML blocks have 7 types with different termination rules, and an edit can
/// change where a *preceding* block ends (paragraph interruption / merging
/// across the HTML boundary). Localizing this cheaply proved intractable in the
/// spike (see report). Conservative, provably-safe rule: if either the old or
/// new region contains any line starting with '<', fall back to a full reparse.
fn html_in_play<S: TextSource>(
    old_src: &S,
    new_src: &S,
    edit: &Edit,
    region_old_start: usize,
    region_old_end: usize,
) -> bool {
    let os = region_old_start.min(old_src.len());
    let oe = region_old_end.min(old_src.len());
    let old_region = old_src.slice(os.min(oe)..oe);
    let new_start = region_old_start.min(new_src.len());
    let new_region_end = ((region_old_end as isize + edit.delta()) as usize).min(new_src.len());
    let new_region = new_src.slice(new_start.min(new_region_end)..new_region_end);
    html_opener_count(old_region.as_ref()) > 0 || html_opener_count(new_region.as_ref()) > 0
}

/// True if the edited region (old or new) contains a BARE carriage return — a
/// `\r` not immediately followed by `\n`. pulldown-cmark treats bare CR (and CR)
/// as line breaks, but every line/region helper here is LF-only (`str::lines`,
/// `line_start`, `line_end`), so a bare CR hides a line boundary that the parser
/// honors, desyncing the localized reparse. Callers route such edits to a full
/// reparse. `\r\n` is NOT flagged: the LF machinery already handles it (`lines`
/// strips the trailing `\r`, and `line_start`/`line_end` key on the `\n`).
fn region_has_bare_cr<S: TextSource>(
    old_src: &S,
    new_src: &S,
    edit: &Edit,
    region_old_start: usize,
    region_old_end: usize,
) -> bool {
    let os = region_old_start.min(old_src.len());
    let oe = region_old_end.min(old_src.len());
    let old_region = old_src.slice(os.min(oe)..oe);
    let new_start = region_old_start.min(new_src.len());
    let new_region_end = ((region_old_end as isize + edit.delta()) as usize).min(new_src.len());
    let new_region = new_src.slice(new_start.min(new_region_end)..new_region_end);
    has_bare_cr(old_region.as_ref()) || has_bare_cr(new_region.as_ref())
}

/// A `\r` not part of a `\r\n` pair (handles a trailing `\r` at end of slice as
/// bare — erring toward the safe full-reparse path).
fn has_bare_cr(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter()
        .enumerate()
        .any(|(i, &c)| c == b'\r' && b.get(i + 1) != Some(&b'\n'))
}

/// Maximum bytes scanned from the document head when confirming front-matter
/// extent. Front matter is a tiny head region; a `---\n` head with no closing
/// fence within this cap is treated as UNCONFIRMED (=> conservative full
/// reparse), which keeps the FM check O(cap) rather than O(document) even on a
/// pathological never-closed head.
const FM_HEAD_CAP: usize = 8192;

/// If `src` begins with a COMPLETE front-matter block whose closing fence falls
/// within `FM_HEAD_CAP` bytes, return its end offset `E` (one past the closing
/// fence's newline); else `None`.
///
/// This is the bounded, `TextSource`-generic counterpart to `front_matter_span`
/// used by the incremental hot path: it scans at most `FM_HEAD_CAP` bytes of the
/// head, so it never materializes the whole (possibly megabyte) document just to
/// answer "is there front matter, and where does it end?". An over-cap head
/// (opening fence present but no closing fence within the cap) returns `None`,
/// which the caller treats conservatively as "not confirmed -> full reparse".
///
/// CAUTION — truncation can manufacture a FALSE close: `front_matter_span` uses
/// `split_inclusive('\n')`, so on a capped slice the final fragment may be a
/// partial line with no terminating `\n`. If that partial fragment equals `---`
/// or `...` exactly, it is accepted as a closing fence, but the WHOLE-document
/// scan sees the line continues (e.g. `---more\n`) and finds the real first
/// close elsewhere. The false close always ends at exactly `cap`. `fm_end_capped`
/// guards against this by rejecting any result where `end == cap` on a truncated
/// head (see inline comment). When no false close is present, the capped scan
/// agrees with the whole-document scan because `front_matter_span` matches the
/// FIRST closing-fence line; the cap only turns a far-from-head or absent close
/// into `None` (conservative full reparse).
fn fm_end_capped<S: TextSource>(src: &S) -> Option<usize> {
    if !starts_with_fm_fence(src) {
        return None;
    }
    let cap = src.len().min(FM_HEAD_CAP);
    // Bind the slice to a local so the borrowed `&str` outlives the call.
    let head = src.slice(0..cap);
    let end = front_matter_span(head.as_ref()).map(|r| r.end)?;
    // GUARD against a TRUNCATED false close. `front_matter_span` matches the
    // closing fence via `split_inclusive('\n')`; over a capped slice the FINAL
    // fragment may be a partial line with no terminating `\n` (the cap cut it
    // off). If that partial fragment happens to equal `---`/`...`, the capped
    // scan reports a close that the WHOLE-document scan does NOT see (the real
    // line continues, e.g. `---more\n`). That false close ALWAYS ends exactly at
    // `cap`. So when the head was truncated (`cap < len`) and the reported end is
    // `cap`, treat the head as UNCONFIRMED -> None -> conservative full reparse.
    // (A genuine close whose `\n` happens to land on the cap boundary is rejected
    // too, but that alignment is astronomically rare and full reparse is correct.)
    if cap < src.len() && end == cap {
        return None;
    }
    Some(end)
}

/// True if `src` begins with the front-matter opening fence `---\n` at byte 0.
/// Slices to `line_end(0)` (the byte just after the first `\n`, always a valid
/// char boundary under LF-only semantics) rather than a fixed byte offset, so it
/// never splits a multibyte char at the head.
fn starts_with_fm_fence<S: TextSource>(src: &S) -> bool {
    src.slice(0..src.line_end(0)).as_ref() == "---\n"
}

fn is_ref_def_line(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with('[') {
        if let Some(close) = t.find("]:") {
            return close > 1;
        }
    }
    false
}

fn contains_ref_def(s: &str) -> bool {
    s.lines().any(is_ref_def_line)
}

/// Does the byte range [lo,hi) in `src` intersect any line that begins
/// (after optional indentation) with a fence marker (``` or ~~~)?
fn edit_touches_fence_line<S: TextSource>(src: &S, lo: usize, hi: usize) -> bool {
    let lo = lo.min(src.len());
    let hi = hi.min(src.len());
    // Expand to whole lines covering [lo, hi].
    let ls = src.line_start(lo);
    let le = if hi <= lo { src.line_end(lo) } else { src.line_end(hi.saturating_sub(1).max(lo)) };
    let region = src.slice(ls..le);
    region.as_ref().lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("```") || t.starts_with("~~~")
    })
}

fn fence_marker_count(s: &str) -> usize {
    s.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("```") || t.starts_with("~~~")
        })
        .count()
}

fn html_opener_count(s: &str) -> usize {
    s.lines()
        .filter(|l| l.trim_start().starts_with('<'))
        .count()
}

/// Walk backward from `pos` to the start of the current blank-line-delimited
/// line group: i.e. just after the most recent blank (whitespace-only) line
/// at-or-above `pos`, or 0. This captures the upstream context that affects how
/// a line parses (ref-def adjacency, setext underline, lazy paragraph lines).
fn blank_delimited_group_start<S: TextSource>(src: &S, pos: usize) -> usize {
    let mut ls = src.line_start(pos);
    while ls > 0 {
        // line above ls is src[prev_ls..ls]; find its start.
        let prev_ls = src.line_start(ls - 1);
        let prev_line = src.slice(prev_ls..ls); // includes the trailing '\n'
        if prev_line.as_ref().trim().is_empty() {
            break; // blank line above -> ls is the group start
        }
        ls = prev_ls;
    }
    ls
}

fn shift_block(b: &Block, delta: isize) -> Block {
    Block {
        kind: b.kind.clone(),
        span: shift_range(&b.span, delta),
        children: b.children.iter().map(|c| shift_block(c, delta)).collect(),
    }
}

fn shift_range(r: &Range<usize>, delta: isize) -> Range<usize> {
    ((r.start as isize + delta) as usize)..((r.end as isize + delta) as usize)
}

// ---------------------------------------------------------------------------
// Helpers exposed for tests / benches.
// ---------------------------------------------------------------------------

/// Apply an edit to text given the replacement string (test helper).
/// Returns the new text and an `Edit` describing it.
pub fn apply_edit(old_text: &str, range: Range<usize>, replacement: &str) -> (String, Edit) {
    let mut s = String::with_capacity(old_text.len() + replacement.len());
    s.push_str(&old_text[..range.start]);
    s.push_str(replacement);
    s.push_str(&old_text[range.end..]);
    let edit = Edit { range: range.clone(), new_len: replacement.len() };
    (s, edit)
}

/// Property oracle (M7 F2): an incremental block-tree update over `[range)`→`repl` must yield the
/// SAME tree as a full reparse of the resulting text. `cfg(any(test, fuzzing))` so the fuzz crate
/// (built with --cfg fuzzing) can call it; the cfg(test) unit oracle uses it too.
#[cfg(any(test, fuzzing))]
pub fn incremental_equals_full(old: &str, range: std::ops::Range<usize>, repl: &str) -> bool {
    let (new, edit) = apply_edit(old, range, repl);
    let outcome = incremental_update_instrumented(&full_parse(old), old, &edit, &new);
    // BoundedStale is deliberately != full_parse (converged later by reconcile); it is NOT a
    // divergence bug, so the oracle treats it as a pass.
    outcome.reason == WidenReason::BoundedStale || outcome.tree == full_parse(&new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(t: &BlockTree) -> Vec<BlockKind> {
        // BlockKind is Clone (not Copy) in the spike — clone, don't move out of &Block.
        t.top_level().iter().map(|b| b.kind.clone()).collect()
    }

    /// Lightweight, linear structural-validity walk (M1: `check_tree` at block_tree.rs:1837
    /// is quadratic via its per-byte `role_at`; this is O(nodes)). Root spans the whole doc;
    /// child spans are ordered + non-overlapping AND contained within their parent at every
    /// level (gaps between blocks allowed — children do not tile).
    fn assert_valid_tree(t: &BlockTree, new_len: usize) {
        assert_eq!(t.root.span, 0..new_len, "root must span [0, new_len)");
        fn walk(parent: &Block) {
            let mut prev_end = parent.span.start;
            for c in &parent.children {
                assert!(c.span.end >= c.span.start, "span well-formed: {:?}", c.span);
                assert!(c.span.start >= prev_end, "children ordered/non-overlapping: {:?}", c.span);
                assert!(c.span.end <= parent.span.end, "child {:?} escapes parent {:?}", c.span, parent.span);
                prev_end = c.span.end;
                walk(c);
            }
        }
        walk(&t.root);
    }

    #[test]
    fn f1_case_a_small_container_widen_is_bounded() {
        // ~1.56 MiB of paragraphs; the enclosing block of an edit near the top is small,
        // but inserting an opening fence fires `needs_widen_to_end` → the extension reaches
        // EOF (> 1 MiB) → BoundedStale (bounded to the small base region).
        let doc = "para\n\n".repeat(260_000);
        assert!(doc.len() > MAX_SYNC_WIDEN_BYTES, "doc must exceed the cap");
        let (new_text, edit) = apply_edit(&doc, 0..0, "```\n");
        let old_tree = full_parse(&doc);
        let outcome = incremental_update_instrumented(&old_tree, &doc, &edit, &new_text);
        assert_eq!(outcome.reason, WidenReason::BoundedStale, "expected a bounded reparse");
        assert!(outcome.reparsed_bytes <= MAX_SYNC_WIDEN_BYTES,
            "reparsed {} bytes, cap {}", outcome.reparsed_bytes, MAX_SYNC_WIDEN_BYTES);
        assert_valid_tree(&outcome.tree, new_text.len());
        let full = full_parse(&new_text);
        assert_ne!(outcome.tree, full, "BoundedStale is deliberately stale vs full_parse");
        // full_parse is the convergence target the reconcile installs.
        assert_eq!(full.root.span, 0..new_text.len());
    }

    #[test]
    fn f1_bounded_stale_absorptive_tail_not_reextended() {
        // I1-keep: a first-gate BoundedStale whose bounded base region's LAST reparsed block
        // is an absorptive container (here a List) reaching the base boundary must STAY
        // BoundedStale in the second-trigger keep-branch — NOT re-extend to EOF. Shape: a small
        // list near the top of >1 MiB of paragraphs, immediately followed by a heading (so the
        // list is the bounded region's tail), edited in the paragraph ABOVE the list so the list
        // is the (absorptive) slack block → widen fires, base is small, extension reaches EOF.
        let doc = format!("para0\n\n- a\n# h\n{}", "para\n\n".repeat(260_000));
        assert!(doc.len() > MAX_SYNC_WIDEN_BYTES);
        let (new_text, edit) = apply_edit(&doc, 0..0, "x");
        let outcome = incremental_update_instrumented(&full_parse(&doc), &doc, &edit, &new_text);
        assert_eq!(outcome.reason, WidenReason::BoundedStale, "keep-branch must stay BoundedStale");
        // reparsed_bytes small proves the second trigger did NOT re-extend the reparse to EOF.
        assert!(outcome.reparsed_bytes <= MAX_SYNC_WIDEN_BYTES,
            "reparsed {} bytes, cap {}", outcome.reparsed_bytes, MAX_SYNC_WIDEN_BYTES);
        assert_valid_tree(&outcome.tree, new_text.len());
    }

    #[test]
    fn f1_small_extension_still_widens_fully() {
        // Small doc: the widen extension is ≤ cap → WidenToEnd, byte-identical to today.
        let doc = "- a\n- b\n\npara\n";
        let (new_text, edit) = apply_edit(doc, 0..0, "```\n");
        let old_tree = full_parse(doc);
        let outcome = incremental_update_instrumented(&old_tree, doc, &edit, &new_text);
        assert_ne!(outcome.reason, WidenReason::BoundedStale, "small docs must not bound");
        assert_eq!(outcome.tree, full_parse(&new_text), "≤cap path stays == full_parse");
    }

    #[test]
    fn f1_case_b_single_huge_container_falls_through() {
        // A single doc-spanning list (> 1 MiB): the BASE local region is already the whole
        // container → base > cap → falls through to WidenToEnd (never BoundedStale).
        let doc = "- item\n".repeat(200_000); // ~1.4 MiB, one list
        assert!(doc.len() > MAX_SYNC_WIDEN_BYTES);
        let (new_text, edit) = apply_edit(&doc, 7..7, "- x\n"); // edit inside the list near the top
        let old_tree = full_parse(&doc);
        let outcome = incremental_update_instrumented(&old_tree, &doc, &edit, &new_text);
        assert_ne!(outcome.reason, WidenReason::BoundedStale,
            "a single >cap container is Case B — not bounded");
    }

    #[test]
    fn f1_successive_bounded_stale_edits_no_reset() {
        // Production feeds a BoundedStale tree into the NEXT incremental_update WITHOUT a
        // reset (unlike the oracle). Two bounded edits in a row must stay panic-free + valid.
        let doc = "para\n\n".repeat(260_000);
        let (t1_text, e1) = apply_edit(&doc, 0..0, "```\n");
        let o1 = incremental_update_instrumented(&full_parse(&doc), &doc, &e1, &t1_text);
        assert_eq!(o1.reason, WidenReason::BoundedStale);
        // Second bounded edit, fed the STALE tree o1.tree (no reset):
        let (t2_text, e2) = apply_edit(&t1_text, 0..0, "```\n");
        let o2 = incremental_update_instrumented(&o1.tree, &t1_text, &e2, &t2_text);
        assert_valid_tree(&o2.tree, t2_text.len()); // no panic + valid
        // Convergence target exists (what reconcile computes):
        assert_eq!(full_parse(&t2_text).root.span, 0..t2_text.len());
    }

    #[test]
    fn parses_heading_and_paragraph() {
        let t = full_parse("# Title\n\nbody text\n");
        assert_eq!(kinds(&t), vec![BlockKind::Heading(1), BlockKind::Paragraph]);
    }

    #[test]
    fn full_parse_captures_heading_level() {
        let t = full_parse("# H1\n\n### H3\n");
        assert_eq!(kinds(&t), vec![BlockKind::Heading(1), BlockKind::Heading(3)]);
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

    // -----------------------------------------------------------------------
    // Hazard regression tests (ported from spike tests/oracle.rs)
    // Each asserts incremental_update == full_parse for a specific edit.
    // -----------------------------------------------------------------------

    fn check(old_text: &str, range: std::ops::Range<usize>, replacement: &str) -> UpdateOutcome {
        let old_tree = full_parse(old_text);
        let (new_text, edit) = apply_edit(old_text, range, replacement);
        let outcome = incremental_update_instrumented(&old_tree, old_text, &edit, &new_text);
        let full = full_parse(&new_text);
        if outcome.reason != WidenReason::BoundedStale {
            assert_eq!(
                outcome.tree, full,
                "\nINCREMENTAL != FULL\nold_text={old_text:?}\nnew_text={new_text:?}\nreason={:?}\nincremental={:#?}\nfull={:#?}",
                outcome.reason, outcome.tree, full
            );
        }
        outcome
    }

    #[test]
    fn hazard_typing_inside_paragraph_is_local() {
        let doc = "First para.\n\nSecond para here.\n\nThird para.\n";
        let pos = doc.find("here").unwrap();
        let out = check(doc, pos..pos, "X");
        assert_eq!(out.reason, WidenReason::Local);
        assert!(out.reparsed_bytes < doc.len(), "reparsed {} of {}", out.reparsed_bytes, doc.len());
    }

    // Regression (M7 final gate): a CRLF document with two adjacent top-level
    // paragraphs. The seam guard's blank-line predicate must recognize a "\r\n"
    // blank line — otherwise it reads the gap as "no blank line", concludes the
    // first paragraph absorbs the next, and forces a FULL REPARSE on ordinary
    // in-paragraph typing. That would make every keystroke O(document) in CRLF
    // docs. This test asserts the edit stays on the fast path (WidenReason::Local)
    // and that the incremental tree equals a full reparse (via `check`).
    #[test]
    fn hazard_crlf_paragraph_typing_stays_local() {
        let doc = "First para.\r\n\r\nSecond para here.\r\n\r\nThird para.\r\n";
        let pos = doc.find("here").unwrap();
        let out = check(doc, pos..pos, "X");
        assert_eq!(out.reason, WidenReason::Local);
        assert!(out.reparsed_bytes < doc.len(), "reparsed {} of {}", out.reparsed_bytes, doc.len());
    }

    #[test]
    fn hazard_opening_a_fence_swallows_rest_of_doc() {
        let doc = "para one\n\npara two\n\npara three\n";
        let out = check(doc, 0..0, "```\n");
        assert_eq!(out.reason, WidenReason::WidenToEnd);
    }

    #[test]
    fn hazard_fenced_code_spanning_blank_lines() {
        let doc = "intro\n\n```\nline1\n\nline2\n```\n\nafter\n";
        let pos = doc.find("line1").unwrap();
        check(doc, pos..pos + 5, "CHANGED");
    }

    #[test]
    fn hazard_link_reference_definition_affects_later_links() {
        let doc = "[foo]: http://example.com\n\nsee [foo] here\n\nmore text\n";
        let pos = doc.find("example").unwrap();
        let out = check(doc, pos..pos + 7, "changed");
        assert_eq!(out.reason, WidenReason::WidenToEnd, "ref def edit must widen to end");
    }

    #[test]
    fn hazard_setext_underline_to_thematic_break_ambiguity() {
        let doc = "para text\n---\n\nbody\n";
        let end = doc.find("\n---").unwrap() + 1;
        check(doc, 0..end, "");
    }

    // Regression (M7 fuzz, target F2 `block_tree`): an empty edit (range 2..2, repl "")
    // inside a link-reference-definition-only document. The doc full-parses to a single
    // synthetic trailing empty Paragraph at span 10..10. The widen-to-end region reparse
    // [0..10] already reproduces that block, but the splice's "after" predicate
    // (b.span.start >= region_old_end) also matched the same zero-length block at offset
    // 10, emitting it a SECOND time — incremental had two empty paragraphs where full had
    // one. Pinned minimized input from `cargo fuzz tmin`; must stay incremental == full.
    #[test]
    fn hazard_zero_length_trailing_block_not_double_spliced() {
        let doc = "[e_]: h\n\t\t";
        check(doc, 2..2, "");
    }

    // Regression (M7 fuzz, target F2 `block_tree`): inserting a byte at end-of-doc
    // that turns the trailing ATX heading "#" into a paragraph "#\0". An upstream
    // link-reference def leaves a degenerate empty Paragraph just before the heading;
    // once the heading becomes a paragraph the two MERGE (full parse: one Paragraph
    // 13..16). The localized splice kept the empty paragraph as a verbatim "before"
    // block and reparsed only the heading region, producing two adjacent paragraphs
    // with no blank line between them. The seam-consistency guard now detects that
    // illegal adjacency and falls back to a full reparse. Pinned minimized input from
    // `cargo fuzz tmin`; must stay incremental == full.
    #[test]
    fn hazard_heading_to_paragraph_merges_preceding_empty_paragraph() {
        let doc = "[\u{0}\u{0}d]: ?ht=\n\t\n#";
        check(doc, 15..15, "\u{0}");
    }

    // Regression (M7 fuzz, target F2 `block_tree`): inserting an HTML-comment opener
    // into a document whose lines are separated by BARE carriage returns. pulldown-cmark
    // honors bare `\r` as a line break and starts the comment at a `\r`-delimited line
    // start, extending it past where the LF-only region machinery (`str::lines`,
    // `line_start`) believes — so `html_in_play` failed to see the opener, the region
    // reparsed Locally with the wrong extent, and a stale trailing Paragraph was left
    // where the full parse keeps it inside the HtmlComment. The bare-CR fallback now
    // routes such edits to a full reparse. Pinned minimized input from `cargo fuzz tmin`.
    #[test]
    fn hazard_bare_cr_html_comment_extent() {
        let old = "<!<!---|~||~#\r\n\n]a\r\r\r\r\r<sCde\n\n] \u{17}~\n|---|----\n\n\0\0\0\0\0\0:`";
        let repl = "<!---\0\0\0\0\0\0\0\0\0\0\0\r\r\r\r\r<sCde\n\n]a\r<sCd\n\n] \u{17}~\n|---|----\n\n\0\r\n\rs";
        check(old, 23..23, repl);
    }

    // Regression (M7 fuzz, target F2 `block_tree`): a no-op edit (empty range, empty
    // repl) inside the blank-line run of a document whose first line group is a link
    // reference definition immediately followed by a `*` line. The gap edit's upstream
    // pull-back set region_old_start to the `*` paragraph's line start, EXCLUDING the
    // ref-def line above it (no blank line separates them). Reparsing the `*` line in
    // isolation flipped it from Paragraph to List, diverging from the full parse. The
    // pull-back now re-runs the blank-delimited group walk to include the ref-def
    // context. Pinned minimized input from `cargo fuzz tmin`.
    #[test]
    fn hazard_gap_edit_pullback_includes_ref_def_group() {
        let doc = "[e\nf]: :`ht\n*\n\n\n\n\n\n\n\n\ne\nh\n|\n\n\u{0}zzzzzzzzzzzzzzzzzz\n";
        check(doc, 20..20, "");
    }

    // Regression (M7 fuzz, target F2 `block_tree`): replacing a blank line ("\t\n")
    // that followed a paragraph with a tab-indented line ("\t~~~`r\n"). Removing the
    // blank line makes the indented line a LAZY CONTINUATION of the preceding
    // paragraph (full parse: one Paragraph 0..14), but the localized splice kept the
    // paragraph verbatim and reparsed the tail as a separate IndentedCode block.
    // Indented code cannot interrupt a paragraph, so the seam guard now folds it back
    // and falls back to a full reparse. Pinned minimized input from `cargo fuzz tmin`.
    #[test]
    fn hazard_paragraph_lazily_absorbs_indented_continuation() {
        let doc = "[]o\n\t\u{b}\n\t\n\u{b}";
        check(doc, 8..10, "~~~`r\n");
    }

    // Regression (M7 fuzz, target F2 `block_tree`): two paragraphs that a full parse
    // merges (Paragraph 0..30) but the splice left split, because the "blank" line
    // between them (" \u{b}\n") holds a VERTICAL TAB — which CommonMark does NOT count
    // as blank-line whitespace, so the line is a non-blank lazy continuation, not a
    // separator. The seam guard's blank-line test now accepts only spaces/tabs (not
    // every '\n', not \u{b}), so it folds the paragraphs and falls back to a full
    // reparse. Pinned minimized input from `cargo fuzz tmin`.
    #[test]
    fn hazard_vertical_tab_line_is_not_a_blank_separator() {
        let old = "\u{0}&|~\u{1}\u{0}<J<Pre\t\n\u{b}\n \n\n\u{1e}x";
        let repl = "\u{b}\n\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}}s";
        check(old, 17..20, repl);
    }

    // Regression (M7 fuzz, target F2 `block_tree`): an edit that CREATES a list at the
    // region start (turning "X" into a "- " bullet) whose last item lazily absorbs an
    // indented code block that lives PAST the localized reparse region. The absorptive-
    // widen gate only inspects OLD-tree blocks, so the newly-created list was missed and
    // the Local reparse cut it at the region boundary, mis-attributing the item's
    // extent. The created-container-growth check now widens to a full reparse when the
    // reparse's tail block is an absorptive container reaching the region end with
    // document remaining. Distilled from a `cargo fuzz`-found nested-list divergence.
    #[test]
    fn hazard_created_list_absorbs_indented_code_past_region() {
        let doc = "Xa\n\n  c\n\n    code\n";
        check(doc, 0..1, "- ");
    }

    #[test]
    fn role_at_classifies_blocks() {
        // doc: "# H\n\n> q\n\n- a\n\n```\nc\n```\n\n---\n\npara\n"
        let doc = "# H\n\n> q\n\n- a\n\n```\nc\n```\n\n---\n\npara\n";
        let t = full_parse(doc);
        use crate::style::BlockRole::*;
        let role = |needle: &str| t.role_at(doc.find(needle).unwrap());
        assert_eq!(role("H"), Heading(1));
        assert_eq!(role("q"), BlockQuote);     // line is inside a blockquote
        assert_eq!(role("a"), ListItem);       // line is a list item
        assert_eq!(role("c"), CodeBlock);      // inside a fenced code block
        assert_eq!(role("---"), ThematicBreak);
        assert_eq!(role("para"), Paragraph);
    }

    #[test]
    fn role_at_gaps_and_boundaries_are_paragraph() {
        let doc = "# H\n\npara\n";
        let t = full_parse(doc);
        use crate::style::BlockRole::*;
        // the blank line (byte 4, the second '\n') is in a gap -> Paragraph
        assert_eq!(t.role_at(4), Paragraph);
        // a byte past document end -> Paragraph
        assert_eq!(t.role_at(doc.len() + 5), Paragraph);
    }

    // -----------------------------------------------------------------------
    // TextSource trait tests
    // -----------------------------------------------------------------------

    /// Helper: for a string `s`, verify that the &str and &Rope impls of
    /// TextSource agree on len, slice over a set of ranges, and
    /// line_start/line_end at every byte position p in 0..=s.len().
    fn check_textsource_agree(s: &str) {
        let r = ropey::Rope::from_str(s);
        let str_src: &dyn TextSource = &s;
        let rope_src: &dyn TextSource = &&r;

        // len
        assert_eq!(
            str_src.len(), rope_src.len(),
            "len mismatch for {:?}", s
        );

        // slice over several representative ranges (all char-boundary-safe)
        let len = s.len();
        let slice_ranges: Vec<std::ops::Range<usize>> = {
            // Collect all char boundaries (valid UTF-8 slice endpoints)
            let boundaries: Vec<usize> = s.char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(len))
                .collect();
            let mut v = Vec::new();
            for i in 0..boundaries.len() {
                for j in i..boundaries.len() {
                    v.push(boundaries[i]..boundaries[j]);
                }
            }
            v.sort_by_key(|r| (r.start, r.end));
            v.dedup();
            v
        };
        for range in &slice_ranges {
            let str_slice = str_src.slice(range.clone());
            let rope_slice = rope_src.slice(range.clone());
            assert_eq!(
                str_slice.as_ref(), rope_slice.as_ref(),
                "slice({:?}) mismatch for {:?}: str={:?} rope={:?}",
                range, s, str_slice, rope_slice
            );
        }

        // line_start and line_end at every position 0..=len
        for p in 0..=len {
            let str_ls = str_src.line_start(p);
            let rope_ls = rope_src.line_start(p);
            assert_eq!(
                str_ls, rope_ls,
                "line_start({}) mismatch for {:?}: str={} rope={}",
                p, s, str_ls, rope_ls
            );

            let str_le = str_src.line_end(p);
            let rope_le = rope_src.line_end(p);
            assert_eq!(
                str_le, rope_le,
                "line_end({}) mismatch for {:?}: str={} rope={}",
                p, s, str_le, rope_le
            );
        }
    }

    #[test]
    fn textsource_str_and_rope_agree() {
        // ASCII basics
        for s in ["", "a", "a\n", "\n", "ab\ncd\n", "ab\ncd", "no newline"] {
            check_textsource_agree(s);
        }
        // Multibyte content
        check_textsource_agree("# 中\n\n🙂 x\nyy");
        // Non-LF separator hazard cases: rope's unicode_lines would diverge
        // on these if we used ropey's line APIs. Our impl must treat ONLY '\n'
        // as a line break.
        check_textsource_agree("a\rb");        // CR: not a line break
        check_textsource_agree("a\r\nb");      // CRLF: only the \n breaks (if any)
        check_textsource_agree("a\x0bb");      // VT: not a line break
        check_textsource_agree("a\x0cb");      // FF: not a line break
        check_textsource_agree("a\u{0085}b"); // NEL: not a line break
        check_textsource_agree("a\u{2028}b"); // LS: not a line break
        check_textsource_agree("a\u{2029}b"); // PS: not a line break

        // Multi-chunk: ropey chunks are ~1 KiB, so this >1 KiB string forces the
        // rope line_start/line_end chunk-crossing loops to execute (the small
        // cases above all fit in one chunk and never exercise that path).
        let multi_chunk = format!("{}\n{}", "a".repeat(600), "b".repeat(600)); // 1201 bytes, '\n' at byte 600
        check_textsource_agree(&multi_chunk);
        // Pin the exact boundary the chunk-crossing scan must find:
        for p in 0..=600 { assert_eq!((&multi_chunk.as_str()).line_end(p), 601, "line_end({p})"); }
        for p in 601..=1201 { assert_eq!((&ropey::Rope::from_str(&multi_chunk)).line_start(p), 601, "line_start({p})"); }
    }

    // -----------------------------------------------------------------------
    // Task 2: full_parse_src over TextSource
    // -----------------------------------------------------------------------

    /// Verify that `full_parse_src` over a `&Rope` produces the same `BlockTree`
    /// as `full_parse` over the same text as `&str`.  Tests cover the
    /// representative document shapes listed in the task brief.
    fn check_rope_eq_str(s: &str) {
        let rope = ropey::Rope::from_str(s);
        let from_rope = full_parse_src(&&rope);
        let from_str  = full_parse(s);
        assert_eq!(
            from_rope, from_str,
            "full_parse_src(&Rope) != full_parse(&str) for {:?}\nrope={:#?}\nstr={:#?}",
            s, from_rope, from_str,
        );
    }

    #[test]
    fn full_parse_src_heading_and_para() {
        check_rope_eq_str("# Title\n\nbody text\n");
    }

    #[test]
    fn full_parse_src_fenced_code_with_internal_blank() {
        check_rope_eq_str("```\na\n\nb\n```\n");
    }

    #[test]
    fn full_parse_src_nested_list() {
        check_rope_eq_str("- item 1\n  - sub A\n  - sub B\n- item 2\n");
    }

    #[test]
    fn full_parse_src_blockquote() {
        check_rope_eq_str("> first line\n> second line\n\nafter\n");
    }

    #[test]
    fn full_parse_src_gfm_table() {
        check_rope_eq_str("| A | B |\n|---|---|\n| 1 | 2 |\n");
    }

    #[test]
    fn full_parse_src_link_ref_def() {
        check_rope_eq_str("[foo]: http://example.com\n\nsee [foo] here\n");
    }

    #[test]
    fn full_parse_src_multibyte() {
        check_rope_eq_str("# 中\n\n- 🙂\n");
    }

    // -----------------------------------------------------------------------
    // Task 4: rope entry point tests
    // -----------------------------------------------------------------------

    /// Step 1 (TDD): write this test FIRST so it fails before the rope entry
    /// points exist.  After the refactor it must pass.
    #[test]
    fn rope_incremental_matches_full_and_str() {
        let old = "para one\n\n- a\n- b\n\n[r]: http://x\n";
        // edit: insert "X" at position 9 (inside the blank line between para and list)
        let (new, edit) = apply_edit(old, 9..9, "X");
        let ot = full_parse(old);
        let str_tree = incremental_update(&ot, old, &edit, &new);
        let rope_tree = incremental_update_rope(
            &ot,
            &ropey::Rope::from_str(old),
            &edit,
            &ropey::Rope::from_str(&new),
        );
        assert!(incremental_equals_full(old, 9..9, "X"), "str incremental != full_parse");
        assert_eq!(rope_tree, str_tree, "rope incremental != str incremental");
    }

    // -----------------------------------------------------------------------
    // Task 4 (theming): byte-0 YAML front matter → BlockKind::FrontMatter
    // -----------------------------------------------------------------------

    #[test]
    fn byte0_front_matter_is_front_matter_role() {
        let doc = "---\ntitle: Hi\n---\n\n# Heading\n";
        let t = full_parse(doc);
        assert_eq!(
            t.role_at(doc.find("title").unwrap()),
            crate::style::BlockRole::FrontMatter
        );
        // the heading after it is unaffected
        assert_eq!(
            t.role_at(doc.find("Heading").unwrap()),
            crate::style::BlockRole::Heading(1)
        );
    }

    #[test]
    fn mid_document_dashes_are_not_front_matter() {
        // a `---` NOT at byte 0 is a thematic break / setext underline, never front matter.
        let doc = "para\n\n---\n\nmore\n";
        let t = full_parse(doc);
        assert_ne!(
            t.role_at(doc.find("more").unwrap()),
            crate::style::BlockRole::FrontMatter
        );
    }

    // -----------------------------------------------------------------------
    // Task 3: block <!-- --> → HtmlComment → BlockRole::Comment
    // -----------------------------------------------------------------------

    #[test]
    fn block_html_comment_maps_to_comment_role() {
        let doc = "<!-- a block comment -->\n\npara\n";
        let t = full_parse(doc);
        let at = |needle: &str| t.role_at(doc.find(needle).unwrap());
        assert_eq!(at("block comment"), crate::style::BlockRole::Comment);
    }

    #[test]
    fn block_div_is_not_comment() {
        let doc = "<div>x</div>\n\npara\n";
        let t = full_parse(doc);
        assert_ne!(t.role_at(doc.find("x").unwrap()), crate::style::BlockRole::Comment);
    }

    /// A block comment nested inside a list item (as a loose list child) must
    /// still resolve to BlockRole::Comment, not BlockRole::ListItem.
    ///
    /// In CommonMark a blank-separated item body can contain block-level HTML.
    /// The markdown `"- item\n\n  <!-- c -->\n"` produces a loose list item
    /// whose children include both a paragraph and an HTML block.  pulldown-cmark
    /// nests the HtmlBlock under the ListItem; the HtmlComment block is therefore
    /// a grandchild of the Document.  role_at walks the full tree, so the
    /// HtmlComment role (precedence 2) beats ListItem (precedence 3), yielding
    /// BlockRole::Comment.
    #[test]
    fn block_comment_nested_in_list_item_wins_over_list_item() {
        // Loose list: blank line between the paragraph and the comment block
        // forces the list item to be "loose", making the HTML block a sibling
        // paragraph inside the list item.
        let doc = "- item\n\n  <!-- c -->\n";
        let t = full_parse(doc);
        // Verify nesting: the HTML comment must actually appear as a nested block.
        // Walk children to check the structure.
        let tops = t.top_level();
        assert!(!tops.is_empty(), "expected at least one top-level block");
        // The top-level block should be a List containing a ListItem.
        assert_eq!(tops[0].kind, BlockKind::List, "expected List at top level");
        // Find the comment block somewhere in the tree
        let comment_pos = doc.find("<!-- c -->").unwrap();
        let role = t.role_at(comment_pos);
        assert_eq!(role, crate::style::BlockRole::Comment,
            "comment nested in list item should resolve to Comment, got {:?}", role);
    }

    /// Extra assertions for the non-LF separator cases: prove that line_start
    /// and line_end never split at the non-LF separators — i.e. for "a\rb" the
    /// whole string is ONE line.
    #[test]
    fn textsource_non_lf_separators_are_single_line() {
        let cases = [
            "a\rb",
            "a\x0bb",
            "a\x0cb",
            "a\u{0085}b",
            "a\u{2028}b",
            "a\u{2029}b",
        ];
        for s in cases {
            let r = ropey::Rope::from_str(s);
            let rope_src: &dyn TextSource = &&r;
            let len = s.len();
            // Every position should have line_start==0 and line_end==len
            // (there's no '\n', so the entire string is one line).
            for p in 0..=len {
                assert_eq!(
                    rope_src.line_start(p), 0,
                    "rope line_start({p}) != 0 for {:?} — rope split on non-LF separator", s
                );
                assert_eq!(
                    rope_src.line_end(p), len,
                    "rope line_end({p}) != len({len}) for {:?} — rope split on non-LF separator", s
                );
            }
        }
    }

    #[test]
    fn empty_tree_is_a_childless_document_root() {
        let t = empty_tree(42);
        assert_eq!(t.root.kind, BlockKind::Document);
        assert_eq!(t.root.span, 0..42);
        assert!(t.top_level().is_empty());
        // Any byte resolves to the default Paragraph role — no child span to slice.
        assert_eq!(t.role_at(0), crate::style::BlockRole::Paragraph);
        assert_eq!(t.role_at(41), crate::style::BlockRole::Paragraph);
    }

    // -----------------------------------------------------------------------
    // F3: role_at binary-search differential + ordering invariant
    // -----------------------------------------------------------------------

    use proptest::prelude::*;

    // Reference: the pre-change linear scan. Lives in-module so it can call the
    // private `kind_to_role`/`role_precedence` without any visibility gymnastics.
    fn collect_role_linear(block: &Block, byte: usize, best: &mut crate::style::BlockRole) {
        if !block.span.contains(&byte) { return; }
        if let Some(role) = kind_to_role(&block.kind) {
            if role_precedence(&role) < role_precedence(best) { *best = role; }
        }
        for child in &block.children { collect_role_linear(child, byte, best); }
    }
    fn role_at_linear(t: &BlockTree, byte: usize) -> crate::style::BlockRole {
        let mut best = crate::style::BlockRole::Paragraph;
        collect_role_linear(&t.root, byte, &mut best);
        best
    }
    fn assert_ordered_nonoverlapping(b: &Block) {
        let mut prev_end = 0usize;
        for c in &b.children {
            assert!(c.span.start >= prev_end, "siblings ordered + non-overlapping");
            prev_end = c.span.end.max(prev_end);
            assert_ordered_nonoverlapping(c);
        }
    }
    fn check_tree(t: &BlockTree, len: usize) {
        assert_ordered_nonoverlapping(&t.root);
        for byte in 0..=len {
            assert_eq!(t.role_at(byte), role_at_linear(t, byte), "role_at divergence @ {byte}");
        }
    }
    // Nested-container markdown snippets — the trees where ordering is least certain.
    fn doc_strategy() -> impl Strategy<Value = String> {
        let snippet = prop::sample::select(vec![
            "# H1\n", "## H2\n", "Setext\n===\n",
            "- a\n- b\n", "- outer\n  - inner\n", "1. one\n2. two\n",
            "> quote\n> - qlist\n", "```\ncode\n```\n",
            "para one two\n", "\n", "text\n",
        ]);
        prop::collection::vec(snippet, 0..10).prop_map(|v| v.concat())
    }

    proptest! {
        #[test]
        fn role_at_binary_matches_linear(
            text in doc_strategy(),
            inserts in prop::collection::vec(("[a-z#>` \\-\n]{1,4}", 0usize..300), 0..4),
        ) {
            let mut cur = text;
            let mut tree = full_parse(&cur);
            check_tree(&tree, cur.len());
            // Incremental chain: apply a few inserts, checking the SPLICED tree each step.
            for (s, raw) in inserts {
                let mut pos = raw % (cur.len() + 1);
                while pos < cur.len() && !cur.is_char_boundary(pos) { pos += 1; }
                let mut next = String::with_capacity(cur.len() + s.len());
                next.push_str(&cur[..pos]); next.push_str(&s); next.push_str(&cur[pos..]);
                let edit = Edit { range: pos..pos, new_len: s.len() };
                let (old_ref, new_ref): (&str, &str) = (&cur, &next);
                tree = incremental_update_src(&tree, &old_ref, &edit, &new_ref);
                check_tree(&tree, next.len());
                cur = next;
            }
        }
    }
}
