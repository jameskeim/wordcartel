//! Named marks + jump-ring command bodies (5c).
use crate::editor::{Buffer, Editor, MarkPending};
use crate::nav;

pub fn set_mark(editor: &mut Editor)  { editor.active_mut().sel_history.clear(); editor.pending_mark = Some(MarkPending::Set); editor.status = "set mark:".into(); }
pub fn jump_to_mark(editor: &mut Editor) { editor.pending_mark = Some(MarkPending::Jump); editor.status = "jump to mark:".into(); }

/// Push `pre` onto the ring as a deliberate jump origin (Task 9 fills in the
/// back/forward navigation; this is the shared push).
#[allow(dead_code)] // wired in Task 9
pub fn record_jump(buf: &mut Buffer, pre: usize) {
    const CAP: usize = 64;
    if buf.ring_cursor < buf.jump_ring.len() {
        buf.jump_ring.truncate(buf.ring_cursor); // drop stale forward tail
    }
    if buf.jump_ring.last() != Some(&pre) {
        buf.jump_ring.push(pre);
        if buf.jump_ring.len() > CAP { buf.jump_ring.remove(0); }
    }
    buf.ring_cursor = buf.jump_ring.len();
}

/// Apply the captured mark char for the pending operation.
pub fn resolve_pending(editor: &mut Editor, ch: char) {
    match editor.pending_mark.take() {
        Some(MarkPending::Set) => {
            let at = nav::head(editor);
            editor.active_mut().marks.insert(ch, at);
            editor.status = format!("mark {ch} set");
        }
        Some(MarkPending::Jump) => {
            editor.active_mut().sel_history.clear();
            let raw = editor.active().marks.get(&ch).copied(); // copy out → borrow ends
            if let Some(raw) = raw {
                let pre = nav::head(editor);
                record_jump(editor.active_mut(), pre);
                let off = nav::clamp_snap(editor, raw);
                editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
                crate::derive::rebuild(editor);
                nav::ensure_visible(editor);
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
}
