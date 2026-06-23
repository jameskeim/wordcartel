use std::ops::Range;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

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
        ThematicBreak  => 2,
        ListItem       => 3,
        BlockQuote     => 4,
        Paragraph      => 5,
        FrontMatter    => 5,
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
    // Recurse into children.
    for child in &block.children {
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
}

/// Result + instrumentation.
pub struct UpdateOutcome {
    pub tree: BlockTree,
    pub reason: WidenReason,
    /// Number of bytes actually reparsed (the slice length).
    pub reparsed_bytes: usize,
}

/// Incrementally update `old_tree` for `edit`, producing the new tree.
pub fn incremental_update(
    old_tree: &BlockTree,
    old_text: &str,
    edit: &Edit,
    new_text: &str,
) -> BlockTree {
    incremental_update_instrumented(old_tree, old_text, edit, new_text).tree
}

/// As `incremental_update` but returns instrumentation about how wide we went.
pub fn incremental_update_instrumented(
    old_tree: &BlockTree,
    old_text: &str,
    edit: &Edit,
    new_text: &str,
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
    region_old_start = line_start(old_text, region_old_start);
    region_old_end = line_end(old_text, region_old_end);

    // UPSTREAM LINE-GROUP CONTEXT: how a line parses depends on the lines
    // immediately above it within the same blank-line-delimited group: a ref
    // def line changes whether the next line continues a paragraph; a setext
    // underline needs the line above; a `-` after a paragraph is a setext
    // underline, but after a blank line is a list bullet. So pull the region
    // start back to the most recent BLANK line (start of the current line
    // group). This guarantees the reparse sees the whole group.
    region_old_start = blank_delimited_group_start(old_text, region_old_start);
    // The group walk can cross a blank line that lives *inside* a fenced or
    // indented code block (where blanks are content, not delimiters), landing
    // mid-block. Snap back to the start of any top-level block it landed inside.
    for b in tops.iter() {
        if b.span.start < region_old_start && b.span.end > region_old_start {
            region_old_start = line_start(old_text, b.span.start);
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
            // gap between block end and region start must be only blank /
            // ref-def lines (no other block intervenes by construction, since
            // this is the nearest preceding block).
            let gap_lo = b.span.end.min(old_text.len());
            let gap_hi = region_old_start.min(old_text.len());
            let gap = &old_text[gap_lo.min(gap_hi)..gap_hi];
            let gap_is_soft = gap
                .lines()
                .all(|l| l.trim().is_empty() || is_ref_def_line(l));
            if absorptive && b.span.end <= region_old_start && gap_is_soft {
                region_old_start = line_start(old_text, b.span.start);
                continue;
            }
        }
        break;
    }

    // HTML blocks can change where a *preceding* block ends, so a downstream-
    // only widen is not enough — fall back to a full reparse. This is cheap to
    // detect and HTML in prose is rare (see report).
    if html_in_play(old_text, new_text, edit, region_old_start, region_old_end) {
        let tree = full_parse(new_text);
        return UpdateOutcome { tree, reason: WidenReason::NoOverlapFull, reparsed_bytes: new_text.len() };
    }

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
    let slack_block = tops.iter().find(|b| b.span.start >= region_old_end);
    let slack_is_absorptive = slack_block.map_or(false, |b| {
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

    let widen = absorptive_in_region
        || slack_is_absorptive
        || needs_widen_to_end(old_text, new_text, edit, region_old_start, region_old_end);
    let reason;
    if widen {
        region_old_end = old_text.len();
        reason = WidenReason::WidenToEnd;
    } else {
        // +1 top-level block of slack: extend region_old_end to cover the
        // first block at or after the current region end (the "slack" block),
        // plus all gap bytes between that block and the NEXT block.
        //
        // Why include the gap bytes? A block's pulldown span can extend into
        // trailing blank lines that logically "belong" to it (e.g. the blank
        // lines terminating a loose list). Those blank lines are gap bytes
        // between the slack block's span.end and the next block's span.start.
        // If the region stops at line_end(slack_block.span.end), those blank
        // lines end up neither in the reparse nor in any shifted "after" block,
        // corrupting the splice.
        //
        // Fix: set region_old_end to the span.start of the block AFTER the
        // slack block (the first true "after" block), so all gap bytes between
        // the slack block and the next "after" block are inside the region.
        // If there's no block after the slack, extend to old_text.len().
        //
        // This is needed in two cases:
        // (a) The edit overlapped at least one block (have_overlap=true): the
        //     block immediately after the region may be affected by context
        //     changes at the region boundary (setext underlines, lazy
        //     continuation, etc.).
        // (b) The edit was entirely in a gap between blocks (have_overlap=false):
        //     inserting content into the gap can collapse it so that the
        //     following block merges with the new content — it must be reparsed
        //     rather than merely shifted.
        let slack_pos = tops.iter().position(|b| b.span.start >= region_old_end);
        if let Some(slack_idx) = slack_pos {
            // Extend to the start of the block after the slack block, so that
            // all gap bytes between the slack block and the next "after" block
            // are inside the region.
            if slack_idx + 1 < tops.len() {
                region_old_end = tops[slack_idx + 1].span.start;
            } else {
                // No block after the slack block — include all trailing bytes.
                region_old_end = old_text.len();
            }
        }
        // When editing in a gap, also include the last block before the region
        // start: upstream context (e.g. a paragraph immediately before the gap)
        // may change how the gap and following content parse.
        if !have_overlap {
            if let Some(b) = tops.iter().rev().find(|b| b.span.end <= region_old_start) {
                region_old_start = line_start(old_text, b.span.start);
            }
        }
        reason = WidenReason::Local;
    }

    // INVARIANT REPAIR: top-level block spans do not tile the document (there
    // are gaps for indented-code leading whitespace, ref-def lines, trailing
    // whitespace). The splice classifies each old block as strictly-before,
    // strictly-after, or replaced-by-reparse. A block that *straddles* a region
    // boundary would be silently dropped. So grow the region until every block
    // is cleanly before/after it.
    loop {
        let mut grew = false;
        for b in tops.iter() {
            // straddles start?
            if b.span.start < region_old_start && b.span.end > region_old_start {
                region_old_start = line_start(old_text, b.span.start);
                grew = true;
            }
            // straddles end?
            if b.span.start < region_old_end && b.span.end > region_old_end {
                region_old_end = line_end(old_text, b.span.end);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    // GAP-BYTE COVERAGE: top-level block spans do not tile the document —
    // trailing gap bytes (beyond the last block) belong to no block and are
    // neither included in the reparse region nor captured by a shifted "after"
    // block. If there are no "after" blocks (no block with span.start >=
    // region_old_end), those trailing bytes disappear from the splice result.
    // Extend region_old_end to old_text.len() so they are included in the
    // reparse when necessary.
    let has_after_block = tops.iter().any(|b| b.span.start >= region_old_end);
    if !has_after_block && region_old_end < old_text.len() {
        region_old_end = old_text.len();
    }

    // Gap 3: machine-check the trailing-gap bound. `region_old_end` is now
    // final; verify it never exceeds the document length before we use it to
    // compute region_new_end and drive the splice.
    debug_assert!(
        region_old_end <= old_text.len(),
        "region_old_end {} past doc len {}",
        region_old_end,
        old_text.len()
    );

    debug_assert!(region_old_start <= edit_lo);
    debug_assert!(region_old_end >= edit_hi);

    let region_new_start = region_old_start;
    let region_new_end = (region_old_end as isize + delta) as usize;

    let new_slice = &new_text[region_new_start..region_new_end];
    let reparsed = parse_region(new_slice, region_new_start);
    let reparsed_bytes = new_slice.len();

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
    result_children.extend(reparsed.root.children.iter().cloned());
    for b in tops.iter() {
        if b.span.start >= region_old_end {
            result_children.push(shift_block(b, delta));
        }
    }

    let root = Block { kind: BlockKind::Document, span: 0..new_text.len(), children: result_children };
    UpdateOutcome { tree: BlockTree { root }, reason, reparsed_bytes }
}

/// Conservative triggers that force reparsing to end-of-document.
fn needs_widen_to_end(
    old_text: &str,
    new_text: &str,
    edit: &Edit,
    region_old_start: usize,
    region_old_end: usize,
) -> bool {
    let os = region_old_start.min(old_text.len());
    let oe = region_old_end.min(old_text.len());
    let old_region = &old_text[os.min(oe)..oe];
    let new_start = region_old_start.min(new_text.len());
    let new_region_end = ((region_old_end as isize + edit.delta()) as usize).min(new_text.len());
    let new_region = &new_text[new_start.min(new_region_end)..new_region_end];

    // (a) Link reference definitions are resolved document-wide.
    if contains_ref_def(old_region) || contains_ref_def(new_region) {
        return true;
    }
    // (b) Fence structure is fragile: editing ANY fence-marker line can flip a
    //     close into an opener (e.g. "```" -> "```> "), leaving the fence open
    //     and swallowing the rest of the document — or vice versa. The
    //     marker-COUNT is not enough (the line still *starts* with backticks),
    //     so we widen whenever the edit intersects a fence-marker line in
    //     either the old or new text, or the marker count changes.
    if fence_marker_count(old_region) != fence_marker_count(new_region) {
        return true;
    }
    let new_edit_start = edit.range.start;
    let new_edit_end = edit.range.start + edit.new_len;
    if edit_touches_fence_line(old_text, edit.range.start, edit.range.end)
        || edit_touches_fence_line(new_text, new_edit_start, new_edit_end)
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
fn html_in_play(
    old_text: &str,
    new_text: &str,
    edit: &Edit,
    region_old_start: usize,
    region_old_end: usize,
) -> bool {
    let os = region_old_start.min(old_text.len());
    let oe = region_old_end.min(old_text.len());
    let old_region = &old_text[os.min(oe)..oe];
    let new_start = region_old_start.min(new_text.len());
    let new_region_end = ((region_old_end as isize + edit.delta()) as usize).min(new_text.len());
    let new_region = &new_text[new_start.min(new_region_end)..new_region_end];
    html_opener_count(old_region) > 0 || html_opener_count(new_region) > 0
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

/// Does the byte range [lo,hi) in `text` intersect any line that begins
/// (after optional indentation) with a fence marker (``` or ~~~)?
fn edit_touches_fence_line(text: &str, lo: usize, hi: usize) -> bool {
    let lo = lo.min(text.len());
    let hi = hi.min(text.len());
    // Expand to whole lines covering [lo, hi].
    let ls = line_start(text, lo);
    let le = if hi <= lo { line_end(text, lo) } else { line_end(text, hi.saturating_sub(1).max(lo)) };
    text[ls..le].lines().any(|l| {
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
fn blank_delimited_group_start(text: &str, pos: usize) -> usize {
    let mut ls = line_start(text, pos);
    while ls > 0 {
        // line above ls is text[prev_ls..ls-? ]; find its start.
        let prev_ls = line_start(text, ls - 1);
        let prev_line = &text[prev_ls..ls]; // includes the trailing '\n'
        if prev_line.trim().is_empty() {
            break; // blank line above -> ls is the group start
        }
        ls = prev_ls;
    }
    ls
}

/// Byte index of the start of the line containing `pos` (i.e. just after the
/// previous '\n', or 0).
///
/// `pos` need not be on a char boundary — we search through the raw bytes for
/// the ASCII newline (0x0A), which is never a continuation byte of a multibyte
/// sequence, so byte-level search is always correct.
fn line_start(text: &str, pos: usize) -> usize {
    let pos = pos.min(text.len());
    let bytes = text.as_bytes();
    // Search backwards through bytes for '\n'.
    match bytes[..pos].iter().rposition(|&b| b == b'\n') {
        Some(nl) => nl + 1,
        None => 0,
    }
}

/// Byte index just past the '\n' terminating the line containing `pos` (or
/// text end). This keeps the region on whole lines.
///
/// `pos` need not be on a char boundary — we search through raw bytes for the
/// ASCII newline (0x0A).
fn line_end(text: &str, pos: usize) -> usize {
    let pos = pos.min(text.len());
    let bytes = text.as_bytes();
    match bytes[pos..].iter().position(|&b| b == b'\n') {
        Some(off) => pos + off + 1,
        None => text.len(),
    }
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
        assert_eq!(
            outcome.tree, full,
            "\nINCREMENTAL != FULL\nold_text={old_text:?}\nnew_text={new_text:?}\nreason={:?}\nincremental={:#?}\nfull={:#?}",
            outcome.reason, outcome.tree, full
        );
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
}
