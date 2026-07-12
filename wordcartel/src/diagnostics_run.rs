//! Diagnostics runtime (shell): per-buffer store, pure debounce helpers,
//! worker dispatch (Task 4), version-gated apply (Task 4), dictionary IO.
use wordcartel_core::diagnostics::{Diagnostic, DiagnosticKind, DiagSource};
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

/// Consume the armed deadline and hand the buffer to the diagnostics provider (Effort A seam).
/// Sets `in_flight_version` ONLY when a notify was actually enqueued — the provider guarantees a
/// (possibly empty) `Msg::DiagnosticsDone` for every accepted version (latch invariant, spec §5.1);
/// on `Accepted::No` the latch stays clear so a fresh dispatch retries instead of wedging.
pub fn dispatch_diagnostics(editor: &mut Editor) {
    let b = editor.active();
    let (buffer_id, version) = (b.id, b.document.version);
    let path = b.document.path.clone();
    let text = b.document.buffer.snapshot().to_string();
    editor.active_mut().diagnostics.recheck_due_at = None; // consumed
    if text.len() as u64 > crate::limits::DIAG_MAX_SEND_BYTES {
        editor.status = "document too large for grammar checking".into();
        return; // no in_flight; nothing outstanding
    }
    use crate::diag_provider::{Availability, Accepted};
    use wordcartel_core::diagnostics::DiagSource;
    editor.diag_providers.ensure_running(DiagSource::Harper);
    // `None` (no Harper entry registered) is treated as unavailable — same as the provider itself
    // reporting `Unavailable` (Tasks 5/6 generalize this to every registered source).
    match editor.diag_providers.availability(DiagSource::Harper) {
        Some(Availability::Unavailable) | None => {
            show_install_hint(editor);
            return; // no in_flight
        }
        Some(Availability::Starting) => {
            editor.status = "starting grammar checker…".into(); // no silent wait (spec §4.3)
        }
        _ => {}
    }
    // LATCH INVARIANT (spec §5.1): set in_flight_version ONLY on Accepted::Yes. On Accepted::No the
    // thread died between the availability read and the send — no terminal DiagnosticsDone would ever
    // arrive, so latching here would wedge diagnostics permanently. Leave the latch clear (a fresh
    // dispatch retries) and surface the degrade hint.
    match editor.diag_providers.notify_change(DiagSource::Harper, buffer_id, version, path, text) {
        Accepted::Yes => { editor.active_mut().diagnostics.in_flight_version = Some(version); }
        Accepted::No => show_install_hint(editor),
    }
}

/// Surface the install hint at most once per deliberate Review entry (`diag_hint_shown` latch,
/// reset in `set_render_mode` on entering Review). Spec §9 — informative, not naggy.
fn show_install_hint(editor: &mut Editor) {
    if !editor.diag_hint_shown {
        editor.diag_hint_shown = true;
        editor.status = crate::harper_ls::INSTALL_HINT.into();
    }
}

/// The client-side ignore union — personal dictionary ∪ session ignores — lowercased for
/// case-insensitive membership (spec §7.3/§7.4). Empty ⟹ nothing is suppressed (the common case).
fn ignore_union_lower(editor: &Editor) -> std::collections::HashSet<String> {
    editor.dictionary.iter().chain(editor.session_ignores.iter())
        .map(|w| w.to_lowercase()).collect()
}

/// Drop every `Spelling` diagnostic whose surface word (sliced from `text`) is in `union`;
/// retain everything else (non-spelling diagnostics are never suppressed). Byte ranges index into
/// `text`, which is the buffer content of the diagnostics' version.
fn retain_over_union(diags: &mut Vec<Diagnostic>, text: &str,
    union: &std::collections::HashSet<String>) {
    diags.retain(|d| {
        if d.kind != DiagnosticKind::Spelling { return true; }
        let surface = text.get(d.range.start..d.range.end).unwrap_or("");
        !union.contains(&surface.to_lowercase())
    });
}

