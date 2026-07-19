//! Fault-injectable atomic-write commit core (M3). The single durability-critical
//! sequence shared by file::save_atomic / save_atomic_bytes / swap::write_atomic.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Seam traits
// ---------------------------------------------------------------------------

/// A writable+syncable temp handle. Owns the underlying file; `set_mode` applies
/// the final permission bits on Unix (no-op elsewhere).
pub trait WriteSync {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
    fn set_mode(&self, mode: u32) -> std::io::Result<()>;
    fn sync_all(&self) -> std::io::Result<()>;
}

/// A resolved metadata probe. `len`/`mtime`/`is_file`/`is_dir` FOLLOW symlinks (they come
/// from `metadata`); `is_symlink` does NOT (it comes from `symlink_metadata`). Two syscalls,
/// one method — both existing behaviours preserved exactly.
///
/// `is_file` is a field and NEVER `!is_dir`: fifos, sockets, and devices are neither, so the
/// equivalence is false and `config_layer_paths`-style probes would misclassify them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // C5 tasks 3-24 use this via implementors; forward reference
pub struct FileStat {
    pub len: u64,
    pub mtime: Option<std::time::SystemTime>,
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    /// Symlink whose target could not be RESOLVED — dangling, permission-denied along the
    /// chain, or a resolution loop. NOT "the target is gone": `metadata` reports all three
    /// as Err and this seam does not distinguish them, so user-facing wording must say
    /// "cannot be resolved" rather than asserting absence.
    pub broken: bool,
}

/// What a directory entry resolved to. An ENUM, not a pair of bools, so `Unknown` cannot be
/// silently absorbed into a false branch — the house rule on exhaustive matches applied to the
/// failure mode this design kept hitting. Critically, `Other` (a legitimately-classified fifo)
/// and `Unknown` (we could not classify it) are DIFFERENT facts that two bools cannot separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// RESOLVED regular file (follows symlinks).
    File,
    /// RESOLVED directory (follows symlinks).
    Dir,
    /// RESOLVED to something that is neither — fifo, socket, block/char device.
    Other,
    /// NOT classified: either the `file_type()` probe itself failed, or this is a symlink
    /// whose target could not be resolved (`broken`). We have a name but no type.
    Unknown,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // C5 tasks 5-24 use this via implementors; forward reference
pub struct DirEntryInfo {
    /// LOSSY-rendered (`to_string_lossy`). A name that is not valid UTF-8 arrives here with
    /// replacement characters, which is fine for display and for the `.lua` suffix test, but
    /// means a caller CANNOT recover the original bytes from this field.
    pub name: String,
    /// The raw, unconverted name. Carried because `plugin::load::discover` must distinguish
    /// "a plugin whose name is not valid UTF-8" (reported by name, lossily) from an ordinary
    /// file — a distinction `name` alone destroys, since the lossy conversion always succeeds.
    /// Every other consumer uses `name`.
    pub raw_name: std::ffi::OsString,
    pub kind: EntryKind,
    /// True when the entry itself is a symlink, whatever it points at.
    pub is_symlink: bool,
    /// Symlink whose target could not be RESOLVED. Same meaning as `FileStat::broken`.
    /// INVARIANT: `broken` implies `is_symlink` and `kind == Unknown`.
    pub broken: bool,
}

/// The result of one directory listing.
///
/// `total_seen` counts EVERY entry the iterator yielded, Ok or Err.
/// `unreadable` counts entries that could not even be NAMED (the iterator itself yielded Err).
/// It is NOT "entries we could not classify" — a named entry whose TYPE probe failed is a
/// perfectly good row with `kind == Unknown` and lives in `entries`, because a name is more
/// useful than a tally and `plugin::load::discover` needs it to test "plausibly a plugin".
///
/// INVARIANT: `total_seen == entries.len() + unreadable + capped_out`.
#[derive(Clone, Debug)]
#[allow(dead_code)] // C5 tasks 5-24 use this via implementors; forward reference
pub struct DirListing {
    pub entries: Vec<DirEntryInfo>,
    pub total_seen: usize,
    pub unreadable: usize,
}

/// The filesystem ops the atomic-write commit needs. Object-safe (no generics,
/// no associated types) so `&dyn Fs` works.
///
/// `pub`, not `pub(crate)` — Task 5 threads `dyn Fs` into `Ctx`/`DispatchCtx` and several `pub`
/// functions (`app::reduce`, `mouse::handle`, `plugin::pump::PluginHost::pump`, …), exactly like
/// its sibling seam traits `wordcartel_core::history::Clock` and `jobs::Executor` already are —
/// a `pub(crate)` trait behind a `pub` item is a `private_interfaces` warning (a build-clean GATE).
pub trait Fs {
    /// O_EXCL create at `path` with `mode` (Unix); returns a write+sync handle.
    fn create_excl(&self, path: &Path, mode: u32) -> std::io::Result<Box<dyn WriteSync>>;
    /// Best-effort mode of an existing file (Unix); `None` if absent/unreadable/off-unix.
    fn existing_mode(&self, path: &Path) -> Option<u32>;
    /// Read at most `limit + 1` bytes from `path`. `Ok(None)` when the file exceeds
    /// `limit`; `Err` on any IO failure. The Option/Result split is deliberate: an
    /// over-cap file is a POLICY outcome, an unreadable file is a FAILURE, and callers
    /// that conflate them (today's `bounded_read_opt`) cannot tell a huge document from
    /// a permission problem.
    #[allow(dead_code)] // C5 tasks 3-24 use this via implementors; forward reference
    fn read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>>;
    /// Metadata probe. See [`FileStat`] for the follow/don't-follow split.
    #[allow(dead_code)] // C5 tasks 3-24 use this via implementors; forward reference
    fn stat(&self, path: &Path) -> std::io::Result<FileStat>;
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    /// Durably flush a directory entry. A dir that cannot be opened is NOT an error.
    fn sync_dir(&self, dir: &Path) -> std::io::Result<()>;
    fn remove_file(&self, path: &Path) -> std::io::Result<()>;
    /// Enumerate `path`. Enumeration is ALWAYS complete; only RETENTION is capped, and only
    /// when `cap` is `Some`. `cap: None` is the non-interactive form (plugin discovery, the
    /// swap scans) — those are uncapped today and capping them would be a refactor-introduced
    /// regression, not a new protection.
    #[allow(dead_code)] // C5 tasks 5-24 use this via implementors; forward reference
    fn list_dir(&self, path: &Path, cap: Option<usize>) -> std::io::Result<DirListing>;
}

