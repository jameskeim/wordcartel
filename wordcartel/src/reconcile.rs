//! Block-tree reconcile runtime (shell): per-buffer store + debounce helper +
//! the background reparse job dispatch. Mirrors `diagnostics_run.rs`. Gives the
//! convergence theorem: quiescence ⇒ `document.blocks == full_parse(text)`.

use crate::editor::Editor;
use crate::jobs::{Executor, Job, JobKind, JobResult, ResultClass};

/// Debounce before a settled buffer's tree is reconciled to `full_parse`.
/// ~150 ms — long enough not to fire mid-burst, short enough to feel instant.
pub const RECONCILE_DEBOUNCE_MS: u64 = 150;

/// Per-buffer reconcile state. `blocks_version` is the memoization key for
/// `derive::rebuild` (the document version `document.blocks` was built for).
#[derive(Debug, Default, Clone)]
pub struct ReconcileStore {
    /// The document version `document.blocks` currently reflects.
    pub blocks_version: u64,
    /// The current tree may differ from `full_parse` (set on an incremental
    /// `Local`/`WidenToEnd` update; cleared whenever a full parse establishes it).
    pub maybe_stale: bool,
    /// Debounce deadline: dispatch a reconcile once `now >= due_at`.
    pub due_at: Option<u64>,
    /// A reconcile job is running for this version (blocks re-dispatch).
    pub in_flight_version: Option<u64>,
    /// The document version the debounce was last armed for (so idle Ticks do
    /// not re-arm and push the deadline forever).
    pub armed_for_version: u64,
}

/// A reconcile is due if the tree may be stale, nothing is in flight, and the
/// debounce deadline has been reached.
pub fn reconcile_due(store: &ReconcileStore, now: u64) -> bool {
    store.maybe_stale
        && store.in_flight_version.is_none()
        && matches!(store.due_at, Some(t) if now >= t)
}

