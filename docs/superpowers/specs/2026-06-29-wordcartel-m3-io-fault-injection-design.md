# M3 — IO Fault Injection for Durability — Design

**Status:** Approved (brainstorm complete)
**Date:** 2026-06-29
**Parent:** Hardening campaign workstream **M3**
(`docs/superpowers/plans/2026-06-28-wordcartel-hardening-fuzz-proptest-plan.md`).
**Crate:** `wordcartel` shell (durability paths).

## Goal

Make the atomic-write durability path **fault-testable** and prove the no-data-loss
invariants under injected filesystem failures (disk-full / partial write / fsync-fail /
rename-fail / leftover temp). Behavior-preserving in production; the seam is internal.
This is the workstream the blind-spot analysis flagged as fundamentally untestable today
(durability can't be fuzzed — it needs *injected* failures), and it gates the
"no data loss" definition-of-done before Effort P.

## Background (the surface, from the IO map)

Three near-duplicate atomic-write primitives, all on direct `std::fs` with NO seam:
- `file::save_atomic(path, content) -> Result<SaveOutcome, SaveError>` (file.rs:163) —
  symlink guard + skip-unchanged + Unix mode-preservation + commit + dir-fsync.
- `file::save_atomic_bytes(path, content) -> Result<(), SaveError>` (file.rs:243) —
  commit (0600) + dir-fsync; no skip-unchanged.
- `swap::write_atomic(path, content) -> io::Result<()>` (swap.rs:198) — commit (0600);
  NO dir-fsync.

The shared **commit sequence** is `create-temp(O_EXCL,0600) → write_all → flush → fsync →
rename → [dir-fsync] → TempGuard cleanup-on-failure`. The data-loss-critical failure
points (ENOSPC mid-write, fsync EIO, rename fail, leftover temp) are unreachable by any
test. `file::save_atomic` runs inside `do_save_to`'s Job closure (worker thread,
save.rs:69), so any seam impl must be `Send`. The merge closure (save.rs:76, main
thread) updates `saved_version`/`stored_fp` ONLY on `Ok` — a `SaveError` keeps the buffer
dirty.

## Decisions (from brainstorm)

1. **Targeted seam, not a full FileSystem trait, not cfg hooks.** A small `Fs` trait over
   the atomic-write ops only; public signatures unchanged. The seam lives at the shared
   commit core `atomic_replace(fs: &dyn Fs, …)`; tests exercise it by calling
   `atomic_replace` directly with a `FaultFs`, and the three public primitives call it with
   a locally-constructed `&RealFs` (no `*_with` public variants; no fault fs threaded
   through `do_save_to`). Reads/metadata/session-load faults are OUT of scope (low
   data-loss risk, already graceful: a failed read → open error, a failed session-load →
   empty session).
2. **One shared, fault-tested commit core** (`atomic_replace`): all three primitives route
   their durability commit through it, so every durability write inherits one
   proven-failure-safe path. Per-function pre-checks (symlink/skip-unchanged) and the
   mode-policy choice (`PreserveExistingOr` vs `Fixed`) stay at the caller; the mode-APPLY
   step (`set_mode`) lives inside the core so it is fault-injectable.
3. **Pin the current dir-fsync-failure behavior** (return `SaveError::Io`) — tested, not
   changed. (dir-fsync runs AFTER rename, so the new content is already live; this is a
   durability-not-atomicity edge.)

## Components

### 1. `Fs` seam trait + `RealFs` (new `wordcartel/src/fsx.rs`, or in `file.rs`)

