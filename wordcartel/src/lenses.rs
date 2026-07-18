//! Prose-lens spine (shell leaf, S8): per-buffer POS-match store, single-active lens state,
//! commands (Rule 8), the doc-wide count, and the visible-window helper. The sweep that FILLS the
//! store lives in the same module but is wired to the worker in Task 4. POS matches stay OUT of the
//! Diagnostic contract by construction (this is not a DiagSource).
use crate::editor::Editor;
use crate::jobs::{Executor, Job, JobKind, JobResult, ResultClass};
pub use wordcartel_nlp::ProseLensCategory;

/// One flagged span + its lens category. `start/end` (not `Range`) so `PosMatch` stays `Copy`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PosMatch { pub start: usize, pub end: usize, pub category: ProseLensCategory }

/// Per-buffer POS-match store. Fuses the diag `SourceSlot` in-flight latch with the reconcile
/// `armed_for_version` anti-re-arm latch. `computed_for` is `Option<u64>` — an EMPTY match set is
/// meaningful ("0 passives"), so validity is `computed_for == Some(version)` regardless of emptiness
/// (diverges from `SourceSlot::valid_for`'s non-empty sentinel). All four category Vecs are sorted by
/// `start` and non-overlapping within a category (the classifier + a post-sort guarantee it).
///
/// `armed_for_version` is a SENTINEL-initialized latch (`u64::MAX`, NOT `0`): the `advance()` arm gate
/// (Task 4) is `armed_for_version != document.version`, which arms a fresh buffer (version 0, sentinel
/// MAX ≠ 0) exactly once and — crucially — does NOT re-arm once armed/dispatched for a version. This is
/// what latches the oversized-doc cap-skip (CRITICAL-3): the arm that led to `dispatch_pos_sweep` set
/// `armed_for_version = version`, so the cap-skip path (which never dispatches a job to clear in-flight)
/// is not re-armed on the same version — no arm→skip→re-arm loop. A real edit bumps the version and
/// re-arms naturally. (`Default` is hand-written for the sentinel; `derive(Default)` would give 0.)
#[derive(Clone, Debug)]
pub struct PosStore {
    pub adverbs: Vec<PosMatch>,
    pub adjectives: Vec<PosMatch>,
    pub passive: Vec<PosMatch>,
    pub weak: Vec<PosMatch>,
    pub computed_for: Option<u64>,
    pub due_at: Option<u64>,
    pub in_flight_version: Option<u64>,
    pub armed_for_version: u64,
}

impl Default for PosStore {
    fn default() -> Self {
        PosStore {
            adverbs: Vec::new(), adjectives: Vec::new(), passive: Vec::new(), weak: Vec::new(),
            computed_for: None, due_at: None, in_flight_version: None,
            armed_for_version: u64::MAX, // sentinel: a fresh buffer (version 0) still arms once
        }
    }
}

impl PosStore {
    /// The matches for `cat`, or `None` unless `computed_for == Some(version)`.
    pub fn matches_for(&self, cat: ProseLensCategory, version: u64) -> Option<&[PosMatch]> {
        if self.computed_for != Some(version) { return None; }
        Some(match cat {
            ProseLensCategory::Adverbs => &self.adverbs,
            ProseLensCategory::Adjectives => &self.adjectives,
            ProseLensCategory::Passive => &self.passive,
            ProseLensCategory::Weak => &self.weak,
        })
    }
}

/// Human label for the status segment and the menu-cycle mark.
pub fn category_label(cat: ProseLensCategory) -> &'static str {
    match cat {
        ProseLensCategory::Adverbs => "Adverbs",
        ProseLensCategory::Adjectives => "Adjectives",
        ProseLensCategory::Passive => "Passive",
        ProseLensCategory::Weak => "Weak",
    }
}

/// The ONE shared setter (contract Law 6). Sets the active buffer's lens; arming the sweep is handled
/// edge-triggered in `app.rs advance()` (it fires whenever a lens is active and the store is stale),
/// so this setter only sets state. (Kept symmetric with `ventilate::set_ventilate`.)
pub fn set_prose_lens(editor: &mut Editor, lens: Option<ProseLensCategory>) {
    editor.active_mut().view.prose_lens = lens;
}

/// Cycle Adverbs -> Adjectives -> Passive -> Weak -> off -> Adverbs.
pub fn cycle_prose_lens(editor: &mut Editor) {
    use ProseLensCategory::*;
    let next = match editor.active().view.prose_lens {
        None => Some(Adverbs),
        Some(Adverbs) => Some(Adjectives),
        Some(Adjectives) => Some(Passive),
        Some(Passive) => Some(Weak),
        Some(Weak) => None,
    };
    set_prose_lens(editor, next);
}

