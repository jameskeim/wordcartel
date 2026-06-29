# M3 — IO Fault Injection for Durability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the atomic-write durability path fault-testable by routing all three write primitives through one shared, fault-injectable `atomic_replace` commit core, and prove the no-data-loss invariants under injected filesystem failures.

**Architecture:** A new crate-internal `fsx` module holds a small object-safe `Fs`/`WriteSync` seam, a zero-size `RealFs` that delegates to `std::fs` exactly as today, and the shared `atomic_replace` core (temp-naming + retry + `TempGuard` + mode step + commit sequence). `file::save_atomic`, `file::save_atomic_bytes`, and `swap::write_atomic` keep their public signatures and pre-checks but route their durability commit through `atomic_replace(&RealFs, …)`. A `#[cfg(test)] FaultFs` wraps `RealFs` and injects exactly one failure per run; the fault tests assert atomicity, no-litter, error-surfaced, partial-write, mode-preservation, and the pinned dir-fsync semantics.

**Tech Stack:** Rust (the `wordcartel` shell crate). `std::fs` only. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-29-wordcartel-m3-io-fault-injection-design.md` (Codex-reviewed ×3, GO).

## Global Constraints

- **Behavior-preserving in production.** The seam is internal; the three primitives' public signatures (`save_atomic(&Path, &str) -> Result<SaveOutcome, SaveError>`, `save_atomic_bytes(&Path, &[u8]) -> Result<(), SaveError>`, `write_atomic(&Path, &str) -> io::Result<()>`) are UNCHANGED. All existing callers (`save.rs`, `state.rs:88`, `recovery.rs:24`) compile untouched.
- **Seam visibility:** the `Fs`/`WriteSync` traits, `RealFs`, `ModePolicy`, `WriteOpts`, and `atomic_replace` are `pub(crate)`. `FaultFs` and the fault enum are `#[cfg(test)]`.
- **Mode behavior matches today exactly.** `save_atomic` preserves the existing target's mode (`existing_mode.unwrap_or(0o600)`), captured BEFORE temp creation. `save_atomic_bytes` and `swap::write_atomic` are always `0600`.
- **dir-fsync matches today exactly.** `save_atomic` and `save_atomic_bytes` dir-fsync after rename (`dir_fsync: true`); `swap::write_atomic` does NOT (`dir_fsync: false`). A dir that can't be opened for sync is NOT an error (`Ok(())`); a successful-open `sync_all` failure propagates as `Err`.
- **No emoji / non-ASCII in code** (Unicode in multibyte-handling tests is allowed).
- **Four merge gates (CLAUDE.md):** `cargo test` green; `cargo build` + `cargo test --no-run` warning-free; `cargo clippy --all-targets -- -D warnings` clean; `cargo fmt --check` clean.
- No `unwrap()`/`expect()` on fallible production paths beyond what exists today; tests may use `expect("…")`.

## File Structure

- **Create** `wordcartel/src/fsx.rs` — the `Fs`/`WriteSync` traits, `RealFs`, `ModePolicy`, `WriteOpts`, `atomic_replace`, the temp-naming + `TempGuard`, and (`#[cfg(test)]`) `FaultFs` + the fault tests. One responsibility: the fault-injectable atomic-write commit core.
- **Modify** `wordcartel/src/lib.rs` — add `pub mod fsx;`.
- **Modify** `wordcartel/src/file.rs` — route `save_atomic` / `save_atomic_bytes` through `atomic_replace`; delete the now-unused `create_temp` / `open_excl` / `TempGuard` / `TEMP_SEQ` (moved into `fsx`).
- **Modify** `wordcartel/src/swap.rs` — route `write_atomic` through `atomic_replace`; delete `open_excl_0600` + the manual temp logic.
- **Modify** `wordcartel/src/save.rs` — add a second Component-5 end-to-end "failed save keeps dirty" test driving a real (parent-is-a-file) failure.

---

### Task 1: `fsx` module — seam traits, `RealFs`, and the `atomic_replace` core

