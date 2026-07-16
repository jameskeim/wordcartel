//! Persistent marked-block creation and operations (Effort 9A Tasks 2–3).
//! ^KB = block_begin, ^KK = block_end, promote-from-selection.
//! Task 3 adds: copy/move/delete/jump/hide/clear.

use crate::editor::{Editor, MarkedBlock};
use crate::nav;
use wordcartel_core::history::Clock;

// --- Task 3: act-on-block operations ---

fn block(editor: &Editor) -> Option<crate::editor::MarkedBlock> { editor.active().marked_block }

/// Copy the marked block's text and insert it at the caret. The block survives the
/// copy: its endpoints map through the insertion via `apply`.
///
/// Unlike `block_move`, there is intentionally NO caret-inside-`[start, end)` guard.
/// With the caret strictly inside the block, the inserted duplicate lands within the
/// block's span, so `map_pos_before` advances `end` past the insert and the block
/// GROWS to include its own copy. This is by design: copy is non-destructive (it
/// never loses data), so a caret-inside copy is well-defined and safe — the guard
/// `block_move` needs (to avoid relocating a block into the hole it leaves) does not
/// apply here.
pub fn block_copy(editor: &mut Editor, clock: &dyn Clock) {
    let Some(b) = block(editor) else { editor.set_status(crate::status::StatusKind::Info, "no marked block"); return; };
    let text = editor.active().document.buffer.slice(b.start..b.end);
    let caret = nav::head(editor);
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = crate::commands::build_multi_replace(&[(caret, caret, text.clone())], doc_len);
    let new_caret = caret + text.len();
    apply_edit(editor, cs, edit, new_caret, clock);
    // block stays — its endpoints map through the insertion via apply.
    editor.set_status(crate::status::StatusKind::Info, "block copied");
}

pub fn block_move(editor: &mut Editor, clock: &dyn Clock) {
    let Some(b) = block(editor) else { editor.set_status(crate::status::StatusKind::Info, "no marked block"); return; };
    let caret = nav::head(editor);
    if caret >= b.start && caret < b.end {
        editor.set_status(crate::status::StatusKind::Info, "can't move a block into itself");
        return;
    }
    let text = editor.active().document.buffer.slice(b.start..b.end);
    let doc_len = editor.active().document.buffer.len();
    // ascending, non-overlapping edits (build_multi_replace requires order)
    let (edits, new_caret) = if caret < b.start {
        (vec![(caret, caret, text.clone()), (b.start, b.end, String::new())], caret + text.len())
    } else {
        // caret >= b.end (inside guarded above)
        (vec![(b.start, b.end, String::new()), (caret, caret, text.clone())], caret - (b.end - b.start) + text.len())
    };
    let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
    // dest of the moved block's start in FINAL coords: caret (moved before) or caret-len (moved after).
    let dest = if caret < b.start { caret } else { caret - (b.end - b.start) };
    let corrected = if !editor.active().folds.is_empty() {
        Some(crate::fold::corrected_after_move(&editor.active().folds, &[(b.start, b.end, dest)], &cs))
    } else { None };
    apply_edit(editor, cs, edit, new_caret, clock); // core: mutate + rebuild #1 + ensure_visible
    if let Some(c) = corrected {
        editor.active_mut().folds.replace_folded(c);
        crate::edit_apply::resettle(editor);        // rebuild #2 — relayout + reconcile corrected folds
        // The fold correction can re-fold the destination section around `new_caret`, leaving the
        // head on a hidden line — snap it out (the shipped-fold-command guard) so typing never
        // edits invisible text.
        crate::registry::snap_caret_out_of_fold(editor);
    }
    editor.active_mut().marked_block = None; // consumed
    editor.set_status(crate::status::StatusKind::Info, "block moved");
}

pub fn block_delete(editor: &mut Editor, clock: &dyn Clock) {
    let Some(b) = block(editor) else { editor.set_status(crate::status::StatusKind::Info, "no marked block"); return; };
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = crate::commands::build_multi_replace(&[(b.start, b.end, String::new())], doc_len);
    apply_edit(editor, cs, edit, b.start, clock);
    editor.active_mut().marked_block = None;
    editor.set_status(crate::status::StatusKind::Info, "block deleted");
}

fn apply_edit(
    editor: &mut Editor,
    cs: wordcartel_core::change::ChangeSet,
    edit: wordcartel_core::block_tree::Edit,
    new_caret: usize,
    clock: &dyn Clock,
) {
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(new_caret));
    editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
}

