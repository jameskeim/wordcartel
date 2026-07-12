//! The `DiagnosticsProvider` seam (Effort A/SPINE): a thin, mockable trait behind which a
//! diagnostics backend runs, plus the [`ProviderSet`] registry that holds every installed engine
//! keyed by `DiagSource`. The empty `ProviderSet` (no entries) is the hermetic default — no
//! thread, no process, no emissions — the role `NullProvider` used to play. `HarperLs`
//! (harper_ls.rs) is the first real provider.
use crate::editor::{BufferId, Editor};
use wordcartel_core::diagnostics::DiagSource;
use wordcartel_core::history::Clock;

/// Coarse lifecycle state of the backing process/connection (spec §2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Availability { Idle, Starting, Ready, Unavailable }

/// Whether a `notify_change` will produce a terminal `Msg::DiagnosticsDone` (spec §5.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accepted { Yes, No }

/// Provider-facing config, derived from `crate::config::DiagnosticsConfig` (spec §2).
#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub grammar: bool,
    pub dictionary: Option<std::path::PathBuf>,
    pub max_file_length: u64,
}

/// Lifecycle events a provider emits asynchronously, delivered as `Msg::DiagProviderEvent`.
#[derive(Clone, Debug)]
pub enum ProviderEvent { Restarted, Degraded(String) }

/// The diagnostics backend seam (Effort A). Thin, mockable; results are emitted asynchronously as
/// `Msg::DiagnosticsDone` (and lifecycle as `Msg::DiagProviderEvent`) on the `Sender<Msg>` the impl
/// was constructed with. All methods are non-blocking (hot-path law).
pub trait DiagnosticsProvider: std::fmt::Debug {
    /// The engine identity — the namespace tag other subsystems (store, status line, lens) key on.
    fn source(&self) -> DiagSource;
    /// Status-line hint shown when this engine is unavailable (spec §9). `&'static str`: each
    /// provider owns its own install copy (harper's lives in `harper_ls.rs`).
    fn install_hint(&self) -> &'static str;
    fn availability(&self) -> Availability;
    fn ensure_running(&mut self);
    fn configure(&mut self, cfg: ProviderConfig);
    /// Full-document sync. `Accepted::Yes` ⟹ at least one terminal `DiagnosticsDone` for
    /// `(buffer_id, version)` is guaranteed (spec §5.1); `Accepted::No` ⟹ nothing emitted, caller
    /// must NOT latch.
    fn notify_change(&mut self, buffer_id: BufferId, version: u64,
        path: Option<std::path::PathBuf>, text: String) -> Accepted;
    fn notify_close(&mut self, buffer_id: BufferId);
    /// Best-effort: ask the server to re-read `userDictPath` (a config resend). NOT a writer.
    fn reload_dictionary(&mut self);
    fn shutdown(&mut self);
}

/// The registered diagnostic engines, identified by `DiagSource`. Insertion order is the lens
/// cycle order (core catalog order — harper first). Hermetic default: empty (no thread, no
/// process, no emissions — the role `NullProvider` used to play).
#[derive(Debug, Default)]
pub struct ProviderSet { entries: Vec<ProviderEntry> }

#[derive(Debug)]
struct ProviderEntry { enabled: bool, provider: Box<dyn DiagnosticsProvider> }

