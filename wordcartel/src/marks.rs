//! Named marks + jump-ring command bodies (5c).
use crate::editor::{Buffer, Editor, MarkPending};
use crate::nav;
use crate::registry::{place_caret_visible, CaretPlace};

const CAP: usize = 64;

pub fn set_mark(editor: &mut Editor)  { editor.active_mut().sel_history.clear(); editor.pending_mark = Some(MarkPending::Set); editor.status = "set mark:".into(); }
pub fn jump_to_mark(editor: &mut Editor) { editor.pending_mark = Some(MarkPending::Jump); editor.status = "jump to mark:".into(); }

/// Push `pre` onto the ring as a deliberate jump origin.
pub fn record_jump(buf: &mut Buffer, pre: usize) {
    if buf.ring_cursor < buf.jump_ring.len() {
        buf.jump_ring.truncate(buf.ring_cursor); // drop stale forward tail
    }
    if buf.jump_ring.last() != Some(&pre) {
        buf.jump_ring.push(pre);
        if buf.jump_ring.len() > CAP { buf.jump_ring.remove(0); }
    }
    buf.ring_cursor = buf.jump_ring.len();
}

pub fn jump_back(editor: &mut Editor) {
    editor.active_mut().sel_history.clear();
    let here = nav::head(editor);
    let raw: Option<usize> = {
        let buf = editor.active_mut();
        if buf.ring_cursor == buf.jump_ring.len() {
            // parked at the live caret — record it as the forward anchor
            if buf.jump_ring.last() != Some(&here) {
                buf.jump_ring.push(here);
                if buf.jump_ring.len() > CAP {
                    buf.jump_ring.remove(0);
                    if buf.ring_cursor > 0 { buf.ring_cursor -= 1; }
                }
            }
        }
        if buf.ring_cursor == 0 {
            None
        } else {
            buf.ring_cursor -= 1;
            Some(buf.jump_ring[buf.ring_cursor])
        }
    }; // <- mutable borrow ends here
    let Some(raw) = raw else { editor.status = "ring: at oldest".into(); return; };
    let off = nav::clamp_snap(editor, raw);
    let off = place_caret_visible(editor, off, CaretPlace::UnfoldTo);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
    crate::derive::rebuild(editor);
    nav::ensure_visible(editor);
}

pub fn jump_forward(editor: &mut Editor) {
    editor.active_mut().sel_history.clear();
    let raw: Option<usize> = {
        let buf = editor.active_mut();
        if buf.ring_cursor + 1 >= buf.jump_ring.len() {
            None
        } else {
            buf.ring_cursor += 1;
            Some(buf.jump_ring[buf.ring_cursor])
        }
    };
    let Some(raw) = raw else { editor.status = "ring: at newest".into(); return; };
    let off = nav::clamp_snap(editor, raw);
    let off = place_caret_visible(editor, off, CaretPlace::UnfoldTo);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
    crate::derive::rebuild(editor);
    nav::ensure_visible(editor);
}

/// Store a mark at the caret under `ch` (no status — caller sets wording).
/// Clears `sel_history` to match the interactive `set_mark` path (marks.rs:8) so a
/// numbered-bookmark set resets the expand-selection ladder identically (Codex).
pub fn set_char_mark(editor: &mut Editor, ch: char) {
    editor.active_mut().sel_history.clear();
    let at = nav::head(editor);
    editor.active_mut().marks.insert(ch, at);
}

/// Jump to mark `ch` if set (fold-aware, records jump-back). Returns whether it existed.
/// No status — caller sets wording.
pub fn jump_char_mark(editor: &mut Editor, ch: char) -> bool {
    editor.active_mut().sel_history.clear();
    let raw = editor.active().marks.get(&ch).copied();
    let Some(raw) = raw else { return false; };
    let pre = nav::head(editor);
    record_jump(editor.active_mut(), pre);
    let off = nav::clamp_snap(editor, raw);
    let off = place_caret_visible(editor, off, CaretPlace::UnfoldTo);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
    crate::derive::rebuild(editor);
    nav::ensure_visible(editor);
    true
}