// ---------------------------------------------------------------------------
// RealFs — production delegate (zero-size, Send)
// ---------------------------------------------------------------------------

pub(crate) struct RealFs;

struct RealHandle(fs::File);

impl WriteSync for RealHandle {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.write_all(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
    fn set_mode(&self, _mode: u32) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            self.0.set_permissions(fs::Permissions::from_mode(_mode))?;
        }
        Ok(())
    }
    fn sync_all(&self) -> std::io::Result<()> {
        self.0.sync_all()
    }
}

impl Fs for RealFs {
    fn create_excl(&self, path: &Path, mode: u32) -> std::io::Result<Box<dyn WriteSync>> {
        let f = open_excl(path, mode)?;
        Ok(Box::new(RealHandle(f)))
    }
    fn existing_mode(&self, path: &Path) -> Option<u32> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(path).ok().map(|m| m.permissions().mode())
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            None
        }
    }
    fn read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
        use std::io::Read as _;
        let f = fs::File::open(path)?;
        let mut buf = Vec::new();
        f.take(limit + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > limit { return Ok(None); }
        Ok(Some(buf))
    }
    fn stat(&self, path: &Path) -> std::io::Result<FileStat> {
        // symlink_metadata FIRST: it establishes that the entry exists at all, and whether
        // it is a link. A path that does not exist in any form is Err — the ordinary
        // "new file" answer, which must stay distinguishable from a broken link.
        let lm = fs::symlink_metadata(path)?;
        let is_symlink = lm.file_type().is_symlink();
        match fs::metadata(path) {
            Ok(m) => Ok(FileStat {
                len: m.len(),
                mtime: m.modified().ok(),
                is_file: m.is_file(),
                is_dir: m.is_dir(),
                is_symlink,
                broken: false,
            }),
            // A symlink we cannot resolve is `broken` — the link exists, its target is
            // unreachable for SOME reason we deliberately do not distinguish.
            Err(_) if is_symlink => Ok(FileStat {
                len: 0, mtime: None, is_file: false, is_dir: false,
                is_symlink: true, broken: true,
            }),
            // Not a symlink but metadata failed: a genuine IO/permission error on a real
            // entry. `broken` is never used to paper over an unreadable regular file.
            Err(e) => Err(e),
        }
    }
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        fs::rename(from, to)
    }
    fn sync_dir(&self, dir: &Path) -> std::io::Result<()> {
        // Match today's dir-open swallow (file.rs): an un-openable dir is not an error;
        // only a successful-open sync_all failure propagates.
        match fs::File::open(dir) {
            Ok(fh) => fh.sync_all(),
            Err(_) => Ok(()),
        }
    }
    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        fs::remove_file(path)
    }
    fn list_dir(&self, path: &Path, cap: Option<usize>) -> std::io::Result<DirListing> {
        let rd = fs::read_dir(path)?;
        let mut entries = Vec::new();
        let mut total_seen = 0usize;
        let mut unreadable = 0usize;
        for item in rd {
            total_seen += 1;
            let Ok(entry) = item else { unreadable += 1; continue };
            // Past the cap we still COUNT (the total must be real) but do no further work:
            // no allocation retained and, critically, no `metadata` call — symlink
            // resolution below runs on retained entries only.
            if cap.is_some_and(|c| entries.len() >= c) { continue; }
            let raw_name = entry.file_name();
            let name = raw_name.to_string_lossy().into_owned();
            // NOTE: no `?` on file_type() — one unclassifiable entry must NOT abort the whole
            // directory. A named-but-unclassified entry is emitted with kind == Unknown.
            let mut stats = 0usize; // observed by the syscall-economy test via classify_entry
            let (kind, is_symlink, broken) = match entry.file_type() {
                Err(_) => (EntryKind::Unknown, false, false),
                Ok(ft) => classify_entry(&entry, ft, &mut stats),
            };
            entries.push(DirEntryInfo { name, raw_name, kind, is_symlink, broken });
        }
        Ok(DirListing { entries, total_seen, unreadable })
    }
}

