//! M2: the untrusted edit-submission boundary (Effort P's `apply(Transaction)` seam).
//! Validates an untrusted Transaction against the live buffer; on Err: zero mutation.
//! On Ok: snaps the cursor, derives a conservative whole-doc Edit, applies via the
//! trusted `editor.apply`. Never panics, never partially edits.

use crate::editor::Editor;
pub use wordcartel_core::change::EditError;
use wordcartel_core::history::{Clock, EditKind, Transaction};
use wordcartel_core::selection::Selection;

/// Untrusted edit-submission boundary. See module docs.
pub fn submit_transaction(
    editor: &mut Editor,
    txn: Transaction,
    clock: &dyn Clock,
) -> Result<(), EditError> {
    // Bind locals early to avoid partial-move issues with the borrow checker.
    let changes = txn.changes;
    let selection = txn.selection;

    // 1. Validate against the LIVE buffer — no mutation. Early-return on Err.
    changes.validate_against(&editor.active().document.buffer)?;

    let len_before = changes.len_before();
    let len_after = changes.len_after();

    // 2. Snap the (single-range) selection against a CLONE, pre-apply, so history records
    //    the snapped cursor (redo-safe). Cursor positions snap — never a reject.
    let snapped_sel: Option<Selection> = selection.as_ref().map(|sel| {
        let mut clone = editor.active().document.buffer.clone();
        changes.apply(&mut clone); // validated → cannot panic; gives post-edit text
        let r = sel.primary();
        Selection::range(clone.clamp_to_boundary(r.anchor), clone.clamp_to_boundary(r.head))
    });

    // 3. Conservative whole-doc reparse Edit.
    let edit = wordcartel_core::block_tree::Edit { range: 0..len_before, new_len: len_after };

    // 4. Build the final transaction (original changes + snapped selection) and apply
    //    once via the trusted path — the only live mutation.
    let mut final_txn = Transaction::new(changes);
    if let Some(sel) = snapped_sel { final_txn = final_txn.with_selection(sel); }
    editor.apply(final_txn, edit, EditKind::Other, clock);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_core::change::{ChangeSet, Op, Tendril};
    struct C(u64);
    impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }

    fn ed(s: &str) -> Editor { Editor::new_from_text(s, None, (40, 10)) }

    #[test]
    fn valid_transaction_applies() {
        let mut e = ed("hello\n"); // len 6
        let cs = ChangeSet::insert(0, "X", 6);
        let r = submit_transaction(&mut e, Transaction::new(cs), &C(0));
        assert!(r.is_ok());
        assert_eq!(e.active().document.buffer.to_string(), "Xhello\n");
    }

    #[test]
    fn stale_length_rejected_no_mutation() {
        let mut e = ed("hello\n"); // len 6
        let before = e.active().document.buffer.to_string();
        let cs = ChangeSet::insert(0, "X", 3); // built for len 3 ≠ 6
        let r = submit_transaction(&mut e, Transaction::new(cs), &C(0));
        assert!(matches!(r, Err(EditError::StaleLength { .. })));
        assert_eq!(e.active().document.buffer.to_string(), before, "buffer unchanged");
    }

    #[test]
    fn op_boundary_rejected_no_mutation_no_panic() {
        let mut e = ed("é\n"); // 'é'=2 bytes + '\n' → len 3
        let before = e.active().document.buffer.to_string();
        // Delete(1) ends at byte 1 (mid-é); Retain(2) covers "\n"+... sum = 1+2 = 3.
        let cs = ChangeSet::from_ops(vec![Op::Delete(1), Op::Retain(2)], 3);
        let r = submit_transaction(&mut e, Transaction::new(cs), &C(0));
        assert!(matches!(r, Err(EditError::OpBoundary { .. })));
        assert_eq!(e.active().document.buffer.to_string(), before, "buffer unchanged");
    }

    #[test]
    fn out_of_bounds_selection_snaps_not_rejects() {
        let mut e = ed("hi\n"); // len 3
        let cs = ChangeSet::insert(0, "X", 3); // → len_after 4 ("Xhi\n")
        let txn = Transaction::new(cs).with_selection(Selection::range(999, 999));
        let r = submit_transaction(&mut e, txn, &C(0));
        assert!(r.is_ok());
        assert_eq!(e.active().document.buffer.to_string(), "Xhi\n");
        let head = e.active().document.selection.primary().head;
        assert!(head <= 4, "cursor snapped into [0, len_after]; got {head}");
    }
}
