use crate::editor::Editor;
use wordcartel_core::block_tree;
use wordcartel_core::layout;

pub use crate::lines::{line_start, line_text, total_logical_lines};
pub(crate) use crate::lines::line_render_for;

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
    pub mode: crate::editor::RenderMode, // view.mode — drives per-line LineRender
    pub ventilate: bool,                 // S6 — view.ventilate; sentence-per-line layout path
    pub heading_level_glyph: bool,
}

#[cfg(test)]
thread_local! {
    pub static LAYOUT_RUNS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// Counts `outline::heading_starts` reconcile walks in `rebuild_downstream`. A
    /// no-folds keystroke must not increment this (R1 invariant guard).
    pub static HEADING_STARTS_WALKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// R1 typing-latency bench instrumentation (test-only). Fine-grained phase spans
/// recorded at the hot lines in `rebuild`/`rebuild_downstream` (`parse`,
/// `heading_starts`, `foldview`, `layout_fill`), drained per keystroke by the
/// `e2e_bench` harness. STRICTLY `#[cfg(test)]` — the recording calls and their
/// `Instant::now()` timers are cfg'd out entirely, so production/release builds
/// carry ZERO cost from this module.
#[cfg(test)]
pub(crate) mod bench_spans {
    use std::cell::RefCell;
    use std::time::Duration;

    thread_local! {
        pub static PHASE_SPANS: RefCell<Vec<(&'static str, Duration)>> =
            const { RefCell::new(Vec::new()) };
    }

    /// Append one phase span for the current keystroke/tick.
    pub(crate) fn record(label: &'static str, dur: Duration) {
        PHASE_SPANS.with(|s| s.borrow_mut().push((label, dur)));
    }

    /// Take and clear all spans recorded since the last drain.
    pub(crate) fn drain() -> Vec<(&'static str, Duration)> {
        PHASE_SPANS.with(|s| s.borrow_mut().drain(..).collect())
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
/// `Local`/`WidenToEnd`/`BoundedStale` result or a parse panic; false for a
/// clean full parse).
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
        #[cfg(test)]
        let bench_parse_t0 = std::time::Instant::now();
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
                // block_tree.rs:537). `old_rope` is already `&Rope` → `&old_rope`;
                // bind `new_rope` to a ref → `&new_rope_ref`.
                let new_rope_ref = &new_rope;
                let old_tree = editor.active_mut().document.take_blocks();
                match crate::panicx::catch(move || {
                    block_tree::incremental_update_instrumented_src_owned(
                        old_tree, &old_rope, edit, &new_rope_ref,
                    )
                }) {
                    Ok(outcome) => {
                        let stale = matches!(
                            outcome.reason,
                            block_tree::WidenReason::Local
                                | block_tree::WidenReason::WidenToEnd
                                | block_tree::WidenReason::BoundedStale
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

        editor.active_mut().document.set_blocks(new_blocks);
        editor.active_mut().reconcile.blocks_version = version;
        editor.active_mut().reconcile.maybe_stale = stale;
        #[cfg(test)]
        crate::derive::bench_spans::record("parse", bench_parse_t0.elapsed());
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
/// Runs every draw and does NOT reparse. Only `rebuild` calls it (the reconcile
/// merge just updates `document.blocks`; the pre-draw `rebuild` runs downstream).
pub(crate) fn rebuild_downstream(editor: &mut Editor) {
    // ------------------------------------------------------------------
    // 5g: Reconcile fold anchors against the fresh block tree, then build
    // a FoldView for the visible-line walk below.
    // ------------------------------------------------------------------
    // Generation-gated fold-anchor prune (was every-draw). No per-draw deep clone:
    // compute heading starts under an immutable borrow, then retain.
    {
        let gen = editor.active().document.blocks_generation();
        if !editor.active().folds.is_empty()
            && editor.active().last_reconciled_generation != Some(gen)
        {
            #[cfg(test)]
            HEADING_STARTS_WALKS.with(|c| c.set(c.get() + 1));
            #[cfg(test)]
            let bench_hs_t0 = std::time::Instant::now();
            let starts = {
                let b = editor.active();
                wordcartel_core::outline::heading_starts(b.document.blocks(), &b.document.buffer.snapshot())
            };
            #[cfg(test)]
            crate::derive::bench_spans::record("heading_starts", bench_hs_t0.elapsed());
            editor.active_mut().folds.reconcile_to(&starts);
            editor.active_mut().last_reconciled_generation = Some(gen);
        }
    }
    #[cfg(test)]
    let bench_fv_t0 = std::time::Instant::now();
    let fold_view = editor.active_fold_view();
    #[cfg(test)]
    crate::derive::bench_spans::record("foldview", bench_fv_t0.elapsed());

    // ------------------------------------------------------------------
    // 2. Visible range
    // ------------------------------------------------------------------
    // Snapshot all read-only scalar values from the active buffer before any
    // mutable borrow, so the borrow checker sees no overlap.
    let (total_lines, active_line, area_height, first_line, b_mode, b_ventilate, scroll_row) = {
        let b = editor.active();
        let buf = &b.document.buffer;
        let total_lines = total_logical_lines(buf);
        let area_height = b.view.area.1 as usize;
        let caret_byte = b.document.selection.primary().head;
        let active_line = if buf.is_empty() {
            0
        } else {
            // Clamp to `len`, NOT `len-1`: a caret on the phantom line past a trailing newline
            // must map to the phantom line so the last CONTENT line conceals like any inactive
            // line (ux-H2), instead of staying "active" and rendering raw. `len` is a boundary.
            buf.byte_to_line(caret_byte.min(buf.len()))
        };
        // 5g: normalize scroll to the nearest visible line before the walk.
        let raw_scroll = b.view.scroll.min(total_lines.saturating_sub(1));
        let first_line = fold_view.normalize_line(raw_scroll);
        let b_mode = b.view.mode;
        let b_ventilate = b.view.ventilate;
        let scroll_row = b.view.scroll_row;
        (total_lines, active_line, area_height, first_line, b_mode, b_ventilate, scroll_row)
    };
    // Persist the normalized scroll so consumers agree.
    editor.active_mut().view.scroll = first_line;

    // Use the shared geometry helper so rebuild, render, and nav all agree on width.
    // text_geometry returns an owned value; the immutable borrow ends here, before
    // the later active_mut() calls.
    let vp_width = crate::nav::text_geometry(editor).text_width as usize;

    let key = LayoutKey {
        blocks_generation: editor.active().document.blocks_generation(),
        fold_epoch: editor.active().folds.epoch(),
        scroll: first_line,
        scroll_row,
        area: editor.active().view.area,
        text_width: vp_width,
        active_line,
        mode: b_mode,
        ventilate: b_ventilate,
        heading_level_glyph: editor.theme.heading_level_glyph,
    };
    if editor.active().layout_key.as_ref() == Some(&key) {
        return; // line_layouts already valid for this key — skip the pass
    }

    // S6 — the ventilate lens layout path. A THIN delegation into ventilate.rs (the window-scoped
    // fill lives there; this hub stays a dispatcher). It populates line_layouts + vent_blocks.
    if editor.active().view.ventilate {
        crate::ventilate::fill_visible(editor);
        editor.active_mut().layout_key = Some(key);
        return;
    }
    // IMPORTANT 2 — the non-ventilate path only clears line_layouts (below); it must ALSO clear
    // vent_blocks, or stale resolver metadata survives a toggle-off (runs on the gate miss the
    // ventilate flip causes).
    editor.active_mut().view.vent_blocks.clear();

    #[cfg(test)]
    let bench_lf_t0 = std::time::Instant::now();
    let mut visual_rows_accumulated: usize = 0;
    let overscan_budget = area_height.saturating_add(scroll_row).saturating_add(1);

    // Clear the old cache and fill for the visible range.
    editor.active_mut().view.line_layouts.clear();
    #[cfg(test)]
    LAYOUT_RUNS.with(|c| c.set(c.get() + 1));

    let mut l = first_line;
    while l < total_lines && visual_rows_accumulated < overscan_budget {
        let (text, role) = {
            let b = editor.active();
            let buf = &b.document.buffer;
            let text = line_text(buf, l);
            let role = b.document.blocks().role_at(line_start(buf, l));
            (text, role)
        };
        let render = line_render_for(b_mode, l == active_line);
        let (rows, map) = layout::layout(&text, role, render, vp_width, editor.theme.heading_level_glyph, 0);
        visual_rows_accumulated += rows.len();
        editor.active_mut().view.line_layouts.insert(l, (rows, map));
        // 5g: jump past any folded body that follows this line.
        l = fold_view.next_visible(l).unwrap_or(total_lines);
    }
    editor.active_mut().layout_key = Some(key);
    #[cfg(test)]
    crate::derive::bench_spans::record("layout_fill", bench_lf_t0.elapsed());
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
    // R1 Task 2: no-folds fast path — skip the heading-starts reconcile walk
    // ------------------------------------------------------------------

    #[test]
    fn no_folds_downstream_skips_heading_starts_walk() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("# H1\n\npara\n\n## H2\n\nbody\n", None, (80, 24));
        crate::derive::rebuild(&mut e); // settle: last_reconciled_generation == current gen
        // Simulate a keystroke's tree bump WITHOUT reparsing: re-set the same blocks so
        // blocks_generation advances (set_blocks bumps unconditionally), reopening the gate.
        let tree = e.active().document.blocks().clone();
        e.active_mut().document.set_blocks(tree);
        HEADING_STARTS_WALKS.with(|c| c.set(0));
        crate::derive::rebuild_downstream(&mut e);
        assert_eq!(HEADING_STARTS_WALKS.with(|c| c.get()), 0, "no folds → no reconcile walk");
    }

    #[test]
    fn folds_active_downstream_runs_heading_starts_walk() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("# H1\n\npara\n\n## H2\n\nbody\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.active_mut().folds.toggle(0); // fold H1 (FoldState::toggle, fold.rs:34)
        let tree = e.active().document.blocks().clone();
        e.active_mut().document.set_blocks(tree);
        HEADING_STARTS_WALKS.with(|c| c.set(0));
        crate::derive::rebuild_downstream(&mut e);
        assert!(HEADING_STARTS_WALKS.with(|c| c.get()) >= 1, "folds active → the walk DOES run");
    }

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
        e.active_mut().document.set_blocks(sentinel.clone());
        e.active_mut().pre_edit_rope = None;
        e.active_mut().last_edit = None;
        crate::derive::rebuild(&mut e);
        // version == blocks_version → parse phase skipped → sentinel survives.
        assert_eq!(*e.active().document.blocks(), sentinel, "non-edit rebuild must not reparse");
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
        assert_eq!(*e.active().document.blocks(), block_tree::full_parse(&e.active().document.buffer.to_string()));
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

    /// ux-H2: with the caret on the phantom line past a trailing newline, the last CONTENT
    /// line must conceal (show "Title"), not stay active and render raw ("# Title").
    #[test]
    fn caret_on_phantom_line_conceals_last_content_line() {
        let mut e = Editor::new_from_text("# Title\n", None, (80, 24));
        // Caret at buf.len() = 8 — the phantom line past the trailing '\n', NOT on line 0.
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(8);
        rebuild(&mut e);
        let (rows0, _) = &e.active().view.line_layouts[&0];
        assert_eq!(rows0[0].display, "Title", "last content line must conceal when caret is past the newline");
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
        assert_eq!(e.active().document.blocks().role_at(0), BlockRole::Heading(1));
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
        assert!(!ed.active().folds.folded().contains(&0));
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

        // scroll: advance first_line by changing view.scroll
        check_reruns!(
            "scroll",
            Editor::new_from_text(doc, None, (80, 4)),
            |e: &mut Editor| {
                e.active_mut().view.scroll = 2;
            }
        );

        // area height: simulates a Resize in height
        check_reruns!(
            "area",
            Editor::new_from_text(doc, None, (80, 24)),
            |e: &mut Editor| {
                e.active_mut().view.area = (80, 12);
            }
        );

        // text_width (via area width): gutter+wrap geometry feeds vp_width
        check_reruns!(
            "text_width",
            Editor::new_from_text(doc, None, (80, 24)),
            |e: &mut Editor| {
                e.active_mut().view.area = (40, 24);
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
            }
        );

        // source_mode: toggle view mode away from LivePreview
        check_reruns!(
            "source_mode",
            Editor::new_from_text(doc, None, (80, 24)),
            |e: &mut Editor| {
                e.active_mut().view.mode = crate::editor::RenderMode::SourceHighlighted;
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
                let t = e.active().document.blocks().clone();
                e.active_mut().document.set_blocks(t);
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

    #[test]
    fn ventilate_flag_reruns_layout() {
        // Flipping view.ventilate must change LayoutKey → the gate misses → the fill re-runs.
        let mut e = Editor::new_from_text("Hello there. Bye now.\n", None, (80, 24));
        LAYOUT_RUNS.with(|c| c.set(0));
        rebuild(&mut e);
        let before = LAYOUT_RUNS.with(|c| c.get());
        e.active_mut().view.ventilate = true;
        rebuild(&mut e);
        let after = LAYOUT_RUNS.with(|c| c.get());
        assert!(after > before, "toggling ventilate must re-run the layout loop");
        // Default is off.
        let e2 = Editor::new_from_text("x\n", None, (80, 24));
        assert!(!e2.active().view.ventilate, "ventilate defaults OFF");
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

    // ------------------------------------------------------------------
    // Task 2 (SRC-HI): cache must distinguish SourceHighlighted from SourcePlain
    // ------------------------------------------------------------------

    #[test]
    fn layout_cache_distinguishes_srchi_from_source() {
        let mut e = Editor::new_from_text("**bold**\n", None, (40, 6));
        e.active_mut().view.mode = crate::editor::RenderMode::SourceHighlighted;
        crate::derive::rebuild(&mut e);
        let sh_segs: String = e.active().view.line_layouts[&0].0.iter()
            .flat_map(|r| r.segs.iter()).map(|s| format!("{:?}", s.style)).collect();
        e.active_mut().view.mode = crate::editor::RenderMode::SourcePlain;
        crate::derive::rebuild(&mut e);
        let sp_segs: String = e.active().view.line_layouts[&0].0.iter()
            .flat_map(|r| r.segs.iter()).map(|s| format!("{:?}", s.style)).collect();
        assert_ne!(sh_segs, sp_segs, "SH must re-layout with styles; SP stays Plain (cache must not alias)");
    }

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