/// In-place refilter of the ACTIVE buffer's `DiagStore` against the ignore union — an immediate,
/// server-round-trip-free underline update after an ignore/add-dict overlay row (spec §7.3).
pub fn retain_unignored(editor: &mut Editor) {
    let union = ignore_union_lower(editor);
    if union.is_empty() { return; } // nothing suppressed → no work, no snapshot
    let text = editor.active().document.buffer.to_string();
    retain_over_union(&mut editor.active_mut().diagnostics.diagnostics, &text, &union);
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
    source: DiagSource,
    diagnostics: Vec<Diagnostic>,
) {
    // `source` is threaded through the signature this task (Task 3/4 route the flat store below
    // by source); the single-store body does not yet read it.
    let _ = source;
    // Build the ignore union BEFORE borrowing the buffer mutably (dictionary/session_ignores live
    // on `editor`, not the buffer). Empty in the common case → the filter below is skipped.
    let union = ignore_union_lower(editor);
    if let Some(b) = editor.by_id_mut(buffer_id) {
        if b.document.version == version {
            let mut diagnostics = diagnostics;
            if !union.is_empty() {
                // Apply-time ignore filter (spec §7.3): the text is this buffer at `version`, so the
                // stored byte ranges slice the right surface words.
                let text = b.document.buffer.to_string();
                retain_over_union(&mut diagnostics, &text, &union);
            }
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
            source: DiagSource::Harper, code: None, href: None,
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

    // ------------------------------------------------------------------
    // Effort A: dispatch_diagnostics over the DiagnosticsProvider seam.
    // ------------------------------------------------------------------
    use crate::editor::{Editor, RenderMode};
    use crate::diag_provider::{RecordingProvider, Availability, Accepted};
    use crate::harper_ls::INSTALL_HINT;
    use wordcartel_core::diagnostics::DiagSource;

    fn review_editor(text: &str) -> Editor {
        let mut e = Editor::new_from_text(text, None, (80, 24));
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::Review;
        e
    }

    #[test]
    fn dispatch_latches_in_flight_only_on_accepted_yes() {
        let mut e = review_editor("teh\n");
        let rec = RecordingProvider::new().with_source(DiagSource::Harper); // Ready, accepts
        let calls = rec.calls_handle();
        e.diag_providers.install(Box::new(rec), true);
        let v = e.active().document.version;
        e.active_mut().diagnostics.arm(0, 400);
        dispatch_diagnostics(&mut e);
        assert_eq!(e.active().diagnostics.in_flight_version, Some(v), "accepted → latch set");
        assert_eq!(e.active().diagnostics.recheck_due_at, None, "armed deadline consumed");
        let log = calls.lock().unwrap();
        assert!(log.iter().any(|c| matches!(c, crate::diag_provider::ProviderCall::EnsureRunning)));
        assert!(log.iter().any(|c| matches!(c,
            crate::diag_provider::ProviderCall::NotifyChange { version, .. } if *version == v)));
    }

    #[test]
    fn dispatch_no_latch_and_hint_on_accepted_no() {
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)
            .with_accepted(Accepted::No).with_availability(Availability::Ready)), true);
        dispatch_diagnostics(&mut e);
        assert_eq!(e.active().diagnostics.in_flight_version, None, "Accepted::No must not latch");
        assert!(e.diag_hint_shown, "the degrade hint latch is set");
        assert_eq!(e.status, INSTALL_HINT);
    }

    #[test]
    fn dispatch_over_cap_sets_status_and_never_touches_provider() {
        let big = "x".repeat((crate::limits::DIAG_MAX_SEND_BYTES as usize) + 1);
        let mut e = review_editor(&big);
        let rec = RecordingProvider::new().with_source(DiagSource::Harper);
        let calls = rec.calls_handle();
        e.diag_providers.install(Box::new(rec), true);
        dispatch_diagnostics(&mut e);
        assert_eq!(e.status, "document too large for grammar checking");
        assert_eq!(e.active().diagnostics.in_flight_version, None, "over-cap: no latch");
        assert!(calls.lock().unwrap().is_empty(), "over-cap short-circuits before the provider");
    }

    #[test]
    fn dispatch_unavailable_shows_hint_once() {
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)
            .with_availability(Availability::Unavailable)), true);
        dispatch_diagnostics(&mut e);
        assert_eq!(e.status, INSTALL_HINT);
        assert!(e.diag_hint_shown);
        // Second dispatch: hint already shown → status is not re-set (informative, not naggy).
        e.status = String::new();
        e.active_mut().diagnostics.arm(0, 400);
        dispatch_diagnostics(&mut e);
        assert_eq!(e.status, "", "hint shows at most once per Review entry");
        assert_eq!(e.active().diagnostics.in_flight_version, None);
    }

    #[test]
    fn dispatch_starting_shows_no_silent_wait_status_and_latches() {
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)
            .with_availability(Availability::Starting)), true); // still accepts (queued post-handshake)
        let v = e.active().document.version;
        dispatch_diagnostics(&mut e);
        assert_eq!(e.status, "starting grammar checker…");
        assert_eq!(e.active().diagnostics.in_flight_version, Some(v), "Starting still accepts + latches");
    }

    fn spelling(range: std::ops::Range<usize>) -> Diagnostic {
        Diagnostic { range, kind: DiagnosticKind::Spelling, source: DiagSource::Harper, code: None,
            href: None, message: "x".into(), suggestions: vec![] }
    }
    fn grammar(range: std::ops::Range<usize>) -> Diagnostic {
        Diagnostic { range, kind: DiagnosticKind::Grammar, source: DiagSource::Harper, code: None,
            href: None, message: "x".into(), suggestions: vec![] }
    }

    #[test]
    fn apply_filters_ignored_spelling_over_the_union_keeps_grammar() {
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        let id = e.active().id;
        let v = e.active().document.version;
        e.dictionary.insert("TEH".into()); // case-insensitive union membership
        // "teh" (0..3) is a spelling hit → dropped; the grammar diagnostic on "cat" (4..7) stays.
        apply_diagnostics_done(&mut e, id, v, DiagSource::Harper, vec![spelling(0..3), grammar(4..7)]);
        let kept = &e.active().diagnostics.diagnostics;
        assert_eq!(kept.len(), 1, "spelling 'teh' filtered by dictionary; grammar retained");
        assert_eq!(kept[0].kind, DiagnosticKind::Grammar);
    }

    #[test]
    fn retain_unignored_refilters_the_active_store_in_place() {
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        e.active_mut().diagnostics.diagnostics = vec![spelling(0..3), grammar(4..7)];
        e.session_ignores.insert("teh".into());
        retain_unignored(&mut e);
        let kept = &e.active().diagnostics.diagnostics;
        assert_eq!(kept.len(), 1, "the newly-ignored spelling word is dropped in place");
        assert_eq!(kept[0].kind, DiagnosticKind::Grammar);
    }
}
