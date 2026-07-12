//! Diagnostics runtime (shell): per-source-partitioned store, pure debounce helpers,
//! worker dispatch (Task 4), version-gated apply (Task 4), dictionary IO.
use wordcartel_core::diagnostics::{Diagnostic, DiagnosticKind, DiagSource};
use crate::editor::{BufferId, Editor};

/// One engine's diagnostics state on one buffer: the current results, the version they were
/// computed against, and the debounce/in-flight latch (spec §5) — an INSTANCE of the machinery
/// the flat pre-SPINE `DiagStore` used to own directly, now held once per `DiagSource`.
#[derive(Debug, Default, Clone)]
pub struct SourceSlot {
    pub diagnostics: Vec<Diagnostic>,
    pub computed_version: u64,
    pub recheck_due_at: Option<u64>,
    pub in_flight_version: Option<u64>,
}

impl SourceSlot {
    /// Markers paintable only when computed against the current version AND non-empty.
    pub fn valid_for(&self, version: u64) -> bool {
        !self.diagnostics.is_empty() && self.computed_version == version
    }
    /// Arm this source's re-check `debounce_ms` from `now`.
    pub fn arm(&mut self, now: u64, debounce_ms: u64) {
        self.recheck_due_at = Some(now.saturating_add(debounce_ms));
    }
}

/// Per-buffer diagnostics, partitioned by engine (`DiagSource`). A source with no entry has
/// never been armed/computed — equivalent to an all-default `SourceSlot`, but without paying for
/// one until the source is actually used (spec §5, multi-provider generalization of the old flat
/// single-slot store).
#[derive(Debug, Default, Clone)]
pub struct DiagStore { slots: std::collections::BTreeMap<DiagSource, SourceSlot> }

impl DiagStore {
    /// An empty store — no source has a slot yet.
    pub fn new() -> Self { DiagStore::default() }
    /// The slot for `source`, if it has ever been touched (armed, computed, or latched).
    pub fn slot(&self, source: DiagSource) -> Option<&SourceSlot> { self.slots.get(&source) }
    /// The slot for `source`, creating a fresh default one on first touch.
    /// Callers must not resurrect slots for disabled sources — a result routed here for a source
    /// that is not enabled would leave a phantom empty slot behind (spec §6.2: the apply path reads
    /// `slot(source)` first and only touches `slot_mut` when the source is enabled and current).
    pub fn slot_mut(&mut self, source: DiagSource) -> &mut SourceSlot {
        self.slots.entry(source).or_default()
    }
    /// Drop `source`'s slot entirely (e.g. the engine was disabled/uninstalled).
    pub fn clear_source(&mut self, source: DiagSource) { self.slots.remove(&source); }
    /// Every slot, mutably — for whole-store operations (the ignore-union refilter).
    pub fn slots_mut(&mut self) -> impl Iterator<Item = &mut SourceSlot> { self.slots.values_mut() }
    /// Earliest armed deadline among slots with NO check in flight (per-source A3 gate) — `None`
    /// when nothing is armed or every armed slot is mid-check.
    pub fn due_deadline(&self) -> Option<u64> {
        self.slots.values()
            .filter(|s| s.in_flight_version.is_none())
            .filter_map(|s| s.recheck_due_at).min()
    }
    /// Whether ANY slot's re-check is due at `now` (armed, reached, not in flight).
    pub fn any_due(&self, now: u64) -> bool { self.due_sources(now).next().is_some() }
    /// Every source whose re-check is due at `now` (armed, reached, not in flight), in
    /// `DiagSource`'s `Ord` (BTreeMap iteration) order.
    pub fn due_sources(&self, now: u64) -> impl Iterator<Item = DiagSource> + '_ {
        self.slots.iter()
            .filter(move |(_, s)| s.in_flight_version.is_none()
                && matches!(s.recheck_due_at, Some(t) if now >= t))
            .map(|(src, _)| *src)
    }
}

/// Arm every ENABLED engine's slot on the active buffer — the multi-provider generalization of
/// the old single `store.arm`. Callers: `set_render_mode`'s arm-on-enter-Review, `arm_if_edited`'s
/// per-edit re-arm, `recheck_diagnostics`. (Dispatch itself stays single-provider-shaped this
/// task — Task 5 fans the actual worker send out over enabled sources.)
pub fn arm_enabled(editor: &mut Editor, now: u64, debounce_ms: u64) {
    let sources: Vec<DiagSource> = editor.diag_providers.enabled_sources().collect();
    let store = &mut editor.active_mut().diagnostics;
    for s in sources { store.slot_mut(s).arm(now, debounce_ms); }
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
        // Re-arm every enabled engine on the edit (spec §5): each source debounces independently.
        arm_enabled(editor, clock.now_ms(), debounce_ms);
    }
}