pub fn block_jump_begin(editor: &mut Editor) { block_jump(editor, true); }
pub fn block_jump_end(editor: &mut Editor)   { block_jump(editor, false); }
fn block_jump(editor: &mut Editor, to_start: bool) {
    let Some(b) = block(editor) else { editor.set_status(crate::status::StatusKind::Info, "no marked block"); return; };
    let target = if to_start { b.start } else { b.end };
    let pre = nav::head(editor);
    crate::marks::record_jump(editor.active_mut(), pre);
    let off = nav::clamp_snap(editor, target);
    let off = crate::registry::place_caret_visible(editor, off, crate::registry::CaretPlace::UnfoldTo);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
    crate::derive::rebuild(editor);
    nav::ensure_visible(editor);
}

pub fn block_toggle_hidden(editor: &mut Editor) {
    match editor.active_mut().marked_block.as_mut() {
        Some(b) => {
            b.hidden = !b.hidden;
            let h = b.hidden;
            editor.set_status(crate::status::StatusKind::Info, if h { "block hidden" } else { "block shown" });
        }
        None => editor.set_status(crate::status::StatusKind::Info, "no marked block"),
    }
}

pub fn block_clear(editor: &mut Editor) {
    editor.active_mut().marked_block = None;
    editor.active_mut().pending_block_begin = None;
    editor.set_status(crate::status::StatusKind::Info, "block cleared");
}

/// ^KW: open the Write-Block minibuffer pre-filled with the document's directory.
pub fn block_write(editor: &mut Editor) {
    if editor.active().marked_block.is_none() {
        editor.set_status(crate::status::StatusKind::Info, "no marked block");
        return;
    }
    let pre = editor.active().document.path.as_ref()
        .and_then(|p| p.parent())
        .map(|d| format!("{}/", d.display()))
        .unwrap_or_default();
    editor.open_minibuffer("Write block to: ", crate::minibuffer::MinibufferKind::WriteBlock);
    if let Some(mb) = editor.minibuffer.as_mut() {
        mb.cursor = pre.len();
        mb.text = pre;
    }
}

/// Set `pending_block_begin` to the current caret position (^KB).
pub fn block_begin(editor: &mut Editor) {
    let at = nav::head(editor);
    editor.active_mut().pending_block_begin = Some(at);
    editor.set_status(crate::status::StatusKind::Info, "block begin set");
}

/// Complete the block from pending begin to current caret (^KK).
/// Normalizes so start <= end; rejects empty; clears pending on success or error.
pub fn block_end(editor: &mut Editor) {
    let Some(begin) = editor.active().pending_block_begin else {
        editor.set_status(crate::status::StatusKind::Info, "set block begin first");
        return;
    };
    let end = nav::head(editor);
    set_block(editor, begin, end);
    editor.active_mut().pending_block_begin = None;
}

/// Promote: convert the live selection to a marked block and clear the selection.
/// Empty selection → status "no selection to mark".
pub fn mark_block_from_selection(editor: &mut Editor) {
    let sel = editor.active().document.selection.primary();
    let (from, to) = (sel.from(), sel.to());
    if from == to {
        editor.active_mut().pending_block_begin = None;
        editor.set_status(crate::status::StatusKind::Info, "no selection to mark");
        return;
    }
    let caret = nav::head(editor);
    // INVARIANT: `from != to` (guarded above) → set_block cannot hit its empty-reject path,
    // so the block IS set; clearing the selection below is unconditionally safe. If set_block
    // ever gains a new rejection path, gate the selection-clear on success.
    set_block(editor, from, to);
    editor.active_mut().pending_block_begin = None;
    // Convert: clear the live selection back to a single caret.
    editor.active_mut().document.selection =
        wordcartel_core::selection::Selection::single(caret);
}

/// Select the marked block's range (the block → selection bridge, A11.3). The marked
/// block is a target, not implicit scope — this makes it the active selection so the
/// universal selection-primary convention then governs filter/transform/case ops.
pub fn select_marked_block(editor: &mut Editor) {
    match editor.active().marked_block {
        Some(MarkedBlock { start, end, .. }) => {
            editor.active_mut().document.selection =
                wordcartel_core::selection::Selection::range(start, end);
            crate::derive::rebuild(editor);
            nav::ensure_visible(editor);
        }
        None => { editor.set_status(crate::status::StatusKind::Info, "no marked block"); }
    }
}

/// Normalize `(a, b)` to `(start, end)` where start <= end, then set marked_block.
/// Rejects an empty block (start == end) with status "empty block".
fn set_block(editor: &mut Editor, a: usize, b: usize) {
    let (start, end) = (a.min(b), a.max(b));
    if start == end {
        editor.set_status(crate::status::StatusKind::Info, "empty block");
        return;
    }
    editor.active_mut().marked_block = Some(MarkedBlock { start, end, hidden: false });
    editor.set_status(crate::status::StatusKind::Info, "block marked");
}

#[cfg(test)]
mod tests {
    use crate::editor::{Editor, MarkedBlock};