/// Classify ONE directory entry, recording how many `metadata` calls it cost.
///
/// Extracted as a named function — rather than inlined in `list_dir` — specifically so the
/// syscall-economy test can drive it and OBSERVE the stat count. A counter wrapped around
/// `list_dir` cannot see inside `RealFs::list_dir`, so it could not detect a regression to
/// stat-everything; this can.
///
/// `stats` is incremented once per `metadata` call, which happens ONLY for symlinks.
fn classify_entry(entry: &fs::DirEntry, ft: fs::FileType, stats: &mut usize)
    -> (EntryKind, bool, bool)
{
    if !ft.is_symlink() {
        return (kind_of(ft.is_file(), ft.is_dir()), false, false);
    }
    *stats += 1;
    match fs::metadata(entry.path()) {
        Ok(m) => (kind_of(m.is_file(), m.is_dir()), true, false),
        Err(_) => (EntryKind::Unknown, true, true),
    }
}

/// Map a resolved (is_file, is_dir) pair onto an `EntryKind`. Neither true means `Other`
/// — a fifo, socket, or device — which is a CLASSIFIED answer, not an unknown one.
fn kind_of(is_file: bool, is_dir: bool) -> EntryKind {
    if is_file { EntryKind::File } else if is_dir { EntryKind::Dir } else { EntryKind::Other }
}

/// `Path::exists()` through the seam. `Path::exists()` FOLLOWS symlinks, so a broken link
/// answers `false` — and `stat` reports such a link as `Ok(broken: true)`. Both facts are
/// reconciled here in ONE place so no call site re-derives them.
pub(crate) fn exists_via(fs: &dyn Fs, path: &Path) -> bool {
    matches!(fs.stat(path), Ok(st) if !st.broken)
}

/// `Path::is_file()` through the seam — a RESOLVED regular file. Returns `false` on any
/// error, which is exactly what `Path::is_file()` does today at every migrated site
/// (it swallows the error). NEVER `!is_dir`: fifos, sockets, and devices are neither.
pub(crate) fn is_file_via(fs: &dyn Fs, path: &Path) -> bool {
    matches!(fs.stat(path), Ok(st) if st.is_file)
}

/// A WRITE destination the caller asked to save through cannot be honoured.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DestError {
    /// The destination is a symlink whose target cannot be resolved. Refused BEFORE any
    /// write is dispatched — it must never reach `atomic_replace`, and must never surface
    /// as `SaveError::Symlink`, which names a mechanism rather than the problem.
    BrokenSymlink,
}

/// Resolve a WRITE destination through symlinks (spec §7.6.1).
///
/// `file::save_atomic` refuses to write through a symlink — correctly, because
/// `atomic_replace` renames a temp over the target and would replace the LINK with a
/// regular file, destroying it. That refusal stays an unconditional last-resort guard;
/// resolution happens here, BEFORE a path ever reaches it, which is why
/// `file::tests::save_through_symlink_refused` continues to pass unmodified.
///
/// Applied at all four write-destination boundaries — Save, Save-As, Write-Block, and the
/// Export destination — so a writer who navigates through symlinks cannot pick a
/// destination that fails at the end of a save they thought would work.
///
/// * not a symlink         -> unchanged
/// * symlink that resolves -> the resolved target (the link is preserved, because
///   `atomic_replace` then renames over the TARGET)
/// * broken symlink        -> `Err(DestError::BrokenSymlink)`
/// * does not exist yet    -> unchanged (the ordinary new-file case)
pub(crate) fn resolve_write_destination(fs: &dyn Fs, path: &Path)
    -> Result<PathBuf, DestError>
{
    match fs.stat(path) {
        // Broken link: refuse now, with a reason a writer can act on.
        Ok(st) if st.broken => Err(DestError::BrokenSymlink),
        // Resolvable link: write to the target, so the link survives the rename.
        Ok(st) if st.is_symlink => match std::fs::canonicalize(path) {
            Ok(target) => Ok(target),
            // `stat` said it resolves but canonicalize disagrees — a race. Fail closed.
            Err(_) => Err(DestError::BrokenSymlink),
        },
        // Ordinary existing file, or nothing there yet (Err from `stat`): unchanged.
        _ => Ok(path.to_path_buf()),
    }
}

// Platform-specific O_EXCL open with `mode` on Unix; plain create_new elsewhere.
#[cfg(unix)]
fn open_excl(path: &Path, mode: u32) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
}

#[cfg(not(unix))]
fn open_excl(path: &Path, _mode: u32) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

// ---------------------------------------------------------------------------
// Temp-name generation + RAII cleanup
// ---------------------------------------------------------------------------

/// Monotonic counter: with pid produces a process-unique + call-unique temp name.
static TEMP_SEQ: AtomicU32 = AtomicU32::new(0);

/// Removes a temp via the seam on drop unless disarmed.
struct TempGuard<'a> {
    fs: &'a dyn Fs,
    path: Option<PathBuf>,
}
impl TempGuard<'_> {
    fn disarm(&mut self) {
        self.path = None;
    }
}
impl Drop for TempGuard<'_> {
    fn drop(&mut self) {
        if let Some(p) = &self.path {
            let _ = self.fs.remove_file(p);
        }
    }
}