**Files:**
- Create: `wordcartel/src/fsx.rs`
- Modify: `wordcartel/src/lib.rs` (add `pub mod fsx;`, alphabetical-ish near the other IO modules)
- Test: `wordcartel/src/fsx.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing (new leaf module; `std::fs`, `std::io`, `std::path` only).
- Produces (later tasks rely on these EXACT signatures):
  - `pub(crate) trait WriteSync { fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>; fn flush(&mut self) -> std::io::Result<()>; fn set_mode(&self, mode: u32) -> std::io::Result<()>; fn sync_all(&self) -> std::io::Result<()>; }`
  - `pub(crate) trait Fs { fn create_excl(&self, path: &std::path::Path, mode: u32) -> std::io::Result<Box<dyn WriteSync>>; fn existing_mode(&self, path: &std::path::Path) -> Option<u32>; fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()>; fn sync_dir(&self, dir: &std::path::Path) -> std::io::Result<()>; fn remove_file(&self, path: &std::path::Path) -> std::io::Result<()>; }`
  - `pub(crate) struct RealFs;` (zero-size, `Send`) implementing `Fs`.
  - `pub(crate) enum ModePolicy { Fixed(u32), PreserveExistingOr(u32) }`
  - `pub(crate) struct WriteOpts { pub mode: ModePolicy, pub dir_fsync: bool }`
  - `pub(crate) fn atomic_replace(fs: &dyn Fs, final_path: &std::path::Path, bytes: &[u8], opts: WriteOpts) -> std::io::Result<()>`

- [ ] **Step 1: Add the module declaration**

In `wordcartel/src/lib.rs`, add alongside the other IO modules (e.g. just after `pub mod file;`):

```rust
pub mod fsx;
```

- [ ] **Step 2: Write the success-path failing tests**

Create `wordcartel/src/fsx.rs` with the test module first (it will not compile until Step 3 adds the items — that is the RED state):

```rust
//! Fault-injectable atomic-write commit core (M3). The single durability-critical
//! sequence shared by file::save_atomic / save_atomic_bytes / swap::write_atomic.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

// (production code added in Step 3)

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
            WriteOpts { mode: ModePolicy::Fixed(0o600), dir_fsync: true },
        )
        .expect("atomic_replace must succeed");
        assert_eq!(fs::read(&target).expect("read target"), b"new bytes\n");
        assert!(tmp_litter(&dir).is_empty(), "no temp may remain: {:?}", tmp_litter(&dir));
    }

    /// PreserveExistingOr keeps a non-default existing mode; absent target lands on the fallback.
    #[cfg(unix)]
    #[test]
    fn preserve_existing_mode_else_fallback() {
        use std::os::unix::fs::PermissionsExt;
        let dir = private_dir("mode");

        // Existing 0644 target → mode preserved.
        let existing = dir.join("keep.txt");
        fs::write(&existing, b"x\n").expect("seed");
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o644)).expect("chmod 644");
        atomic_replace(
            &RealFs,
            &existing,
            b"y\n",
            WriteOpts { mode: ModePolicy::PreserveExistingOr(0o600), dir_fsync: true },
        )
        .expect("replace existing");
        let m = fs::metadata(&existing).expect("meta").permissions().mode() & 0o777;
        assert_eq!(m, 0o644, "existing mode must be preserved");

        // Absent target → fallback 0600.
        let fresh = dir.join("fresh.txt");
        atomic_replace(
            &RealFs,
            &fresh,
            b"z\n",
            WriteOpts { mode: ModePolicy::PreserveExistingOr(0o600), dir_fsync: true },
        )
        .expect("create fresh");
        let m = fs::metadata(&fresh).expect("meta").permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "absent target must land on the fallback mode");
    }
}
```

- [ ] **Step 3: Implement the seam + core**

Add the production code to `wordcartel/src/fsx.rs` (above the test module):

```rust
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
            self.0.set_permissions(fs::Permissions::from_mode(_mode))?;
        }
        Ok(())
    }
    fn sync_all(&self) -> std::io::Result<()> {
        self.0.sync_all()
    }
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

