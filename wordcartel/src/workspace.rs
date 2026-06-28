//! Effort 6: multi-buffer workspace navigation + lifecycle.
use crate::editor::Editor;

/// Switch active buffer by index and refresh the view.
pub fn switch_to(editor: &mut Editor, idx: usize) {
    editor.switch_to_index(idx);
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}
pub fn next_buffer(editor: &mut Editor) { cycle(editor, 1); }
pub fn prev_buffer(editor: &mut Editor) { cycle(editor, -1); }
fn cycle(editor: &mut Editor, delta: isize) {
    let n = editor.buffers.len();
    if n <= 1 { return; }
    let idx = ((editor.active as isize + delta).rem_euclid(n as isize)) as usize;
    switch_to(editor, idx);
}
/// Jump directly to the scratch buffer (no-op if none installed).
pub fn goto_scratch(editor: &mut Editor) {
    if let Some(sid) = editor.scratch_id {
        if let Some(idx) = editor.buffers.iter().position(|b| b.id == sid) {
            switch_to(editor, idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cycle_wraps_in_stable_order_including_scratch() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch(); // indices [0 doc, 1 scratch]
        assert_eq!(e.active, 0);
        next_buffer(&mut e); assert_eq!(e.active, 1);
        next_buffer(&mut e); assert_eq!(e.active, 0, "wraps");
        prev_buffer(&mut e); assert_eq!(e.active, 1, "prev wraps back");
    }
    #[test]
    fn goto_scratch_jumps_to_scratch() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch();
        goto_scratch(&mut e);
        assert_eq!(e.buffers[e.active].id, e.scratch_id.unwrap());
    }
    #[test]
    fn cycle_single_buffer_is_noop() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10)); // no scratch → 1 buffer
        next_buffer(&mut e);
        assert_eq!(e.active, 0);
    }
}
