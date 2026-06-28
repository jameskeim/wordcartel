//! Persistent marked-block creation (Effort 9A Task 2).
//! ^KB = block_begin, ^KK = block_end, promote-from-selection.

use crate::editor::{Editor, MarkedBlock};
use crate::nav;

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
