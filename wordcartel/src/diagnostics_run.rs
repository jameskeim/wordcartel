//! Diagnostics runtime (shell): per-buffer store, pure debounce helpers,
//! worker dispatch (Task 4), version-gated apply (Task 4), dictionary IO.
use wordcartel_core::diagnostics::Diagnostic;

#[derive(Debug, Default, Clone)]
pub struct DiagStore {
    pub diagnostics: Vec<Diagnostic>,
    pub computed_version: u64,
    pub recheck_due_at: Option<u64>,
    pub in_flight_version: Option<u64>,
}

impl DiagStore {
    pub fn new() -> Self { DiagStore::default() }
    /// Markers are paintable only when computed against the current version
    /// AND there is something to paint.
    pub fn valid_for(&self, version: u64) -> bool {
        !self.diagnostics.is_empty() && self.computed_version == version
    }
    /// Arm a re-check `debounce_ms` from `now`.
    pub fn arm(&mut self, now: u64, debounce_ms: u64) {
        self.recheck_due_at = Some(now.saturating_add(debounce_ms));
    }
}

/// Smallest of the deadline terms; None terms ignored.
pub fn next_deadline(terms: &[Option<u64>]) -> Option<u64> {
    terms.iter().flatten().copied().min()
}

/// A re-check is due if armed, the time has been reached, and no check is
/// already in flight for this exact version.
pub fn diag_due(store: &DiagStore, now: u64, version: u64) -> bool {
    matches!(store.recheck_due_at, Some(t) if now >= t)
        && store.in_flight_version != Some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_deadline_is_min_ignoring_none() {
        assert_eq!(next_deadline(&[None, Some(50), None, Some(20), Some(99)]), Some(20));
        assert_eq!(next_deadline(&[None, None]), None);
    }

    #[test]
    fn arm_sets_due_and_valid_for_tracks_version() {
        let mut s = DiagStore::new();
        assert!(!s.valid_for(0)); // empty store: computed_version default != a fresh version? see new()
        s.arm(1000, 400);
        assert_eq!(s.recheck_due_at, Some(1400));
    }

    #[test]
    fn diag_due_requires_armed_reached_and_not_in_flight() {
        let mut s = DiagStore::new();
        s.arm(1000, 400);
        assert!(!diag_due(&s, 1399, 7), "not yet due");
        assert!(diag_due(&s, 1400, 7), "due at deadline");
        s.in_flight_version = Some(7);
        assert!(!diag_due(&s, 1500, 7), "already in flight for this version");
    }

    #[test]
    fn valid_for_only_when_computed_version_matches() {
        let mut s = DiagStore::new();
        s.computed_version = 5;
        s.diagnostics.push(wordcartel_core::diagnostics::Diagnostic {
            range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            message: "x".into(), suggestions: vec![] });
        assert!(s.valid_for(5));
        assert!(!s.valid_for(6)); // edited since → hidden
    }
}
