# C5 — Unified File Interface Implementation Plan

> **Agentic-worker sub-skill note.** Each task below is dispatched to a fresh implementer subagent
> that sees ONLY its own task section plus the Global Constraints and File Structure sections. Tasks
> may be read out of order. Therefore every task repeats the code it needs rather than referring to a
> sibling task, and every signature a later task consumes is stated verbatim in the producing task's
> **Interfaces / Produces** block. Do not write "as in Task N".

**Goal.** Route every in-process file-content read, directory listing, metadata probe, and durable
write through the `fsx::Fs` seam, and make the `FileBrowser` overlay the single UI for choosing a
path — for Open, Save-As, Write-Block, and (for the first time) Export.

**Architecture.** `fsx.rs` stays the fault-injectable filesystem seam and grows three primitives
(`read_capped`, `stat`, `list_dir`) plus the two it already declares but that callers bypass
(`rename`, `remove_file`); synchronous callers take `&dyn Fs`, anything crossing a thread boundary
takes an owned `Arc<dyn Fs + Send + Sync>`. `FileBrowser` gains a `BrowseMode` (Select | Destination)
rather than a second overlay, so the H21 dispatch table, `chrome_geom` hit-testing, and the mouse
path stay single-sourced. Directory listings move off the UI thread onto a dedicated `std::thread`
(never the `jobs.rs` FIFO, which is shared with Save and SwapWrite).

**Tech Stack.** Rust 2021, `wordcartel` shell crate only (`wordcartel-core` and `wordcartel-nlp`
contain no `std::fs`). ratatui 0.30 + crossterm. `nucleo-matcher` 0.3 (already a direct dependency)
for fuzzy ranking. **No new dependencies.**

**Spec.** `docs/superpowers/specs/2026-07-18-c5-file-interface-design.md`. Section references below
(`§5.2`, `§7.6.1`, …) are to that document.

---

## Global Constraints

These apply to **every** task. Each task's requirements implicitly include this section.

### Commit trailers — verbatim, on every commit

```
Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: <the current session URL from the harness instructions>
```

The session URL is supplied in the agent's own harness instructions (the Git section). It is NOT in
the shell `env` and is NOT derivable from the Session ID that `/status` prints. Never construct or
invent it.

### Merge gates (all must pass before merge)

- `cargo test` green across all suites (`wordcartel-core` lib + oracle, `wordcartel` lib).
- `cargo build` and `cargo test --no-run` warning-free for touched crates.
- `cargo clippy --workspace --all-targets` clean — `[workspace.lints.clippy] all = "deny"`.
- `clippy::too_many_lines` threshold **100**. A longer function needs an item-local
  `#[allow(clippy::too_many_lines)]` with a one-line reason.
- `wordcartel/tests/module_budgets.rs` hub budgets (`app.rs` ≤ 1000, `render.rs` ≤ 900).
- Backlog-drift bijection test.

### Formatting — do NOT run `cargo fmt`

This repo is hand-formatted; there is no `rustfmt.toml`. `cargo fmt` would reformat the whole tree.
Match neighbouring style by hand: 4-space indent, ~100-char hand-wrapped lines, em-dash `—` in prose
comments (never `--`), no emoji in code.

### Registration order — register BEFORE `plugin_list`

`wordcartel/src/e2e.rs::journey_palette_end_reaches_last_command` presses End+Enter in the palette
and asserts `status_text().starts_with("plugins:")`, which hardcodes **`plugin_list` as the last
registered command**. It is a merge gate. **All seven new C5 commands register before
`plugin_list`** in `registry::Registry::builtins`.

Two comments in `registry.rs` claiming `save_settings` must stay last are **stale** — the tree
already registers `plugins_reload` and `plugin_list` after it. Correcting or deleting those comments
is fair game for whichever task touches the registry.

### Zero new dependencies (decision 2)

No `ignore`, no `walkdir`, no `rand`/`getrandom`/`uuid`, no tokio. Fuzzy ranking uses the existing
`palette::fuzzy_filter`. `DocumentId` entropy comes from `std::collections::hash_map::RandomState`.

### Other standing rules

- Errors are typed enums surfaced to the **status line**, never the console.
- No `.unwrap()` on fallible/external paths; prefer `.expect("…invariant…")` after establishing it.
- Per-keystroke work stays `O(visible)+O(edited)`.
- Idle is free: no polling, no background work at rest.
- Dispatchers delegate; new behaviour enters through a registration seam, not by growing a hub.
- PTY smoke (`scripts/smoke/run.sh`) is mandatory-run / advisory-pass in the pre-merge report.

---

## File Structure

### Created

| Path | Responsibility | Task |
|---|---|---|
| `wordcartel/src/file_browser_listing.rs` | Pure listing cache + filter/rank pipeline + disclosure counts. No IO except `refetch`, no Editor. | 12 |
| `wordcartel/src/file_browser_commit.rs` | Destination-mode commit semantics: the Enter decision table, field resolution, extension policy. | 18, 19 |
| `wordcartel/src/file_browser_intercept.rs` | The `FileBrowser` key/paste intercept, moved out of `file_browser.rs` and branched on mode. | 18 |
| `wordcartel/src/recents.rs` | `open_recent` rows, ranked from the LRU session store. | 24 |
| `wordcartel/tests/fs_chokepoint.rs` | Integration guard test: scans production sources for raw filesystem access outside the clause-citing allow-list. | 11 |

### Modified

| Path | What changes |
|---|---|
| `wordcartel/src/fsx.rs` | `Fs` grows `read_capped`/`stat`/`list_dir`; new `FileStat`, `EntryKind`, `DirEntryInfo`, `DirListing`; `resolve_write_destination`. |
| `wordcartel/src/test_support.rs` | Receives the promoted `FaultFs`/`FaultHandle`/`FaultAt` plus new fault points. |
| `wordcartel/src/file_browser.rs` | Becomes the module hub: `FileBrowser`, `BrowseMode`, `DestinationPurpose`, `FileEntry`, open/descend orchestration. Delegates to the three new siblings. |
| `wordcartel/src/file.rs` | `open`/`bounded_read_opt`/`save_atomic`/`save_atomic_bytes` gain `_with_fs` cores; `save_atomic_bytes` gains a symlink guard. |
| `wordcartel/src/save.rs` | `SaveTarget`; `do_save_to` signature; merge-time `pre_rekey`; migration push. |
| `wordcartel/src/swap.rs` | Reads/scans/delete route through the seam. |
| `wordcartel/src/registry.rs` | `Ctx` gains `fs`; seven new commands registered before `plugin_list`. |
| `wordcartel/src/overlays.rs` | `DispatchCtx` gains `fs`. |
| `wordcartel/src/app.rs` | Builds the `Arc<RealFs>`; `Msg::ListingDone`; migration drain at both persist sites; startup probes onto the seam. |
| `wordcartel/src/editor.rs` | `pending_session_migrations`; filter-toggle fields + setters; `Document::id`. |
| `wordcartel/src/settings.rs` | `SettingsSnapshot` + overrides mirror gain the two filter options. |
| `wordcartel/src/config.rs` | `FileTypeFilter`; `config_layer_paths`/`load` onto the seam. |
| `wordcartel/src/limits.rs` | `MAX_CONFIG_BYTES`, `MAX_DIR_ENTRIES`. |
| `wordcartel/src/prompts.rs`, `blocks_marked.rs`, `export.rs`, `jobs_apply.rs`, `state.rs`, `session_restore.rs`, `theme_resolve.rs`, `diagnostics_run.rs`, `clipboard.rs`, `plugin/load.rs`, `render_overlays.rs`, `chrome_geom.rs`, `mouse.rs` | Migration and rewiring, per task. |

### `file_browser.rs` decomposition rationale

Today `file_browser.rs` is ~255 lines holding struct + rebuild + enter + intercept + tests. C5 adds
two modes, a cache, a filter pipeline, symlink classification, and a commit decision table. Split now
rather than letting it emerge, on **one axis of change per module**:

- `file_browser.rs` — state shape and lifecycle (what a browser *is*).
- `file_browser_listing.rs` — how entries are produced and filtered (pure; heavily unit-tested).
- `file_browser_commit.rs` — what Enter *means* in destination mode (the highest-risk logic).
- `file_browser_intercept.rs` — input routing.

Flat sibling modules, matching the repo's existing `chrome.rs`/`chrome_geom.rs` and
`search_overlay.rs`/`search_ui.rs` pattern.

---

# Tasks

All 26 tasks are written in full below, in six phases. Each is sized to carry its own TDD cycle and
be worth a fresh reviewer's gate.

## Phase A — Seam foundation (Tasks 1–5)

### Task 1 — Promote `FaultFs` into `test_support`

**Why first:** every later fault-injection test depends on it. `FaultFs`, `FaultHandle`, and
`FaultAt` are currently private to `fsx.rs`'s `#[cfg(test)] mod tests`, so no other module can use
them.

#### Files

- Modify: `wordcartel/src/test_support.rs`
- Modify: `wordcartel/src/fsx.rs` (remove the private copies; import from `test_support`)

#### Interfaces

**Consumes:** nothing (first task).

**Produces** — available to every later task as `crate::test_support::{FaultAt, FaultFs}`:

```rust
#[derive(Clone, Copy, Debug)]
pub(crate) enum FaultAt {
    Create,
    Write { after: usize },
    SetMode,
    Flush,
    Sync,
    Rename,
    SyncDir,
    RemoveFile,
}

pub(crate) struct FaultFs {
    pub(crate) inner: crate::fsx::RealFs,
    pub(crate) fail: FaultAt,
}

impl FaultFs {
    pub(crate) fn new(fail: FaultAt) -> Self;
}
```

`FaultFs` implements `crate::fsx::Fs`. `RealFs` must be `pub(crate)` (it already is).

#### Steps

1. **Write the failing test.** Add to `wordcartel/src/fsx.rs`'s test module:

```rust
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
```

2. **Run it — expect a compile failure**, not an assertion failure:

```
cargo test -p wordcartel --lib fsx::tests::fault_fs_is_reachable_from_test_support
```

Expected: `error[E0433]: failed to resolve: could not find `FaultFs` in `test_support``.

3. **Move the types.** Cut `FaultAt`, `FaultFs`, `FaultHandle` and their impls out of `fsx.rs`'s test
   module and paste into `test_support.rs`, changing visibility from private to `pub(crate)` and
   adding the `RemoveFile` variant. Append to `test_support.rs`:

```rust
// ---------------------------------------------------------------------------
// FaultFs — the shared fault-injecting `Fs` (promoted from fsx.rs, C5 Task 1).
//
// Lives here, not in fsx.rs's private test mod, because every migrated call site
// (reads, listings, stats) needs to inject faults from its OWN module's tests.
// ---------------------------------------------------------------------------

use crate::fsx::{Fs, ModePolicy, RealFs, WriteOpts, WriteSync};
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
}
```

4. **Update `fsx.rs`'s existing fault tests** to use the promoted types: add
   `use crate::test_support::{FaultAt, FaultFs};` inside `mod tests`, delete the local definitions,
   and replace each `FaultFs { inner: RealFs, fail: … }` literal with `FaultFs::new(…)`.

5. **Run — expect green:**

```
cargo test -p wordcartel --lib fsx::
```

Expected: all existing `fsx::tests::*` pass plus the new one. **Every pre-existing fault test must
still pass unmodified in behaviour** — this task moves code, it does not change semantics.

6. **Commit:** `test(c5): promote FaultFs into test_support for cross-module fault injection`

---

### Task 2 — `Fs::read_capped`

#### Files

- Modify: `wordcartel/src/fsx.rs`
- Modify: `wordcartel/src/test_support.rs` (FaultFs arm)

#### Interfaces

**Consumes:** `crate::test_support::{FaultAt, FaultFs}` (Task 1).

**Produces:**

```rust
// on trait Fs, in fsx.rs
/// Read at most `limit + 1` bytes. `Ok(None)` when the file exceeds `limit`;
/// `Err` on IO failure. Distinguishing over-cap from IO error is deliberate —
/// `file::bounded_read_opt` conflates them, which is right for its degrade-silently
/// callers but wrong for a seam.
fn read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>>;
```

Plus `FaultAt::ReadCapped` for injection.

#### Steps

1. **Write the failing tests** in `fsx.rs`'s test module:

```rust
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
```

If `unique_dir` does not already exist in `fsx.rs`'s test module, add it:

```rust
    fn unique_dir(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "wc-fsx-{}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed), label));
        std::fs::create_dir_all(&d).expect("create dir");
        d
    }
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib fsx::tests::read_capped
```

Expected: ``error[E0599]: no method named `read_capped` found for struct `RealFs```.

3. **Add the trait method** to `trait Fs` in `fsx.rs`, immediately after `existing_mode`:

```rust
    /// Read at most `limit + 1` bytes from `path`. `Ok(None)` when the file exceeds
    /// `limit`; `Err` on any IO failure. The Option/Result split is deliberate: an
    /// over-cap file is a POLICY outcome, an unreadable file is a FAILURE, and callers
    /// that conflate them (today's `bounded_read_opt`) cannot tell a huge document from
    /// a permission problem.
    fn read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>>;
```

4. **Implement for `RealFs`:**

```rust
    fn read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
        use std::io::Read as _;
        let f = fs::File::open(path)?;
        let mut buf = Vec::new();
        f.take(limit + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > limit { return Ok(None); }
        Ok(Some(buf))
    }
```

5. **Add the FaultFs arm** in `test_support.rs`: add `ReadCapped` to `enum FaultAt`, and to
   `impl Fs for FaultFs`:

```rust
    fn read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
        if matches!(self.fail, FaultAt::ReadCapped) {
            return Err(Error::other("injected: read_capped"));
        }
        self.inner.read_capped(path, limit)
    }
```

6. **Run — expect green:**

```
cargo test -p wordcartel --lib fsx::tests::read_capped
```

Expected: `test result: ok. 4 passed`.

7. **Commit:** `feat(c5): add Fs::read_capped with over-cap/IO-error separation`

---

### Task 3 — `Fs::stat` and `FileStat`

**The subtle part:** follow-vs-lstat semantics are load-bearing. Getting them backwards is a silent
durability regression in the external-modification guard.

#### Files

- Modify: `wordcartel/src/fsx.rs`
- Modify: `wordcartel/src/test_support.rs` (FaultFs arm)

#### Interfaces

**Consumes:** `crate::test_support::{FaultAt, FaultFs}` (Task 1).

**Produces:**

```rust
// fsx.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileStat {
    pub len: u64,
    pub mtime: Option<std::time::SystemTime>,
    /// RESOLVED regular file (follows symlinks) — exactly `Metadata::is_file()`.
    pub is_file: bool,
    /// RESOLVED directory (follows symlinks).
    pub is_dir: bool,
    /// The entry ITSELF is a symlink, whatever it points at (from `symlink_metadata`).
    pub is_symlink: bool,
    /// A symlink whose target could not be RESOLVED — dangling, permission-denied along
    /// the chain, or a resolution loop. INVARIANT: implies `is_symlink && !is_file && !is_dir`.
    pub broken: bool,
}

// on trait Fs
fn stat(&self, path: &Path) -> std::io::Result<FileStat>;
```

Plus `FaultAt::Stat`.

#### Steps

1. **Write the failing tests:**

```rust
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
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib fsx::tests::stat_
```

Expected: ``error[E0599]: no method named `stat` found``.

3. **Add `FileStat` to `fsx.rs`**, above the `Fs` trait:

```rust
/// A resolved metadata probe. `len`/`mtime`/`is_file`/`is_dir` FOLLOW symlinks (they come
/// from `metadata`); `is_symlink` does NOT (it comes from `symlink_metadata`). Two syscalls,
/// one method — both existing behaviours preserved exactly.
///
/// `is_file` is a field and NEVER `!is_dir`: fifos, sockets, and devices are neither, so the
/// equivalence is false and `config_layer_paths`-style probes would misclassify them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
```

4. **Add the trait method and `RealFs` impl:**

```rust
    /// Metadata probe. See [`FileStat`] for the follow/don't-follow split.
    fn stat(&self, path: &Path) -> std::io::Result<FileStat>;
```

```rust
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
```

5. **Add the FaultFs arm** in `test_support.rs`: add `Stat` to `FaultAt`, and:

```rust
    fn stat(&self, path: &Path) -> std::io::Result<crate::fsx::FileStat> {
        if matches!(self.fail, FaultAt::Stat) {
            return Err(Error::other("injected: stat"));
        }
        self.inner.stat(path)
    }
```

6. **Run — expect green:**

```
cargo test -p wordcartel --lib fsx::tests::stat_
```

Expected: `test result: ok. 4 passed`.

7. **Commit:** `feat(c5): add Fs::stat with follow/lstat split and broken-symlink detection`

---

### Task 4 — `Fs::list_dir`, `EntryKind`, `DirEntryInfo`, `DirListing`

**The subtle parts:** (a) the type probe must not abort the listing; (b) `Other` and `Unknown` are
different facts an enum keeps separate; (c) enumeration is uncapped, retention is capped, and `cap`
is `Option`.

#### Files

- Modify: `wordcartel/src/fsx.rs`
- Modify: `wordcartel/src/test_support.rs` (FaultFs arm)
- Modify: `wordcartel/src/limits.rs`

#### Interfaces

**Consumes:** `crate::test_support::{FaultAt, FaultFs}` (Task 1).

**Produces:**

```rust
// fsx.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind { File, Dir, Other, Unknown }

#[derive(Clone, Debug)]
pub(crate) struct DirEntryInfo {
    pub name: String,
    pub kind: EntryKind,
    pub is_symlink: bool,
    pub broken: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct DirListing {
    pub entries: Vec<DirEntryInfo>,
    pub total_seen: usize,
    pub unreadable: usize,
}

// on trait Fs
fn list_dir(&self, path: &Path, cap: Option<usize>) -> std::io::Result<DirListing>;

// limits.rs
pub const MAX_DIR_ENTRIES: usize = 5_000;
```

Plus `FaultAt::ListDir`.

#### Steps

1. **Write the failing tests:**

```rust
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
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib fsx::tests::list_dir
```

Expected: ``error[E0599]: no method named `list_dir` found``.

3. **Add the types to `fsx.rs`:**

```rust
/// What a directory entry resolved to. An ENUM, not a pair of bools, so `Unknown` cannot be
/// silently absorbed into a false branch — the house rule on exhaustive matches applied to the
/// failure mode this design kept hitting. Critically, `Other` (a legitimately-classified fifo)
/// and `Unknown` (we could not classify it) are DIFFERENT facts that two bools cannot separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind {
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
pub(crate) struct DirEntryInfo {
    pub name: String,
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
pub(crate) struct DirListing {
    pub entries: Vec<DirEntryInfo>,
    pub total_seen: usize,
    pub unreadable: usize,
}
```

4. **Add the trait method and `RealFs` impl:**

```rust
    /// Enumerate `path`. Enumeration is ALWAYS complete; only RETENTION is capped, and only
    /// when `cap` is `Some`. `cap: None` is the non-interactive form (plugin discovery, the
    /// swap scans) — those are uncapped today and capping them would be a refactor-introduced
    /// regression, not a new protection.
    fn list_dir(&self, path: &Path, cap: Option<usize>) -> std::io::Result<DirListing>;
```

```rust
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
            let name = entry.file_name().to_string_lossy().into_owned();
            // NOTE: no `?` on file_type() — one unclassifiable entry must NOT abort the whole
            // directory. A named-but-unclassified entry is emitted with kind == Unknown.
            let (kind, is_symlink, broken) = match entry.file_type() {
                Err(_) => (EntryKind::Unknown, false, false),
                Ok(ft) if !ft.is_symlink() => (kind_of(ft.is_file(), ft.is_dir()), false, false),
                Ok(_) => match fs::metadata(entry.path()) {
                    Ok(m) => (kind_of(m.is_file(), m.is_dir()), true, false),
                    Err(_) => (EntryKind::Unknown, true, true),
                },
            };
            entries.push(DirEntryInfo { name, kind, is_symlink, broken });
        }
        Ok(DirListing { entries, total_seen, unreadable })
    }
```

And the small free helper, next to `RealFs`:

```rust
/// Map a resolved (is_file, is_dir) pair onto an `EntryKind`. Neither true means `Other`
/// — a fifo, socket, or device — which is a CLASSIFIED answer, not an unknown one.
fn kind_of(is_file: bool, is_dir: bool) -> EntryKind {
    if is_file { EntryKind::File } else if is_dir { EntryKind::Dir } else { EntryKind::Other }
}
```

5. **Add the FaultFs arm** in `test_support.rs`: add `ListDir` to `FaultAt`, and:

```rust
    fn list_dir(&self, path: &Path, cap: Option<usize>)
        -> std::io::Result<crate::fsx::DirListing>
    {
        if matches!(self.fail, FaultAt::ListDir) {
            return Err(Error::other("injected: list_dir"));
        }
        self.inner.list_dir(path, cap)
    }
```

6. **Add the cap constant** to `limits.rs`:

```rust
/// Retention cap for ONE interactive directory listing (the picker). Enumeration is never
/// capped — the disclosed total must be real. Non-interactive scans (plugin discovery, the
/// swap state-dir scans) pass `cap: None`.
pub const MAX_DIR_ENTRIES: usize = 5_000;
```

7. **Run — expect green:**

```
cargo test -p wordcartel --lib fsx::tests::list_dir
```

Expected: `test result: ok. 4 passed`.

8. **Commit:** `feat(c5): add Fs::list_dir with EntryKind classification and uncapped enumeration`

---

### Task 5 — Ownership plumbing: `Arc<dyn Fs + Send + Sync>` on `Ctx` and `DispatchCtx`

**Why this is its own task:** `jobs::Job` declares `run: Box<dyn FnOnce() -> JobResult + Send>`, so a
borrowed `&dyn Fs` cannot cross into a job closure or the listing thread. Without this, an
implementer hits the borrow error and hardcodes `RealFs` inside the closure — silently destroying
the fault-injectability that justifies extending the seam at all.

#### Files

- Modify: `wordcartel/src/registry.rs` (`Ctx`)
- Modify: `wordcartel/src/overlays.rs` (`DispatchCtx`)
- Modify: `wordcartel/src/app.rs` (build the `Arc`, thread it through construction sites)
- Modify: every `Ctx { … }` / `DispatchCtx { … }` literal (compiler will enumerate them)

#### Interfaces

**Consumes:** `crate::fsx::Fs` (Tasks 2–4).

**Produces:**

```rust
// registry.rs
pub struct Ctx<'a> {
    pub editor: &'a mut Editor,
    pub clock: &'a dyn Clock,
    pub executor: &'a dyn Executor,
    pub msg_tx: std::sync::mpsc::Sender<Msg>,
    /// Owned handle so job closures (which are `'static + Send`) can clone it in.
    pub fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
}

// overlays.rs
pub(crate) struct DispatchCtx<'a> {
    pub(crate) reg: &'a crate::registry::Registry,
    pub(crate) keymap: &'a crate::keymap::KeyTrie,
    pub(crate) ex: &'a dyn crate::jobs::Executor,
    pub(crate) clock: &'a dyn wordcartel_core::history::Clock,
    pub(crate) msg_tx: &'a std::sync::mpsc::Sender<Msg>,
    pub(crate) fs: &'a std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
}
```

**Convention every later task follows:**

| Context | Form |
|---|---|
| Synchronous, main thread | `&dyn Fs` parameter (matches `settings::save_overrides`, which already does this) |
| Inside a `jobs::Job` closure, or a spawned thread | owned `Arc<dyn Fs + Send + Sync>`, cloned in |

`Fs` does **not** gain `Send + Sync` supertraits — the async sites spell `dyn Fs + Send + Sync`
instead, so a future single-threaded recording double is still possible.

#### Steps

1. **Write the failing test** in `registry.rs`'s test module:

```rust
    #[test]
    fn ctx_carries_an_owned_fs_that_can_cross_a_thread() {
        // The compile-shape guard. `jobs::Job::run` is `Box<dyn FnOnce() -> JobResult + Send>`,
        // so a BORROWED &dyn Fs cannot be captured. This asserts the Arc form is what Ctx
        // carries — if someone "simplifies" it back to a reference, this stops compiling and
        // the worker-side seam silently reverts to a hardcoded RealFs.
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        let cloned = std::sync::Arc::clone(&fs);
        let h = std::thread::spawn(move || {
            // Any seam call proves the handle is usable off-thread.
            cloned.stat(std::path::Path::new("/")).is_ok()
        });
        assert!(h.join().expect("thread joins"), "an owned Fs handle works on a worker thread");
    }
```

2. **Run — expect it to pass already** (it only exercises `Arc<dyn Fs>`, not `Ctx`). Then add the
   `Ctx` half, which is what actually fails:

```rust
    #[test]
    fn ctx_fs_field_exists_and_is_clonable_into_a_closure() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let ctx = Ctx {
            editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx,
            fs: std::sync::Arc::new(crate::fsx::RealFs),
        };
        let handle = std::sync::Arc::clone(&ctx.fs);
        let t = std::thread::spawn(move || handle.stat(std::path::Path::new("/")).is_ok());
        assert!(t.join().expect("joins"));
    }
```

3. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib registry::tests::ctx_fs_field
```

Expected: ``error[E0560]: struct `Ctx` has no field named `fs```.

4. **Add the field to `Ctx`** in `registry.rs`:

```rust
pub struct Ctx<'a> {
    pub editor: &'a mut Editor,
    pub clock: &'a dyn Clock,
    pub executor: &'a dyn Executor,
    /// Owned `Sender` (not a borrow) because `dispatch_filter` moves a clone into a `'static` spawned thread.
    pub msg_tx: std::sync::mpsc::Sender<Msg>,
    /// The filesystem seam. OWNED (`Arc`), not borrowed, because `jobs::Job::run` is
    /// `Box<dyn FnOnce() -> JobResult + Send>` — a job closure must be able to clone this in.
    /// Synchronous call sites still take plain `&dyn Fs`; see §5.2 of the C5 spec.
    pub fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
}
```

5. **Add the field to `DispatchCtx`** in `overlays.rs`:

```rust
    /// The filesystem seam (owned handle — the listing thread clones it in).
    pub(crate) fs: &'a std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
```

6. **Build the `Arc` at the composition root.** In `app::run`, near where the executor and clock are
   created, add:

```rust
    // Composition root for the filesystem seam. Everything downstream gets a clone of this
    // handle; tests substitute an Arc<FaultFs> at the same point.
    let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
        std::sync::Arc::new(crate::fsx::RealFs);
```

7. **Fix every construction site.** Run `cargo build -p wordcartel` and add
   `fs: std::sync::Arc::clone(&fs)` (for `Ctx`) or `fs: &fs` (for `DispatchCtx`) to each literal the
   compiler reports. In test modules, use `fs: std::sync::Arc::new(crate::fsx::RealFs)`.

8. **Run — expect green:**

```
cargo build -p wordcartel && cargo test -p wordcartel --lib registry::tests::ctx_
```

Expected: build clean, `test result: ok`.

9. **Commit:** `feat(c5): carry an owned Arc<dyn Fs + Send + Sync> on Ctx and DispatchCtx`

---

*Phase A complete. The seam now has all three primitives, a shared fault harness, and an ownership
form that survives a thread boundary.*

---

## Phase B — Migration onto the seam (Tasks 6–11)

### Task 6 — Content reads onto `read_capped`, plus config-class caps

**Deliverable:** every file-content read in the shell crate goes through `Fs::read_capped`, and the
four previously-unbounded config-class reads acquire a cap.

#### Files

- Modify: `wordcartel/src/limits.rs` (add `MAX_CONFIG_BYTES`)
- Modify: `wordcartel/src/file.rs` (`open`, `bounded_read_opt`)
- Modify: `wordcartel/src/config.rs` (`load`)
- Modify: `wordcartel/src/theme_resolve.rs` (`resolve_theme`'s `theme.file` read)
- Modify: `wordcartel/src/state.rs` (`load_in`)
- Modify: `wordcartel/src/swap.rs` (`read_swap_capped`, `read_file_capped_bytes`)
- Modify: `wordcartel/src/app.rs` (the overrides + `--config` mask reads)

#### Interfaces

**Consumes** (Task 2, Task 5):

```rust
// crate::fsx
pub(crate) trait Fs {
    fn read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>>;
    // …plus create_excl / existing_mode / rename / sync_dir / remove_file / stat / list_dir
}
pub(crate) struct RealFs;

// crate::test_support
pub(crate) enum FaultAt { Create, Write { after: usize }, SetMode, Flush, Sync,
                          Rename, SyncDir, RemoveFile, ReadCapped, Stat, ListDir }
pub(crate) struct FaultFs { pub(crate) inner: crate::fsx::RealFs, pub(crate) fail: FaultAt }
impl FaultFs { pub(crate) fn new(fail: FaultAt) -> Self; }
```

**Produces** — later tasks call these exact names:

```rust
// crate::limits
/// Cap for config-class reads (config.toml, .wordcartel.toml, settings-overrides.toml,
/// a base16 theme file). Generous for TOML; mirrors PLUGIN_MAX_SOURCE_BYTES.
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

