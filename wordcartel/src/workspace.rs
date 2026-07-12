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
/// Buffers not yet in the MRU list are appended in buffer-vec order. Scratch is
/// excluded (A12) — it is reached via `goto_scratch`/`toggle_scratch`, not the switcher.
pub fn buffer_switch_rows(editor: &Editor) -> Vec<(BufferId, String)> {
    let mut out: Vec<(BufferId, String)> = Vec::new();
    for &id in &editor.mru {
        if editor.by_id(id).is_some() && !editor.is_scratch(id) {
            out.push((id, buffer_display_name(editor, id)));
        }
    }
    for b in &editor.buffers {
        if !editor.is_scratch(b.id) && !out.iter().any(|(id, _)| *id == b.id) {
            out.push((b.id, buffer_display_name(editor, b.id)));
        }
    }
    out
}

/// Live rows for the Documents dynamic menu section (Task 4.2, `DYNAMIC_SECTIONS` seam):
/// one row per open buffer in OPEN ORDER (buffer-vec order, stable — NOT MRU), scratch
/// excluded. Data, not registered commands — exempt from the palette/registry surfaces
/// (command-surface contract's Task 4.3 amendment); the row action reaches the same
/// shared `switch_to` setter registered commands use.
pub fn documents_menu_rows(editor: &Editor) -> Vec<(String, crate::menu::MenuRowAction)> {
    editor.buffers.iter()
        .filter(|b| !editor.is_scratch(b.id))
        .map(|b| (buffer_display_name(editor, b.id), crate::menu::MenuRowAction::SwitchBuffer(b.id)))
        .collect()
}

/// Switch active buffer by index and refresh the view.
pub fn switch_to(editor: &mut Editor, idx: usize) {
    editor.switch_to_index(idx);
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}
pub fn next_buffer(editor: &mut Editor) { cycle(editor, 1); }
pub fn prev_buffer(editor: &mut Editor) { cycle(editor, -1); }
/// Cycle to the nearest non-scratch buffer in the requested direction (A12: scratch
/// is excluded from rotation). Anchors on the ACTIVE index — which IS scratch's real
/// `editor.buffers` index when active is scratch, since `install_scratch` pushes
/// scratch into `buffers` — and steps until it lands on an ordinary buffer.
fn cycle(editor: &mut Editor, delta: isize) {
    let n = editor.buffers.len();
    if n == 0 { return; }
    // Need at least two ordinary (non-scratch) buffers to rotate between.
    let ordinary = editor.buffers.iter().filter(|b| !editor.is_scratch(b.id)).count();
    if ordinary <= 1 { return; }
    let start = editor.active as isize;   // == scratch's index when active is scratch
    let step = if delta >= 0 { 1 } else { -1 };
    let mut i = start;
    loop {
        i = (i + step).rem_euclid(n as isize);
        let idx = i as usize;
        if !editor.is_scratch(editor.buffers[idx].id) { switch_to(editor, idx); return; }
        if i == start { return; } // full loop with no ordinary landing (guarded above)
    }
}

/// Shared scratch-entry mechanics for `goto_scratch` and `toggle_scratch`: records
/// the current buffer as the return target (unless already on scratch) before
/// switching to scratch.
fn enter_scratch(editor: &mut Editor) {
    let cur = editor.active().id;
    if !editor.is_scratch(cur) { editor.scratch_return = Some(cur); }
    if let Some(sid) = editor.scratch_id {
        if let Some(idx) = editor.buffers.iter().position(|b| b.id == sid) {
            switch_to(editor, idx);
        }
    }
}

/// Jump directly to the scratch buffer (no-op if none installed). Records the
/// current buffer as the `toggle_scratch` return target (A12 coherence).
pub fn goto_scratch(editor: &mut Editor) { enter_scratch(editor); }

