//! Diagnostics runtime (shell): per-buffer store, pure debounce helpers,
//! worker dispatch (Task 4), version-gated apply (Task 4), dictionary IO.
use wordcartel_core::diagnostics::Diagnostic;
use crate::editor::{BufferId, Editor};

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
/// already in flight (for any version). A check in flight for a different
/// version also blocks dispatch — the result will arrive shortly and the
/// debounce will re-arm for the latest version, preventing pile-up during
/// the slow first Harper init.
pub fn diag_due(store: &DiagStore, now: u64, _version: u64) -> bool {
    matches!(store.recheck_due_at, Some(t) if now >= t)
        && store.in_flight_version.is_none()
}

/// Spawn a worker thread that runs Harper and sends Msg::DiagnosticsDone.
/// Mirrors filter::dispatch_filter (spawn + msg_tx). Sets in_flight_version.
pub fn dispatch_diagnostics(
    editor: &mut Editor,
    cfg: &crate::config::DiagnosticsConfig,
    ignore_words: std::sync::Arc<std::collections::HashSet<String>>,
    msg_tx: std::sync::mpsc::Sender<crate::app::Msg>,
) {
    let b = editor.active();
    let buffer_id = b.id;
    let version = b.document.version;
    let text = b.document.buffer.snapshot().to_string();
    let grammar = cfg.grammar;
    editor.active_mut().diagnostics.in_flight_version = Some(version);
    editor.active_mut().diagnostics.recheck_due_at = None; // consumed
    std::thread::spawn(move || {
        let opts = wordcartel_core::diagnostics::CheckOpts { grammar, ignore_words: &ignore_words };
        let diagnostics = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wordcartel_core::diagnostics::check(&text, &opts)
        })).unwrap_or_default(); // Harper panic → no diagnostics, never crash the loop (spec §8)
        let _ = msg_tx.send(crate::app::Msg::DiagnosticsDone { buffer_id, version, diagnostics });
    });
}

/// Append `word` to the personal dictionary file (create if missing).
/// Returns `Ok(())` on success, `Err(e)` on IO failure (caller shows status).
pub fn append_word_to_dict(path: &std::path::Path, word: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", word)
}

/// Version-gated apply: store only if `version` is still current for `buffer_id`.
pub fn apply_diagnostics_done(
    editor: &mut Editor,
    buffer_id: BufferId,
    version: u64,
    diagnostics: Vec<Diagnostic>,
) {
    if let Some(b) = editor.by_id_mut(buffer_id) {
        if b.document.version == version {
            b.diagnostics.diagnostics = diagnostics;
            b.diagnostics.computed_version = version;
        }
        // clear in_flight for this version regardless (the check completed)
        if b.diagnostics.in_flight_version == Some(version) {
            b.diagnostics.in_flight_version = None;
        }
    }
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
        // same version in flight → blocks
        s.in_flight_version = Some(7);
        assert!(!diag_due(&s, 1500, 7), "already in flight for this version");
        // different version in flight → ALSO blocks (single-in-flight invariant)
        s.in_flight_version = Some(8);
        assert!(!diag_due(&s, 1500, 7), "in flight for a different version also blocks dispatch");
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
