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

/// A resolved metadata probe. `len`/`mtime`/`is_file`/`is_dir` FOLLOW symlinks (they come
/// from `metadata`); `is_symlink` does NOT (it comes from `symlink_metadata`). Two syscalls,
/// one method — both existing behaviours preserved exactly.
///
/// `is_file` is a field and NEVER `!is_dir`: fifos, sockets, and devices are neither, so the
/// equivalence is false and `config_layer_paths`-style probes would misclassify them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // C5 tasks 3-24 use this via implementors; forward reference
pub(crate) struct FileStat {
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

/// The filesystem ops the atomic-write commit needs. Object-safe (no generics,
/// no associated types) so `&dyn Fs` works.
pub(crate) trait Fs {
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
}
