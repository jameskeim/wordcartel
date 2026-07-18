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

// ---------------------------------------------------------------------------
// FaultFs — the shared fault-injecting `Fs` (promoted from fsx.rs, C5 Task 1).
//
// Lives here, not in fsx.rs's private test mod, because every migrated call site
// (reads, listings, stats) needs to inject faults from its OWN module's tests.
// ---------------------------------------------------------------------------

use crate::fsx::{Fs, RealFs, WriteSync};
use std::io::{Error, ErrorKind};
use std::path::Path;

/// Which step of the write sequence fails. Single-fault model: exactly one step is
/// injected per `FaultFs`, so cleanup paths still run for real.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FaultAt {
    Create,
    Write { after: usize },
    SetMode,
    Flush,
    Sync,
    Rename,
    SyncDir,
    // Not yet constructed by any test as of C5 Task 1 — a later task's remove_file
    // fault-injection test (a non-fsx module) is the first consumer. Silence the
    // dead-code lint for this deliberate forward reference rather than dropping the
    // variant the interface contract requires.
    #[allow(dead_code)]
    RemoveFile,
}

pub(crate) struct FaultFs {
    pub(crate) inner: RealFs,
    pub(crate) fail: FaultAt,
}

impl FaultFs {
    pub(crate) fn new(fail: FaultAt) -> Self {
        FaultFs { inner: RealFs, fail }
    }
}

/// A write handle that may inject a partial-write or a set_mode/flush/sync failure.
/// Owns its injected config by value (the boxed handle is `'static`, so it cannot
/// borrow from the FaultFs).
pub(crate) struct FaultHandle {
    inner: Box<dyn WriteSync>,
    fail: FaultAt,
}

impl WriteSync for FaultHandle {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        if let FaultAt::Write { after } = self.fail {
            let n = after.min(buf.len());
            self.inner.write_all(&buf[..n])?;
            return Err(Error::new(ErrorKind::WriteZero, "injected: storage full"));
        }
        self.inner.write_all(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if matches!(self.fail, FaultAt::Flush) {
            return Err(Error::other("injected: flush"));
        }
        self.inner.flush()
    }
    fn set_mode(&self, mode: u32) -> std::io::Result<()> {
        if matches!(self.fail, FaultAt::SetMode) {
            return Err(Error::other("injected: set_mode"));
        }
        self.inner.set_mode(mode)
    }
    fn sync_all(&self) -> std::io::Result<()> {
        if matches!(self.fail, FaultAt::Sync) {
            return Err(Error::other("injected: fsync"));
        }
        self.inner.sync_all()
    }
}

impl Fs for FaultFs {
    fn create_excl(&self, path: &Path, mode: u32) -> std::io::Result<Box<dyn WriteSync>> {
        if matches!(self.fail, FaultAt::Create) {
            return Err(Error::other("injected: create"));
        }
        let inner = self.inner.create_excl(path, mode)?;
        Ok(Box::new(FaultHandle { inner, fail: self.fail }))
    }
    fn existing_mode(&self, path: &Path) -> Option<u32> { self.inner.existing_mode(path) }
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        if matches!(self.fail, FaultAt::Rename) {
            return Err(Error::other("injected: rename"));
        }
        self.inner.rename(from, to)
    }
    fn sync_dir(&self, dir: &Path) -> std::io::Result<()> {
        if matches!(self.fail, FaultAt::SyncDir) {
            return Err(Error::other("injected: sync_dir"));
        }
        self.inner.sync_dir(dir)
    }
    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        if matches!(self.fail, FaultAt::RemoveFile) {
            return Err(Error::other("injected: remove_file"));
        }
        self.inner.remove_file(path)
    }
}