/// Round-trip to/from the scratch buffer (A12). Not on scratch → record + go to scratch.
/// On scratch → return to the recorded buffer if it still resolves, else the MRU-front
/// ordinary buffer; else stay with a hint.
pub fn toggle_scratch(editor: &mut Editor) {
    if editor.is_scratch(editor.active().id) {
        // MRU is most-recent-FIRST (Editor::touch_mru does mru.insert(0, id)),
        // so iterate FORWARD (no .rev()) and take the first live, non-scratch id.
        let target = editor.scratch_return
            .filter(|id| editor.by_id(*id).is_some())
            .or_else(|| editor.mru.iter().copied()
                .find(|id| !editor.is_scratch(*id) && editor.by_id(*id).is_some())
                .or_else(|| editor.buffers.iter().map(|b| b.id)
                    .find(|id| !editor.is_scratch(*id))));
        match target.and_then(|id| editor.buffers.iter().position(|b| b.id == id)) {
            Some(idx) => switch_to(editor, idx),
            None => editor.status = "no other buffer".into(),
        }
    } else {
        enter_scratch(editor);
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
        crate::session_restore::open_into_current(editor, path); // replace-in-place seam
        return;
    }
    let id = editor.alloc_id();
    let area = editor.active().view.area;
    match crate::editor::Buffer::from_file(id, path, area) {
        Ok(b) => {
            editor.buffers.push(b);
            let idx = editor.buffers.len() - 1;
            editor.switch_to_index(idx);
            if editor.resume_enabled { crate::session_restore::restore_resume(editor, path); }
            crate::derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
            editor.status = String::new();
            crate::plugin::fire_event(editor, crate::plugin::PluginEventKind::Open, Some(path));
        }
        Err(e) => editor.status = e.to_string(),
    }
}

/// Close the active buffer. Scratch → no-op (status set). Dirty → raise the
/// Save/Discard/Cancel close-confirm prompt (C4) — unless another save/quit
/// flow has pending state, in which case refuse with a status (the shared
/// Cancel/Esc arms would clobber that flow's pendings — spec D1 busy guard).
/// Clean → close immediately. Last ordinary buffer → replaced with a fresh
/// empty untitled. New active = same-index neighbor.
pub fn close_buffer(editor: &mut Editor) {
    let id = editor.active().id;
    if editor.is_scratch(id) { editor.status = "can't close the scratch buffer".into(); return; }
    if editor.is_dirty(id) {
        if editor.pending_after_save.is_some() || editor.pending_save_as.is_some() || editor.quit_drain.is_some() {
            editor.status = "another save or quit is in progress — try again".into();
            return;
        }
        let name = buffer_display_name(editor, id);
        editor.open_prompt(crate::prompt::Prompt::close_confirm(&name, id));
        return;
    }
    close_buffer_now(editor, id);
}

