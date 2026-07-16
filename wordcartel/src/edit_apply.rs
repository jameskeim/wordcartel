//! The one funnel every internal buffer edit passes through (H22, decision B). Owns the loud
//! read-only guard, a debug validation backstop, the single `Buffer::apply` mutation, and the
//! active-buffer epilogue (`resettle`). `submit_transaction` validates then calls here; internal
//! callers call here pre-trusted. `Buffer::apply` is `pub(crate)` and MUST be called ONLY from
//! this module — the compiler-guarded no-bypass seam (INV-SEAM, enforced by `tests/edit_seam.rs`).

use crate::editor::{BufferId, Editor};
use wordcartel_core::block_tree::Edit;
use wordcartel_core::history::{Clock, EditKind, Transaction};

/// Outcome of a funnelled edit — callers gate their status acks on this (INV-GUARD, F4).
/// `#[must_use]` (H24): makes INV-GUARD ack-gating self-enforcing at every FUTURE call site —
/// a caller that forgets to gate its ack on `Applied` gets a compiler warning, not a silent bug.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[must_use]
pub enum EditOutcome {
    /// The edit committed.
    Applied,
    /// The target buffer is read-only; the canonical Sticky Warning was set here, nothing mutated.
    RejectedReadOnly,
    /// The target buffer id was not found (raced close/dispose); nothing mutated, no status.
    BufferGone,
}

/// The shared post-edit epilogue (F2=A — relocated from `commands::edit::settle_after_edit`):
/// re-derive the block tree, re-scroll to the caret, reset vertical-motion memory. Operates on
/// the ACTIVE buffer. The core runs it after an active edit; the two fold-correction commands
/// (`block_move`/`swap`, §3.6) call it as their post-`replace_folded` rebuild #2.
pub(crate) fn resettle(editor: &mut Editor) {
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
    editor.active_mut().desired_col = None;
}