/// Create an O_EXCL 0600 temp in `dir`, retrying on name collision.
fn create_temp(
    fs: &dyn Fs,
    dir: &Path,
    name: &str,
) -> std::io::Result<(Box<dyn WriteSync>, PathBuf)> {
    let pid = std::process::id();
    let mut counter = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    loop {
        let temp = dir.join(format!(".{name}.wcartel-{pid}-{counter}.tmp"));
        match fs.create_excl(&temp, 0o600) {
            Ok(handle) => return Ok((handle, temp)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                counter = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => return Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// ModePolicy / WriteOpts / atomic_replace
// ---------------------------------------------------------------------------

/// How the temp's final mode is chosen before rename.
pub(crate) enum ModePolicy {
    /// Always this mode (save_atomic_bytes, swap::write_atomic).
    Fixed(u32),
    /// Preserve the existing target's mode if present, else this fallback (save_atomic).
    PreserveExistingOr(u32),
}

pub(crate) struct WriteOpts {
    pub mode: ModePolicy,
    pub dir_fsync: bool,
}

/// The single durability-critical sequence:
/// [resolve PreserveExistingOr mode-read] -> create-temp(O_EXCL,0600) -> write_all ->
/// set_mode -> flush -> fsync -> close -> rename -> [dir-fsync]. The temp is removed on ANY
/// failure before rename; the target is never half-replaced (rename is the commit).
pub(crate) fn atomic_replace(
    fs: &dyn Fs,
    final_path: &Path,
    bytes: &[u8],
    opts: WriteOpts,
) -> std::io::Result<()> {
    // Resolve the final mode BEFORE temp creation (matches save_atomic's capture order).
    let final_mode = match opts.mode {
        ModePolicy::Fixed(m) => m,
        ModePolicy::PreserveExistingOr(fallback) => {
            fs.existing_mode(final_path).unwrap_or(fallback)
        }
    };

    let dir = final_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let (handle, temp) = create_temp(fs, &dir, &name)?;
    let mut guard = TempGuard {
        fs,
        path: Some(temp.clone()),
    };

    // write_and_sync CONSUMES the handle, so the temp file is closed at its frame exit on
    // EVERY path — success or an early `?`-failure. That ordering matters: TempGuard::drop
    // (which fires on a pre-rename failure, since `guard` is still armed) must unlink a
    // CLOSED file, not an open one. Unlinking an open file works on Unix but fails on
    // Windows, so closing first keeps cleanup correct on every path, not just before rename.
    write_and_sync(handle, bytes, final_mode)?;

    fs.rename(&temp, final_path)?;
    guard.disarm(); // temp renamed away; nothing to clean up regardless of what follows

    if opts.dir_fsync {
        fs.sync_dir(&dir)?;
    }
    Ok(())
}

/// Write the content, widen the mode, flush, and fsync — consuming the handle so it is
/// closed at return on every path. The temp is created 0600 (create_temp) and only widened
/// to `mode` AFTER the content is written, so the bytes are never momentarily group-/
/// world-readable during the write. Do NOT reorder set_mode before write_all — that would
/// open a readable window on the partially-written temp.
fn write_and_sync(mut handle: Box<dyn WriteSync>, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    handle.write_all(bytes)?;
    handle.set_mode(mode)?;
    handle.flush()?;
    handle.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    // A private per-test dir under the system temp dir, so a glob for ".tmp" is
    // not polluted by other processes' files.
    fn private_dir(label: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!(
            "wcartel-fsx-{}-{}-{}",
            std::process::id(),
            n,
            label
        ));
        fs::create_dir_all(&d).expect("create private dir");
        d
    }

    fn tmp_litter(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .expect("read_dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().ends_with(".tmp"))
                    .unwrap_or(false)
            })
            .collect()
    }

    /// atomic_replace via RealFs replaces the target and leaves no temp.
    #[test]
    fn real_fs_replaces_target_no_litter() {
        let dir = private_dir("replace");
        let target = dir.join("doc.txt");
        fs::write(&target, b"original\n").expect("seed original");
        atomic_replace(
            &RealFs,
            &target,
            b"new bytes\n",
            WriteOpts {
                mode: ModePolicy::Fixed(0o600),
                dir_fsync: true,
            },
        )
        .expect("atomic_replace must succeed");
        assert_eq!(fs::read(&target).expect("read target"), b"new bytes\n");
        assert!(
            tmp_litter(&dir).is_empty(),
            "no temp may remain: {:?}",
            tmp_litter(&dir)
        );
    }

    /// PreserveExistingOr keeps a non-default existing mode; absent target lands on the fallback.
    #[cfg(unix)]
    #[test]
    fn preserve_existing_mode_else_fallback() {
        use std::os::unix::fs::PermissionsExt;
        let dir = private_dir("mode");

        // Existing 0644 target -> mode preserved.
        let existing = dir.join("keep.txt");
        fs::write(&existing, b"x\n").expect("seed");
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o644)).expect("chmod 644");
        atomic_replace(
            &RealFs,
            &existing,
            b"y\n",
            WriteOpts {
                mode: ModePolicy::PreserveExistingOr(0o600),
                dir_fsync: true,
            },
        )
        .expect("replace existing");
        let m = fs::metadata(&existing).expect("meta").permissions().mode() & 0o777;
        assert_eq!(m, 0o644, "existing mode must be preserved");

        // Absent target -> fallback 0600.
        let fresh = dir.join("fresh.txt");
        atomic_replace(
            &RealFs,
            &fresh,
            b"z\n",
            WriteOpts {
                mode: ModePolicy::PreserveExistingOr(0o600),
                dir_fsync: true,
            },
        )
        .expect("create fresh");
        let m = fs::metadata(&fresh).expect("meta").permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "absent target must land on the fallback mode");
    }

    /// RealFs::sync_dir swallows an un-openable directory (Err(_) => Ok(())) — pins the
    /// PRODUCTION side of the dir-fsync semantic (the FaultFs::SyncDir test only covers the
    /// Err-propagation path). An existing dir syncs Ok; a missing dir is NOT an error.
    #[test]
    fn real_fs_sync_dir_swallows_unopenable() {
        let dir = private_dir("syncdir");
        RealFs.sync_dir(&dir).expect("existing dir must sync Ok");
        let missing = dir.join("does-not-exist");
        assert!(RealFs.sync_dir(&missing).is_ok(), "un-openable dir must be swallowed to Ok");
    }

    #[test]
    fn fault_fs_is_reachable_from_test_support() {
        // The promotion guard: FaultFs must live in test_support so other modules' tests can
        // inject it. A rename/move back into this file's private test mod breaks this line.
        let fs = crate::test_support::FaultFs::new(crate::test_support::FaultAt::Rename);
        let dir = std::env::temp_dir().join(format!("wc-faultfs-promo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create dir");
        let target = dir.join("t.txt");
        let err = atomic_replace(&fs, &target, b"x", WriteOpts {
            mode: ModePolicy::Fixed(0o600), dir_fsync: false,
        }).expect_err("injected rename must fail");
        assert!(err.to_string().contains("injected: rename"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------------------------------------------------------------------------
    // FaultFs harness — fault-injectable Fs/WriteSync for durability tests
    // (promoted to `test_support`, C5 Task 1, so other modules can inject faults too).
    // ---------------------------------------------------------------------------

    use crate::test_support::{FaultAt, FaultFs};

    // Helper: run atomic_replace with one injected fault over a freshly-seeded target.
    fn run_fault(
        label: &str,
        fail: FaultAt,
        opts: WriteOpts,
    ) -> (PathBuf, PathBuf, std::io::Result<()>) {
        let dir = private_dir(label);
        let target = dir.join("doc.txt");
        fs::write(&target, b"ORIGINAL-CONTENTS\n").expect("seed original");
        let fs_impl = FaultFs::new(fail);
        let res = atomic_replace(&fs_impl, &target, b"NEW-CONTENTS-LONGER\n", opts);
        (dir, target, res)
    }

    fn opts_preserve() -> WriteOpts {
        WriteOpts { mode: ModePolicy::PreserveExistingOr(0o600), dir_fsync: true }
    }

    // ---------------------------------------------------------------------------
    // Durability fault tests
    // ---------------------------------------------------------------------------

    /// Every pre-rename fault: original byte-identical, no litter, Err surfaced.
    #[test]
    fn pre_rename_faults_preserve_original_and_leave_no_litter() {
        for fail in [
            FaultAt::Create,
            FaultAt::Write { after: 4 },
            FaultAt::SetMode,
            FaultAt::Flush,
            FaultAt::Sync,
            FaultAt::Rename,
        ] {
            let (dir, target, res) = run_fault("pre-rename", fail, opts_preserve());
            assert!(res.is_err(), "fault {fail:?} must surface an Err");
            assert_eq!(
                fs::read(&target).expect("read target"),
                b"ORIGINAL-CONTENTS\n",
                "fault {fail:?} must leave the original byte-identical"
            );
            assert!(
                tmp_litter(&dir).is_empty(),
                "fault {fail:?} left temp litter: {:?}",
                tmp_litter(&dir)
            );
        }
    }

    /// A partial write (ENOSPC after k bytes): caught, original intact, no litter.
    #[test]
    fn partial_write_caught_original_intact() {
        let (dir, target, res) =
            run_fault("partial", FaultAt::Write { after: 6 }, opts_preserve());
        let err = res.expect_err("partial write must Err");
        assert_eq!(err.kind(), std::io::ErrorKind::WriteZero);
        assert_eq!(fs::read(&target).expect("read"), b"ORIGINAL-CONTENTS\n");
        assert!(tmp_litter(&dir).is_empty(), "no litter after partial write");
    }

    /// dir-fsync (pinned): SyncDir failure returns Err even though the rename already
    /// committed and the target holds the NEW bytes.
    #[test]
    fn dir_fsync_failure_is_err_but_target_has_new_bytes() {
        let (dir, target, res) = run_fault("dirsync", FaultAt::SyncDir, opts_preserve());
        assert!(res.is_err(), "SyncDir failure must surface as Err");
        assert_eq!(
            fs::read(&target).expect("read"),
            b"NEW-CONTENTS-LONGER\n",
            "the write committed (rename succeeded); only the durability barrier failed"
        );
        assert!(tmp_litter(&dir).is_empty(), "no litter after committed write");
    }

    // ---------------------------------------------------------------------------
    // read_capped tests
    // ---------------------------------------------------------------------------

    fn unique_dir(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "wc-fsx-{}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed), label));
        std::fs::create_dir_all(&d).expect("create dir");
        d
    }

    #[cfg(unix)]
    #[test]
    fn stat_follows_symlinks_for_size_but_reports_the_link_bit() {
        // Load-bearing: every existing stat caller uses `metadata` (which FOLLOWS).
        // A FileStat built only from symlink_metadata would report the LINK's size to
        // save::fingerprint, silently breaking external-mod detection for symlinked docs.
        let d = unique_dir("stat-follow");
        let real = d.join("real.txt");
        let link = d.join("link.txt");
        std::fs::write(&real, b"0123456789").expect("seed");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let s = RealFs.stat(&link).expect("stat");
        assert_eq!(s.len, 10, "len must be the TARGET's, not the link's");
        assert!(s.is_file, "resolves to a regular file");
        assert!(!s.is_dir);
        assert!(s.is_symlink, "but the entry itself is a link");
        assert!(!s.broken);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn stat_broken_symlink_is_broken_not_err_and_missing_is_err() {
        // These two MUST stay distinguishable: `canonicalize` fails identically for both,
        // which is exactly why §7.6.1's broken-destination refusal needs this field.
        let d = unique_dir("stat-broken");
        let link = d.join("dangling.txt");
        std::os::unix::fs::symlink(d.join("does-not-exist"), &link).expect("symlink");

        let s = RealFs.stat(&link).expect("a broken link still stats — it exists as a link");
        assert!(s.broken, "unresolvable target -> broken");
        assert!(s.is_symlink);
        assert!(!s.is_file && !s.is_dir, "broken implies neither");

        let missing = RealFs.stat(&d.join("nothing-at-all.txt"));
        assert!(missing.is_err(), "a path that does not exist at all is Err — the new-file case");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stat_regular_file_and_dir_classify() {
        let d = unique_dir("stat-kinds");
        let f = d.join("f.txt");
        std::fs::write(&f, b"x").expect("seed");
        let sf = RealFs.stat(&f).expect("stat file");
        assert!(sf.is_file && !sf.is_dir && !sf.is_symlink && !sf.broken);
        let sd = RealFs.stat(&d).expect("stat dir");
        assert!(sd.is_dir && !sd.is_file);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stat_fault_is_injectable() {
        let d = unique_dir("stat-fault");
        let f = d.join("f.txt");
        std::fs::write(&f, b"x").expect("seed");
        let fs = crate::test_support::FaultFs::new(crate::test_support::FaultAt::Stat);
        let err = fs.stat(&f).expect_err("injected stat must fail");
        assert!(err.to_string().contains("injected: stat"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn is_file_via_rejects_a_socket_and_a_dir() {
        // `!is_dir` is NOT "regular file". config_layer_paths, plugin discovery, and the
        // clipboard PATH search all ask `is_file()`, and a special file answering `true` would
        // turn "skip it" into a blocking read.
        // MUST create a real filesystem entry that is neither a regular file nor a directory.
        // Without one, `is_file_via` implemented as `!st.is_dir` — the exact defect this test's
        // own comment warns about — passes every assertion (file→true, dir→false,
        // missing→Err→false). That third kind of entry is the only case that separates the two
        // implementations.
        //
        // FAIL-VERIFY: implement `is_file_via` as `matches!(fs.stat(p), Ok(st) if !st.is_dir)`,
        // watch the socket assertion fail.
        let d = unique_dir("isfile-socket");
        let f = d.join("plain.txt");
        std::fs::write(&f, b"x").expect("seed");
        assert!(is_file_via(&RealFs, &f), "regular file");
        assert!(!is_file_via(&RealFs, &d), "a directory is not a file");
        assert!(!is_file_via(&RealFs, &d.join("absent")), "a missing path is false, not an error");
        #[cfg(unix)]
        {
            let sock_path = d.join("sock");
            // A Unix domain socket, like a fifo, is neither a regular file nor a directory —
            // exactly the property under test — but `std::os::unix::net::UnixListener::bind`
            // creates one with zero `unsafe` and zero new dependencies (this effort's C5
            // standing constraint is zero new deps; a fifo would need `libc::mkfifo`, which
            // needs `unsafe` this crate's `#![forbid(unsafe_code)]` rules out, or a `nix` dev-
            // dependency this effort chose not to add). Bind it to a variable and keep it alive
            // for the duration of the assertion below — dropping the listener can unlink the
            // socket file on some platforms, which would turn this into a "missing path" case
            // instead of the "special file" case the test needs.
            let listener = std::os::unix::net::UnixListener::bind(&sock_path)
                .expect("bind must succeed — this IS the guard, not an optional extra");
            assert!(!is_file_via(&RealFs, &sock_path),
                "a socket is NOT a regular file — the one assertion an `!is_dir` implementation \
                 fails, and the reason this test exists");
            drop(listener);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn read_capped_returns_bytes_within_cap() {
        let d = unique_dir("readcap-ok");
        let p = d.join("f.txt");
        std::fs::write(&p, b"hello").expect("seed");
        let got = RealFs.read_capped(&p, 1024).expect("no io error");
        assert_eq!(got.as_deref(), Some(&b"hello"[..]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn read_capped_over_cap_is_ok_none_not_err() {
        // Over-cap must be Ok(None) — a DISTINCT outcome from an IO failure, which is the
        // whole reason this returns Result<Option<_>> rather than Option<_>.
        let d = unique_dir("readcap-over");
        let p = d.join("f.txt");
        std::fs::write(&p, b"0123456789").expect("seed");
        let got = RealFs.read_capped(&p, 4).expect("over-cap is not an IO error");
        assert!(got.is_none(), "over-cap yields Ok(None)");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn read_capped_missing_is_err_not_ok_none() {
        let d = unique_dir("readcap-missing");
        let err = RealFs.read_capped(&d.join("nope.txt"), 1024);
        assert!(err.is_err(), "a missing file is an IO error, not an over-cap None");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn read_capped_fault_is_injectable() {
        let d = unique_dir("readcap-fault");
        let p = d.join("f.txt");
        std::fs::write(&p, b"x").expect("seed");
        let fs = crate::test_support::FaultFs::new(crate::test_support::FaultAt::ReadCapped);
        let err = fs.read_capped(&p, 1024).expect_err("injected read must fail");
        assert!(err.to_string().contains("injected: read_capped"));
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---------------------------------------------------------------------------
    // list_dir tests
    // ---------------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn list_dir_classifies_kinds_and_resolves_symlinks() {
        let d = unique_dir("list-kinds");
        std::fs::write(d.join("a.txt"), b"x").expect("seed file");
        std::fs::create_dir_all(d.join("sub")).expect("seed dir");
        std::os::unix::fs::symlink(d.join("a.txt"), d.join("lf")).expect("link->file");
        std::os::unix::fs::symlink(d.join("sub"), d.join("ld")).expect("link->dir");
        std::os::unix::fs::symlink(d.join("gone"), d.join("lb")).expect("link->nothing");

        let l = RealFs.list_dir(&d, None).expect("list");
        let by = |n: &str| l.entries.iter().find(|e| e.name == n).expect("entry").clone();

        assert_eq!(by("a.txt").kind, EntryKind::File);
        assert_eq!(by("sub").kind, EntryKind::Dir);
        // Resolved through the link — the §4.9 regression.
        assert_eq!(by("lf").kind, EntryKind::File);
        assert!(by("lf").is_symlink);
        assert_eq!(by("ld").kind, EntryKind::Dir);
        assert!(by("ld").is_symlink);
        // Broken: Unknown, not Other. These are different facts.
        assert_eq!(by("lb").kind, EntryKind::Unknown);
        assert!(by("lb").broken && by("lb").is_symlink);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn list_dir_cap_none_retains_everything_and_counts_truthfully() {
        let d = unique_dir("list-uncapped");
        for i in 0..12 { std::fs::write(d.join(format!("f{i}.txt")), b"x").expect("seed"); }
        let l = RealFs.list_dir(&d, None).expect("list");
        assert_eq!(l.entries.len(), 12);
        assert_eq!(l.total_seen, 12);
        assert_eq!(l.unreadable, 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn list_dir_caps_retention_but_not_enumeration() {
        // The count must be REAL: capping enumeration would make "showing N of TOTAL"
        // unknowable, and §7.4's disclosure law requires shown + withheld to account for
        // what is really there.
        let d = unique_dir("list-capped");
        for i in 0..12 { std::fs::write(d.join(format!("f{i:02}.txt")), b"x").expect("seed"); }
        let l = RealFs.list_dir(&d, Some(5)).expect("list");
        assert_eq!(l.entries.len(), 5, "retention capped");
        assert_eq!(l.total_seen, 12, "enumeration NOT capped — the total is real");
        assert_eq!(l.total_seen, l.entries.len() + l.unreadable + 7, "accounting balances");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn list_dir_fault_is_injectable() {
        let d = unique_dir("list-fault");
        let fs = crate::test_support::FaultFs::new(crate::test_support::FaultAt::ListDir);
        let err = fs.list_dir(&d, None).expect_err("injected list must fail");
        assert!(err.to_string().contains("injected: list_dir"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn resolution_stats_only_symlinks_not_every_entry() {
        // SYSCALL ECONOMY (spec §14). The naive fix — `metadata()` on every entry — costs one
        // stat per entry, 5,000 in a capped listing. `d_type` already yields the symlink bit
        // for free, so resolution runs ONLY on symlinks.
        //
        // FAIL-VERIFY: change `classify_entry` to call `metadata` unconditionally, watch
        // this fail with a non-zero count, then revert.
        //
        // This drives the resolution helper DIRECTLY rather than through a wrapper around
        // `list_dir`. An earlier draft counted via a `CountingFs` whose `list_dir` delegated
        // to `RealFs::list_dir` — so a regression to stat-everything would happen INSIDE the
        // delegate, never passing through the counter, and the test would pass while the
        // defect shipped. A guard that cannot observe the code under test is not a guard.
        let d = unique_dir("resolve-economy");
        std::fs::write(d.join("plain.md"), b"x").expect("seed");
        std::fs::create_dir_all(d.join("sub")).expect("seed");

        let mut stats = 0usize;
        for entry in std::fs::read_dir(&d).expect("read").flatten() {
            let ft = entry.file_type().expect("file_type");
            let (_kind, _link, _broken) = classify_entry(&entry, ft, &mut stats);
        }
        assert_eq!(stats, 0,
            "a directory of NON-symlink entries performs ZERO metadata calls — d_type answers \
             it all. If this is non-zero, resolution is stat-ing entries it does not need to.");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("plain.md"), d.join("link.md")).expect("symlink");
            let mut stats2 = 0usize;
            for entry in std::fs::read_dir(&d).expect("read").flatten() {
                let ft = entry.file_type().expect("file_type");
                let (_k, _l, _b) = classify_entry(&entry, ft, &mut stats2);
            }
            assert_eq!(stats2, 1, "exactly ONE metadata call — for the one symlink");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    // WHAT THIS GUARD COVERS, AND WHAT IT DOES NOT.
    //
    // Covers: a stat-everything regression introduced INSIDE `classify_entry` — the helper
    // that owns per-entry resolution, and where such a change would naturally land.
    //
    // Does NOT cover: a stat call added in `list_dir` AROUND the helper (e.g. an extra
    // `fs.metadata(...)` in the loop body). Observing that would need `list_dir` itself to
    // report a count, which means either a counting parameter threaded through production
    // code purely for a test, or a second implementation to drift from the first. Neither is
    // worth it for a cost regression that is visible in a profile and harmless to
    // correctness.
    //
    // Stated plainly because an honest partial guard is fine; one that READS as complete is
    // not — that is how the previous version of this test shipped as vacuous.

    #[test]
    fn list_dir_emits_a_named_entry_whose_type_cannot_be_determined() {
        // CASE 2 of the three entry categories: named, unclassifiable. It belongs in
        // `entries` with `kind == Unknown`, NOT in `unreadable` (which means "could not even
        // be NAMED"), and it must NOT abort the listing — an earlier draft used
        // `entry.file_type()?`, which would take the whole directory down over one entry.
        //
        // Exercises the REAL path. Constructing a `DirEntryInfo { kind: Unknown }` by hand
        // would assert nothing about `list_dir` — it would test the struct literal.
        //
        // A broken symlink is the portable way to reach the Unknown arm: `file_type()`
        // succeeds (it is a symlink), the follow-up `metadata()` fails, and the entry must
        // come back NAMED with `kind == Unknown` rather than being dropped or aborting the
        // listing.
        //
        // FAIL-VERIFY: change the resolution arm to `continue` on metadata failure (dropping
        // the entry), watch this fail; then to `?` (aborting), watch the second assert fail.
        let d = unique_dir("list-unknown");
        std::fs::write(d.join("ok.md"), b"x").expect("seed");
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.join("nothing"), d.join("mystery")).expect("symlink");

        let l = RealFs.list_dir(&d, None).expect("one odd entry must NOT abort the listing");
        assert!(l.entries.iter().any(|e| e.name == "ok.md"),
            "the well-formed sibling still comes back");
        #[cfg(unix)]
        {
            let m = l.entries.iter().find(|e| e.name == "mystery")
                .expect("the unclassifiable entry is EMITTED, with its name — not dropped");
            assert_eq!(m.kind, EntryKind::Unknown);
            assert_eq!(l.unreadable, 0,
                "it is a NAMED entry in `entries`, never counted in `unreadable` — that field \
                 means 'could not even be named'");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---------------------------------------------------------------------------
    // resolve_write_destination tests (Task 15)
    // ---------------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn resolve_write_destination_follows_a_link_and_preserves_it() {
        let d = unique_dir("resolve-link");
        let real = d.join("real.md");
        let link = d.join("link.md");
        std::fs::write(&real, b"original\n").expect("seed");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let got = resolve_write_destination(&RealFs, &link).expect("resolves");
        assert_eq!(std::fs::canonicalize(&got).expect("canon"),
                   std::fs::canonicalize(&real).expect("canon"),
                   "a symlinked destination resolves to its target — that is what makes \
                    writing through it work at all");
        assert!(link.symlink_metadata().expect("lstat").file_type().is_symlink(),
            "and the link itself is untouched");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resolve_write_destination_passes_a_new_path_through_unchanged() {
        // The ORDINARY Save-As case. `canonicalize` cannot serve as the mechanism here,
        // because it fails identically for "does not exist yet" and "broken symlink" —
        // which is exactly why FileStat carries `broken`.
        let d = unique_dir("resolve-new");
        let fresh = d.join("brand-new.md");
        assert_eq!(resolve_write_destination(&RealFs, &fresh).expect("passes through"), fresh);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_write_destination_refuses_a_broken_symlink() {
        let d = unique_dir("resolve-broken");
        let link = d.join("dangling.md");
        std::os::unix::fs::symlink(d.join("gone.md"), &link).expect("symlink");
        assert_eq!(resolve_write_destination(&RealFs, &link),
                   Err(DestError::BrokenSymlink),
                   "refused BEFORE dispatch — it must never reach atomic_replace");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    #[allow(clippy::print_stderr)] // env-conditional skip notice — mirrors main.rs's harness allow
    fn a_permission_denied_symlink_chain_reports_broken_not_a_listing_failure() {
        // `broken` means UNRESOLVABLE — dangling, permission-denied, or looping — not "the
        // target is gone". A permission failure on the chain must classify as broken rather
        // than failing the listing or masquerading as `Other`.
        use std::os::unix::fs::PermissionsExt;
        let d = unique_dir("list-perm");
        let hidden = d.join("hidden");
        std::fs::create_dir_all(&hidden).expect("dir");
        std::fs::write(hidden.join("t.md"), b"x").expect("seed");
        std::os::unix::fs::symlink(hidden.join("t.md"), d.join("link.md")).expect("symlink");
        std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        // Root ignores mode bits, so the chain resolves and `broken` is legitimately false.
        // Skip rather than assert the opposite.
        if std::fs::metadata(hidden.join("t.md")).is_ok() {
            std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o755)).ok();
            let _ = std::fs::remove_dir_all(&d);
            eprintln!("skip: privileged process — chmod 000 does not restrict this test");
            return;
        }

        let l = RealFs.list_dir(&d, None).expect("the listing itself still succeeds");
        let link = l.entries.iter().find(|e| e.name == "link.md").expect("link listed");
        assert!(link.is_symlink);
        assert!(link.broken, "an unresolvable chain is broken, whatever the reason");
        assert_eq!(link.kind, EntryKind::Unknown, "and therefore unclassified, not Other");

        std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o755)).expect("restore");
        let _ = std::fs::remove_dir_all(&d);
    }
}
