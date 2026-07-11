//! The `DiagnosticsProvider` seam (Effort A): a thin, mockable trait behind which a diagnostics
//! backend runs. `NullProvider` is the hermetic default; `HarperLs` (harper_ls.rs) is the real one.
//! No merge/multi-provider machinery — harper is the only provider; the seam is Open-Closed
//! insurance for provider #2.
use crate::editor::{BufferId, Editor};
use wordcartel_core::history::Clock;

/// Status hint shown when no checker is available (spec §9).
pub const INSTALL_HINT: &str =
    "grammar checker unavailable — install harper-ls (Arch: pacman -S harper)";

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
    fn name(&self) -> &'static str;
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

/// Hermetic default (production): no thread, no process, no emissions.
#[derive(Debug, Default)]
pub struct NullProvider;
impl DiagnosticsProvider for NullProvider {
    fn name(&self) -> &'static str { "none" }
    fn availability(&self) -> Availability { Availability::Idle }
    fn ensure_running(&mut self) {}
    fn configure(&mut self, _cfg: ProviderConfig) {}
    fn notify_change(&mut self, _b: BufferId, _v: u64, _p: Option<std::path::PathBuf>, _t: String)
        -> Accepted { Accepted::No }
    fn notify_close(&mut self, _b: BufferId) {}
    fn reload_dictionary(&mut self) {}
    fn shutdown(&mut self) {}
}

/// Thin reduce/prompts delegation (spec §2). `clock` is needed for the Restarted re-arm.
pub fn apply_provider_event(editor: &mut Editor, ev: ProviderEvent, clock: &dyn Clock) {
    match ev {
        ProviderEvent::Restarted => {
            editor.status = "grammar checker restarted".into();
            if crate::diagnostics_run::should_run_diagnostics(editor) {
                let now = clock.now_ms();
                let debounce = editor.diag_cfg.debounce_ms;
                editor.active_mut().diagnostics.arm(now, debounce);
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
#[derive(Debug)]
pub(crate) struct RecordingProvider {
    calls: Vec<ProviderCall>,
    accepted: Accepted,
    availability: Availability,
}

#[cfg(test)]
impl RecordingProvider {
    /// New recorder: `notify_change` accepts, `availability()` reports `Ready`.
    pub(crate) fn new() -> Self {
        RecordingProvider { calls: Vec::new(), accepted: Accepted::Yes, availability: Availability::Ready }
    }
    pub(crate) fn with_accepted(mut self, accepted: Accepted) -> Self { self.accepted = accepted; self }
    pub(crate) fn with_availability(mut self, availability: Availability) -> Self {
        self.availability = availability; self
    }
    pub(crate) fn calls(&self) -> &[ProviderCall] { &self.calls }
}

#[cfg(test)]
impl DiagnosticsProvider for RecordingProvider {
    fn name(&self) -> &'static str { "recording" }
    fn availability(&self) -> Availability { self.availability }
    fn ensure_running(&mut self) { self.calls.push(ProviderCall::EnsureRunning); }
    fn configure(&mut self, cfg: ProviderConfig) { self.calls.push(ProviderCall::Configure(cfg)); }
    fn notify_change(&mut self, buffer_id: BufferId, version: u64,
        path: Option<std::path::PathBuf>, text: String) -> Accepted {
        self.calls.push(ProviderCall::NotifyChange { buffer_id, version, path, text });
        self.accepted
    }
    fn notify_close(&mut self, buffer_id: BufferId) { self.calls.push(ProviderCall::NotifyClose(buffer_id)); }
    fn reload_dictionary(&mut self) { self.calls.push(ProviderCall::ReloadDictionary); }
    fn shutdown(&mut self) { self.calls.push(ProviderCall::Shutdown); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Editor, RenderMode};
    use crate::test_support::TestClock;

    // ------------------------------------------------------------------
    // NullProvider: hermetic default, never accepts, never emits.
    // ------------------------------------------------------------------

    #[test]
    fn null_provider_is_idle_and_never_accepts() {
        let mut p = NullProvider;
        assert_eq!(p.name(), "none");
        assert_eq!(p.availability(), Availability::Idle);
        p.ensure_running(); // no-op; must not panic
        p.configure(ProviderConfig { grammar: true, dictionary: None, max_file_length: 1 });
        let accepted = p.notify_change(BufferId(0), 1, None, "hi".into());
        assert_eq!(accepted, Accepted::No, "null provider never latches a check");
        p.notify_close(BufferId(0));
        p.reload_dictionary();
        p.shutdown();
    }

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

        assert_eq!(p.calls(), &[
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
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::Review;
        apply_provider_event(&mut e, ProviderEvent::Restarted, &TestClock::new(1000));
        assert_eq!(e.status, "grammar checker restarted");
        assert_eq!(e.active().diagnostics.recheck_due_at, Some(1000 + e.diag_cfg.debounce_ms));
    }

    #[test]
    fn restarted_sets_status_but_does_not_arm_outside_review() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::LivePreview;
        apply_provider_event(&mut e, ProviderEvent::Restarted, &TestClock::new(1000));
        assert_eq!(e.status, "grammar checker restarted");
        assert_eq!(e.active().diagnostics.recheck_due_at, None, "not Review: no arm");
    }

    #[test]
    fn restarted_sets_status_but_does_not_arm_when_disabled() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        e.diag_cfg.enabled = false;
        e.active_mut().view.mode = RenderMode::Review;
        apply_provider_event(&mut e, ProviderEvent::Restarted, &TestClock::new(1000));
        assert_eq!(e.status, "grammar checker restarted");
        assert_eq!(e.active().diagnostics.recheck_due_at, None, "disabled: no arm");
    }

    #[test]
    fn degraded_sets_status_to_the_hint_verbatim() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        apply_provider_event(&mut e, ProviderEvent::Degraded(INSTALL_HINT.into()), &TestClock::new(0));
        assert_eq!(e.status, INSTALL_HINT);
    }
}