/// THE internal-edit funnel. Applies `txn`/`edit` to `buffer_id` (not necessarily active) through
/// the single mutation channel, then runs the epilogue iff the edited buffer is active. Pre-trusted:
/// the changeset is assumed valid-by-construction (built from a live `doc_len`); a `debug_assert`
/// backstops that (F5). Never panics in release, never partially edits.
///
/// # Examples
/// ```
/// # use wordcartel::editor::Editor;
/// # use wordcartel::edit_apply::{apply_edit, EditOutcome};
/// # use wordcartel_core::history::{Clock, EditKind, Transaction};
/// # struct C; impl Clock for C { fn now_ms(&self) -> u64 { 0 } }
/// let mut e = Editor::new_from_text("hi\n", None, (40, 6));
/// let id = e.active().id;
/// let (cs, edit) = wordcartel::commands::build_multi_replace(&[(0, 0, "X".into())], 3);
/// assert_eq!(apply_edit(&mut e, id, Transaction::new(cs), edit, EditKind::Other, &C),
///            EditOutcome::Applied);
/// assert_eq!(e.active().document.buffer.to_string(), "Xhi\n");
/// ```
pub fn apply_edit(
    editor: &mut Editor,
    buffer_id: BufferId,
    txn: Transaction,
    edit: Edit,
    kind: EditKind,
    clock: &dyn Clock,
) -> EditOutcome {
    // INV-GUARD (F4): uniform loud read-only. Absent buffer → BufferGone (no status).
    match editor.by_id(buffer_id) {
        None => return EditOutcome::BufferGone,
        Some(b) if b.read_only => { editor.reject_read_only(); return EditOutcome::RejectedReadOnly; }
        Some(_) => {}
    }
    // F5 (H7 blast-radius stance): debug-only backstop that the pre-trusted changeset applies
    // cleanly against the live target text. Release = trust-by-construction, zero cost.
    #[cfg(debug_assertions)]
    {
        let b = editor.by_id(buffer_id).expect("buffer presence checked above");
        debug_assert!(
            txn.changes.validate_against(&b.document.buffer).is_ok(),
            "internal edit built an invalid changeset for {buffer_id:?}",
        );
    }
    // Mutate through the single channel (scoped borrow — the transform.rs:290–300 borrow-split
    // made canonical). The borrow ends before the epilogue's `&mut Editor` calls below.
    {
        let b = editor.by_id_mut(buffer_id).expect("buffer presence checked above");
        b.apply(txn, edit, kind, clock);
    }
    // INV-EPILOGUE (F2) / INV-LAZY-HEAL (F3): epilogue on the ACTIVE buffer only. A non-active
    // edit leaves a lagging tree that every activation path heals before its first render.
    if buffer_id == editor.active().id {
        resettle(editor);
    }
    EditOutcome::Applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use wordcartel_core::history::{Clock, EditKind, Transaction};

    struct C(u64);
    impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }

    fn ins(doc_len: usize) -> (wordcartel_core::change::ChangeSet, Edit) {
        crate::commands::build_multi_replace(&[(0, 0, "X".into())], doc_len)
    }

    #[test]
    fn active_edit_applies_and_runs_epilogue() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        let id = e.active().id;
        let (cs, edit) = ins(e.active().document.buffer.len());
        let out = apply_edit(&mut e, id, Transaction::new(cs), edit, EditKind::Other, &C(0));
        assert_eq!(out, EditOutcome::Applied);
        assert_eq!(e.active().document.buffer.to_string(), "Xabc\n");
        // Epilogue ran on the active buffer: tree reparsed (blocks_version caught up).
        assert_eq!(e.active().reconcile.blocks_version, e.active().document.version);
    }

    #[test]
    fn read_only_target_is_a_loud_reject_no_mutation() {
        let mut e = Editor::new_from_text("keep\n", None, (80, 24));
        let id = e.active().id;
        e.active_mut().read_only = true;
        let before = e.active().document.buffer.to_string();
        let (cs, edit) = ins(e.active().document.buffer.len());
        let out = apply_edit(&mut e, id, Transaction::new(cs), edit, EditKind::Other, &C(0));
        assert_eq!(out, EditOutcome::RejectedReadOnly);
        assert_eq!(e.active().document.buffer.to_string(), before);
        assert_eq!(e.status_text(), "buffer is read-only");
    }

    #[test]
    fn missing_buffer_is_buffer_gone_no_status() {
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let (cs, edit) = ins(1);
        let out = apply_edit(&mut e, crate::editor::BufferId(9999),
            Transaction::new(cs), edit, EditKind::Other, &C(0));
        assert_eq!(out, EditOutcome::BufferGone);
        assert_eq!(e.status_text(), "", "BufferGone sets no status");
    }

    // INV-LAZY-HEAL (F3): a non-active edit skips the epilogue and leaves its tree lagging;
    // every activation path (here, workspace::switch_to) heals it before the buffer's first
    // render. The only Task-7 test that needs crate::editor::Buffer directly (m-P2).
    #[test]
    fn non_active_edit_lags_then_heals_on_switch() {
        let mut e = Editor::new_from_text("alpha\n", None, (80, 24));
        let id1 = e.alloc_id();
        let area = e.active().view.area;
        // buffer 0 stays active; edit lands on the non-active buffer id1.
        e.buffers.push(crate::editor::Buffer::from_text(id1, "beta\n", None, area));
        let doc_len = e.by_id(id1).unwrap().document.buffer.len();
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "X".into())], doc_len);
        let out = apply_edit(&mut e, id1, Transaction::new(cs), edit, EditKind::Other, &C(0));
        assert_eq!(out, EditOutcome::Applied);
        let b = e.by_id(id1).unwrap();
        assert!(b.reconcile.blocks_version < b.document.version,
            "non-active tree lags — the core skips the epilogue (INV-LAZY-HEAL)");
        // Every activation path heals before first render:
        let idx = e.buffers.iter().position(|x| x.id == id1).unwrap();
        crate::workspace::switch_to(&mut e, idx);
        let b = e.by_id(id1).unwrap();
        assert_eq!(b.reconcile.blocks_version, b.document.version, "switch_to heals the tree");
    }
}