impl ProviderSet {
    /// Register an engine. Duplicate sources are a wiring bug (cold startup path).
    pub fn install(&mut self, provider: Box<dyn DiagnosticsProvider>, enabled: bool) {
        let src = provider.source();
        assert!(!self.entries.iter().any(|e| e.provider.source() == src),
            "duplicate diagnostics provider source: {src:?}");
        self.entries.push(ProviderEntry { enabled, provider });
    }
    pub fn sources(&self) -> impl Iterator<Item = DiagSource> + '_ {
        self.entries.iter().map(|e| e.provider.source())
    }
    pub fn enabled_sources(&self) -> impl Iterator<Item = DiagSource> + '_ {
        self.entries.iter().filter(|e| e.enabled).map(|e| e.provider.source())
    }
    pub fn is_enabled(&self, source: DiagSource) -> bool {
        self.entries.iter().any(|e| e.provider.source() == source && e.enabled)
    }
    pub fn set_enabled(&mut self, source: DiagSource, on: bool) -> bool {
        match self.entries.iter_mut().find(|e| e.provider.source() == source) {
            Some(e) => { e.enabled = on; true }
            None => false,
        }
    }
    fn get_mut(&mut self, source: DiagSource) -> Option<&mut ProviderEntry> {
        self.entries.iter_mut().find(|e| e.provider.source() == source)
    }
    fn get(&self, source: DiagSource) -> Option<&ProviderEntry> {
        self.entries.iter().find(|e| e.provider.source() == source)
    }
    pub fn availability(&self, source: DiagSource) -> Option<Availability> {
        self.get(source).map(|e| e.provider.availability())
    }
    pub fn install_hint(&self, source: DiagSource) -> Option<&'static str> {
        self.get(source).map(|e| e.provider.install_hint())
    }
    pub fn ensure_running(&mut self, source: DiagSource) {
        if let Some(e) = self.get_mut(source) { e.provider.ensure_running(); }
    }
    pub fn notify_change(&mut self, source: DiagSource, buffer_id: BufferId, version: u64,
        path: Option<std::path::PathBuf>, text: String) -> Accepted {
        match self.get_mut(source) {
            Some(e) => e.provider.notify_change(buffer_id, version, path, text),
            None => Accepted::No,
        }
    }
    pub fn configure(&mut self, source: DiagSource, cfg: ProviderConfig) {
        if let Some(e) = self.get_mut(source) { e.provider.configure(cfg); }
    }
    pub fn notify_close_all(&mut self, buffer_id: BufferId) {
        for e in self.entries.iter_mut() { e.provider.notify_close(buffer_id); }
    }
    pub fn reload_dictionary_enabled(&mut self) {
        for e in self.entries.iter_mut().filter(|e| e.enabled) { e.provider.reload_dictionary(); }
    }
    pub fn shutdown_all(&mut self) {
        for e in self.entries.iter_mut() { e.provider.shutdown(); }
    }
}

/// Thin reduce/prompts delegation (spec §2). `clock` is needed for the Restarted re-arm.
/// `source` identifies the emitting provider — the re-arm below routes to that source's OWN
/// slot (Task 3: the store is now source-partitioned).
pub fn apply_provider_event(editor: &mut Editor, source: DiagSource, ev: ProviderEvent, clock: &dyn Clock) {
    match ev {
        ProviderEvent::Restarted => {
            editor.status = format!("{} restarted", source.label());
            if crate::diagnostics_run::should_run_diagnostics(editor)
                && editor.diag_providers.is_enabled(source)
            {
                let now = clock.now_ms();
                let debounce = editor.diag_cfg.debounce_ms;
                editor.active_mut().diagnostics.slot_mut(source).arm(now, debounce);
            }
        }
        ProviderEvent::Degraded(hint) => { editor.status = hint; }
    }
}

/// One recorded call into a [`RecordingProvider`] (mockability: dispatch/handler tests assert on
/// provider interaction without harper-ls installed).
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ProviderCall {
    EnsureRunning,
    Configure(ProviderConfig),
    NotifyChange { buffer_id: BufferId, version: u64, path: Option<std::path::PathBuf>, text: String },
    NotifyClose(BufferId),
    ReloadDictionary,
    Shutdown,
}

// `ProviderConfig` needs `PartialEq` only for the recorded-call assertions above; derived here
// (rather than on the production type) to keep the production struct's derive list minimal.
#[cfg(test)]
impl PartialEq for ProviderConfig {
    fn eq(&self, other: &Self) -> bool {
        self.grammar == other.grammar
            && self.dictionary == other.dictionary
            && self.max_file_length == other.max_file_length
    }
}

/// `#[cfg(test)]` mock `DiagnosticsProvider`: records every call it receives and returns a
/// settable `Accepted`/`Availability`, so dispatch/handler tests (T3/T5) exercise the seam
/// without harper-ls installed.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct RecordingProvider {
    // Shared handle so a test can read the call log AFTER the provider is boxed into the
    // `ProviderSet` and moved out of reach: clone `calls_handle()` before installing.
    calls: std::sync::Arc<std::sync::Mutex<Vec<ProviderCall>>>,
    accepted: Accepted,
    availability: Availability,
    source: DiagSource,
}

