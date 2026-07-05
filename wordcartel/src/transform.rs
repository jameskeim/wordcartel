//! In-process repar transforms (Reflow / Unwrap / Ventilate). The typed wrapper
//! `run_transform` is the ONLY place that touches repar's stringly public API.

pub const DEFAULT_REFLOW_WIDTH: u32 = 72;
/// Regions at or above this byte length run off the keystroke thread (§5.2).
pub const TRANSFORM_ASYNC_THRESHOLD: usize = 1 << 20; // 1 MiB

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformKind { Reflow, Unwrap, Ventilate }

impl TransformKind {
    fn verb(self) -> &'static str {
        match self {
            TransformKind::Reflow    => "--reflow",
            TransformKind::Unwrap    => "--unwrap",
            TransformKind::Ventilate => "--ventilate",
        }
    }
    /// Past-tense success word: "reflowed" / "unwrapped" / "ventilated".
    pub fn past_tense(self) -> &'static str {
        match self { Self::Reflow => "reflowed", Self::Unwrap => "unwrapped", Self::Ventilate => "ventilated" }
    }
    /// Gerund for in-progress: "reflowing" / "unwrapping" / "ventilating".
    pub fn gerund(self) -> &'static str {
        match self { Self::Reflow => "reflowing", Self::Unwrap => "unwrapping", Self::Ventilate => "ventilating" }
    }
}

#[derive(Debug)]
pub enum TransformError { Repar(String), OutputTooLarge { limit: usize }, Panicked(String) }

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransformError::Repar(m) => write!(f, "{m}"),
            TransformError::OutputTooLarge { limit } => write!(f, "transform output too large (> {limit} bytes)"),
            // No "transform failed" prefix here — the status renderer adds it (the
            // `format!("transform failed: {e}")` site), matching the other variants' Display.
            TransformError::Panicked(m) => write!(f, "internal error: {m}"),
        }
    }
}

impl TransformError {
    fn from_repar(e: repar::ParError) -> TransformError { TransformError::Repar(e.to_string()) }
}

/// Run a transform body, mapping a panic in untrusted (`repar`) code to a recoverable error.
fn guarded_transform(work: impl FnOnce() -> Result<String, TransformError>) -> Result<String, TransformError> {
    match crate::panicx::catch(work) {
        Ok(r) => r,
        Err(msg) => Err(TransformError::Panicked(msg)),
    }
}

use wordcartel_core::block_tree::TextSource;

/// Extend a unit span to the start of its first line — nested-item and quote spans
/// exclude their leading indent/prefix bytes, and a mid-line slice makes repar
/// reflow at the wrong content column (spec C2 D1, Fable r5 C1).
fn extend_to_line_start(text: impl TextSource, span: std::ops::Range<usize>) -> std::ops::Range<usize> {
    text.line_start(span.start)..span.end
}

/// The deepest ListItem beneath `node` whose span STARTS exactly at `at` — the
/// N5 line-keyed refinement's target (the line's first non-whitespace content
/// begins this item; List spans start at the same byte, so descend through them).
fn item_starting_at(node: &wordcartel_core::block_tree::Block, at: usize) -> Option<&wordcartel_core::block_tree::Block> {
    for c in &node.children {
        if at >= c.span.start && at < c.span.end {
            if matches!(c.kind, wordcartel_core::block_tree::BlockKind::ListItem) && c.span.start == at {
                return Some(c);
            }
            if let Some(f) = item_starting_at(c, at) {
                return Some(f);
            }
        }
    }
    None
}