impl Fs for RealFs {
    fn create_excl(&self, path: &Path, _mode: u32) -> std::io::Result<Box<dyn WriteSync>> {
        let f = open_excl(path, _mode)?;
        Ok(Box::new(RealHandle(f)))
    }
    fn existing_mode(&self, path: &Path) -> Option<u32> {
        #[cfg(unix)]
        {
            return fs::metadata(path).ok().map(|m| m.permissions().mode());
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
    fs::OpenOptions::new().write(true).create_new(true).open(path)
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
fn create_temp(fs: &dyn Fs, dir: &Path, name: &str) -> std::io::Result<(Box<dyn WriteSync>, PathBuf)> {
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
    let mut guard = TempGuard { fs, path: Some(temp.clone()) };

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
```

> Note for the implementer: the `#[cfg(unix)] use ... PermissionsExt;` import is used by `RealHandle::set_mode`. If clippy flags an unused import on non-unix, gate the `use` the same way it's used. Keep the `_mode` underscore-prefix on non-unix to avoid unused-variable warnings.

- [ ] **Step 4: Run the success-path tests**

Run: `cargo test -p wordcartel --lib fsx::tests`
Expected: `real_fs_replaces_target_no_litter` and `preserve_existing_mode_else_fallback` PASS.

- [ ] **Step 5: Gate-check this task**

Run: `cargo build -p wordcartel 2>&1 | grep -i warning` (expect no output), then
`cargo clippy -p wordcartel --all-targets -- -D warnings` and `cargo fmt --check`.
Expected: all clean.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/fsx.rs wordcartel/src/lib.rs
git commit -m "feat(m3): add fsx atomic-write seam + atomic_replace core"
```

---

### Task 2: `FaultFs` harness + the durability fault tests

**Files:**
- Modify: `wordcartel/src/fsx.rs` (extend `#[cfg(test)] mod tests` with `FaultFs` + fault tests)
- Test: same module

**Interfaces:**
- Consumes: `atomic_replace`, `RealFs`, `Fs`, `WriteSync`, `ModePolicy`, `WriteOpts` (Task 1).
- Produces: `#[cfg(test)] enum FaultAt { Create, Write { after: usize }, SetMode, Flush, Sync, Rename, SyncDir }` and `struct FaultFs { inner: RealFs, fail: FaultAt }` — test-only, not used outside this module.

- [ ] **Step 1: Write the FaultFs harness**

Add to the `tests` module of `wordcartel/src/fsx.rs`:

```rust
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
            return Err(Error::new(ErrorKind::Other, "injected: flush"));
        }
        self.inner.flush()
    }
    fn set_mode(&self, mode: u32) -> std::io::Result<()> {
        if matches!(self.fail, FaultAt::SetMode) {
            return Err(Error::new(ErrorKind::Other, "injected: set_mode"));
        }
        self.inner.set_mode(mode)
    }
    fn sync_all(&self) -> std::io::Result<()> {
        if matches!(self.fail, FaultAt::Sync) {
            return Err(Error::new(ErrorKind::Other, "injected: fsync"));
        }
        self.inner.sync_all()
    }
}

impl Fs for FaultFs {
    fn create_excl(&self, path: &Path, mode: u32) -> std::io::Result<Box<dyn WriteSync>> {
        if matches!(self.fail, FaultAt::Create) {
            return Err(Error::new(ErrorKind::Other, "injected: create"));
        }
        let inner = self.inner.create_excl(path, mode)?;
        Ok(Box::new(FaultHandle { inner, fail: self.fail, written: Cell::new(0) }))
    }
    fn existing_mode(&self, path: &Path) -> Option<u32> {
        self.inner.existing_mode(path)
    }
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        if matches!(self.fail, FaultAt::Rename) {
            return Err(Error::new(ErrorKind::Other, "injected: rename"));
        }
        self.inner.rename(from, to)
    }
    fn sync_dir(&self, dir: &Path) -> std::io::Result<()> {
        if matches!(self.fail, FaultAt::SyncDir) {
            return Err(Error::new(ErrorKind::Other, "injected: sync_dir"));
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
fn run_fault(label: &str, fail: FaultAt, opts: WriteOpts) -> (PathBuf, PathBuf, std::io::Result<()>) {
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
```

- [ ] **Step 2: Write the pre-rename atomicity + no-litter + error-surfaced tests**

```rust
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
```

- [ ] **Step 3: Write the partial-write test (explicit)**

```rust
/// A partial write (ENOSPC after k bytes): caught, original intact, no litter.
#[test]
fn partial_write_caught_original_intact() {
    let (dir, target, res) = run_fault("partial", FaultAt::Write { after: 6 }, opts_preserve());
    let err = res.expect_err("partial write must Err");
    assert_eq!(err.kind(), std::io::ErrorKind::WriteZero);
    assert_eq!(fs::read(&target).expect("read"), b"ORIGINAL-CONTENTS\n");
    assert!(tmp_litter(&dir).is_empty(), "no litter after partial write");
}
```

- [ ] **Step 4: Write the pinned dir-fsync test**

```rust
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
```

- [ ] **Step 5: Run the fault tests**

Run: `cargo test -p wordcartel --lib fsx::tests`
Expected: all fault tests + the Task-1 success tests PASS. If any pre-rename fault leaves litter or mutates the original, that is a real `atomic_replace` bug — fix it in `fsx.rs` (do not weaken the test).

- [ ] **Step 6: Gate-check + commit**

Run the four gates (build/clippy/fmt as in Task 1 Step 5). Then:

```bash
git add wordcartel/src/fsx.rs
git commit -m "test(m3): FaultFs harness + durability fault tests (atomicity, no-litter, partial-write, pinned dir-fsync)"
```

---

### Task 3: Route `file::save_atomic` + `save_atomic_bytes` through `atomic_replace`

**Files:**
- Modify: `wordcartel/src/file.rs` (lines ~163-279 rewritten to delegate; delete `create_temp`/`open_excl`/`TempGuard`/`TEMP_SEQ` at lines ~107-157)
- Test: existing `file.rs` tests stay green (no new test required; behavior-preserving)

**Interfaces:**
- Consumes: `crate::fsx::{atomic_replace, RealFs, ModePolicy, WriteOpts}` (Task 1).
- Produces: `save_atomic` / `save_atomic_bytes` with UNCHANGED public signatures.

- [ ] **Step 1: Confirm the existing file.rs tests pass (baseline)**

Run: `cargo test -p wordcartel --lib file::tests`
Expected: PASS (record the count; it must not drop after this task).

- [ ] **Step 2: Rewrite `save_atomic` to delegate**

Replace the body of `save_atomic` (file.rs:163-236) so the pre-checks stay and the commit delegates. The new body:

```rust
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
```

- [ ] **Step 3: Rewrite `save_atomic_bytes` to delegate**

Replace the body of `save_atomic_bytes` (file.rs:243-279):

```rust
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
```

- [ ] **Step 4: Delete the now-unused temp helpers from file.rs**

Remove `TEMP_SEQ` (file.rs:107-108), `TempGuard` (110-123), `create_temp` (125-141), and `open_excl` (143-157) — they live in `fsx` now. Remove the now-unused imports `use std::sync::atomic::{AtomicU32, Ordering};` (line 8) and `use std::io::Write as IoWrite;` (line 6) — both were only used by the deleted helpers (the tests import their own atomics at file.rs:288).

**Move `PathBuf` into the test module.** After the deletions and the `save_atomic`/`save_atomic_bytes` rewrites, NO production code in file.rs references `PathBuf` anymore (the dir/name resolution moved into `fsx::atomic_replace`). A top-level `use std::path::PathBuf` would then be unused during `cargo build` (which does NOT compile `#[cfg(test)]` modules) → fatal under `-D warnings`. So:
- Change the top-level import (file.rs:7) to `use std::path::Path;` (drop `PathBuf`).
- Add `use std::path::PathBuf;` INSIDE the `#[cfg(test)] mod tests` block (near its existing `use super::*;` at file.rs:287), since `scratch_path` returns `PathBuf` (file.rs:290-300).

After the edits, run `cargo build -p wordcartel 2>&1 | grep -i warning` (Step 5) to confirm `AtomicU32`/`Ordering`/`IoWrite`/`PathBuf` are all warning-free at top level, and `cargo test --no-run` to confirm the test module compiles with its own `PathBuf` import.

- [ ] **Step 5: Run the file.rs tests + full crate build**

Run: `cargo test -p wordcartel --lib file::tests`
Expected: the SAME tests pass (roundtrip, unchanged, symlink-refused, no-litter, bytes-roundtrip-no-litter, background_save_command_clears_dirty). Count must match Step 1.

Run: `cargo build -p wordcartel 2>&1 | grep -i warning`
Expected: no output (the deletions must not leave dead code / unused imports).

- [ ] **Step 6: Gate-check + commit**

Run clippy + fmt gates. Then:

```bash
git add wordcartel/src/file.rs
git commit -m "refactor(m3): route file::save_atomic + save_atomic_bytes through fsx::atomic_replace"
```

---

### Task 4: Route `swap::write_atomic` through `atomic_replace`

**Files:**
- Modify: `wordcartel/src/swap.rs` (rewrite `write_atomic` at 198-213; delete `open_excl_0600` at 215-223)
- Test: existing `swap.rs` tests stay green

**Interfaces:**
- Consumes: `crate::fsx::{atomic_replace, RealFs, ModePolicy, WriteOpts}`.
- Produces: `write_atomic(&Path, &str) -> io::Result<()>` with UNCHANGED signature.

- [ ] **Step 1: Confirm the existing swap.rs tests pass (baseline)**

Run: `cargo test -p wordcartel --lib swap::tests`
Expected: PASS (record the count).

- [ ] **Step 2: Rewrite `write_atomic` to delegate**

Replace `write_atomic` (swap.rs:197-213):

```rust
/// Atomic 0600 write into our own state dir (no symlink/skip-unchanged logic, no
/// dir-fsync). Routes through the shared fault-tested core, inheriting its
/// TempGuard cleanup (this path previously left a temp behind on write failure).
pub fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    crate::fsx::atomic_replace(
        &crate::fsx::RealFs,
        path,
        content.as_bytes(),
        crate::fsx::WriteOpts { mode: crate::fsx::ModePolicy::Fixed(0o600), dir_fsync: false },
    )
}
```

- [ ] **Step 3: Delete the now-unused `open_excl_0600`**

Remove `open_excl_0600` (both `#[cfg(unix)]` and `#[cfg(not(unix))]` arms, swap.rs:215-223). Remove the `use std::io::Write as _;` import (line 8) if it is no longer referenced elsewhere in swap.rs (grep for other `.write_all`/`.flush` uses first; `build_header`/`serialize` may not use it). Keep `use std::io;` (the return type).

- [ ] **Step 4: Run the swap.rs tests + recovery/save callers**

Run: `cargo test -p wordcartel --lib swap::tests`
Expected: the SAME tests pass (roundtrip, orphan detection, recovery assess). Count matches Step 1.

Run: `cargo test -p wordcartel --lib recovery` then `cargo test -p wordcartel --lib save::tests` (two separate invocations — `cargo test` does not reliably accept multiple filter strings).
Expected: PASS — `recovery.rs:24` and `save.rs` swap-using tests still work through the unchanged signature.

- [ ] **Step 5: Gate-check + commit**

Run build (warning-free) + clippy + fmt gates. Then:

```bash
git add wordcartel/src/swap.rs
git commit -m "refactor(m3): route swap::write_atomic through fsx::atomic_replace (gains TempGuard cleanup)"
```

---

### Task 5: Component-5 end-to-end — a real failed save keeps the buffer dirty

**Files:**
- Modify: `wordcartel/src/save.rs` (`#[cfg(test)] mod tests`, alongside `background_save_failure_keeps_dirty_and_status` at ~339)
- Test: same module

**Interfaces:**
- Consumes: `dispatch_save`, `Ctx`, `InlineExecutor`, `apply_result` (existing test harness in save.rs).
- Produces: nothing (test-only).

- [ ] **Step 1: Confirm the existing keeps-dirty test passes**

Run: `cargo test -p wordcartel --lib save::tests::background_save_failure_keeps_dirty_and_status`
Expected: PASS (this is the existing symlink-based Unix-only case; keep it).

- [ ] **Step 2: Add a second real-failure case (parent is a file)**

Add to the `tests` module of `save.rs`:

```rust
/// A real (non-symlink) save failure also keeps the buffer dirty: the target's
/// parent is a regular FILE, so temp creation fails immediately (parent not a
/// directory). Drives the do_save_to merge's Err arm without any test seam.
#[cfg(unix)]
#[test]
fn background_save_failure_parent_is_file_keeps_dirty() {
    let parent = scratch();
    std::fs::write(&parent, "i am a file, not a dir\n").unwrap();
    // target sits "inside" a regular file → ENOTDIR on temp create.
    let target = parent.join("doc.md");

    let mut e = Editor::new_from_text("hello\n", Some(target.clone()), (80, 24));
    e.active_mut().document.saved_version = None;
    e.active_mut().document.version = 1;
    let ex = InlineExecutor::default();
    let clk = Z;
    {
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx() };
        dispatch_save(&mut ctx);
    }
    for r in ex.drain() { crate::app::apply_result(r, &mut e); }

    assert!(e.active().document.dirty(), "failed save must leave the buffer dirty");
    assert!(e.active().document.saved_version.is_none());
    assert!(!e.status.is_empty(), "an error status must be surfaced");
    let _ = std::fs::remove_file(&parent);
}
```

> Note: `dispatch_save` runs the external-mod check first. Because `target` does not exist on disk, `fingerprint(&target)` yields the same "absent" fingerprint as the buffer's `stored_fp` default — confirm the buffer's `stored_fp` is `None`/absent so the check passes and the save actually dispatches (it is, for `new_from_text` with an unsaved edit). If the external-mod modal opens instead of dispatching, the test will show an empty drain and a non-dirty assert failure — in that case set `e.active_mut().document.stored_fp = fingerprint(&target);` before dispatch to align them.

- [ ] **Step 3: Run the new test**

Run: `cargo test -p wordcartel --lib save::tests::background_save_failure_parent_is_file_keeps_dirty`
Expected: PASS — the buffer is dirty, `saved_version` is `None`, status non-empty.

- [ ] **Step 4: Full-suite gate**

Run: `cargo test -p wordcartel` and `cargo test -p wordcartel-core`
Expected: all green (the M3 changes are behavior-preserving; only fsx tests + this one are net-new).

- [ ] **Step 5: Final gate-check + commit**

Run build (warning-free) + `cargo clippy --all-targets -- -D warnings` + `cargo fmt --check`.
Expected: all clean.

```bash
git add wordcartel/src/save.rs
git commit -m "test(m3): end-to-end real-failure (parent-is-file) keeps buffer dirty"
```

---

## Self-Review

**Spec coverage:**
- Component 1 (`Fs` seam + `RealFs`) → Task 1. ✔ (incl. `set_mode`, `existing_mode`, `sync_dir` swallow, threading model — `RealFs` constructed inside each primitive, never crossing the worker boundary.)
- Component 2 (`atomic_replace` + `ModePolicy`/`WriteOpts`, mode-read-before-temp, temp-name unification, `TempGuard`) → Task 1. ✔
- Component 3 (`FaultFs` incl. `SetMode`) → Task 2. ✔
- Component 4 (fault tests: atomicity, no-litter, error-surfaced, partial-write, pinned dir-fsync, success-path + mode-preservation) → Tasks 1-2. ✔
- Component 5 (failed save keeps dirty) → Task 5 (existing symlink test kept; real parent-is-file case added). ✔
- Routing all three primitives → Tasks 3 (file ×2) + 4 (swap). ✔ `state.rs`/`recovery.rs` covered transitively (unchanged signatures).
- Out-of-scope (export raw rename, reads/metadata/session-load faults) → not touched. ✔

**Type consistency:** `atomic_replace(fs: &dyn Fs, final_path: &Path, bytes: &[u8], opts: WriteOpts) -> io::Result<()>` is referenced identically in Tasks 1-4. `ModePolicy::{Fixed, PreserveExistingOr}`, `WriteOpts{mode, dir_fsync}`, `WriteSync::{write_all, flush, set_mode, sync_all}`, `Fs::{create_excl, existing_mode, rename, sync_dir, remove_file}` are consistent across tasks.

**Placeholder scan:** none — every step has concrete code or an exact command.

**Compile-correctness:** `FaultAt` derives `Debug` and assertion messages use `{fail:?}` (a bare `as u8` cast would not compile for the data-carrying `Write { after }` variant). The `#[cfg(unix)] use PermissionsExt;` in `fsx.rs` is referenced only by Unix code paths; the non-unix arms underscore-prefix `mode` to stay warning-free.

## Execution Handoff

Two execution options:

**1. Subagent-Driven (recommended)** — fresh subagent per task, task review (spec + quality) between tasks, broad whole-branch review at the end.

**2. Inline Execution** — batch execution in this session with checkpoints.

Which approach?
