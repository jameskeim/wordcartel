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

/// True iff the active buffer is a reusable empty untitled throwaway (NOT scratch).
/// Reuse rule: path-less, not dirty, content `""` or `"\n"`, and not the scratch buffer.
pub fn active_is_reusable_throwaway(editor: &Editor) -> bool {
    let b = editor.active();
    if editor.is_scratch(b.id) { return false; }
    if b.document.path.is_some() { return false; }
    if editor.is_dirty(b.id) { return false; }
    let t = b.document.buffer.to_string();
    t.is_empty() || t == "\n"
}

/// Open `path` additively: reuse a throwaway active buffer in-place, else push a new buffer and switch.
pub fn open_as_new_buffer(editor: &mut Editor, path: &std::path::Path) {
    if active_is_reusable_throwaway(editor) {
        crate::app::open_into_current(editor, path); // replace-in-place seam
        return;
    }
    let id = editor.alloc_id();
    let area = editor.active().view.area;
    match crate::editor::Buffer::from_file(id, path, area) {
        Ok(b) => {
            editor.buffers.push(b);
            let idx = editor.buffers.len() - 1;
            editor.switch_to_index(idx);
            if editor.resume_enabled { crate::app::restore_resume(editor, path); }
            crate::derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
            editor.status = String::new();
        }
        Err(e) => editor.status = e.to_string(),
    }
}

/// Close the active buffer. Scratch → no-op. Dirty → refuse (keep work; the quit
/// flow handles interactive save). Last ordinary buffer → replace with a fresh
/// empty untitled. New active = same-index neighbor.
pub fn close_buffer(editor: &mut Editor) {
    let id = editor.active().id;
    if editor.is_scratch(id) { editor.status = "can't close the scratch buffer".into(); return; }
    if editor.is_dirty(id) { editor.status = "unsaved changes — save or discard first".into(); return; }
    let ordinary = editor.buffers.iter().filter(|b| !editor.is_scratch(b.id)).count();
    if ordinary <= 1 {
        // Last ordinary buffer: replace in place with a fresh empty untitled.
        let nid = editor.alloc_id();
        let area = editor.active().view.area;
        let a = editor.active;
        editor.buffers[a] = crate::editor::Buffer::from_text(nid, "\n", None, area);
        editor.mru.retain(|&x| x != id);
        editor.touch_mru(nid);
        crate::derive::rebuild(editor);
        crate::nav::ensure_visible(editor);
        editor.status = String::new();
        return;
    }
    let a = editor.active;
    editor.mru.retain(|&x| x != id);
    editor.buffers.remove(a);
    let new_idx = a.min(editor.buffers.len() - 1);
    switch_to(editor, new_idx);
    editor.status = String::new();
}