/// The single source of truth for "what the active prose lens shows" — the active category's slice,
/// gated on a lens being active AND the store being current (`computed_for == version`). Mirrors
/// `diagnostics_run::active_lens_diags` but for POS matches.
pub fn active_pos_matches(editor: &Editor) -> Option<&[PosMatch]> {
    let lens = editor.active().view.prose_lens?;
    let v = editor.active().document.version;
    editor.active().pos.matches_for(lens, v)
}

/// Right-side status segment "Passive: 47", shown only when a lens is active AND the count is honest
/// (`computed_for == version`). While a sweep is in flight or the store is stale → `None`.
pub fn prose_lens_count_segment(editor: &Editor) -> Option<String> {
    let lens = editor.active().view.prose_lens?;
    let n = active_pos_matches(editor)?.len();
    Some(format!("{}: {}", category_label(lens), n))
}

/// Upper-bound a sorted-by-`start` match slice to `start < hi` (the diag idiom, `partition_point`).
/// Returns the contiguous `[..hi_idx]` prefix; the `end > lo` LOWER bound is applied per glyph by the
/// paint loop (`overlaps` in `row_spans_placed`), so `lo` is intentionally NOT a parameter here. Lives
/// in `lenses.rs` (NOT render.rs) for the hub budget.
pub fn window_matches(ms: &[PosMatch], hi: usize) -> &[PosMatch] {
    let hi_idx = ms.partition_point(|m| m.start < hi);
    &ms[..hi_idx]
}

use wordcartel_core::selection::Selection;

fn nav_to(editor: &mut Editor, start: usize, end: usize) {
    crate::registry::unfold_ancestors_of(editor, start);
    // Head-at-start (C-9): Selection::range(anchor, head) puts head on the 2nd arg → pass (end, start)
    // so from()==start, to()==end, head==start. The whole span is the visible abortable selection (D6).
    editor.active_mut().document.selection = Selection::range(end, start);
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}

/// Move the caret to the NEXT prose-lens match after the caret, range-selecting the WHOLE span with
/// the head at the span's START (C-9 — diverges from `diag_next`'s `Selection::single`). Wraps to the
/// first match past the end of the document. No-op (no panic) when no lens is active or the active
/// category's store is empty.
pub fn prose_lens_next_match(editor: &mut Editor) {
    let Some(ms) = active_pos_matches(editor) else { return; };
    if ms.is_empty() { return; }
    let caret = editor.active().document.selection.primary().to();
    let (start, end) = ms.iter().find(|m| m.start > caret)
        .map(|m| (m.start, m.end))
        .unwrap_or((ms[0].start, ms[0].end)); // wrap
    nav_to(editor, start, end);
}

/// Move the caret to the PREVIOUS prose-lens match before the caret, range-selecting the WHOLE span
/// with the head at the span's START (C-9). Wraps to the last match. No-op (no panic) when no lens is
/// active or the active category's store is empty.
pub fn prose_lens_prev_match(editor: &mut Editor) {
    let Some(ms) = active_pos_matches(editor) else { return; };
    if ms.is_empty() { return; }
    let caret = editor.active().document.selection.primary().to();
    let last = ms.len() - 1;
    let (start, end) = ms.iter().rev().find(|m| m.start < caret)
        .map(|m| (m.start, m.end))
        .unwrap_or((ms[last].start, ms[last].end)); // wrap
    nav_to(editor, start, end);
}

// ---------------------------------------------------------------------------
// S8 Task 4 — the doc-wide async sweep that FILLS the store. Near-copy of
// `reconcile::dispatch_reconcile`; rides the `jobs.rs` worker.
// ---------------------------------------------------------------------------

/// Debounce before a settled buffer is swept. Slightly longer than reconcile's 150 ms so the block
/// tree (which the prose-range enumeration reads) settles first.
pub const POS_SWEEP_DEBOUNCE_MS: u64 = 300;

/// A sweep is due if a lens is active, the tree has converged, nothing is in flight, and the debounce
/// deadline has passed. Gated identically in the timers deadline fn (idle-free / A3 anti-spin).
pub fn pos_sweep_due(editor: &Editor, now: u64) -> bool {
    editor.active().view.prose_lens.is_some()
        && !editor.active().reconcile.maybe_stale        // tree converged (correction #4)
        && editor.active().pos.in_flight_version.is_none()
        && matches!(editor.active().pos.due_at, Some(t) if now >= t)
}