#[cfg(test)]
impl RecordingProvider {
    /// New recorder: `notify_change` accepts, `availability()` reports `Ready`, source is
    /// `DiagSource::Plugin("recording")` (override with `with_source`).
    pub(crate) fn new() -> Self {
        RecordingProvider {
            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            accepted: Accepted::Yes, availability: Availability::Ready,
            source: DiagSource::Plugin("recording"),
        }
    }
    pub(crate) fn with_accepted(mut self, accepted: Accepted) -> Self { self.accepted = accepted; self }
    pub(crate) fn with_availability(mut self, availability: Availability) -> Self {
        self.availability = availability; self
    }
    pub(crate) fn with_source(mut self, source: DiagSource) -> Self { self.source = source; self }
    /// Shared call-log handle — clone it before boxing to observe interaction post-install.
    pub(crate) fn calls_handle(&self) -> std::sync::Arc<std::sync::Mutex<Vec<ProviderCall>>> {
        std::sync::Arc::clone(&self.calls)
    }
    /// Snapshot of the recorded calls in order.
    pub(crate) fn calls(&self) -> Vec<ProviderCall> { self.calls.lock().expect("calls mutex").clone() }
    fn push(&self, call: ProviderCall) { self.calls.lock().expect("calls mutex").push(call); }
}

#[cfg(test)]
impl DiagnosticsProvider for RecordingProvider {
    fn source(&self) -> DiagSource { self.source }
    fn install_hint(&self) -> &'static str { "test provider unavailable" }
    fn availability(&self) -> Availability { self.availability }
    fn ensure_running(&mut self) { self.push(ProviderCall::EnsureRunning); }
    fn configure(&mut self, cfg: ProviderConfig) { self.push(ProviderCall::Configure(cfg)); }
    fn notify_change(&mut self, buffer_id: BufferId, version: u64,
        path: Option<std::path::PathBuf>, text: String) -> Accepted {
        self.push(ProviderCall::NotifyChange { buffer_id, version, path, text });
        self.accepted
    }
    fn notify_close(&mut self, buffer_id: BufferId) { self.push(ProviderCall::NotifyClose(buffer_id)); }
    fn reload_dictionary(&mut self) { self.push(ProviderCall::ReloadDictionary); }
    fn shutdown(&mut self) { self.push(ProviderCall::Shutdown); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Editor, RenderMode};
    use crate::test_support::TestClock;

    // ------------------------------------------------------------------
    // RecordingProvider: records every call; return values are settable.
    // ------------------------------------------------------------------

    #[test]
    fn recording_provider_records_every_call_in_order() {
        let mut p = RecordingProvider::new();
        p.ensure_running();
        p.configure(ProviderConfig { grammar: false, dictionary: Some("/d".into()), max_file_length: 9 });
        let accepted = p.notify_change(BufferId(3), 7, Some("/f.md".into()), "text".into());
        assert_eq!(accepted, Accepted::Yes, "default recorder accepts");
        p.notify_close(BufferId(3));
        p.reload_dictionary();
        p.shutdown();

        assert_eq!(p.calls(), vec![
            ProviderCall::EnsureRunning,
            ProviderCall::Configure(ProviderConfig { grammar: false, dictionary: Some("/d".into()), max_file_length: 9 }),
            ProviderCall::NotifyChange { buffer_id: BufferId(3), version: 7, path: Some("/f.md".into()), text: "text".into() },
            ProviderCall::NotifyClose(BufferId(3)),
            ProviderCall::ReloadDictionary,
            ProviderCall::Shutdown,
        ]);
    }

    #[test]
    fn recording_provider_accepted_and_availability_are_settable() {
        let mut p = RecordingProvider::new().with_accepted(Accepted::No).with_availability(Availability::Unavailable);
        assert_eq!(p.availability(), Availability::Unavailable);
        assert_eq!(p.notify_change(BufferId(0), 0, None, String::new()), Accepted::No);
    }