/// Fan out to every ENABLED source whose re-check is due at `now` (spec §7): snapshot the active
/// buffer ONCE, apply the whole-document `DIAG_MAX_SEND_BYTES` cap once, then hand the snapshot to
/// `dispatch_one` per due source — each source consumes its own slot's deadline and latches
/// independently (spec §5.1), so one source's `Accepted::No`/unavailable never blocks another's.
pub fn dispatch_diagnostics(editor: &mut Editor, now: u64) {
    let due: Vec<DiagSource> = editor.active().diagnostics.due_sources(now)
        .filter(|s| editor.diag_providers.is_enabled(*s)).collect();
    if due.is_empty() { return; }
    let b = editor.active();
    let (buffer_id, version) = (b.id, b.document.version);
    let path = b.document.path.clone();
    let text = b.document.buffer.snapshot().to_string();
    if text.len() as u64 > crate::limits::DIAG_MAX_SEND_BYTES {
        for s in &due { editor.active_mut().diagnostics.slot_mut(*s).recheck_due_at = None; }
        editor.status = "document too large for grammar checking".into();
        return;
    }
    for source in due { dispatch_one(editor, source, buffer_id, version, &path, &text); }
}

/// Dispatch a single due `source` against the already-snapshotted buffer (Effort A seam,
/// generalized to per-source in Task 5). Consumes the armed deadline, ensures the provider is
/// running, honors `Unavailable`/`Starting` with an explicit status (no silent wait, spec §4.3),
/// and sets `in_flight_version` ONLY on `Accepted::Yes` — the provider guarantees a (possibly
/// empty) `Msg::DiagnosticsDone` for every accepted version (latch invariant, spec §5.1); on
/// `Accepted::No` the thread died between the availability read and the send, so latching here
/// would wedge diagnostics permanently — leave the latch clear (a fresh dispatch retries) and
/// surface the degrade hint instead.
fn dispatch_one(editor: &mut Editor, source: DiagSource, buffer_id: BufferId, version: u64,
    path: &Option<std::path::PathBuf>, text: &str) {
    use crate::diag_provider::{Availability, Accepted};
    editor.active_mut().diagnostics.slot_mut(source).recheck_due_at = None; // consumed
    editor.diag_providers.ensure_running(source);
    // `None` (no entry registered for this source) is treated as unavailable — same as the
    // provider itself reporting `Unavailable`.
    match editor.diag_providers.availability(source) {
        Some(Availability::Unavailable) | None => { show_install_hint(editor, source); return; }
        Some(Availability::Starting) => {
            editor.status = format!("starting {}…", source.label()); // no silent wait (spec §4.3)
        }
        Some(Availability::Idle) | Some(Availability::Ready) => {}
    }
    match editor.diag_providers.notify_change(source, buffer_id, version, path.clone(),
        text.to_string()) {
        Accepted::Yes => {
            editor.active_mut().diagnostics.slot_mut(source).in_flight_version = Some(version);
        }
        Accepted::No => show_install_hint(editor, source),
    }
}