/// Close `id` unconditionally (no dirty check) — the shared mechanics behind
/// the clean-path close, the Discard arm, and the post-save close (spec D2).
/// Per-case BY DESIGN: when `id` is not active, the viewer must not be yanked
/// (no switch_to), and the last-ordinary replacement targets id's OWN slot —
/// never buffers[active], which would overwrite the scratch and dangle
/// scratch_id.
pub(crate) fn close_buffer_now(editor: &mut Editor, id: BufferId) {
    let Some(i) = editor.buffers.iter().position(|b| b.id == id) else {
        editor.status = "buffer already closed".into();
        return;
    };
    // P2 on_buffer_close fire site: capture the path BEFORE the slot is removed/replaced —
    // covers all three close shapes below (this is the one place all of them funnel through).
    let closing = editor.by_id(id).and_then(|b| b.document.path.clone());
    // Effort A: tell the provider to abandon this doc's generation before the slot is removed or
    // replaced (all three shapes below) so the server never keeps a closed doc open until shutdown.
    // The last-ordinary replacement's fresh buffer re-opens lazily under its own new id/generation.
    editor.diag_provider.notify_close(id);
    crate::plugin::fire_event(editor, crate::plugin::PluginEventKind::BufferClose, closing.as_deref());
    let ordinary = editor.buffers.iter().filter(|b| !editor.is_scratch(b.id)).count();
    if ordinary <= 1 {
        // Last ordinary buffer: replace id's own slot with a fresh empty untitled.
        let nid = editor.alloc_id();
        let area = editor.buffers[i].view.area;
        let was_active = i == editor.active;
        editor.buffers[i] = crate::editor::Buffer::from_text(nid, "\n", None, area);
        editor.mru.retain(|&x| x != id);
        if was_active {
            editor.touch_mru(nid);
            crate::derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
        } else {
            // Untouched fresh buffer: back of the MRU, not most-recent (spec D2 —
            // fronting it would break the weak MRU-front == active convention).
            editor.mru.push(nid);
        }
        editor.status = String::new();
        return;
    }
    if i == editor.active {
        editor.mru.retain(|&x| x != id);
        editor.buffers.remove(i);
        let new_idx = i.min(editor.buffers.len() - 1);
        switch_to(editor, new_idx);
    } else {
        // The viewer stays put: remove id's slot, then re-point `active` by the
        // previously-active buffer's ID (its index shifts down when i < active).
        let active_id = editor.active().id;
        editor.mru.retain(|&x| x != id);
        editor.buffers.remove(i);
        if let Some(na) = editor.buffers.iter().position(|b| b.id == active_id) {
            editor.active = na;
        }
    }
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

    /// Task 4.2: Documents dynamic menu rows — open order (buffer-vec order, not MRU),
    /// scratch excluded, each row labeled by buffer_display_name and carrying SwitchBuffer.
    #[test]
    fn documents_menu_rows_open_order_excludes_scratch() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch(); // [A(0), scratch(1)]
        let b_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(crate::editor::Buffer::from_text(b_id, "b\n", None, area)); // [A(0), scratch(1), B(2)]
        let a_id = e.buffers[0].id;
        // Touch MRU out of buffer-vec order — rows must still follow buffer-vec (open) order.
        switch_to(&mut e, 2); // touch B most-recently
        let rows = documents_menu_rows(&e);
        assert_eq!(rows.len(), 2, "scratch excluded, two ordinary buffers");
        assert_eq!(rows[0], (buffer_display_name(&e, a_id), crate::menu::MenuRowAction::SwitchBuffer(a_id)),
            "row 0 is A (buffer-vec order), not B (MRU order)");
        assert_eq!(rows[1], (buffer_display_name(&e, b_id), crate::menu::MenuRowAction::SwitchBuffer(b_id)));
        assert!(rows.iter().all(|(name, _)| name != "*scratch*"), "scratch never appears");
    }

    #[test]
    fn switcher_rows_mru_order_with_display_names() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch();
        // Make buffer 0 a named file display, scratch second.
        e.buffers[0].document.path = Some(std::path::PathBuf::from("/tmp/notes.md"));
        goto_scratch(&mut e);     // scratch entered, but excluded from the switcher rows
        let rows = buffer_switch_rows(&e);
        assert!(rows.iter().all(|(_, n)| n != "*scratch*"), "scratch excluded from switcher rows");
        assert!(rows.iter().any(|(_, n)| n.contains("notes.md")));
    }

    #[test]
    fn cycle_skips_scratch() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10)); // buf A at index 0
        e.install_scratch(); // [A(0), scratch(1)]
        let b_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(crate::editor::Buffer::from_text(b_id, "b\n", None, area)); // [A(0), scratch(1), B(2)]
        goto_scratch(&mut e);
        assert!(e.is_scratch(e.active().id), "precondition: active is scratch");
        next_buffer(&mut e);
        assert_eq!(e.active().id, b_id, "next from scratch lands on the buffer following it");
        goto_scratch(&mut e);
        prev_buffer(&mut e);
        assert_ne!(e.active().id, b_id, "prev from scratch does not land on B");
        assert!(!e.is_scratch(e.active().id), "prev from scratch never lands on scratch");
        let a_id = e.buffers[0].id;
        assert_eq!(e.active().id, a_id, "prev from scratch lands on the buffer preceding it");

        // From an ordinary buffer, neither direction ever lands on scratch.
        switch_to(&mut e, 0); // A
        next_buffer(&mut e);
        assert!(!e.is_scratch(e.active().id), "next from an ordinary buffer never lands on scratch");
        switch_to(&mut e, 0);
        prev_buffer(&mut e);
        assert!(!e.is_scratch(e.active().id), "prev from an ordinary buffer never lands on scratch");
    }
    #[test]
    fn goto_scratch_jumps_to_scratch() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch();
        goto_scratch(&mut e);
        assert_eq!(e.buffers[e.active].id, e.scratch_id.unwrap());
    }

    #[test]
    fn toggle_scratch_round_trips_to_prior_buffer() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch();
        let b_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(crate::editor::Buffer::from_text(b_id, "b\n", None, area));
        let b_idx = e.buffers.iter().position(|b| b.id == b_id).unwrap();
        switch_to(&mut e, b_idx); // active = B
        toggle_scratch(&mut e);
        assert!(e.is_scratch(e.active().id), "first toggle enters scratch");
        toggle_scratch(&mut e);
        assert_eq!(e.active().id, b_id, "second toggle returns to B");
    }

    /// Discriminates forward vs. `.rev()` MRU iteration: after B (the recorded return
    /// buffer) closes, the two live candidates A and C are touched in an order such
    /// that A is MORE recent than C. Forward iteration (most-recent-first) must land
    /// on A; a `.rev()` bug would wrongly land on C.
    #[test]
    fn toggle_scratch_from_closed_prior_falls_back_to_mru() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch(); // [A(0), scratch(1)]
        let a_id = e.buffers[0].id;
        let b_id = e.alloc_id();
        let c_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(crate::editor::Buffer::from_text(b_id, "b\n", None, area)); // [A, scratch, B]
        e.buffers.push(crate::editor::Buffer::from_text(c_id, "c\n", None, area)); // [A, scratch, B, C]
        fn idx_of(e: &Editor, id: BufferId) -> usize { e.buffers.iter().position(|b| b.id == id).unwrap() }
        let (a_idx, b_idx, c_idx) = (idx_of(&e, a_id), idx_of(&e, b_id), idx_of(&e, c_id));
        switch_to(&mut e, a_idx); // touch A
        switch_to(&mut e, c_idx); // touch C
        switch_to(&mut e, b_idx); // touch B
        switch_to(&mut e, a_idx); // touch A again — A now more recent than C
        switch_to(&mut e, b_idx); // active = B (MRU front = B, A, C, scratch)
        toggle_scratch(&mut e); // records scratch_return = B, active = scratch
        // Simulate B closed: remove from buffers and MRU.
        e.buffers.retain(|b| b.id != b_id);
        e.mru.retain(|&id| id != b_id);
        toggle_scratch(&mut e);
        assert_eq!(e.active().id, a_id, "falls back to the MOST-recent live non-scratch buffer (A, not C)");
    }

    #[test]
    fn goto_scratch_records_return_for_toggle() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch();
        let b_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(crate::editor::Buffer::from_text(b_id, "b\n", None, area));
        let b_idx = e.buffers.iter().position(|b| b.id == b_id).unwrap();
        switch_to(&mut e, b_idx); // active = B
        goto_scratch(&mut e);
        assert!(e.is_scratch(e.active().id));
        toggle_scratch(&mut e);
        assert_eq!(e.active().id, b_id, "goto_scratch records the return buffer for toggle_scratch");
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
    fn close_dirty_raises_prompt() {
        use wordcartel_core::history::Clock;
        struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let mut e = Editor::new_from_text("x\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch();
        let aid = e.active().id;
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "z".into())], 2);
        let txn = wordcartel_core::history::Transaction::new(cs).with_selection(wordcartel_core::selection::Selection::single(1));
        e.by_id_mut(aid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
        close_buffer(&mut e);
        assert!(e.by_id(aid).is_some(), "dirty buffer not closed by the prompt raise");
        let p = e.prompt.as_ref().expect("close-confirm prompt raised");
        assert_eq!(p.action_for('s'), Some(crate::prompt::PromptAction::CloseSave { id: aid }));
        assert_eq!(p.action_for('d'), Some(crate::prompt::PromptAction::CloseDiscard { id: aid }));
        assert_eq!(p.action_for('c'), Some(crate::prompt::PromptAction::Cancel));
    }

    #[test]
    fn close_dirty_refuses_while_flow_pending() {
        use wordcartel_core::history::Clock;
        struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let mut e = Editor::new_from_text("x\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch();
        let aid = e.active().id;
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "z".into())], 2);
        let txn = wordcartel_core::history::Transaction::new(cs).with_selection(wordcartel_core::selection::Selection::single(1));
        e.by_id_mut(aid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
        e.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id: aid, version: 1, action: crate::editor::PostSaveAction::Quit, at_ms: 0,
        });
        close_buffer(&mut e);
        assert!(e.prompt.is_none(), "busy guard: no prompt over pending state");
        assert!(e.status.contains("in progress"), "refusal status set: {:?}", e.status);
    }

    #[test]
    fn close_buffer_now_by_id_closes_inactive_buffer() {
        // Three buffers incl. scratch; close a NON-active id → viewed buffer still active by ID, count drops.
        let mut e = Editor::new_from_text("doc0\n", None, (40, 10));
        e.install_scratch(); // [doc0(0), scratch(1)]
        let doc0_id = e.buffers[0].id;
        let doc1_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(crate::editor::Buffer::from_text(doc1_id, "doc1\n", None, area)); // [doc0(0), scratch(1), doc1(2)]
        e.mru.push(doc1_id);
        assert_eq!(e.active().id, doc0_id, "precondition: doc0 active");
        let before_id = e.active().id;
        // Close doc1 (non-active, index 2)
        close_buffer_now(&mut e, doc1_id);
        assert_eq!(e.active().id, before_id, "viewed buffer still doc0 by id");
        assert!(e.by_id(doc1_id).is_none(), "doc1 removed");
        assert_eq!(e.buffers.len(), 2, "count drops");
    }

    #[test]
    fn close_buffer_now_nonactive_normal_keeps_view() {
        // Removing a slot BELOW the active index: editor.active re-pointed by id, viewed buffer unchanged.
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch(); // [a(0), scratch(1)]
        let a_id = e.buffers[0].id;
        let b_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(crate::editor::Buffer::from_text(b_id, "b\n", None, area)); // [a(0), scratch(1), b(2)]
        e.mru.push(b_id);
        e.active = 2; // view b
        assert_eq!(e.active().id, b_id, "precondition: B is active");
        // Close A (index 0, which is below the active index 2)
        close_buffer_now(&mut e, a_id);
        // After: [scratch(0), b(1)], active re-pointed to b's new index
        assert_eq!(e.active().id, b_id, "viewed buffer unchanged by id");
        assert!(e.by_id(a_id).is_none(), "A removed");
        assert_eq!(e.buffers.len(), 2);
    }

    #[test]
    fn close_buffer_now_vanished_id_is_noop_with_status() {
        use crate::editor::BufferId;
        let mut e = Editor::new_from_text("doc\n", None, (40, 10));
        e.install_scratch();
        let phantom = BufferId(9999);
        close_buffer_now(&mut e, phantom);
        assert_eq!(e.status, "buffer already closed");
        assert_eq!(e.buffers.len(), 2, "buffer count unchanged");
    }

    /// Effort A: `notify_close(id)` fires for the closed buffer in ALL THREE close shapes —
    /// last-ordinary replacement, active removal, and inactive removal — so the server never keeps
    /// a closed doc open until shutdown. A vanished-id no-op sends nothing.
    #[test]
    fn close_buffer_now_notifies_provider_in_all_three_shapes() {
        use crate::diag_provider::{RecordingProvider, ProviderCall};
        // Helper: build a 3-ordinary editor, install a fresh recorder, return (editor, call handle).
        let install = |e: &mut Editor| {
            let rec = RecordingProvider::new();
            let calls = rec.calls_handle();
            e.diag_provider = Box::new(rec);
            calls
        };
        let closed = |calls: &std::sync::Arc<std::sync::Mutex<Vec<ProviderCall>>>, id: crate::editor::BufferId| {
            calls.lock().unwrap().iter().any(|c| matches!(c, ProviderCall::NotifyClose(x) if *x == id))
        };

        // (1) last-ordinary replacement: one ordinary buffer (+ scratch) → replace branch.
        let mut e = Editor::new_from_text("only\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch();
        let only_id = e.buffers[0].id;
        let calls = install(&mut e);
        close_buffer_now(&mut e, only_id);
        assert!(closed(&calls, only_id), "replace-last-ordinary notifies close for the old id");

        // (2) active removal: two ordinary buffers, close the active one → active-remove branch.
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        let a_id = e.buffers[0].id;
        let b_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(crate::editor::Buffer::from_text(b_id, "b\n", None, area));
        e.mru.push(b_id);
        let calls = install(&mut e);
        assert_eq!(e.active().id, a_id, "precondition: a active");
        close_buffer_now(&mut e, a_id);
        assert!(closed(&calls, a_id), "active removal notifies close");

        // (3) inactive removal: three ordinary buffers, close a non-active one → inactive branch.
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        let c_id = e.alloc_id();
        let d_id = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(crate::editor::Buffer::from_text(c_id, "c\n", None, area));
        e.buffers.push(crate::editor::Buffer::from_text(d_id, "d\n", None, area));
        e.mru.push(c_id); e.mru.push(d_id);
        let calls = install(&mut e);
        close_buffer_now(&mut e, d_id); // d is inactive (active is a, index 0)
        assert!(closed(&calls, d_id), "inactive removal notifies close");
    }
}