/// Enumerate prose-PARAGRAPH byte ranges (role_at == Paragraph) from the block tree — the sweep must
/// NOT analyze code fences / headings / front matter / tables (correction #4). Walks the block tree
/// (`BlockTree::top_level` + recursion into containers) and keeps each block whose role is Paragraph.
/// A list-item/blockquote paragraph has a higher-precedence role (`role_at` returns ListItem/BlockQuote
/// there, `block_tree.rs role_precedence`), so it is naturally excluded — consistent with
/// `ventilate::prose_block_at`, which the S7/lens paint window uses. `TextBuffer` has NO `len_lines()`
/// (verified — buffer.rs), so this walks BLOCKS, not lines.
fn prose_paragraph_ranges(editor: &Editor) -> Vec<(usize, usize)> {
    use wordcartel_core::block_tree::Block;
    use wordcartel_core::style::BlockRole;
    let blocks = editor.active().document.blocks();
    let mut out = Vec::new();
    fn walk(blocks: &wordcartel_core::block_tree::BlockTree, b: &Block, out: &mut Vec<(usize, usize)>) {
        if b.children.is_empty() {
            // Leaf: keep it iff its role resolves to Paragraph (excludes code/heading/rule/etc.).
            if blocks.role_at(b.span.start) == BlockRole::Paragraph {
                out.push((b.span.start, b.span.end));
            }
        } else {
            for c in &b.children { walk(blocks, c, out); }
        }
    }
    for b in blocks.top_level() { walk(blocks, b, &mut out); }
    out
}

