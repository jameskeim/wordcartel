//! Block-tree reconcile runtime (shell): per-buffer store + debounce helper +
//! the background reparse job dispatch. Mirrors `diagnostics_run.rs`. Gives the
//! convergence theorem: quiescence ⇒ `document.blocks == full_parse(text)`.
//!
//! NOTE (Task 1): no `use` imports here yet — `ReconcileStore` + `reconcile_due`
//! reference no external types. Task 3 adds `use crate::editor::Editor;` and
//! `use crate::jobs::{Executor, Job, JobKind, JobResult, ResultClass};` when it
//! implements `dispatch_reconcile` (adding them now would be unused → clippy-deny).

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
}
