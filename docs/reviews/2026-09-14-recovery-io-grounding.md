# Recovery IO feasibility — 2026-09-14

Bounded read-only audit of design revision 2 against the pinned local Rust 1.96 standard-library source and the current `fsx::Fs` seam. No compiler, cargo, runtime probe, source implementation edits or delegation were used. This checks feasibility and interface obligations, not the lifecycle design gate.

## Outcome

The proposed primitives are implementable safely on the current Linux target without changing the toolchain. Standard-library file locking needs no dependency. **The whole no-follow IO boundary is not currently implementable as a portable, dependency-free wrapper around the existing Fs methods.** The plan must choose explicit platform support and ancestor-path semantics. The design's preserve-and-report Unsupported fallback is appropriate outside validated support.

Local primary sources are under `/home/jkeim/.rustup/toolchains/1.96.0-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/`; the relative paths and symbols below locate them.

## Primitive audit

| Primitive | Local std evidence | Required contract |
| --- | --- | --- |
| Nonblocking exclusive lease | `fs.rs::File::try_lock`, stable since 1.89.0; `sys/fs/unix.rs::File::try_lock` uses `flock(LOCK_EX | LOCK_NB)` on Linux; Windows uses `LockFileEx` fail-immediately | Distinguish `TryLockError::WouldBlock` from `Error`; only the former is ordinary busy. Open lock read/write, never truncate an existing lock file. |
| Lease lifetime | `fs.rs::File::try_lock` documentation: released after all duplicated/inherited descriptors close or explicit unlock | Encapsulate one acquired File in a shared owned guard. Never call `try_lock` again on the same or cloned descriptor: std explicitly declares that behavior platform-dependent, possibly deadlocking. Drop the last guard to release; do not expose early unlock. |
| Exclusive owner directory | `fs.rs::DirBuilder::create`: nonrecursive existing directory is an error; `sys/fs/unix.rs::DirBuilder::mkdir` calls mkdir | Use nonrecursive `DirBuilder`, Unix `DirBuilderExt::mode(0o700)`, retry only AlreadyExists. Recursive create is not exclusive allocation. Existing directories are never adopted by allocator. |
| Strict directory sync | `fs.rs::File::sync_all`; Unix delegates fsync; Windows delegates FlushFileBuffers | Propagate directory-open and sync failure. Existing `RealFs::sync_dir` intentionally swallows open failure and cannot establish the new receipt. Sync ancestor links introduced by provisioning, not only owner/checkpoint entries. |
| No-follow leaf open | `os/unix/fs.rs::OpenOptionsExt::custom_flags`, stable 1.10; docs demonstrate `libc::O_NOFOLLOW` | Open with O_NOFOLLOW plus O_NONBLOCK, then inspect metadata on that same opened handle and require a regular file before bounded read. O_NONBLOCK avoids hanging while opening a FIFO before metadata can reject it. A stat-before-open sequence is not equivalent. |
| No-follow directory open | Unix custom flags support O_DIRECTORY and O_NOFOLLOW | Leaf flags only constrain the final component. State root, recovery root and owner-directory ancestor policy must be explicit. Ordinary Path joins plus a no-follow leaf do not by themselves meet an unrestricted “no symlink traversal” guarantee. |
| Read, verify and sync the same saved file | Existing `Fs::read_capped` returns detached bytes; existing `WriteSync` only represents writable temp handles | Recovery save receipts need a new handle abstraction or a combined read/compare/sync method. Reading via one Fs call and separately reopening to sync does not prove the same file was synced. Read-only handle sync may be unsupported on some platforms; preserve checkpoint then. |

The lease's shared type must be **Send + Sync** if it is put in an Arc and captured by Send job closures; specifying only an owned Send trait object is insufficient for `Arc<dyn Lease>`. A uniquely moved `Box<dyn Lease + Send>` is a different valid interface but does not alone supply the stated Buffer/job shared ownership. Keep the File implementation opaque and use Rust's automatic traits; no unsafe Send implementation is necessary.

## Dependency and platform choices

`wordcartel/src/lib.rs` forbids unsafe code. Safe std OpenOptions APIs can consume platform constants without adding unsafe blocks. The crate currently has no direct `libc`, `rustix` or `windows-sys` dependency, although all appear transitively in Cargo.lock. Transitive presence does not expose a usable crate import.

For a Linux/Unix implementation with a trusted, validated private hierarchy, add an explicit target-specific `libc` dependency for O_NOFOLLOW/O_NONBLOCK/O_DIRECTORY constants and keep operations in safe std APIs. Do not hardcode Linux flag values for all Unix platforms. This changes a direct dependency declaration, not the toolchain; the spec's “no dependency change” should remain limited to the lock primitive if this route is chosen.

