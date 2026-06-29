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
pub(crate) trait WriteSync {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
    fn set_mode(&self, mode: u32) -> std::io::Result<()>;
    fn sync_all(&self) -> std::io::Result<()>;
}

/// The filesystem ops the atomic-write commit needs. Object-safe (no generics,
/// no associated types) so `&dyn Fs` works.
pub(crate) trait Fs {
    /// O_EXCL create at `path` with `mode` (Unix); returns a write+sync handle.
    fn create_excl(&self, path: &Path, mode: u32) -> std::io::Result<Box<dyn WriteSync>>;
    /// Best-effort mode of an existing file (Unix); `None` if absent/unreadable/off-unix.
    fn existing_mode(&self, path: &Path) -> Option<u32>;
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    /// Durably flush a directory entry. A dir that cannot be opened is NOT an error.
    fn sync_dir(&self, dir: &Path) -> std::io::Result<()>;
    fn remove_file(&self, path: &Path) -> std::io::Result<()>;
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

    let (mut handle, temp) = create_temp(fs, &dir, &name)?;
    let mut guard = TempGuard {
        fs,
        path: Some(temp.clone()),
    };

    handle.write_all(bytes)?;
    handle.set_mode(final_mode)?;
    handle.flush()?;
    handle.sync_all()?;
    drop(handle); // close the temp before rename (consistent across all paths; required on Windows)

    fs.rename(&temp, final_path)?;
    guard.disarm(); // temp renamed away; nothing to clean up regardless of what follows

    if opts.dir_fsync {
        fs.sync_dir(&dir)?;
    }
    Ok(())
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

    // ---------------------------------------------------------------------------
    // FaultFs harness — fault-injectable Fs/WriteSync for durability tests
    // ---------------------------------------------------------------------------

    use std::cell::Cell;
    use std::io::{Error, ErrorKind};

    // Note: no `Remove` variant. `remove_file` is only ever called by TempGuard::drop on a
    // pre-rename early return — which is itself caused by the ONE injected fault — so the
    // single-fault model can never make the cleanup-remove ALSO fail. The remove path IS still
    // exercised (and must succeed) in every pre-rename fault test, where it is what makes the
    // no-litter assertion hold.
    #[derive(Clone, Copy, Debug)]
    enum FaultAt {
        Create,
        Write { after: usize },
        SetMode,
        Flush,
        Sync,
        Rename,
        SyncDir,
    }

    struct FaultFs {
        inner: RealFs,
        fail: FaultAt,
    }

    // A write handle that may inject a partial-write or a set_mode/flush/sync failure.
    // Owns its injected config by value (the boxed handle is `'static`, so it cannot
    // borrow from the FaultFs).
    struct FaultHandle {
        inner: Box<dyn WriteSync>,
        fail: FaultAt,
        written: Cell<usize>,
    }

    impl WriteSync for FaultHandle {
        fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
            if let FaultAt::Write { after } = self.fail {
                // Write `after` real bytes to the temp, then fail with ENOSPC-like error.
                let n = after.min(buf.len());
                self.inner.write_all(&buf[..n])?;
                self.written.set(self.written.get() + n);
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
            Ok(Box::new(FaultHandle { inner, fail: self.fail, written: Cell::new(0) }))
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
            // No injection: cleanup-remove must succeed for the no-litter assertion to hold
            // under every pre-rename fault. (See the FaultAt note — Remove is unreachable as
            // a *second* fault.)
            self.inner.remove_file(path)
        }
    }

    // Helper: run atomic_replace with one injected fault over a freshly-seeded target.
    fn run_fault(
        label: &str,
        fail: FaultAt,
        opts: WriteOpts,
    ) -> (PathBuf, PathBuf, std::io::Result<()>) {
        let dir = private_dir(label);
        let target = dir.join("doc.txt");
        fs::write(&target, b"ORIGINAL-CONTENTS\n").expect("seed original");
        let fs_impl = FaultFs { inner: RealFs, fail };
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
}
