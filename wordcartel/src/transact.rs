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

    // 4. Build the final transaction (original changes + snapped selection) and apply once via
    //    the core — the only live mutation. A read-only active buffer is refused loudly here
    //    (INV-GUARD); the untrusted boundary still returns Ok — the edit was cleanly declined.
    let mut final_txn = Transaction::new(changes);
    if let Some(sel) = snapped_sel { final_txn = final_txn.with_selection(sel); }
    let id = editor.active().id;
    let _ = crate::edit_apply::apply_edit(editor, id, final_txn, edit, EditKind::Other, clock);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use wordcartel_core::change::{ChangeSet, Op};
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

    // Regression guard (green before AND after Task 2): the validate→core refactor preserves the
    // loud read-only refusal and the Ok(()) return (a read-only view's edit is cleanly declined).
    #[test]
    fn submit_into_read_only_is_ok_after_loud_reject() {
        let mut e = ed("hello\n");
        e.active_mut().read_only = true;
        let before = e.active().document.buffer.to_string();
        let cs = ChangeSet::insert(0, "X", 6);
        let r = submit_transaction(&mut e, Transaction::new(cs), &C(0));
        assert!(r.is_ok(), "read-only edit is refused loudly, not errored");
        assert_eq!(e.active().document.buffer.to_string(), before, "no mutation");
        assert_eq!(e.status_text(), "buffer is read-only");
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
        assert_eq!(head, 4, "cursor snapped to end of [0, len_after]");
    }

    // ── M2 full-pipeline proptest ─────────────────────────────────────────────

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        /// M2 gate (spec §11.2): drives the WHOLE `submit_transaction` pipeline
        /// with a random unicode document × a randomly-built `Transaction`, mixing
        /// valid, StaleLength, and OpBoundary cases.  Three invariants are checked:
        ///
        /// 1. **Never panics** — a panic fails the proptest.
        /// 2. **On `Err`**: the editor's buffer is byte-identical to before (no
        ///    partial / any mutation).
        /// 3. **On `Ok`**: buffer length == `len_after`, and the active selection's
        ///    head + anchor are within `[0, len_after]` and on char boundaries
        ///    (`clamp_to_boundary(pos) == pos`).
        ///
        /// The changeset generator mirrors the core proptest in `change.rs`: build
        /// sum-valid changesets via `ChangeSet::insert`/`delete` for a random
        /// `claimed_len` that may differ from the actual buffer length (→
        /// `StaleLength`), or may land mid-char in the target buffer (→
        /// `OpBoundary`).
        #[test]
        fn prop_submit_transaction_never_panics_or_partially_mutates(
            // Random unicode document using the same alphabet as the core proptest.
            doc in proptest::collection::vec(
                proptest::sample::select(vec!['a', 'b', 'é', '中', '🙂', '\n']),
                0..=20usize,
            ).prop_map(|cs| cs.into_iter().collect::<String>()),
            // claimed_len independent of doc length → exercises StaleLength + valid.
            claimed_len in 0usize..28,
            is_delete in proptest::bool::ANY,
            p1 in 0usize..28,
            p2 in 0usize..28,
            // Insert text may contain multi-byte chars, landing positions mid-char.
            text in proptest::string::string_regex("[aé中]{0,4}").unwrap(),
            // Out-of-bounds selection values; submit_transaction must snap, not panic.
            sel_head in 0usize..40,
            sel_anchor in 0usize..40,
            include_selection in proptest::bool::ANY,
        ) {
            let mut e = ed(&doc);
            let before = e.active().document.buffer.to_string();

            // Build a sum-valid changeset for `claimed_len`.  Positions are clamped
            // to [0, claimed_len] but may land mid-char in the target buffer.
            let cs = if is_delete {
                let a = p1 % (claimed_len + 1);
                let b = p2 % (claimed_len + 1);
                ChangeSet::delete(a.min(b)..a.max(b), claimed_len)
            } else {
                let at = p1 % (claimed_len + 1);
                ChangeSet::insert(at, &text, claimed_len)
            };
            let len_after = cs.len_after();

            let txn = if include_selection {
                Transaction::new(cs).with_selection(Selection::range(sel_head, sel_anchor))
            } else {
                Transaction::new(cs)
            };

            // Drives the whole pipeline — panics here fail the proptest.
            let result = submit_transaction(&mut e, txn, &C(0));

            match result {
                Ok(()) => {
                    // Buffer length must equal the transaction's len_after.
                    let buf_len = e.active().document.buffer.len();
                    prop_assert_eq!(buf_len, len_after,
                        "on Ok: buffer length must equal len_after");
                    // Active selection must be in-bounds and on char boundaries.
                    let sel = e.active().document.selection.primary();
                    prop_assert!(sel.head <= buf_len,
                        "head {} out of [0, {}]", sel.head, buf_len);
                    prop_assert!(sel.anchor <= buf_len,
                        "anchor {} out of [0, {}]", sel.anchor, buf_len);
                    let snapped_head   = e.active().document.buffer.clamp_to_boundary(sel.head);
                    let snapped_anchor = e.active().document.buffer.clamp_to_boundary(sel.anchor);
                    prop_assert_eq!(snapped_head, sel.head,
                        "head {} is not on a char boundary", sel.head);
                    prop_assert_eq!(snapped_anchor, sel.anchor,
                        "anchor {} is not on a char boundary", sel.anchor);
                }
                Err(_) => {
                    // On Err: the buffer must be byte-identical to before — zero mutation.
                    let after = e.active().document.buffer.to_string();
                    prop_assert_eq!(after, before,
                        "on Err: buffer must not be modified");
                }
            }
        }
    }
}