```rust
pub(crate) trait WriteSync {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
    fn set_mode(&self, mode: u32) -> std::io::Result<()>; // Unix set_permissions on temp; no-op off-unix
    fn sync_all(&self) -> std::io::Result<()>;
}
pub(crate) trait Fs {
    /// O_EXCL create at `path` with `mode` (Unix); returns a write+sync handle.
    fn create_excl(&self, path: &std::path::Path, mode: u32) -> std::io::Result<Box<dyn WriteSync>>;
    /// Best-effort mode of an existing file (Unix); `None` if absent/unreadable/off-unix.
    fn existing_mode(&self, path: &std::path::Path) -> Option<u32>;
    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()>;
    fn sync_dir(&self, dir: &std::path::Path) -> std::io::Result<()>;
    fn remove_file(&self, path: &std::path::Path) -> std::io::Result<()>;
}
```
`RealFs` is a zero-size unit (`Send`) delegating to `std::fs` exactly as today
(`OpenOptions::create_new(true).mode(0600)`, `write_all`, `flush`,
`set_permissions(from_mode(..))`, `sync_all`, `fs::rename`, `fs::remove_file`;
`existing_mode` = `fs::metadata(path).ok().map(|m| m.permissions().mode())`).
**`RealFs::sync_dir` matches today's dir-open swallow** (file.rs:231-232, 273-275):
`File::open(dir)` → `Ok(f) => f.sync_all()`, `Err(_) => Ok(())` — a dir that can't be opened
is NOT an error (only a successful-open `sync_all` failure propagates). Trait-object
(`&dyn Fs`) + boxed `WriteSync` handle — no associated types (so `&dyn Fs` works).

**Threading model (pinned).** `RealFs` is constructed **locally at each call site**:
`save_atomic` / `save_atomic_bytes` / `write_atomic` each instantiate `RealFs` and call
`atomic_replace(&RealFs, …)` themselves. The `&dyn Fs` borrow never crosses a thread
boundary and is never stored — in `do_save_to` the worker-thread `Job` closure calls
`save_atomic`, which builds its own `RealFs` inside the closure. So **no
`Arc<dyn Fs + Send + Sync>` is needed**; the seam stays a borrow. `FaultFs` is used **only**
in direct unit tests of `atomic_replace`, never threaded through `do_save_to` (Component 5
drives a real failure instead). The boxed handle is `Box<dyn WriteSync + 'static>`, so it
cannot borrow from `&self`; the `FaultFs` handle **owns its injected config by value** (the
`FaultAt` plus a write-budget counter) — no shared `Arc<Mutex<_>>` state.

### 2. The shared commit core: `atomic_replace`

