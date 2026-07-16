//! Effort 6: send-to-scratch verbs. Append the active buffer's marked block to the
//! permanent *scratch* buffer; move also deletes the block from the source buffer.
//! Cross-buffer undo is two independent steps (scratch append; source delete).

use crate::editor::Editor;
use wordcartel_core::history::Clock;

/// Append `text` to the scratch buffer (blank line before it when scratch is
/// non-empty). One undo step in the SCRATCH buffer's history. Returns false if no
/// scratch is installed.
fn append_to_scratch(editor: &mut Editor, text: &str, clock: &dyn Clock) -> bool {
    let Some(sid) = editor.scratch_id else { return false; };
    let Some(sb) = editor.by_id(sid) else { return false; };
    let cur_len = sb.document.buffer.len();
    let sep = if cur_len == 0 { "" } else { "\n\n" };
    let insert = format!("{sep}{text}");
    let new_caret = cur_len + insert.len();
    let (cs, edit) = crate::commands::build_multi_replace(&[(cur_len, cur_len, insert)], cur_len);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(new_caret));
    matches!(
        crate::edit_apply::apply_edit(editor, sid, txn, edit,
            wordcartel_core::history::EditKind::Other, clock),
        crate::edit_apply::EditOutcome::Applied
    )
}

/// Copy the active buffer's marked block into scratch; source unchanged, block kept.
pub fn copy_block_to_scratch(editor: &mut Editor, clock: &dyn Clock) {
    if editor.scratch_id == Some(editor.active().id) {
        editor.set_status(crate::status::StatusKind::Info, "already in the scratch buffer");
        return;
    }
    let Some(b) = editor.active().marked_block else { editor.set_status(crate::status::StatusKind::Info, "no marked block"); return; };
    let text = editor.active().document.buffer.slice(b.start..b.end);
    if append_to_scratch(editor, &text, clock) {
        editor.set_status(crate::status::StatusKind::Info, "block copied to scratch");
    } else {
        editor.set_status(crate::status::StatusKind::Info, "no scratch buffer");
    }
}

/// Move the active buffer's marked block into scratch; delete it from the source
/// (a separate undo step in the source's history). Block is consumed.
pub fn move_block_to_scratch(editor: &mut Editor, clock: &dyn Clock) {
    if editor.scratch_id == Some(editor.active().id) {
        editor.set_status(crate::status::StatusKind::Info, "already in the scratch buffer");
        return;
    }
    let Some(b) = editor.active().marked_block else { editor.set_status(crate::status::StatusKind::Info, "no marked block"); return; };
    let text = editor.active().document.buffer.slice(b.start..b.end);
    if !append_to_scratch(editor, &text, clock) {
        editor.set_status(crate::status::StatusKind::Info, "no scratch buffer");
        return;
    }
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = crate::commands::build_multi_replace(&[(b.start, b.end, String::new())], doc_len);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(b.start));
    editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock); // active (source) buffer
    editor.active_mut().marked_block = None;
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
    editor.set_status(crate::status::StatusKind::Info, "block moved to scratch");
}

#[cfg(test)]
mod tests {
    use super::*;
    struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }

    fn setup() -> Editor {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.install_scratch();
        e
    }

    #[test]
    fn copy_to_scratch_appends_and_keeps_source() {
        let mut e = setup();
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false }); // "hello"
        copy_block_to_scratch(&mut e, &C(0));
        let sid = e.scratch_id.unwrap();
        assert_eq!(e.by_id(sid).unwrap().document.buffer.to_string(), "hello");
        assert_eq!(e.active().document.buffer.to_string(), "hello world\n", "source untouched");
        assert!(e.active().marked_block.is_some(), "block kept after copy");
    }

    #[test]
    fn second_copy_separates_entries_with_blank_line() {
        let mut e = setup();
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false }); // "hello"
        copy_block_to_scratch(&mut e, &C(0));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 6, end: 11, hidden: false }); // "world"
        copy_block_to_scratch(&mut e, &C(0));
        let sid = e.scratch_id.unwrap();
        assert_eq!(e.by_id(sid).unwrap().document.buffer.to_string(), "hello\n\nworld");
    }

    #[test]
    fn move_to_scratch_appends_and_deletes_source_two_undo_steps() {
        let mut e = setup();
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 6, hidden: false }); // "hello "
        move_block_to_scratch(&mut e, &C(0));
        let sid = e.scratch_id.unwrap();
        assert_eq!(e.by_id(sid).unwrap().document.buffer.to_string(), "hello ");
        assert_eq!(e.active().document.buffer.to_string(), "world\n", "block deleted from source");
        assert!(e.active().marked_block.is_none(), "block consumed by move");
        // Undo in source restores the deletion (one step).
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "hello world\n");
        // Scratch append is a SEPARATE undo in the scratch buffer's own history.
        if let Some(i) = e.buffers.iter().position(|b| b.id == sid) { e.active = i; }
        assert!(e.undo(), "scratch has its own undo step");
        assert_eq!(e.by_id(sid).unwrap().document.buffer.to_string(), "");
    }

    #[test]
    fn no_block_sets_status() {
        let mut e = setup();
        copy_block_to_scratch(&mut e, &C(0));
        assert_eq!(e.status_text(), "no marked block");
    }

    #[test]
    fn move_no_block_sets_status() {
        let mut e = setup();
        move_block_to_scratch(&mut e, &C(0));
        assert_eq!(e.status_text(), "no marked block");
        assert_eq!(e.active().document.buffer.to_string(), "hello world\n", "buffer unchanged");
    }

    #[test]
    fn verbs_noop_when_scratch_is_active() {
        let mut e = setup();
        // Switch to the scratch buffer.
        crate::workspace::goto_scratch(&mut e);
        assert_eq!(e.buffers[e.active].id, e.scratch_id.unwrap(), "active must be scratch");

        // Give the scratch buffer a marked block so we can detect if anything is appended.
        let scratch_before = e.active().document.buffer.to_string();
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 0, hidden: false });

        copy_block_to_scratch(&mut e, &C(0));
        assert_eq!(e.status_text(), "already in the scratch buffer");
        assert_eq!(e.active().document.buffer.to_string(), scratch_before, "copy: scratch content unchanged");

        move_block_to_scratch(&mut e, &C(0));
        assert_eq!(e.status_text(), "already in the scratch buffer");
        assert_eq!(e.active().document.buffer.to_string(), scratch_before, "move: scratch content unchanged");
    }
}
