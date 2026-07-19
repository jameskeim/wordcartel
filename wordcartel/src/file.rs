// wordcartel/src/file.rs — file open (binary refusal) + atomic save
// Ported atomic primitive from ~/projects/par-command/repar/src/atomic.rs (MIT, user's own).
// Added: symlink refusal, skip-unchanged, mode preservation (#[cfg(unix)]).

#[cfg(test)]
use std::fs;
use std::path::Path;

// ---------------------------------------------------------------------------
// Public error / outcome types
// ---------------------------------------------------------------------------

#[derive(thiserror::Error, Debug)]
pub enum OpenError {
    #[error("{0}: not found")]
    NotFound(String),
    #[error("{0}: not valid UTF-8 / binary — refused")]
    Binary(String),
    #[error("{0}: permission denied")]
    Permission(String),
    #[error("{0}: is a directory")]
    IsDir(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}: too large (> {1} bytes)")]
    TooLarge(String, u64),
}

#[derive(thiserror::Error, Debug)]
pub enum SaveError {
    #[error("no path")]
    NoPath,
    #[error("refusing to write through symlink")]
    Symlink,
    #[error("{0}")]
    Io(String),
}

#[derive(PartialEq, Debug)]
pub enum SaveOutcome {
    Saved,
    Unchanged,
}

// ---------------------------------------------------------------------------
// is_binary — ported verbatim from repar/src/atomic.rs
// ---------------------------------------------------------------------------

fn is_binary(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_err() || bytes.contains(&0)
}

// ---------------------------------------------------------------------------
// open
// ---------------------------------------------------------------------------

/// Map an IO error from File::open or read_to_end to the appropriate OpenError.
/// Preserves BOTH is_dir() disambiguation sites — the NotFound arm (some FS
/// surfaces a dir-read as NotFound) and the catch-all (some OSes return Other).
fn map_open_io_err(e: std::io::Error, label: &str, path: &Path) -> OpenError {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            // Double-check: could be PermissionDenied that looks like NotFound
            // on some FS. is_dir() does a separate stat — if that fails we keep
            // NotFound. If the path IS a dir, that takes precedence.
            if path.is_dir() {
                OpenError::IsDir(label.to_owned())
            } else {
                OpenError::NotFound(label.to_owned())
            }
        }
        std::io::ErrorKind::PermissionDenied => OpenError::Permission(label.to_owned()),
        _ => {
            // For anything else, still check IsDir (some OSes return Other
            // when read() is called on a directory).
            if path.is_dir() {
                OpenError::IsDir(label.to_owned())
            } else {
                OpenError::Io(format!("{label}: {e}"))
            }
        }
    }
}

pub fn open(path: &Path) -> Result<String, OpenError> {
    open_with_fs(&crate::fsx::RealFs, path)
}

/// Seam-taking core of [`open`]. Kept `pub(crate)` so tests can inject a `FaultFs`.
pub(crate) fn open_with_fs(fs: &dyn crate::fsx::Fs, path: &Path) -> Result<String, OpenError> {
    open_bounded_with_fs(fs, path, crate::limits::MAX_OPEN_BYTES)
}

/// `open_with_fs` with an explicit cap — the seam-taking core proper.
pub(crate) fn open_bounded_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, limit: u64)
    -> Result<String, OpenError>
{
    let label = path.display().to_string();

    // (a) Fast refusal when metadata is trustworthy. `stat` follows symlinks, matching the
    // `fs::metadata` this replaces; a `broken` link falls through to the read, which fails —
    // exactly what the old `if let Ok(meta)` did.
    if let Ok(st) = fs.stat(path) {
        if st.is_file && st.len > limit {
            return Err(OpenError::TooLarge(label, limit));
        }
    }

    // (b) Bounded read — caps the allocation even if metadata lied (/proc, sparse).
    let bytes = match fs.read_capped(path, limit) {
        Ok(Some(b)) => b,
        Ok(None) => return Err(OpenError::TooLarge(label, limit)),
        Err(e) => return Err(map_open_io_err(e, &label, path)),
    };

    // Explicit is_dir check AFTER a successful read is unlikely on most OSes, but guard it
    // anyway (opening a dir with read() sometimes succeeds on some FS).
    if matches!(fs.stat(path), Ok(st) if st.is_dir) {
        return Err(OpenError::IsDir(label));
    }

    if is_binary(&bytes) {
        return Err(OpenError::Binary(label));
    }

    Ok(String::from_utf8(bytes).expect("already verified by is_binary"))
}