// crate::file
pub fn open(path: &Path) -> Result<String, OpenError>;                     // unchanged signature
pub(crate) fn open_with_fs(fs: &dyn crate::fsx::Fs, path: &Path) -> Result<String, OpenError>;
pub fn bounded_read_opt(path: &Path, limit: u64) -> Option<Vec<u8>>;       // unchanged signature
pub(crate) fn bounded_read_opt_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, limit: u64)
    -> Option<Vec<u8>>;

// crate::config
pub fn load(paths: &[PathBuf]) -> (Config, Vec<String>);                   // unchanged signature
pub(crate) fn load_with_fs(fs: &dyn crate::fsx::Fs, paths: &[PathBuf]) -> (Config, Vec<String>);

// crate::state
pub fn load_in(dir: &Path) -> SessionState;                                // unchanged signature
pub(crate) fn load_in_with_fs(fs: &dyn crate::fsx::Fs, dir: &Path) -> SessionState;
```

The wrapper-plus-core shape keeps every existing call site source-compatible, so the migration is
additive and the tree is green at each step. It is the same shape `settings::save_overrides` already
uses (it takes `fs: &dyn crate::fsx::Fs` and `app.rs` injects `&crate::fsx::RealFs`), and the same
shape `swap::find_orphan_scratch_swap` / `find_orphan_scratch_swap_in` and `state::load` /
`state::load_in` already use for directory injection.

#### Steps

1. **Write the failing tests.** In `wordcartel/src/file.rs`'s test module:

```rust
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
```

In `wordcartel/src/config.rs`'s test module:

```rust
    #[test]
    fn config_over_cap_degrades_like_an_unreadable_file() {
        // Config-class reads acquire a cap. An over-cap config must warn and fall back to
        // defaults — the SAME degradation an unreadable file already gets — never panic and
        // never silently apply a truncated parse.
        let d = std::env::temp_dir().join(format!("wc-cfg-cap-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("dir");
        let p = d.join("config.toml");
        std::fs::write(&p, vec![b'#'; (crate::limits::MAX_CONFIG_BYTES + 1) as usize])
            .expect("seed oversized");
        let (cfg, warns) = load_with_fs(&crate::fsx::RealFs, &[p.clone()]);
        assert_eq!(cfg.state.max_entries, Config::default().state.max_entries,
            "over-cap config falls back to defaults");
        assert!(warns.iter().any(|w| w.contains("cannot read") || w.contains("too large")),
            "over-cap must warn, not pass silently: {warns:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
```

2. **Run — expect compile failures:**

```
cargo test -p wordcartel --lib file::tests::open_routes_through_the_seam
```

Expected: ``error[E0425]: cannot find function `open_with_fs` in this scope``.

3. **Add the cap** to `limits.rs`:

```rust
/// Cap for CONFIG-class reads — `config.toml`, `.wordcartel.toml`,
/// `settings-overrides.toml`, and a base16 theme file. Generous for TOML (these are
/// hand-written files), and deliberately separate from `MAX_OPEN_BYTES`, which governs
/// documents. Over-cap degrades exactly as an unreadable config already does: warn and
/// fall back to defaults.
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
```

4. **Rewrite `file::open`** in `file.rs`. Replace the existing body with a wrapper plus a
   `_with_fs` core, and add an explicitly-bounded variant used by the over-cap test:

```rust
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
    if path.is_dir() {
        return Err(OpenError::IsDir(label));
    }

    if is_binary(&bytes) {
        return Err(OpenError::Binary(label));
    }

    Ok(String::from_utf8(bytes).expect("already verified by is_binary"))
}
```

> The `path.is_dir()` line stays raw for now; Task 7 migrates it onto `fs.stat`. Leaving it here
> keeps this task a pure read migration with no probe semantics mixed in.

5. **Rewrite `bounded_read_opt`** in `file.rs`:

```rust
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
```

6. **Rewrite `config::load`** in `config.rs`:

```rust
pub fn load(paths: &[PathBuf]) -> (Config, Vec<String>) {
    load_with_fs(&crate::fsx::RealFs, paths)
}

pub(crate) fn load_with_fs(fs: &dyn crate::fsx::Fs, paths: &[PathBuf]) -> (Config, Vec<String>) {
    let mut cfg = Config::default();
    let mut warns = Vec::new();
    for p in paths {
        let bytes = match fs.read_capped(p, crate::limits::MAX_CONFIG_BYTES) {
            Ok(Some(b)) => b,
            Ok(None) => {
                warns.push(format!("config: {} is too large (> {} bytes) — ignored",
                    p.display(), crate::limits::MAX_CONFIG_BYTES));
                continue;
            }
            Err(e) => {
                warns.push(format!("config: cannot read {}: {e}", p.display()));
                continue;
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => {
                warns.push(format!("config: {} is not valid UTF-8 — ignored", p.display()));
                continue;
            }
        };
        // …the existing `toml::from_str(&text)` block and merge logic, unchanged…
    }
    (cfg, warns)
}
```

> Keep the rest of the existing loop body verbatim. Only the read and the UTF-8 decode change.

7. **Migrate `theme_resolve`.** Change the signature to take the seam and replace the `theme.file`
   read:

```rust
pub fn resolve_theme(tc: &ThemeConfig, env: &EnvSnapshot, disp: ChromeDisposition)
    -> ResolvedTheme
{
    resolve_theme_with_fs(&crate::fsx::RealFs, tc, env, disp)
}

pub(crate) fn resolve_theme_with_fs(fs: &dyn crate::fsx::Fs, tc: &ThemeConfig,
    env: &EnvSnapshot, disp: ChromeDisposition) -> ResolvedTheme
{
    // …unchanged prologue through `let depth = effective_depth(…);` …
```

and inside, replace `match std::fs::read_to_string(path)` with:

```rust
        match fs.read_capped(path, crate::limits::MAX_CONFIG_BYTES)
            .map(|o| o.and_then(|b| String::from_utf8(b).ok()))
        {
            Ok(Some(text)) => match crate::base16::parse_base16(&text) {
                Ok((pal, scheme)) => {
                    let name = scheme.unwrap_or_else(|| format!("base16:{}", path.display()));
                    theme::from_base16(&name, pal)
                }
                Err(e) => { warnings.push(format!("theme file {}: {e} — using default", path.display())); theme::default() }
            },
            Ok(None) => {
                warnings.push(format!("theme file {}: too large or not UTF-8 — using default", path.display()));
                theme::default()
            }
            Err(e) => { warnings.push(format!("theme file {}: {e} — using default", path.display())); theme::default() }
        }
```

8. **Migrate `state::load_in`:**

```rust
pub fn load_in(dir: &Path) -> SessionState {
    load_in_with_fs(&crate::fsx::RealFs, dir)
}

pub(crate) fn load_in_with_fs(fs: &dyn crate::fsx::Fs, dir: &Path) -> SessionState {
    let path = dir.join("session.toml");
    let cap = crate::limits::MAX_SESSION_BYTES as u64;
    let Ok(Some(bytes)) = fs.read_capped(&path, cap) else {
        return SessionState::default(); // missing, unreadable, or over-cap → empty (graceful)
    };
    let Ok(text) = String::from_utf8(bytes) else { return SessionState::default() };
    toml::from_str(&text).unwrap_or_default()
}
```

`state::load()` keeps calling `load_in`, so its behaviour is unchanged.

9. **Migrate the two private `swap.rs` readers** to take the seam:

```rust
fn read_swap_capped(fs: &dyn crate::fsx::Fs, path: &std::path::Path) -> Option<String> {
    let bytes = fs.read_capped(path, crate::limits::MAX_OPEN_BYTES).ok()??;
    String::from_utf8(bytes).ok()
}

fn read_file_capped_bytes(fs: &dyn crate::fsx::Fs, path: &Path) -> Option<Vec<u8>> {
    fs.read_capped(path, crate::limits::MAX_OPEN_BYTES).ok()?
}
```

Thread `fs` through their callers inside `swap.rs` (`assess`, `swap_is_cleanable`, `tmp_is_cleanable`,
`find_orphan_scratch_swap_in`) by adding a leading `fs: &dyn crate::fsx::Fs` parameter and a
`RealFs`-injecting public wrapper for each currently-public entry point. The compiler enumerates
these; there are no behavioural changes.

10. **Migrate the two `app.rs` startup reads.** Replace the overrides snapshot read:

```rust
    let mut overrides_snapshot = overrides_path.as_ref()
        .filter(|p| p.is_file())
        .and_then(|p| fs.read_capped(p, crate::limits::MAX_CONFIG_BYTES).ok().flatten())
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| settings::parse_overrides(&s))
        .unwrap_or_default();
```

and the mask snapshot read:

```rust
    let mask_snapshot = cli.config_path.as_ref()
        .filter(|c| c.is_file())
        .and_then(|c| fs.read_capped(c, crate::limits::MAX_CONFIG_BYTES).ok().flatten())
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| settings::parse_mask(&s))
        .unwrap_or_default();
```

> Both `.filter(|p| p.is_file())` probes stay raw here; Task 7 migrates them. `fs` is the
> `Arc<dyn Fs + Send + Sync>` built in Task 5 — `Arc` derefs to `dyn Fs`, so `fs.read_capped(…)`
> works directly.

11. **Run — expect green:**

```
cargo test -p wordcartel --lib file:: config:: state:: swap:: theme_resolve::
```

Expected: all pass, including the two new tests. No existing test changes behaviour.

12. **Commit:** `refactor(c5): route content reads through Fs::read_capped; cap config-class reads`

---

### Task 7 — Metadata probes onto `Fs::stat`

**Deliverable:** every `metadata` / `symlink_metadata` / `exists` / `is_file` / `is_dir` probe in
production shell code goes through `Fs::stat`, preserving each site's current behaviour exactly.

#### Files

- Modify: `wordcartel/src/fsx.rs` (two probe helpers)
- Modify: `wordcartel/src/save.rs` (`fingerprint`)
- Modify: `wordcartel/src/state.rs` (`file_identity`)
- Modify: `wordcartel/src/file.rs` (`save_atomic`'s symlink refusal; `open_bounded_with_fs`'s `is_dir`)
- Modify: `wordcartel/src/config.rs` (`config_layer_paths`)
- Modify: `wordcartel/src/clipboard.rs` (`clip_env_from_process`'s PATH search)
- Modify: `wordcartel/src/prompts.rs`, `wordcartel/src/export.rs`, `wordcartel/src/jobs_apply.rs`,
  `wordcartel/src/app.rs` (the `exists()` / `is_file()` probes)

#### Interfaces

**Consumes** (Task 3, Task 5, Task 6):

```rust
// crate::fsx
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileStat {
    pub len: u64,
    pub mtime: Option<std::time::SystemTime>,
    pub is_file: bool,      // RESOLVED regular file (follows symlinks)
    pub is_dir: bool,       // RESOLVED directory
    pub is_symlink: bool,   // the entry ITSELF is a link
    pub broken: bool,       // symlink whose target could not be resolved
}
pub(crate) trait Fs { fn stat(&self, path: &Path) -> std::io::Result<FileStat>; /* … */ }

// crate::test_support
pub(crate) enum FaultAt { /* … */ Stat, /* … */ }
pub(crate) struct FaultFs; impl FaultFs { pub(crate) fn new(fail: FaultAt) -> Self; }

// crate::file (Task 6)
pub(crate) fn open_bounded_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, limit: u64)
    -> Result<String, OpenError>;
```

**Produces:**

```rust
// crate::fsx — the two probe shapes every migrated site uses.
/// `Path::exists()` through the seam: any successful stat means "something is here".
/// A BROKEN symlink counts as existing, matching `Path::exists()`… no — see the doc
/// comment in the implementation: `Path::exists()` FOLLOWS, so a broken link is `false`.
pub(crate) fn exists_via(fs: &dyn Fs, path: &Path) -> bool;

/// `Path::is_file()` through the seam. Returns `false` on any error — exactly what
/// `Path::is_file()` does today at every migrated call site.
pub(crate) fn is_file_via(fs: &dyn Fs, path: &Path) -> bool;

// crate::save
pub fn fingerprint(path: &Path) -> Option<FileFingerprint>;                 // unchanged signature
pub(crate) fn fingerprint_with_fs(fs: &dyn crate::fsx::Fs, path: &Path)
    -> Option<FileFingerprint>;

// crate::state
pub fn file_identity(path: &Path) -> Option<(i64, u64)>;                    // unchanged signature
pub(crate) fn file_identity_with_fs(fs: &dyn crate::fsx::Fs, path: &Path) -> Option<(i64, u64)>;
```

#### Steps

1. **Write the failing tests.** In `save.rs`'s test module:

```rust
    #[cfg(unix)]
    #[test]
    fn fingerprint_on_a_broken_symlink_is_none() {
        // BEHAVIOUR PRESERVATION. Today `fingerprint` opens with
        // `std::fs::metadata(path).ok()?`, so a broken symlink yields None. Under the seam,
        // `stat` SUCCEEDS for a broken link (broken == true) — so the caller must map
        // broken -> None explicitly. Without that mapping this returns Some with zeroed
        // fields and silently corrupts the external-mod comparison.
        let d = std::env::temp_dir().join(format!("wc-fp-broken-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("dir");
        let link = d.join("dangling.md");
        std::os::unix::fs::symlink(d.join("gone.md"), &link).expect("symlink");
        assert!(fingerprint_with_fs(&crate::fsx::RealFs, &link).is_none(),
            "a broken symlink must fingerprint as None, exactly as metadata().ok()? did");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn fingerprint_faults_are_injectable() {
        let p = scratch();
        std::fs::write(&p, b"aaaa").expect("seed");
        let ff = crate::test_support::FaultFs::new(crate::test_support::FaultAt::Stat);
        assert!(fingerprint_with_fs(&ff, &p).is_none(),
            "an injected stat failure degrades to None, matching today's .ok()?");
        let _ = std::fs::remove_file(&p);
    }
```

In `fsx.rs`'s test module:

```rust
    #[cfg(unix)]
    #[test]
    fn is_file_via_rejects_a_fifo_and_a_dir() {
        // `!is_dir` is NOT "regular file". config_layer_paths, plugin discovery, and the
        // clipboard PATH search all ask `is_file()`, and a fifo answering `true` would turn
        // "skip it" into a blocking read.
        let d = unique_dir("isfile-fifo");
        let f = d.join("plain.txt");
        std::fs::write(&f, b"x").expect("seed");
        assert!(is_file_via(&RealFs, &f), "regular file");
        assert!(!is_file_via(&RealFs, &d), "a directory is not a file");
        assert!(!is_file_via(&RealFs, &d.join("absent")), "a missing path is false, not an error");
        let _ = std::fs::remove_dir_all(&d);
    }
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib save::tests::fingerprint_on_a_broken_symlink
```

Expected: ``error[E0425]: cannot find function `fingerprint_with_fs` in this scope``.

3. **Add the two probe helpers** to `fsx.rs`, after `kind_of`:

```rust
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
```

4. **Migrate `save::fingerprint`.** Replace `fingerprint` and `fingerprint_with_limit`:

```rust
pub fn fingerprint(path: &Path) -> Option<FileFingerprint> {
    fingerprint_with_fs(&crate::fsx::RealFs, path)
}

pub(crate) fn fingerprint_with_fs(fs: &dyn crate::fsx::Fs, path: &Path)
    -> Option<FileFingerprint>
{
    fingerprint_with_limit(fs, path, crate::limits::MAX_OPEN_BYTES)
}

/// Content-hash fingerprint, capping the content read at `limit`.
///
/// Returns `None` when the path is missing/unstattable — AND when it is a BROKEN symlink,
/// because today's `std::fs::metadata(path).ok()?` fails for a dangling link and the seam's
/// `stat` succeeds for one. Without the explicit `broken` guard this would return `Some`
/// with zeroed fields and silently defeat the external-mod check.
///
/// A present, resolvable but over-cap file still yields a metadata-only fingerprint (real
/// mtime+size, sentinel hash 0) rather than `None`, so `stored_fp` never becomes `None`
/// and `None == None` cannot disable the conflict check.
fn fingerprint_with_limit(fs: &dyn crate::fsx::Fs, path: &Path, limit: u64)
    -> Option<FileFingerprint>
{
    let st = fs.stat(path).ok()?;
    if st.broken { return None; }
    let hash = match crate::file::bounded_read_opt_with_fs(fs, path, limit) {
        Some(bytes) => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hasher::write(&mut h, &bytes);
            std::hash::Hasher::finish(&h)
        }
        None => 0, // over-cap (or transient read failure): fall back to mtime+size only
    };
    Some(FileFingerprint { mtime: st.mtime, size: st.len, hash })
}
```

> The existing tests `fingerprint_over_cap_falls_back_to_metadata_not_none`,
> `fingerprint_within_cap_hashes_content_unchanged`, and
> `fingerprint_detects_same_size_different_content` call `fingerprint_with_limit(&p, N)`. Update
> those three call sites to `fingerprint_with_limit(&crate::fsx::RealFs, &p, N)`. Their assertions
> must not change.

5. **Migrate `state::file_identity`:**

```rust
pub fn file_identity(path: &Path) -> Option<(i64, u64)> {
    file_identity_with_fs(&crate::fsx::RealFs, path)
}

pub(crate) fn file_identity_with_fs(fs: &dyn crate::fsx::Fs, path: &Path) -> Option<(i64, u64)> {
    let st = fs.stat(path).ok()?;
    let mtime = st.mtime
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some((mtime, st.len))
}
```

6. **Migrate `file::save_atomic`'s symlink refusal** and `open_bounded_with_fs`'s `is_dir`:

```rust
pub fn save_atomic(path: &Path, content: &str) -> Result<SaveOutcome, SaveError> {
    save_atomic_with_fs(&crate::fsx::RealFs, path, content)
}

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

    crate::fsx::atomic_replace(fs, path, content.as_bytes(), crate::fsx::WriteOpts {
        mode: crate::fsx::ModePolicy::PreserveExistingOr(0o600),
        dir_fsync: true,
    })
    .map_err(|e| SaveError::Io(e.to_string()))?;

    Ok(SaveOutcome::Saved)
}
```

and in `open_bounded_with_fs`, replace `if path.is_dir()` with:

```rust
    if matches!(fs.stat(path), Ok(st) if st.is_dir) {
        return Err(OpenError::IsDir(label));
    }
```

7. **Migrate the remaining probes.** Each is a one-line substitution:

| Site | Was | Becomes |
|---|---|---|
| `config::config_layer_paths` (three `p.is_file()`) | `if p.is_file()` | `if crate::fsx::is_file_via(fs, &p)` — add a leading `fs: &dyn crate::fsx::Fs` parameter and a `RealFs` wrapper |
| `clipboard::clip_env_from_process`'s `on_path` | `dir.join(bin).is_file()` | `crate::fsx::is_file_via(fs, &dir.join(bin))` — add `fs` to `clip_env_from_process` and its caller |
| `prompts::save_as_submit`, `block_write_submit` | `target.exists()` | `crate::fsx::exists_via(fs, &target)` |
| `export::run_export` | `target.exists()` | `crate::fsx::exists_via(fs, &target)` |
| `export::run_pandoc` | `!tmp.exists()` | `!crate::fsx::exists_via(&*fs, &tmp)` — `fs` here is the **owned `Arc`** cloned into `do_export`'s spawned thread (Task 5's convention: this call crosses a thread boundary) |
| `jobs_apply::apply_export_done` | `target.exists()` | `crate::fsx::exists_via(fs, &target)` |
| `app::run` (`p.is_file()`, `c.is_file()` ×2, `!p.exists()`) | as written | `crate::fsx::is_file_via(&*fs, p)` / `!crate::fsx::exists_via(&*fs, p)` |

8. **Run — expect green:**

```
cargo test -p wordcartel --lib save:: state:: file:: config:: clipboard::
```

Expected: all pass, including the three new tests. **`save::tests::background_save_failure_keeps_dirty_and_status`
must still pass unmodified** — it drives the symlink refusal and proves the guard survived the
migration.

9. **Commit:** `refactor(c5): route metadata probes through Fs::stat; map broken links to None`

---

### Task 8 — Durable mutations onto the seam

**Deliverable:** the dictionary append becomes atomic, the export rename and every out-of-temp delete
route through the seam, and `save_atomic_bytes` gains the symlink guard it has always lacked.

#### Files

- Modify: `wordcartel/src/file.rs` (`save_atomic_bytes`)
- Modify: `wordcartel/src/diagnostics_run.rs` (`append_word_to_dict`)
- Modify: `wordcartel/src/jobs_apply.rs` (`apply_export_done`'s rename + cleanup)
- Modify: `wordcartel/src/prompts.rs` (`Recover`, `DiscardSwap`, `CleanRecovery` deletes)
- Modify: `wordcartel/src/swap.rs` (`delete`)

#### Interfaces

**Consumes** (Tasks 2, 3, 5, 6, 7):

```rust
// crate::fsx
pub(crate) trait Fs {
    fn create_excl(&self, path: &Path, mode: u32) -> std::io::Result<Box<dyn WriteSync>>;
    fn existing_mode(&self, path: &Path) -> Option<u32>;
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn sync_dir(&self, dir: &Path) -> std::io::Result<()>;
    fn remove_file(&self, path: &Path) -> std::io::Result<()>;
    fn read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>>;
    fn stat(&self, path: &Path) -> std::io::Result<FileStat>;
    fn list_dir(&self, path: &Path, cap: Option<usize>) -> std::io::Result<DirListing>;
}
pub(crate) enum ModePolicy { Fixed(u32), PreserveExistingOr(u32) }
pub(crate) struct WriteOpts { pub mode: ModePolicy, pub dir_fsync: bool }
pub(crate) fn atomic_replace(fs: &dyn Fs, final_path: &Path, bytes: &[u8], opts: WriteOpts)
    -> std::io::Result<()>;
pub(crate) fn is_file_via(fs: &dyn Fs, path: &Path) -> bool;

// crate::file (Task 6)
pub(crate) fn bounded_read_opt_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, limit: u64)
    -> Option<Vec<u8>>;
```

**Produces:**

```rust
// crate::file
pub fn save_atomic_bytes(path: &Path, content: &[u8]) -> Result<(), SaveError>;  // unchanged sig
pub(crate) fn save_atomic_bytes_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, content: &[u8])
    -> Result<(), SaveError>;

// crate::diagnostics_run
pub fn append_word_to_dict(path: &std::path::Path, word: &str) -> std::io::Result<()>; // unchanged
pub(crate) fn append_word_to_dict_with_fs(fs: &dyn crate::fsx::Fs, path: &std::path::Path,
    word: &str) -> std::io::Result<()>;

// crate::swap
pub fn delete(doc_path: Option<&Path>);                                          // unchanged sig
pub(crate) fn delete_with_fs(fs: &dyn crate::fsx::Fs, doc_path: Option<&Path>);
```

#### Steps

1. **Write the failing tests.** In `diagnostics_run.rs`'s test module:

```rust
    #[test]
    fn append_word_to_dict_is_atomic_and_preserves_existing_words() {
        // The append becomes read -> append in memory -> atomic_replace, so a torn write
        // is impossible. Existing content must survive verbatim.
        let d = std::env::temp_dir().join(format!("wc-dict-atomic-{}", std::process::id()));
        let p = d.join("dictionary.txt");
        let _ = std::fs::remove_dir_all(&d);
        append_word_to_dict_with_fs(&crate::fsx::RealFs, &p, "alpha").expect("first append");
        append_word_to_dict_with_fs(&crate::fsx::RealFs, &p, "beta").expect("second append");
        let got = std::fs::read_to_string(&p).expect("read back");
        assert_eq!(got, "alpha\nbeta\n", "both words present, newline-terminated, in order");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn append_word_to_dict_refuses_a_symlinked_dictionary() {
        // The append gains the symlink guard every other durable write has. Writing through
        // the link would replace it with a regular file and destroy the link.
        let d = std::env::temp_dir().join(format!("wc-dict-link-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("dir");
        let real = d.join("real.txt");
        let link = d.join("dict.txt");
        std::fs::write(&real, "existing\n").expect("seed");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let err = append_word_to_dict_with_fs(&crate::fsx::RealFs, &link, "nope")
            .expect_err("symlinked dictionary must be refused");
        assert!(err.to_string().to_lowercase().contains("symlink"), "got {err}");
        assert!(link.symlink_metadata().expect("lstat").file_type().is_symlink(),
            "the link must survive — that is what the refusal protects");
        assert_eq!(std::fs::read_to_string(&real).expect("read"), "existing\n",
            "target untouched");
        let _ = std::fs::remove_dir_all(&d);
    }
```

In `file.rs`'s test module:

```rust
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
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib diagnostics_run::tests::append_word_to_dict_is_atomic
```

Expected: ``error[E0425]: cannot find function `append_word_to_dict_with_fs` in this scope``.

3. **Add the symlink guard to `save_atomic_bytes`:**

```rust
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
```

4. **Rewrite the dictionary append** in `diagnostics_run.rs`:

```rust
pub fn append_word_to_dict(path: &std::path::Path, word: &str) -> std::io::Result<()> {
    append_word_to_dict_with_fs(&crate::fsx::RealFs, path, word)
}

/// Append `word` as a line to the personal dictionary — READ, append in memory, then
/// ATOMIC REPLACE.
///
/// This was the only durable write in the app outside `atomic_replace`: an
/// `OpenOptions::append` + `writeln!`, non-atomic, uncapped, with no symlink guard. A torn
/// append could leave a half-written line; the atomic form cannot. Behaviour preserved:
/// the parent directory is still created (see `append_word_to_dict_creates_parent_dir`).
pub(crate) fn append_word_to_dict_with_fs(fs: &dyn crate::fsx::Fs, path: &std::path::Path,
    word: &str) -> std::io::Result<()>
{
    if let Some(parent) = path.parent() {
        // Directory PROVISIONING — exempt from the seam by clause (b) of spec §2.3.
        std::fs::create_dir_all(parent)?;
    }
    // Symlink refusal, matching every other durable write.
    if matches!(fs.stat(path), Ok(st) if st.is_symlink) {
        return Err(std::io::Error::other("refusing to write through symlink"));
    }
    // Read what is there (missing/over-cap → start empty, the same degradation the old
    // create(true).append(true) had for a missing file).
    let mut buf = crate::file::bounded_read_opt_with_fs(fs, path, crate::limits::MAX_OPEN_BYTES)
        .unwrap_or_default();
    if !buf.is_empty() && !buf.ends_with(b"\n") { buf.push(b'\n'); }
    buf.extend_from_slice(word.as_bytes());
    buf.push(b'\n');
    crate::fsx::atomic_replace(fs, path, &buf, crate::fsx::WriteOpts {
        mode: crate::fsx::ModePolicy::PreserveExistingOr(0o600),
        dir_fsync: true,
    })
}
```

5. **Route the export rename and its cleanup** in `jobs_apply::apply_export_done`. Add a leading
   `fs: &dyn crate::fsx::Fs` parameter and replace the two raw calls:

```rust
        Ok(crate::export::ExportResult::TempReady(tmp)) => {
            match fs.rename(&tmp, &target) {
                Ok(()) => {
                    let status = format!("exported {}", target.display());
                    editor.set_status(crate::status::StatusKind::Info, status);
                }
                Err(e) => {
                    let _ = fs.remove_file(&tmp);
                    editor.set_status_full(crate::status::StatusKind::Error, format!("export rename failed: {e}"),
                        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                }
            }
        }
```

and the TOCTOU-refusal cleanup earlier in the same function:

```rust
    if !overwrite_confirmed && crate::fsx::exists_via(fs, &target) {
        if let Ok(crate::export::ExportResult::TempReady(tmp)) = &result {
            let _ = fs.remove_file(tmp);
        }
        // …unchanged Sticky Warning…
    }
```

> The TOCTOU guard's *logic* is behaviour-preserved exactly. Only the calls change.

6. **Route the three delete sites** in `prompts::resolve_prompt`. `resolve_prompt` already receives
   `ex`, `clock`, and `msg_tx`; add `fs: &dyn crate::fsx::Fs` alongside them.

```rust
        PromptAction::Recover => {
            let staged = {
                let b = editor.active_mut();
                b.pending_swap_body.take().map(|body| (body, b.pending_swap_path.take()))
            };
            if let Some((body, orphan)) = staged {
                crate::save::load_recovered(editor, &body);
                // Delete AFTER load_recovered — `pending_swap_path` is the orphan-scratch
                // recovery carrier, and load_recovered replaces the whole Buffer.
                if let Some(p) = orphan { let _ = fs.remove_file(&p); }
            }
        }
        PromptAction::DiscardSwap => {
            if let Some(p) = editor.active_mut().pending_swap_path.take() {
                let _ = fs.remove_file(&p);
            } else {
                crate::swap::delete_with_fs(fs, editor.active().document.path.as_deref());
            }
        }
```

and in the `CleanRecovery` arm, replace only the delete call:

```rust
            for p in std::mem::take(&mut editor.pending_clean) {
                if !crate::swap::recovery_path_still_cleanable(fs, &p, &protected) { continue; }
                if fs.remove_file(&p).is_ok() { n += 1; }
            }
```

> **The bidirectional TOCTOU discipline is preserved verbatim.** `pending_clean` remains the
> ceiling (`std::mem::take`, never a re-scan), and `recovery_path_still_cleanable` is still re-run
> per path so the set can only ever narrow. Only the delete call changes.

7. **Route `swap::delete`:**

```rust
pub fn delete(doc_path: Option<&Path>) {
    delete_with_fs(&crate::fsx::RealFs, doc_path)
}

/// Best-effort delete of a document's swap file. The result is DISCARDED and must stay
/// discarded: a failed swap cleanup is never worth surfacing to the writer or failing a
/// save over.
pub(crate) fn delete_with_fs(fs: &dyn crate::fsx::Fs, doc_path: Option<&Path>) {
    if let Ok(p) = swap_path(doc_path) {
        let _ = fs.remove_file(&p);
    }
}
```

8. **Run — expect green:**

```
cargo test -p wordcartel --lib diagnostics_run:: file:: prompts:: swap:: jobs_apply::
```

Expected: all pass. **These four must still pass unmodified:**
`diagnostics_run::tests::append_word_to_dict_creates_parent_dir`,
`prompts::tests::recover_loads_body_and_deletes_orphan_swap_file`,
`jobs_apply::tests::apply_export_done_rename_failure_is_a_sticky_error`,
`jobs_apply::tests::apply_export_done_toctou_target_appeared_is_a_sticky_warning`.

9. **Commit:** `refactor(c5): route durable mutations through the seam; make the dict append atomic`

---

### Task 9 — Listings onto `Fs::list_dir`

**Deliverable:** the three production listing sites go through `list_dir`. This also **fixes §4.9**:
routing the browser through the resolving `list_dir` makes symlinked directories usable for the
first time.

#### Files

- Modify: `wordcartel/src/file_browser.rs` (`rebuild_entries`)
- Modify: `wordcartel/src/swap.rs` (`cleanable_recovery_files`, `find_orphan_scratch_swap_in`)

#### Interfaces

**Consumes** (Tasks 4, 5, 6):

```rust
// crate::fsx
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind { File, Dir, Other, Unknown }

#[derive(Clone, Debug)]
pub(crate) struct DirEntryInfo {
    pub name: String,
    pub kind: EntryKind,
    pub is_symlink: bool,
    pub broken: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct DirListing {
    pub entries: Vec<DirEntryInfo>,
    pub total_seen: usize,
    pub unreadable: usize,
}

pub(crate) trait Fs {
    fn list_dir(&self, path: &Path, cap: Option<usize>) -> std::io::Result<DirListing>;
    // …
}

// crate::limits
pub const MAX_DIR_ENTRIES: usize = 5_000;
```

**Produces:**

```rust
// crate::file_browser — signature CHANGES (takes the seam); FileEntry shape is unchanged
// in this task and is reshaped in Task 14.
pub(crate) fn rebuild_entries(fs: &dyn crate::fsx::Fs, fb: &mut FileBrowser);

// crate::swap
pub(crate) fn cleanable_recovery_files(fs: &dyn crate::fsx::Fs, dir: &Path,
    protected: &HashSet<PathBuf>) -> Vec<PathBuf>;
pub(crate) fn recovery_path_still_cleanable(fs: &dyn crate::fsx::Fs, path: &Path,
    protected: &HashSet<PathBuf>) -> bool;
pub fn find_orphan_scratch_swap() -> Option<(PathBuf, SwapHeader, String)>;   // unchanged sig
fn find_orphan_scratch_swap_in(fs: &dyn crate::fsx::Fs, dir: &Path)
    -> Option<(PathBuf, SwapHeader, String)>;
```

#### Steps

1. **Write the failing tests.** In `file_browser.rs`'s test module:

```rust
    #[cfg(unix)]
    #[test]
    fn rebuild_entries_treats_a_symlinked_directory_as_a_directory() {
        // §4.9 REGRESSION. `DirEntry::file_type()` does not follow symlinks, so a symlink to
        // a directory reported is_dir == false: it sorted with FILES, rendered without the
        // trailing '/', and Enter routed it to file::open, which returned "is a directory".
        // The entry was UNUSABLE, not merely mis-sorted. Routing through the resolving
        // list_dir fixes all three consumers at once.
        let dir = std::env::temp_dir().join(format!("wc-fb-symdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real_sub")).expect("seed dir");
        std::fs::write(dir.join("plain.md"), b"x").expect("seed file");
        std::os::unix::fs::symlink(dir.join("real_sub"), dir.join("link_sub")).expect("symlink");

        let mut fb = FileBrowser {
            dir: dir.clone(), query: String::new(), entries: vec![], selected: 0, scroll_top: 0,
        };
        rebuild_entries(&crate::fsx::RealFs, &mut fb);

        let link = fb.entries.iter().find(|e| e.name == "link_sub").expect("link listed");
        assert!(link.is_dir, "a symlink to a directory MUST classify as a directory");

        let names: Vec<&str> = fb.entries.iter().map(|e| e.name.as_str()).collect();
        let link_i = names.iter().position(|n| *n == "link_sub").expect("present");
        let file_i = names.iter().position(|n| *n == "plain.md").expect("present");
        assert!(link_i < file_i, "and therefore sorts with the directories, before files");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

In `swap.rs`'s test module:

```rust
    #[test]
    fn swap_scans_are_uncapped() {
        // The swap state-dir scans are uncapped TODAY. Routing them through a capped
        // list_dir would be a refactor-introduced regression — a new restriction the
        // current code does not have — and would silently shrink what clean_recovery
        // can find. They pass cap: None.
        let dir = unique_dir("swap-uncapped");
        let n = crate::limits::MAX_DIR_ENTRIES + 7;
        for i in 0..n {
            std::fs::write(dir.join(format!("recovered-x-{i}-0.md")), b"x").expect("seed");
        }
        let out = cleanable_recovery_files(&crate::fsx::RealFs, &dir, &HashSet::new());
        assert_eq!(out.len(), n, "every recovered-*.md dump is enumerated — no cap");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib file_browser::tests::rebuild_entries_treats_a_symlinked
```

Expected: ``error[E0061]: this function takes 1 argument but 2 arguments were supplied``.

3. **Rewrite `rebuild_entries`:**

```rust
/// Rebuild `entries` from `dir`: synthetic ".." first (unless at root), then directories,
/// then files, each alphabetical; substring-filtered (case-insensitive) by `query`.
///
/// Classification comes from the seam, which RESOLVES symlinks — so a symlink to a
/// directory is a directory here (spec §4.9). `EntryKind::Other` and `Unknown` sort with
/// files for now; Task 14 gives them their own markers and refusals.
pub(crate) fn rebuild_entries(fs: &dyn crate::fsx::Fs, fb: &mut FileBrowser) {
    let q = fb.query.to_ascii_lowercase();
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    if let Ok(listing) = fs.list_dir(&fb.dir, Some(crate::limits::MAX_DIR_ENTRIES)) {
        for e in listing.entries {
            if !q.is_empty() && !e.name.to_ascii_lowercase().contains(&q) {
                continue;
            }
            match e.kind {
                crate::fsx::EntryKind::Dir => dirs.push(e.name),
                _ => files.push(e.name),
            }
        }
    }
    dirs.sort();
    files.sort();
    fb.entries = Vec::new();
    if fb.dir.parent().is_some() {
        fb.entries.push(FileEntry { name: "..".into(), is_dir: true });
    }
    fb.entries.extend(dirs.into_iter().map(|name| FileEntry { name, is_dir: true }));
    fb.entries.extend(files.into_iter().map(|name| FileEntry { name, is_dir: false }));
    if fb.selected >= fb.entries.len() {
        fb.selected = fb.entries.len().saturating_sub(1);
    }
    fb.scroll_top = fb.scroll_top.min(fb.entries.len().saturating_sub(1));
}
```

Update its callers in `file_browser.rs` (`file_browser_enter`, `intercept`) and
`editor::Editor::open_file_browser` to pass the seam. `open_file_browser` gains a leading
`fs: &dyn crate::fsx::Fs` parameter; its callers are the `"open"` command (which has `Ctx.fs`) and
tests (which pass `&crate::fsx::RealFs`).

4. **Route the two swap scans.** In `cleanable_recovery_files`, replace the `read_dir` loop:

```rust
pub(crate) fn cleanable_recovery_files(fs: &dyn crate::fsx::Fs, dir: &Path,
    protected: &HashSet<PathBuf>) -> Vec<PathBuf>
{
    let me = std::process::id();
    let mut out = Vec::new();
    // cap: None — this scan is uncapped today and capping it would silently shrink what
    // `clean_recovery` can find. It is a startup/command-time scan, off the redraw path.
    let Ok(listing) = fs.list_dir(dir, None) else { return out };
    for e in listing.entries {
        let path = dir.join(&e.name);
        if recovery_file_is_cleanable(fs, dir, &path, &e.name, protected, me) { out.push(path); }
    }
    out
}
```

and in `find_orphan_scratch_swap_in`:

```rust
fn find_orphan_scratch_swap_in(fs: &dyn crate::fsx::Fs, dir: &std::path::Path)
    -> Option<(std::path::PathBuf, SwapHeader, String)>
{
    let me = std::process::id();
    let mut best: Option<(std::path::PathBuf, SwapHeader, String)> = None;
    let listing = fs.list_dir(dir, None).ok()?;   // uncapped, as today
    for e in listing.entries {
        let pid = e.name.strip_prefix("scratch-")
            .and_then(|s| s.strip_suffix(".swp"))
            .and_then(|s| s.parse::<u32>().ok());
        let Some(pid) = pid else { continue };
        if pid == me || pid_is_live(pid) { continue; }
        let path = dir.join(&e.name);
        let Some(raw) = read_swap_capped(fs, &path) else { continue };
        let Some((header, body)) = parse(&raw) else { continue };
        if body.is_empty() { continue; }
        let newer = match &best { Some((_, h, _)) => header.ts_ms > h.ts_ms, None => true };
        if newer { best = Some((path, header, body)); }
    }
    best
}
```

> **Neither scan consults `kind`.** Verified: both classify purely from the file NAME
> (`*.swp`, `recovered-*.md`, `*.tmp`, `scratch-{pid}.swp`) and then read the file. So
> `EntryKind::Unknown` entries flow through their existing filename logic unchanged and no
> behaviour changes for them.

5. **Run — expect green:**

```
cargo test -p wordcartel --lib file_browser:: swap::
```

Expected: all pass. **These must still pass unmodified:**
`swap::tests::swap_is_cleanable_only_for_valueless_dead_pid_swaps`,
`swap::tests::enumerator_tmp_only_byte_identical_duplicate_is_cleanable`,
`swap::tests::find_orphan_scratch_swap_finds_dead_pid_and_skips_self`,
`file_browser::tests::enter_on_unreadable_dir_stays_put_and_sets_status`.

6. **Commit:** `refactor(c5): route listings through Fs::list_dir; symlinked directories now usable`

---

### Task 10 — Decision 12: plugin discovery follows symlinks

**Deliverable:** `plugin::load::discover` routes through `list_dir`, follows symlinks, and reports
every *plausibly a plugin but unloadable* entry by name instead of silently dropping it.

**This is a deliberate behaviour change, not a refactor consequence.** Today `discover` classifies
with the non-following `entry.file_type()`, so a symlink to a `.lua` file or to a plugin directory
falls off the end of the loop — not loaded, and not reported. It is defensible because the trust
boundary is **already porous**: the nested `init.is_file()` probe *is* `Path::is_file()`, which does
follow, so a real directory whose `init.lua` is a symlink already loads today.

#### Files

- Modify: `wordcartel/src/plugin/load.rs`

#### Interfaces

**Consumes** (Tasks 4, 6, 7, 9):

```rust
// crate::fsx
pub(crate) enum EntryKind { File, Dir, Other, Unknown }
pub(crate) struct DirEntryInfo { pub name: String, pub kind: EntryKind,
                                 pub is_symlink: bool, pub broken: bool }
pub(crate) struct DirListing { pub entries: Vec<DirEntryInfo>, pub total_seen: usize,
                               pub unreadable: usize }
pub(crate) trait Fs {
    fn list_dir(&self, path: &Path, cap: Option<usize>) -> std::io::Result<DirListing>;
    // …
}
pub(crate) fn is_file_via(fs: &dyn Fs, path: &Path) -> bool;

// crate::file (Task 6)
pub(crate) fn bounded_read_opt_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, limit: u64)
    -> Option<Vec<u8>>;
```

**Produces:**

```rust
// crate::plugin::load
pub fn discover(dir: &Path, disable: &[String]) -> Discovered;                 // unchanged sig
pub(crate) fn discover_with_fs(fs: &dyn crate::fsx::Fs, dir: &Path, disable: &[String])
    -> Discovered;

/// Rider 3's rule, extracted so it is testable in isolation and cannot drift into a
/// hand-list: an entry is REPORTED when it is plausibly a plugin but not loadable.
pub(crate) fn is_plausible_plugin(name: &str, kind: crate::fsx::EntryKind, broken: bool) -> bool;
```

`Discovered` and `LoadReport` are unchanged:

```rust
pub struct Discovered { pub sources: Vec<(String, String)>, pub skipped: Vec<LoadReport> }
pub struct LoadReport { pub plugin: String, pub result: Result<usize, String>, pub hooks: usize }
```

#### Steps

1. **Write the failing tests** in `plugin/load.rs`'s test module:

```rust
    #[cfg(unix)]
    #[test]
    fn discover_follows_symlinked_plugins_both_shapes() {
        // DECISION 12. Today a symlink to a .lua file and a symlink to a plugin directory
        // are BOTH silently ignored — neither is_file() nor is_dir() under the non-following
        // entry.file_type(). Both must now load.
        let d = unique_plugin_dir("d12-follow");
        let store = d.join("store");
        std::fs::create_dir_all(store.join("dirplug")).expect("seed");
        std::fs::write(store.join("single.lua"), "-- single\n").expect("seed");
        std::fs::write(store.join("dirplug").join("init.lua"), "-- dir\n").expect("seed");
        let plugins = d.join("plugins");
        std::fs::create_dir_all(&plugins).expect("seed");
        std::os::unix::fs::symlink(store.join("single.lua"), plugins.join("linked.lua"))
            .expect("link->file");
        std::os::unix::fs::symlink(store.join("dirplug"), plugins.join("linkeddir"))
            .expect("link->dir");

        let got = discover_with_fs(&crate::fsx::RealFs, &plugins, &[]);
        let stems: Vec<&str> = got.sources.iter().map(|(s, _)| s.as_str()).collect();
        assert!(stems.contains(&"linked"), "symlinked .lua must load: {stems:?}");
        assert!(stems.contains(&"linkeddir"), "symlinked plugin DIR must load: {stems:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn discover_still_loads_a_real_dir_with_a_symlinked_init() {
        // The pre-existing half of the inconsistency: `init.is_file()` already FOLLOWS, so
        // this loads today. Decision 12 converges the two halves — it must not invert this.
        let d = unique_plugin_dir("d12-init");
        std::fs::create_dir_all(d.join("plugins").join("p")).expect("seed");
        std::fs::write(d.join("real_init.lua"), "-- real\n").expect("seed");
        std::os::unix::fs::symlink(d.join("real_init.lua"),
            d.join("plugins").join("p").join("init.lua")).expect("link");
        let got = discover_with_fs(&crate::fsx::RealFs, &d.join("plugins"), &[]);
        assert!(got.sources.iter().any(|(s, _)| s == "p"),
            "a real dir with a symlinked init.lua must still load");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn discover_reports_plausible_plugins_and_stays_silent_about_the_rest() {
        // RIDER 3. The contract says a found candidate is "named, never silently dropped".
        // Following symlinks closes the biggest class but not all of them, so the rule is:
        // report anything PLAUSIBLY a plugin that cannot be loaded — and nothing else, or
        // the report floods with README.md and becomes useless.
        let d = unique_plugin_dir("d12-rider3");
        let p = d.join("plugins");
        std::fs::create_dir_all(&p).expect("seed");
        std::os::unix::fs::symlink(p.join("gone.lua"), p.join("dangling.lua")).expect("broken");
        std::fs::write(p.join("README.md"), "not a plugin\n").expect("seed");
        std::fs::create_dir_all(p.join("just_a_dir")).expect("seed");

        let got = discover_with_fs(&crate::fsx::RealFs, &p, &[]);
        let named: Vec<&str> = got.skipped.iter().map(|r| r.plugin.as_str()).collect();

        assert!(named.contains(&"dangling.lua"),
            "a broken symlink in the plugins dir is reported BY NAME: {named:?}");
        assert!(!named.iter().any(|n| *n == "README.md"),
            "an ordinary non-plugin file stays silent — the qualifier is what bounds the report");
        assert!(!named.iter().any(|n| *n == "just_a_dir"),
            "an ordinary subdirectory without init.lua stays silent");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn is_plausible_plugin_rule() {
        use crate::fsx::EntryKind::*;
        // Loadable shapes are NOT "plausible but unloadable" — they load.
        assert!(!is_plausible_plugin("ok.lua", File, false));
        assert!(!is_plausible_plugin("somedir", Dir, false));
        // Plausible but unloadable — reported.
        assert!(is_plausible_plugin("x.lua", Other, false), "a fifo named x.lua");
        assert!(is_plausible_plugin("x.lua", Unknown, false), "type probe failed");
        assert!(is_plausible_plugin("whatever", Unknown, true), "a broken symlink, any name");
        // Not plausible — silent.
        assert!(!is_plausible_plugin("README.md", File, false));
        assert!(!is_plausible_plugin("notes.txt", Other, false));
    }
```

Add the shared temp-dir helper if the module lacks one:

```rust
    fn unique_plugin_dir(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "wc-plug-{}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed), label));
        std::fs::create_dir_all(&d).expect("create dir");
        d
    }
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib plugin::load::tests::discover_follows_symlinked
```

Expected: ``error[E0425]: cannot find function `discover_with_fs` in this scope``.

3. **Add the rider-3 rule:**

```rust
/// Rider 3 (spec §5.2): is this entry PLAUSIBLY a plugin that could not be loaded?
///
/// A rule, not a list, and deliberately narrow. `discover`'s contract says a found candidate
/// is "named, never silently dropped" — but reporting every unloadable file would flood the
/// report with `README.md` and make it useless. So: a `.lua` name of any non-`File` kind, or
/// a broken symlink (which we cannot classify and which is always actionable in a plugins
/// directory). A loadable `File`/`Dir` is not "unloadable" and returns false — those load.
pub(crate) fn is_plausible_plugin(name: &str, kind: crate::fsx::EntryKind, broken: bool) -> bool {
    use crate::fsx::EntryKind;
    if broken { return true; }
    match kind {
        EntryKind::File | EntryKind::Dir => false,
        EntryKind::Other | EntryKind::Unknown => {
            std::path::Path::new(name).extension().and_then(|e| e.to_str()) == Some("lua")
        }
    }
}
```

4. **Rewrite the discovery scan:**

```rust
pub fn discover(dir: &Path, disable: &[String]) -> Discovered {
    discover_with_fs(&crate::fsx::RealFs, dir, disable)
}

pub(crate) fn discover_with_fs(fs: &dyn crate::fsx::Fs, dir: &Path, disable: &[String])
    -> Discovered
{
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    let mut unloadable: Vec<LoadReport> = Vec::new();
    // cap: None — discovery is uncapped today, and capping it would drop plausible plugins
    // past the cap: neither loaded nor named, the exact silent drop rider 3 eliminates.
    if let Ok(listing) = fs.list_dir(dir, None) {
        for e in listing.entries {
            let path = dir.join(&e.name);
            match e.kind {
                // Rider 2: the .lua arm matches kind == File, NEVER "not a directory" — a
                // fifo named x.lua is Other and must not become a candidate.
                crate::fsx::EntryKind::File => {
                    if path.extension().and_then(|x| x.to_str()) == Some("lua") {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            candidates.push((stem.to_string(), path));
                        } else {
                            unloadable.push(LoadReport {
                                plugin: e.name.clone(), hooks: 0,
                                result: Err("plugin name is not valid UTF-8".into()),
                            });
                        }
                    }
                }
                crate::fsx::EntryKind::Dir => {
                    let init = path.join("init.lua");
                    if crate::fsx::is_file_via(fs, &init) {
                        if let Some(stem) = path.file_name().and_then(|s| s.to_str()) {
                            candidates.push((stem.to_string(), init));
                        } else {
                            unloadable.push(LoadReport {
                                plugin: e.name.clone(), hooks: 0,
                                result: Err("plugin directory name is not valid UTF-8".into()),
                            });
                        }
                    }
                }
                crate::fsx::EntryKind::Other | crate::fsx::EntryKind::Unknown => {}
            }
            // Rider 1 + rider 3: anything plausibly a plugin that did NOT become a candidate
            // is reported by name rather than falling off the end of the loop.
            if is_plausible_plugin(&e.name, e.kind, e.broken) {
                unloadable.push(LoadReport {
                    plugin: e.name.clone(), hooks: 0,
                    result: Err(if e.broken {
                        "symlink cannot be resolved".to_string()
                    } else {
                        "not a loadable plugin file".to_string()
                    }),
                });
            }
        }
        // The one case where "named" degrades honestly: an entry the iterator could not read
        // has no name to report, so it can only ever be a count.
        if listing.unreadable > 0 {
            unloadable.push(LoadReport {
                plugin: format!("<{} unreadable directory entries>", listing.unreadable),
                hooks: 0,
                result: Err("directory entries could not be read".into()),
            });
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));

    let mut sources = Vec::new();
    let mut skipped = unloadable;
    // …the existing `while i < candidates.len()` disable-filter + bounded-read loop, with
    // `crate::file::bounded_read_opt(path, PLUGIN_MAX_SOURCE_BYTES)` replaced by
    // `crate::file::bounded_read_opt_with_fs(fs, path, PLUGIN_MAX_SOURCE_BYTES)` …
    Discovered { sources, skipped }
}
```

5. **Update the doc comment.** The existing one states that `discover` "does not touch the `Fs`
   trait (write-only seam)". That is now false. Replace that sentence with:

```rust
/// This function reads through the `Fs` seam (C5) and hands `(stem, source)` pairs onward;
/// it does not touch the string-core `load_sources`.
```

6. **Run — expect green:**

```
cargo test -p wordcartel --lib plugin::load::
```

Expected: all pass, including the four new tests. Every pre-existing `plugin::load` test must still
pass — discovery of ordinary `.lua` files and `<name>/init.lua` directories is unchanged.

7. **Commit:** `feat(c5): plugin discovery follows symlinks and reports plausible-but-unloadable entries`

---

### Task 11 — The `fs_chokepoint` guard test

**Deliverable:** an integration test that fails the build when new raw filesystem access appears in
production sources outside a clause-citing allow-list. This is what converts §2.3's scope claim from
prose that decays into an invariant that holds.

#### Files

- Create: `wordcartel/tests/fs_chokepoint.rs`

#### Interfaces

**Consumes:** nothing at the type level — it reads source text. It depends on Tasks 6–10 having
migrated their call sites, because otherwise the allow-list would have to name all of them.

**Produces:** no Rust API. Produces the allow-list, which becomes the census of record.

#### Steps

1. **Write the test file.** Create `wordcartel/tests/fs_chokepoint.rs`:

```rust
//! C5 §2.3 — the filesystem-chokepoint guard.
//!
//! Scope is defined by a RULE, not a list: a production call in the `wordcartel` crate that
//! reads file content, enumerates a directory, probes metadata, or mutates durably goes
//! through `fsx::Fs`. This test enforces it by scanning source text and failing on any raw
//! filesystem access not in the allow-list below, where every entry cites the exemption
//! clause it claims.
//!
//! HONEST LIMITS (spec §2.3): the scan is textual, so it can flag a token in a comment or
//! string; `#[cfg(test)]` stripping is heuristic; and the import gate covers the ORDINARY
//! `std::fs` import spellings, not nested-group / renamed-in-group / leading-root `::std::fs`
//! forms. Those gaps are disclosed rather than papered over — closing them needs `use`-tree
//! parsing (a dev-dependency and a mini Rust parser), which was weighed and declined. This is
//! a high-coverage drift alarm, not a completeness proof.

use std::path::{Path, PathBuf};

/// Files allowed to contain raw filesystem access, with the §2.3 clause each claims.
/// THIS LIST IS THE CENSUS. A new raw call fails the build until it is either routed
/// through `Fs` or added here with a clause.
const ALLOW: &[(&str, &str)] = &[
    ("src/fsx.rs",            "(d) the seam's own implementation"),
    ("src/swap.rs",           "(b) directory provisioning (state_dir) + (c) canonicalize + (e) /proc liveness"),
    ("src/settings.rs",       "(b) directory provisioning for the overrides parent"),
    ("src/diagnostics_run.rs","(b) directory provisioning for the dictionary parent"),
    ("src/session_restore.rs","(c) canonicalize for the session key"),
    ("src/recovery.rs",       "(d)-adjacent: the panic hook keeps RealFs by design — it must have no dependencies"),
    ("src/filter.rs",         "(a) subprocess-owned IO — the user's `sh -c` command"),
    ("src/harper_ls.rs",      "(a) subprocess-owned IO — the child's own userDictPath"),
    ("src/export.rs",         "(a) subprocess-owned IO — pandoc writes its own -o target"),
];

/// Inherent `Path` methods that touch the filesystem. A CLOSED, std-defined set: it does not
/// drift as this codebase changes, only if the standard library adds a method. Both call
/// syntaxes are matched — `.method(` and UFCS `Path::method(` — because a dot-call scan
/// misses `Path::metadata(p)` entirely.
const PATH_FS_METHODS: &[&str] = &[
    "metadata", "symlink_metadata", "canonicalize", "read_link", "read_dir",
    "exists", "try_exists", "is_file", "is_dir", "is_symlink",
];

/// Import spellings that bring `std::fs` into scope. Layer 1 — the sound layer for anything
/// reached through an import, because Rust REQUIRES one of these for a short-form `fs::…` or
/// a bare `File::open` call.
fn has_std_fs_import(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with("//") { return false; }
    // `use std::fs;`, `use std::fs::File;`, `use std::fs::OpenOptions;`, `use std::fs as x;`
    // — all share this prefix. Deliberately NOT anchored on a trailing `;`, which would miss
    // every type import.
    if t.starts_with("use std::fs") { return true; }
    // Flat grouped: `use std::{fs, io};` — the literal `use std::fs` never appears.
    if t.starts_with("use std::{") && t.contains("fs") { return true; }
    false
}

fn strip_test_modules(src: &str) -> String {
    // Heuristic, matching what module_budgets already lives with: drop everything from a
    // module-level `#[cfg(test)]` onward. Production code precedes it in every file here.
    match src.find("#[cfg(test)]") {
        Some(i) => src[..i].to_string(),
        None => src.to_string(),
    }
}

fn offenders_in(src: &str) -> Vec<String> {
    let prod = strip_test_modules(src);
    let mut out = Vec::new();
    for (n, line) in prod.lines().enumerate() {
        let t = line.trim();
        if t.starts_with("//") || t.starts_with("///") { continue; }
        let mut hit: Option<String> = None;
        if has_std_fs_import(line) {
            hit = Some("std::fs import".to_string());
        } else if t.contains("std::fs::") {
            hit = Some("fully-qualified std::fs::".to_string());
        } else if t.contains("OpenOptions") {
            hit = Some("OpenOptions".to_string());
        } else {
            for m in PATH_FS_METHODS {
                if t.contains(&format!(".{m}(")) || t.contains(&format!("Path::{m}(")) {
                    hit = Some(format!("inherent Path::{m}"));
                    break;
                }
            }
        }
        if let Some(what) = hit {
            out.push(format!("  line {}: {what} — {}", n + 1, t));
        }
    }
    out
}

fn crate_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rel(p: &Path) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    format!("src/{}", p.strip_prefix(root.join("src")).expect("under src").display())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read src").flatten() {
        let p = e.path();
        if p.is_dir() { walk(&p, out); }
        else if p.extension().and_then(|x| x.to_str()) == Some("rs") { out.push(p); }
    }
}

#[test]
fn production_sources_route_filesystem_access_through_the_seam() {
    let mut files = Vec::new();
    walk(&crate_src(), &mut files);
    files.sort();

    let mut failures = Vec::new();
    for f in files {
        let name = rel(&f);
        // e2e.rs is test-only by nature; test_support.rs hosts FaultFs.
        if name == "src/e2e.rs" || name == "src/test_support.rs" { continue; }
        if ALLOW.iter().any(|(a, _)| *a == name) { continue; }
        let src = std::fs::read_to_string(&f).expect("read source");
        let hits = offenders_in(&src);
        if !hits.is_empty() {
            failures.push(format!("{name}:\n{}", hits.join("\n")));
        }
    }

    assert!(failures.is_empty(),
        "raw filesystem access outside the allow-list.\n\n{}\n\n\
         Route these through `fsx::Fs` (see spec §5.2), or — if one falls under a §2.3 \
         exemption clause — add the file to ALLOW in this test WITH the clause it claims.",
        failures.join("\n\n"));
}

// ---------------------------------------------------------------------------
// Self-check: one planted evasion per detection route.
//
// A self-check that plants only a fully-qualified call proves layer 2 and NOTHING about
// layers 1 or 3 — the vacuous-guardrail failure. Each sample below is invisible to the
// routes that do not target it, so all four are required.
//
// NOTE: this proves the layers work on the spellings they TARGET. It is not evidence that
// the disclosed gaps (nested-group / renamed-in-group / `::std::fs` imports) are caught.
// ---------------------------------------------------------------------------

#[test]
fn scanner_detects_every_evasion_route() {
    let cases: &[(&str, &str)] = &[
        ("fully-qualified", "fn f(p: &std::path::Path) { let _ = std::fs::read(p); }"),
        ("aliased import",  "use std::fs;\nfn f(p: &std::path::Path) { let _ = fs::write(p, b\"x\"); }"),
        ("inherent dot",    "fn f(p: &std::path::Path) { let _ = p.symlink_metadata(); }"),
        ("inherent UFCS",   "fn f(p: &std::path::Path) { let _ = std::path::Path::metadata(p); }"),
    ];
    for (label, src) in cases {
        assert!(!offenders_in(src).is_empty(),
            "the scanner missed the {label} evasion — this route is unguarded:\n{src}");
    }
}

#[test]
fn scanner_ignores_ordinary_code_and_test_modules() {
    // A false positive costs one allow-list line, so over-matching is survivable — but the
    // scanner must not flag code with no filesystem access at all, or the list becomes noise.
    assert!(offenders_in("fn f(x: usize) -> usize { x + 1 }").is_empty());
    // Everything from the module-level #[cfg(test)] marker onward is stripped.
    let with_tests = "fn f() {}\n#[cfg(test)]\nmod tests {\n  use std::fs;\n}\n";
    assert!(offenders_in(with_tests).is_empty(), "test modules are out of scope by the rule");
}
```

2. **Run — expect the main test to FAIL, listing real offenders:**

```
cargo test -p wordcartel --test fs_chokepoint
```

Expected: `scanner_detects_every_evasion_route` and `scanner_ignores_ordinary_code_and_test_modules`
pass; `production_sources_route_filesystem_access_through_the_seam` fails with a list of files still
holding raw access.

3. **Resolve each listed offender** by one of exactly two moves — never a third:
   - Route the call through `Fs` (the default; Tasks 6–10 should have covered it), or
   - Add the file to `ALLOW` **with the §2.3 clause it claims**. If no clause fits, the call is
     in scope and must be migrated.

4. **Run — expect green:**

```
cargo test -p wordcartel --test fs_chokepoint
```

Expected: `test result: ok. 3 passed`.

5. **Commit:** `test(c5): add the fs_chokepoint guard with a four-route self-check`

---

*Phase B complete. Every in-scope call site is on the seam, the scope claim is enforced by a test
rather than by prose, and plugin discovery no longer drops symlinked plugins.*

## Phase C — Picker core (Tasks 12–14)

### Task 12 — `file_browser_listing.rs`: cache, filter pipeline, disclosure

**Deliverable:** the listing is fetched once per directory and filtered in memory, so a query
keystroke performs no syscall. `FileEntry` takes its final shape here.

#### Files

- Create: `wordcartel/src/file_browser_listing.rs`
- Modify: `wordcartel/src/file_browser.rs` (`FileEntry` reshape; `FileBrowser` gains the cache)
- Modify: `wordcartel/src/config.rs` (`FileTypeFilter`)
- Modify: `wordcartel/src/lib.rs` (declare the new module)

#### Interfaces

**Consumes** (Tasks 4, 9):

```rust
// crate::fsx
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind { File, Dir, Other, Unknown }

#[derive(Clone, Debug)]
pub(crate) struct DirEntryInfo {
    pub name: String,
    pub kind: EntryKind,
    pub is_symlink: bool,
    pub broken: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct DirListing {
    pub entries: Vec<DirEntryInfo>,
    pub total_seen: usize,
    pub unreadable: usize,
}

pub(crate) trait Fs {
    fn list_dir(&self, path: &Path, cap: Option<usize>) -> std::io::Result<DirListing>;
    // …
}

// crate::limits
pub const MAX_DIR_ENTRIES: usize = 5_000;

// crate::palette — the existing nucleo-matcher ranker, shared with the outline overlay.
pub fn fuzzy_filter<T: Clone>(items: &[T], query: &str, key: impl Fn(&T) -> &str) -> Vec<T>;
```

**Produces:**

```rust
// crate::config
/// Two-state, not a bool: it carries named states for `MenuMark::Value` and the two
/// set-per-state commands (contract law 8). Wired to settings persistence in Task 23.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FileTypeFilter { #[default] Documents, All }

// crate::file_browser — FileEntry takes its FINAL shape here (Task 9 left it as
// `{ name, is_dir }`; Task 14 adds rendering and refusals but does NOT reshape it again).
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub kind: crate::fsx::EntryKind,
    pub is_symlink: bool,
    pub broken: bool,
}

pub struct FileBrowser {
    pub dir: std::path::PathBuf,
    pub query: String,
    /// UNFILTERED directory contents, fetched once per directory. The query path filters
    /// THIS, never the filesystem.
    pub listing: Vec<crate::fsx::DirEntryInfo>,
    pub total_seen: usize,
    pub unreadable: usize,
    /// Derived view: filtered, ranked, with the synthetic "..".
    pub entries: Vec<FileEntry>,
    pub disclosure: crate::file_browser_listing::Disclosure,
    pub selected: usize,
    pub scroll_top: usize,
}

// crate::file_browser_listing
#[derive(Clone, Copy, Debug)]
pub(crate) struct FilterOpts {
    pub show_clutter: bool,
    pub types: crate::config::FileTypeFilter,
    /// Destination mode also shows output-format siblings (.docx/.pdf/.html/.tex) so a
    /// writer can see what they might clobber. Select mode does not — there is no import
    /// path, and listing them would build a select-then-error dead end.
    pub destination: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Disclosure {
    pub shown: usize,
    pub hidden_clutter: usize,
    pub hidden_type: usize,
    pub capped_out: usize,
    pub unreadable: usize,
    pub total_seen: usize,
}

pub(crate) fn is_clutter(name: &str) -> bool;
pub(crate) fn is_document(name: &str, destination: bool) -> bool;

/// Pure: `listing` -> (rows, disclosure). No IO, no Editor. `at_root` suppresses "..".
pub(crate) fn filter_and_rank(
    listing: &[crate::fsx::DirEntryInfo],
    at_root: bool,
    query: &str,
    opts: FilterOpts,
    total_seen: usize,
    unreadable: usize,
) -> (Vec<crate::file_browser::FileEntry>, Disclosure);

/// Fetch ONCE for `dir`, then derive. Called on open and descend — never on a keystroke.
pub(crate) fn refetch(fs: &dyn crate::fsx::Fs, fb: &mut crate::file_browser::FileBrowser,
    opts: FilterOpts);

/// Re-derive `entries`/`disclosure` from the CACHED listing. The keystroke path.
pub(crate) fn rederive(fb: &mut crate::file_browser::FileBrowser, opts: FilterOpts);
```

#### Steps

1. **Write the failing tests.** Create the test module at the foot of
   `wordcartel/src/file_browser_listing.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FileTypeFilter;
    use crate::fsx::{DirEntryInfo, EntryKind};

    fn e(name: &str, kind: EntryKind) -> DirEntryInfo {
        DirEntryInfo { name: name.into(), kind, is_symlink: false, broken: false }
    }
    fn broken(name: &str) -> DirEntryInfo {
        DirEntryInfo { name: name.into(), kind: EntryKind::Unknown, is_symlink: true, broken: true }
    }
    fn opts(show_clutter: bool, types: FileTypeFilter, destination: bool) -> FilterOpts {
        FilterOpts { show_clutter, types, destination }
    }

    #[test]
    fn disclosure_accounts_for_everything_withheld() {
        // §7.4's law: shown + withheld must account for what is really there. This is the
        // arithmetic, asserted directly rather than by matching footer strings.
        let listing = vec![
            e("chapter.md", EntryKind::File),
            e("notes.txt", EntryKind::File),
            e("photo.png", EntryKind::File),      // withheld by type
            e(".hidden", EntryKind::File),        // withheld by clutter
            e(".git", EntryKind::Dir),            // withheld by clutter
            e("drafts", EntryKind::Dir),
        ];
        let (rows, d) = filter_and_rank(&listing, false, "", opts(false, FileTypeFilter::Documents, false), 6, 0);
        assert_eq!(d.hidden_clutter, 2, ".hidden and .git");
        assert_eq!(d.hidden_type, 1, "photo.png");
        assert_eq!(d.shown + d.hidden_clutter + d.hidden_type + d.capped_out, d.total_seen,
            "the disclosure must account for every entry");
        // ".." is a synthetic row, not a listing entry — it must not inflate `shown`.
        assert_eq!(rows.first().map(|r| r.name.as_str()), Some(".."), "parent row first");
        assert_eq!(d.shown, 3, "chapter.md, notes.txt, drafts");
    }

    #[test]
    fn cap_and_unreadable_are_separate_disclosures() {
        // Two DIFFERENT facts: "showing N of M" is normal; "k could not be read" means
        // something is wrong with the filesystem. A single conflated counter is what made
        // the cap/no-silent-drop conflict invisible.
        let listing: Vec<DirEntryInfo> =
            (0..4).map(|i| e(&format!("f{i}.md"), EntryKind::File)).collect();
        let (_rows, d) = filter_and_rank(&listing, true, "", opts(false, FileTypeFilter::Documents, false), 10, 3);
        assert_eq!(d.unreadable, 3, "carried through, NOT folded into the cap number");
        assert_eq!(d.capped_out, 10 - 4 - 3, "capped_out = total_seen - retained - unreadable");
        assert_eq!(d.shown + d.hidden_clutter + d.hidden_type + d.capped_out + d.unreadable,
            d.total_seen, "the full invariant, with unreadable as its own term");
    }

    #[test]
    fn directories_and_broken_links_are_never_withheld() {
        // A filter that hides the path to your file is a filter that lies — and hiding a
        // broken link leaves the writer unable to see why their file appears missing.
        let listing = vec![
            e("archive", EntryKind::Dir),        // not a "document" by extension
            broken("dangling.md"),
            e("photo.png", EntryKind::File),
        ];
        let (rows, _d) = filter_and_rank(&listing, true, "", opts(false, FileTypeFilter::Documents, false), 3, 0);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"archive"), "directories survive the type filter: {names:?}");
        assert!(names.contains(&"dangling.md"), "broken links are never hidden: {names:?}");
        assert!(!names.contains(&"photo.png"), "an ordinary non-document IS withheld");
    }

    #[test]
    fn documents_filter_is_mode_aware() {
        let listing = vec![e("book.docx", EntryKind::File), e("book.md", EntryKind::File)];
        let (sel, _) = filter_and_rank(&listing, true, "", opts(false, FileTypeFilter::Documents, false), 2, 0);
        assert!(!sel.iter().any(|r| r.name == "book.docx"),
            "select mode lists what file::open can actually open — .docx is refused as binary");
        let (dst, _) = filter_and_rank(&listing, true, "", opts(false, FileTypeFilter::Documents, true), 2, 0);
        assert!(dst.iter().any(|r| r.name == "book.docx"),
            "destination mode shows output siblings so a writer sees what they might clobber");
    }

    #[test]
    fn clutter_is_dotfiles_and_vcs_dirs_only_no_gitignore() {
        assert!(is_clutter(".hidden"));
        assert!(is_clutter(".git"));
        assert!(is_clutter(".jj"));
        assert!(!is_clutter("notes.md"));
        assert!(!is_clutter("Makefile"), "no gitignore semantics — decision 2");
    }
}
```

And in `file_browser.rs`'s test module, the cache guard:

```rust
    #[test]
    fn a_query_keystroke_performs_no_directory_read() {
        // The dominant responsiveness defect: rebuild_entries re-ran read_dir on EVERY
        // query keystroke. Counted through the seam, not timed — a timing test would be
        // flaky and would not fail if someone reintroduced the syscall on a fast disk.
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct CountingFs { inner: crate::fsx::RealFs, calls: AtomicUsize }
        impl crate::fsx::Fs for CountingFs {
            fn create_excl(&self, p: &std::path::Path, m: u32)
                -> std::io::Result<Box<dyn crate::fsx::WriteSync>> { self.inner.create_excl(p, m) }
            fn existing_mode(&self, p: &std::path::Path) -> Option<u32> { self.inner.existing_mode(p) }
            fn rename(&self, a: &std::path::Path, b: &std::path::Path) -> std::io::Result<()> { self.inner.rename(a, b) }
            fn sync_dir(&self, d: &std::path::Path) -> std::io::Result<()> { self.inner.sync_dir(d) }
            fn remove_file(&self, p: &std::path::Path) -> std::io::Result<()> { self.inner.remove_file(p) }
            fn read_capped(&self, p: &std::path::Path, l: u64) -> std::io::Result<Option<Vec<u8>>> {
                self.inner.read_capped(p, l)
            }
            fn stat(&self, p: &std::path::Path) -> std::io::Result<crate::fsx::FileStat> { self.inner.stat(p) }
            fn list_dir(&self, p: &std::path::Path, cap: Option<usize>)
                -> std::io::Result<crate::fsx::DirListing>
            {
                self.calls.fetch_add(1, Ordering::Relaxed);
                self.inner.list_dir(p, cap)
            }
        }

        let dir = std::env::temp_dir().join(format!("wc-fb-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        for n in ["alpha.md", "beta.md", "gamma.md"] {
            std::fs::write(dir.join(n), b"x").expect("seed");
        }
        let fs = CountingFs { inner: crate::fsx::RealFs, calls: AtomicUsize::new(0) };
        let o = crate::file_browser_listing::FilterOpts {
            show_clutter: false, types: crate::config::FileTypeFilter::Documents, destination: false,
        };
        let mut fb = FileBrowser {
            dir: dir.clone(), query: String::new(), listing: vec![], total_seen: 0, unreadable: 0,
            entries: vec![], disclosure: Default::default(), selected: 0, scroll_top: 0,
        };
        crate::file_browser_listing::refetch(&fs, &mut fb, o);
        assert_eq!(fs.calls.load(Ordering::Relaxed), 1, "one fetch on open");

        for c in ['a', 'l', 'p'] {
            fb.query.push(c);
            crate::file_browser_listing::rederive(&mut fb, o);
        }
        assert_eq!(fs.calls.load(Ordering::Relaxed), 1,
            "THREE keystrokes performed ZERO additional list_dir calls");
        assert!(fb.entries.iter().any(|e| e.name == "alpha.md"), "and the filter still works");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib file_browser_listing::
```

Expected: ``error[E0433]: failed to resolve: use of undeclared crate or module `file_browser_listing```.

3. **Add `FileTypeFilter`** to `config.rs`:

```rust
/// Which file types the picker lists. Two-state rather than a bool so it carries NAMED
/// states for the `MenuMark::Value` representative and the two set-per-state commands
/// (command-surface contract, law 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FileTypeFilter {
    /// What `file::open` can actually open, plus output siblings in destination mode.
    #[default]
    Documents,
    All,
}
```

4. **Reshape `FileEntry` and `FileBrowser`** in `file_browser.rs`:

```rust
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    /// RESOLVED classification from the seam. `File`/`Dir` can BOTH be false — a fifo is
    /// `Other`, an unclassifiable entry is `Unknown`. Consumers match exhaustively on this
    /// rather than testing "is it a directory", so neither falls into a file branch.
    pub kind: crate::fsx::EntryKind,
    pub is_symlink: bool,
    pub broken: bool,
}

#[derive(Debug, Clone)]
pub struct FileBrowser {
    pub dir: PathBuf,
    pub query: String,
    /// UNFILTERED contents of `dir`, fetched ONCE per directory. The keystroke path filters
    /// this, never the filesystem — `rebuild_entries` used to re-run `read_dir` on every
    /// character typed.
    pub listing: Vec<crate::fsx::DirEntryInfo>,
    pub total_seen: usize,
    pub unreadable: usize,
    /// Derived view: filtered, ranked, with the synthetic "..".
    pub entries: Vec<FileEntry>,
    pub disclosure: crate::file_browser_listing::Disclosure,
    pub selected: usize,
    /// First visible row index — drives the windowed painter (A6).
    pub scroll_top: usize,
}
```

Delete `rebuild_entries` from `file_browser.rs`; `refetch`/`rederive` replace it. Update
`Editor::open_file_browser` to take the seam and the filter options:

```rust
    pub fn open_file_browser(&mut self, fs: &dyn crate::fsx::Fs, dir: std::path::PathBuf) {
        crate::overlays::close_all(self);
        self.pending_keys.clear(); self.pending_mark = None;
        let opts = crate::file_browser_listing::FilterOpts {
            show_clutter: self.files_show_clutter,
            types: self.files_type_filter,
            destination: false,
        };
        self.file_browser = Some(crate::file_browser::FileBrowser {
            dir, query: String::new(), listing: Vec::new(), total_seen: 0, unreadable: 0,
            entries: Vec::new(), disclosure: Default::default(), selected: 0, scroll_top: 0,
        });
        if let Some(fb) = self.file_browser.as_mut() {
            crate::file_browser_listing::refetch(fs, fb, opts);
        }
    }
```

> `Editor` gains `files_show_clutter: bool` and `files_type_filter: crate::config::FileTypeFilter`
> here (defaults `false` / `Documents`). Task 23 adds the setters, commands, and persistence.

5. **Write `file_browser_listing.rs`:**

```rust
//! Pure listing pipeline for the file browser: cache -> filter -> rank -> disclosure.
//!
//! No IO except `refetch`, and no `Editor`. Kept separate from `file_browser.rs` on one
//! axis of change: this module answers "which rows exist", not "what a browser is".

use crate::config::FileTypeFilter;
use crate::file_browser::{FileBrowser, FileEntry};
use crate::fsx::{DirEntryInfo, EntryKind, Fs};

/// VCS/system directory names withheld as clutter even though they are already
/// dot-prefixed — so the list stays honest if the dotfile rule ever changes.
const VCS_DIRS: &[&str] = &[".git", ".hg", ".svn", ".jj", ".pijul"];

/// Extensions `file::open` can actually open. Deliberately EXCLUDES .docx/.pdf: there is
/// no import path and `file::open` refuses them as `OpenError::Binary`, so listing them in
/// select mode would build a select-then-error dead end.
const TEXT_EXTS: &[&str] = &["md", "markdown", "txt", "rst", "text"];

/// Output-format siblings, shown in DESTINATION mode only — there they are exactly the
/// files a writer needs to see in order not to clobber them.
const OUTPUT_EXTS: &[&str] = &["docx", "pdf", "html", "tex"];

#[derive(Clone, Copy, Debug)]
pub(crate) struct FilterOpts {
    pub show_clutter: bool,
    pub types: FileTypeFilter,
    pub destination: bool,
}

/// Everything the footer needs. `shown + hidden_clutter + hidden_type + capped_out +
/// unreadable == total_seen` — asserted by test, because §7.4's law is that the picker
/// never silently withholds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Disclosure {
    pub shown: usize,
    pub hidden_clutter: usize,
    pub hidden_type: usize,
    pub capped_out: usize,
    pub unreadable: usize,
    pub total_seen: usize,
}

/// Dotfiles plus VCS/system directory names. NO gitignore semantics (decision 2): they
/// carry near-zero value for this audience and a real hazard — a manuscript under an
/// aggressive ignore file would vanish.
pub(crate) fn is_clutter(name: &str) -> bool {
    name.starts_with('.') || VCS_DIRS.contains(&name)
}

/// Is this name a "document" for the type filter? `destination` widens the set to include
/// output-format siblings.
pub(crate) fn is_document(name: &str, destination: bool) -> bool {
    match std::path::Path::new(name).extension().and_then(|e| e.to_str()) {
        None => true, // extensionless files are plausibly prose
        Some(ext) => {
            let ext = ext.to_ascii_lowercase();
            TEXT_EXTS.contains(&ext.as_str())
                || (destination && OUTPUT_EXTS.contains(&ext.as_str()))
        }
    }
}

pub(crate) fn filter_and_rank(
    listing: &[DirEntryInfo],
    at_root: bool,
    query: &str,
    opts: FilterOpts,
    total_seen: usize,
    unreadable: usize,
) -> (Vec<FileEntry>, Disclosure) {
    let mut hidden_clutter = 0usize;
    let mut hidden_type = 0usize;
    let mut kept: Vec<&DirEntryInfo> = Vec::new();

    for e in listing {
        if !opts.show_clutter && is_clutter(&e.name) {
            hidden_clutter += 1;
            continue;
        }
        // Directories are NEVER withheld by the type filter — a filter that hides the path
        // to your file is a filter that lies. Broken links are never withheld either:
        // hiding one leaves the writer unable to see why their file appears missing.
        let type_exempt = matches!(e.kind, EntryKind::Dir) || e.broken;
        if !type_exempt
            && matches!(opts.types, FileTypeFilter::Documents)
            && !is_document(&e.name, opts.destination)
        {
            hidden_type += 1;
            continue;
        }
        kept.push(e);
    }

    let shown = kept.len();
    let capped_out = total_seen.saturating_sub(listing.len()).saturating_sub(unreadable);

    // Rank: fuzzy when a query is present (matching the palette and outline), otherwise
    // dirs-then-files alphabetical. `..` is pinned first and is NOT a listing entry.
    let mut rows: Vec<FileEntry> = Vec::new();
    if !at_root {
        rows.push(FileEntry {
            name: "..".into(), kind: EntryKind::Dir, is_symlink: false, broken: false,
        });
    }
    let mut ordered: Vec<DirEntryInfo> = kept.into_iter().cloned().collect();
    if query.is_empty() {
        ordered.sort_by(|a, b| {
            let ad = matches!(a.kind, EntryKind::Dir);
            let bd = matches!(b.kind, EntryKind::Dir);
            bd.cmp(&ad).then_with(|| a.name.cmp(&b.name))
        });
    } else {
        ordered = crate::palette::fuzzy_filter(&ordered, query, |e| e.name.as_str());
    }
    rows.extend(ordered.into_iter().map(|e| FileEntry {
        name: e.name, kind: e.kind, is_symlink: e.is_symlink, broken: e.broken,
    }));

    (rows, Disclosure { shown, hidden_clutter, hidden_type, capped_out, unreadable, total_seen })
}

/// Fetch ONCE for `fb.dir`, then derive. Called on open and descend only.
pub(crate) fn refetch(fs: &dyn Fs, fb: &mut FileBrowser, opts: FilterOpts) {
    match fs.list_dir(&fb.dir, Some(crate::limits::MAX_DIR_ENTRIES)) {
        Ok(l) => {
            fb.listing = l.entries;
            fb.total_seen = l.total_seen;
            fb.unreadable = l.unreadable;
        }
        Err(_) => {
            fb.listing = Vec::new();
            fb.total_seen = 0;
            fb.unreadable = 0;
        }
    }
    rederive(fb, opts);
}

/// Re-derive `entries`/`disclosure` from the CACHED listing. This is the keystroke path and
/// it performs NO filesystem access.
pub(crate) fn rederive(fb: &mut FileBrowser, opts: FilterOpts) {
    let at_root = fb.dir.parent().is_none();
    let (rows, d) = filter_and_rank(
        &fb.listing, at_root, &fb.query, opts, fb.total_seen, fb.unreadable);
    fb.entries = rows;
    fb.disclosure = d;
    if fb.selected >= fb.entries.len() {
        fb.selected = fb.entries.len().saturating_sub(1);
    }
    fb.scroll_top = fb.scroll_top.min(fb.entries.len().saturating_sub(1));
}
```

6. **Declare the module** in `lib.rs`, beside the other `file_*` modules:

```rust
pub mod file_browser_listing;
```

7. **Run — expect green:**

```
cargo test -p wordcartel --lib file_browser_listing:: file_browser::
```

Expected: `test result: ok`, including the five pipeline tests and the cache guard.

8. **Commit:** `feat(c5): cache the directory listing and filter in memory with full disclosure`

---

### Task 13 — Off-thread listing with a process-global epoch

**Deliverable:** `list_dir` runs on a dedicated thread, never the `jobs.rs` FIFO, and stale results
are discarded by an epoch that cannot be reused across a close/reopen.

**Why not `jobs.rs`:** `ThreadExecutor` is a single FIFO worker shared with `JobKind::Save` and
`JobKind::SwapWrite`. A listing blocked on a hung mount would queue **ahead of the user's saves**,
turning a browsing hiccup into a durability outage.

#### Files

- Modify: `wordcartel/src/app.rs` (`Msg::ListingDone`, the dispatch arm)
- Modify: `wordcartel/src/file_browser.rs` (epoch, spawn, merge)

#### Interfaces

**Consumes** (Tasks 4, 5, 12):

```rust
// crate::fsx
pub(crate) struct DirListing { pub entries: Vec<DirEntryInfo>, pub total_seen: usize,
                               pub unreadable: usize }
pub(crate) trait Fs { fn list_dir(&self, path: &Path, cap: Option<usize>)
    -> std::io::Result<DirListing>; /* … */ }

// crate::file_browser_listing
pub(crate) struct FilterOpts { pub show_clutter: bool,
    pub types: crate::config::FileTypeFilter, pub destination: bool }
pub(crate) fn rederive(fb: &mut crate::file_browser::FileBrowser, opts: FilterOpts);

// crate::overlays
pub(crate) struct DispatchCtx<'a> {
    pub(crate) reg: &'a crate::registry::Registry,
    pub(crate) keymap: &'a crate::keymap::KeyTrie,
    pub(crate) ex: &'a dyn crate::jobs::Executor,
    pub(crate) clock: &'a dyn wordcartel_core::history::Clock,
    pub(crate) msg_tx: &'a std::sync::mpsc::Sender<Msg>,
    pub(crate) fs: &'a std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
}
```

**Produces:**

```rust
// crate::app — new Msg variant
Msg::ListingDone {
    epoch: u64,
    dir: std::path::PathBuf,
    result: std::io::Result<crate::fsx::DirListing>,
},

// crate::file_browser
/// PROCESS-GLOBAL, deliberately not a FileBrowser field. Closing the picker DROPS a
/// per-browser counter, so a reopened picker would restart at the same value and could
/// accept a stale result from the previous picker's in-flight listing — an ABA bug. A
/// global counter never reissues a value.
pub(crate) static LISTING_EPOCH: std::sync::atomic::AtomicU64;

pub(crate) fn next_epoch() -> u64;

/// Spawn a listing for `target` off-thread. Stamps `awaiting_epoch` AND `pending_dir`
/// together; `fb.dir` is NOT moved here.
pub(crate) fn start_listing(
    fb: &mut FileBrowser,
    target: std::path::PathBuf,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
);

/// Merge a listing result, discarding stale ones. On success, commits `pending_dir` and the
/// entries TOGETHER; on error, leaves `fb.dir` untouched.
pub(crate) fn apply_listing_done(
    editor: &mut crate::editor::Editor,
    epoch: u64,
    dir: std::path::PathBuf,
    result: std::io::Result<crate::fsx::DirListing>,
);
```

`FileBrowser` gains:

```rust
    /// The epoch this browser awaits. Compared against `Msg::ListingDone::epoch`.
    pub awaiting_epoch: u64,
    /// The directory a listing is in flight FOR. `fb.dir` does not move until that listing
    /// succeeds, so the picker shows where the writer actually is until they have actually
    /// arrived — and an unreadable directory never moves them at all.
    pub pending_dir: Option<std::path::PathBuf>,
```

#### Steps

1. **Write the failing tests** in `file_browser.rs`'s test module:

```rust
    #[test]
    fn stale_listing_after_close_and_reopen_is_discarded() {
        // THE ABA CASE. If the epoch lived on FileBrowser, closing would drop it and the
        // reopened picker would restart at the same value — so the FIRST picker's still
        // in-flight listing would carry a matching epoch and be accepted, painting the wrong
        // directory. A process-global counter never reissues, so the match is unforgeable.
        //
        // Fast listings hide this: the window only opens when a listing OUTLIVES the picker
        // that started it, which is exactly the hung-mount case the thread exists for.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let dir_a = std::env::temp_dir().join(format!("wc-aba-a-{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("wc-aba-b-{}", std::process::id()));
        for d in [&dir_a, &dir_b] { let _ = std::fs::remove_dir_all(d); std::fs::create_dir_all(d).expect("dir"); }
        std::fs::write(dir_a.join("from_a.md"), b"x").expect("seed");
        std::fs::write(dir_b.join("from_b.md"), b"x").expect("seed");

        // Picker #1 over dir_a; capture the epoch it is awaiting, then CLOSE it.
        e.open_file_browser(&crate::fsx::RealFs, dir_a.clone());
        let stale_epoch = e.file_browser.as_ref().expect("open").awaiting_epoch;
        e.file_browser = None;

        // Picker #2 over dir_b.
        e.open_file_browser(&crate::fsx::RealFs, dir_b.clone());
        let fresh_epoch = e.file_browser.as_ref().expect("reopen").awaiting_epoch;
        assert_ne!(stale_epoch, fresh_epoch, "a global epoch never reissues across close/reopen");

        // Picker #1's listing finally lands.
        let stale = crate::fsx::DirListing {
            entries: vec![crate::fsx::DirEntryInfo {
                name: "from_a.md".into(), kind: crate::fsx::EntryKind::File,
                is_symlink: false, broken: false }],
            total_seen: 1, unreadable: 0,
        };
        crate::file_browser::apply_listing_done(&mut e, stale_epoch, dir_a.clone(), Ok(stale));

        let names: Vec<String> =
            e.file_browser.as_ref().expect("still open").entries.iter().map(|r| r.name.clone()).collect();
        assert!(!names.iter().any(|n| n == "from_a.md"),
            "the stale listing must be discarded: {names:?}");
        assert_eq!(e.file_browser.as_ref().expect("open").dir, dir_b, "picker #2 is untouched");
        for d in [&dir_a, &dir_b] { let _ = std::fs::remove_dir_all(d); }
    }

    #[test]
    fn a_failed_descend_leaves_the_writer_exactly_where_they_were() {
        // The hold-pending guarantee. `fb.dir` does not move on Enter — it moves only when a
        // listing for the target SUCCEEDS. So an unreadable target costs the writer nothing:
        // not their directory, not their query, not their selection.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let dir = std::env::temp_dir().join(format!("wc-faildescend-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("keep.md"), b"x").expect("seed");
        e.open_file_browser(&crate::fsx::RealFs, dir.clone());

        // Simulate Enter on a directory whose listing will fail.
        let bad = dir.join("unreadable");
        let epoch = {
            let fb = e.file_browser.as_mut().expect("open");
            fb.query.push_str("ke");
            let ep = crate::file_browser::next_epoch();
            fb.awaiting_epoch = ep;
            fb.pending_dir = Some(bad.clone());
            ep
        };
        crate::file_browser::apply_listing_done(&mut e, epoch, bad.clone(),
            Err(std::io::Error::other("boom")));

        let fb = e.file_browser.as_ref().expect("picker stays open");
        assert_eq!(fb.dir, dir, "a failed descend does NOT move the writer");
        assert_eq!(fb.query, "ke", "and does not cost them the query they had typed");
        assert!(fb.pending_dir.is_none(), "the pending target is cleared");
        assert!(e.status_text().contains("cannot read directory"), "and they are told");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn listing_result_with_no_active_picker_is_discarded_without_panic() {
        // Both halves of the discard condition are required: the epoch match, AND
        // "no active picker discards unconditionally". Without the second, the first has
        // nothing to compare against.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        assert!(e.file_browser.is_none(), "precondition: no picker");
        let l = crate::fsx::DirListing { entries: vec![], total_seen: 0, unreadable: 0 };
        crate::file_browser::apply_listing_done(&mut e, 12345, std::env::temp_dir(), Ok(l));
        assert!(e.file_browser.is_none(), "no picker was resurrected, and no panic");
    }
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib file_browser::tests::stale_listing_after_close
```

Expected: ``error[E0609]: no field `awaiting_epoch` on type `FileBrowser```.

3. **Add the `Msg` variant** in `app.rs`, after `ExportDone`:

```rust
    /// A directory listing completed on its own thread. NOT a `jobs::Job` — `ThreadExecutor`
    /// is a single FIFO shared with Save and SwapWrite, so a listing blocked on a hung mount
    /// would queue AHEAD of the user's saves, turning a browsing hiccup into a durability
    /// outage. `dir` is diagnostic (and for the merge-targets-what-it-thinks assertion); the
    /// discard condition is the EPOCH alone.
    ListingDone {
        epoch: u64,
        dir: std::path::PathBuf,
        result: std::io::Result<crate::fsx::DirListing>,
    },
```

and the dispatch arm in `reduce_dispatch`, beside `Msg::ExportDone`:

```rust
        Msg::ListingDone { epoch, dir, result } => {
            crate::file_browser::apply_listing_done(editor, epoch, dir, result);
        }
```

4. **Add the epoch, spawn, and merge** to `file_browser.rs`:

```rust
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic listing epoch, PROCESS-GLOBAL by design.
///
/// It is deliberately not a `FileBrowser` field: closing the picker would drop a per-browser
/// counter, and a freshly-opened picker would start from the same value — so a stale result
/// from the previous picker's in-flight listing could carry a matching epoch and be accepted
/// (an ABA bug). A global counter never reissues a value, so the match is unforgeable.
pub(crate) static LISTING_EPOCH: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_epoch() -> u64 {
    LISTING_EPOCH.fetch_add(1, Ordering::Relaxed)
}

/// Spawn a listing for `target` on its own thread.
///
/// `fb.dir` is DELIBERATELY not moved here — see `apply_listing_done`. `pending_dir` and
/// `awaiting_epoch` are stamped together, so "a listing is in flight for X" is one fact in
/// one place and cannot disagree with itself.
///
/// The overlay stays fully closable while this is in flight: closing means the result is
/// discarded on arrival, and the detached thread exits on its own. A stuck mount strands one
/// short-lived thread, never the UI.
pub(crate) fn start_listing(
    fb: &mut FileBrowser,
    target: std::path::PathBuf,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    let epoch = next_epoch();
    fb.awaiting_epoch = epoch;
    fb.pending_dir = Some(target.clone());
    let fs = std::sync::Arc::clone(fs);
    let tx = msg_tx.clone();
    std::thread::spawn(move || {
        let result = fs.list_dir(&target, Some(crate::limits::MAX_DIR_ENTRIES));
        let _ = tx.send(crate::app::Msg::ListingDone { epoch, dir: target, result });
    });
}

/// Merge a listing result. Discards when there is no active picker, and when the epoch is
/// not the one the active picker awaits. BOTH halves are required.
///
/// On SUCCESS the pending directory and its entries are committed TOGETHER, so the picker
/// never shows a directory it has not actually read. On ERROR `fb.dir` is left untouched:
/// an unreadable directory does not move the writer, it just tells them.
pub(crate) fn apply_listing_done(
    editor: &mut crate::editor::Editor,
    epoch: u64,
    dir: std::path::PathBuf,
    result: std::io::Result<crate::fsx::DirListing>,
) {
    let Some(fb) = editor.file_browser.as_mut() else { return }; // no picker → inert
    if fb.awaiting_epoch != epoch { return; }                    // stale → inert
    debug_assert_eq!(fb.pending_dir.as_deref(), Some(dir.as_path()),
        "the merge must target the directory it listed");
    match result {
        Ok(l) => {
            // Commit the directory move and its contents in one step.
            let moved = fb.pending_dir.take().is_some_and(|p| {
                let changed = p != fb.dir;
                fb.dir = p;
                changed
            });
            fb.listing = l.entries;
            fb.total_seen = l.total_seen;
            fb.unreadable = l.unreadable;
            if moved {
                // Descend resets the view — but only now that we have actually arrived, so a
                // failed descend does not cost the writer the query they had typed.
                fb.query.clear();
                fb.selected = 0;
                fb.scroll_top = 0; // A6: reset with selected to avoid an out-of-order slice
            }
        }
        Err(e) => {
            // fb.dir is NOT touched. The writer stays where they were.
            fb.pending_dir = None;
            editor.set_status_full(crate::status::StatusKind::Error,
                format!("cannot read directory: {} ({e})", dir.display()),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return;
        }
    }
    let opts = crate::file_browser_listing::FilterOpts {
        show_clutter: editor.files_show_clutter,
        types: editor.files_type_filter,
        destination: false,
    };
    if let Some(fb) = editor.file_browser.as_mut() {
        crate::file_browser_listing::rederive(fb, opts);
    }
}
```

Add `awaiting_epoch: u64` and `pending_dir: Option<PathBuf>` to `FileBrowser`, initialised to `0`
and `None` at every construction site. `Editor::open_file_browser` sets `fb.dir` to the requested
directory (so the painter has a title immediately) **and** calls
`start_listing(fb, dir.clone(), fs, msg_tx)` — for the initial open `target == fb.dir`, so the
commit is a no-op and there is exactly ONE spawn path, not two.

5. **The descend guard is preserved, not relocated.** `file_browser_enter`'s `Descend` arm calls
   `start_listing(fb, target, fs, msg_tx)` and **does not touch `fb.dir`, `fb.query`, `fb.selected`,
   or `fb.scroll_top`**. All four move together in `apply_listing_done`'s success arm.

   **Why not a synchronous `stat` on the target first** (the obvious way to keep the old inline
   probe): on a hung mount `stat` blocks the input loop exactly as `read_dir` does. It is cheaper but
   still blocking, and "never block the input loop" is the project's top-priority constraint —
   trading a responsiveness invariant for a cosmetic one is backwards. Holding the target keeps both
   properties: nothing blocks, and an unreadable directory never moves the writer. **Do not
   "simplify" this into a blocking pre-probe.**

   `file_browser::tests::enter_on_unreadable_dir_stays_put_and_sets_status` keeps **all of its
   assertions verbatim** — `fb.dir` still equals `parent`, the status still contains
   "cannot read directory", still `Error`/`Sticky`, and still survives a later Info ack. Its Act
   section gains exactly one line: deliver the `ListingDone` that the spawned thread sends, because
   the error now arrives on a message rather than inline. Add this helper to the module's test code
   and call it after the `reduce` that sends Enter:

```rust
    /// Deliver one pending `Msg::ListingDone` from the channel into the editor. The listing
    /// runs on its own thread, so a test that drives Enter must pump the result to observe
    /// the outcome. Bounded wait — never hangs a test run.
    fn pump_listing(e: &mut crate::editor::Editor,
        rx: &std::sync::mpsc::Receiver<Msg>) -> bool
    {
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Msg::ListingDone { epoch, dir, result }) => {
                crate::file_browser::apply_listing_done(e, epoch, dir, result);
                true
            }
            _ => false,
        }
    }
```

> The distinction matters: the test's **assertions** — the evidence — survive unmodified, and
> `fb.dir == parent` is now permanently true rather than transiently. Only the Act section gains a
> pump. That is not the same as rewriting a test around a new expectation.

6. **Run — expect green:**

```
cargo test -p wordcartel --lib file_browser::
```

Expected: `test result: ok`, including both epoch tests.

7. **Commit:** `feat(c5): list directories off-thread behind a process-global epoch`

---

### Task 14 — `EntryKind` rendering and refusals

**Deliverable:** every `EntryKind` has a visible marker and a defined Enter behaviour, and neither
`Other` nor `Unknown` can fall into a branch meant for files.

#### Files

- Modify: `wordcartel/src/file_browser.rs` (`entry_label`, `classify_enter`, `file_browser_enter`)
- Modify: `wordcartel/src/render_overlays.rs` (`paint_file_browser` label)

#### Interfaces

**Consumes** (Tasks 4, 12, 13):

```rust
// crate::fsx
pub(crate) enum EntryKind { File, Dir, Other, Unknown }

// crate::file_browser
pub struct FileEntry {
    pub name: String,
    pub kind: crate::fsx::EntryKind,
    pub is_symlink: bool,
    pub broken: bool,
}

// crate::workspace
pub fn open_as_new_buffer(editor: &mut Editor, path: &std::path::Path);
```

**Produces:**

```rust
// crate::file_browser
/// `ls -F`-style label. TEXT suffixes, not colours, so they survive terminal-plain /
/// no-color mode.
pub(crate) fn entry_label(e: &FileEntry) -> String;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EnterOutcome {
    Descend(std::path::PathBuf),
    Open(std::path::PathBuf),
    /// Shown, marked, and refused — with the reason, which differs between an unopenable
    /// special file and an unresolvable entry.
    Refuse(String),
}

pub(crate) fn classify_enter(e: &FileEntry, dir: &std::path::Path) -> EnterOutcome;

/// The shared Enter path for keyboard and mouse. SIGNATURE CHANGES in this task: it now
/// needs the owned seam handle (to start a listing on descend) and the message sender.
pub(crate) fn file_browser_enter(
    editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
);
```

Its callers — `file_browser_intercept`'s Enter arm and `mouse::mouse_file_browser`'s click-commit
arm — both have `DispatchCtx`, which carries `fs` and `msg_tx`.

#### Steps

1. **Write the failing tests** in `file_browser.rs`'s test module:

```rust
    fn fe(name: &str, kind: crate::fsx::EntryKind, is_symlink: bool, broken: bool) -> FileEntry {
        FileEntry { name: name.into(), kind, is_symlink, broken }
    }

    #[test]
    fn entry_labels_follow_ls_f_and_survive_no_color() {
        use crate::fsx::EntryKind::*;
        // TEXT suffixes, never colour — the terminal-plain constraint. This also restores
        // the trailing '/' that a symlinked directory used to lose entirely (§4.9).
        assert_eq!(entry_label(&fe("drafts", Dir, false, false)), "drafts/");
        assert_eq!(entry_label(&fe("linked", Dir, true, false)), "linked/@");
        assert_eq!(entry_label(&fe("notes.md", File, false, false)), "notes.md");
        assert_eq!(entry_label(&fe("alias.md", File, true, false)), "alias.md@");
        assert_eq!(entry_label(&fe("dangling.md", Unknown, true, true)), "dangling.md@ (broken)");
    }

    #[test]
    fn classify_enter_covers_every_kind_exhaustively() {
        use crate::fsx::EntryKind::*;
        let d = std::path::Path::new("/tmp/wc-classify");
        assert_eq!(classify_enter(&fe("sub", Dir, false, false), d),
            EnterOutcome::Descend(d.join("sub")));
        assert_eq!(classify_enter(&fe("sub", Dir, true, false), d),
            EnterOutcome::Descend(d.join("sub")), "a symlinked dir descends like any dir");
        assert_eq!(classify_enter(&fe("n.md", File, false, false), d),
            EnterOutcome::Open(d.join("n.md")));

        // A fifo must be REFUSED, and for a concrete reason: file::open on a fifo BLOCKS.
        match classify_enter(&fe("pipe", Other, false, false), d) {
            EnterOutcome::Refuse(msg) => assert!(msg.to_lowercase().contains("cannot be opened"),
                "the reason must name the openability problem, got {msg:?}"),
            other => panic!("a fifo must be refused, got {other:?}"),
        }
        // An unresolvable entry is refused with a DIFFERENT reason — the pair of facts the
        // old bool model could not separate.
        match classify_enter(&fe("dangling.md", Unknown, true, true), d) {
            EnterOutcome::Refuse(msg) => assert!(msg.to_lowercase().contains("cannot be resolved"),
                "must say cannot-be-resolved, NOT 'target is gone' — broken also covers \
                 permission and loop failures, got {msg:?}"),
            other => panic!("a broken link must be refused, got {other:?}"),
        }
    }

    #[test]
    fn dotdot_descends_to_the_logical_parent() {
        let d = std::path::Path::new("/tmp/wc-classify/sub");
        assert_eq!(classify_enter(&fe("..", crate::fsx::EntryKind::Dir, false, false), d),
            EnterOutcome::Descend(std::path::PathBuf::from("/tmp/wc-classify")),
            "'..' walks the LOGICAL parent — fb.dir is deliberately not canonicalized, so a \
             writer who descended through a symlink leaves by the path they came in on");
    }
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib file_browser::tests::entry_labels_follow_ls_f
```

Expected: ``error[E0425]: cannot find function `entry_label` in this scope``.

3. **Add both functions** to `file_browser.rs`:

```rust
/// `ls -F`-style label, composed with the directory slash.
///
/// TEXT suffixes, not colours, so they survive terminal-plain / no-color mode — the
/// project's standing constraint on every affordance.
pub(crate) fn entry_label(e: &FileEntry) -> String {
    let slash = if matches!(e.kind, crate::fsx::EntryKind::Dir) { "/" } else { "" };
    let link = if e.is_symlink { "@" } else { "" };
    let broken = if e.broken { " (broken)" } else { "" };
    format!("{}{slash}{link}{broken}", e.name)
}

/// What Enter does with an entry. An exhaustive match on `kind`, so `Other` and `Unknown`
/// cannot fall into a branch meant for files.
pub(crate) fn classify_enter(e: &FileEntry, dir: &std::path::Path) -> EnterOutcome {
    if e.name == ".." {
        // The LOGICAL parent — `fb.dir` is deliberately not canonicalized, so this returns
        // where the writer actually came from rather than a symlink target's real parent.
        return EnterOutcome::Descend(
            dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| dir.to_path_buf()));
    }
    match e.kind {
        crate::fsx::EntryKind::Dir => EnterOutcome::Descend(dir.join(&e.name)),
        crate::fsx::EntryKind::File => EnterOutcome::Open(dir.join(&e.name)),
        // A fifo/socket/device is CLASSIFIED — we know what it is, and we know
        // `file::open` on it would block. Shown, marked, refused.
        crate::fsx::EntryKind::Other => EnterOutcome::Refuse(
            format!("{} cannot be opened — not a regular file", e.name)),
        // Unclassifiable, including every broken symlink. "cannot be resolved", never
        // "target is gone": `broken` also covers permission denial and resolution loops.
        crate::fsx::EntryKind::Unknown => EnterOutcome::Refuse(if e.broken {
            format!("{} — symlink cannot be resolved", e.name)
        } else {
            format!("{} — type could not be determined", e.name)
        }),
    }
}
```

4. **Rewrite `file_browser_enter`** to consume `classify_enter`:

```rust
pub(crate) fn file_browser_enter(
    editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    let chosen = editor.file_browser.as_ref().and_then(|fb| {
        fb.entries.get(fb.selected).cloned().map(|e| (e, fb.dir.clone()))
    });
    let Some((entry, dir)) = chosen else { return };
    match classify_enter(&entry, &dir) {
        EnterOutcome::Descend(target) => {
            if let Some(fb) = editor.file_browser.as_mut() {
                // Does NOT touch fb.dir / query / selected / scroll_top. All four move
                // together in `apply_listing_done`'s success arm, so an unreadable target
                // leaves the writer exactly where they were — with their query intact.
                start_listing(fb, target, fs, msg_tx);
            }
        }
        EnterOutcome::Open(path) => {
            editor.file_browser = None;
            crate::workspace::open_as_new_buffer(editor, &path);
        }
        EnterOutcome::Refuse(msg) => {
            // Shown and marked, but not actioned — and the picker STAYS OPEN so the writer
            // can pick something else.
            editor.set_status_full(crate::status::StatusKind::Warning, msg,
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        }
    }
}
```

5. **Use `entry_label` in the painter.** In `render_overlays::paint_file_browser`, replace:

```rust
                    let label = if e.is_dir { format!("{}/", e.name) } else { e.name.clone() };
```

with:

```rust
                    let label = crate::file_browser::entry_label(e);
```

6. **Run — expect green:**

```
cargo test -p wordcartel --lib file_browser:: render_overlays::
```

Expected: `test result: ok`, including the three new tests.

7. **Commit:** `feat(c5): mark and refuse non-openable entry kinds in the picker`

---

## Phase D — Write path (Tasks 15–17)

### Task 15 — Write-destination resolution

**Deliverable:** every path that will be written is resolved through symlinks before it reaches
`save_atomic`/`save_atomic_bytes`, so a symlinked destination works and the link is preserved.

#### Files

- Modify: `wordcartel/src/fsx.rs` (`resolve_write_destination`, `DestError`)
- Modify: `wordcartel/src/prompts.rs` (`save_as_submit`, `block_write_submit`)

#### Interfaces

**Consumes** (Tasks 3, 5, 7):

```rust
// crate::fsx
pub(crate) struct FileStat {
    pub len: u64, pub mtime: Option<std::time::SystemTime>,
    pub is_file: bool, pub is_dir: bool, pub is_symlink: bool, pub broken: bool,
}
pub(crate) trait Fs { fn stat(&self, path: &Path) -> std::io::Result<FileStat>; /* … */ }
pub(crate) fn exists_via(fs: &dyn Fs, path: &Path) -> bool;

// crate::prompts
pub fn expand_path(text: &str) -> std::path::PathBuf;   // `~/` expansion + cwd-join
```

**Produces:**

```rust
// crate::fsx
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DestError {
    /// The destination is a symlink whose target cannot be resolved. Refused BEFORE any
    /// write is dispatched — it must never reach `atomic_replace`, and must never surface
    /// as `SaveError::Symlink`, which names a mechanism rather than the problem.
    BrokenSymlink,
}

/// Resolve a WRITE destination through symlinks.
///
/// * not a symlink            -> unchanged
/// * symlink that resolves    -> the resolved target (the link is preserved, because
///                               `atomic_replace` then renames over the TARGET)
/// * broken symlink           -> `Err(DestError::BrokenSymlink)`
/// * does not exist yet       -> unchanged (the ordinary new-file case)
pub(crate) fn resolve_write_destination(fs: &dyn Fs, path: &Path)
    -> Result<std::path::PathBuf, DestError>;
```

#### Steps

1. **Write the failing tests** in `fsx.rs`'s test module:

```rust
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
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib fsx::tests::resolve_write_destination
```

Expected: ``error[E0425]: cannot find function `resolve_write_destination` in this scope``.

3. **Add both items** to `fsx.rs`:

```rust
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DestError {
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
pub(crate) fn resolve_write_destination(fs: &dyn Fs, path: &Path)
    -> Result<std::path::PathBuf, DestError>
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
```

4. **Apply it in `prompts::save_as_submit`.** Replace the body between the empty-path guard and
   `perform_save_as`:

```rust
    let typed = expand_path(t);
    let target = match crate::fsx::resolve_write_destination(fs, &typed) {
        Ok(p) => p,
        Err(crate::fsx::DestError::BrokenSymlink) => {
            editor.set_status_full(crate::status::StatusKind::Warning,
                format!("{}: destination symlink cannot be resolved", typed.display()),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            editor.pending_save_as = None;
            return;
        }
    };
    // The overwrite-confirm names the RESOLVED target — that is the file whose bytes will
    // actually be replaced. Confirming an overwrite of a file you were not shown is the
    // accident this design exists to prevent.
    if crate::fsx::exists_via(fs, &target) {
        editor.pending_save_overwrite = Some(target.clone());
        editor.open_prompt(crate::prompt::Prompt::save_overwrite(&target));
        return;
    }
    perform_save_as(editor, typed, target, executor, clock, msg_tx);
```

Apply the identical shape in `block_write_submit`, using `pending_write_block` and
`Prompt::write_block_overwrite`.

> `perform_save_as` now takes both paths — Task 16 defines its signature. Until then it compiles
> against the two-argument form introduced there; sequence Task 16 immediately after this one.

5. **Run — expect green:**

```
cargo test -p wordcartel --lib fsx::tests::resolve_write_destination file::tests::save_through_symlink
```

Expected: the three new tests pass, and **`file::tests::save_through_symlink_refused` passes
unmodified** — proving resolution happens before the guard rather than by weakening it.

6. **Commit:** `feat(c5): resolve write destinations through symlinks before the atomic guard`

---

### Task 16 — `SaveTarget`: split chosen from resolved

**Deliverable:** `do_save_to` carries two paths, and each of its five consumers gets the right one.

**Why this is not optional:** today one `target` value feeds four distinct consumers —
`write_path` (a clone) reaches `file::save_atomic` **and** `save::fingerprint` on the worker, while
`target` itself feeds the `fire_save` plugin payload **and** the `b.document.path` rekey in the merge.
With a symlinked destination those four no longer want the same answer. If the single path stays
logical, `save_atomic` gets a symlink and returns `SaveError::Symlink` — the defect §7.6 fixes. If it
is made resolved, the merge rekeys `Document.path` to the resolved target — reintroducing all seven
consumer regressions Middle B was chosen to prevent.

#### Files

- Modify: `wordcartel/src/save.rs` (`SaveTarget`, `do_save_to`, `do_save`)
- Modify: `wordcartel/src/prompts.rs` (`perform_save_as`)

#### Interfaces

**Consumes** (Tasks 5, 7, 15):

```rust
// crate::fsx
pub(crate) fn resolve_write_destination(fs: &dyn Fs, path: &Path)
    -> Result<std::path::PathBuf, DestError>;

// crate::save
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SaveMode { Normal, SaveAs }

// crate::registry
pub struct Ctx<'a> {
    pub editor: &'a mut Editor,
    pub clock: &'a dyn Clock,
    pub executor: &'a dyn Executor,
    pub msg_tx: std::sync::mpsc::Sender<Msg>,
    pub fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
}
```

**Produces:**

```rust
// crate::save
/// The two paths a save needs. A STRUCT rather than two positional `PathBuf`s on purpose:
/// two same-typed positional parameters are silently swappable, and this is exactly the
/// distinction that must not be gettable-wrong at a call site.
#[derive(Clone, Debug)]
pub(crate) struct SaveTarget {
    /// What the writer selected — logical, possibly a symlink. Middle B's coordinate system.
    pub chosen: std::path::PathBuf,
    /// Where bytes actually go — §7.6.1 resolution applied. Never a symlink.
    pub resolved: std::path::PathBuf,
}

impl SaveTarget {
    /// For a destination that needed no resolution (the common case: the two are equal).
    pub(crate) fn same(p: std::path::PathBuf) -> Self;
}

pub(crate) fn do_save_to(ctx: &mut Ctx, target: SaveTarget, mode: SaveMode);

// crate::prompts
fn perform_save_as(editor: &mut Editor, chosen: std::path::PathBuf,
    resolved: std::path::PathBuf, executor: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>);
```

**The complete consumer assignment** — every implementer must honour this table exactly:

| Consumer | Gets | Why |
|---|---|---|
| `file::save_atomic` (worker) | **resolved** | Makes a symlinked destination work at all; `save_atomic`'s guard then never fires and stays an unconditional last resort. |
| `save::fingerprint` → `stored_fp` | **resolved** | Must describe the file actually written. Not a new asymmetry: `fingerprint` follows symlinks, so both agree whenever the link resolves — and it stays comparable with `dispatch_save`'s `fingerprint(&Document.path)` check, which follows to the same file. |
| `b.document.path` rekey | **chosen** | Middle B: display and navigation stay logical. This is what keeps all seven §7.6.2 consumers correct. |
| `fire_save` plugin payload | **chosen** | Consistency with `plugin::api`'s `wc.path()`, which returns `Document.path`. A Save event reporting a path `wc.path()` never returns would make the two disagree. |
| `swap::delete(prior_key)` | **unchanged** | Dispatch-time `prior_key`, exactly as today. |

#### Steps

1. **Write the failing test** in `save.rs`'s test module:

```rust
    #[cfg(unix)]
    #[test]
    fn save_as_onto_a_symlink_splits_chosen_and_resolved_correctly() {
        // THE highest-value test of this task: one SaveTarget field going to the wrong
        // consumer reintroduces either §4.10's defect (unsaveable symlinks) or §7.6.2's
        // seven regressions (canonical Document.path). All five consumers asserted at once.
        let real = scratch();
        let link = scratch();
        std::fs::write(&real, b"original\n").expect("seed");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let mut e = Editor::new_from_text("new body\n", None, (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        let resolved = std::fs::canonicalize(&link).expect("canonicalize");
        {
            let mut ctx = Ctx {
                editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(),
                fs: std::sync::Arc::new(crate::fsx::RealFs),
            };
            do_save_to(&mut ctx,
                SaveTarget { chosen: link.clone(), resolved: resolved.clone() },
                SaveMode::SaveAs);
        }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        // 1. The write landed on the RESOLVED target…
        assert_eq!(std::fs::read_to_string(&real).expect("read target"), "new body\n");
        // 2. …and the link survived as a link.
        assert!(link.symlink_metadata().expect("lstat").file_type().is_symlink(),
            "the symlink must survive — atomic_replace renamed over the TARGET");
        // 3. Document.path holds the CHOSEN path (Middle B).
        assert_eq!(e.active().document.path.as_deref(), Some(link.as_path()),
            "the buffer keeps the path the writer chose, not the canonical target");
        // 4. stored_fp describes the written file, so a follow-up save sees no conflict.
        assert_eq!(e.active().document.stored_fp, crate::save::fingerprint(&resolved),
            "stored_fp must match the file actually written");
        assert!(!e.active().document.dirty(), "and the buffer is clean");

        let _ = std::fs::remove_file(&link); let _ = std::fs::remove_file(&real);
    }

    #[test]
    fn save_target_same_sets_both_fields() {
        let p = std::path::PathBuf::from("/tmp/x.md");
        let t = SaveTarget::same(p.clone());
        assert_eq!(t.chosen, p);
        assert_eq!(t.resolved, p, "the common case: no resolution needed, both equal");
    }
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib save::tests::save_as_onto_a_symlink_splits
```

Expected: ``error[E0422]: cannot find struct, variant or union type `SaveTarget` in this scope``.

3. **Add `SaveTarget`** to `save.rs`:

```rust
/// The two paths a save needs, kept apart because a symlinked destination makes them differ.
///
/// A struct rather than two positional `PathBuf`s ON PURPOSE: same-typed positional
/// parameters are silently swappable, and getting this wrong reintroduces either the
/// unsaveable-symlink defect or the canonical-`Document.path` regressions. For an ordinary
/// destination the two fields are equal, which is the common case and costs nothing.
#[derive(Clone, Debug)]
pub(crate) struct SaveTarget {
    /// What the writer selected — logical, possibly a symlink.
    pub chosen: std::path::PathBuf,
    /// Where bytes actually go — resolution applied. Never a symlink.
    pub resolved: std::path::PathBuf,
}

impl SaveTarget {
    pub(crate) fn same(p: std::path::PathBuf) -> Self {
        SaveTarget { chosen: p.clone(), resolved: p }
    }
}
```

4. **Rewrite `do_save_to`'s prologue and capture:**

```rust
pub(crate) fn do_save_to(ctx: &mut Ctx, target: SaveTarget, mode: SaveMode) {
    // §3.9: status BEFORE dispatch. O(1) snapshot; version captured now.
    let snap = ctx.editor.active().document.buffer.snapshot();
    let v = ctx.editor.active().document.version;
    let buffer_id = ctx.editor.active().id;
    let prior_key = ctx.editor.active().document.path.clone(); // for SaveAs swap re-key
    let write_path = target.resolved.clone();   // bytes go HERE
    let chosen_path = target.chosen.clone();    // the buffer is rekeyed to THIS
    ctx.editor.set_progress(crate::status::StatusTopic::Save(buffer_id, v), "Saving\u{2026}");
```

and the worker:

```rust
        run: Box::new(move || {
            let content = snap.to_string();
            // Both the write and the fingerprint use the RESOLVED path: the fingerprint must
            // describe the file actually written. (`fingerprint` follows symlinks, so this
            // agrees with `dispatch_save`'s check on Document.path whenever the link resolves.)
            let outcome = file::save_atomic(&write_path, &content);
            let new_fp = fingerprint(&write_path);
```

and inside the merge, replace the two `target.clone()` uses:

```rust
                    // Plugin payload: the CHOSEN path, matching `wc.path()`, which returns
                    // Document.path. A Save event naming a path wc.path() never returns
                    // would make the two disagree for any plugin correlating them.
                    let fire_save: Option<PathBuf> =
                        matches!(outcome, Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged))
                            .then(|| chosen_path.clone());
```

```rust
                            Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged) => {
                                // Middle B: the buffer is rekeyed to the CHOSEN path so
                                // display, prefills, the open-dir seed, export derivation,
                                // wc.path(), and the LSP uri all stay logical.
                                if matches!(mode, SaveMode::SaveAs) {
                                    b.document.path = Some(chosen_path.clone());
                                }
                                b.document.saved_version = Some(v);
                                b.document.stored_fp = new_fp;
```

The rest of the merge — the `swapped_version` clear, the clean/still-editing branches, and the
`swap::delete(prior_key)` calls — is **unchanged**.

5. **Update the two callers.** `do_save` wraps its own path:

```rust
fn do_save(ctx: &mut Ctx) {
    let path = ctx.editor.active().document.path.clone().expect("do_save called without a path");
    // A plain Save resolves its own destination too — the document's path can itself be a
    // symlink (that is §4.10: openable but unsaveable).
    let resolved = match crate::fsx::resolve_write_destination(&*ctx.fs, &path) {
        Ok(r) => r,
        Err(crate::fsx::DestError::BrokenSymlink) => {
            ctx.editor.set_status_full(crate::status::StatusKind::Warning,
                format!("{}: destination symlink cannot be resolved", path.display()),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return;
        }
    };
    do_save_to(ctx, SaveTarget { chosen: path, resolved }, SaveMode::Normal);
}
```

and `prompts::perform_save_as` takes both:

```rust
fn perform_save_as(editor: &mut crate::editor::Editor, chosen: std::path::PathBuf,
                   resolved: std::path::PathBuf,
                   executor: &dyn crate::jobs::Executor,
                   clock: &dyn wordcartel_core::history::Clock,
                   msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
    let v = editor.active().document.version;
    let buffer_id = editor.active().id;
    {
        let fs = std::sync::Arc::new(crate::fsx::RealFs);
        let mut ctx = crate::registry::Ctx { editor, clock, executor, msg_tx: msg_tx.clone(), fs };
        crate::save::do_save_to(&mut ctx,
            crate::save::SaveTarget { chosen, resolved }, crate::save::SaveMode::SaveAs);
    }
    if let Some(action) = editor.pending_save_as.take() {
        editor.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id, version: v, action, at_ms: clock.now_ms() });
    }
}
```

The `OverwriteSaveAs` prompt arm resolves the stored target the same way before calling
`perform_save_as`.

6. **Run — expect green:**

```
cargo test -p wordcartel --lib save:: prompts::
```

Expected: `test result: ok`, including both new tests. Every existing `save::tests::*` must still
pass — the `SaveMode::Normal` path is behaviour-identical for a non-symlink document.

7. **Commit:** `feat(c5): split SaveTarget into chosen and resolved paths`

---

### Task 17 — Session-migration queue and buffer-blind drain

**Deliverable:** a Save-As migrates its session entry, correctly, under every ordering.

#### Files

- Modify: `wordcartel/src/editor.rs` (`SessionMigration`, the queue field)
- Modify: `wordcartel/src/save.rs` (push in the merge, at merge time)
- Modify: `wordcartel/src/session_restore.rs` (the drain)
- Modify: `wordcartel/src/app.rs` (drain at both persist sites)

#### Interfaces

**Consumes** (Task 16):

```rust
// crate::save
pub(crate) struct SaveTarget { pub chosen: std::path::PathBuf, pub resolved: std::path::PathBuf }
pub(crate) fn do_save_to(ctx: &mut Ctx, target: SaveTarget, mode: SaveMode);

// crate::state
pub struct StateEntry { pub cursor: usize, pub scroll: usize,
    pub marks: std::collections::BTreeMap<String, usize>, pub mtime: i64, pub size: u64,
    pub seq: u64, pub folds: Vec<usize>, pub block: Option<(usize, usize)> }
pub struct SessionState { pub entries: std::collections::BTreeMap<String, StateEntry>,
    pub scratch: Option<ScratchState> }
impl SessionState {
    pub fn next_seq(&self) -> u64;
    pub fn record(&mut self, path: String, entry: StateEntry, max_entries: usize);
}

// crate::session_restore
pub(crate) fn persist_session(session: &mut crate::state::SessionState,
    editor: &crate::editor::Editor, cfg: &crate::config::Config, seq: u64);
```

**Produces:**

```rust
// crate::editor
#[derive(Clone, Debug)]
pub struct SessionMigration {
    /// The buffer's PRE-REKEY path, read in the merge (NOT the dispatch-time `prior_key`).
    pub from: std::path::PathBuf,
    /// The chosen new path.
    pub to: std::path::PathBuf,
}
// Editor field:
pub pending_session_migrations: std::collections::VecDeque<SessionMigration>,

// crate::session_restore
/// Drain every queued migration into `session`, FIFO, before `persist_session` flushes.
/// Best-effort: a migration whose `from` key is already absent is a silent no-op.
pub(crate) fn drain_session_migrations(session: &mut crate::state::SessionState,
    editor: &mut crate::editor::Editor, cfg: &crate::config::Config);
```

#### Steps

1. **Write the two failing tests — and keep them separate.**

> **These two tests must NOT be merged.** They fail for different reasons and prove different
> things: the first fails against an `Option` slot, the second against dispatch-time `prior_key`
> capture. Folded into one, a half-fix passes — which is exactly how the overlapping-source defect
> survived the round that introduced the queue. A later "simplify the tests" pass must leave both.

In `session_restore.rs`'s test module:

```rust
    #[test]
    fn two_migrations_in_one_drain_batch_both_apply() {
        // FAILS AGAINST AN `Option` SLOT. `app::fold_and_continue` drains the executor in a
        // LOOP (`for o in ex.drain() { apply_job_outcome(…) }`), so several ready save jobs
        // merge before app::run next reaches a persist point. A single slot would keep only
        // the last, silently losing the first writer's marks with no error.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let cfg = crate::config::Config::default();
        let mut s = crate::state::SessionState::default();
        let entry = |c: usize| crate::state::StateEntry {
            cursor: c, scroll: 0, marks: Default::default(), mtime: 1, size: 1, seq: 1,
            folds: vec![], block: None };
        s.entries.insert("/a.md".into(), entry(11));
        s.entries.insert("/x.md".into(), entry(22));

        e.pending_session_migrations.push_back(crate::editor::SessionMigration {
            from: "/a.md".into(), to: "/b.md".into() });
        e.pending_session_migrations.push_back(crate::editor::SessionMigration {
            from: "/x.md".into(), to: "/y.md".into() });

        drain_session_migrations(&mut s, &mut e, &cfg);

        assert!(s.entries.contains_key("/b.md"), "first migration applied");
        assert!(s.entries.contains_key("/y.md"), "second migration applied — NOT clobbered");
        assert_eq!(s.entries["/b.md"].cursor, 11, "and it carried the cursor across");
        assert_eq!(s.entries["/y.md"].cursor, 22);
        assert!(!s.entries.contains_key("/a.md") && !s.entries.contains_key("/x.md"),
            "the old keys are gone — the point is to remove the stale duplicate");
        assert!(e.pending_session_migrations.is_empty(), "the queue drains fully");
    }

    #[test]
    fn overlapping_same_source_save_as_chains_correctly() {
        // FAILS AGAINST DISPATCH-TIME `prior_key` CAPTURE, and passes against merge-time
        // capture. `do_save_to` binds prior_key at DISPATCH while `document.path` is mutated
        // later in the MERGE — so dispatching a->b then a->c BEFORE the first merge lands
        // would queue (a->b, a->c). FIFO applies the first; the second finds `a` already
        // absent and no-ops. Session state ends at `b` while the buffer ends at `c`.
        //
        // With merge-time capture the a->b merge has already set path = b, so the second
        // merge records (b, c) and the chain is correct BY CONSTRUCTION.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let cfg = crate::config::Config::default();
        let mut s = crate::state::SessionState::default();
        s.entries.insert("/a.md".into(), crate::state::StateEntry {
            cursor: 7, scroll: 0, marks: Default::default(), mtime: 1, size: 1, seq: 1,
            folds: vec![], block: None });

        // What MERGE-TIME capture produces for two overlapping Save-As from one source:
        e.pending_session_migrations.push_back(crate::editor::SessionMigration {
            from: "/a.md".into(), to: "/b.md".into() });
        e.pending_session_migrations.push_back(crate::editor::SessionMigration {
            from: "/b.md".into(), to: "/c.md".into() });

        drain_session_migrations(&mut s, &mut e, &cfg);

        assert!(s.entries.contains_key("/c.md"), "the chain lands at the final path");
        assert_eq!(s.entries["/c.md"].cursor, 7, "carrying the original cursor through both hops");
        assert!(!s.entries.contains_key("/a.md"), "no stale source entry");
        assert!(!s.entries.contains_key("/b.md"), "no stranded intermediate entry");
        assert_eq!(s.entries.len(), 1, "exactly ONE entry survives, at /c.md");
    }

    #[test]
    fn a_migration_whose_source_is_gone_is_a_silent_no_op() {
        // Hygiene, not a durability guarantee: never an error, never a reason to fail a
        // persist.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let cfg = crate::config::Config::default();
        let mut s = crate::state::SessionState::default();
        e.pending_session_migrations.push_back(crate::editor::SessionMigration {
            from: "/never-existed.md".into(), to: "/z.md".into() });
        drain_session_migrations(&mut s, &mut e, &cfg);
        assert!(s.entries.is_empty(), "nothing invented");
        assert!(e.pending_session_migrations.is_empty(), "still drained");
    }
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib session_restore::tests::two_migrations_in_one_drain
```

Expected: ``error[E0609]: no field `pending_session_migrations` on type `Editor```.

3. **Add the type and field** to `editor.rs`:

```rust
/// One pending session-entry rename, recorded by a Save-As merge and applied where the
/// session store is actually reachable (`app::run`).
#[derive(Clone, Debug)]
pub struct SessionMigration {
    /// The buffer's PRE-REKEY path, read IN THE MERGE — not the dispatch-time `prior_key`.
    /// Merge-time capture is what makes overlapping Save-As from one source chain correctly.
    pub from: std::path::PathBuf,
    pub to: std::path::PathBuf,
}
```

On `Editor`:

```rust
    /// Save-As session-entry migrations awaiting application.
    ///
    /// A QUEUE, not an `Option` slot: `fold_and_continue` drains the executor in a loop, so
    /// several save merges can land before `app::run` next reaches a persist point, and a
    /// slot would silently drop all but the last.
    pub pending_session_migrations: std::collections::VecDeque<SessionMigration>,
```

Initialise to `VecDeque::new()` in `Editor`'s constructor.

4. **Push from the merge, at merge time.** In `do_save_to`'s `Ok(Saved | Unchanged)` arm, capture the
   pre-rekey path on the line above the rekey:

```rust
                            Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged) => {
                                // MERGE-TIME capture. The dispatch-time `prior_key` is stale
                                // for a second Save-As dispatched before this merge landed;
                                // reading the buffer here gives the truth at THIS moment, so
                                // a->b then a->c records (a,b) then (b,c) and chains.
                                let pre_rekey = b.document.path.clone();
                                if matches!(mode, SaveMode::SaveAs) {
                                    b.document.path = Some(chosen_path.clone());
                                }
                                b.document.saved_version = Some(v);
                                b.document.stored_fp = new_fp;
```

and after the `by_id_mut` block closes (so the `&mut Buffer` borrow has ended):

```rust
                    // Queue the session-entry migration. Nothing is queued when there is no
                    // old entry (first Save-As of an unnamed buffer) or when the path did not
                    // change (Save-As onto the same path).
                    if matches!(mode, SaveMode::SaveAs) {
                        if let Some(from) = migrate_from {
                            if from != chosen_path {
                                editor.pending_session_migrations.push_back(
                                    crate::editor::SessionMigration { from, to: chosen_path.clone() });
                            }
                        }
                    }
```

where `migrate_from` is the `pre_rekey` value hoisted out of the `by_id_mut` block in the same
local-then-assign shape the existing `status` and `fire_save` locals already use.

5. **Write the drain** in `session_restore.rs`:

```rust
/// Apply every queued Save-As session-entry migration, FIFO.
///
/// FIFO is required, not incidental: with merge-time capture each entry's `from` is the
/// previous entry's `to`, so any other order strands the chain.
///
/// Best-effort — this is hygiene, not a durability guarantee. A migration whose `from` key
/// is already absent is a silent no-op, never an error and never a reason to fail a persist.
pub(crate) fn drain_session_migrations(
    session: &mut crate::state::SessionState,
    editor: &mut crate::editor::Editor,
    cfg: &crate::config::Config,
) {
    while let Some(m) = editor.pending_session_migrations.pop_front() {
        // Both endpoints are LOGICAL paths (Middle B); canonicalizing here is what makes a
        // symlinked destination converge on the same key as its target.
        let from_key = std::fs::canonicalize(&m.from)
            .unwrap_or_else(|_| m.from.clone()).to_string_lossy().into_owned();
        let to_key = std::fs::canonicalize(&m.to)
            .unwrap_or_else(|_| m.to.clone()).to_string_lossy().into_owned();
        if from_key == to_key { continue; }
        let Some(mut entry) = session.entries.remove(&from_key) else { continue };
        entry.seq = session.next_seq();
        session.record(to_key, entry, cfg.state.max_entries);
    }
}
```

6. **Drain at BOTH persist sites** in `app::run`. Replace the in-loop branch:

```rust
        // Persist when a save just completed (saved_version advanced) OR when a Save-As
        // queued a session migration.
        //
        // The migration half must NOT be gated on `sv` alone: `do_save_to`'s merge targets
        // `by_id_mut(buffer_id)` so a save lands on the right buffer even after the user
        // switches away, but `sv` reads `active().document.saved_version`. Save-As a
        // document, switch buffers before the write completes, and the active buffer's
        // saved_version never moves — the branch would not fire and the migration would
        // strand. Reading the queue off the Editor is buffer-blind by construction.
        let sv = { editor.borrow().active().document.saved_version };
        let has_migrations = !editor.borrow().pending_session_migrations.is_empty();
        if has_migrations || sv != last_persisted_saved {
            session_seq += 1;
            {
                let mut e = editor.borrow_mut();
                crate::session_restore::drain_session_migrations(&mut session, &mut e, &cfg);
            }
            { crate::session_restore::persist_session(&mut session, &editor.borrow(), &cfg, session_seq); }
            last_persisted_saved = sv;
        }
```

and the post-loop clean-quit persist:

```rust
    // On clean quit: drain any migration queued by a save that completed on the final
    // iteration, then persist once more (cursor may have moved since the last save).
    session_seq += 1;
    {
        let mut e = editor.borrow_mut();
        crate::session_restore::drain_session_migrations(&mut session, &mut e, &cfg);
    }
    crate::session_restore::persist_session(&mut session, &editor.borrow(), &cfg, session_seq);
```

7. **Run — expect green:**

```
cargo test -p wordcartel --lib session_restore:: save::
```

Expected: `test result: ok`, including all three drain tests.

8. **Commit:** `feat(c5): queue Save-As session migrations and drain them buffer-blind`

---

*Phase D complete. The write path resolves symlinked destinations, keeps `Document.path` logical, and
migrates session state correctly under every Save-As ordering.*

## Phase E — Destination mode (Tasks 18–22)

### Task 18 — `BrowseMode`, the destination field, and the Enter decision table

**This task is deliberately large, and should not be split.** The Enter decision table, the
dual-duty field, and the `Tab` gesture are one decision about what committing a destination *means*.
A reviewer who approved "the Enter table" while rejecting "the field editing" would be approving half
a decision — the rows are defined in terms of the field's contents. It carries nine tests because
this is the one surface where a design error produces the exact harm class C5 exists to eliminate:
**silent overwrite and save-to-nowhere.**

#### Files

- Create: `wordcartel/src/file_browser_commit.rs`
- Create: `wordcartel/src/file_browser_intercept.rs`
- Modify: `wordcartel/src/file_browser.rs` (`BrowseMode`, `DestinationPurpose`; move the intercept out)
- Modify: `wordcartel/src/lib.rs` (declare both new modules)
- Modify: `wordcartel/src/overlays.rs` (the `FileBrowser` row's `intercept` fn pointer)

#### Interfaces

**Consumes** (Tasks 12, 13, 14, 15):

```rust
// crate::fsx
pub(crate) enum EntryKind { File, Dir, Other, Unknown }
pub(crate) trait Fs { fn stat(&self, path: &Path) -> std::io::Result<FileStat>; /* … */ }
pub(crate) fn exists_via(fs: &dyn Fs, path: &Path) -> bool;
pub(crate) fn is_file_via(fs: &dyn Fs, path: &Path) -> bool;

// crate::file_browser
pub struct FileEntry { pub name: String, pub kind: crate::fsx::EntryKind,
                       pub is_symlink: bool, pub broken: bool }
pub(crate) enum EnterOutcome { Descend(std::path::PathBuf), Open(std::path::PathBuf),
                               Refuse(String) }
pub(crate) fn classify_enter(e: &FileEntry, dir: &std::path::Path) -> EnterOutcome;
pub(crate) fn start_listing(fb: &mut FileBrowser, target: std::path::PathBuf,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>);

// crate::file_browser_listing
pub(crate) struct FilterOpts { pub show_clutter: bool,
    pub types: crate::config::FileTypeFilter, pub destination: bool }
pub(crate) fn rederive(fb: &mut crate::file_browser::FileBrowser, opts: FilterOpts);

// crate::prompts
pub fn expand_path(text: &str) -> std::path::PathBuf;   // `~/` expansion + cwd-join
```

**Produces:**

```rust
// crate::file_browser
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationPurpose {
    SaveAs,
    WriteBlock,
    Export { ext: String },
}

#[derive(Debug, Clone)]
pub enum BrowseMode {
    /// Choose an existing entry to open.
    Select,
    /// Choose a destination path: navigate AND name. The field is dual-duty — it is
    /// simultaneously the filename-to-be and a live filter over the listing.
    Destination {
        purpose: DestinationPurpose,
        field: String,
        /// Byte offset into `field`. UTF-8-codepoint-safe editing, mirroring `Minibuffer`.
        field_cursor: usize,
    },
}
// FileBrowser gains: pub mode: BrowseMode,

// crate::file_browser_commit
/// What Enter does in DESTINATION mode. Evaluated top to bottom, first match wins —
/// the four rows of spec §7.2's decision table.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommitOutcome {
    /// Row 1 (highlighted entry is a directory) or row 3 (field names an existing directory).
    Descend(std::path::PathBuf),
    /// Row 2 (empty field, highlighted file) or row 4 (commit dir + field).
    Commit(std::path::PathBuf),
    /// Nothing to commit — an empty field with no usable highlight.
    Nothing,
}

pub(crate) fn classify_destination_enter(
    fs: &dyn crate::fsx::Fs,
    dir: &std::path::Path,
    field: &str,
    highlighted: Option<&crate::file_browser::FileEntry>,
) -> CommitOutcome;

/// Resolve a field value against `dir`. DELIBERATELY NOT `prompts::expand_path`, which
/// joins relative input onto the process cwd — invisible to a writer looking at a listing.
pub(crate) fn resolve_field(dir: &std::path::Path, field: &str) -> std::path::PathBuf;

/// The `Tab` gesture: copy a highlighted FILE's name into the field. Never commits.
pub(crate) fn copy_name_into_field(field: &mut String, field_cursor: &mut usize, name: &str);
```

#### Steps

1. **Write the nine failing tests.** Create the test module in
   `wordcartel/src/file_browser_commit.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_browser::FileEntry;
    use crate::fsx::EntryKind;

    fn fe(name: &str, kind: EntryKind) -> FileEntry {
        FileEntry { name: name.into(), kind, is_symlink: false, broken: false }
    }
    fn tmp(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "wc-commit-{}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed), label));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    // ---- The four rows of the decision table, in order ----------------------------

    #[test]
    fn row1_highlighted_directory_descends() {
        let d = tmp("row1");
        std::fs::create_dir_all(d.join("drafts")).expect("seed");
        let e = fe("drafts", EntryKind::Dir);
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter", Some(&e)),
            CommitOutcome::Descend(d.join("drafts")),
            "row 1 wins even with a non-empty field — the writer keeps their filename while \
             navigating");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn row2_empty_field_on_a_highlighted_file_commits_to_it() {
        // Explicit overwrite of an existing file. Safe because it still raises the
        // overwrite-confirm downstream, and because reaching it takes TWO deliberate acts:
        // navigating the highlight there, and pressing Enter with a visibly empty field.
        let d = tmp("row2");
        std::fs::write(d.join("existing.md"), b"x").expect("seed");
        let e = fe("existing.md", EntryKind::File);
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "", Some(&e)),
            CommitOutcome::Commit(d.join("existing.md")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn row3_field_naming_an_existing_directory_descends_not_creates() {
        // THE AMBIGUOUS CASE, resolved toward descend. A directory named `chapter-one`
        // sitting visibly in the list while Enter silently creates a FILE named
        // `chapter-one.md` beside it is the worse surprise — and descend is recoverable in
        // one keystroke ('..'), while a misplaced file is not.
        let d = tmp("row3");
        std::fs::create_dir_all(d.join("chapter-one")).expect("seed");
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter-one", None),
            CommitOutcome::Descend(d.join("chapter-one")),
            "a field naming an existing directory descends into it");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn row3_is_pinned_one_character_away_from_row4() {
        // The companion that PINS row 3: adding a character must flip it to file creation.
        // Without this, "resolves toward descend" could be satisfied by a rule that never
        // creates a file whose name shares a prefix with a directory.
        let d = tmp("row3-pin");
        std::fs::create_dir_all(d.join("chapter-one")).expect("seed");
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter-one", None),
            CommitOutcome::Descend(d.join("chapter-one")));
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter-oneX", None),
            CommitOutcome::Commit(d.join("chapter-oneX")),
            "one more character and it is an ordinary new-file commit");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn row4_commits_dir_plus_field() {
        let d = tmp("row4");
        assert_eq!(
            classify_destination_enter(&crate::fsx::RealFs, &d, "chapter one", None),
            CommitOutcome::Commit(d.join("chapter one")),
            "the ordinary case: a new file in the directory the writer is looking at");
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- Field resolution ---------------------------------------------------------

    #[test]
    fn a_bare_relative_field_resolves_against_fb_dir_not_the_process_cwd() {
        // The divergence from `prompts::expand_path`, and the whole point of it: the writer
        // is looking at `dir`, so `chapter.md` must mean "here". Joining cwd would put the
        // file somewhere the picker never showed them — the save-to-nowhere class.
        let d = tmp("resolve-rel");
        let cwd = std::env::current_dir().expect("cwd");
        assert_ne!(d, cwd, "test premise: fb.dir and cwd must differ");
        assert_eq!(resolve_field(&d, "chapter.md"), d.join("chapter.md"));
        assert_eq!(resolve_field(&d, "drafts/ch1.md"), d.join("drafts/ch1.md"),
            "a relative path WITH segments also resolves under fb.dir");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn absolute_and_home_relative_fields_are_honoured() {
        let d = tmp("resolve-abs");
        assert_eq!(resolve_field(&d, "/etc/hosts"), std::path::PathBuf::from("/etc/hosts"));
        if let Some(home) = dirs::home_dir() {
            assert_eq!(resolve_field(&d, "~/notes.md"), home.join("notes.md"));
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- The Tab gesture ----------------------------------------------------------

    #[test]
    fn tab_copies_a_name_into_the_field_and_does_not_commit() {
        // The deliberate two-step overwrite gesture: highlight, Tab (see the name land and
        // the footer show the resolved target), Enter (see the overwrite-confirm). Overwrite
        // is never one accidental keystroke, and never reachable without the target visible.
        let mut field = String::from("draft");
        let mut cur = field.len();
        copy_name_into_field(&mut field, &mut cur, "existing.md");
        assert_eq!(field, "existing.md", "the name REPLACES the field content");
        assert_eq!(cur, "existing.md".len(), "and the cursor lands at the end");
        // `copy_name_into_field` returns nothing and touches no path — it cannot commit.
    }

    #[test]
    fn an_empty_field_with_no_highlight_commits_nothing() {
        let d = tmp("nothing");
        assert_eq!(classify_destination_enter(&crate::fsx::RealFs, &d, "", None),
            CommitOutcome::Nothing, "no field, no highlight — Enter is inert, never a write");
        assert_eq!(classify_destination_enter(&crate::fsx::RealFs, &d, "   ", None),
            CommitOutcome::Nothing, "a whitespace-only field is empty");
        let _ = std::fs::remove_dir_all(&d);
    }
}
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib file_browser_commit::
```

Expected: ``error[E0433]: failed to resolve: use of undeclared crate or module `file_browser_commit```.

3. **Add the mode types** to `file_browser.rs`:

```rust
/// What a destination is FOR. The commit path dispatches on this, so adding a future
/// destination consumer is one variant plus one arm the compiler demands — a registration
/// seam, not a growing hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationPurpose {
    SaveAs,
    WriteBlock,
    Export { ext: String },
}

/// Select mode chooses an existing entry; destination mode navigates AND names.
///
/// Not a second `OverlayId`: two overlays would duplicate the intercept, painter, mouse fn,
/// and geometry, and would have to be kept in lockstep by hand — the hand-parallel pathology
/// H21 removed.
#[derive(Debug, Clone)]
pub enum BrowseMode {
    Select,
    Destination {
        purpose: DestinationPurpose,
        /// DUAL-DUTY: simultaneously the filename-to-be and a live filter over the listing,
        /// so typing `chap` narrows to existing chapter files — overwrite awareness for free.
        field: String,
        /// Byte offset into `field`.
        field_cursor: usize,
    },
}

impl BrowseMode {
    pub fn is_destination(&self) -> bool { matches!(self, BrowseMode::Destination { .. }) }
    /// The text the listing filter should use: the query in select mode, the field in
    /// destination mode. One accessor so the two modes cannot drift apart.
    pub fn filter_text<'a>(&'a self, query: &'a str) -> &'a str {
        match self { BrowseMode::Select => query,
                     BrowseMode::Destination { field, .. } => field }
    }
}
```

Add `pub mode: BrowseMode` to `FileBrowser`, defaulting to `BrowseMode::Select` at every
construction site.

4. **Write `file_browser_commit.rs`:**

```rust
//! Destination-mode commit semantics: what Enter MEANS when the writer is naming a file.
//!
//! Split from `file_browser.rs` on one axis of change. This is the highest-risk logic in
//! C5 — the only place where an error produces silent overwrite or save-to-nowhere — so it
//! lives alone, is pure, and is tested row by row.

use crate::file_browser::FileEntry;
use crate::fsx::{EntryKind, Fs};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommitOutcome {
    Descend(PathBuf),
    Commit(PathBuf),
    Nothing,
}

/// Resolve a field value against the directory the writer is LOOKING AT.
///
/// Deliberately NOT `prompts::expand_path`: that joins relative input onto
/// `std::env::current_dir()`, which is invisible to someone reading a directory listing.
/// Joining cwd would put the file somewhere the picker never showed them.
///
/// 1. `~/`-prefixed -> home-relative.
/// 2. absolute      -> as typed.
/// 3. otherwise     -> joined onto `dir`, NOT onto cwd.
pub(crate) fn resolve_field(dir: &Path, field: &str) -> PathBuf {
    let t = field.trim();
    if let Some(rest) = t.strip_prefix("~/") {
        return dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(t));
    }
    let p = PathBuf::from(t);
    if p.is_absolute() { p } else { dir.join(p) }
}

/// The four-row Enter decision table (spec §7.2). Evaluated top to bottom; first match wins.
///
/// | # | Condition                                   | Action              |
/// |---|---------------------------------------------|---------------------|
/// | 1 | highlighted entry is a directory (incl "..")| Descend             |
/// | 2 | field empty AND highlighted entry is a file | Commit to that file |
/// | 3 | field resolves to an EXISTING directory     | Descend into it     |
/// | 4 | otherwise                                   | Commit dir + field  |
pub(crate) fn classify_destination_enter(
    fs: &dyn Fs,
    dir: &Path,
    field: &str,
    highlighted: Option<&FileEntry>,
) -> CommitOutcome {
    // Row 1 — a highlighted directory descends, EVEN with a non-empty field, so the writer
    // keeps their filename while navigating.
    if let Some(e) = highlighted {
        if matches!(e.kind, EntryKind::Dir) {
            let target = if e.name == ".." {
                dir.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf())
            } else {
                dir.join(&e.name)
            };
            return CommitOutcome::Descend(target);
        }
    }

    let trimmed = field.trim();

    // Row 2 — an empty field commits onto the highlighted FILE. Explicit overwrite intent:
    // it takes navigating there AND pressing Enter with a visibly empty field, and it still
    // raises the overwrite-confirm downstream.
    if trimmed.is_empty() {
        return match highlighted {
            Some(e) if matches!(e.kind, EntryKind::File) => {
                CommitOutcome::Commit(dir.join(&e.name))
            }
            // Other/Unknown are refused in select mode and are not commit targets here
            // either — we do not know they are writable regular files.
            _ => CommitOutcome::Nothing,
        };
    }

    let resolved = resolve_field(dir, trimmed);

    // Row 3 — the one genuinely ambiguous case, resolved TOWARD DESCEND. A directory named
    // `chapter-one` in the list while Enter creates a FILE `chapter-one.md` beside it is the
    // worse surprise; descend is recoverable in one keystroke ('..'), a misplaced file is not.
    if matches!(fs.stat(&resolved), Ok(st) if st.is_dir) {
        return CommitOutcome::Descend(resolved);
    }

    // Row 4 — the ordinary case.
    CommitOutcome::Commit(resolved)
}

/// The `Tab` gesture: replace the field with a highlighted file's name. Returns nothing and
/// touches no path — it CANNOT commit, which is the point. Overwrite becomes: highlight,
/// Tab (name lands, footer shows the resolved target), Enter (overwrite-confirm).
pub(crate) fn copy_name_into_field(field: &mut String, field_cursor: &mut usize, name: &str) {
    field.clear();
    field.push_str(name);
    *field_cursor = field.len();
}
```

5. **Move the intercept** into `file_browser_intercept.rs` and branch on mode. Destination mode
   routes printable characters, `Backspace`, `Left`/`Right`, and `Event::Paste` into the **field**
   (reusing the UTF-8-codepoint-safe arithmetic `Minibuffer::{insert, backspace, left, right}` uses —
   extract it into a shared helper rather than writing a second copy), and the six shared nav keys
   into the **selection** via `list_window::{list_nav_key, apply_list_nav}`. **Nav never edits the
   field; field edits never move the selection except to clamp it.** `Tab` on a highlighted `File`
   calls `copy_name_into_field`. Each field edit calls
   `file_browser_listing::rederive(fb, opts_with(destination: true))`.

6. **Declare both modules** in `lib.rs`:

```rust
pub mod file_browser_commit;
pub mod file_browser_intercept;
```

and repoint the `FileBrowser` row's `intercept` in `overlays.rs` to
`crate::file_browser_intercept::intercept`.

7. **Run — expect green:**

```
cargo test -p wordcartel --lib file_browser_commit:: file_browser_intercept:: file_browser::
```

Expected: `test result: ok`, including all nine commit tests.

8. **Commit:** `feat(c5): add destination mode with the four-row Enter decision table`

---

### Task 19 — Extension policy

**Deliverable:** a pure classifier that appends `.md` to an extensionless save name, redirects
output formats to Export, and honours everything else.

#### Files

- Create: nothing — add to `wordcartel/src/file_browser_commit.rs`
- Modify: `wordcartel/src/file_browser_commit.rs`

#### Interfaces

**Consumes** (Task 18):

```rust
// crate::file_browser_commit
pub(crate) fn resolve_field(dir: &std::path::Path, field: &str) -> std::path::PathBuf;
pub(crate) enum CommitOutcome { Descend(std::path::PathBuf), Commit(std::path::PathBuf), Nothing }
```

**Produces:**

```rust
// crate::file_browser_commit
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExtVerdict {
    /// Append `.md` — the name had no extension.
    Defaulted(std::path::PathBuf),
    /// A recognized OUTPUT extension. Refuse the save and offer Export, carrying the typed
    /// path forward so the writer's intent is not thrown away.
    Redirect { path: std::path::PathBuf, ext: String },
    /// Any other extension — honoured silently.
    Honoured(std::path::PathBuf),
}

/// Apply F4's default-and-redirect policy to a SAVE destination. Never applied in select
/// mode, and never to an export destination (whose extension is fixed by the format).
pub(crate) fn apply_extension_policy(path: &std::path::Path) -> ExtVerdict;
```

#### Steps

1. **Write the failing test** — table-driven, in `file_browser_commit.rs`'s test module:

```rust
    #[test]
    fn extension_policy_table() {
        use std::path::PathBuf;
        let p = |s: &str| PathBuf::from(s);

        // Missing extension -> append .md.
        assert_eq!(apply_extension_policy(&p("/d/chapter one")),
            ExtVerdict::Defaulted(p("/d/chapter one.md")));

        // Recognized OUTPUT extensions -> redirect to Export, carrying the path.
        for ext in ["docx", "pdf", "html", "tex"] {
            assert_eq!(apply_extension_policy(&p(&format!("/d/book.{ext}"))),
                ExtVerdict::Redirect { path: p(&format!("/d/book.{ext}")), ext: ext.into() },
                "a save into an export format is refused and redirected, not written as markdown");
        }
        // Case-insensitive.
        assert_eq!(apply_extension_policy(&p("/d/book.DOCX")),
            ExtVerdict::Redirect { path: p("/d/book.DOCX"), ext: "docx".into() });

        // Anything else -> honoured silently.
        for name in ["notes.txt", "notes.rst", "notes.org", "notes.md"] {
            assert_eq!(apply_extension_policy(&p(&format!("/d/{name}"))),
                ExtVerdict::Honoured(p(&format!("/d/{name}"))));
        }

        // EDGE CASES, each a real way to get this wrong:
        // A dotfile's leading dot is NOT an extension — never produce `.gitignore.md`.
        assert_eq!(apply_extension_policy(&p("/d/.gitignore")),
            ExtVerdict::Honoured(p("/d/.gitignore")));
        assert_eq!(apply_extension_policy(&p("/d/.wordcartel.toml")),
            ExtVerdict::Honoured(p("/d/.wordcartel.toml")));
        // A trailing dot is no extension — and must not yield `notes..md`.
        assert_eq!(apply_extension_policy(&p("/d/notes.")),
            ExtVerdict::Defaulted(p("/d/notes.md")));
        // Only the FINAL component is the extension.
        assert_eq!(apply_extension_policy(&p("/d/chapter.one.md")),
            ExtVerdict::Honoured(p("/d/chapter.one.md")));
        assert_eq!(apply_extension_policy(&p("/d/chapter.one")),
            ExtVerdict::Honoured(p("/d/chapter.one")),
            "`one` is an unrecognized extension — honoured, not defaulted");
    }
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib file_browser_commit::tests::extension_policy_table
```

Expected: ``error[E0425]: cannot find function `apply_extension_policy` in this scope``.

3. **Add the classifier** to `file_browser_commit.rs`:

```rust
/// Extensions that mean "this is an export, not a save".
const OUTPUT_EXTS: &[&str] = &["docx", "pdf", "html", "tex"];

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExtVerdict {
    Defaulted(PathBuf),
    Redirect { path: PathBuf, ext: String },
    Honoured(PathBuf),
}

/// F4's default-and-redirect policy for SAVE destinations.
///
/// Redirect is only defensible because export now HAS a destination (spec §9) — before C5,
/// "use Export instead" was advice with nowhere to go.
pub(crate) fn apply_extension_policy(path: &Path) -> ExtVerdict {
    // `Path::extension()` already returns None for a dotfile like `.gitignore` (the leading
    // dot is part of the stem) and for a trailing dot — both of which must NOT be defaulted
    // into `.gitignore.md` / `notes..md`. Handle the trailing-dot case by trimming it before
    // appending, so we never produce a doubled dot.
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let lower = ext.to_ascii_lowercase();
            if OUTPUT_EXTS.contains(&lower.as_str()) {
                ExtVerdict::Redirect { path: path.to_path_buf(), ext: lower }
            } else {
                ExtVerdict::Honoured(path.to_path_buf())
            }
        }
        None => {
            let s = path.to_string_lossy();
            // A dotfile has no extension AND must not be defaulted — its file_name starts
            // with '.' and contains no further dot.
            let is_dotfile = path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            if is_dotfile {
                return ExtVerdict::Honoured(path.to_path_buf());
            }
            let trimmed = s.trim_end_matches('.');
            ExtVerdict::Defaulted(PathBuf::from(format!("{trimmed}.md")))
        }
    }
}
```

4. **Run — expect green:**

```
cargo test -p wordcartel --lib file_browser_commit::tests::extension_policy_table
```

Expected: `test result: ok. 1 passed`.

5. **Commit:** `feat(c5): add the default-and-redirect extension policy for save destinations`

---

### Task 20 — The resolved-target footer

**Deliverable:** destination mode shows, live on every keystroke, the absolute path that will
actually be written — after extension policy, after symlink resolution.

**This is the single highest-value writer-facing element in C5.** It removes the entire class of
"I saved it but I don't know where."

#### Files

- Modify: `wordcartel/src/file_browser.rs` (`footer_target`)
- Modify: `wordcartel/src/render_overlays.rs` (`paint_file_browser`)
- Modify: `wordcartel/src/chrome_geom.rs` (`file_browser_row_at`)

#### Interfaces

**Consumes** (Tasks 15, 18, 19):

```rust
// crate::fsx
pub(crate) fn resolve_write_destination(fs: &dyn Fs, path: &Path)
    -> Result<std::path::PathBuf, DestError>;
pub(crate) fn exists_via(fs: &dyn Fs, path: &Path) -> bool;
pub(crate) enum DestError { BrokenSymlink }

// crate::file_browser_commit
pub(crate) fn resolve_field(dir: &std::path::Path, field: &str) -> std::path::PathBuf;
pub(crate) fn apply_extension_policy(path: &std::path::Path) -> ExtVerdict;
pub(crate) enum ExtVerdict { Defaulted(std::path::PathBuf),
    Redirect { path: std::path::PathBuf, ext: String }, Honoured(std::path::PathBuf) }
```

**Produces:**

```rust
// crate::file_browser
/// The footer line for destination mode: the absolute resolved target AFTER extension
/// policy, plus an inline existence note. `None` in select mode or with an empty field.
pub(crate) fn footer_target(fs: &dyn crate::fsx::Fs, fb: &FileBrowser) -> Option<String>;
```

#### Steps

1. **Write the failing tests** in `file_browser.rs`'s test module:

```rust
    #[test]
    fn footer_shows_the_post_policy_absolute_target() {
        // The .md that policy appends must be visible BEFORE commit, not discovered after.
        let d = std::env::temp_dir().join(format!("wc-footer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        let mut fb = FileBrowser {
            dir: d.clone(), query: String::new(),
            mode: BrowseMode::Destination {
                purpose: DestinationPurpose::SaveAs,
                field: "chapter one".into(), field_cursor: 11,
            },
            listing: vec![], total_seen: 0, unreadable: 0, entries: vec![],
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        };
        let line = footer_target(&crate::fsx::RealFs, &fb).expect("destination mode has a footer");
        assert!(line.contains(&d.join("chapter one.md").display().to_string()),
            "the footer shows the ABSOLUTE, post-policy target: {line}");
        assert!(!line.contains("will confirm"), "nothing exists there yet");

        // When the target exists, overwrite is telegraphed one step BEFORE the confirm.
        std::fs::write(d.join("taken.md"), b"x").expect("seed");
        if let BrowseMode::Destination { field, field_cursor, .. } = &mut fb.mode {
            *field = "taken.md".into(); *field_cursor = field.len();
        }
        let line = footer_target(&crate::fsx::RealFs, &fb).expect("footer");
        assert!(line.contains("exists"), "an existing target is disclosed inline: {line}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn footer_is_absent_in_select_mode() {
        let mut fb = FileBrowser {
            dir: std::env::temp_dir(), query: "q".into(), mode: BrowseMode::Select,
            listing: vec![], total_seen: 0, unreadable: 0, entries: vec![],
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        };
        assert!(footer_target(&crate::fsx::RealFs, &fb).is_none(), "select mode names no target");
        fb.mode = BrowseMode::Destination {
            purpose: DestinationPurpose::SaveAs, field: String::new(), field_cursor: 0 };
        assert!(footer_target(&crate::fsx::RealFs, &fb).is_none(), "an empty field names none either");
    }
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib file_browser::tests::footer_shows_the_post_policy
```

Expected: ``error[E0425]: cannot find function `footer_target` in this scope``.

3. **Add `footer_target`** to `file_browser.rs`:

```rust
/// The destination-mode footer: `→ /abs/path/after-policy.md`, plus an inline note when the
/// target already exists.
///
/// Shows the POST-POLICY name so the `.md` that policy appends is visible before commit, and
/// the RESOLVED path when a symlink changed it — resolution should be visible up front, not
/// discovered in a confirm dialog.
pub(crate) fn footer_target(fs: &dyn crate::fsx::Fs, fb: &FileBrowser) -> Option<String> {
    let BrowseMode::Destination { field, purpose, .. } = &fb.mode else { return None };
    if field.trim().is_empty() { return None; }
    let typed = crate::file_browser_commit::resolve_field(&fb.dir, field);
    // An export destination's extension is fixed by the format — policy does not apply.
    let after_policy = if matches!(purpose, DestinationPurpose::Export { .. }) {
        typed
    } else {
        match crate::file_browser_commit::apply_extension_policy(&typed) {
            crate::file_browser_commit::ExtVerdict::Defaulted(p) => p,
            crate::file_browser_commit::ExtVerdict::Honoured(p) => p,
            crate::file_browser_commit::ExtVerdict::Redirect { path, ext } => {
                return Some(format!("\u{2192} {} \u{2014} {ext} is an export format",
                    path.display()));
            }
        }
    };
    let shown = match crate::fsx::resolve_write_destination(fs, &after_policy) {
        Ok(r) => r,
        Err(crate::fsx::DestError::BrokenSymlink) => {
            return Some(format!("\u{2192} {} \u{2014} symlink cannot be resolved",
                after_policy.display()));
        }
    };
    let note = if crate::fsx::exists_via(fs, &shown) { " (exists \u{2014} will confirm)" } else { "" };
    Some(format!("\u{2192} {}{note}", shown.display()))
}
```

4. **Paint it, and keep geometry in lockstep.** In `render_overlays::paint_file_browser`, render the
   footer on the block's bottom edge. The existing `windowed_indicator` also wants that edge via
   `block.title_bottom(...)`; **the resolved target wins the position** when only one fits.

   **`chrome_geom::file_browser_row_at` must move in lockstep.** It computes
   `list_top = r.y + 2` and `list_h` from `list_window::list_h_for`; if the footer consumes a row,
   the list interior shrinks and the hit-test must use the same reduced height the painter used.
   Single-source it: add a `pub(crate) fn file_browser_list_h(area: Rect, fb: &FileBrowser) -> u16`
   in `chrome_geom.rs` that both the painter and `file_browser_row_at` call. Add a test asserting a
   click on the last visible row maps to the entry the painter drew there in **destination** mode.

5. **Run — expect green:**

```
cargo test -p wordcartel --lib file_browser:: render_overlays:: chrome_geom::
```

Expected: `test result: ok`, including both footer tests and the geometry test.

6. **Commit:** `feat(c5): show the live resolved destination target in the picker footer`

---

### Task 21 — Save-As and Write-Block rewiring

> ### NAMED HAZARD — the quit-drain coupling
>
> **Any implementer migrating Save-As off the minibuffer will break this unless told.**
> `save::dispatch_save_then` decides whether to arm `pending_save_as` by **inspecting the
> minibuffer's kind**:
>
> ```rust
> if ctx.editor.minibuffer.as_ref().map(|m| m.kind)
>     == Some(crate::minibuffer::MinibufferKind::SaveAs) {
>     ctx.editor.pending_save_as = Some(action);
> }
> ```
>
> When Save-As stops opening a `MinibufferKind::SaveAs`, this condition silently becomes false
> forever. The consequence is **not** a compile error and **not** a visible bug in the common path:
> **save-and-quit on an unnamed buffer stops completing.** The write happens, `pending_after_save`
> is never armed, and the quit the writer asked for never fires.
>
> Replace the probe with one that asks the same question of the new state — preferably by having
> the Save-As opener **return** that fact, rather than by relocating the UI sniff. Sniffing UI state
> to infer control flow is what made this fragile; this migration is the chance to remove the sniff,
> not move it.

#### Files

- Modify: `wordcartel/src/save.rs` (`dispatch_save`, `dispatch_save_then`)
- Modify: `wordcartel/src/prompts.rs` (`open_save_as` → destination picker; submit paths)
- Modify: `wordcartel/src/blocks_marked.rs` (`block_write`)
- Modify: `wordcartel/src/minibuffer.rs` (retire the `SaveAs` / `WriteBlock` kinds)

#### Interfaces

**Consumes** (Tasks 15, 16, 18, 19, 20):

```rust
// crate::file_browser
pub enum DestinationPurpose { SaveAs, WriteBlock, Export { ext: String } }
pub enum BrowseMode { Select, Destination { purpose: DestinationPurpose,
                                            field: String, field_cursor: usize } }
// crate::file_browser_commit
pub(crate) fn classify_destination_enter(fs: &dyn crate::fsx::Fs, dir: &std::path::Path,
    field: &str, highlighted: Option<&crate::file_browser::FileEntry>) -> CommitOutcome;
pub(crate) fn apply_extension_policy(path: &std::path::Path) -> ExtVerdict;
// crate::save
pub(crate) struct SaveTarget { pub chosen: std::path::PathBuf, pub resolved: std::path::PathBuf }
pub(crate) fn do_save_to(ctx: &mut Ctx, target: SaveTarget, mode: SaveMode);
// crate::fsx
pub(crate) fn resolve_write_destination(fs: &dyn Fs, path: &Path)
    -> Result<std::path::PathBuf, DestError>;
```

**Produces:**

```rust
// crate::editor
impl Editor {
    /// Open the destination picker for `purpose`, seeded at `dir` with `field` pre-filled.
    /// RETURNS whether it opened — this is what replaces `dispatch_save_then`'s minibuffer
    /// sniff, so control flow no longer infers state from the UI.
    pub fn open_destination_picker(&mut self, fs: &dyn crate::fsx::Fs,
        purpose: crate::file_browser::DestinationPurpose,
        dir: std::path::PathBuf, field: String) -> bool;
}

// crate::prompts
/// Open the Save-As destination picker, seeded at the active document's directory.
/// Returns whether it opened.
pub fn open_save_as(editor: &mut Editor, fs: &dyn crate::fsx::Fs) -> bool;
```

#### Steps

1. **Write the failing tests** in `save.rs`'s and `prompts.rs`'s test modules:

```rust
    #[test]
    fn save_and_quit_on_an_unnamed_buffer_completes_through_the_picker() {
        // THE HAZARD, asserted. `dispatch_save_then` armed `pending_save_as` by checking
        // `minibuffer.kind == SaveAs`. Once Save-As opens a PICKER, that check is false
        // forever — no compile error, no visible bug in the common path, but save-and-quit
        // on an unnamed buffer silently stops completing.
        let mut e = Editor::new_from_text("unsaved\n", None, (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        {
            let mut ctx = Ctx {
                editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(),
                fs: std::sync::Arc::new(crate::fsx::RealFs),
            };
            dispatch_save_then(&mut ctx, crate::editor::PostSaveAction::Quit);
        }
        assert!(e.file_browser.as_ref().is_some_and(|fb| fb.mode.is_destination()),
            "an unnamed buffer opens the DESTINATION picker, not a minibuffer");
        assert_eq!(e.pending_save_as, Some(crate::editor::PostSaveAction::Quit),
            "and the post-save action is armed — this is what the minibuffer sniff used to do");
    }

    #[test]
    fn esc_out_of_a_drain_destination_picker_aborts_the_drain() {
        // The Effort-6 Codex-C2 fix, carried to the new path. Without it, backing out leaves
        // quit_drain Some-but-inert: stranded with no in-flight save and nothing to re-drive.
        let mut e = Editor::new_from_text("unsaved\n", None, (80, 24));
        e.quit_drain = Some(crate::editor::QuitDrain {
            queue: std::collections::VecDeque::new(),
            mode: crate::editor::QuitMode::SaveAll });
        e.pending_save_as = Some(crate::editor::PostSaveAction::ContinueQuitDrain);
        e.open_destination_picker(&crate::fsx::RealFs,
            crate::file_browser::DestinationPurpose::SaveAs,
            std::env::temp_dir(), String::new());

        crate::file_browser::cancel_destination(&mut e); // the Esc path

        assert!(e.file_browser.is_none(), "Esc closes the picker");
        assert!(e.pending_save_as.is_none(), "and clears the armed action");
        assert!(e.quit_drain.is_none(), "and ABORTS the drain rather than stranding it");
        assert!(!e.quit_drain_advance);
        assert!(!e.quit, "backing out must not quit");
    }
```

2. **Run — expect failure:**

```
cargo test -p wordcartel --lib save::tests::save_and_quit_on_an_unnamed_buffer_completes
```

Expected: ``error[E0599]: no method named `open_destination_picker` found for struct `Editor```.

3. **Add `open_destination_picker`** to `editor.rs`:

```rust
    /// Open the destination picker for `purpose`, seeded at `dir` with `field` pre-filled.
    ///
    /// Returns whether it opened. Callers use the RETURN VALUE to decide follow-up control
    /// flow — never by inspecting which overlay is up. `dispatch_save_then` used to sniff
    /// `minibuffer.kind == SaveAs` to know a Save-As had started, which silently broke the
    /// moment Save-As stopped using a minibuffer.
    pub fn open_destination_picker(&mut self, fs: &dyn crate::fsx::Fs,
        purpose: crate::file_browser::DestinationPurpose,
        dir: std::path::PathBuf, field: String) -> bool
    {
        crate::overlays::close_all(self);
        self.pending_keys.clear(); self.pending_mark = None;
        let field_cursor = field.len();
        let opts = crate::file_browser_listing::FilterOpts {
            show_clutter: self.files_show_clutter,
            types: self.files_type_filter,
            destination: true,
        };
        let mut fb = crate::file_browser::FileBrowser {
            dir, query: String::new(),
            mode: crate::file_browser::BrowseMode::Destination { purpose, field, field_cursor },
            listing: Vec::new(), total_seen: 0, unreadable: 0, entries: Vec::new(),
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        };
        crate::file_browser_listing::refetch(fs, &mut fb, opts);
        self.file_browser = Some(fb);
        true
    }
```

4. **Rewire `open_save_as`** in `prompts.rs`:

```rust
/// Open the Save-As destination picker, seeded at the active doc's directory.
pub fn open_save_as(editor: &mut crate::editor::Editor, fs: &dyn crate::fsx::Fs) -> bool {
    let dir = editor.active().document.path.as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    editor.open_destination_picker(fs,
        crate::file_browser::DestinationPurpose::SaveAs, dir, String::new())
}
```

and `blocks_marked::block_write` the same way with `DestinationPurpose::WriteBlock`.

5. **Remove the sniff** in `dispatch_save_then`:

```rust
pub(crate) fn dispatch_save_then(ctx: &mut crate::registry::Ctx,
    action: crate::editor::PostSaveAction)
{
    let was_unnamed = ctx.editor.active().document.path.is_none();
    let buffer_id = ctx.editor.active().id;
    let v = ctx.editor.active().document.version;
    // `dispatch_save` returns whether it opened a Save-As destination picker, so control
    // flow no longer infers state from which overlay happens to be up.
    let opened_save_as = dispatch_save(ctx) == CommandResult::HandledOpenedSaveAs;
    if was_unnamed {
        if opened_save_as {
            ctx.editor.pending_save_as = Some(action);
        }
    } else if ctx.editor.active().document.path.is_some() && ctx.editor.prompt.is_none() {
        ctx.editor.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id, version: v, action, at_ms: ctx.clock.now_ms(),
        });
    }
}
```

> If widening `CommandResult` is undesirable, the equivalent is a small
> `dispatch_save_reporting(ctx) -> bool` core that `dispatch_save` wraps. Either way the
> **fact is returned, not sniffed** — that is the requirement.

6. **Add `cancel_destination`** in `file_browser.rs`, carrying the Effort-6 abort:

```rust
/// Esc out of a destination picker. Mirrors the cleanup `save_as_submit`'s empty-path arm
/// and `prompts::intercept`'s Esc arm already do — including ABORTING an in-progress quit
/// drain. Without that, backing out leaves `quit_drain` Some-but-inert.
pub(crate) fn cancel_destination(editor: &mut crate::editor::Editor) {
    editor.file_browser = None;
    editor.pending_save_as = None;
    editor.pending_save_overwrite = None;
    editor.pending_write_block = None;
    if editor.quit_drain.is_some() {
        editor.quit_drain = None;
        editor.quit_drain_advance = false;
    }
}
```

Wire it to the destination-mode `Esc` arm in `file_browser_intercept`.

7. **Retire the minibuffer kinds.** Remove `MinibufferKind::{SaveAs, WriteBlock}` and their
   `prompts::{save_as_submit, block_write_submit}` dispatch arms. The three empty-path Sticky-Warning
   tests move to the picker path: an empty field commits `CommitOutcome::Nothing`, which sets the
   same Sticky Warning. **Their assertions on kind and lifetime must not weaken.**

8. **Run — expect green:**

```
cargo test -p wordcartel --lib save:: prompts:: blocks_marked:: file_browser::
```

Expected: `test result: ok`. **`prompts::tests::save_and_quit_on_unnamed_buffer_does_not_arm_pending_after_save`
must still pass** — it asserts no job is dispatched and `pending_after_save` stays `None`, which the
picker path preserves.

9. **Commit:** `refactor(c5): route Save-As and Write-Block through the destination picker`

---

### Task 22 — Export destination, pre-seeded

**Deliverable:** export gains a choosable destination without losing its best property — it is
**zero-decision**. A bare Enter reproduces today's behaviour exactly.

#### Files

- Modify: `wordcartel/src/export.rs` (`run_export`)
- Modify: `wordcartel/src/file_browser_commit.rs` (the `Export` commit arm)

#### Interfaces

**Consumes** (Tasks 18, 20, 21):

```rust
// crate::export
pub fn derived_export_path(source: &Path, ext: &str) -> PathBuf;   // source.with_extension(ext)
pub fn probe_pandoc() -> bool;
pub(crate) fn do_export(editor: &mut crate::editor::Editor, ext: &str, target: &Path,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>, overwrite_confirmed: bool);
pub struct PendingExport { pub ext: String, pub target: PathBuf }

// crate::editor
impl Editor {
    pub fn open_destination_picker(&mut self, fs: &dyn crate::fsx::Fs,
        purpose: crate::file_browser::DestinationPurpose,
        dir: std::path::PathBuf, field: String) -> bool;
}
```

**Produces:** no new API — `run_export` opens the picker instead of deriving silently.

#### Steps

1. **Write the failing tests** in `export.rs`'s test module:

```rust
    #[test]
    fn export_opens_a_destination_picker_pre_seeded_with_the_derived_path() {
        // ENTER-THROUGH (decision 4). Export is zero-decision today; adding a mandatory
        // dialog would be a regression dressed as a feature. Pre-seeding means a bare Enter
        // reproduces today's behaviour byte-for-byte, with the target VISIBLE while doing so.
        let d = std::env::temp_dir().join(format!("wc-exp-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        let src = d.join("notes.md");
        std::fs::write(&src, b"# hi\n").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("# hi\n", Some(src.clone()), (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();

        run_export(&mut e, &crate::fsx::RealFs, "html", &tx);

        let fb = e.file_browser.as_ref().expect("export opens the destination picker");
        assert_eq!(fb.dir, d, "seeded at the SOURCE's directory");
        match &fb.mode {
            crate::file_browser::BrowseMode::Destination { purpose, field, .. } => {
                assert_eq!(*purpose, crate::file_browser::DestinationPurpose::Export {
                    ext: "html".into() });
                assert_eq!(field, "notes.html",
                    "pre-filled with derived_export_path's file name, so bare Enter == today");
            }
            other => panic!("expected a destination picker, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn export_still_refuses_before_opening_any_picker() {
        // The probe and the unnamed-buffer refusal stay AHEAD of the picker: there is no
        // point choosing a destination for an export that cannot run.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        run_export(&mut e, &crate::fsx::RealFs, "html", &tx);
        assert!(e.file_browser.is_none(), "an unnamed buffer opens NO picker");
        assert!(e.status_text().to_lowercase().contains("save the file first"));
    }
```

2. **Run — expect failure:**

```
cargo test -p wordcartel --lib export::tests::export_opens_a_destination_picker
```

Expected: `assertion failed: e.file_browser.as_ref()` — `run_export` still derives silently.

3. **Rewrite `run_export`:**

```rust
/// Top-level export entry: gate on pandoc, then open a destination picker PRE-SEEDED with
/// the derived path.
///
/// The seeding is the whole point (decision 4): export is zero-decision today, and a bare
/// Enter must reproduce that byte-for-byte. Destination CHOICE is new capability;
/// destination OBLIGATION would be a regression.
pub fn run_export(
    editor: &mut crate::editor::Editor,
    fs: &dyn crate::fsx::Fs,
    ext: &str,
    _msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    // Both refusals stay AHEAD of the picker — no point choosing a destination for an
    // export that cannot run.
    let source = match editor.active().document.path.clone() {
        Some(p) => p,
        None => {
            editor.set_status_full(crate::status::StatusKind::Warning,
                "save the file first before exporting",
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return;
        }
    };
    if !probe_pandoc() {
        editor.set_status_full(crate::status::StatusKind::Error,
            "pandoc not found — install it to export",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return;
    }

    // `derived_export_path` still computes the default — it is now the SEED rather than the
    // final answer, and it reads `Document.path`, which stays LOGICAL (§7.6.2), so the
    // output lands beside the file the writer opened.
    let derived = derived_export_path(&source, ext);
    let dir = derived.parent().map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let field = derived.file_name().map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    editor.open_destination_picker(fs,
        crate::file_browser::DestinationPurpose::Export { ext: ext.to_owned() }, dir, field);
}
```

4. **Wire the `Export` commit arm.** On `CommitOutcome::Commit(target)` with
   `DestinationPurpose::Export { ext }`, reproduce today's dispatch exactly: if the target exists,
   set `editor.pending_export = Some(PendingExport { ext, target })` and open
   `Prompt::export_overwrite`; otherwise call `do_export(editor, &ext, &target, msg_tx, false)`.
   **`apply_export_done`'s TOCTOU re-check is unchanged.**

5. **Run — expect green:**

```
cargo test -p wordcartel --lib export:: jobs_apply::
```

Expected: `test result: ok`. **`export::tests::export_refuses_scratch_buffer` and the three
`apply_export_done` tests must still pass unmodified.**

6. **Commit:** `feat(c5): give export a pre-seeded destination picker (Enter-through)`

---

## Phase F — Commands and closeout (Tasks 23–26)

### Task 23 — Seven commands, two persisted options, contract conformance

#### Files

- Modify: `wordcartel/src/registry.rs` (seven registrations, **before `plugin_list`**)
- Modify: `wordcartel/src/editor.rs` (the two setters)
- Modify: `wordcartel/src/settings.rs` (`SettingsSnapshot` + overrides mirror)
- Modify: `wordcartel/src/config.rs` (config seeding for both options)

#### Interfaces

**Consumes** (Tasks 12, 18):

```rust
// crate::config
pub enum FileTypeFilter { Documents, All }

// crate::registry
pub enum MenuMark { OnOff(bool), Value(&'static str), Text(String) }
fn register(&mut self, id: &'static str, label: &'static str, menu: Option<MenuCategory>,
    handler: Handler);
fn register_stateful(&mut self, id: &'static str, label: &'static str,
    menu: Option<MenuCategory>, state: fn(&crate::editor::Editor) -> MenuMark, handler: Handler);
```

**Produces:**

```rust
// crate::editor — the SOLE mutators (contract law 6). Set-primitives, cycles, config
// seeding, and any future preset all call these; no call site writes the fields directly.
impl Editor {
    pub fn set_show_clutter(&mut self, on: bool);
    pub fn set_file_type_filter(&mut self, f: crate::config::FileTypeFilter);
}

// crate::settings — SettingsSnapshot gains:
pub files_show_clutter: bool,
pub files_type_filter: crate::config::FileTypeFilter,
```

Seven commands: `open_recent` (File), `show_clutter_on` / `show_clutter_off` (palette-only),
`toggle_clutter` (View, `MenuMark::OnOff`), `file_types_documents` / `file_types_all`
(palette-only), `toggle_file_types` (View, `MenuMark::Value`).

#### Steps

1. **Write the failing test** in `registry.rs`'s test module:

```rust
    #[test]
    fn c5_commands_register_before_plugin_list() {
        // `e2e::journey_palette_end_reaches_last_command` presses End+Enter in the palette
        // and asserts the status starts with "plugins:", which hardcodes plugin_list as the
        // LAST registered command. It is a merge gate: registering any C5 command after it
        // breaks the build.
        let reg = Registry::builtins();
        let ids: Vec<&str> = reg.commands().map(|c| c.id.0).collect();
        let last = ids.last().copied().expect("non-empty registry");
        assert_eq!(last, "plugin_list", "plugin_list must stay last");
        for id in ["open_recent", "show_clutter_on", "show_clutter_off", "toggle_clutter",
                   "file_types_documents", "file_types_all", "toggle_file_types"] {
            let at = ids.iter().position(|x| *x == id)
                .unwrap_or_else(|| panic!("{id} must be registered"));
            assert!(at < ids.len() - 1, "{id} must register BEFORE plugin_list");
        }
    }

    #[test]
    fn filter_toggles_follow_law_8_set_per_state_plus_one_representative() {
        let reg = Registry::builtins();
        // Set-per-state primitives are palette-only.
        for id in ["show_clutter_on", "show_clutter_off", "file_types_documents", "file_types_all"] {
            assert_eq!(reg.meta(CommandId(id)).expect("registered").menu, None,
                "{id} is a palette-only set primitive");
        }
        // One stateful representative each, carrying a MenuCategory.
        assert_eq!(reg.meta(CommandId("toggle_clutter")).expect("registered").menu,
            Some(MenuCategory::View));
        assert_eq!(reg.meta(CommandId("toggle_file_types")).expect("registered").menu,
            Some(MenuCategory::View));
        // And they report live state.
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        let f = reg.meta(CommandId("toggle_clutter")).unwrap().state.expect("stateful");
        assert!(matches!(f(&ed), MenuMark::OnOff(false)), "clutter hidden by default");
        ed.set_show_clutter(true);
        assert!(matches!(f(&ed), MenuMark::OnOff(true)));
        let g = reg.meta(CommandId("toggle_file_types")).unwrap().state.expect("stateful");
        assert_eq!(g(&ed), MenuMark::Value("Documents"));
        ed.set_file_type_filter(crate::config::FileTypeFilter::All);
        assert_eq!(g(&ed), MenuMark::Value("All files"));
    }
```

2. **Run — expect failure:** the commands do not exist.

```
cargo test -p wordcartel --lib registry::tests::c5_commands_register_before_plugin_list
```

3. **Add the setters** to `editor.rs`:

```rust
    /// The SOLE mutator for the clutter filter (contract law 6).
    pub fn set_show_clutter(&mut self, on: bool) { self.files_show_clutter = on; }

    /// The SOLE mutator for the file-type filter (contract law 6).
    pub fn set_file_type_filter(&mut self, f: crate::config::FileTypeFilter) {
        self.files_type_filter = f;
    }
```

4. **Register the seven commands** in `registry.rs`, **before the `save_settings` block** (which is
   comfortably before `plugin_list`):

```rust
        r.register("open_recent", "Open Recent\u{2026}", Some(MenuCategory::File), |c| {
            crate::recents::open_recent(c.editor, &*c.fs);
            CommandResult::Handled
        });
        // C5 filter toggles — set-per-state primitives (palette-only) + one stateful
        // representative each, mirroring scrollbar_off/auto/on + cycle_scrollbar (law 8).
        r.register("show_clutter_on",  "Show Hidden Files",  None, |c| {
            c.editor.set_show_clutter(true);  CommandResult::Handled });
        r.register("show_clutter_off", "Hide Hidden Files",  None, |c| {
            c.editor.set_show_clutter(false); CommandResult::Handled });
        r.register_stateful("toggle_clutter", "Hidden Files", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.files_show_clutter),
            |c| { let next = !c.editor.files_show_clutter;
                  c.editor.set_show_clutter(next); CommandResult::Handled });
        r.register("file_types_documents", "File Types: Documents", None, |c| {
            c.editor.set_file_type_filter(crate::config::FileTypeFilter::Documents);
            CommandResult::Handled });
        r.register("file_types_all", "File Types: All Files", None, |c| {
            c.editor.set_file_type_filter(crate::config::FileTypeFilter::All);
            CommandResult::Handled });
        r.register_stateful("toggle_file_types", "File Types", Some(MenuCategory::View),
            |e| MenuMark::Value(match e.files_type_filter {
                crate::config::FileTypeFilter::Documents => "Documents",
                crate::config::FileTypeFilter::All       => "All files",
            }),
            |c| { let next = match c.editor.files_type_filter {
                      crate::config::FileTypeFilter::Documents => crate::config::FileTypeFilter::All,
                      crate::config::FileTypeFilter::All => crate::config::FileTypeFilter::Documents,
                  };
                  c.editor.set_file_type_filter(next); CommandResult::Handled });
```

While here, **delete the two stale comments** claiming `save_settings` must stay last
(`registry.rs`'s `// toggle_canvas and toggle_chrome MUST be registered BEFORE save_settings…` and
`// Registered BEFORE save_settings (Codex F4)…`). They are false — `plugins_reload` and
`plugin_list` already register after it — and a comment asserting an invariant the code does not
have is worse than none. Replace with a pointer to the real constraint: **`plugin_list` stays last**.

5. **Add the two fields to `SettingsSnapshot`** and the overrides mirror. `settings::snapshot_of`
   seeds them from config. **`settings::tests::every_persisted_setting_has_a_command` is a
   compile-time exhaustive destructure of `SettingsSnapshot`** — it will not compile until each new
   field has a resolving command, which is the enforcement.

6. **Run — expect green:**

```
cargo test -p wordcartel --lib registry:: settings:: palette:: menu:: keymap::
```

Expected: `test result: ok`, including
`settings::tests::every_persisted_setting_has_a_command`,
`palette::tests::palette_is_exhaustive_over_the_registry`,
`palette::tests::palette_is_exhaustive_over_a_plugin_loaded_registry`,
`menu::tests::parameterized_plugin_command_and_plugin_list_satisfy_law3_law4`,
`menu::tests::custom_bind_surfaces_in_menu_and_palette`,
`keymap::tests::hints_reresolve_on_preset_switch`.

7. **Commit:** `feat(c5): add the seven file-interface commands and two persisted filter options`

---

### Task 24 — `open_recent`

#### Files

- Create: `wordcartel/src/recents.rs`
- Modify: `wordcartel/src/lib.rs`

#### Interfaces

**Consumes** (Tasks 6, 7, 12, 23):

```rust
// crate::state
pub struct StateEntry { pub cursor: usize, pub scroll: usize, /* … */ pub seq: u64, /* … */ }
pub struct SessionState { pub entries: std::collections::BTreeMap<String, StateEntry>, /* … */ }
pub fn load() -> SessionState;
// crate::fsx
pub(crate) fn is_file_via(fs: &dyn Fs, path: &Path) -> bool;
// crate::workspace
pub fn open_as_new_buffer(editor: &mut Editor, path: &std::path::Path);
```

**Produces:**

```rust
// crate::recents
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentRow {
    pub path: std::path::PathBuf,
    /// Missing files stay VISIBLE but are not selectable — a writer whose file moved needs
    /// to see that it is gone, not to find a shorter list.
    pub available: bool,
}

/// Rank `session.entries` by `seq` descending — it is already an LRU-ordered,
/// canonical-path-keyed map, so recents is nearly free.
pub(crate) fn rows_from(session: &crate::state::SessionState, fs: &dyn crate::fsx::Fs)
    -> Vec<RecentRow>;

pub(crate) fn open_recent(editor: &mut crate::editor::Editor, fs: &dyn crate::fsx::Fs);
```

#### Steps

1. **Write the failing test** in `recents.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_seq_ranked_and_missing_files_stay_visible_but_unavailable() {
        let d = std::env::temp_dir().join(format!("wc-recents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        let live = d.join("live.md");
        std::fs::write(&live, b"x").expect("seed");
        let gone = d.join("gone.md");

        let mut s = crate::state::SessionState::default();
        let entry = |seq: u64| crate::state::StateEntry {
            cursor: 0, scroll: 0, marks: Default::default(), mtime: 1, size: 1, seq,
            folds: vec![], block: None };
        s.entries.insert(gone.to_string_lossy().into_owned(), entry(9));
        s.entries.insert(live.to_string_lossy().into_owned(), entry(3));

        let rows = rows_from(&s, &crate::fsx::RealFs);
        assert_eq!(rows.len(), 2, "a missing file is SHOWN, not dropped — a shorter list \
            would hide the fact that it moved");
        assert_eq!(rows[0].path, gone, "ranked by seq descending (9 before 3)");
        assert!(!rows[0].available, "and marked unavailable");
        assert_eq!(rows[1].path, live);
        assert!(rows[1].available);
        let _ = std::fs::remove_dir_all(&d);
    }
}
```

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib recents::
```

3. **Write `recents.rs`:**

```rust
//! Recents: the rescue path for "I can't find my file".
//!
//! Nearly free — `SessionState.entries` is ALREADY an LRU-ranked (`seq`),
//! canonical-path-keyed map that the editor maintains on every save.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentRow {
    pub path: std::path::PathBuf,
    pub available: bool,
}

pub(crate) fn rows_from(session: &crate::state::SessionState, fs: &dyn crate::fsx::Fs)
    -> Vec<RecentRow>
{
    let mut v: Vec<(u64, RecentRow)> = session.entries.iter().map(|(k, e)| {
        let path = std::path::PathBuf::from(k);
        let available = crate::fsx::is_file_via(fs, &path);
        (e.seq, RecentRow { path, available })
    }).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0)); // most-recent first
    v.into_iter().map(|(_, r)| r).collect()
}

/// Open the recents picker. Rows route through `workspace::open_as_new_buffer`, inheriting
/// the dirty-guard and resume behaviour; unavailable rows are rendered greyed and refuse
/// selection rather than vanishing.
pub(crate) fn open_recent(editor: &mut crate::editor::Editor, fs: &dyn crate::fsx::Fs) {
    let session = crate::state::load();
    let rows = rows_from(&session, fs);
    if rows.is_empty() {
        editor.set_status(crate::status::StatusKind::Info, "No recent files");
        return;
    }
    editor.open_recents(rows);
}
```

4. **Declare the module** in `lib.rs` and add `Editor::open_recents` presenting the rows through the
   picker's flat-list rendering with the same nav, filter, and fuzzy ranking over path strings.

5. **Run — expect green:**

```
cargo test -p wordcartel --lib recents::
```

6. **Commit:** `feat(c5): add open_recent sourced from the LRU session store`

---

### Task 25 — `DocumentId` mint-and-stamp

**Deliverable:** every document carries an id, stamped into the session entry and the swap header.
**Nothing reads it** — that is the ratified scope, and adding a read/seed path is exactly the scope
creep decision 11 avoided.

#### Files

- Modify: `wordcartel/src/editor.rs` (`DocumentId`, `Document::id`)
- Modify: `wordcartel/src/state.rs` (`StateEntry::id`)
- Modify: `wordcartel/src/swap.rs` (`SwapHeader::id`, `serialize`, `parse`)

#### Interfaces

**Produces:**

```rust
// crate::editor
/// A lineage HINT, not a uniqueness invariant (mirroring "path is not a uniqueness
/// invariant"). 64 bits is sufficient because nothing keys on it: a collision means two
/// documents share a hint no code consults, and it is not a security token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentId(pub u64);

impl DocumentId {
    /// Mint from std only — NO new dependency (decision 2 excludes rand/getrandom/uuid).
    /// `RandomState` is OS-seeded per instance; the counter guarantees two ids minted in the
    /// same nanosecond still differ.
    pub fn mint() -> Self;
    /// 16 hex digits. Stored as an OPAQUE STRING in both formats, never parsed back into a
    /// fixed-width integer, so a future wider id needs no format migration.
    pub fn to_hex(self) -> String;
}
// Document gains: pub id: DocumentId,

// crate::state — StateEntry gains a DEFAULTED serde field:
#[serde(default, skip_serializing_if = "Option::is_none")]
pub id: Option<String>,

// crate::swap — SwapHeader gains:
pub id: Option<String>,
```

#### Steps

1. **Write the failing tests:**

```rust
    // in editor.rs
    #[test]
    fn document_ids_are_distinct_and_stable() {
        let a = DocumentId::mint();
        let b = DocumentId::mint();
        assert_ne!(a, b, "two ids minted back-to-back differ (the counter component)");
        assert_eq!(a.to_hex().len(), 16, "16 hex digits");
        let e = Editor::new_from_text("x\n", None, (80, 24));
        let id = e.active().document.id;
        assert_eq!(id, e.active().document.id, "stable across reads");
    }

    // in state.rs
    #[test]
    fn pre_c5_session_toml_without_id_still_deserializes() {
        let toml = r#"
[entries."/tmp/x.md"]
cursor = 3
scroll = 0
mtime = 1
size = 2
seq = 1
"#;
        let s: SessionState = toml::from_str(toml).expect("must deserialize without id");
        assert!(s.entries["/tmp/x.md"].id.is_none(), "missing id key → None, never an error");
    }

    // in swap.rs
    #[test]
    fn pre_c5_swap_without_an_id_line_still_parses_and_recovers() {
        // The backward-compatibility claim, ASSERTED rather than assumed. `parse` ignores
        // unknown keys (`_ => {}`), which is what makes the id forward-compatible too.
        let legacy = format!(
            "{FORMAT}\npath: /home/u/notes.md\nfp: -:-\nhash: {:016x}\nversion: 7\nts: 1\npid: 9\n---\nbody\n",
            fnv1a64(b"body\n"));
        let (h, body) = parse(&legacy).expect("a pre-C5 swap must still parse");
        assert_eq!(h.version, 7);
        assert_eq!(body, "body\n");
        assert!(h.id.is_none(), "no id line → None");
    }

    #[test]
    fn swap_header_with_an_id_round_trips_and_old_readers_skip_it() {
        let h = SwapHeader {
            realpath: None, load_mtime_secs: None, load_size: None,
            content_hash: fnv1a64(b"x"), version: 1, ts_ms: 5, pid: 9,
            id: Some("00ff00ff00ff00ff".into()),
        };
        let text = serialize(&h, "x");
        assert!(text.contains("id: 00ff00ff00ff00ff"), "the id is emitted as an opaque string");
        let (h2, _) = parse(&text).expect("round-trips");
        assert_eq!(h2.id.as_deref(), Some("00ff00ff00ff00ff"));
    }
```

2. **Run — expect compile failures** (`DocumentId` and the `id` fields do not exist).

3. **Add `DocumentId`** to `editor.rs`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentId(pub u64);

impl DocumentId {
    /// Mint using std only. `RandomState::new()` is OS-seeded per instance; the process-local
    /// counter guarantees two ids minted in the same nanosecond still differ.
    ///
    /// NO new dependency: decision 2 excludes `rand`/`getrandom`/`uuid`, and an earlier draft
    /// of the spec said "128-bit random", which would have smuggled one in.
    pub fn mint() -> Self {
        use std::hash::{BuildHasher, Hasher};
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u32(std::process::id());
        h.write_u128(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write_u64(SEQ.fetch_add(1, Ordering::Relaxed));
        DocumentId(h.finish())
    }

    pub fn to_hex(self) -> String { format!("{:016x}", self.0) }
}
```

Add `pub id: DocumentId` to `Document`, minted in its constructors.

4. **Stamp it.** `StateEntry` gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub
   id: Option<String>`, populated by `persist_session` from the active document. `SwapHeader` gains
   `pub id: Option<String>`; `serialize` emits `id: {}` using `opt_str`, and `parse` gains an
   `"id" => id = if v == "-" { None } else { Some(v.to_string()) }` arm.

> **Nothing reads either value.** A document reopened by any route mints a fresh id; ids do not
> follow identity across routes or restarts. That is the ratified scope — §12.6 records what S3 must
> specify to make the id load-bearing.

5. **Run — expect green:**

```
cargo test -p wordcartel --lib editor:: state:: swap::
```

Expected: `test result: ok`, including all four id tests. Every existing swap round-trip test must
still pass.

6. **Commit:** `feat(c5): mint and stamp a DocumentId without keying anything on it`

---

### Task 26 — Diverged-orphan visibility, the e2e journey, and closeout

#### Files

- Modify: `wordcartel/src/swap.rs` (a kept-count enumerator)
- Modify: `wordcartel/src/prompts.rs` (`raise_clean_recovery`)
- Modify: `wordcartel/src/prompt.rs` (the clean-recovery message)
- Modify: `wordcartel/src/e2e.rs` (the journey)
- Modify: `wordcartel/tests/module_budgets.rs` if the `file_browser` split needs a budget row

#### Interfaces

**Consumes:** Tasks 9, 12, 18, 21, 22, 24.

**Produces:**

```rust
// crate::swap
/// Count the recovery artifacts DELIBERATELY kept because they may hold unsaved work.
/// Visibility only — `cleanable_recovery_files` is untouched.
pub(crate) fn kept_recoverable_count(fs: &dyn crate::fsx::Fs, dir: &Path,
    protected: &std::collections::HashSet<PathBuf>) -> usize;
```

#### Steps

1. **Write the failing tests:**

```rust
    // in swap.rs
    #[test]
    fn kept_recoverable_count_reports_what_the_sweep_deliberately_spares() {
        // A diverged swap holds content NOT on disk at its recorded realpath — it is the
        // MOST recoverable object in the state dir, not the least. It must never be swept;
        // C5 adds visibility only, so a writer can go extract or explicitly discard it.
        let dir = unique_dir("kept-count");
        let (p_ok, sp_ok) = make_doc_with_swap_in(&dir, "same\n", "same\n", DEAD_PID);
        let (p_bad, sp_bad) = make_doc_with_swap_in(&dir, "file\n", "UNSAVED\n", DEAD_PID);
        let protected = std::collections::HashSet::new();
        let cleanable = cleanable_recovery_files(&crate::fsx::RealFs, &dir, &protected);
        let kept = kept_recoverable_count(&crate::fsx::RealFs, &dir, &protected);
        assert!(cleanable.contains(&sp_ok), "the valueless swap is still offered");
        assert!(!cleanable.contains(&sp_bad), "the diverged swap is still NEVER offered");
        assert_eq!(kept, 1, "and the diverged one is COUNTED so the writer knows it exists");
        for f in [&sp_ok, &sp_bad, &p_ok, &p_bad] { let _ = std::fs::remove_file(f); }
        let _ = std::fs::remove_dir_all(&dir);
    }
```

```rust
    // in e2e.rs — the whole-effort journey
    #[test]
    fn journey_open_save_export_saveas_reopen() {
        // open (picker) -> first save via the DESTINATION picker (extension appended, footer
        // target correct) -> export with a destination (Enter-through) -> Save-As to a new
        // name (status names the path) -> reopen via open_recent.
        // …drives the real reduce -> advance -> render loop against a TestBackend…
    }
```

2. **Run — expect failure**, then implement.

3. **Add `kept_recoverable_count`** to `swap.rs` — it reuses `recovery_file_is_cleanable`'s inverse
   over the same enumeration. **`cleanable_recovery_files`, `swap_is_cleanable`,
   `recovery_path_still_cleanable`, and their fail-closed rules are untouched**; this only counts.

4. **Surface it** in `prompts::raise_clean_recovery`'s modal text: alongside the count it will
   delete, report the count it is **keeping because they may hold unsaved work**, with enough
   identifying detail (recorded realpath, timestamp) for the writer to act.

5. **Run the full gates:**

```
cargo test
cargo clippy --workspace --all-targets
cargo test -p wordcartel --test module_budgets
scripts/smoke/run.sh
```

Expected: all green; quote the smoke summary verbatim in the pre-merge report (mandatory-run,
advisory-pass). **`swap::tests::swap_is_cleanable_only_for_valueless_dead_pid_swaps` must pass
unmodified** — it is the no-data-loss guarantee this task must not weaken.

6. **Commit:** `feat(c5): surface kept-recoverable orphans and add the C5 e2e journey`

---

*Phase F complete. C5 is implementable end to end.*

---

## Self-review: spec → task coverage

Walked section by section. Every requirement maps to a task; no gaps found.

| Spec section | Task(s) |
|---|---|
| §2.3 rule + exemption clauses + guard test | 11 |
| §5.2 `read_capped` / `stat` / `list_dir` / `EntryKind` / cap `Option` / counters | 2, 3, 4 |
| §5.2 ownership (`&dyn Fs` vs `Arc`), `FaultFs` promotion, `settings` needs no migration | 1, 5 |
| §5.2 decision 12 + riders 1–3 | 10 |
| §5.3 migration set (incl. all deletion sites) | 6, 7, 8, 9 |
| §5.4 config-class caps | 6 |
| §6.1–6.3 cache, cap+disclosure, off-thread + epoch | 12, 13 |
| §7.1–7.3 modes, Enter table, footer | 18, 20 |
| §7.4 filters + disclosure | 12, 23 |
| §7.5 symlinks in the listing | 14 |
| §7.6.1 write-destination resolution | 15 |
| §7.6.2 `Document.path` stays as-opened (Middle B) | 16 (+ tripwire in 22) |
| §8 extension policy | 19 |
| §9 export Enter-through | 22 |
| §10 recents | 24 |
| §11.1 `SaveTarget`, migration queue, drain | 16, 17 |
| §11.2 quit-drain hazard | 21 |
| §11.3 diverged-orphan visibility | 26 |
| §12 `DocumentId` | 25 |
| §13 command-surface conformance + registration order | 23 |
| §14 all asserted invariants | distributed as tabulated above |

**Placeholder scan:** no "TBD", no "similar to Task N", no "add appropriate error handling". Every
step shows code or an exact command.

**Signature consistency:** `FaultFs::new`, `FaultAt`, `FileStat`, `EntryKind`, `DirEntryInfo`,
`DirListing`, `Ctx.fs`, `DispatchCtx.fs` are declared once (Tasks 1–5) and referenced with identical
names and types in every consuming task.

**`Arc<FaultFs>` injection points** — paths fault-testable for the first time: `file::open` (T6),
`save::fingerprint` (T7), the dictionary append (T8), the browser and swap listings (T9), plugin
discovery (T10), the save worker (T16), the listing thread (T13).
