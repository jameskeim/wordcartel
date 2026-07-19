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

/// A throwaway owned `Fs` handle for test call sites that must supply one but exercise no
/// fault behavior — the plain `RealFs` case. Kept as ONE helper (rather than a
/// `std::sync::Arc::new(crate::fsx::RealFs)` literal repeated at every call site) so the many
/// `reduce`/`pump`/`resolve_prompt`/… test callers threaded in C5 Task 5 stay one short call.
pub(crate) fn test_fs() -> std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> {
    std::sync::Arc::new(crate::fsx::RealFs)
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
    ReadCapped,
    Stat,
    ListDir,
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
    fn read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
        if matches!(self.fail, FaultAt::ReadCapped) {
            return Err(Error::other("injected: read_capped"));
        }
        self.inner.read_capped(path, limit)
    }
    fn stat(&self, path: &std::path::Path) -> std::io::Result<crate::fsx::FileStat> {
        if matches!(self.fail, FaultAt::Stat) {
            return Err(Error::other("injected: stat"));
        }
        self.inner.stat(path)
    }
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
    fn list_dir(&self, path: &Path, cap: Option<usize>)
        -> std::io::Result<crate::fsx::DirListing>
    {
        if matches!(self.fail, FaultAt::ListDir) {
            return Err(Error::other("injected: list_dir"));
        }
        self.inner.list_dir(path, cap)
    }
}

// ---------------------------------------------------------------------------
// File-browser keystroke helpers (C5 Task 12) — reused by Tasks 13-26.
//
// Every test claiming a keystroke reaches something must drive the real entry point; a
// direct call to the handler is the vacuous-guard pattern this plan has had to correct
// repeatedly. These live here rather than in one task's `mod tests` for two reasons:
// helpers inside a `#[cfg(test)] mod tests` are not reachable from another module's tests
// at all, and `test_support` is already the crate's sanctioned home for exactly this
// (`press`, `key_char`, `install_enabled_harper`).
//
// They route through `crate::file_browser::intercept`, the intercept's home as of this
// task. Task 18 moves the intercept into `file_browser_intercept.rs` and updates this ONE
// call path — noted there as an explicit step, since every keystroke test in the effort
// depends on it.
// ---------------------------------------------------------------------------

/// Drive ANY key through the real intercept, exactly as `reduce` would. `press_char_fb`
/// and `press_enter_fb` are thin wrappers; tests needing Tab or Esc use this directly.
pub(crate) fn press_key_fb(e: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    tx: &std::sync::mpsc::Sender<crate::app::Msg>, code: crossterm::event::KeyCode)
{
    use crossterm::event::{Event, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let reg = crate::registry::Registry::builtins();
    let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
    let ex = crate::jobs::InlineExecutor::default();
    let clk = crate::test_support::TestClock(0);
    let ctx = crate::overlays::DispatchCtx {
        reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: tx, fs };
    let ev = Event::Key(KeyEvent { code, modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press, state: KeyEventState::NONE });
    let _ = crate::file_browser_intercept::intercept(crate::app::Msg::Input(ev), e, &ctx);
}

/// One printable character through the intercept.
pub(crate) fn press_char_fb(e: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    tx: &std::sync::mpsc::Sender<crate::app::Msg>, c: char)
{ press_key_fb(e, fs, tx, crossterm::event::KeyCode::Char(c)); }

/// Enter through the intercept.
pub(crate) fn press_enter_fb(e: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    tx: &std::sync::mpsc::Sender<crate::app::Msg>)
{ press_key_fb(e, fs, tx, crossterm::event::KeyCode::Enter); }

/// True when the process can read a mode-000 directory (root / CAP_DAC_OVERRIDE), which
/// voids the premise of any chmod-based unreadability test. Tests that would otherwise
/// assert a false negative skip loudly on this rather than passing vacuously.
pub(crate) fn nix_privileged() -> bool {
    let d = std::env::temp_dir().join(format!("wc-priv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    if std::fs::create_dir_all(&d).is_err() { return false; }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o000));
        let readable = std::fs::read_dir(&d).is_ok();
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&d);
        return readable;
    }
    #[allow(unreachable_code)] { let _ = std::fs::remove_dir_all(&d); false }
}

// ---------------------------------------------------------------------------
// Task 13: the file-browser listing now runs off-thread behind a process-global
// epoch (`file_browser::start_listing` / `apply_listing_done`). Every test that opens
// a picker and wants to read its entries must pump the `Msg::ListingDone` the spawned
// thread sends — these two helpers are shared across every module with a file-browser
// test (`file_browser`, `mouse`, `render`, `app`, `session_restore`, `overlays`).
// ---------------------------------------------------------------------------

/// Deliver one pending `Msg::ListingDone` from the channel into the editor. The listing
/// runs on its own thread, so a test that drives Enter/open must pump the result to
/// observe the outcome. Bounded wait — never hangs a test run.
pub(crate) fn pump_listing(e: &mut crate::editor::Editor,
    rx: &std::sync::mpsc::Receiver<crate::app::Msg>) -> bool
{
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(crate::app::Msg::ListingDone { epoch, dir, result }) => {
            crate::file_browser::apply_listing_done(e, epoch, dir, result);
            true
        }
        _ => false,
    }
}

/// Open the file browser via the real async path (`Editor::open_file_browser`) and pump
/// its initial listing so `fb.entries` is populated before the caller's first assertion.
/// Returns the receiver so the caller can pump further listings (e.g. a subsequent
/// descend) on the SAME channel.
pub(crate) fn open_and_pump(e: &mut crate::editor::Editor, dir: std::path::PathBuf)
    -> std::sync::mpsc::Receiver<crate::app::Msg>
{
    let (tx, rx) = std::sync::mpsc::channel();
    let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> = std::sync::Arc::new(crate::fsx::RealFs);
    e.open_file_browser(&fs, &tx, dir);
    pump_listing(e, &rx);
    rx
}