// ---------------------------------------------------------------------------
// save_atomic
// ---------------------------------------------------------------------------

/// Read `path` fully, capping the allocation at `limit` bytes. Returns `None` if the
/// file exceeds `limit` OR any open/read error occurs — every caller treats `None` as
/// its existing safe degradation. Mirrors `open`'s `.take(limit + 1)` + `len > limit`.
pub fn bounded_read_opt(path: &Path, limit: u64) -> Option<Vec<u8>> {
    bounded_read_opt_with_fs(&crate::fsx::RealFs, path, limit)
}

/// Seam-taking core. Preserves the historical contract EXACTLY: `None` for both over-cap
/// and IO failure, because every caller treats `None` as its own safe degradation. The
/// seam distinguishes the two; this wrapper deliberately discards the distinction.
pub(crate) fn bounded_read_opt_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, limit: u64)
    -> Option<Vec<u8>>
{
    match fs.read_capped(path, limit) {
        Ok(Some(b)) => Some(b),
        Ok(None) | Err(_) => None,
    }
}

pub fn save_atomic(path: &Path, content: &str) -> Result<SaveOutcome, SaveError> {
    save_atomic_with_fs(&crate::fsx::RealFs, path, content)
}

/// Seam-taking core of [`save_atomic`]. Kept `pub(crate)` so tests can inject a `FaultFs`.
pub(crate) fn save_atomic_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, content: &str)
    -> Result<SaveOutcome, SaveError>
{
    // (1) Symlink refusal. UNCHANGED semantics: `stat` reports `is_symlink` from
    // `symlink_metadata`, which does not follow — exactly what this check needs.
    // This stays an unconditional last-resort guard; C5 resolves destinations BEFORE
    // they reach here (spec §7.6.1), so it simply never fires on the save path.
    match fs.stat(path) {
        Ok(st) if st.is_symlink => return Err(SaveError::Symlink),
        _ => {}
    }

    // (2) Skip-unchanged — bounded read; over-cap or unreadable → skip the optimization.
    if let Some(existing) = bounded_read_opt_with_fs(fs, path, crate::limits::MAX_OPEN_BYTES) {
        if existing == content.as_bytes() {
            return Ok(SaveOutcome::Unchanged);
        }
    }

    // (3) Commit through the shared fault-tested core. Mode is preserved from the
    // existing target (else 0600); dir-fsync after rename for durability.
    crate::fsx::atomic_replace(fs, path, content.as_bytes(), crate::fsx::WriteOpts {
        mode: crate::fsx::ModePolicy::PreserveExistingOr(0o600),
        dir_fsync: true,
    })
    .map_err(|e| SaveError::Io(e.to_string()))?;

    Ok(SaveOutcome::Saved)
}

// ---------------------------------------------------------------------------
// save_atomic_bytes — mirrors save_atomic but accepts raw bytes (no UTF-8 check,
// no skip-unchanged).  Used by the export path (Task 5) for HTML/binary output.
// ---------------------------------------------------------------------------

pub fn save_atomic_bytes(path: &Path, content: &[u8]) -> Result<(), SaveError> {
    save_atomic_bytes_with_fs(&crate::fsx::RealFs, path, content)
}

