//! Shared `#[cfg(test)]` helpers for the shell's test modules (`app::tests`, `e2e`).
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use wordcartel_core::history::Clock;
use crate::app::Msg;

/// Deterministic virtual clock: `now_ms()` returns a fixed value.
pub(crate) struct TestClock(pub(crate) u64);
impl TestClock {
    pub(crate) fn new(ms: u64) -> Self { TestClock(ms) }
}
impl Clock for TestClock {
    fn now_ms(&self) -> u64 { self.0 }
}

/// A `KeyEvent` for a printable character (no modifiers, Press).
pub(crate) fn key_char(c: char) -> KeyEvent {
    KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }
}

/// A `Msg::Input` key press with explicit code + modifiers. NOTE: `press` already
/// returns `Msg` — the harness sugar passes it straight to `step`; never wrap it as
/// `Msg::Input(press(...))`.
pub(crate) fn press(code: KeyCode, mods: KeyModifiers) -> Msg {
    Msg::Input(Event::Key(KeyEvent { code, modifiers: mods, kind: KeyEventKind::Press, state: KeyEventState::NONE }))
}

/// Install an ENABLED Harper `RecordingProvider` into a bare test editor so
/// `apply_diagnostics_done`'s `is_enabled(Harper)` guard (and `arm_enabled`'s enabled-only
/// arming) accept Harper results/arms. `Editor::new_from_text` builds an EMPTY `ProviderSet`;
/// production seeds harper via `install_core_providers` before the run loop — this mirrors that
/// for unit tests. Call it once per test editor, ONLY where the test installs no Harper provider
/// of its own — a second Harper install trips `ProviderSet::install`'s duplicate-source assert.
pub(crate) fn install_enabled_harper(e: &mut crate::editor::Editor) {
    e.diag_providers.install(
        Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(wordcartel_core::diagnostics::DiagSource::Harper)),
        true);
}