/// The transform UNIT enclosing `pos` (spec C2 D1): the nearest ListItem on the
/// descent path (marker included), else the nearest BlockQuote, else the deepest
/// leaf. None when `pos` sits on a blank line inside a container (a gap — never
/// snap structural blanks to whole containers) or matches nothing. Non-blank
/// container-interior bytes resolve via the same preference set, with the N5
/// refinement: when the byte's line's first non-whitespace content begins a
/// ListItem at any depth beneath the descent's final node, THAT item is the unit
/// — Home on a nested item's line transforms the item the eye is on. Every
/// returned span is extended to its line start.
fn transform_unit_at(
    text: impl TextSource + Copy,
    blocks: &wordcartel_core::block_tree::BlockTree,
    pos: usize,
) -> Option<std::ops::Range<usize>> {
    let mut path: Vec<&wordcartel_core::block_tree::Block> = vec![&blocks.root];
    loop {
        let node = *path.last().expect("path is never empty");
        match node.children.iter().find(|c| pos >= c.span.start && pos < c.span.end) {
            Some(c) => path.push(c),
            None => break,
        }
    }
    let last = *path.last().expect("path is never empty");
    // The ROOT is never a leaf: the degraded-parse fallback (empty_tree,
    // block_tree.rs:333-335) yields a childless Document root — treating it as a
    // leaf would return 0..len, the whole-buffer transform this effort kills
    // (Fable plan C2; the container branch correctly yields None instead).
    let in_leaf = path.len() > 1
        && last.children.is_empty()
        && pos >= last.span.start
        && pos < last.span.end;
    let nearest = |kind_test: fn(&wordcartel_core::block_tree::BlockKind) -> bool| {
        path.iter().rev().find(|b| kind_test(&b.kind)).map(|b| b.span.clone())
    };
    if in_leaf {
        let unit = nearest(|k| matches!(k, wordcartel_core::block_tree::BlockKind::ListItem))
            .or_else(|| nearest(|k| matches!(k, wordcartel_core::block_tree::BlockKind::BlockQuote)))
            .unwrap_or_else(|| last.span.clone());
        return Some(extend_to_line_start(text, unit));
    }
    // Container-interior (or unmatched) byte: discriminate by line blankness.
    let ls = text.line_start(pos);
    let le = text.line_end(pos);
    let line = text.slice(ls..le);
    if line.trim().is_empty() {
        return None; // structural blank = gap, regardless of ancestors (spec r3/r5)
    }
    // N5 line-keyed refinement (user-ratified A, r7 P1 wording).
    let first_content = ls + (line.len() - line.trim_start().len());
    if let Some(item) = item_starting_at(last, first_content) {
        return Some(extend_to_line_start(text, item.span.clone()));
    }
    nearest(|k| matches!(k, wordcartel_core::block_tree::BlockKind::ListItem))
        .or_else(|| nearest(|k| matches!(k, wordcartel_core::block_tree::BlockKind::BlockQuote)))
        .map(|span| extend_to_line_start(text, span))
}

/// Snap a non-empty selection's ENDPOINTS to their transform units (spec C2 D2):
/// start = the unit at `from` (its extended start), end = the unit at the last
/// selected byte (its end); gap endpoints stay raw. The interior rides between.
pub fn snap_to_blocks(
    text: impl TextSource + Copy,
    blocks: &wordcartel_core::block_tree::BlockTree,
    from: usize,
    to: usize,
) -> std::ops::Range<usize> {
    let start = transform_unit_at(text, blocks, from).map(|u| u.start).unwrap_or(from);
    let end = transform_unit_at(text, blocks, to.saturating_sub(1)).map(|u| u.end).unwrap_or(to);
    if start < end { start..end } else { from..to }
}

/// The byte range a transform should reformat: the transform unit under the
/// caret when the primary selection is empty (an empty range on a gap — the
/// dispatch guard turns that into "nothing to transform"), else the selection
/// endpoint-snapped to whole units.
pub fn region_for_transform(doc: &crate::editor::Document) -> std::ops::Range<usize> {
    let sel = doc.selection.primary();
    let buf_len = doc.buffer.len();
    let snapshot = doc.buffer.snapshot();
    if sel.is_empty() {
        let caret = sel.from().min(buf_len.saturating_sub(1));
        transform_unit_at(&snapshot, doc.blocks(), caret).unwrap_or(sel.from()..sel.from())
    } else {
        snap_to_blocks(&snapshot, doc.blocks(), sel.from(), sel.to())
    }
}

use std::ops::Range;