    // ------------------------------------------------------------------
    // apply_provider_event
    // ------------------------------------------------------------------

    #[test]
    fn restarted_sets_status_and_arms_when_review_and_enabled() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        crate::test_support::install_enabled_harper(&mut e); // arm now gated on is_enabled(source)
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::Review;
        apply_provider_event(&mut e, DiagSource::Harper, ProviderEvent::Restarted, &TestClock::new(1000));
        assert_eq!(e.status, "Harper restarted");
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at,
            Some(1000 + e.diag_cfg.debounce_ms));
    }

    #[test]
    fn restarted_sets_status_but_does_not_arm_outside_review() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        crate::test_support::install_enabled_harper(&mut e);
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::LivePreview;
        apply_provider_event(&mut e, DiagSource::Harper, ProviderEvent::Restarted, &TestClock::new(1000));
        assert_eq!(e.status, "Harper restarted");
        assert!(e.active().diagnostics.slot(DiagSource::Harper).is_none(), "not Review: no arm");
    }

    #[test]
    fn restarted_sets_status_but_does_not_arm_when_disabled() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        crate::test_support::install_enabled_harper(&mut e);
        e.diag_cfg.enabled = false;
        e.active_mut().view.mode = RenderMode::Review;
        apply_provider_event(&mut e, DiagSource::Harper, ProviderEvent::Restarted, &TestClock::new(1000));
        assert_eq!(e.status, "Harper restarted");
        assert!(e.active().diagnostics.slot(DiagSource::Harper).is_none(), "disabled: no arm");
    }

    #[test]
    fn restarted_arms_only_its_source_slot() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)), true);
        e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::Review;
        apply_provider_event(&mut e, DiagSource::Plugin("mock"), ProviderEvent::Restarted, &TestClock::new(1000));
        assert_eq!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap().recheck_due_at,
            Some(1000 + e.diag_cfg.debounce_ms));
        assert!(e.active().diagnostics.slot(DiagSource::Harper).is_none(), "other source not armed");
    }

    #[test]
    fn degraded_sets_status_to_the_hint_verbatim() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        apply_provider_event(&mut e, DiagSource::Harper,
            ProviderEvent::Degraded(crate::harper_ls::INSTALL_HINT.into()), &TestClock::new(0));
        assert_eq!(e.status, crate::harper_ls::INSTALL_HINT);
    }

    // ------------------------------------------------------------------
    // ProviderSet: the multi-provider registry.
    // ------------------------------------------------------------------

    #[test]
    fn provider_set_registers_and_reports_enabled() {
        let mut set = ProviderSet::default();
        set.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)), true);
        set.install(Box::new(RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), false);
        assert_eq!(set.sources().collect::<Vec<_>>(), vec![DiagSource::Harper, DiagSource::Plugin("mock")]);
        assert_eq!(set.enabled_sources().collect::<Vec<_>>(), vec![DiagSource::Harper]);
        assert!(set.is_enabled(DiagSource::Harper));
        assert!(!set.is_enabled(DiagSource::Plugin("mock")));
        assert!(set.set_enabled(DiagSource::Plugin("mock"), true));
        assert!(set.is_enabled(DiagSource::Plugin("mock")));
        assert!(!set.set_enabled(DiagSource::Vale, true), "unknown source → false");
    }

    #[test]
    fn provider_set_source_keyed_delegation() {
        let mut set = ProviderSet::default();
        let rec = RecordingProvider::new().with_source(DiagSource::Harper);
        let calls = rec.calls_handle();
        set.install(Box::new(rec), true);
        set.ensure_running(DiagSource::Harper);
        assert_eq!(set.availability(DiagSource::Harper), Some(Availability::Ready));
        assert_eq!(set.availability(DiagSource::Vale), None, "unknown source → None");
        let a = set.notify_change(DiagSource::Harper, BufferId(1), 3, None, "t".into());
        assert_eq!(a, Accepted::Yes);
        assert_eq!(set.notify_change(DiagSource::Vale, BufferId(1), 3, None, "t".into()), Accepted::No,
            "unknown source never latches");
        assert!(calls.lock().unwrap().iter().any(|c| matches!(c, ProviderCall::EnsureRunning)));
    }
}
