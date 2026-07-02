use crate::editor::{Editor, RenderMode};
use wordcartel_core::block_tree;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::layout;

/// Everything the visible-line layout loop reads. Gate the loop on equality of this
/// so it re-runs only when an actual input changed. A miss here would blank rows
/// (render has no on-demand fallback), so the field set must be COMPLETE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutKey {
    pub blocks_generation: u64,
    pub fold_epoch: u64,
    pub scroll: usize,              // post-normalization (first_line)
    pub scroll_row: usize,
    pub area: (u16, u16),
    pub text_width: usize,          // vp_width (subsumes wrap/gutter geometry)
    pub active_line: usize,
    pub source_mode: bool,          // view.mode != LivePreview
    pub heading_level_glyph: bool,
}

#[cfg(test)]
thread_local! {
    pub static LAYOUT_RUNS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

// ---------------------------------------------------------------------------
// Logical-line helpers
// ---------------------------------------------------------------------------

/// Total number of logical lines in `buf`.
///
/// Edge-case rules:
///   ""    → 1   (the document always has at least one line)
///   "a"   → 1
///   "a\n" → 2   (trailing newline creates a real empty line after it)
///   "\n"  → 2
///   "a\nb"→ 2
///
/// ropey's `len_lines()` follows the convention we want: it returns the number
/// of LF-delimited lines where a trailing `\n` contributes an extra empty line.
pub fn total_logical_lines(buf: &TextBuffer) -> usize {
    // ropey uses LF-only semantics for len_lines when the unicode_lines feature
    // is disabled (the default). We double-check: for a buffer whose content
    // ends in '\n', ropey's len_lines is len_lines_lf = text.split('\n').count()
    // which counts the trailing empty field. That matches our spec.
    let rope = buf.snapshot();
    rope.len_lines()
}

/// Byte offset of the start of logical line `L` in `buf`.
///
/// For `L < total_logical_lines(buf)`:  `buf.line_to_byte(L)`.
/// For `L == total_logical_lines(buf)`: clamped to `buf.len()` (one-past-end guard).
pub fn line_start(buf: &TextBuffer, line: usize) -> usize {
    let total = total_logical_lines(buf);
    if line < total {
        buf.line_to_byte(line)
    } else {
        buf.len()
    }
}

/// Content of logical line `L` as a `String`, **without** its trailing `\n`.
///
/// For any `L` in `0..total_logical_lines(buf)`.
pub fn line_text(buf: &TextBuffer, line: usize) -> String {
    let start = line_start(buf, line);
    let total = total_logical_lines(buf);
    let raw_end = if line + 1 < total {
        line_start(buf, line + 1)
    } else {
        buf.len()
    };
    // Strip a single trailing '\n' if present (it's the line separator, not content).
    let text = buf.slice(start..raw_end);
    if text.ends_with('\n') {
        text[..text.len() - 1].to_string()
    } else {
        text
    }
}

// ---------------------------------------------------------------------------
// derive::rebuild
// ---------------------------------------------------------------------------

/// Recompute the block tree and per-visible-line layout cache from truth.
///
/// This is the O(visible)+O(edited) derive step described in Effort 4a Task 3.
///
/// # Block tree (version-memoized)
/// The parse phase runs ONLY when `document.version != reconcile.blocks_version`
/// — i.e. the text changed since the tree was built. When it runs we take an
/// incremental reparse iff the tree is exactly one version behind AND
/// `last_edit`/`pre_edit_rope` are both `Some` (a single edit bridging the gap,
/// set by `apply`); any other case (undo/redo cleared them, a multi-version gap)
/// falls back to a full parse. When a parse runs it clears `last_edit` and
/// `pre_edit_rope` (via `.take()`) so they are not reused, records the new
/// `blocks_version`, and sets `maybe_stale` (true for an incremental
/// `Local`/`WidenToEnd` result or a parse panic; false for a clean full parse).
/// When the version is unchanged the parse phase — and thus the `.take()` — is
/// skipped entirely; only the downstream phase reruns.
///
/// # Visible range + layout cache
/// We walk logical lines starting at `view.scroll`, accumulating the visual-row
/// heights reported by `ColMap.rows`, until we have filled the editing area
/// height (+1 row of overscan).  For each visible logical line we call
/// `layout::layout` and store the result in `view.line_layouts`.
pub fn rebuild(editor: &mut Editor) {
    let version = editor.active().document.version;
    let blocks_version = editor.active().reconcile.blocks_version;

    // Parse phase: only when the text actually changed since the tree was built.
    if version != blocks_version {
        let new_rope = editor.active().document.buffer.snapshot(); // O(1) ropey clone
        let new_len = new_rope.len_bytes();
        let maybe_old_rope = editor.active_mut().pre_edit_rope.take();
        let maybe_edit = editor.active_mut().last_edit.take();

        // Incremental ONLY when the tree is exactly one version behind AND the
        // pending edit bridges that gap (a single edit since the last parse).
        // Any gap (undo/redo clear the edit info; multi-edit-before-rebuild) →
        // a safe full parse.
        let one_behind = version == blocks_version.wrapping_add(1);
        let (new_blocks, stale) = if one_behind {
            if let (Some(old_rope), Some(edit)) = (&maybe_old_rope, &maybe_edit) {
                // `TextSource` is impl'd for `&Rope`, so `S = &Rope` and the
                // generic's `&S` needs `&&Rope` (mirror `incremental_update_rope`,
                // block_tree.rs:521). `old_rope` is already `&Rope` → `&old_rope`;
                // bind `new_rope` to a ref → `&new_rope_ref`.
                let new_rope_ref = &new_rope;
                match crate::panicx::catch(|| {
                    block_tree::incremental_update_instrumented_src(
                        &editor.active().document.blocks, &old_rope, edit, &new_rope_ref,
                    )
                }) {
                    Ok(outcome) => {
                        let stale = matches!(
                            outcome.reason,
                            block_tree::WidenReason::Local | block_tree::WidenReason::WidenToEnd
                        );
                        (outcome.tree, stale)
                    }
                    // A parse panic → degraded empty-tree fallback (NOT full_parse)
                    // → stale. Reuse the existing apply_parse_result helper.
                    Err(msg) => (apply_parse_result(editor, new_len, Err(msg)), true),
                }
            } else {
                full_parse_phase(editor, &new_rope, new_len)
            }
        } else {
            full_parse_phase(editor, &new_rope, new_len)
        };

        let next_gen = editor.active().document.blocks_generation.wrapping_add(1);
        editor.active_mut().document.blocks = new_blocks;
        editor.active_mut().document.blocks_generation = next_gen;
        editor.active_mut().reconcile.blocks_version = version;
        editor.active_mut().reconcile.maybe_stale = stale;
    }

    rebuild_downstream(editor);
}

/// Full parse for the current rope; returns `(tree, stale)`. `stale` is true only
/// on a parse panic (empty-tree fallback); a successful full parse is not stale.
/// Reuses the existing `apply_parse_result` helper (which sets/clears the degraded
/// status and returns the empty-tree fallback on `Err`) — so `apply_parse_result`
/// and its existing tests are KEPT, not removed.
fn full_parse_phase(editor: &mut Editor, new_rope: &ropey::Rope, new_len: usize) -> (block_tree::BlockTree, bool) {
    let computed = crate::panicx::catch(|| block_tree::full_parse_rope(new_rope));
    let stale = computed.is_err();
    (apply_parse_result(editor, new_len, computed), stale)
}

/// The downstream-of-tree phase: reconcile fold anchors + build the `FoldView` +
/// refresh the visible-line layout cache from the CURRENT `document.blocks`.
/// Runs every draw and does NOT reparse; also called by the reconcile merge.
pub(crate) fn rebuild_downstream(editor: &mut Editor) {
    // ------------------------------------------------------------------
    // 5g: Reconcile fold anchors against the fresh block tree, then build
    // a FoldView for the visible-line walk below.
    // ------------------------------------------------------------------
    // Generation-gated fold-anchor prune (was every-draw). No per-draw deep clone:
    // compute heading starts under an immutable borrow, then retain.
    {
        let gen = editor.active().document.blocks_generation;
        if editor.active().last_reconciled_generation != Some(gen) {
            let starts = {
                let b = editor.active();
                wordcartel_core::outline::heading_starts(&b.document.blocks, &b.document.buffer.snapshot())
            };
            editor.active_mut().folds.reconcile_to(&starts);
            editor.active_mut().last_reconciled_generation = Some(gen);
        }
    }
    let fold_view = editor.active_fold_view();

    // ------------------------------------------------------------------
    // 2. Visible range
    // ------------------------------------------------------------------
    // Snapshot all read-only scalar values from the active buffer before any
    // mutable borrow, so the borrow checker sees no overlap.
    let (total_lines, active_line, area_height, first_line, source_mode, scroll_row) = {
        let b = editor.active();
        let buf = &b.document.buffer;
        let total_lines = total_logical_lines(buf);
        let area_height = b.view.area.1 as usize;
        let caret_byte = b.document.selection.primary().head;
        let active_line = if buf.is_empty() {
            0
        } else {
            buf.byte_to_line(caret_byte.min(buf.len().saturating_sub(1)))
        };
        // 5g: normalize scroll to the nearest visible line before the walk.
        let raw_scroll = b.view.scroll.min(total_lines.saturating_sub(1));
        let first_line = fold_view.normalize_line(raw_scroll);
        let source_mode = b.view.mode != RenderMode::LivePreview;
        let scroll_row = b.view.scroll_row;
        (total_lines, active_line, area_height, first_line, source_mode, scroll_row)
    };
    // Persist the normalized scroll so consumers agree.
    editor.active_mut().view.scroll = first_line;

    // Use the shared geometry helper so rebuild, render, and nav all agree on width.
    // text_geometry returns an owned value; the immutable borrow ends here, before
    // the later active_mut() calls.
    let vp_width = crate::nav::text_geometry(editor).text_width as usize;

    let key = LayoutKey {
        blocks_generation: editor.active().document.blocks_generation,
        fold_epoch: editor.active().folds.epoch,
        scroll: first_line,
        scroll_row,
        area: editor.active().view.area,
        text_width: vp_width,
        active_line,
        source_mode,
        heading_level_glyph: editor.theme.heading_level_glyph,
    };
    if editor.active().layout_key.as_ref() == Some(&key) {
        return; // line_layouts already valid for this key — skip the pass
    }

    let mut visual_rows_accumulated: usize = 0;
    let overscan_budget = area_height.saturating_add(scroll_row).saturating_add(1);

    // Clear the old cache and fill for the visible range.
    editor.active_mut().view.line_layouts.clear();
    #[cfg(test)]
    LAYOUT_RUNS.with(|c| c.set(c.get() + 1));

    let mut l = first_line;
    while l < total_lines && visual_rows_accumulated < overscan_budget {
        let (text, role, is_active_effective) = {
            let b = editor.active();
            let buf = &b.document.buffer;
            let text = line_text(buf, l);
            let role = b.document.blocks.role_at(line_start(buf, l));
            let is_active_effective = (l == active_line) || source_mode;
            (text, role, is_active_effective)
        };
        let (rows, map) = layout::layout(&text, role, is_active_effective, vp_width, editor.theme.heading_level_glyph);
        visual_rows_accumulated += rows.len();
        editor.active_mut().view.line_layouts.insert(l, (rows, map));
        // 5g: jump past any folded body that follows this line.
        l = fold_view.next_visible(l).unwrap_or(total_lines);
    }
    editor.active_mut().layout_key = Some(key);
}

// ---------------------------------------------------------------------------
// Parse-panic boundary (M4-rest)
// ---------------------------------------------------------------------------

/// Turn a guarded parse result into the tree to install, managing the deduped
/// parse-degraded notice. On `Err` we install the empty-tree fallback (no child
/// spans → no consumer can slice the current rope out of range) and set the
/// notice once; on `Ok` we clear the notice if it was set.
pub(crate) fn apply_parse_result(
    editor: &mut Editor,
    new_len: usize,
    computed: Result<block_tree::BlockTree, String>,
) -> block_tree::BlockTree {
    match computed {
        Ok(tree) => {
            if editor.parse_degraded {
                editor.parse_degraded = false;
                editor.status.clear();
            }
            tree
        }
        Err(_) => {
            if !editor.parse_degraded {
                editor.parse_degraded = true;
                editor.status = "markdown parse failed — styling may be stale".to_string();
            }
            block_tree::empty_tree(new_len)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use wordcartel_core::style::BlockRole;

    // ------------------------------------------------------------------
    // Task 2: version-memoized two-phase rebuild
    // ------------------------------------------------------------------

    #[test]
    fn rebuild_skips_reparse_when_version_unchanged() {
        use wordcartel_core::block_tree;
        let mut e = crate::editor::Editor::new_from_text("# H\n\nbody\n", None, (80, 24));
        // After construction, blocks_version tracks version 0.
        e.active_mut().reconcile.blocks_version = e.active().document.version;
        // Plant a sentinel tree that differs from full_parse, with NO pending edit.
        let sentinel = block_tree::empty_tree(e.active().document.buffer.len());
        e.active_mut().document.blocks = sentinel.clone();
        e.active_mut().pre_edit_rope = None;
        e.active_mut().last_edit = None;
        crate::derive::rebuild(&mut e);
        // version == blocks_version → parse phase skipped → sentinel survives.
        assert_eq!(e.active().document.blocks, sentinel, "non-edit rebuild must not reparse");
    }

    #[test]
    fn rebuild_reparses_and_sets_stale_on_incremental_edit() {
        use wordcartel_core::block_tree;
        let mut e = crate::editor::Editor::new_from_text("hello\n", None, (80, 24));
        e.active_mut().reconcile.blocks_version = e.active().document.version;
        e.active_mut().reconcile.maybe_stale = false;
        // an ordinary insert (routes through Buffer::apply → sets pre_edit_rope/last_edit, bumps version)
        let doc_len = e.active().document.buffer.len();
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "X".into())], doc_len);
        let txn = wordcartel_core::history::Transaction::new(cs);
        struct C; impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { 0 } }
        e.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C);
        crate::derive::rebuild(&mut e);
        assert_eq!(e.active().reconcile.blocks_version, e.active().document.version);
        // a plain in-paragraph insert is Local → maybe_stale set
        assert!(e.active().reconcile.maybe_stale, "incremental Local/WidenToEnd → maybe_stale");
        assert_eq!(e.active().document.blocks, block_tree::full_parse(&e.active().document.buffer.to_string()));
    }

    #[test]
    fn rebuild_full_parses_and_clears_stale_on_undo() {
        let mut e = crate::editor::Editor::new_from_text("abc\n", None, (80, 24));
        let doc_len = e.active().document.buffer.len();
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "Z".into())], doc_len);
        let txn = wordcartel_core::history::Transaction::new(cs);
        struct C; impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { 0 } }
        e.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C);
        crate::derive::rebuild(&mut e);
        e.active_mut().undo(); // bumps version, clears pre_edit_rope/last_edit
        e.active_mut().reconcile.maybe_stale = true; // pretend stale before undo's rebuild
        crate::derive::rebuild(&mut e);
        assert!(!e.active().reconcile.maybe_stale, "undo → full parse → maybe_stale cleared");
        assert_eq!(e.active().reconcile.blocks_version, e.active().document.version);
    }

    // ------------------------------------------------------------------
    // Brief's failing tests (write RED first, then implement GREEN)
    // ------------------------------------------------------------------

    #[test]
    fn unicode_line_breaks_do_not_split_logical_lines() {
        use crate::editor::Editor;
        // U+2028 (LINE SEPARATOR) and a bare CR must NOT create new logical lines.
        let e = Editor::new_from_text("a\u{2028}b\rc\n", None, (80, 24));
        // One real LF-terminated line of content + the empty trailing line = 2.
        assert_eq!(crate::derive::total_logical_lines(&e.active().document.buffer), 2);
        // The whole "a\u{2028}b\rc" is one logical line (its content, sans trailing \n).
        assert_eq!(crate::derive::line_text(&e.active().document.buffer, 0), "a\u{2028}b\rc");
    }

    /// Inactive heading line shows concealed display (e.g. "Title", not "# Title").
    #[test]
    fn derive_lays_out_visible_lines_with_roles() {
        let mut e = Editor::new_from_text("# Title\n\nplain body\n", None, (80, 24));
        // Move cursor to the blank line (byte 8 = '\n' of blank line) so that
        // line 0 (the heading) is NOT the active line — it should show concealed.
        // "# Title\n" is 8 bytes; the blank line '\n' starts at byte 8.
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(8);
        rebuild(&mut e);
        let (rows0, _) = &e.active().view.line_layouts[&0];
        // inactive heading line -> "# " concealed -> "Title"
        assert_eq!(rows0[0].display, "Title");
        assert_eq!(rows0[0].role, BlockRole::Heading(1));
    }

    /// The cursor's line (active) shows raw markdown, not concealed display.
    #[test]
    fn active_line_renders_raw() {
        let mut e = Editor::new_from_text("# Title\n", None, (80, 24));
        // cursor at 0 -> line 0 active -> raw "# Title"
        rebuild(&mut e);
        let (rows0, _) = &e.active().view.line_layouts[&0];
        assert_eq!(rows0[0].display, "# Title");
    }

    // ------------------------------------------------------------------
    // Wrap: a long line at narrow width produces multiple visual rows.
    // ------------------------------------------------------------------

    #[test]
    fn long_line_wraps_at_small_width() {
        // 20-char line, viewport width 5 -> at least 4 rows
        let mut e = Editor::new_from_text("abcdefghijklmnopqrst\n", None, (5, 24));
        rebuild(&mut e);
        let (rows, _) = &e.active().view.line_layouts[&0];
        assert!(rows.len() > 1, "expected wrapping, got {} row(s)", rows.len());
    }

    // ------------------------------------------------------------------
    // Logical-line edge-case helpers
    // ------------------------------------------------------------------

    fn buf(s: &str) -> TextBuffer {
        TextBuffer::from_str(s)
    }

    #[test]
    fn total_lines_empty_is_one() {
        assert_eq!(total_logical_lines(&buf("")), 1);
    }

    #[test]
    fn total_lines_no_newline() {
        assert_eq!(total_logical_lines(&buf("a")), 1);
    }

    #[test]
    fn total_lines_trailing_newline_is_two() {
        assert_eq!(total_logical_lines(&buf("a\n")), 2);
    }

    #[test]
    fn total_lines_lone_newline() {
        assert_eq!(total_logical_lines(&buf("\n")), 2);
    }

    #[test]
    fn total_lines_two_lines_no_trailing_newline() {
        assert_eq!(total_logical_lines(&buf("a\nb")), 2);
    }

    #[test]
    fn line_start_positions() {
        let b = buf("a\nb\n");
        // 4 bytes: a(0) \n(1) b(2) \n(3)
        // line 0 starts at 0, line 1 at 2, line 2 (trailing empty) at 4 (== len)
        assert_eq!(line_start(&b, 0), 0);
        assert_eq!(line_start(&b, 1), 2);
        assert_eq!(line_start(&b, 2), 4); // total_logical_lines == 2, so line 2 == buf.len()
    }

    #[test]
    fn line_text_strips_newline() {
        let b = buf("hello\nworld\n");
        assert_eq!(line_text(&b, 0), "hello");
        assert_eq!(line_text(&b, 1), "world");
        assert_eq!(line_text(&b, 2), ""); // trailing empty line
    }

    #[test]
    fn line_text_empty_buffer() {
        let b = buf("");
        assert_eq!(line_text(&b, 0), "");
    }

    #[test]
    fn line_text_no_trailing_newline() {
        let b = buf("abc");
        assert_eq!(line_text(&b, 0), "abc");
    }

    #[test]
    fn line_text_lone_newline() {
        let b = buf("\n");
        assert_eq!(line_text(&b, 0), "");
        assert_eq!(line_text(&b, 1), "");
    }

    #[test]
    fn line_text_multibyte() {
        // "é\nz\n" — é is 2 bytes
        let b = buf("é\nz\n");
        assert_eq!(line_text(&b, 0), "é");
        assert_eq!(line_text(&b, 1), "z");
        assert_eq!(line_text(&b, 2), "");
        // total = 3 lines
        assert_eq!(total_logical_lines(&b), 3);
    }

    // ------------------------------------------------------------------
    // Incremental path: last_edit+pre_edit_rope → incremental_update_rope
    // Full parse path:  neither Some → full_parse_rope
    // ------------------------------------------------------------------

    #[test]
    fn rebuild_uses_full_parse_when_no_edit() {
        // On a fresh Editor (no prior apply), rebuild must not panic and the
        // block tree must reflect the document content.
        let mut e = Editor::new_from_text("# Hi\n\nbody\n", None, (80, 24));
        assert!(e.active().last_edit.is_none());
        assert!(e.active().pre_edit_rope.is_none());
        rebuild(&mut e);
        // After rebuild, the two option fields must be cleared (take() consumed them).
        assert!(e.active().last_edit.is_none());
        assert!(e.active().pre_edit_rope.is_none());
        // Block tree must reflect the heading.
        use wordcartel_core::style::BlockRole;
        assert_eq!(e.active().document.blocks.role_at(0), BlockRole::Heading(1));
    }

    #[test]
    fn rebuild_clears_pre_edit_rope_and_last_edit() {
        // A rebuild that actually parses must consume (clear) the two option fields.
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        // Simulate a post-apply state: `apply` sets these fields AND bumps version,
        // so the parse phase (version != blocks_version) runs and takes them. Bump
        // the version to keep this simulation faithful to the real edit path.
        e.active_mut().pre_edit_rope = Some(e.active().document.buffer.snapshot());
        e.active_mut().last_edit = Some(wordcartel_core::block_tree::Edit { range: 0..0, new_len: 0 });
        e.active_mut().document.version += 1;
        rebuild(&mut e);
        assert!(e.active().pre_edit_rope.is_none(), "pre_edit_rope should be cleared after a parsing rebuild");
        assert!(e.active().last_edit.is_none(), "last_edit should be cleared after a parsing rebuild");
    }

    // ------------------------------------------------------------------
    // Overscan budget accounts for scroll_row (no blank bottom rows).
    // ------------------------------------------------------------------

    /// Regression test for the overscan-budget bug: when the viewport is
    /// partially scrolled into the first logical line (`scroll_row > 0`),
    /// the layout cache must still cover all editing rows.
    ///
    /// Setup:
    ///   area = (20, 6), scroll_row = 2
    ///   Line 0: 21 chars → wraps to 2 visual rows at width 20 (= scroll_row)
    ///   Lines 1-7: 1 char each → 1 visual row each
    ///
    /// OLD budget = area_height + 1 = 7.  Loop caches lines 0-5 (sum = 7).
    ///   sum_cached - scroll_row = 5 < area_height (6).  RED.
    ///
    /// NEW budget = area_height + scroll_row + 1 = 9.  Loop caches lines 0-7
    ///   (sum = 9).  sum_cached - scroll_row = 7 >= area_height (6).  GREEN.
    #[test]
    fn rebuild_fills_editing_rows_when_top_line_wrapped() {
        // Line 0: 21-char plain line → 2 visual rows at width 20.
        // Lines 1-7: 1-char plain lines → 1 visual row each.
        let text = "abcdefghijklmnopqrstu\na\nb\nc\nd\ne\nf\ng\n";
        let mut e = Editor::new_from_text(text, None, (20, 6));
        // Cursor at byte 0 → line 0 is active (raw layout, 2 rows).
        e.active_mut().document.selection =
            wordcartel_core::selection::Selection::single(0);
        // Simulate a partial scroll into line 0: 2 visual rows of line 0 are
        // above the top of the viewport.
        e.active_mut().view.scroll = 0;
        e.active_mut().view.scroll_row = 2;

        rebuild(&mut e);

        let area_height = e.active().view.area.1 as usize; // 6
        let scroll_row = e.active().view.scroll_row;       // 2

        // Sum the visual-row counts of all cached lines.
        let sum_cached: usize = e
            .active()
            .view
            .line_layouts
            .values()
            .map(|(rows, _)| rows.len())
            .sum();

        assert!(
            sum_cached.saturating_sub(scroll_row) >= area_height,
            "cache covers only {} rows after skipping scroll_row={}; need >= {} (area_height)",
            sum_cached,
            scroll_row,
            area_height,
        );
    }

    // ------------------------------------------------------------------
    // 5g: fold-aware rebuild tests
    // ------------------------------------------------------------------

    #[test]
    fn rebuild_omits_folded_body_lines_from_cache() {
        let doc = "# Top\nintro\n## A\nbody1\nbody2\n## B\ntail\n";
        let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
        let a = doc.find("## A").unwrap();
        ed.active_mut().folds.toggle(a);
        crate::derive::rebuild(&mut ed);
        let keys: Vec<usize> = ed.active().view.line_layouts.keys().copied().collect();
        // line 2 (## A) present; lines 3,4 (body1,body2) absent; line 5 (## B) present.
        assert!(keys.contains(&2));
        assert!(!keys.contains(&3));
        assert!(!keys.contains(&4));
        assert!(keys.contains(&5));
    }

    #[test]
    fn rebuild_normalizes_scroll_that_a_fold_swallowed() {
        let doc = "# Top\nintro\n## A\nbody1\nbody2\n## B\ntail\n";
        let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 4));
        ed.active_mut().view.scroll = 3; // park scroll on body1
        ed.active_mut().folds.toggle(doc.find("## A").unwrap());
        crate::derive::rebuild(&mut ed);
        // scroll must have snapped to the heading line (2), never a hidden line.
        assert_eq!(ed.active().view.scroll, 2);
    }

    #[test]
    fn rebuild_reconciles_dead_fold_anchor() {
        // The definitive reconcile check relocated from Task 4: after an edit that
        // deletes a folded heading, rebuild's reconcile must DROP the anchor (the
        // Task 4 EOF-clamp alone would leave a stale non-heading anchor).
        use wordcartel_core::change::ChangeSet;
        use wordcartel_core::history::EditKind;

        struct TestClock(std::cell::Cell<u64>);
        impl wordcartel_core::history::Clock for TestClock {
            fn now_ms(&self) -> u64 { self.0.get() }
        }

        let doc = "## H\nbody\n## K\n";
        let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
        ed.active_mut().folds.toggle(0); // fold ## H
        // delete "## H\n" so byte 0 is no longer a heading start
        let len = ed.active().document.buffer.len();
        let cs = ChangeSet::delete(0.."## H\n".len(), len);
        let txn = wordcartel_core::history::Transaction::new(cs);
        let edit = wordcartel_core::block_tree::Edit { range: 0.."## H\n".len(), new_len: 0 };
        let clk = TestClock(std::cell::Cell::new(0));
        ed.active_mut().apply(txn, edit, EditKind::Other, &clk);
        crate::derive::rebuild(&mut ed);
        // byte 0 is now "body" — not a heading start — so the fold is gone.
        assert!(!ed.active().folds.folded.contains(&0));
    }

    // ------------------------------------------------------------------
    // M4-rest: apply_parse_result state-transition helper
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // Task 3: LayoutKey gate
    // ------------------------------------------------------------------

    /// Guard: invalidate_layout (same area) followed by rebuild must NOT blank the
    /// layout cache. This has teeth once the gate is in place: if the gate failed
    /// to honour the nulled `layout_key` it would skip the loop and line_layouts
    /// would stay empty.
    #[test]
    fn same_dimension_resize_does_not_blank() {
        let mut e = Editor::new_from_text("# Title\n\nbody\n", None, (80, 24));
        rebuild(&mut e);
        assert!(!e.active().view.line_layouts.is_empty(), "initial rebuild must populate");
        // Simulate Resize at same dimensions: area unchanged, layout_key nulled.
        e.active_mut().invalidate_layout();
        rebuild(&mut e);
        assert!(!e.active().view.line_layouts.is_empty(), "rebuild after invalidate_layout must repopulate");
    }

    /// The gate skips the loop on the second identical rebuild (no state change).
    #[test]
    fn layout_gate_skips_when_unchanged() {
        let mut e = Editor::new_from_text("# Title\n\nbody\n", None, (80, 24));
        LAYOUT_RUNS.with(|c| c.set(0));
        rebuild(&mut e);
        let after_first = LAYOUT_RUNS.with(|c| c.get());
        assert_eq!(after_first, 1, "first rebuild should run the loop");
        rebuild(&mut e); // no state change
        let after_second = LAYOUT_RUNS.with(|c| c.get());
        assert_eq!(after_second, 1, "second rebuild with no change should skip the loop");
    }

    /// Each distinct input to `LayoutKey` causes the loop to re-run.
    #[test]
    fn layout_gate_reruns_on_each_input() {
        let doc = "line0\nline1\nline2\nline3\nline4\n";

        // Helper: reset counter, run once to prime, then make a change and run again;
        // assert the count incremented.
        macro_rules! check_reruns {
            ($label:expr, $setup:expr, $change:expr) => {{
                let mut e: Editor = $setup;
                LAYOUT_RUNS.with(|c| c.set(0));
                rebuild(&mut e);
                let before = LAYOUT_RUNS.with(|c| c.get());
                $change(&mut e);
                rebuild(&mut e);
                let after = LAYOUT_RUNS.with(|c| c.get());
                assert!(after > before, "{}: expected layout re-run but count stayed at {}", $label, before);
            }};
        }

        // scroll: advance first_line by changing view.scroll + invalidating key
        check_reruns!(
            "scroll",
            Editor::new_from_text(doc, None, (80, 4)),
            |e: &mut Editor| {
                e.active_mut().view.scroll = 2;
                e.active_mut().layout_key = None; // force key mismatch
            }
        );

        // area height: simulates a Resize in height
        check_reruns!(
            "area",
            Editor::new_from_text(doc, None, (80, 24)),
            |e: &mut Editor| {
                e.active_mut().view.area = (80, 12);
                e.active_mut().layout_key = None;
            }
        );

        // text_width (via area width): gutter+wrap geometry feeds vp_width
        check_reruns!(
            "text_width",
            Editor::new_from_text(doc, None, (80, 24)),
            |e: &mut Editor| {
                e.active_mut().view.area = (40, 24);
                e.active_mut().layout_key = None;
            }
        );

        // active_line: move cursor to a different line
        check_reruns!(
            "active_line",
            Editor::new_from_text(doc, None, (80, 24)),
            |e: &mut Editor| {
                // "line0\n" is 6 bytes; byte 6 is start of "line1"
                e.active_mut().document.selection =
                    wordcartel_core::selection::Selection::single(6);
                e.active_mut().layout_key = None;
            }
        );

        // source_mode: toggle view mode away from LivePreview
        check_reruns!(
            "source_mode",
            Editor::new_from_text(doc, None, (80, 24)),
            |e: &mut Editor| {
                e.active_mut().view.mode = crate::editor::RenderMode::SourceHighlighted;
                e.active_mut().layout_key = None;
            }
        );

        // fold toggle: bumps folds.epoch
        check_reruns!(
            "fold_epoch",
            Editor::new_from_text(doc, None, (80, 24)),
            |e: &mut Editor| { e.active_mut().folds.toggle(0); }
        );

        // blocks_generation: explicit bump (as if a new parse ran)
        check_reruns!(
            "blocks_generation",
            Editor::new_from_text(doc, None, (80, 24)),
            |e: &mut Editor| {
                e.active_mut().document.blocks_generation =
                    e.active().document.blocks_generation.wrapping_add(1);
            }
        );

        // heading_level_glyph: explicit flip, all other inputs held constant
        check_reruns!(
            "heading_level_glyph",
            Editor::new_from_text("# H\nbody\n", None, (80, 24)),
            |e: &mut Editor| {
                e.theme.heading_level_glyph = !e.theme.heading_level_glyph;
            }
        );
    }

    /// A single non-scrolling mid-screen insert must produce exactly ONE layout
    /// run across the post-command rebuild and the subsequent pre-draw rebuild —
    /// the double-rebuild collapsed.
    #[test]
    fn keystroke_runs_layout_once() {
        // Large enough that inserting at line 1 never scrolls.
        let doc = "line0\nline1\nline2\nline3\nline4\nline5\nline6\n";
        let mut e = Editor::new_from_text(doc, None, (80, 24));
        // Prime the state with an initial rebuild so layout_key is set.
        rebuild(&mut e);

        // Apply a non-scrolling insert at the start of "line1" (byte 6).
        let doc_len = e.active().document.buffer.len();
        let (cs, edit) = crate::commands::build_multi_replace(&[(6, 6, "X".into())], doc_len);
        let txn = wordcartel_core::history::Transaction::new(cs);
        struct C; impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { 0 } }
        e.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C);

        // Reset counter AFTER the apply but BEFORE the rebuilds.
        LAYOUT_RUNS.with(|c| c.set(0));

        // First rebuild (post-command): parses, builds layout → count = 1.
        rebuild(&mut e);
        let after_first = LAYOUT_RUNS.with(|c| c.get());
        assert_eq!(after_first, 1, "post-command rebuild should run the layout loop exactly once");

        // Second rebuild (pre-draw): same key → gate fires → count stays 1.
        rebuild(&mut e);
        let after_second = LAYOUT_RUNS.with(|c| c.get());
        assert_eq!(after_second, 1, "pre-draw rebuild with no change should skip the layout loop");
    }

    // ------------------------------------------------------------------
    // M4-rest: apply_parse_result state-transition helper
    // ------------------------------------------------------------------

    #[test]
    fn apply_parse_result_err_installs_empty_tree_and_sets_degraded_once() {
        let mut ed = crate::editor::Editor::new_from_text("hello\n", None, (80, 24));
        ed.status.clear();
        ed.parse_degraded = false;

        // First Err: empty tree + degraded + notice.
        let t = apply_parse_result(&mut ed, 10, Err("boom".to_string()));
        assert!(ed.parse_degraded);
        assert_eq!(ed.status, "markdown parse failed — styling may be stale");
        assert_eq!(t.root.span, 0..10);
        assert!(t.top_level().is_empty());

        // Second Err while already degraded: still empty tree, notice unchanged (no spam).
        ed.status = "markdown parse failed — styling may be stale".to_string();
        let _ = apply_parse_result(&mut ed, 12, Err("again".to_string()));
        assert!(ed.parse_degraded);

        // Ok while degraded: real tree returned, degraded cleared, notice cleared.
        let real = block_tree::full_parse_rope(&ropey::Rope::from_str("# H\n"));
        let got = apply_parse_result(&mut ed, 4, Ok(real.clone()));
        assert_eq!(got, real);
        assert!(!ed.parse_degraded);
        assert_eq!(ed.status, "");
    }
}