/// Run a transform over the active buffer's resolved region.
/// For regions >= TRANSFORM_ASYNC_THRESHOLD bytes, runs off the keystroke thread
/// and sends Msg::TransformDone; smaller regions run synchronously.
/// `clock` is the same &dyn Clock that resolve_prompt receives.
pub fn dispatch_transform(
    editor: &mut crate::editor::Editor,
    kind: TransformKind,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    if editor.transform_in_flight {
        editor.status = "a transform is already running".into();
        return;
    }
    let range = region_for_transform(&editor.active().document);
    if range.is_empty() {
        editor.status = "nothing to transform".into();
        return;
    }
    if range.len() >= TRANSFORM_ASYNC_THRESHOLD {
        let buffer_id = editor.active().id;
        let version = editor.active().document.version;
        let snapshot = editor.active().document.buffer.snapshot(); // O(1) rope snapshot
        editor.transform_in_flight = true;
        editor.status = format!("{}…", kind.gerund());
        let range_c = range.clone();
        let msg_tx = msg_tx.clone();
        std::thread::spawn(move || {
            let input = snapshot.byte_slice(range_c.clone()).to_string();
            let result = guarded_transform(|| run_transform(kind, &input, DEFAULT_REFLOW_WIDTH));
            let _ = msg_tx.send(crate::app::Msg::TransformDone {
                buffer_id, version, range: range_c, kind, result,
            });
        });
        return;
    }
    // Sync branch: region is small enough to run on the keystroke thread.
    let input = editor.active().document.buffer.slice(range.clone()).to_string();
    let result = guarded_transform(|| run_transform(kind, &input, DEFAULT_REFLOW_WIDTH));
    apply_transform_result(editor, kind, range, result, clock);
}

/// Shared merge body used by BOTH the sync path and the async path (via
/// `apply_transform_done` in `jobs_apply.rs`). Targets `buffer_id` (not necessarily
/// active) so both callers route correctly.
///
/// Active-buffer guard: `derive::rebuild` and `nav::ensure_visible` operate on
/// `editor.active()` ONLY. We call them only when the merged buffer IS the
/// active buffer, future-proofing for Effort 6 multi-buffer.
pub fn merge_transform_into(
    editor: &mut crate::editor::Editor,
    buffer_id: crate::editor::BufferId,
    kind: TransformKind,
    range: Range<usize>,
    result: Result<String, TransformError>,
    clock: &dyn wordcartel_core::history::Clock,
) {
    match result {
        Err(e) => {
            editor.status = format!("transform failed: {e}");
        }
        Ok(out) => {
            // Read the current bytes for the no-op check; borrow ends before apply.
            let current = editor.by_id(buffer_id)
                .map(|b| b.document.buffer.slice(range.clone()).to_string())
                .unwrap_or_default();
            if out == current {
                editor.status = format!("already {}", kind.past_tense());
                return;
            }
            let doc_len = editor.by_id(buffer_id).map(|b| b.document.buffer.len()).unwrap_or(0);
            let (cs, edit) = crate::commands::build_range_replace(range.start, range.end, &out, doc_len);
            let txn = wordcartel_core::history::Transaction::new(cs);
            // Apply via by_id_mut — borrow ends before derive/nav calls below.
            if let Some(b) = editor.by_id_mut(buffer_id) {
                b.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
            } else {
                return; // buffer disappeared mid-flight
            }
            // derive::rebuild / ensure_visible are active-buffer-only.
            if buffer_id == editor.active().id {
                crate::derive::rebuild(editor);
                crate::nav::ensure_visible(editor);
            }
            editor.status = kind.past_tense().to_string();
        }
    }
}

/// Sync-path entry point (called by dispatch_transform for small regions).
/// Delegates to merge_transform_into targeting the active buffer.
pub fn apply_transform_result(
    editor: &mut crate::editor::Editor,
    kind: TransformKind,
    range: Range<usize>,
    result: Result<String, TransformError>,
    clock: &dyn wordcartel_core::history::Clock,
) {
    let buffer_id = editor.active().id;
    merge_transform_into(editor, buffer_id, kind, range, result, clock);
}

fn check_output_size(out: String) -> Result<String, TransformError> {
    if out.len() > crate::limits::MAX_TRANSFORM_OUTPUT {
        Err(TransformError::OutputTooLarge { limit: crate::limits::MAX_TRANSFORM_OUTPUT })
    } else { Ok(out) }
}