    // --- Task 3: ops tests ---
    struct TestClock(u64);
    impl wordcartel_core::history::Clock for TestClock { fn now_ms(&self) -> u64 { self.0 } }

    #[test]
    fn block_copy_inserts_at_caret_and_keeps_block() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false }); // "hello"
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(11); // before "\n"
        crate::blocks_marked::block_copy(&mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "hello worldhello\n");
        assert!(e.active().marked_block.is_some(), "block stays after copy");
        assert_eq!(e.active().document.selection.primary().head, 16, "caret at end of inserted text");
    }

    /// Pins the documented caret-inside-block copy GROW behavior (see `block_copy`
    /// doc): with the caret strictly inside `[start, end)`, the inserted duplicate
    /// lands within the block, `end` maps past the insert, and the block grows to
    /// span the original text plus its copy. Non-destructive by design (no guard,
    /// unlike `block_move`).
    #[test]
    fn block_copy_caret_inside_grows_block() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        // Block = "hello world" (0..11). Caret strictly inside at 5 (after "hello").
        e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 11, hidden: false });
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5);
        crate::blocks_marked::block_copy(&mut e, &TestClock(0));
        // The 11-byte block text is duplicated at offset 5 → buffer gains the copy.
        assert_eq!(e.active().document.buffer.to_string(), "hellohello world world\n");
        let b = e.active().marked_block.expect("block survives copy");
        // Block grows: start unchanged, end advances past the 11-byte insert (11→22).
        assert_eq!(b.start, 0, "start anchored");
        assert_eq!(b.end, 22, "end mapped past the inserted duplicate → block grew");
    }

    #[test]
    fn block_move_relocates_and_clears_one_undo() {
        let mut e = Editor::new_from_text("AAA BBB\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false }); // "AAA "
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(7); // end (before \n)
        crate::blocks_marked::block_move(&mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "BBBAAA \n"); // "AAA " moved to caret
        assert!(e.active().marked_block.is_none(), "block consumed by move");
        let before = e.active().document.buffer.to_string();
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "AAA BBB\n", "one undo step restores");
        let _ = before;
    }

    /// T8 (C-7): a FOLDED section moved via block_move stays folded at its DESTINATION byte, and the
    /// vacated original byte is NOT folded. Asserts the SPECIFIC relocated heading byte — a stale fold
    /// at the wrong heading would pass a bare `len == 1` (plan-gate finding 6). The stationary-heading-
    /// at-destination arithmetic is proven by `corrected_after_move_stationary_at_destination_caret_
    /// advances_past_block` in fold.rs.
    #[test]
    fn block_move_keeps_a_folded_section_folded_at_its_new_byte() {
        let doc = "intro para.\n\n## A\n\nbody a.\n";
        let mut e = Editor::new_from_text(doc, None, (60, 20));
        crate::derive::rebuild(&mut e);
        let a = doc.find("## A").unwrap(); // 13
        e.active_mut().folds.toggle(a);
        let (b_from, b_to) = crate::commands::section_range_at(&e, a + 1).unwrap();
        e.active_mut().marked_block = Some(MarkedBlock { start: b_from, end: b_to, hidden: false });
        // caret BEFORE the block → dest = caret = 0.
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        crate::blocks_marked::block_move(&mut e, &TestClock(0));
        let folded = e.active().folds.folded();
        assert!(folded.contains(&0), "A's heading is folded at its NEW byte 0: {folded:?}");
        assert!(!folded.contains(&a), "the fold did NOT stay at the vacated original byte {a}");
        assert_eq!(folded.len(), 1, "exactly one fold — no double, no drop");
    }

    /// FIX 2 (Fable/Codex must-fix): a `block_move` of a FOLDED section must leave the caret on a
    /// VISIBLE line. The fold correction re-folds the destination section around the post-move
    /// caret, so without the SnapOut guard the head lands on a fold-hidden line where typing would
    /// edit invisible text.
    #[test]
    fn block_move_of_folded_section_snaps_caret_out_of_hidden_line() {
        let doc = "intro para.\n\n## A\n\nbody a line.\n\nmore text after.\n";
        let mut e = Editor::new_from_text(doc, None, (60, 20));
        crate::derive::rebuild(&mut e);
        let a = doc.find("## A").unwrap();
        e.active_mut().folds.toggle(a); // fold section A
        let (b_from, b_to) = crate::commands::section_range_at(&e, a + 1).unwrap();
        e.active_mut().marked_block = Some(MarkedBlock { start: b_from, end: b_to, hidden: false });
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0); // caret before block
        crate::blocks_marked::block_move(&mut e, &TestClock(0));
        let fold_view = e.active_fold_view();
        let head = e.active().document.selection.primary().head;
        let caret_line = e.active().document.buffer.byte_to_line(head);
        assert!(!fold_view.is_hidden(caret_line),
            "caret line {caret_line} must be visible after moving a folded section (snapped out of the fold)");
    }

    /// H22 Task 5 regression tripwire: a folded heading section moved PAST the caret (the `caret >=
    /// b.end` branch — the mirror of the `caret < b.start` branch the two tests above exercise) keeps
    /// its fold at the destination byte through the core-backed `apply_edit`, and the caret is never
    /// left on a hidden line. Must stay green before AND after the Surface C migration (behavior-
    /// preserving, §3.6) — grounded on the `corrected_after_move` fixtures in `fold.rs:536-564`.
    #[test]
    fn block_move_preserves_a_folded_region_through_the_core() {
        let doc = "## A\n\nbody a.\n\n## B\n\nbody b.\n";
        let mut e = Editor::new_from_text(doc, None, (60, 20));
        crate::derive::rebuild(&mut e);
        let a = doc.find("## A").unwrap(); // 0
        let len = doc.len();
        e.active_mut().folds.toggle(a); // fold section A
        let (b_from, b_to) = crate::commands::section_range_at(&e, a + 1).unwrap(); // A's own range
        e.active_mut().marked_block = Some(MarkedBlock { start: b_from, end: b_to, hidden: false });
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(len); // caret at end, past B
        crate::blocks_marked::block_move(&mut e, &TestClock(0));
        let dest = len - (b_to - b_from); // A relocates to just before the (now-shifted) caret
        let folded = e.active().folds.folded();
        assert!(folded.contains(&dest), "A's heading is folded at its NEW byte {dest}: {folded:?}");
        assert!(!folded.contains(&a), "the fold did NOT stay at the vacated original byte {a}");
        assert_eq!(folded.len(), 1, "exactly one fold — no double, no drop");
        let fold_view = e.active_fold_view();
        let head = e.active().document.selection.primary().head;
        let caret_line = e.active().document.buffer.byte_to_line(head);
        assert!(!fold_view.is_hidden(caret_line),
            "caret line {caret_line} must be visible after moving a folded section past the caret");
    }

    #[test]
    fn block_move_into_itself_is_noop() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false });
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2); // inside
        crate::blocks_marked::block_move(&mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "hello\n");
        assert_eq!(e.status_text(), "can't move a block into itself");
    }

    #[test]
    fn block_delete_removes_and_clears() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 5, end: 11, hidden: false }); // " world"
        crate::blocks_marked::block_delete(&mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "hello\n");
        assert!(e.active().marked_block.is_none());
    }

    #[test]
    fn ops_with_no_block_status() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::blocks_marked::block_copy(&mut e, &TestClock(0));
        assert_eq!(e.status_text(), "no marked block");
    }

    #[test]
    fn begin_then_end_forms_normalized_block() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(11);
        crate::blocks_marked::block_begin(&mut e); // pending at 11
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(6);
        crate::blocks_marked::block_end(&mut e);    // end at 6 → normalize (6,11)
        assert_eq!(e.active().marked_block, Some(MarkedBlock { start: 6, end: 11, hidden: false }));
        assert!(e.active().pending_block_begin.is_none());
    }

    #[test]
    fn end_without_begin_is_noop() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::blocks_marked::block_end(&mut e);
        assert!(e.active().marked_block.is_none());
        assert_eq!(e.status_text(), "set block begin first");
    }

    #[test]
    fn empty_block_rejected() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        crate::blocks_marked::block_begin(&mut e);
        crate::blocks_marked::block_end(&mut e); // begin==end==2 → reject
        assert!(e.active().marked_block.is_none());
        assert!(e.active().pending_block_begin.is_none(), "pending cleared even on empty-reject");
        assert_eq!(e.status_text(), "empty block");
    }

    #[test]
    fn promote_sets_block_and_clears_selection() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5); // "hello"
        crate::blocks_marked::mark_block_from_selection(&mut e);
        assert_eq!(e.active().marked_block, Some(MarkedBlock { start: 0, end: 5, hidden: false }));
        assert!(e.active().document.selection.primary().is_empty(), "selection converted → cleared");
    }

    // --- A11.3: block → selection bridge ---

    #[test]
    fn select_marked_block_selects_range_and_keeps_block() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 5, hidden: false }); // "hello"
        crate::blocks_marked::select_marked_block(&mut e);
        let sel = e.active().document.selection.primary();
        assert_eq!(sel.from(), 0);
        assert_eq!(sel.to(), 5);
        assert!(e.active().marked_block.is_some(), "block survives select");
    }

    #[test]
    fn select_marked_block_no_block_sets_status() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        let before = e.active().document.selection.clone();
        crate::blocks_marked::select_marked_block(&mut e);
        assert_eq!(e.active().document.selection, before, "selection unchanged with no block");
        assert!(!e.status_text().is_empty());
    }
}