```rust
/// How the temp's final mode is chosen before rename.
pub(crate) enum ModePolicy {
    /// Always this mode (save_atomic_bytes, swap::write_atomic — both 0600 today).
    Fixed(u32),
    /// Preserve the EXISTING target's mode if present, else this fallback
    /// (save_atomic today: existing_mode.unwrap_or(0o600)).
    PreserveExistingOr(u32),
}
pub(crate) struct WriteOpts { pub mode: ModePolicy, pub dir_fsync: bool }

/// [resolve PreserveExistingOr mode-read] → create-temp(O_EXCL,0600) → write_all →
/// set_mode → flush → fsync → close → rename → [dir-fsync], with TempGuard cleanup of the
/// temp on ANY failure. The handle is closed before rename (consistent across all paths;
/// required on Windows). The single durability-critical sequence; all three primitives
/// route their commit here. Never leaves a temp behind on failure; never half-replaces the
/// target (rename is the all-or-nothing commit).
pub(crate) fn atomic_replace(
    fs: &dyn Fs,
    final_path: &std::path::Path,
    bytes: &[u8],
    opts: WriteOpts,
) -> std::io::Result<()>;
```
- **Mode step (matches `save_atomic` today, including order).** For `PreserveExistingOr(f)`,
  the metadata read `fs.existing_mode(final_path).unwrap_or(f)` happens **at the start of
  `atomic_replace`, before temp creation** — matching today's `save_atomic`, which captures
  `existing_mode` before `create_temp` (file.rs:177-182), so the chmod-race window is
  identical (a failed read just falls back, exactly as today's `fs::metadata(path).ok()`).
  `Fixed(m)` needs no read → `m`. The temp is always created `0600` (O_EXCL); then after
  `write_all`, `handle.set_mode(resolved)` is applied **before** `flush`/`sync_all`/`rename`
  — the same ordering as file.rs today (lines 204-218). `set_mode` is a seam call, so it is
  fault-injectable.
- Temp name unifies to one O_EXCL scheme (e.g. `.{name}.wcartel-{pid}-{counter}.tmp`) in
  the target's parent dir. (Internal/transient; safe to unify across the three.) **Note:**
  this **intentionally changes `swap::write_atomic`'s collision behavior** from "one fixed
  `.{name}.tmp-{pid}` name, fail on a pre-existing temp" to the counter-retry scheme — a
  deliberate robustness improvement, not a regression. `find_orphan_scratch_swap` globs
  final `scratch-*.swp` names, **not** temp names, so orphan detection is unaffected.
  **Also intentional:** `swap::write_atomic` today has **no `TempGuard`** — a write/flush/
  sync failure leaves its temp behind (swap.rs:198-212). Routing it through `atomic_replace`
  gives it the same `TempGuard` cleanup as the file-save paths, so swap inherits the
  no-litter guarantee. This is an improvement, not a behavior a test should pin to the old
  (litter-on-failure) semantics.
- `TempGuard` (RAII) removes the temp via `fs.remove_file` on any early return; disarmed
  only after a successful `rename` (the temp is gone after rename anyway).
- `file::save_atomic` (→ `PreserveExistingOr(0o600)`, `dir_fsync: true`),
  `file::save_atomic_bytes` (→ `Fixed(0o600)`, `dir_fsync: true`), `swap::write_atomic`
  (→ `Fixed(0o600)`, `dir_fsync: false`) keep their pre-checks, build `bytes`, then call
  `atomic_replace(&RealFs, …, opts)`; they map `io::Error` → their existing error types
  (`SaveError`/`io::Error`) exactly as before.

### 3. `FaultFs` (test-only)

```rust
#[cfg(test)]
enum FaultAt { Create, Write { after: usize }, SetMode, Flush, Sync, Rename, SyncDir }
#[cfg(test)] struct FaultFs { inner: RealFs, fail: FaultAt }
```
Wraps `RealFs`; injects exactly one failure: `Create`/`SetMode`/`Flush`/`Sync`/`Rename`/
`SyncDir` return an `io::Error` (e.g. `ErrorKind::Other` / a simulated `StorageFull`);
`Write { after: n }` writes `n` real bytes to the temp then returns `ErrorKind::WriteZero` /
ENOSPC (partial write). All other ops delegate to `RealFs` so the real temp dir reflects
reality. `SetMode` is the fault point for the mode-preservation step (it occurs before the
rename, so a `SetMode` failure must leave the original intact + no litter, just like the
other pre-rename faults). **No `Remove` variant:** `fs.remove_file` is only called by
`TempGuard::drop` on a pre-rename early return — which is itself caused by the one injected
fault — so the single-fault model can never make the cleanup-remove ALSO fail. The
`remove_file` seam method is still exercised (and must succeed) in every pre-rename fault
test, where it is what makes the no-litter assertion hold.

### 4. Fault tests — the durability invariants

Each test runs `atomic_replace(&FaultFs{fail: …}, &target, new_bytes, opts)` against a
real private temp dir where `target` already holds known original bytes, then asserts:
- **Atomicity:** after a `Create`/`Write{partial}`/`SetMode`/`Flush`/`Sync`/`Rename`
  failure, the ORIGINAL file is **byte-identical** to before (the commit is all-or-nothing;
  the rename is the commit point; a failure before it never touches the target).
- **No litter:** **no `.tmp` file remains** in the dir after any failure (`TempGuard`
  ran via `fs.remove_file`).
- **Error surfaced:** `atomic_replace` returns `Err` (the right `io::ErrorKind`); never
  silently `Ok`.
- **Partial write** (`Write { after: k }`, ENOSPC mid-content): caught → `Err`, original
  intact, no litter.
- **dir-fsync (pinned):** a `SyncDir` failure returns `Err` (current behavior) even though
  the rename already succeeded — assert `Err` AND that the target now holds the NEW bytes
  (the write committed; only the durability barrier failed). This documents the pinned
  semantics.
- **Success path:** `RealFs` (no fault) replaces the target with the new bytes and leaves
  no temp — pins `RealFs ≡ atomic_replace ≡ today`.
- **Mode preservation (success path):** with `PreserveExistingOr(0o600)` over a target that
  already has a non-default mode (e.g. `0644`), the replaced file retains the EXISTING
  mode, not `0600`; over a non-existent target it lands `0600`. Pins that the `ModePolicy`
  step reproduces `save_atomic`'s current `existing_mode.unwrap_or(0o600)` behavior. (Unix
  only.)