/// Snapshot the active buffer + dispatch the doc-wide POS sweep (near-copy of `dispatch_reconcile`).
/// Oversized docs (> `LENS_MAX_SWEEP_BYTES`) are skipped WITHOUT dispatching a job — the CRITICAL-3
/// latch (see `PosStore::armed_for_version`'s doc comment) pins `armed_for_version` so `advance()`
/// cannot re-arm on the same version, giving at most one skip + one sticky notice per version.
pub fn dispatch_pos_sweep(editor: &mut Editor, ex: &dyn Executor) {
    let b = editor.active();
    let buffer_id = b.id;
    let version = b.document.version;
    if b.document.buffer.len() as u64 > crate::limits::LENS_MAX_SWEEP_BYTES {
        editor.active_mut().pos.due_at = None;
        editor.active_mut().pos.armed_for_version = version;
        editor.set_status_full(crate::status::StatusKind::Warning,
            "document too large for prose lenses — sweep skipped",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return;
    }
    let ranges = prose_paragraph_ranges(editor);
    let rope = editor.active().document.buffer.snapshot();  // O(1)
    editor.active_mut().pos.in_flight_version = Some(version);
    editor.active_mut().pos.due_at = None;

    let job = Job {
        buffer_id, class: ResultClass::BufferLocal, version, kind: JobKind::PosSweep,
        run: Box::new(move || {
            let mut adverbs = Vec::new(); let mut adjectives = Vec::new();
            let mut passive = Vec::new(); let mut weak = Vec::new();
            for (ps, pe) in ranges {
                // snapshot() returns ropey::Rope (verified — buffer.rs); slice by BYTE via
                // byte_slice (NOT .slice, which is char-indexed) then to_string for analyze's &str.
                let slice = rope.byte_slice(ps..pe).to_string();
                let sentences = wordcartel_nlp::analyze(&slice);
                for m in wordcartel_nlp::classify(&sentences, &slice) {
                    let pm = PosMatch { start: ps + m.range.start, end: ps + m.range.end, category: m.category };
                    match m.category {
                        ProseLensCategory::Adverbs => adverbs.push(pm),
                        ProseLensCategory::Adjectives => adjectives.push(pm),
                        ProseLensCategory::Passive => passive.push(pm),
                        ProseLensCategory::Weak => weak.push(pm),
                    }
                }
            }
            for v in [&mut adverbs, &mut adjectives, &mut passive, &mut weak] {
                v.sort_by_key(|m| m.start);
            }
            JobResult {
                buffer_id, class: ResultClass::BufferLocal, version, kind: JobKind::PosSweep,
                merge: Box::new(move |editor: &mut Editor| {
                    if let Some(b) = editor.by_id_mut(buffer_id) {
                        if b.document.version == version {   // version-discard INSIDE the merge
                            b.pos.adverbs = adverbs; b.pos.adjectives = adjectives;
                            b.pos.passive = passive; b.pos.weak = weak;
                            b.pos.computed_for = Some(version);
                        }
                        b.pos.in_flight_version = None;      // clear regardless
                    }
                }),
            }
        }),
    };
    ex.dispatch(job);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use wordcartel_nlp::ProseLensCategory::*;

    // --- helpers ---
    fn matches(cat: ProseLensCategory, spans: &[(usize, usize)]) -> Vec<PosMatch> {
        spans.iter().map(|&(s,e)| PosMatch { start: s, end: e, category: cat }).collect()
    }

    #[test]
    fn set_prose_lens_sets_view_state_per_buffer() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        assert_eq!(e.active().view.prose_lens, None);
        set_prose_lens(&mut e, Some(Passive));
        assert_eq!(e.active().view.prose_lens, Some(Passive));
        set_prose_lens(&mut e, None);
        assert_eq!(e.active().view.prose_lens, None);
    }

    #[test]
    fn active_pos_matches_gated_on_computed_for_version() {
        let mut e = Editor::new_from_text("The report was written here.\n", None, (80, 24));
        let v = e.active().document.version;
        e.active_mut().pos.passive = matches(Passive, &[(4, 21)]);
        e.active_mut().pos.computed_for = Some(v);
        // no lens active → None
        assert!(active_pos_matches(&e).is_none());
        set_prose_lens(&mut e, Some(Passive));
        assert_eq!(active_pos_matches(&e).map(|s| s.len()), Some(1));
        // version bump without re-sweep → stale → None (computed_for != version)
        e.active_mut().document.version += 1;
        assert!(active_pos_matches(&e).is_none(), "stale store must not paint");
    }

    #[test]
    fn active_pos_matches_empty_set_is_meaningful_zero() {
        // computed_for == version but zero matches → Some(&[]) (NOT None) — diverges from SourceSlot.
        let mut e = Editor::new_from_text("Nothing here.\n", None, (80, 24));
        let v = e.active().document.version;
        e.active_mut().pos.computed_for = Some(v);      // swept, found nothing
        set_prose_lens(&mut e, Some(Passive));
        assert_eq!(active_pos_matches(&e).map(|s| s.len()), Some(0), "empty is a real answer");
    }

    #[test]
    fn count_segment_gated_and_labeled() {
        let mut e = Editor::new_from_text("The report was written here.\n", None, (80, 24));
        let v = e.active().document.version;
        e.active_mut().pos.passive = matches(Passive, &[(4, 21)]);
        e.active_mut().pos.computed_for = Some(v);
        assert_eq!(prose_lens_count_segment(&e), None, "no lens → no segment");
        set_prose_lens(&mut e, Some(Passive));
        assert_eq!(prose_lens_count_segment(&e), Some("Passive: 1".into()));
        e.active_mut().document.version += 1;
        assert_eq!(prose_lens_count_segment(&e), None, "stale → suppressed");
    }

    #[test]
    fn cycle_walks_all_then_off() {
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let seq = [Some(Adverbs), Some(Adjectives), Some(Passive), Some(Weak), None, Some(Adverbs)];
        for want in seq {
            cycle_prose_lens(&mut e);
            assert_eq!(e.active().view.prose_lens, want);
        }
    }

    #[test]
    fn nav_next_range_selects_whole_span_head_at_start() {
        let mut e = Editor::new_from_text("The report was written by them.\n", None, (80, 24));
        let v = e.active().document.version;
        // "was written" span (bytes 11..22 of the text). Compute concretely:
        let t = e.active().document.buffer.to_string();
        let start = t.find("was written").unwrap();
        let end = start + "was written".len();
        e.active_mut().pos.passive = matches(Passive, &[(start, end)]);
        e.active_mut().pos.computed_for = Some(v);
        set_prose_lens(&mut e, Some(Passive));
        // caret at 0 → next finds the match, range-selects it, head at START.
        prose_lens_next_match(&mut e);
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (start, end), "whole span selected");
        assert_eq!(sel.head, start, "head-at-start (C-9) — caret lands at span start");
        assert!(!sel.is_empty(), "a visible abortable selection (D6)");
    }

    /// S8 Task 6 (integration pin): nav range-selects the WHOLE flagged span (head at start,
    /// D6 abortable selection) — typing over it then replaces the whole span, not just the
    /// caret position. Exercises `prose_lens_next_match` → `commands::insert_char` against a
    /// real `Editor`, end to end.
    #[test]
    fn typing_replaces_the_nav_selected_match() {
        use crate::test_support::TestClock;
        let t = "The report was written by them.\n";
        let mut e = crate::editor::Editor::new_from_text(t, None, (80, 24));
        let v = e.active().document.version;
        let start = t.find("was written").unwrap();
        let end = start + "was written".len();
        e.active_mut().pos.passive = vec![crate::lenses::PosMatch { start, end, category: crate::lenses::ProseLensCategory::Passive }];
        e.active_mut().pos.computed_for = Some(v);
        crate::lenses::set_prose_lens(&mut e, Some(crate::lenses::ProseLensCategory::Passive));
        crate::lenses::prose_lens_next_match(&mut e);
        // typing over the selection replaces the whole flagged span (D6 — abortable, then mutate by typing).
        crate::commands::insert_char(&mut e, 'X', &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "The report X by them.\n");
    }

    #[test]
    fn nav_wraps_and_noops_when_empty_or_offlens() {
        let mut e = Editor::new_from_text("no matches at all here.\n", None, (80, 24));
        // off-lens: no-op, no panic
        prose_lens_next_match(&mut e);
        prose_lens_prev_match(&mut e);
        // lens on, empty store: no-op, no panic
        set_prose_lens(&mut e, Some(Passive));
        e.active_mut().pos.computed_for = Some(e.active().document.version);
        prose_lens_next_match(&mut e);
        assert!(e.active().document.selection.primary().is_empty());
    }

    #[test]
    fn window_matches_upper_bounds_by_start() {
        // `window_matches` is ONLY the cheap upper-bound prefilter (partition_point on `start < hi`):
        // it returns the contiguous `[..hi_idx]` slice, the diag idiom. The `end > lo` lower bound is
        // NOT applied here — the paint loop applies it per glyph via `overlaps` (row_spans_placed).
        let ms = matches(Passive, &[(0, 5), (10, 15), (20, 25), (30, 35)]);
        // hi = 28 → keep every match with start < 28 → (0,5),(10,15),(20,25); (30,35) dropped.
        let w = window_matches(&ms, 28);
        assert_eq!(w.iter().map(|m| (m.start, m.end)).collect::<Vec<_>>(), vec![(0,5),(10,15),(20,25)]);
        // hi = 10 → start < 10 → only (0,5).
        assert_eq!(window_matches(&ms, 10).iter().map(|m| m.start).collect::<Vec<_>>(), vec![0]);
        // hi = 0 → empty slice.
        assert!(window_matches(&ms, 0).is_empty());
    }

    // -----------------------------------------------------------------------
    // S8 Task 4: the doc-wide sweep — converge, version-discard, prose-only, cap latch.
    // -----------------------------------------------------------------------

    #[test]
    fn sweep_fills_store_and_marks_computed_for() {
        use crate::jobs::{Executor, InlineExecutor};
        let mut e = crate::editor::Editor::new_from_text("The report was written by them.\n", None, (80, 24));
        let v = e.active().document.version;
        set_prose_lens(&mut e, Some(ProseLensCategory::Passive));
        let ex = InlineExecutor::default();
        dispatch_pos_sweep(&mut e, &ex);
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert_eq!(e.active().pos.computed_for, Some(v));
        assert_eq!(e.active().pos.passive.len(), 1, "one passive found doc-wide");
        assert!(e.active().pos.in_flight_version.is_none());
    }

    #[test]
    fn sweep_discards_when_version_advanced() {
        use crate::jobs::{Executor, InlineExecutor};
        let mut e = crate::editor::Editor::new_from_text("The report was written.\n", None, (80, 24));
        set_prose_lens(&mut e, Some(ProseLensCategory::Passive));
        let ex = InlineExecutor::default();
        dispatch_pos_sweep(&mut e, &ex);
        e.active_mut().document.version += 1;   // edit lands before merge
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert_ne!(e.active().pos.computed_for, Some(e.active().document.version - 1));
        assert!(e.active().pos.computed_for.is_none() || e.active().pos.computed_for != Some(e.active().document.version));
        assert!(e.active().pos.in_flight_version.is_none(), "in-flight cleared even on discard");
    }

    #[test]
    fn sweep_is_prose_only_code_fence_is_not_flagged() {
        use crate::jobs::{Executor, InlineExecutor};
        // "is" inside a fenced code block must NOT be classified.
        let t = "Para one is here.\n\n```\nthis is code\n```\n";
        let mut e = crate::editor::Editor::new_from_text(t, None, (80, 24));
        crate::derive::rebuild(&mut e);   // build the block tree
        set_prose_lens(&mut e, Some(ProseLensCategory::Weak));
        let ex = InlineExecutor::default();
        dispatch_pos_sweep(&mut e, &ex);
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        // The paragraph's "is" is weak; the code fence's "is" is not in any match.
        let code_is = t.rfind("is code").unwrap();
        assert!(!e.active().pos.weak.iter().any(|m| m.start <= code_is && code_is < m.end),
            "code-fence 'is' must not be flagged");
    }

    #[test]
    fn no_lens_active_arms_no_pos_sweep_deadline() {
        // idle-free: with no lens active, the pos_sweep subsystem yields None even with a past-due arm.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let dl = || crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "pos_sweep").unwrap().deadline;
        e.active_mut().pos.due_at = Some(0);              // armed, past due
        assert_eq!((dl())(&e, 10_000), None, "no lens → None (no wake)");
        // lens on + converged tree + not in flight → the armed deadline reappears (gate is load-bearing).
        set_prose_lens(&mut e, Some(ProseLensCategory::Passive));
        e.active_mut().reconcile.maybe_stale = false;
        assert_eq!((dl())(&e, 10_000), Some(0), "lens on + converged → armed deadline live");
        // in-flight gate:
        e.active_mut().pos.in_flight_version = Some(1);
        assert_eq!((dl())(&e, 10_000), None, "in-flight → None");
        e.active_mut().pos.in_flight_version = None;
        // tree-stale gate:
        e.active_mut().reconcile.maybe_stale = true;
        assert_eq!((dl())(&e, 10_000), None, "tree not converged → None");
    }

    #[test]
    fn oversized_doc_cap_latches_no_rearm_loop() {
        // CRITICAL-3: an oversized doc skips WITHOUT dispatching a job, latches armed_for_version to the
        // current version so advance() will NOT re-arm on the same version, and yields a None deadline.
        // Drives the REAL crate::app::advance() arm (mirrors
        // app::tests::advance_arms_on_change_due_with_subscriber_version_latched) so this test FAILS if
        // the OR-escape `due_at.is_none() ||` (Critical-3) is reintroduced into the sweep arm gate.
        use crate::jobs::{Executor, InlineExecutor};
        use crate::test_support::TestClock;
        let big = "word ".repeat((crate::limits::LENS_MAX_SWEEP_BYTES as usize / 5) + 2); // just over 8 MiB
        let mut e = crate::editor::Editor::new_from_text(&big, None, (80, 24));
        let v = e.active().document.version;
        set_prose_lens(&mut e, Some(ProseLensCategory::Weak));

        // Drive the REAL arm: advance() must set armed_for_version = v and a debounce deadline.
        let clk = TestClock(1_000);
        crate::app::advance(&mut e, &clk);
        assert_eq!(e.active().pos.armed_for_version, v, "advance() armed for this version");
        assert!(e.active().pos.due_at.is_some(), "advance() set a debounce deadline");

        // Fire the debounce — the REAL cap-skip path (dispatch_pos_sweep) runs, as timers::on_tick
        // would once due_at elapses.
        let ex = InlineExecutor::default();
        dispatch_pos_sweep(&mut e, &ex);
        assert!(ex.drain().is_empty(), "cap path dispatches NO job");
        assert_eq!(e.active().pos.due_at, None, "due_at cleared");
        assert_eq!(e.active().pos.armed_for_version, v, "latched to this version");
        assert_eq!(e.active().pos.computed_for, None, "never computed");
        assert!(e.active().pos.in_flight_version.is_none());
        assert!(e.status_text().contains("too large"), "one-time sticky notice");

        // No-rearm-loop: driving the REAL advance() AGAIN at the same version/document must NOT
        // resurrect due_at or push armed_for_version forward. If the OR-escape were reintroduced,
        // `due_at.is_none()` (true, just cleared above) would re-arm here — this assertion catches it.
        let clk2 = TestClock(1_050);
        crate::app::advance(&mut e, &clk2);
        assert_eq!(e.active().pos.due_at, None, "idle re-run at the same version must not re-arm");
        assert_eq!(e.active().pos.armed_for_version, v, "latch holds across a second advance()");

        // Deadline None after the skip (due_at None) even with a converged tree + lens active → idle-free.
        let dl = crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "pos_sweep").unwrap().deadline;
        e.active_mut().reconcile.maybe_stale = false;
        assert_eq!((dl)(&e, 10_000), None, "deadline None after the one skip");
    }
}