/// Surface `source`'s install hint at most once per deliberate Review entry (`diag_hint_shown` is a
/// per-source latch, cleared in `set_render_mode` on entering Review). Spec §9 — informative, not
/// naggy: each engine gets to explain itself once, independently of the others.
fn show_install_hint(editor: &mut Editor, source: DiagSource) {
    if editor.diag_hint_shown.insert(source) {
        if let Some(hint) = editor.diag_providers.install_hint(source) {
            editor.status = hint.into();
        }
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
/// Refilters EVERY source's slot (not just Harper's) — the ignore union is client-side and
/// engine-agnostic, so a newly-ignored word must disappear from whichever engine flagged it.
pub fn retain_unignored(editor: &mut Editor) {
    let union = ignore_union_lower(editor);
    if union.is_empty() { return; } // nothing suppressed → no work, no snapshot
    let text = editor.active().document.buffer.to_string();
    for slot in editor.active_mut().diagnostics.slots_mut() {
        retain_over_union(&mut slot.diagnostics, &text, &union);
    }
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

/// Version-gated apply: store only if `version` is still current for `buffer_id`. Routes into
/// `source`'s own slot — the store is source-partitioned, so a result from one engine never
/// clobbers another's (spec §5).
pub fn apply_diagnostics_done(
    editor: &mut Editor,
    buffer_id: BufferId,
    version: u64,
    source: DiagSource,
    diagnostics: Vec<Diagnostic>,
) {
    // Disabled-source drop (spec §6.2): a result for an engine that is no longer enabled is
    // dropped and its slot removed — never resurrected. `clear_source` on an absent slot is a
    // no-op, so no phantom empty slot is created (the durability invariant save.rs asserts).
    if !editor.diag_providers.is_enabled(source) {
        if let Some(b) = editor.by_id_mut(buffer_id) { b.diagnostics.clear_source(source); }
        return;
    }
    // Build the ignore union BEFORE borrowing the buffer mutably (dictionary/session_ignores live
    // on `editor`, not the buffer). Empty in the common case → the filter below is skipped.
    let union = ignore_union_lower(editor);
    if let Some(b) = editor.by_id_mut(buffer_id) {
        if b.document.version == version {
            let mut diagnostics = diagnostics;
            debug_assert!(diagnostics.iter().all(|d| d.source == source),
                "DiagnosticsDone payload sources match the message tag");
            if !union.is_empty() {
                // Apply-time ignore filter (spec §7.3): the text is this buffer at `version`, so the
                // stored byte ranges slice the right surface words.
                let text = b.document.buffer.to_string();
                retain_over_union(&mut diagnostics, &text, &union);
            }
            let slot = b.diagnostics.slot_mut(source);
            slot.diagnostics = diagnostics;
            slot.computed_version = version;
        }
        // Clear in_flight for this version if the latch is still armed for it (the check completed) —
        // but READ `slot(source)` first, so a stale result for a source with no live slot never
        // creates a phantom one (spec §6.2 non-creating latch-clear).
        if b.diagnostics.slot(source).map(|s| s.in_flight_version) == Some(Some(version)) {
            b.diagnostics.slot_mut(source).in_flight_version = None;
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
        let mut s = SourceSlot::default();
        assert!(!s.valid_for(0)); // empty slot: no diagnostics yet, regardless of version
        s.arm(1000, 400);
        assert_eq!(s.recheck_due_at, Some(1400));
    }

    #[test]
    fn any_due_requires_armed_reached_and_not_in_flight() {
        let mut s = DiagStore::new();
        s.slot_mut(DiagSource::Harper).arm(1000, 400);
        assert!(!s.any_due(1399), "not yet due");
        assert!(s.any_due(1400), "due at deadline");
        // same version in flight → blocks
        s.slot_mut(DiagSource::Harper).in_flight_version = Some(7);
        assert!(!s.any_due(1500), "already in flight for this version");
        // different version in flight → ALSO blocks (single-in-flight invariant)
        s.slot_mut(DiagSource::Harper).in_flight_version = Some(8);
        assert!(!s.any_due(1500), "in flight for a different version also blocks dispatch");
    }

    #[test]
    fn valid_for_only_when_computed_version_matches() {
        let mut s = SourceSlot { computed_version: 5, ..Default::default() };
        s.diagnostics.push(wordcartel_core::diagnostics::Diagnostic {
            range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: DiagSource::Harper, code: None, href: None,
            message: "x".into(), suggestions: vec![] });
        assert!(s.valid_for(5));
        assert!(!s.valid_for(6)); // edited since → hidden
    }

    // ------------------------------------------------------------------
    // Task 3: source-partitioned DiagStore/SourceSlot
    // ------------------------------------------------------------------

    #[test]
    fn source_slots_are_independent() {
        let mut s = DiagStore::new();
        s.slot_mut(DiagSource::Harper).arm(1000, 400);
        assert_eq!(s.slot(DiagSource::Harper).unwrap().recheck_due_at, Some(1400));
        assert!(s.slot(DiagSource::Plugin("mock")).is_none(), "untouched source has no slot");
        s.slot_mut(DiagSource::Plugin("mock")).arm(1000, 100);
        assert_eq!(s.due_deadline(), Some(1100), "earliest armed deadline across slots");
        assert!(s.any_due(1100) && !s.any_due(1099));
        assert_eq!(s.due_sources(1400).collect::<Vec<_>>(),
            vec![DiagSource::Harper, DiagSource::Plugin("mock")]); // BTreeMap order
    }

    #[test]
    fn due_deadline_excludes_in_flight_slot() {
        let mut s = DiagStore::new();
        s.slot_mut(DiagSource::Harper).arm(1000, 400);
        s.slot_mut(DiagSource::Harper).in_flight_version = Some(7);
        assert_eq!(s.due_deadline(), None, "an in-flight slot never re-drives the deadline");
        assert!(!s.any_due(2000));
    }

    #[test]
    fn arm_enabled_arms_only_enabled_sources() {
        use crate::editor::{Editor, RenderMode};
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), false);
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::Review;
        arm_enabled(&mut e, 500, 400);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, Some(900));
        assert!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).is_none(), "disabled: no slot");
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
        crate::test_support::install_enabled_harper(&mut e); // arm_if_edited → arm_enabled arms enabled sources
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::Review;
        use crate::test_support::TestClock;
        let id = e.active().id;
        let v = e.active().document.version;
        // no version change → no arm
        arm_if_edited(&mut e, id, v, &TestClock(100));
        assert!(e.active().diagnostics.slot(DiagSource::Harper).is_none(), "equal version: no arm");
        // version increased, same buffer, Review, enabled → arm at now+debounce
        e.active_mut().document.version += 1;
        arm_if_edited(&mut e, id, v, &TestClock(100));
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at,
            Some(100 + e.diag_cfg.debounce_ms));
        // same edit but in LivePreview → no arm
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).recheck_due_at = None;
        e.active_mut().view.mode = RenderMode::LivePreview;
        arm_if_edited(&mut e, id, v, &TestClock(200));
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, None,
            "not Review: no arm");
        // buffer-identity guard: active id != before_id → no arm even with a version delta
        e.active_mut().view.mode = RenderMode::Review;
        let other = crate::editor::BufferId(id.0.wrapping_add(999));
        arm_if_edited(&mut e, other, v, &TestClock(300));
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, None,
            "switch (id changed): no arm");
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
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, Some(v),
            "accepted → latch set");
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, None,
            "armed deadline consumed");
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
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, None,
            "Accepted::No must not latch");
        assert!(e.diag_hint_shown.contains(&DiagSource::Harper), "the degrade hint latch is set");
        assert_eq!(e.status, "test provider unavailable", "the installed provider's own hint");
    }

    #[test]
    fn dispatch_over_cap_sets_status_and_never_touches_provider() {
        let big = "x".repeat((crate::limits::DIAG_MAX_SEND_BYTES as usize) + 1);
        let mut e = review_editor(&big);
        let rec = RecordingProvider::new().with_source(DiagSource::Harper);
        let calls = rec.calls_handle();
        e.diag_providers.install(Box::new(rec), true);
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.status, "document too large for grammar checking");
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, None,
            "over-cap: no latch");
        assert!(calls.lock().unwrap().is_empty(), "over-cap short-circuits before the provider");
    }

    #[test]
    fn dispatch_unavailable_shows_hint_once() {
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)
            .with_availability(Availability::Unavailable)), true);
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.status, "test provider unavailable");
        assert!(e.diag_hint_shown.contains(&DiagSource::Harper));
        // Second dispatch: hint already shown → status is not re-set (informative, not naggy).
        e.status = String::new();
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 20);
        assert_eq!(e.status, "", "hint shows at most once per Review entry");
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, None);
    }

    #[test]
    fn dispatch_starting_shows_no_silent_wait_status_and_latches() {
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)
            .with_availability(Availability::Starting)), true); // still accepts (queued post-handshake)
        let v = e.active().document.version;
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.status, "starting Harper…");
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, Some(v),
            "Starting still accepts + latches");
    }

    // ------------------------------------------------------------------
    // Task 5: dispatch fan-out over enabled+due sources (dispatch_one).
    // ------------------------------------------------------------------

    #[test]
    fn dispatch_fans_out_to_all_due_enabled_sources() {
        let mut e = review_editor("teh\n");
        let h = crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper);
        let m = crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"));
        let (hc, mc) = (h.calls_handle(), m.calls_handle());
        e.diag_providers.install(Box::new(h), true);
        e.diag_providers.install(Box::new(m), true);
        let v = e.active().document.version;
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, Some(v));
        assert_eq!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap().in_flight_version, Some(v));
        assert!(hc.lock().unwrap().iter().any(|c| matches!(c, crate::diag_provider::ProviderCall::NotifyChange { version, .. } if *version == v)));
        assert!(mc.lock().unwrap().iter().any(|c| matches!(c, crate::diag_provider::ProviderCall::NotifyChange { .. })));
    }

    #[test]
    fn dispatch_skips_not_due_source_and_latches_independently() {
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new()
                .with_source(DiagSource::Plugin("mock")).with_accepted(crate::diag_provider::Accepted::No)), true);
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version.is_some());
        assert!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap().in_flight_version.is_none(),
            "Accepted::No mock does not latch; harper unaffected");
    }

    #[test]
    fn dispatch_over_cap_consumes_deadlines_and_never_latches() {
        let big = "x".repeat((crate::limits::DIAG_MAX_SEND_BYTES as usize) + 1);
        let mut e = review_editor(&big);
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.status, "document too large for grammar checking");
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, None);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, None);
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
        crate::test_support::install_enabled_harper(&mut e); // apply's is_enabled(Harper) guard
        let id = e.active().id;
        let v = e.active().document.version;
        e.dictionary.insert("TEH".into()); // case-insensitive union membership
        // "teh" (0..3) is a spelling hit → dropped; the grammar diagnostic on "cat" (4..7) stays.
        apply_diagnostics_done(&mut e, id, v, DiagSource::Harper, vec![spelling(0..3), grammar(4..7)]);
        let kept = &e.active().diagnostics.slot(DiagSource::Harper).unwrap().diagnostics;
        assert_eq!(kept.len(), 1, "spelling 'teh' filtered by dictionary; grammar retained");
        assert_eq!(kept[0].kind, DiagnosticKind::Grammar);
    }

    // ------------------------------------------------------------------
    // Task 4: source-routed apply (is_enabled guard + non-creating latch-clear).
    // ------------------------------------------------------------------

    #[test]
    fn apply_routes_to_the_named_source_slot_only() {
        let mut e = review_editor("teh cat\n");
        crate::test_support::install_enabled_harper(&mut e);
        e.diag_providers.install(Box::new(
            RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
        let id = e.active().id;
        let v = e.active().document.version;
        let mock_grammar = Diagnostic { range: 4..7, kind: DiagnosticKind::Grammar,
            source: DiagSource::Plugin("mock"), code: None, href: None,
            message: "x".into(), suggestions: vec![] };
        apply_diagnostics_done(&mut e, id, v, DiagSource::Harper, vec![spelling(0..3)]);
        apply_diagnostics_done(&mut e, id, v, DiagSource::Plugin("mock"), vec![mock_grammar]);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().diagnostics.len(), 1);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap().diagnostics.len(), 1);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().diagnostics[0].kind,
            DiagnosticKind::Spelling, "each result lands in its own source's slot, no cross-clobber");
    }

    #[test]
    fn apply_stale_version_clears_latch_without_storing() {
        let mut e = review_editor("teh\n");
        crate::test_support::install_enabled_harper(&mut e);
        let id = e.active().id;
        let v = e.active().document.version;
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).in_flight_version = Some(v);
        e.active_mut().document.version = v + 1; // edited after dispatch
        apply_diagnostics_done(&mut e, id, v, DiagSource::Harper, vec![spelling(0..3)]);
        let slot = e.active().diagnostics.slot(DiagSource::Harper).unwrap();
        assert!(slot.diagnostics.is_empty(), "stale result not stored");
        assert_eq!(slot.in_flight_version, None, "latch cleared (in_flight == msg.version)");
    }

    #[test]
    fn apply_for_disabled_source_drops_and_clears_slot() {
        let mut e = review_editor("teh\n");
        // mock is NOT installed/enabled → result dropped, slot removed.
        e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).in_flight_version = Some(1);
        let id = e.active().id;
        let v = e.active().document.version;
        let mock = Diagnostic { range: 0..3, kind: DiagnosticKind::Spelling,
            source: DiagSource::Plugin("mock"), code: None, href: None,
            message: "x".into(), suggestions: vec![] };
        apply_diagnostics_done(&mut e, id, v, DiagSource::Plugin("mock"), vec![mock]);
        assert!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).is_none(),
            "disabled source: result dropped and phantom slot removed");
    }

    #[test]
    fn retain_unignored_refilters_the_active_store_in_place() {
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).diagnostics = vec![spelling(0..3), grammar(4..7)];
        e.session_ignores.insert("teh".into());
        retain_unignored(&mut e);
        let kept = &e.active().diagnostics.slot(DiagSource::Harper).unwrap().diagnostics;
        assert_eq!(kept.len(), 1, "the newly-ignored spelling word is dropped in place");
        assert_eq!(kept[0].kind, DiagnosticKind::Grammar);
    }
}