/// Create a fresh empty untitled buffer additively (no-op when active is already a reusable throwaway).
pub fn new_empty_buffer(editor: &mut Editor) {
    if active_is_reusable_throwaway(editor) { return; } // already an empty untitled — nothing to do
    let id = editor.alloc_id();
    let area = editor.active().view.area;
    editor.buffers.push(crate::editor::Buffer::from_text(id, "\n", None, area));
    let idx = editor.buffers.len() - 1;
    editor.switch_to_index(idx);
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
    editor.status = String::new();
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

    // -------------------------------------------------------------------------
    // Task 6: additive open/new + throwaway reuse
    // -------------------------------------------------------------------------

    #[test]
    fn open_reuses_clean_untitled_throwaway() {
        let mut e = Editor::new_from_text("\n", None, (40, 10)); // throwaway launch buffer
        e.install_scratch();
        assert_eq!(e.buffers.len(), 2);
        let tmp = std::env::temp_dir().join(format!("wc-open-{}.md", std::process::id()));
        std::fs::write(&tmp, "file body\n").unwrap();
        open_as_new_buffer(&mut e, &tmp);
        assert_eq!(e.buffers.len(), 2, "throwaway reused, not added");
        assert_eq!(e.active().document.buffer.to_string(), "file body\n");
        let _ = std::fs::remove_file(&tmp);
    }

    /// Regression: open_into_current (reached via throwaway-reuse path) must update MRU.
    /// Before the fix: old throwaway id remained in MRU as a ghost; new id was absent.
    #[test]
    fn open_into_current_updates_mru_no_ghost() {
        let tmp = std::env::temp_dir().join(format!("wc-oic-mru-{}.md", std::process::id()));
        std::fs::write(&tmp, "content\n").unwrap();
        let mut e = Editor::new_from_text("\n", None, (40, 10)); // clean empty untitled throwaway
        e.install_scratch(); // seeds MRU as [doc, scratch]
        let old_id = e.active().id; // throwaway id (doc buffer)
        assert!(e.mru.contains(&old_id), "throwaway id must be in MRU before open");
        // open_as_new_buffer takes the reuse path → calls open_into_current
        assert!(active_is_reusable_throwaway(&e), "pre-condition: active must be throwaway");
        open_as_new_buffer(&mut e, &tmp);
        let new_id = e.active().id;
        assert_ne!(new_id, old_id, "fresh id allocated for new buffer");
        assert!(!e.mru.contains(&old_id), "ghost old throwaway id must NOT remain in MRU");
        assert!(e.mru.contains(&new_id), "new buffer id must be in MRU");
        assert_eq!(e.mru[0], new_id, "new buffer must be at MRU front");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn open_adds_buffer_when_active_is_real() {
        let mut e = Editor::new_from_text("real content\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch();
        let tmp = std::env::temp_dir().join(format!("wc-open2-{}.md", std::process::id()));
        std::fs::write(&tmp, "second\n").unwrap();
        open_as_new_buffer(&mut e, &tmp);
        assert_eq!(e.buffers.len(), 3, "added a new buffer");
        assert_eq!(e.active().document.buffer.to_string(), "second\n");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn new_empty_buffer_is_additive_and_not_scratch() {
        let mut e = Editor::new_from_text("real\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch();
        new_empty_buffer(&mut e);
        assert_eq!(e.buffers.len(), 3);
        assert!(e.active().document.path.is_none());
        assert!(!e.is_scratch(e.active().id), "New buffer is not the scratch buffer");
    }

    #[test]
    fn scratch_is_never_a_reuse_target() {
        let mut e = Editor::new_from_text("real\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch();
        goto_scratch(&mut e); // active = scratch (empty, path-less, "clean")
        assert!(!active_is_reusable_throwaway(&e), "scratch must not be reused");
    }

    // -------------------------------------------------------------------------
    // Task 7: close_buffer
    // -------------------------------------------------------------------------

    #[test]
    fn close_scratch_is_noop_with_status() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch();
        goto_scratch(&mut e);
        close_buffer(&mut e);
        assert_eq!(e.buffers.len(), 2, "scratch not closed");
        assert!(e.status.contains("scratch"));
    }

    #[test]
    fn close_last_ordinary_leaves_fresh_untitled() {
        let mut e = Editor::new_from_text("only\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch(); // [a.md, scratch]
        close_buffer(&mut e); // close a.md → invariant keeps ≥1 ordinary
        assert_eq!(e.buffers.len(), 2, "scratch + a fresh untitled");
        assert!(!e.is_scratch(e.active().id));
        assert!(e.active().document.path.is_none(), "fresh untitled");
    }

    #[test]
    fn close_selects_same_index_neighbor() {
        let mut e = Editor::new_from_text("first\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch();
        let tmp = std::env::temp_dir().join(format!("wc-c-{}.md", std::process::id()));
        std::fs::write(&tmp, "second\n").unwrap();
        open_as_new_buffer(&mut e, &tmp); // [a.md(0), scratch(1), second(2)] active=2
        switch_to(&mut e, 0); // active a.md
        close_buffer(&mut e); // remove index 0 → neighbor shifts into slot 0
        assert!(
            e.buffers.iter().all(|b| b.document.path.as_deref() != Some(std::path::Path::new("/tmp/a.md"))),
            "closed buffer a.md should be gone"
        );
        assert!(
            e.buffers.iter().any(|b| b.document.path.as_deref() == Some(tmp.as_path())),
            "neighbor (second file) should still be present"
        );
        assert_eq!(e.active, 0, "same-index neighbor active");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn close_last_ordinary_prunes_old_mru_id() {
        let mut e = Editor::new_from_text("only\n", None, (40, 10));
        e.install_scratch();
        let old_id = e.active().id;
        // Simulate the buffer having been visited so its id is in MRU.
        e.touch_mru(old_id);
        assert!(e.mru.contains(&old_id), "old id must be in MRU before close");
        close_buffer(&mut e); // last-ordinary path: replaces in place
        assert!(!e.mru.contains(&old_id), "old buffer id should be pruned from MRU after close");
        let new_id = e.active().id;
        assert!(e.mru.contains(&new_id), "new untitled buffer id should be in MRU");
    }

    #[test]
    fn close_refuses_dirty_buffer() {
        use wordcartel_core::history::Clock;
        struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let mut e = Editor::new_from_text("x\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch();
        let aid = e.active().id;
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "z".into())], 2);
        let txn = wordcartel_core::history::Transaction::new(cs).with_selection(wordcartel_core::selection::Selection::single(1));
        e.by_id_mut(aid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
        close_buffer(&mut e);
        assert!(e.by_id(aid).is_some(), "dirty buffer not closed");
        assert!(e.status.to_lowercase().contains("unsaved") || e.status.to_lowercase().contains("save"));
    }
}
