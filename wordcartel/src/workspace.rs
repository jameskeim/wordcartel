//! Effort 6: multi-buffer workspace navigation + lifecycle.
use crate::editor::{Editor, BufferId};

/// Display name for a buffer: `*scratch*` for the scratch buffer,
/// `*untitled*` for a path-less ordinary buffer, else the filename.
/// Prefixed with `*` when the buffer is dirty (`is_dirty` — scratch excluded).
pub fn buffer_display_name(editor: &Editor, id: BufferId) -> String {
    let base = if editor.is_scratch(id) {
        "*scratch*".to_string()
    } else {
        match editor.by_id(id).and_then(|b| b.document.path.as_ref()) {
            Some(p) => p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned()),
            None => "*untitled*".to_string(),
        }
    };
    if editor.is_dirty(id) { format!("*{base}") } else { base }
}

/// Return buffers in MRU order (most-recent first) as `(id, display_name)` pairs.
/// Buffers not yet in the MRU list are appended in buffer-vec order.
pub fn buffer_switch_rows(editor: &Editor) -> Vec<(BufferId, String)> {
    let mut out: Vec<(BufferId, String)> = Vec::new();
    for &id in &editor.mru {
        if editor.by_id(id).is_some() {
            out.push((id, buffer_display_name(editor, id)));
        }
    }
    for b in &editor.buffers {
        if !out.iter().any(|(id, _)| *id == b.id) {
            out.push((b.id, buffer_display_name(editor, b.id)));
        }
    }
    out
}

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
    fn buffer_display_name_scratch_untitled_and_named() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        e.install_scratch();
        let sid = e.scratch_id.unwrap();
        let buf0_id = e.buffers[0].id;
        assert_eq!(buffer_display_name(&e, sid), "*scratch*", "scratch always shows as *scratch*");
        assert_eq!(buffer_display_name(&e, buf0_id), "*untitled*", "path-less ordinary buffer is *untitled*");
        e.buffers[0].document.path = Some(std::path::PathBuf::from("/home/user/notes.md"));
        assert_eq!(buffer_display_name(&e, buf0_id), "notes.md", "shows filename only");
    }

    #[test]
    fn switcher_rows_mru_order_with_display_names() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch();
        // Make buffer 0 a named file display, scratch second.
        e.buffers[0].document.path = Some(std::path::PathBuf::from("/tmp/notes.md"));
        goto_scratch(&mut e);     // MRU front = scratch
        let rows = buffer_switch_rows(&e);
        assert_eq!(rows.first().unwrap().1, "*scratch*", "MRU front is scratch");
        assert!(rows.iter().any(|(_, n)| n.contains("notes.md")));
    }

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
