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

1. **Targeted seam via inner functions (not a full FileSystem trait, not cfg hooks).**
   A small `Fs` trait over the atomic-write ops only; public signatures unchanged; tests
   call the `*_with(fs, …)` inner form. Reads/metadata/session-load faults are OUT of
   scope (low data-loss risk, already graceful: a failed read → open error, a failed
   session-load → empty session).
2. **One shared, fault-tested commit core** (`atomic_replace`): all three primitives route
   their durability commit through it, so every durability write inherits one
   proven-failure-safe path. Per-function pre-checks (symlink/skip-unchanged/mode) stay.
3. **Pin the current dir-fsync-failure behavior** (return `SaveError::Io`) — tested, not
   changed. (dir-fsync runs AFTER rename, so the new content is already live; this is a
   durability-not-atomicity edge.)

## Components

### 1. `Fs` seam trait + `RealFs` (new `wordcartel/src/fsx.rs`, or in `file.rs`)

```rust
pub(crate) trait WriteSync {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
    fn sync_all(&self) -> std::io::Result<()>;
}
pub(crate) trait Fs {
    /// O_EXCL create at `path` with `mode` (Unix); returns a write+sync handle.
    fn create_excl(&self, path: &std::path::Path, mode: u32) -> std::io::Result<Box<dyn WriteSync>>;
    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()>;
    fn sync_dir(&self, dir: &std::path::Path) -> std::io::Result<()>;
    fn remove_file(&self, path: &std::path::Path) -> std::io::Result<()>;
}
```
`RealFs` is a zero-size unit (`Send`) delegating to `std::fs` exactly as today
(`OpenOptions::create_new(true).mode(0600)`, `write_all`, `flush`, `sync_all`,
`fs::rename`, `File::open(dir).sync_all()`, `fs::remove_file`). Trait-object (`&dyn Fs`)
+ boxed `WriteSync` handle — no associated types (so `&dyn Fs` works).

### 2. The shared commit core: `atomic_replace`

```rust
pub(crate) struct WriteOpts { pub mode: u32, pub dir_fsync: bool }

/// Create-temp(O_EXCL) → write_all → flush → fsync → rename → [dir-fsync], with
/// TempGuard cleanup of the temp on ANY failure. The single durability-critical
/// sequence; all three primitives route their commit here. Never leaves a temp behind
/// on failure; never half-replaces the target (rename is the all-or-nothing commit).
pub(crate) fn atomic_replace(
    fs: &dyn Fs,
    final_path: &std::path::Path,
    bytes: &[u8],
    opts: WriteOpts,
) -> std::io::Result<()>;
```
- Temp name unifies to one O_EXCL scheme (e.g. `.{name}.wcartel-{pid}-{counter}.tmp`) in
  the target's parent dir. (Internal/transient; safe to unify across the three.)
- `TempGuard` (RAII) removes the temp via `fs.remove_file` on any early return; disarmed
  only after a successful `rename` (the temp is gone after rename anyway).
- `file::save_atomic`, `file::save_atomic_bytes`, `swap::write_atomic` keep their
  pre-checks, build `bytes`, then call `atomic_replace(&RealFs, …, opts)`; they map
  `io::Error` → their existing error types (`SaveError`/`io::Error`) exactly as before.

### 3. `FaultFs` (test-only)

```rust
#[cfg(test)]
enum FaultAt { Create, Write { after: usize }, Flush, Sync, Rename, SyncDir, Remove }
#[cfg(test)] struct FaultFs { inner: RealFs, fail: FaultAt }
```
Wraps `RealFs`; injects exactly one failure: `Create`/`Flush`/`Sync`/`Rename`/`SyncDir`/
`Remove` return an `io::Error` (e.g. `ErrorKind::Other` / a simulated `StorageFull`);
`Write { after: n }` writes `n` real bytes to the temp then returns `ErrorKind::WriteZero`
/ ENOSPC (partial write). All other ops delegate to `RealFs` so the real temp dir
reflects reality.

### 4. Fault tests — the durability invariants

Each test runs `atomic_replace(&FaultFs{fail: …}, &target, new_bytes, opts)` against a
real private temp dir where `target` already holds known original bytes, then asserts:
- **Atomicity:** after a `Create`/`Write{partial}`/`Flush`/`Sync`/`Rename` failure, the
  ORIGINAL file is **byte-identical** to before (the commit is all-or-nothing; the rename
  is the commit point; a failure before it never touches the target).
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

### 5. End-to-end: a failed save keeps the buffer dirty

The `do_save_to` merge (save.rs:76) updates `saved_version` only on `Ok`. Verify there is
an existing test that a save **failure** leaves the buffer **dirty** (and surfaces an
error status). If absent, add one driving a real failure (e.g. saving into a path whose
parent is a file, or — if cleanly threadable without violating "no call-site threading" —
a fault-injected `do_save_to_with`). Prefer the real-failure approach to keep the seam
internal; only add a `*_with` job seam if the real failure can't be provoked
deterministically.

## Data flow (production unchanged)

`do_save_to` Job (worker) → `file::save_atomic(path, content)` → pre-checks →
`atomic_replace(&RealFs, …)` → `JobResult` → merge (main, marks saved only on `Ok`).
Swap/session/recovery writes likewise route their commit through `atomic_replace(&RealFs)`.

## Error handling

Every `atomic_replace` failure returns `Err` with the real `io::Error`, mapped by each
caller to its existing error type. No failure is swallowed; no partial replace; no temp
litter. A failed save therefore keeps the buffer dirty (no false "saved").

## Testing strategy

The Component-4 fault tests are the deliverable (atomicity + no-litter + error-surfaced +
partial-write + pinned dir-fsync + success-path equivalence). The existing real-temp-dir
round-trip / no-litter tests for `save_atomic` / `save_atomic_bytes` / `write_atomic`
stay GREEN (they pin `RealFs ≡ today`). Plus the Component-5 save-failure-keeps-dirty
check.

## Out of scope (deferred)

- Reads / metadata / `session-load` / `find_orphan` faults (low data-loss risk; already
  graceful) — not seamed.
- The full `FileSystem` trait threaded through all callers (rejected in favor of the
  internal seam).
- Changing the dir-fsync-failure semantics (pinned).
- Resource caps (M5), fuzz (M7), the rest of async panic isolation (M4).

## New code surface (checklist for the plan)

- `wordcartel/src/fsx.rs` (new) OR `file.rs`: `Fs` + `WriteSync` traits; `RealFs`;
  `WriteOpts`; `atomic_replace(fs, final_path, bytes, opts)`; `#[cfg(test)] FaultFs`.
- `wordcartel/src/file.rs`: `save_atomic` / `save_atomic_bytes` route their commit through
  `atomic_replace(&RealFs, …)` (pre-checks preserved, error mapping preserved).
- `wordcartel/src/swap.rs`: `write_atomic` routes through `atomic_replace(&RealFs, …)`
  (dir_fsync: false).
- Tests: the Component-4 fault tests; Component-5 save-failure-keeps-dirty.
