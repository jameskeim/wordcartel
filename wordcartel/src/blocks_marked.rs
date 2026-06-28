//! Persistent marked-block creation and operations (Effort 9A Tasks 2–3).
//! ^KB = block_begin, ^KK = block_end, promote-from-selection.
//! Task 3 adds: copy/move/delete/jump/hide/clear.

use crate::editor::{Editor, MarkedBlock};
use crate::nav;
use wordcartel_core::history::Clock;

// --- Task 3: act-on-block operations ---

fn block(editor: &Editor) -> Option<crate::editor::MarkedBlock> { editor.active().marked_block }

pub fn block_copy(editor: &mut Editor, clock: &dyn Clock) {
    let Some(b) = block(editor) else { editor.status = "no marked block".into(); return; };
    let text = editor.active().document.buffer.slice(b.start..b.end);
    let caret = nav::head(editor);
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = crate::commands::build_multi_replace(&[(caret, caret, text.clone())], doc_len);
    let new_caret = caret + text.len();
    apply_edit(editor, cs, edit, new_caret, clock);
    // block stays — its endpoints map through the insertion via apply.
    editor.status = "block copied".into();
}

pub fn block_move(editor: &mut Editor, clock: &dyn Clock) {
    let Some(b) = block(editor) else { editor.status = "no marked block".into(); return; };
    let caret = nav::head(editor);
    if caret >= b.start && caret < b.end {
        editor.status = "can't move a block into itself".into();
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
    apply_edit(editor, cs, edit, new_caret, clock);
    editor.active_mut().marked_block = None; // consumed
    editor.status = "block moved".into();
}

pub fn block_delete(editor: &mut Editor, clock: &dyn Clock) {
    let Some(b) = block(editor) else { editor.status = "no marked block".into(); return; };
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = crate::commands::build_multi_replace(&[(b.start, b.end, String::new())], doc_len);
    apply_edit(editor, cs, edit, b.start, clock);
    editor.active_mut().marked_block = None;
    editor.status = "block deleted".into();
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
    crate::derive::rebuild(editor);
    nav::ensure_visible(editor);
    editor.active_mut().desired_col = None;
}

pub fn block_jump_begin(editor: &mut Editor) { block_jump(editor, true); }
pub fn block_jump_end(editor: &mut Editor)   { block_jump(editor, false); }
fn block_jump(editor: &mut Editor, to_start: bool) {
    let Some(b) = block(editor) else { editor.status = "no marked block".into(); return; };
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
            editor.status = if h { "block hidden".into() } else { "block shown".into() };
        }
        None => editor.status = "no marked block".into(),
    }
}

pub fn block_clear(editor: &mut Editor) {
    editor.active_mut().marked_block = None;
    editor.active_mut().pending_block_begin = None;
    editor.status = "block cleared".into();
}

/// ^KW: open the Write-Block minibuffer pre-filled with the document's directory.
pub fn block_write(editor: &mut Editor) {
    if editor.active().marked_block.is_none() {
        editor.status = "no marked block".into();
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
    editor.status = "block begin set".into();
}

/// Complete the block from pending begin to current caret (^KK).
/// Normalizes so start <= end; rejects empty; clears pending on success or error.
pub fn block_end(editor: &mut Editor) {
    let Some(begin) = editor.active().pending_block_begin else {
        editor.status = "set block begin first".into();
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
        editor.status = "no selection to mark".into();
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

/// Normalize `(a, b)` to `(start, end)` where start <= end, then set marked_block.
/// Rejects an empty block (start == end) with status "empty block".
fn set_block(editor: &mut Editor, a: usize, b: usize) {
    let (start, end) = (a.min(b), a.max(b));
    if start == end {
        editor.status = "empty block".into();
        return;
    }
    editor.active_mut().marked_block = Some(MarkedBlock { start, end, hidden: false });
    editor.status = "block marked".into();
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

    #[test]
    fn block_move_into_itself_is_noop() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false });
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2); // inside
        crate::blocks_marked::block_move(&mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "hello\n");
        assert_eq!(e.status, "can't move a block into itself");
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
        assert_eq!(e.status, "no marked block");
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
        assert_eq!(e.status, "set block begin first");
    }

    #[test]
    fn empty_block_rejected() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        crate::blocks_marked::block_begin(&mut e);
        crate::blocks_marked::block_end(&mut e); // begin==end==2 → reject
        assert!(e.active().marked_block.is_none());
        assert!(e.active().pending_block_begin.is_none(), "pending cleared even on empty-reject");
        assert_eq!(e.status, "empty block");
    }

    #[test]
    fn promote_sets_block_and_clears_selection() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5); // "hello"
        crate::blocks_marked::mark_block_from_selection(&mut e);
        assert_eq!(e.active().marked_block, Some(MarkedBlock { start: 0, end: 5, hidden: false }));
        assert!(e.active().document.selection.primary().is_empty(), "selection converted → cleared");
    }
}