/// Run a repar transform over `input`, markdown-aware. Pure (no IO).
pub fn run_transform(kind: TransformKind, input: &str, width: u32) -> Result<String, TransformError> {
    let mut opts = repar::Options::new().width(width);
    // apply_par_args takes &mut self and returns PResult<()> — not chainable.
    opts.apply_par_args([kind.verb()]).map_err(TransformError::from_repar)?;
    opts.apply_fixups("markdown").map_err(TransformError::from_repar)?; // Compat::MARKDOWN
    let out = opts.format(input).map_err(TransformError::from_repar)?;
    check_output_size(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    fn blocks_of(text: &str) -> wordcartel_core::block_tree::BlockTree {
        Editor::new_from_text(text, None, (80, 24)).active().document.blocks().clone()
    }

    // Exact-span discipline (Codex plan review, I-3): assert the snapped range
    // EQUALS the block tree's own span(s), so there is no off-by-one that
    // build_range_replace would turn into a dropped/duplicated newline. We compare
    // against `top_level()[i].span` (whatever the parser produced) rather than
    // hand-counted byte offsets.

    #[test]
    fn snap_expands_mid_paragraph_selection_to_exactly_the_paragraph_block() {
        let text = "alpha beta gamma\ndelta epsilon zeta\n\nsecond para\n";
        let bt = blocks_of(text);
        let para0 = bt.top_level()[0].span.clone(); // the first paragraph block
        // Selection lands inside the first paragraph (bytes 5..9 = "beta").
        let r = snap_to_blocks(text, &bt, 5, 9);
        assert_eq!(r, para0, "snap must equal the first paragraph block's exact span");
        // …and must not reach into the second paragraph block.
        assert!(r.end <= bt.top_level()[1].span.start);
    }

    #[test]
    fn snap_inside_fenced_code_block_with_interior_blank_covers_whole_fence() {
        // The CRITICAL case: a blank line INSIDE a fenced code block. The parser
        // emits ONE FencedCode block spanning the interior blank; snapping must
        // return exactly that block's span (opener through closer), never a fragment.
        let text = "```\ncode line one\n\nstill code\n```\n\nprose after\n";
        let bt = blocks_of(text);
        let fence = bt.top_level()[0].span.clone();
        // Sanity: the fence block really spans the interior blank.
        assert!(text[fence.clone()].starts_with("```"), "block 0 is the fence");
        assert!(text[fence.clone()].contains("\n\nstill code"), "fence spans the interior blank");
        // A selection on "still code" (after the interior blank) snaps to the WHOLE fence.
        let sel_from = text.find("still").unwrap();
        let r = snap_to_blocks(text, &bt, sel_from, sel_from + 5);
        assert_eq!(r, fence, "must snap to exactly the whole fenced block");
    }

    #[test]
    fn snap_multi_block_selection_covers_exactly_the_touched_blocks() {
        let text = "para one here\n\npara two here\n\npara three\n";
        let bt = blocks_of(text);
        let p0 = bt.top_level()[0].span.clone();
        let p1 = bt.top_level()[1].span.clone();
        let p2 = bt.top_level()[2].span.clone();
        // Selection spans from inside para one to inside para two.
        let from = 5;
        let to = text.find("two").unwrap() + 1;
        let r = snap_to_blocks(text, &bt, from, to);
        assert_eq!(r, p0.start..p1.end, "snap must be exactly the union of the two touched blocks");
        assert!(r.end <= p2.start, "must not reach the untouched third block");
    }

    #[test]
    fn snap_with_no_intersecting_block_returns_input_range() {
        let text = "only para\n";
        let bt = blocks_of(text);
        // A range past the end intersects nothing → unchanged.
        let r = snap_to_blocks(text, &bt, 100, 105);
        assert_eq!(r, 100..105);
    }

    #[test]
    fn reflow_wraps_long_prose_within_width() {
        let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega";
        let out = run_transform(TransformKind::Reflow, long, 72).unwrap();
        for line in out.lines() {
            // repar::display_width(s, start_col, tab, compat) — 4 args (width.rs).
            let cols = repar::display_width(line, 0, 8, repar::Compat::empty());
            assert!(cols <= 72, "line over width ({cols}): {line:?}");
        }
        // Round-trip back to words: unwrapping the reflow yields one line with the same words.
        let unwrapped = run_transform(TransformKind::Unwrap, &out, 72).unwrap();
        assert_eq!(unwrapped.split_whitespace().collect::<Vec<_>>(),
                   long.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn unwrap_joins_a_wrapped_paragraph_to_one_logical_line() {
        let wrapped = "one two three\nfour five six\nseven eight\n";
        let out = run_transform(TransformKind::Unwrap, wrapped, 72).unwrap();
        // One paragraph → one non-empty logical line.
        assert_eq!(out.lines().filter(|l| !l.trim().is_empty()).count(), 1);
        assert_eq!(out.split_whitespace().collect::<Vec<_>>(),
                   wrapped.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn ventilate_breaks_one_sentence_per_line() {
        let para = "First sentence here. Second sentence here. Third one here.\n";
        let out = run_transform(TransformKind::Ventilate, para, 72).unwrap();
        assert_eq!(out.lines().filter(|l| !l.trim().is_empty()).count(), 3);
    }

    #[test]
    fn markdown_mode_passes_fenced_code_through_verbatim() {
        // A long line INSIDE a fenced code block must NOT be reflowed/wrapped.
        let long_code = "let x = aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;";
        let input = format!("```\n{long_code}\n```\n");
        let out = run_transform(TransformKind::Reflow, &input, 72).unwrap();
        assert!(out.contains(long_code), "fenced code line must survive verbatim:\n{out}");
    }

    #[test]
    fn markdown_mode_leaves_heading_unwrapped() {
        let input = "# A heading that is fairly long but is a heading not prose\n\nbody text\n";
        let out = run_transform(TransformKind::Reflow, input, 72).unwrap();
        assert!(out.contains("# A heading that is fairly long but is a heading not prose"),
                "heading must pass through:\n{out}");
    }

    #[test]
    fn transform_output_over_cap_refused() {
        let big = "x".repeat(crate::limits::MAX_TRANSFORM_OUTPUT + 1);
        assert!(matches!(check_output_size(big), Err(TransformError::OutputTooLarge { .. })));
        let ok = "small".to_string();
        assert!(check_output_size(ok).is_ok());
    }

    #[test]
    fn guarded_transform_maps_panic_to_error() {
        let r = guarded_transform(|| panic!("kaboom"));
        assert!(matches!(r, Err(TransformError::Panicked(ref m)) if m == "kaboom"));
        let ok = guarded_transform(|| Ok("hi".to_string()));
        assert_eq!(ok.unwrap(), "hi");
    }

    // ---------------------------------------------------------------------------
    // transform_unit_at — unit lookup battery (spec C2 D1, Task 1)
    // ---------------------------------------------------------------------------

    #[test]
    fn caret_region_is_the_transform_unit() {
        // Item body → the ITEM span (marker included), line-start-extended (§B.1:
        // inner item span 14..84 excludes the 2-space indent; the unit is 12..84).
        let text = "- outer one\n  - inner one two three four five six seven\n    continuation words here\n";
        let bt = blocks_of(text);
        let outer = &bt.top_level()[0].children[0];            // outer ListItem
        let inner_list = outer.children.iter().find(|c| matches!(c.kind, wordcartel_core::block_tree::BlockKind::List)).unwrap();
        let inner = &inner_list.children[0];                    // inner ListItem
        assert_eq!(inner.span.start, 14, "precondition: item span excludes indent");
        let u = transform_unit_at(text, &bt, 20).expect("caret in inner body");
        assert_eq!(u, 12..inner.span.end, "nearest ListItem, extended to line start");
        // Plain paragraph → its own leaf span.
        let text2 = "para one here\n\npara two here\n";
        let bt2 = blocks_of(text2);
        let p0 = bt2.top_level()[0].span.clone();
        assert_eq!(transform_unit_at(text2, &bt2, 3), Some(p0));
    }

    #[test]
    fn transform_unit_in_item_body_is_the_item_not_the_paragraph() {
        // Loose item: the body sits in a Paragraph CHILD (2..13, probe) — the unit must be
        // the ITEM (0..14, trailing blank included per anatomy), never the paragraph.
        let text = "- alpha item\n\n- beta item\n";
        let bt = blocks_of(text);
        let item1 = &bt.top_level()[0].children[0];
        assert!(item1.children.iter().any(|c| matches!(c.kind, wordcartel_core::block_tree::BlockKind::Paragraph)),
            "precondition: loose item wraps its text in a Paragraph");
        let u = transform_unit_at(text, &bt, 5).expect("caret in alpha body");
        assert_eq!(u, item1.span.clone(), "the item, marker and trailing blank included");
    }

    #[test]
    fn transform_unit_in_nested_item_is_the_deepest_item() {
        let text = "- outer one\n  - inner one two three four five six seven\n    continuation words here\n";
        let bt = blocks_of(text);
        let u = transform_unit_at(text, &bt, 60).expect("caret in the continuation line");
        assert!(u.start == 12, "the INNER item (line-start-extended), not the outer: {u:?}");
    }

    #[test]
    fn caret_region_in_gap_is_none_and_blankline_gaps() {
        // Top-level gap (blank between paragraphs) → None.
        let text = "para one\n\npara two\n";
        let bt = blocks_of(text);
        assert_eq!(transform_unit_at(text, &bt, 9), None, "top-level blank");
        // Loose-list trailing blank (byte 13 — INSIDE item 1's span per anatomy) →
        // container descent + blank line → None.
        let text2 = "- alpha item\n\n- beta item\n";
        let bt2 = blocks_of(text2);
        assert_eq!(transform_unit_at(text2, &bt2, 13), None, "loose blank is a gap");
    }

    #[test]
    fn caret_on_loose_item_marker_transforms_the_item() {
        // Marker bytes (0..2) are container-interior on a NON-blank line → the item.
        let text = "- alpha item\n\n- beta item\n";
        let bt = blocks_of(text);
        let item1 = bt.top_level()[0].children[0].span.clone();
        assert_eq!(transform_unit_at(text, &bt, 0), Some(item1));
    }

    #[test]
    fn caret_in_tight_item_lead_text_transforms_the_item() {
        // Tight item lead text (bytes 2..11) has NO Paragraph child → outer item 0..84.
        let text = "- outer one\n  - inner one two three four five six seven\n    continuation words here\n";
        let bt = blocks_of(text);
        let outer = bt.top_level()[0].children[0].span.clone();
        assert_eq!(transform_unit_at(text, &bt, 5), Some(outer));
    }

    #[test]
    fn caret_in_nested_item_indent_transforms_the_child_item() {
        // The N5 line-keyed refinement, all three ratified shapes:
        // (a) first-nested indent (§B.5: bytes 8-9 → inner item 10..18 → unit 8..18);
        let text = "- outer\n  - inner\n";
        let bt = blocks_of(text);
        let inner_first = &bt.top_level()[0].children[0]
            .children.iter().find(|c| matches!(c.kind, wordcartel_core::block_tree::BlockKind::List))
            .and_then(|l| l.children.first()).expect("nested item").span;
        assert_eq!(*inner_first, 10..18, "precondition (probe-verified)");
        let u = transform_unit_at(text, &bt, 8).expect("indent byte");
        assert_eq!(u, 8..18, "the INNER item, line-start-extended");
        // (b) space-indented TOP-LEVEL item " - a" (precondition-assert it parses as a list);
        let text2 = " - a\n";
        let bt2 = blocks_of(text2);
        assert!(matches!(bt2.top_level()[0].kind, wordcartel_core::block_tree::BlockKind::List),
            "precondition: 1-space indent still a list");
        let item = bt2.top_level()[0].children[0].span.clone();
        assert_eq!(transform_unit_at(text2, &bt2, 0), Some(0..item.end));
        // (c) TAB-indented NESTED item — probe-verified corpus (Fable plan C1: for
        // "- x\n\t- a\n" pulldown starts the inner span at the PREVIOUS newline —
        // mid-tab, unsplittable — so THAT shape degrades; use the ordered-outer form
        // whose inner span starts on the tab line).
        let text3 = "1. x\n\t- a\n";
        let bt3 = blocks_of(text3);
        let outer3 = &bt3.top_level()[0].children[0];
        let inner3 = outer3.children.iter().find(|c| matches!(c.kind, wordcartel_core::block_tree::BlockKind::List))
            .and_then(|l| l.children.first()).expect("nested item exists");
        assert_eq!(inner3.span.start, 5, "precondition: inner span starts ON the tab line");
        let u3 = transform_unit_at(text3, &bt3, 5).expect("tab indent byte");
        assert_eq!(u3, 5..inner3.span.end, "the nested item, line-start-extended");
        // (d) The DEGRADATION pin (user-ratified 2026-07-05): the mid-tab shape's
        // inner span starts at the previous newline → the unit is the OUTER item.
        let text4 = "- x\n\t- a\n";
        let bt4 = blocks_of(text4);
        let outer4 = bt4.top_level()[0].children[0].span.clone();
        let u4 = transform_unit_at(text4, &bt4, 4).expect("tab byte");
        assert_eq!(u4, 0..outer4.end, "mid-tab span shape degrades to the outer item — accepted");
    }

    #[test]
    fn caret_in_quote_body_is_the_blockquote_and_item_beats_quote() {
        // Bare quote → the BlockQuote span (a bare Paragraph slice mixes "> " prefixes).
        let text = "> quoted line one\n> quoted line two\n";
        let bt = blocks_of(text);
        let q = bt.top_level()[0].span.clone();
        assert_eq!(transform_unit_at(text, &bt, 5), Some(q));
        // Quote nested in an item → the ITEM wins (ListItem beats BlockQuote).
        let text2 = "- outer\n  > quoted line\n";
        let bt2 = blocks_of(text2);
        let item = bt2.top_level()[0].children[0].span.clone();
        let u2 = transform_unit_at(text2, &bt2, 14).expect("caret in nested quote body");
        assert_eq!(u2.end, item.end, "the outer item's unit, not the inner quote");
    }

    #[test]
    fn degraded_parse_caret_is_nothing_to_transform() {
        // Fable plan C2: a childless Document root (the M4-rest panic fallback's
        // empty_tree shape) must yield None — never the whole buffer. Build the
        // degenerate tree by hand (Block fields are pub).
        let text = "some text here ok\n";
        let bt = wordcartel_core::block_tree::BlockTree {
            root: wordcartel_core::block_tree::Block {
                kind: wordcartel_core::block_tree::BlockKind::Document,
                span: 0..text.len(),
                children: Vec::new(),
            },
        };
        assert_eq!(transform_unit_at(text, &bt, 5), None, "degraded parse: no unit, the guard says nothing-to-transform");
    }

    #[test]
    fn caret_region_at_end_of_buffer_clamps() {
        let text = "only para\n";
        let bt = blocks_of(text);
        // region_for_transform clamps caret==buf_len to the last byte.
        let mut e = Editor::new_from_text(text, None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(text.len());
        let r = region_for_transform(&e.active().document);
        assert_eq!(r, bt.top_level()[0].span.clone());
    }

    // ---------------------------------------------------------------------------
    // snap_to_blocks — endpoint-unit snapping pins (spec C2 D2, Task 1)
    // ---------------------------------------------------------------------------

    #[test]
    fn snap_inside_one_list_item_touches_only_that_item() {
        // Three-item tight list — a selection wholly inside item 1 (middle item)
        // must snap to exactly that item's span; items 0 and 2 must not be touched.
        let text = "- one\n- two\n- three\n";
        let bt = blocks_of(text);
        let list = &bt.top_level()[0];
        assert_eq!(list.children.len(), 3, "precondition: three tight list items");
        let item0 = list.children[0].span.clone();
        let item1 = list.children[1].span.clone();
        let item2 = list.children[2].span.clone();
        assert_eq!(item0.end, item1.start, "precondition: items abut");
        assert_eq!(item1.end, item2.start, "precondition: items abut");
        // selection from inside item 1's text to its end
        let from = item1.start + 2; // past "- "
        let to = item1.end;
        let r = snap_to_blocks(text, &bt, from, to);
        assert_eq!(r.start, item1.start, "start snaps to item 1 start");
        assert_eq!(r.end, item1.end, "end snaps to item 1 end");
        assert!(r.start >= item0.end, "does not reach into item 0");
        assert!(r.end <= item2.start, "does not reach into item 2");
    }

    #[test]
    fn snap_across_three_items_touches_exactly_those() {
        // Five-item tight list — select from inside item 0 to inside item 2;
        // the snap must cover exactly items 0-2 and leave items 3 and 4 untouched.
        let text = "- aa\n- bb\n- cc\n- dd\n- ee\n";
        let bt = blocks_of(text);
        let list = &bt.top_level()[0];
        assert_eq!(list.children.len(), 5, "precondition: five items");
        let item0 = list.children[0].span.clone();
        let item2 = list.children[2].span.clone();
        let item3 = list.children[3].span.clone();
        let from = item0.start + 2; // inside item 0's text
        let to = item2.start + 3;   // inside item 2's text
        let r = snap_to_blocks(text, &bt, from, to);
        assert_eq!(r.start, item0.start, "start snaps to item 0");
        assert_eq!(r.end, item2.end, "end snaps to item 2 end");
        assert!(r.end <= item3.start, "item 3 not touched");
    }

    #[test]
    fn snap_paragraph_into_list_unions_endpoints() {
        // Paragraph followed by a list — from inside the paragraph, to inside
        // the first list item; the snap must cover the paragraph start through item 0 end.
        let text = "a paragraph\n\n- list item\n";
        let bt = blocks_of(text);
        let para = bt.top_level()[0].span.clone();
        let list = &bt.top_level()[1];
        let item0 = list.children[0].span.clone();
        let from = para.start + 2;  // inside paragraph
        let to = item0.start + 3;   // inside item 0
        let r = snap_to_blocks(text, &bt, from, to);
        assert_eq!(r.start, para.start, "start snaps to paragraph start");
        assert_eq!(r.end, item0.end, "end snaps to item 0 end");
    }

    #[test]
    fn snap_selection_wholly_in_gap_returns_input() {
        // Both endpoints on the blank line between two paragraphs → no unit found →
        // from..to returned unchanged.
        let text = "para one\n\npara two\n";
        let bt = blocks_of(text);
        // byte 9 is '\n' (the blank line between paragraphs)
        assert_eq!(&text[9..10], "\n", "precondition: byte 9 is the gap");
        let from = 9;
        let to = 10;
        let r = snap_to_blocks(text, &bt, from, to);
        assert_eq!(r, from..to, "wholly in gap returns input unchanged");
    }

    #[test]
    fn snap_endpoint_on_loose_list_blank_is_gap_not_container() {
        // §B.2 corpus: loose two-item list. The trailing blank of item 1 is a gap —
        // an endpoint landing there must NOT pull in item 2 or extend to the container.
        let text = "- alpha item\n\n- beta item\n";
        let bt = blocks_of(text);
        let list = &bt.top_level()[0];
        let item1 = list.children[0].span.clone();
        let item2 = list.children[1].span.clone();
        // precondition: item 1's span includes the trailing blank (anatomy: 0..14)
        assert!(item1.end > 13, "precondition: item 1 span includes the trailing blank");
        // from=5 (inside item 1 body), to=14 (one past the blank byte at 13)
        let r = snap_to_blocks(text, &bt, 5, 14);
        // start snaps to item 1 (line-start-extended = 0); end is on blank → raw → 14
        assert_eq!(r.start, item1.start, "start snaps to item 1 start");
        assert_eq!(r.end, 14, "blank endpoint stays raw");
        assert!(r.end <= item2.start, "item 2 not pulled in");
    }

    #[test]
    fn snap_endpoint_on_nested_list_interitem_blank_is_gap() {
        // §B.4 corpus: outer item with a loose nested sub-list. The blank between
        // the two inner items is a gap — an endpoint there must NOT pull in the outer item.
        let text = "- outer\n  - a\n\n  - b\n";
        let bt = blocks_of(text);
        let outer = &bt.top_level()[0].children[0]; // outer ListItem
        // precondition: byte 14 is the blank line between inner items
        assert_eq!(&text[14..15], "\n", "precondition: byte 14 is the blank line");
        let inner_list = outer.children.iter()
            .find(|c| matches!(c.kind, wordcartel_core::block_tree::BlockKind::List))
            .expect("outer has a nested List");
        let inner_a = &inner_list.children[0];
        // precondition: inner_a's span reaches at least to the blank byte
        assert!(inner_a.span.end >= 14, "precondition: inner_a span covers the blank boundary");
        // snap: from inside inner_a, to just past the blank line (byte 15)
        let from = inner_a.span.start;
        let to = 15; // one past the blank
        let r = snap_to_blocks(text, &bt, from, to);
        // to.saturating_sub(1) == 14 (blank) → None → end stays raw at 15
        assert_eq!(r.end, to, "blank endpoint stays raw — outer item not pulled in");
        assert!(r.end < outer.span.end, "outer item end not pulled in");
    }
}