/// Byte-exact atomic write. NO UTF-8 check and NO skip-unchanged (unlike `save_atomic`),
/// but it DOES share the symlink refusal: `atomic_replace` renames over the target, which
/// through a link would replace the link with a regular file.
///
/// The guard is new in C5. Before, export targets were derived and never user-chosen, so
/// the exposure did not exist; C5 lets a writer pick an export destination (spec §9), and
/// a target can become a symlink between resolution and write. Session-state writes
/// (`state::SessionState::save_in`) acquire the same guard — a deliberate change, not a
/// side effect.
pub(crate) fn save_atomic_bytes_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, content: &[u8])
    -> Result<(), SaveError>
{
    match fs.stat(path) {
        Ok(st) if st.is_symlink => return Err(SaveError::Symlink),
        _ => {}
    }
    crate::fsx::atomic_replace(fs, path, content, crate::fsx::WriteOpts {
        mode: crate::fsx::ModePolicy::Fixed(0o600),
        dir_fsync: true,
    })
    .map_err(|e| SaveError::Io(e.to_string()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    // Unique scratch path: pid + monotonic counter + a label.
    static SEQ: AtomicU32 = AtomicU32::new(0);
    fn scratch_path(label: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "wcartel-test-{}-{}-{}.txt",
            std::process::id(),
            n,
            label
        ))
    }

    // -------------------------------------------------------------------------
    // TDD RED tests (from brief) — these are written BEFORE implementation.
    // -------------------------------------------------------------------------

    #[test]
    fn open_routes_through_the_seam_and_faults_are_injectable() {
        // First time file::open is fault-testable at all — it hardcoded RealFs internally.
        let p = scratch_path("open-fault");
        fs::write(&p, b"hello\n").expect("seed");
        let ff = crate::test_support::FaultFs::new(crate::test_support::FaultAt::ReadCapped);
        let err = open_with_fs(&ff, &p).expect_err("injected read must surface as OpenError");
        assert!(matches!(err, OpenError::Io(_)), "injected IO error maps to OpenError::Io, got {err:?}");
        // And the real seam still opens normally.
        assert_eq!(open_with_fs(&crate::fsx::RealFs, &p).expect("real open"), "hello\n");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn open_over_cap_is_still_too_large_not_io() {
        // Behaviour preservation: over-cap must stay OpenError::TooLarge, NOT become an
        // IO error just because read_capped now separates the two outcomes.
        let p = scratch_path("open-over");
        fs::write(&p, vec![b'x'; 64]).expect("seed");
        let err = open_bounded_with_fs(&crate::fsx::RealFs, &p, 8)
            .expect_err("over-cap must be refused");
        assert!(matches!(err, OpenError::TooLarge(_, 8)), "got {err:?}");
        let _ = fs::remove_file(&p);
    }

    /// save_atomic writes content; a subsequent open reads it back → Saved.
    #[test]
    fn save_and_open_roundtrip() {
        let p = scratch_path("roundtrip");
        let outcome = save_atomic(&p, "hello world\n").expect("save must succeed");
        assert_eq!(outcome, SaveOutcome::Saved);
        let got = open(&p).expect("open must succeed after save");
        assert_eq!(got, "hello world\n");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn bounded_read_opt_caps_allocation() {
        let p = scratch_path("bounded");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(bounded_read_opt(&p, 4).as_deref(), Some(&b"abc"[..]), "3 ≤ limit 4 → Some");
        assert_eq!(bounded_read_opt(&p, 3).as_deref(), Some(&b"abc"[..]), "exactly limit → Some");
        fs::write(&p, b"0123456789").unwrap();
        assert_eq!(bounded_read_opt(&p, 4), None, "10 > limit 4 → None");
        let _ = fs::remove_file(&p);
        assert_eq!(bounded_read_opt(&p, 4), None, "missing path → None");
    }

    /// The personal-dictionary load (app.rs) now routes through bounded_read_opt + from_utf8.
    #[test]
    fn dictionary_style_read_is_bounded_and_utf8_guarded() {
        let p = scratch_path("dict-cap");
        std::fs::write(&p, "alpha\nbeta\n").unwrap();
        // In-cap valid file → Some(bytes) → valid UTF-8 → words parse.
        let text = bounded_read_opt(&p, crate::limits::MAX_OPEN_BYTES)
            .and_then(|b| String::from_utf8(b).ok());
        assert_eq!(text.as_deref(), Some("alpha\nbeta\n"));
        // Over-cap file → None → empty dictionary (no slurp, no panic).
        std::fs::write(&p, "x".repeat(10)).unwrap();
        assert_eq!(bounded_read_opt(&p, 4), None, "over-cap → None → empty dict degradation");
        let _ = fs::remove_file(&p);
    }

    /// Saving the SAME content again returns Unchanged (by OUTCOME, not mtime).
    #[test]
    fn save_same_content_returns_unchanged() {
        let p = scratch_path("unchanged");
        save_atomic(&p, "same content\n").expect("first save");
        let outcome = save_atomic(&p, "same content\n").expect("second save");
        assert_eq!(
            outcome,
            SaveOutcome::Unchanged,
            "saving identical content must return Unchanged"
        );
        let _ = fs::remove_file(&p);
    }

    /// open on a file containing a NUL byte returns OpenError::Binary.
    #[test]
    fn open_nul_byte_returns_binary() {
        let p = scratch_path("binary");
        fs::write(&p, b"has\0nul").expect("write binary file");
        let err = open(&p).expect_err("must fail on binary file");
        assert!(
            matches!(err, OpenError::Binary(_)),
            "expected Binary, got {err:?}"
        );
        let _ = fs::remove_file(&p);
    }

    /// open on a missing path returns OpenError::NotFound.
    #[test]
    fn open_missing_returns_not_found() {
        let p = scratch_path("missing");
        // Ensure it really doesn't exist.
        let _ = fs::remove_file(&p);
        let err = open(&p).expect_err("must fail on missing file");
        assert!(
            matches!(err, OpenError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
    }

    /// Saving through a symlink returns SaveError::Symlink.
    #[cfg(unix)]
    #[test]
    fn save_through_symlink_refused() {
        use std::os::unix::fs::symlink;
        let real = scratch_path("symlink-real");
        let link = scratch_path("symlink-link");
        fs::write(&real, "real content\n").expect("write real file");
        symlink(&real, &link).expect("create symlink");
        let err = save_atomic(&link, "new content\n").expect_err("must refuse symlink");
        assert!(
            matches!(err, SaveError::Symlink),
            "expected Symlink, got {err:?}"
        );
        let _ = fs::remove_file(&link);
        let _ = fs::remove_file(&real);
    }

    /// A background save on a real path clears dirty.
    #[test]
    fn background_save_command_clears_dirty() {
        use crate::editor::Editor;
        use crate::jobs::{Executor, InlineExecutor};
        use crate::registry::Ctx;
        use wordcartel_core::history::Clock;
        struct Z; impl Clock for Z { fn now_ms(&self) -> u64 { 0 } }

        let p = scratch_path("cmd-save");
        fs::write(&p, "initial\n").expect("pre-write");
        let mut e = Editor::new_from_text("hello\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None; // unsaved edit
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
          crate::save::dispatch_save(&mut ctx); }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(!e.active().document.dirty(), "dirty must be cleared after a successful background save");
        let _ = fs::remove_file(&p);
    }

    // -------------------------------------------------------------------------
    // Additional correctness tests
    // -------------------------------------------------------------------------

    /// open on a valid UTF-8 file with control chars (not NUL) succeeds.
    #[test]
    fn open_control_chars_ok() {
        let p = scratch_path("ctrl");
        fs::write(&p, b"tab\tcr\r\x1b esc").expect("write");
        let content = open(&p).expect("open must succeed for control chars");
        assert!(content.contains("tab"));
        let _ = fs::remove_file(&p);
    }

    /// open on a file with invalid UTF-8 returns OpenError::Binary.
    #[test]
    fn open_invalid_utf8_returns_binary() {
        let p = scratch_path("utf8bad");
        fs::write(&p, b"\xff\xfe invalid").expect("write");
        let err = open(&p).expect_err("must fail on invalid UTF-8");
        assert!(
            matches!(err, OpenError::Binary(_)),
            "expected Binary, got {err:?}"
        );
        let _ = fs::remove_file(&p);
    }

    /// Saving different content returns Saved (not Unchanged).
    #[test]
    fn save_different_content_returns_saved() {
        let p = scratch_path("different");
        save_atomic(&p, "first\n").expect("first save");
        let outcome = save_atomic(&p, "second\n").expect("second save");
        assert_eq!(
            outcome,
            SaveOutcome::Saved,
            "different content must return Saved"
        );
        let got = open(&p).expect("open after update");
        assert_eq!(got, "second\n");
        let _ = fs::remove_file(&p);
    }

    /// Fix 2: save_atomic propagates a dir-fsync failure as SaveError::Io.
    ///
    /// A true dir-fsync EIO is not portably simulatable in a unit test, so this
    /// test documents the code path via a comment and verifies the *happy path*
    /// (successful dir-sync) still returns Ok(Saved).  The important semantic
    /// change is that the `let _ = dir_fh.sync_all()` in production code has
    /// been changed to propagate the error — see the implementation.
    #[test]
    fn save_atomic_dir_fsync_success_still_returns_saved() {
        let p = scratch_path("dirsync");
        let outcome = save_atomic(&p, "dir-fsync test\n").expect("save must succeed");
        assert_eq!(outcome, SaveOutcome::Saved, "successful dir-fsync path must return Saved");
        let _ = std::fs::remove_file(&p);
    }

    /// save_atomic_bytes: roundtrip with a non-UTF-8 byte (0xFF), no temp litter.
    #[test]
    fn save_atomic_bytes_roundtrip_no_litter() {
        let private_dir = std::env::temp_dir().join(format!(
            "wcartel-bytes-litter-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&private_dir).expect("create private temp subdir");

        let p = private_dir.join("export-test.bin");
        let content: Vec<u8> = vec![0x48, 0x65, 0xFF, 0x6C, 0x6F]; // He<0xFF>Lo

        save_atomic_bytes(&p, &content).expect("save_atomic_bytes must succeed");

        // Verify content was written correctly.
        let got = fs::read(&p).expect("must read back");
        assert_eq!(got, content, "bytes must round-trip exactly");

        // No temp litter.
        let litter: Vec<_> = fs::read_dir(&private_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let s = name.to_string_lossy();
                s.ends_with(".tmp")
            })
            .collect();
        assert!(litter.is_empty(), "temp litter remains: {litter:?}");

        // Clean up.
        let _ = fs::remove_file(&p);
        let _ = fs::remove_dir(&private_dir);
    }

    /// open refuses a file larger than MAX_OPEN_BYTES.
    ///
    /// A sparse file is created with set_len — no 64 MiB of actual disk I/O.
    #[test]
    fn open_refuses_file_over_cap() {
        let p = scratch_path("toobig");
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(crate::limits::MAX_OPEN_BYTES + 1).unwrap();
        drop(f);
        let err = open(&p).expect_err("must refuse oversized file");
        assert!(matches!(err, OpenError::TooLarge(..)), "expected TooLarge, got {err:?}");
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(unix)]
    #[test]
    fn save_atomic_bytes_refuses_a_symlink() {
        // save_atomic_bytes had NO symlink guard. It is the export write path, and C5 makes
        // export targets user-selectable for the first time — so a chosen target can now be
        // a symlink, and the target can be swapped for one between resolution and write.
        let real = scratch_path("bytes-link-real");
        let link = scratch_path("bytes-link");
        fs::write(&real, b"original\n").expect("seed");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let err = save_atomic_bytes_with_fs(&crate::fsx::RealFs, &link, b"new\n")
            .expect_err("must refuse");
        assert!(matches!(err, SaveError::Symlink), "got {err:?}");
        assert_eq!(fs::read(&real).expect("read"), b"original\n", "target untouched");
        let _ = fs::remove_file(&link); let _ = fs::remove_file(&real);
    }

    /// No temp litter left after a successful save.
    ///
    /// Uses a private subdirectory so concurrent test runs cannot produce
    /// `.wcartel-*.tmp` files that the glob picks up and makes this test flaky.
    #[test]
    fn no_temp_litter_after_save() {
        // Create a unique private subdir for this test run so we only see our
        // own temp files, not those of other concurrent saves in the shared
        // system temp dir.
        let private_dir = std::env::temp_dir().join(format!(
            "wcartel-littertest-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&private_dir).expect("create private temp subdir");

        let p = private_dir.join("litter-target.txt");
        save_atomic(&p, "clean\n").expect("save");

        // Check there are no .wcartel-*.tmp files left in our private subdir.
        let litter: Vec<_> = fs::read_dir(&private_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let s = name.to_string_lossy();
                s.ends_with(".tmp")
            })
            .collect();
        assert!(litter.is_empty(), "temp litter remains: {litter:?}");

        // Clean up.
        let _ = fs::remove_file(&p);
        let _ = fs::remove_dir(&private_dir);
    }
}