/// Apply the captured mark char for the pending operation.
pub fn resolve_pending(editor: &mut Editor, ch: char) {
    match editor.pending_mark.take() {
        Some(MarkPending::Set) => {
            set_char_mark(editor, ch);
            editor.status = format!("mark {ch} set");
        }
        Some(MarkPending::Jump) => {
            if jump_char_mark(editor, ch) {
                editor.status = format!("jumped to mark {ch}");
            } else {
                editor.status = format!("no mark {ch}");
            }
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::editor::{Editor, MarkPending};

    #[test]
    fn jump_back_and_forward_walk_the_ring() {
        let mut e = Editor::new_from_text("0123456789\n", None, (80, 24));
        // simulate two deliberate jumps from 0 → 5 → 9
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        super::record_jump(e.active_mut(), 0);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5);
        super::record_jump(e.active_mut(), 5);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(9);
        // back → 5, back → 0
        super::jump_back(&mut e);
        assert_eq!(e.active().document.selection.primary().head, 5);
        super::jump_back(&mut e);
        assert_eq!(e.active().document.selection.primary().head, 0);
        // forward → 5
        super::jump_forward(&mut e);
        assert_eq!(e.active().document.selection.primary().head, 5);
    }

    #[test]
    fn set_then_jump_mark_round_trips() {
        let mut e = Editor::new_from_text("line0\nline1\nline2\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(6); // line1
        super::set_mark(&mut e);
        assert_eq!(e.pending_mark, Some(MarkPending::Set));
        super::resolve_pending(&mut e, 'a');
        assert_eq!(e.active().marks.get(&'a'), Some(&6));
        assert_eq!(e.pending_mark, None);
        // move away, then jump back to 'a'
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        super::jump_to_mark(&mut e);
        super::resolve_pending(&mut e, 'a');
        assert_eq!(e.active().document.selection.primary().head, 6);
    }

    #[test]
    fn jump_to_mark_into_fold_reveals_target() {
        let doc = "# Top\nintro\n## A\nbody1\nbody2\n## B\n";
        let mut e = Editor::new_from_text(doc, None, (80, 24));
        crate::derive::rebuild(&mut e);
        let a = doc.find("## A").unwrap();
        let target = doc.find("body2").unwrap();

        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(target);
        super::set_mark(&mut e);
        super::resolve_pending(&mut e, 'a');
        e.active_mut().folds.toggle(a);
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);

        super::jump_to_mark(&mut e);
        super::resolve_pending(&mut e, 'a');

        assert_eq!(e.active().document.selection.primary().head, target);
        assert!(!e.active().folds.folded().contains(&a));
    }

    #[test]
    fn bookmark_set_and_jump_round_trips() {
        let mut e = Editor::new_from_text("line0\nline1\nline2\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(6); // line1
        super::set_char_mark(&mut e, '3');
        assert_eq!(e.active().marks.get(&'3'), Some(&6));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert!(super::jump_char_mark(&mut e, '3'), "found");
        assert_eq!(e.active().document.selection.primary().head, 6);
    }

    #[test]
    fn jump_unset_bookmark_returns_false_and_does_not_move() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        assert!(!super::jump_char_mark(&mut e, '7'), "unset → false");
        assert_eq!(e.active().document.selection.primary().head, 2, "no move");
    }

    #[test]
    fn bookmark_shares_slot_with_interactive_char_mark() {
        let mut e = Editor::new_from_text("0123456789\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5);
        super::set_mark(&mut e);            // interactive
        super::resolve_pending(&mut e, '5'); // stores under '5'
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert!(super::jump_char_mark(&mut e, '5'), "bookmark 5 == char-mark '5'");
        assert_eq!(e.active().document.selection.primary().head, 5);
    }

    #[test]
    fn jump_bookmark_into_fold_reveals_target() {
        // Mirror jump_to_mark_into_fold_reveals_target with set_char_mark/jump_char_mark.
        let doc = "# Top\nintro\n## A\nbody1\nbody2\n## B\n";
        let mut e = Editor::new_from_text(doc, None, (80, 24));
        crate::derive::rebuild(&mut e);
        let a = doc.find("## A").unwrap();
        let target = doc.find("body2").unwrap();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(target);
        super::set_char_mark(&mut e, '1');
        e.active_mut().folds.toggle(a);
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert!(super::jump_char_mark(&mut e, '1'));
        assert_eq!(e.active().document.selection.primary().head, target);
        assert!(!e.active().folds.folded().contains(&a));
    }
}