If the guarantee must resist concurrent ancestor-directory replacement, use a safe anchored-directory API such as rustix openat operations with held directory descriptors, and similarly anchor creation, rename and unlink. Leaf no-follow plus a pre-open symlink check cannot supply that stronger guarantee. The design currently excludes arbitrary external writers bypassing the protocol, so it can explicitly trust the validated 0700 hierarchy and still reject existing symlink owner entries; that boundary should be stated rather than implied. Cooperative participants must never rename/remove owner directories or replace lock inodes.

On Windows, local `sys/fs/windows.rs` shows directory opens use FILE_FLAG_BACKUP_SEMANTICS and no-follow/reparse opens use FILE_FLAG_OPEN_REPARSE_POINT. `os/windows/fs.rs::OpenOptionsExt::custom_flags` exposes the mechanism; constants require a platform dependency or named platform-specific definitions. A basic `File::open(directory)` is not a valid portable directory-sync implementation. Whether FlushFileBuffers succeeds for the resulting directory handle is not established by reading Linux source or by a Linux probe. Either implement and validate Windows semantics explicitly or return typed Unsupported for destructive recovery cleanup there. Never report a durable-success receipt from a skipped sync.

## Existing seam changes

`wordcartel/src/fsx.rs::Fs` currently provides exclusive **file** creation, capped following reads, following metadata, rename, best-effort directory sync, remove and listing. It provides neither owner-dir allocation nor leases nor no-follow opened read handles. `WriteSync` has no Send bound and should not accidentally become the shared lease interface.

Affected implementors if Fs is extended: `RealFs`, `test_support::FaultFs`, three local `settings.rs::FailFs` implementations and `file_browser.rs::CountingFs`. A separate recovery trait/facade is feasible, but must remain injectable where Ctx currently carries `Arc<dyn Fs + Send + Sync>`; the plan must name how the injected backend reaches recovery rather than falling back to RealFs. Defaults, if used for old test wrappers, should return Unsupported, never pretend success.

Use `DirEntryInfo.raw_name` to preserve names when enumerating. Its `name` is lossy. Existing `Fs::stat` and listing classification follow symlinks for type resolution and therefore cannot substitute for validation of the opened handle.

## Validation obligations

- Same-process separate handle and cross-process lock contention; lock released by process death and last shared guard drop; no reacquisition on cloned handle.
- Lock file symlink, FIFO, directory and special-file rejection without hanging; Unsupported distinct from WouldBlock.
- Exclusive owner collision retries; creation failure does not publish a checkpoint; no reuse of tombstone directories or lock inodes.
- Deterministic faults at strict directory **open** and **sync**, file sync, rename and each newly created ancestor link. Source retirement never follows an uncertain successor.
- Same-handle read/compare/sync receipt and symlink/nonregular target rejection; tests should show safe fallback when sync is unsupported.
- No-follow ancestor policy tests matching the chosen boundary, including symlink owner entries. Do not label simple precheck tests as proof against concurrent malicious directory substitution.
- Platform cfg compilation and runtime tests for every platform claimed supported. Preserve-only Unsupported behavior is itself testable and should be explicit.

No mathematical uniqueness claim is needed for random owner suggestions because exclusive directory creation decides ownership. No runtime durability or portability claim is established by this static audit.

## Follow-up: exact Windows dependency and lease-drop boundary

The existing lockfile and local Cargo registry contain `windows-sys 0.61.2`. A target-specific direct dependency with `version = "=0.61.2"` and feature `Win32_Storage_FileSystem` exposes `Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ATTRIBUTE_REPARSE_POINT}`. The feature hierarchy includes Win32 Foundation. These are constants consumed by safe std APIs; importing them does not require an unsafe block. This is source feasibility only, not Windows compilation/runtime validation.

Lease release by RAII is not literally free of filesystem operations: local `os/fd/owned.rs::OwnedFd::drop` calls close, and `os/windows/io/handle.rs::OwnedHandle::drop` calls CloseHandle. Neither performs an explicit fsync; std's `File::sync_all` documentation says ordinary drop need not wait for durable data, but provides no hard bounded-latency guarantee. The plan should either permit this minimal handle-close operation on foreground last-guard drop, while forbidding sync/unlink/scan work in Drop, or keep final lease ownership on the worker and enqueue its release. Do not claim automatic File destruction does no IO.
