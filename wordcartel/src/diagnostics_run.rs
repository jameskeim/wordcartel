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

/// Compute gate: diagnostics arm/dispatch only when the feature is enabled AND the active buffer
/// is in the Review render mode. (Spec §2.1.)
pub fn should_run_diagnostics(editor: &Editor) -> bool {
    editor.diag_cfg.enabled && editor.active().view.mode == crate::editor::RenderMode::Review
}

/// Display gate: underlines paint under exactly the same predicate. Distinct name for the distinct
/// role (compute vs paint); delegates so the two cannot drift.
pub fn should_show_diagnostics(editor: &Editor) -> bool { should_run_diagnostics(editor) }

/// The single diagnostics re-arm seam (spec §2.2 item 1). After a `reduce` message, if the SAME
/// buffer is still active AND its document.version advanced since the pre-dispatch snapshot, arm the
/// debounced recheck — but only when in Review with checking enabled. Wraps every `reduce` exit path
/// (interceptor early-returns AND the normal tail), so every active-buffer edit re-arms exactly once,
/// with no per-path enumeration, no double-arm, and no false arm on a buffer switch (§2.3).
pub fn arm_if_edited(editor: &mut Editor, before_id: BufferId, before_version: u64,
    clock: &dyn wordcartel_core::history::Clock) {
    if editor.active().id == before_id
        && editor.active().document.version != before_version
        && should_run_diagnostics(editor)
    {
        let debounce_ms = editor.diag_cfg.debounce_ms;
        editor.active_mut().diagnostics.arm(clock.now_ms(), debounce_ms);
    }
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
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

    #[test]
    fn should_run_diagnostics_only_in_review_and_enabled() {
        use crate::editor::{Editor, RenderMode};
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        e.diag_cfg.enabled = true;
        for (mode, want) in [(RenderMode::LivePreview, false), (RenderMode::Review, true),
                             (RenderMode::SourceHighlighted, false), (RenderMode::SourcePlain, false)] {
            e.active_mut().view.mode = mode;
            assert_eq!(should_run_diagnostics(&e), want, "{mode:?} enabled");
            assert_eq!(should_show_diagnostics(&e), want, "show mirrors run: {mode:?}");
        }
        e.active_mut().view.mode = RenderMode::Review;
        e.diag_cfg.enabled = false;
        assert!(!should_run_diagnostics(&e), "disabled → false even in Review");
    }

    #[test]
    fn arm_if_edited_arms_only_on_active_buffer_edit_in_review() {
        use crate::editor::{Editor, RenderMode};
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::Review;
        use crate::test_support::TestClock;
        let id = e.active().id;
        let v = e.active().document.version;
        // no version change → no arm
        arm_if_edited(&mut e, id, v, &TestClock(100));
        assert_eq!(e.active().diagnostics.recheck_due_at, None, "equal version: no arm");
        // version increased, same buffer, Review, enabled → arm at now+debounce
        e.active_mut().document.version += 1;
        arm_if_edited(&mut e, id, v, &TestClock(100));
        assert_eq!(e.active().diagnostics.recheck_due_at, Some(100 + e.diag_cfg.debounce_ms));
        // same edit but in LivePreview → no arm
        e.active_mut().diagnostics.recheck_due_at = None;
        e.active_mut().view.mode = RenderMode::LivePreview;
        arm_if_edited(&mut e, id, v, &TestClock(200));
        assert_eq!(e.active().diagnostics.recheck_due_at, None, "not Review: no arm");
        // buffer-identity guard: active id != before_id → no arm even with a version delta
        e.active_mut().view.mode = RenderMode::Review;
        let other = crate::editor::BufferId(id.0.wrapping_add(999));
        arm_if_edited(&mut e, other, v, &TestClock(300));
        assert_eq!(e.active().diagnostics.recheck_due_at, None, "switch (id changed): no arm");
    }

    #[test]
    fn append_word_to_dict_creates_parent_dir() {
        let temp_dir = format!("/tmp/wordcartel_test_{}", std::process::id());
        let dict_path = std::path::PathBuf::from(&temp_dir)
            .join("subdir")
            .join("nested")
            .join("dictionary.txt");

        // Clean up before test
        let _ = std::fs::remove_dir_all(&temp_dir);

        // Should succeed even though parent dirs don't exist
        append_word_to_dict(&dict_path, "testword").expect("append should succeed");

        assert!(dict_path.exists(), "dictionary file should exist");
        let content = std::fs::read_to_string(&dict_path).expect("should read file");
        assert!(content.contains("testword"), "file should contain the appended word");

        // Clean up after test
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
