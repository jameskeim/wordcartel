// wordcartel/src/file.rs — file open (binary refusal) + atomic save
// Ported atomic primitive from ~/projects/par-command/repar/src/atomic.rs (MIT, user's own).
// Added: symlink refusal, skip-unchanged, mode preservation (#[cfg(unix)]).

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

pub fn open(path: &Path) -> Result<String, OpenError> {
    let label = path.display().to_string();

    // Read raw bytes — map the common IO error kinds before anything else.
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return Err(match e.kind() {
                std::io::ErrorKind::NotFound => {
                    // Double-check: could be PermissionDenied that looks like NotFound
                    // on some FS. is_dir() does a separate stat — if that fails we keep
                    // NotFound. If the path IS a dir, that takes precedence.
                    if path.is_dir() {
                        OpenError::IsDir(label)
                    } else {
                        OpenError::NotFound(label)
                    }
                }
                std::io::ErrorKind::PermissionDenied => OpenError::Permission(label),
                _ => {
                    // For anything else, still check IsDir (some OSes return Other
                    // when read() is called on a directory).
                    if path.is_dir() {
                        OpenError::IsDir(label)
                    } else {
                        OpenError::Io(format!("{label}: {e}"))
                    }
                }
            });
        }
    };

    // Explicit is_dir check AFTER a successful read is unlikely on most OSes, but
    // guard it anyway (opening a dir with read() sometimes succeeds on some FS).
    if path.is_dir() {
        return Err(OpenError::IsDir(label));
    }

    // Binary test: NUL byte OR invalid UTF-8 (exactly repar's is_binary).
    if is_binary(&bytes) {
        return Err(OpenError::Binary(label));
    }

    // SAFETY: is_binary already verified valid UTF-8; from_utf8 will not fail.
    Ok(String::from_utf8(bytes).expect("already verified by is_binary"))
}

// ---------------------------------------------------------------------------
// save_atomic
// ---------------------------------------------------------------------------

pub fn save_atomic(path: &Path, content: &str) -> Result<SaveOutcome, SaveError> {
    // (1) Symlink refusal — symlink_metadata does NOT follow the link.
    match path.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => return Err(SaveError::Symlink),
        _ => {}
    }

    // (2) Skip-unchanged — if on-disk bytes equal content bytes, skip the write.
    if let Ok(existing) = fs::read(path) {
        if existing == content.as_bytes() {
            return Ok(SaveOutcome::Unchanged);
        }
    }

    // (3) Commit through the shared fault-tested core. Mode is preserved from the
    // existing target (else 0600); dir-fsync after rename for durability.
    crate::fsx::atomic_replace(
        &crate::fsx::RealFs,
        path,
        content.as_bytes(),
        crate::fsx::WriteOpts {
            mode: crate::fsx::ModePolicy::PreserveExistingOr(0o600),
            dir_fsync: true,
        },
    )
    .map_err(|e| SaveError::Io(e.to_string()))?;

    Ok(SaveOutcome::Saved)
}

// ---------------------------------------------------------------------------
// save_atomic_bytes — mirrors save_atomic but accepts raw bytes (no UTF-8 check,
// no skip-unchanged).  Used by the export path (Task 5) for HTML/binary output.
// ---------------------------------------------------------------------------

pub fn save_atomic_bytes(path: &Path, content: &[u8]) -> Result<(), SaveError> {
    crate::fsx::atomic_replace(
        &crate::fsx::RealFs,
        path,
        content,
        crate::fsx::WriteOpts {
            mode: crate::fsx::ModePolicy::Fixed(0o600),
            dir_fsync: true,
        },
    )
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
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
          crate::save::dispatch_save(&mut ctx); }
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
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
