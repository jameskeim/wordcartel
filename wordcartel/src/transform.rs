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

use wordcartel_core::block_tree::BlockTree;

/// Expand `[from,to)` to cover every top-level block whose span intersects it.
/// If no block intersects, the range is returned unchanged. Half-open intervals;
/// callers pass a non-empty selection (from < to). The block tree is already
/// maintained per buffer (Effort 3) — this is a bounded scan, not a parse.
pub fn snap_to_blocks(blocks: &BlockTree, from: usize, to: usize) -> std::ops::Range<usize> {
    let mut start = from;
    let mut end = to;
    let mut found = false;
    for b in blocks.top_level() {
        // intersection of [from,to) and [span.start, span.end)
        if b.span.start < to && from < b.span.end {
            if !found {
                start = b.span.start;
                end = b.span.end;
                found = true;
            } else {
                start = start.min(b.span.start);
                end = end.max(b.span.end);
            }
        }
    }
    if found { start..end } else { from..to }
}

/// The byte range a transform should reformat: whole buffer when the primary
/// selection is empty, else the selection snapped to whole blocks.
pub fn region_for_transform(doc: &crate::editor::Document) -> std::ops::Range<usize> {
    let sel = doc.selection.primary();
    let buf_len = doc.buffer.len(); // TextBuffer::len() is the byte length (buffer.rs)
    if sel.is_empty() {
        0..buf_len
    } else {
        snap_to_blocks(doc.blocks(), sel.from(), sel.to())
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
/// `apply_transform_done` in app.rs). Targets `buffer_id` (not necessarily
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
        let r = snap_to_blocks(&bt, 5, 9);
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
        let r = snap_to_blocks(&bt, sel_from, sel_from + 5);
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
        let r = snap_to_blocks(&bt, from, to);
        assert_eq!(r, p0.start..p1.end, "snap must be exactly the union of the two touched blocks");
        assert!(r.end <= p2.start, "must not reach the untouched third block");
    }

    #[test]
    fn snap_with_no_intersecting_block_returns_input_range() {
        let text = "only para\n";
        let bt = blocks_of(text);
        // A range past the end intersects nothing → unchanged.
        let r = snap_to_blocks(&bt, 100, 105);
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
}