/// Snapshot the active buffer + dispatch a background full-parse reconcile.
/// Sets `in_flight_version` and clears the debounce deadline (consumed).
pub fn dispatch_reconcile(editor: &mut Editor, ex: &dyn Executor) {
    let b = editor.active();
    let buffer_id = b.id;
    let version = b.document.version;
    let rope = b.document.buffer.snapshot(); // O(1) ropey clone, moved to the worker
    editor.active_mut().reconcile.in_flight_version = Some(version);
    editor.active_mut().reconcile.due_at = None;

    let job = Job {
        buffer_id,
        class: ResultClass::BufferLocal,
        version,
        kind: JobKind::Reparse,
        run: Box::new(move || {
            let tree = wordcartel_core::block_tree::full_parse_rope(&rope);
            JobResult {
                buffer_id,
                class: ResultClass::BufferLocal,
                version,
                kind: JobKind::Reparse,
                merge: Box::new(move |editor: &mut Editor| {
                    if let Some(b) = editor.by_id_mut(buffer_id) {
                        // Version-check INSIDE the merge (the version-discard): only
                        // adopt the tree if the buffer is still at the job's version.
                        if b.document.version == version {
                            if b.document.blocks() != &tree {
                                b.document.set_blocks(tree);
                            }
                            b.reconcile.blocks_version = version;
                            b.reconcile.maybe_stale = false;
                            // The pre-draw derive::rebuild will refresh downstream
                            // (version == blocks_version → skip parse → downstream).
                        }
                        // Clear in-flight regardless (the reconcile completed), so a
                        // later reconcile can dispatch.
                        b.reconcile.in_flight_version = None;
                    }
                }),
            }
        }),
    };
    ex.dispatch(job);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_due_requires_stale_armed_and_not_in_flight() {
        let mut s = ReconcileStore { maybe_stale: true, due_at: Some(100), ..Default::default() };
        assert!(!reconcile_due(&s, 99), "not yet due");
        assert!(reconcile_due(&s, 100), "due at deadline");
        s.in_flight_version = Some(1);
        assert!(!reconcile_due(&s, 200), "in-flight blocks dispatch");
        s.in_flight_version = None;
        s.maybe_stale = false;
        assert!(!reconcile_due(&s, 200), "not stale → nothing to do");
        s.maybe_stale = true;
        s.due_at = None;
        assert!(!reconcile_due(&s, 200), "not armed");
    }

    #[test]
    fn reconcile_converges_a_diverged_tree_to_full_parse() {
        use wordcartel_core::block_tree;
        let mut e = crate::editor::Editor::new_from_text("para\n", None, (80, 24));
        let bid = e.active().id;
        let v = e.active().document.version;
        // Plant a deliberately-wrong tree at the current version (simulating a
        // diverged incremental result), flagged stale.
        let t = block_tree::empty_tree(e.active().document.buffer.len());
        e.active_mut().document.set_blocks(t);
        e.active_mut().reconcile.blocks_version = v;
        e.active_mut().reconcile.maybe_stale = true;
        let correct = block_tree::full_parse(&e.active().document.buffer.to_string());
        assert_ne!(*e.active().document.blocks(), correct, "precondition: tree is diverged");

        let ex = crate::jobs::InlineExecutor::default();
        dispatch_reconcile(&mut e, &ex);
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        assert_eq!(*e.active().document.blocks(), correct, "reconcile converges to full_parse");
        assert!(!e.active().reconcile.maybe_stale, "stale cleared");
        assert_eq!(e.active().reconcile.blocks_version, v);
        let _ = bid;
    }

    /// A panicked reparse (upstream pulldown-cmark residual) is deterministic for
    /// the offending text — without this fix, `maybe_stale` stays `true` after
    /// `apply_panic`, the main-loop arm re-arms `due_at`, and the worker is
    /// dispatched → panics → re-armed every ~150 ms in an infinite loop.
    /// Fix: `apply_panic`'s `Reparse` arm must also clear `maybe_stale`.
    #[test]
    fn panicked_reparse_clears_maybe_stale_so_no_retry_loop() {
        let mut e = crate::editor::Editor::new_from_text("para\n", None, (80, 24));
        let bid = e.active().id;
        let v = e.active().document.version;

        // Simulate the state just before a panicked reconcile returns:
        // in-flight (the job was running) and stale (the trigger that caused dispatch).
        e.active_mut().reconcile.maybe_stale = true;
        e.active_mut().reconcile.in_flight_version = Some(v);

        // Synthesise a Panicked outcome — no real panic required.
        let outcome = crate::jobs::JobOutcome::Panicked {
            buffer_id: bid,
            version: v,
            kind: crate::jobs::JobKind::Reparse,
            msg: "upstream pulldown residual (simulated)".into(),
        };
        crate::jobs_apply::apply_outcome(outcome, &mut e);

        assert!(
            !e.active().reconcile.maybe_stale,
            "panicked reparse must clear maybe_stale — otherwise the 150 ms retry loop fires forever"
        );
        assert!(
            e.active().reconcile.in_flight_version.is_none(),
            "in_flight_version must be cleared on a panicked reparse"
        );
    }

    #[test]
    fn reconcile_discards_when_version_advanced() {
        use wordcartel_core::block_tree;
        let mut e = crate::editor::Editor::new_from_text("para\n", None, (80, 24));
        e.active_mut().reconcile.maybe_stale = true;
        e.active_mut().reconcile.blocks_version = e.active().document.version;
        let planted = block_tree::empty_tree(e.active().document.buffer.len());
        e.active_mut().document.set_blocks(planted.clone());

        let ex = crate::jobs::InlineExecutor::default();
        dispatch_reconcile(&mut e, &ex); // snapshots the current version
        // an edit lands before the (synchronous, here) merge is applied:
        e.active_mut().document.version += 1;
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        assert_eq!(*e.active().document.blocks(), planted, "stale reconcile did not clobber the newer state");
        assert!(e.active().reconcile.in_flight_version.is_none(), "in-flight cleared even on discard");
    }
}