### 5. End-to-end: a failed save keeps the buffer dirty

The `do_save_to` merge (save.rs:76) updates `saved_version` only on `Ok`. An existing test
already covers this (save.rs ~339, via symlink refusal — Unix-only). Keep it; if a second
real-failure case is wanted, drive one with a path whose **parent is a file** (temp
creation then fails immediately because the parent is not a directory — deterministic on
Linux). No `do_save_to_with` job seam is needed: a real failure suffices, keeping the seam
internal. These end-to-end cases are Unix-conditional (`#[cfg(unix)]`), matching the
existing symlink test; that is acceptable for this Linux-targeted project.

## Data flow (production unchanged)

`do_save_to` Job (worker) → `file::save_atomic(path, content)` → pre-checks →
`atomic_replace(&RealFs, …)` → `JobResult` → merge (main, marks saved only on `Ok`).
Swap/session/recovery writes likewise route their commit through `atomic_replace(&RealFs)`.

## Error handling

Every `atomic_replace` failure on a data-loss-critical op (create/write/set_mode/flush/
sync/rename) returns `Err` with the real `io::Error`, mapped by each caller to its existing
error type. No partial replace; no temp litter. A failed save therefore keeps the buffer
dirty (no false "saved"). The **one** non-erroring case is a dir that can't be opened for
`sync_dir`, which `RealFs` swallows (`Ok(())`) exactly as today — a successful-open
`sync_all` failure still propagates as `Err` (the pinned dir-fsync semantics). `FaultFs::SyncDir`
still returns `Err` to exercise the propagation path.

## Testing strategy

The Component-4 fault tests are the deliverable (atomicity incl. `SetMode` + no-litter +
error-surfaced + partial-write + pinned dir-fsync + success-path + mode-preservation
equivalence). The existing real-temp-dir
round-trip / no-litter tests for `save_atomic` / `save_atomic_bytes` / `write_atomic`
stay GREEN (they pin `RealFs ≡ today`). Plus the Component-5 save-failure-keeps-dirty
check.

## Out of scope (deferred)

- Reads / metadata / `session-load` / `find_orphan` faults (low data-loss risk; already
  graceful) — not seamed.
- The pandoc **export** path (`export.rs`/`app.rs` `TempReady`) uses a raw `std::fs::rename`
  of a pandoc-produced temp, NOT any of the three atomic primitives. It is export *output*
  (regenerable), not document/session/swap durability — out of scope here. The seam covers
  the three durability writers; it is not a blanket "every filesystem write is fault-tested"
  claim.
- The full `FileSystem` trait threaded through all callers (rejected in favor of the
  internal seam).
- Changing the dir-fsync-failure semantics (pinned).
- Resource caps (M5), fuzz (M7), the rest of async panic isolation (M4).

## New code surface (checklist for the plan)

- `wordcartel/src/fsx.rs` (new) OR `file.rs`: `Fs` + `WriteSync` traits (incl. `set_mode`,
  `existing_mode`); `RealFs`; `ModePolicy`; `WriteOpts`;
  `atomic_replace(fs, final_path, bytes, opts)`; `#[cfg(test)] FaultFs` (incl. `SetMode`).
- `wordcartel/src/file.rs`: `save_atomic` (→ `PreserveExistingOr(0o600)`) /
  `save_atomic_bytes` (→ `Fixed(0o600)`) route their commit through
  `atomic_replace(&RealFs, …)` (pre-checks preserved, error mapping preserved).
- `wordcartel/src/swap.rs`: `write_atomic` routes through `atomic_replace(&RealFs, …)`
  (`Fixed(0o600)`, `dir_fsync: false`).
- Tests: the Component-4 fault tests (incl. `SetMode` + mode-preservation success path);
  Component-5 save-failure-keeps-dirty.
