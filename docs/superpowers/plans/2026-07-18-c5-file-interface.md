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

### Keystroke tests drive the real intercept

Any test asserting that a key or click *reaches* something must go through the real entry point:
`test_support::{press_key_fb, press_char_fb, press_enter_fb}` for keys,
`mouse::mouse_file_browser` for clicks. Calling the handler directly proves the handler works,
not that the gesture reaches it — the vacuous-guard shape this plan has had to correct repeatedly.

These helpers are `pub(crate)` in `test_support` (defined in Task 12), so **each consuming test
module needs `use crate::test_support::{…};`** — Tasks 13, 18, 21, 23 and 26 all consume them.
`nix_privileged()` lives there too: any chmod-based unreadability test must skip loudly on it
rather than assert a false negative under root.

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

### Guard tests must be FAIL-VERIFIED before the task is complete

A guard test exists to fail when a specific regression is reintroduced. **A guard nobody has watched
fail is a guard nobody knows works.** This project has shipped vacuous guardrails before (S8 shipped
two), and this plan authored a third — a syscall-economy test that counted through a wrapper the
regression would have bypassed entirely.

So for **every test whose stated purpose is "this fails if someone does X"**, the implementer must,
before committing:

1. Reintroduce X in the implementation (the stat-everything loop, the `Option` slot, the
   dispatch-time capture, the `RealFs` wrapper in the worker, the token-only scanner…).
2. Run the test and **watch it fail** — confirming it observes the code under test, not a
   delegate or a copy.
3. Revert, re-run, confirm green.

Tasks flag these inline as **FAIL-VERIFY**. The rule exists because the failure mode is silent: a
vacuous guard is indistinguishable from a working one until the regression it was meant to catch
ships. Assertions that merely restate what the code just did do not need this; assertions that claim
to *prevent* something do.

### Concrete `Fs` construction — allowed in exactly three places

`Arc::new(RealFs)` / `&RealFs` appears ONLY in:

1. **`app::run`'s composition root** — the one production place a concrete `Fs` is chosen.
2. **Thin `RealFs` wrappers** that preserve a pre-existing public signature
   (`pub fn open(p) { open_with_fs(&RealFs, p) }`). These are the API boundary, not new choices.
3. **Test bodies**, which legitimately pick their own `Fs` — usually a `FaultFs`.

Plus one allow-listed exception: `recovery::dump_on_panic`, which runs from a process-global panic
hook with no access to `Ctx` and must have no dependencies.

**Two defect shapes, both making downstream code unreachable by an injected `FaultFs`:**

- A production function that RECEIVES an `fs` and **constructs** its own instead (caught in the save
  worker, then in `perform_save_as`).
- A production call site that **passes** a concrete `RealFs` to a function which already accepts
  `&dyn Fs` or `Arc<dyn Fs + …>` (caught at `app.rs`'s `perform_settings_save` call).

Sweep for **both**. When stripping test code from the sweep, strip on `#[cfg(test)] mod tests`, not
on any `#[cfg(test)]` attribute — an attribute-based filter stops at test-only imports near the top
of a file and silently skips everything after, which is exactly how the `perform_settings_save` site
was missed on the first pass.

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
| `wordcartel/src/recents.rs` | `open_recent` rows, ranked from the LRU session store. | 23 |
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

# Dependency graph — verified by an ordered walk

Every symbol a task uses must exist by the time that task runs. Plan-gate round 1 found four
ordering violations — four instances of one missing check, not four slips. The walk below is that
check, recorded so a future edit re-runs it rather than rediscovering the class.

### How to re-run this walk — and the two ways it has been got wrong

**Scan EVERY fenced code block in every task, INCLUDING test bodies.** Round 1's walk covered
implementation snippets and skipped tests, and certified "no fifth violation" while a fifth sat in a
test body. Under TDD the test is written **first**, so a forward reference there blocks the task
harder than one in implementation code — it fails before there is any implementation to fix. Treat a
symbol not produced by an earlier task as a violation **regardless of which section it appears in**,
and check prose instructions too (round 2's `cancel_destination` reference was in a step, not a
snippet).

**The walk catches forward references, NOT signature evolution.** It answers "does this symbol exist
yet?" — it cannot see that Task 12 introduced `open_file_browser(&dyn Fs, PathBuf)` and Task 13 needs
`open_file_browser(&Arc<…>, &Sender<Msg>, PathBuf)`. Both tasks name the same symbol, so a
symbol-existence check passes while the code does not compile. **When a task changes a signature an
earlier task produced, re-read that earlier task's call sites by hand.** Round 2's second Critical was
exactly this, and no symbol walk would have found it.

**Producers, in order.** A task may consume only from tasks above it.

| Task | Produces (the symbols later tasks name) |
|---|---|
| 1 | `test_support::{FaultAt, FaultFs}`, `FaultFs::new` |
| 2 | `Fs::read_capped`, `FaultAt::ReadCapped` |
| 3 | `fsx::FileStat`, `Fs::stat`, `FaultAt::Stat` |
| 4 | `fsx::{EntryKind, DirEntryInfo, DirListing}`, `Fs::list_dir`, `limits::MAX_DIR_ENTRIES`, `FaultAt::ListDir` |
| 5 | `Ctx.fs`, `DispatchCtx.fs` (both `Arc<dyn Fs + Send + Sync>`) |
| 6 | `limits::MAX_CONFIG_BYTES`; `*_with_fs` cores for `file::{open, open_bounded, bounded_read_opt}`, `config::load`, `state::load_in`, `theme_resolve::resolve_theme`; **all of `swap.rs`'s `fs` threading** |
| 7 | `fsx::{exists_via, is_file_via}`, `save::fingerprint_with_fs`, `state::file_identity_with_fs`, `file::save_atomic_with_fs` |
| 8 | `file::save_atomic_bytes_with_fs`, `diagnostics_run::append_word_to_dict_with_fs`, `swap::delete_with_fs` |
| 9 | (bodies only — `swap`'s listing signatures came from Task 6) `file_browser::rebuild_entries(fs, fb)` |
| 10 | `plugin::load::{discover_with_fs, is_plausible_plugin}` |
| 11 | — (integration test only) |
| 12 | `config::FileTypeFilter`; `FileEntry`/`FileBrowser` final shapes; `Editor.{files_show_clutter, files_type_filter}`; `file_browser_listing::{FilterOpts, Disclosure, is_clutter, is_document, filter_and_rank, refetch, rederive}` |
| 13 | `Msg::ListingDone`, `file_browser::{LISTING_EPOCH, next_epoch, start_listing, apply_listing_done}`, `FileBrowser.{awaiting_epoch, pending_dir}` |
| 14 | `file_browser::{entry_label, EnterOutcome, classify_enter, file_browser_enter}` |
| 15 | `fsx::{DestError, resolve_write_destination}` |
| 16 | `save::{SaveTarget, SaveTarget::same, do_save_to}`, `prompts::perform_save_as` |
| 17 | `editor::SessionMigration`, `Editor.pending_session_migrations`, `session_restore::drain_session_migrations` |
| 18 | `file_browser::{DestinationPurpose, BrowseMode, click_commit_or_copy}`, `FileBrowser.mode`, `file_browser_commit::{CommitOutcome, classify_destination_enter, resolve_field, copy_name_into_field}`, `minibuffer::{text_insert, text_backspace, text_left, text_right}` |
| 19 | `file_browser_commit::{ExtVerdict, apply_extension_policy}` |
| 20 | `file_browser::footer_target`, `chrome_geom::file_browser_list_h` |
| 21 | `Editor::open_destination_picker`, `prompts::open_save_as`, `file_browser::cancel_destination`, `save::dispatch_save_reporting` |
| 22 | — (rewires `export::run_export`; no new API) |
| 23 | `recents::{RecentRow, rows_from, open_recent}`, `Editor::open_recents` |
| 24 | `Editor::{set_show_clutter, set_file_type_filter}`, two `SettingsSnapshot` fields, seven commands |
| 25 | `editor::DocumentId`, `Document.id`, `StateEntry.id`, `SwapHeader.id` |
| 26 | `swap::kept_recoverable_count` |

**Violations found and how each was resolved** — recorded so the fix is auditable, not just applied:

| Violation | Resolution |
|---|---|
| Task 8 called `swap::recovery_path_still_cleanable(fs, …)`, produced by Task 9 | **Task 6 now threads `fs` through ALL of `swap.rs` in one pass.** Tasks 8 and 9 both consume from Task 6; Task 9 changes only bodies. |
| Task 15 called `prompts::perform_save_as` with the 6-arg form, produced by Task 16 | **Task 15 is now a pure `fsx` task** — the primitive only. Wiring it into the Save-As / Write-Block prompts moved to Task 21, which owns that path. |
| Task 23 registered `open_recent` against `recents::open_recent`, produced by Task 24 | **Tasks 23 and 24 swapped.** Recents is built first; the command task then finds its handler. |
| Task 21 used `CommandResult::HandledOpenedSaveAs`, produced by nothing | **Converted to a private `dispatch_save_reporting(ctx) -> bool` core.** `CommandResult` is `Handled \| Noop \| Quit` and is returned by every registry handler; widening a shared enum for one call site's control-flow fact is the wrong blast radius. |

**Round 2 findings, and how each was resolved:**

| Violation | Resolution |
|---|---|
| Task 18's click test called `Editor::open_destination_picker`, and its `Esc` step named `cancel_destination` — both produced by Task 21 | **Task 18 constructs the destination-mode `FileBrowser` literal directly** from its own types (`BrowseMode`, `DestinationPurpose`, `FileEntry` are all Task 18 productions), and its `Esc` simply nulls `file_browser`. Task 21 later replaces that line with `cancel_destination` when it adds the quit-drain abort. Chosen over moving the picker-opener into Task 18, which would drag Save-As wiring into the commit-semantics task, and over deferring the click test to Task 21, which would split the click divergence from the Enter table it is a counterpart to. |
| Task 13 introduced the async `start_listing` but its own tests called Task 12's synchronous `open_file_browser(&dyn Fs, PathBuf)` | **Task 13 widens `open_file_browser` to `(&Arc<dyn Fs + Send + Sync>, &Sender<Msg>, PathBuf)`** and its tests use an `open_and_pump` helper that consumes the `ListingDone`. This is the signature-evolution class the symbol walk cannot see. |

After both fixes the mechanical walk over all fenced blocks — implementation **and** tests — reports
**zero** forward references.

---

# Signature-change ripple map — verified against the real tree

The dependency walk answers *"does this symbol exist yet?"* It cannot see that a symbol's **arity
changed**, because the name still resolves. That blind spot produced defects in rounds 2, 5, and 7 —
including two compile blockers in `registry.rs`, which is the highest-risk file for this class
because it is a table of handlers calling many subsystems, so nearly any signature change ripples
into it.

**Re-run this before any hand-back**, for every symbol whose signature a task changes:

```
cd wordcartel/src
for sym in <changed symbols>; do
  grep -rn "$sym(" *.rs plugin/*.rs | grep -v "fn $sym"
done
```

**Verified output, with the task that owns each update.** Rows marked ⚠ were found by this sweep
after passing an existence-only walk:

| Changed signature | Call sites in the tree | Updated by |
|---|---|---|
| `prompts::open_save_as` → `(editor, fs, msg_tx) -> bool` | `save.rs:180`; ⚠ **`registry.rs:294`** (`save_as` handler) | T21 — **must update both** |
| `export::run_export` → `(editor, fs, ext, msg_tx)` | ⚠ **`registry.rs:337/341/345/349`** (`export_html`/`docx`/`pdf`/`tex`) | T22 — **must update all four** |
| `blocks_marked::block_write` → gains `fs`, `msg_tx` | ⚠ **`registry.rs:428`** (`block_write` handler) | T21 |
| `swap::assess` → gains `fs` | ⚠ **`app.rs:627`** (startup recovery); `swap.rs:292` | T6 |
| `swap::cleanable_recovery_files` → gains `fs` | ⚠ **`prompts.rs:87`** (`open_clean_recovery`) | T6 |
| `prompts::perform_block_write` → gains `fs` | `prompts.rs:158` (retired in T21); ⚠ **`prompts.rs:297`** (`OverwriteWriteBlock` arm) | T21 |
| `Editor::open_file_browser` → `(fs, msg_tx, dir)` | `registry.rs:289`; `editor.rs:919`; ⚠ **10 test sites** across `overlays.rs` (×3), `render.rs` (×3), `mouse.rs` (×2), `app.rs` (×2), `session_restore.rs`, `file_browser.rs` (×2) | T13 |
| `file_browser::rebuild_entries` → **removed** (→ `refetch`/`rederive`) | 7 sites in `file_browser.rs`, 1 in `editor.rs` | T12 |
| `file_browser::file_browser_enter` → `(editor, fs, msg_tx)` | `file_browser.rs:123`; `mouse.rs:517` | T14 |
| `prompts::resolve_prompt` → gains `fs` | `prompts.rs:40`; `mouse.rs:692`; **~15 test sites** in `prompts.rs`/`jobs_apply.rs`/`app.rs` | T8 |
| `jobs_apply::apply_export_done` → gains `fs` | `prompts.rs:50`; `app.rs:320`; 4 test sites | T8 |
| `export::do_export` → gains `fs` | `export.rs:288`; `prompts.rs:286` | T22 |
| `save::do_save_to` → takes `SaveTarget` | `save.rs:174`; `prompts.rs:176` | T16 |
| `config::config_layer_paths` → gains `fs` | `app.rs:470` | T7 |

**`registry.rs` deserves a dedicated pass.** Its handler table calls `open_save_as`, `run_export` ×4,
`block_write`, and `open_file_browser` — six handlers touched by four different tasks. Whichever task
runs last should re-read the whole table rather than trusting that each earlier task found its own row.

**What this map does not cover:** symbols whose signature changes *within* the plan across two tasks
(e.g. `rederive`, reshaped in T18 after T12 created it) — those are caught by reading the producing
task's own call sites, not the tree's. Stated because this is the second enumeration in this document
that had to be corrected for claiming completeness it did not have.


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

5. **Update EVERY `Fs` impl in the tree, not just the two this task is editing.** Adding a trait
   method breaks every implementor at once. Enumerate them mechanically:

```
grep -rn "impl crate::fsx::Fs for\|impl Fs for" wordcartel/src
```

   **Verified: five impls exist** — `RealFs` (`fsx.rs`), `FaultFs` (`test_support.rs` after Task 1),
   and **three separate `impl crate::fsx::Fs for FailFs` blocks in `settings.rs`** (around `:799`,
   `:870`, `:899`), each a local minimal double inside a different test. All three become incomplete
   the moment this method lands, and they are compile blockers for the test suite.

   Give each `FailFs` a delegating body (they only ever exercise the write path, so delegating the
   new readers to `RealFs` preserves their intent):

```rust
            fn read_capped(&self, p: &std::path::Path, l: u64)
                -> std::io::Result<Option<Vec<u8>>> { crate::fsx::RealFs.read_capped(p, l) }
```

   Tasks 3 and 4 add `stat` and `list_dir` and must repeat this for all three. **This is the
   trait-extension analogue of the signature-change ripple map** — when a trait gains a method,
   enumerate implementors by grep rather than by the set of files the task is already touching.

6. **Add the FaultFs arm** in `test_support.rs`: add `ReadCapped` to `enum FaultAt`, and to
   `impl Fs for FaultFs`:

```rust
    fn read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
        if matches!(self.fail, FaultAt::ReadCapped) {
            return Err(Error::other("injected: read_capped"));
        }
        self.inner.read_capped(path, limit)
    }
```

7. **Run — expect green:**

```
cargo test -p wordcartel --lib fsx::tests::read_capped
```

Expected: `test result: ok. 4 passed`.

8. **Commit:** `feat(c5): add Fs::read_capped with over-cap/IO-error separation`

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

5. **Update EVERY `Fs` impl — repeat the sweep.** `grep -rn "impl crate::fsx::Fs for\|impl Fs for" wordcartel/src`
   reports five: `RealFs`, `FaultFs`, and **three `impl crate::fsx::Fs for FailFs` blocks in
   `settings.rs`** (~`:799`, `:870`, `:899`). Adding `stat` makes all three incomplete — a compile
   blocker for the test suite. Give each a delegating body:

```rust
            fn stat(&self, p: &std::path::Path) -> std::io::Result<crate::fsx::FileStat> {
                crate::fsx::RealFs.stat(p)
            }
```

   The sweep is repeated in each trait-extending task rather than noted once in Task 2, because a
   note one task away is a check that does not get run — that is how this was missed the first time.

6. **Add the FaultFs arm** in `test_support.rs`: add `Stat` to `FaultAt`, and:

```rust
    fn stat(&self, path: &Path) -> std::io::Result<crate::fsx::FileStat> {
        if matches!(self.fail, FaultAt::Stat) {
            return Err(Error::other("injected: stat"));
        }
        self.inner.stat(path)
    }
```

7. **Run — expect green:**

```
cargo test -p wordcartel --lib fsx::tests::stat_
```

Expected: `test result: ok. 4 passed`.

8. **Commit:** `feat(c5): add Fs::stat with follow/lstat split and broken-symlink detection`

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

    #[cfg(unix)]
    #[test]
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
```

And the small free helper, next to `RealFs`:

```rust
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
```

5. **Update EVERY `Fs` impl — repeat the sweep.** Same five implementors
   (`grep -rn "impl crate::fsx::Fs for\|impl Fs for" wordcartel/src`): `RealFs`, `FaultFs`, and the
   **three `FailFs` blocks in `settings.rs`**. Adding `list_dir` makes all three incomplete. Give
   each a delegating body:

```rust
            fn list_dir(&self, p: &std::path::Path, cap: Option<usize>)
                -> std::io::Result<crate::fsx::DirListing> { crate::fsx::RealFs.list_dir(p, cap) }
```

6. **Add the FaultFs arm** in `test_support.rs`: add `ListDir` to `FaultAt`, and:

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

7. **Add the cap constant** to `limits.rs`:

```rust
/// Retention cap for ONE interactive directory listing (the picker). Enumeration is never
/// capped — the disclosed total must be real. Non-interactive scans (plugin discovery, the
/// swap state-dir scans) pass `cap: None`.
pub const MAX_DIR_ENTRIES: usize = 5_000;
```

8. **Run — expect green:**

```
cargo test -p wordcartel --lib fsx::tests::list_dir fsx::tests::resolution_stats
```

Expected: `test result: ok` — the four `list_dir_*` cases plus `resolution_stats_only_symlinks_not_every_entry`,
`list_dir_emits_a_named_entry_whose_type_cannot_be_determined`, and
`a_permission_denied_symlink_chain_reports_broken_not_a_listing_failure`.

9. **Commit:** `feat(c5): add Fs::list_dir with EntryKind classification and uncapped enumeration`

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

2. **Run — expect compile failure:**

```
cargo test -p wordcartel --lib registry::tests::ctx_fs_field
```

Expected: ``error[E0560]: struct `Ctx` has no field named `fs```.

3. **Add the field to `Ctx`** in `registry.rs`:

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

4. **Add the field to `DispatchCtx`** in `overlays.rs`:

```rust
    /// The filesystem seam (owned handle — the listing thread clones it in).
    pub(crate) fs: &'a std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
```

5. **Build the `Arc` at the TOP of `app::run` — before config discovery, not near the executor.**
   Position is load-bearing: `config::config_layer_paths`, `config::load`, the startup `is_file`
   probes, `theme_resolve`, `state::load`, and the launch `file::open` all run in the FIRST part of
   `run`, well before the executor and clock are created — and Tasks 6 and 7 migrate every one of
   them onto the seam. An `Arc` created "near the executor" would not exist yet at those call sites.

```rust
pub fn run(cli: Cli) -> std::io::Result<ExitReason> {
    // COMPOSITION ROOT for the filesystem seam — the first statement, before ANY filesystem
    // work. Config discovery, theme resolution, session load, and the launch file open all
    // happen below this line and all take `&*fs` after Tasks 6/7. Everything downstream gets
    // a clone; tests substitute an Arc<FaultFs> here.
    let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
        std::sync::Arc::new(crate::fsx::RealFs);
    // …existing body follows unchanged…
```

   **Startup calls use the `_with_fs` variants, not the `RealFs` wrappers.** The wrappers exist for
   API compatibility with callers that have no `fs` in scope; `run` has one, so it must use it — or
   startup config/theme/session reads stay unreachable by an injected `FaultFs`. Concretely, in
   Tasks 6 and 7, `run`'s own calls become:

```rust
    let paths = config::config_layer_paths(&*fs, &cli, xdg.as_deref(), &anchor);
    let (baseline_cfg, _w) = config::load_with_fs(&*fs, &hand_paths);
    let (cfg, mut warns)   = config::load_with_fs(&*fs, &all_paths);
    let resolved           = theme_resolve::resolve_theme_with_fs(&*fs, &cfg.theme, &env, disp);
    let session            = crate::state::load_in_with_fs(&*fs, &crate::swap::state_dir()?);
```

   and the existing `settings::perform_settings_save(…, &crate::fsx::RealFs)` call in the run loop
   becomes `settings::perform_settings_save(…, &*fs)` — see step 7a.

7a. **Fix the one production site that ALREADY takes the seam but is handed a concrete `Fs`.**
   `app.rs` calls `settings::perform_settings_save(…, &crate::fsx::RealFs)`, and
   `perform_settings_save` already accepts `&dyn crate::fsx::Fs` — it has taken the seam since
   before C5. Passing `RealFs` there keeps settings saves unreachable by an injected `FaultFs`:

```rust
                if let Some(of) = settings::perform_settings_save(
                    &mut e, cli.no_config, overrides_path.as_deref(),
                    &baseline_snapshot, &overrides_snapshot, &mask_snapshot, &*fs)
```

   This is a distinct defect shape from constructing an `Fs`: the call site **passes** a concrete
   one to something that wanted the trait. It is the only such site in the tree — verified by a
   sweep matching both shapes, with test modules stripped by `#[cfg(test)] mod tests` rather than by
   any `#[cfg(test)]` attribute (an attribute-based filter silently skips the rest of a file that
   has test-only imports near the top, which is how this site was missed once).

6. **Thread `fs` down the whole call chain that BUILDS those contexts.** Adding the field is not
   enough: the functions that construct `Ctx`/`DispatchCtx` have no `fs` value in scope until it is
   passed to them, and the compiler reports those as missing-field errors with no obvious source.
   **Enumerate by SEARCH, not by recall.** Three successive hand-written versions of this list
   were incomplete. Run this and thread `fs` into every production site it reports:

```
cd wordcartel/src
for f in *.rs plugin/*.rs; do
  awk -v F="$f" '/^#\[cfg\(test\)\]$/{getline n; if (n ~ /^mod tests/) t=1}
                 !t && /(registry::Ctx *\{|[^a-zA-Z_]Ctx *\{|DispatchCtx *\{)/{print F":"NR}' "$f"
done
```

   Note the `#[cfg(test)] mod tests` pairing — cutting on a bare `#[cfg(test)]` attribute skips
   most of `app.rs`, which is how earlier versions of this list lost sites.

   **Output at the time of writing, MANUALLY FILTERED** — the raw command also reports ~12 hits in
   `e2e.rs`, which is test-only and out of scope. This table is that output minus `e2e.rs`; it is not
   literal command output, and saying so matters because "verified output" that has been quietly
   edited is the same overclaim this plan has been correcting, in miniature:

| Site | Constructs | Note |
|---|---|---|
| `app.rs:197` | `registry::Ctx` | the command-dispatch helper |
| `app.rs:273` | `DispatchCtx` | `reduce_dispatch`'s overlay chain |
| `input.rs:43` | `registry::Ctx` | keymap → command dispatch |
| `jobs_apply.rs:205` | `Ctx` | the post-save drive path |
| `mouse.rs:779` | `DispatchCtx` | the overlay mouse route (covers `mouse_prompt`, `mouse_file_browser`) |
| `prompts.rs:175, 222, 234, 246, 252` | `Ctx` ×5 | `perform_save_as` plus four `resolve_prompt` arms |
| `timers.rs:209` | `Ctx` | tick-driven dispatch |
| `plugin/pump.rs:258` | `registry::Ctx` | `PluginHost::pump` / `drain_one_dispatch`, reached from `app.rs` |

   Each enclosing function gains an `fs` parameter (or, where it already holds a `Ctx`/`DispatchCtx`,
   uses `ctx.fs`), threaded from `app::run`'s composition root.

**What this list does NOT cover:** sites the search cannot see because they are constructed
indirectly, and test modules. It is a starting set produced mechanically, not a proof of
completeness — treat compiler output after step 8 as the final check. Stated because three prior
versions of this list claimed completeness they did not have.

   Everything downstream — `jobs_apply::apply_export_done` and `prompts::resolve_prompt`, which
   Tasks 7 and 8 give an `fs` parameter — is then reachable, because both are called from sites that
   now hold `ctx.fs`.

   **The `Msg`-arm call sites are the ones to check by hand**, since they are the reason the chain
   exists: `reduce_dispatch`'s `Msg::ExportDone` arm and `prompts::intercept`'s `Msg::ExportDone` arm
   both call `apply_export_done`, and both must pass `&*ctx.fs`.

7. **Fix every remaining construction site.** Run `cargo build -p wordcartel` and add
   `fs: std::sync::Arc::clone(&fs)` (for `Ctx`) or `fs: &fs` (for `DispatchCtx`) to each literal the
   compiler still reports — chiefly test modules, where `fs: std::sync::Arc::new(crate::fsx::RealFs)`
   is the right value. The compiler enumerates these reliably **once step 7 has given each enclosing
   function an `fs` to hand over**; without step 7 the same errors appear with no value in scope to
   satisfy them.

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

**Consumes** (Tasks 2, 3, 5) — Task 3 because `open_bounded_with_fs`'s size pre-check calls
`fs.stat(path)`:

```rust
// crate::fsx
pub(crate) trait Fs {
    fn read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>>;
    fn stat(&self, path: &Path) -> std::io::Result<FileStat>;   // Task 3
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

// crate::swap — ALL of swap.rs's seam-threading happens here, in one pass, so Tasks 8 and 9
// each find the signatures already in place rather than needing half of it apiece.
pub fn assess(fs: &dyn crate::fsx::Fs, doc_path: Option<&Path>,
    current_file_bytes: Option<&[u8]>) -> RecoveryDecision;
pub(crate) fn recovery_path_still_cleanable(fs: &dyn crate::fsx::Fs, path: &Path,
    protected: &std::collections::HashSet<std::path::PathBuf>) -> bool;
pub(crate) fn cleanable_recovery_files(fs: &dyn crate::fsx::Fs, dir: &Path,
    protected: &std::collections::HashSet<std::path::PathBuf>) -> Vec<std::path::PathBuf>;
pub fn write_atomic(path: &Path, content: &str) -> std::io::Result<()>;      // thin RealFs wrapper
pub(crate) fn write_atomic_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, content: &str)
    -> std::io::Result<()>;                                                  // used by the worker
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
        // Names the OVER-CAP branch specifically. `|| w.contains("cannot read")` would let a
        // broken read path read as a cap success — the cap could be absent and an unrelated
        // IO failure would satisfy the assertion.
        assert!(warns.iter().any(|w| w.contains("too large")),
            "the warning must name the CAP, not merely any read failure: {warns:?}");
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

**Thread `fs` through EVERY `swap.rs` caller in one pass** — this task owns all of `swap.rs`'s
seam-threading so that Tasks 8 and 9 both find the signatures already in place. Add a leading
`fs: &dyn crate::fsx::Fs` parameter, plus a `RealFs`-injecting public wrapper for each currently-public
entry point, to:

```rust
pub fn assess(fs: &dyn crate::fsx::Fs, doc_path: Option<&Path>, current_file_bytes: Option<&[u8]>)
    -> RecoveryDecision;
fn swap_is_cleanable(fs: &dyn crate::fsx::Fs, candidate: &Path) -> bool;
fn tmp_is_cleanable(fs: &dyn crate::fsx::Fs, dir: &Path, candidate: &Path, fname: &str, me: u32) -> bool;
fn recovery_file_is_cleanable(fs: &dyn crate::fsx::Fs, dir: &Path, path: &Path, fname: &str,
    protected: &std::collections::HashSet<PathBuf>, me: u32) -> bool;
pub(crate) fn recovery_path_still_cleanable(fs: &dyn crate::fsx::Fs, path: &Path,
    protected: &std::collections::HashSet<PathBuf>) -> bool;
pub(crate) fn cleanable_recovery_files(fs: &dyn crate::fsx::Fs, dir: &Path,
    protected: &std::collections::HashSet<PathBuf>) -> Vec<PathBuf>;
fn find_orphan_scratch_swap_in(fs: &dyn crate::fsx::Fs, dir: &Path)
    -> Option<(PathBuf, SwapHeader, String)>;
```

**Also give `write_atomic` a seam-taking core, and use it from the swap worker.** The spec's
ownership table requires `swap::dispatch_swap_write`'s worker to hold an owned `Arc` — it is a
durability write on a background thread, the same class as the save worker — but `write_atomic`
hardcodes `RealFs` internally, so without this the swap write stays unreachable by an injected
`FaultFs`:

```rust
/// Atomic 0600 write into our own state dir. Thin `RealFs` wrapper for callers with no `fs`
/// in scope (notably `recovery::write_dump`, which runs from the panic hook).
pub fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    write_atomic_with_fs(&crate::fsx::RealFs, path, content)
}

pub(crate) fn write_atomic_with_fs(fs: &dyn crate::fsx::Fs, path: &Path, content: &str)
    -> io::Result<()>
{
    crate::fsx::atomic_replace(fs, path, content.as_bytes(), crate::fsx::WriteOpts {
        mode: crate::fsx::ModePolicy::Fixed(0o600), dir_fsync: false,
    })
}
```

and in `dispatch_swap_write`, clone the handle into the job closure exactly as the save worker does
(`dispatch_swap_write` takes `ctx: &mut Ctx`, so `ctx.fs` is in scope after Task 5):

```rust
    let fs = std::sync::Arc::clone(&ctx.fs);   // owned — the closure is 'static + Send
    ctx.executor.dispatch(Job {
        // …
        run: Box::new(move || {
            let body = snap.to_string();
            let mut h = header;
            h.content_hash = fnv1a64(body.as_bytes());
            let ok = write_atomic_with_fs(&*fs, &path, &serialize(&h, &body)).is_ok();
            // …merge unchanged, INCLUDING the path-aware latch…
        }),
    });
```

**The path-aware latch and its regression test are untouched.** Only the write call changes.

Add a test mirroring the save-worker one: dispatch a swap with an `Arc<FaultFs>` injected at `Ctx`
and assert `swapped_version` is NOT latched (the write failed), which fails if the worker was left
calling the `RealFs` wrapper. **FAIL-VERIFY:** put `write_atomic` back in the closure, watch it fail,
revert.

**Update the two production callers whose arity changes** (both found by the ripple sweep, both
missed by an existence-only walk):

* `app.rs`'s startup recovery — `crate::swap::assess(&*fs, editor.active().document.path.as_deref(), file_bytes.as_deref())`
* `prompts::open_clean_recovery` — `crate::swap::cleanable_recovery_files(fs, &dir, &crate::swap::open_swap_paths(editor))`,
  which means `open_clean_recovery` itself gains an `fs` parameter, passed from its `Ctx`-holding
  caller in `registry.rs`.

There are **no other behavioural changes** here — the reads inside these functions move onto the
seam. Task 9 later swaps their `read_dir` loops for `list_dir`; Task 8 later swaps the delete calls.
Doing the signature threading once, here, is what keeps those two tasks from each needing half of it.

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
        // MUST create a real fifo. Without one, `is_file_via` implemented as `!st.is_dir` —
        // the exact defect this test's own comment warns about — passes every assertion
        // (file→true, dir→false, missing→Err→false). The fifo is the only case that separates
        // the two implementations.
        //
        // FAIL-VERIFY: implement `is_file_via` as `matches!(fs.stat(p), Ok(st) if !st.is_dir)`,
        // watch the fifo assertion fail.
        let d = unique_dir("isfile-fifo");
        let f = d.join("plain.txt");
        std::fs::write(&f, b"x").expect("seed");
        assert!(is_file_via(&RealFs, &f), "regular file");
        assert!(!is_file_via(&RealFs, &d), "a directory is not a file");
        assert!(!is_file_via(&RealFs, &d.join("absent")), "a missing path is false, not an error");
        #[cfg(unix)]
        {
            let fifo = d.join("pipe");
            let cs = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes())
                .expect("path has no interior NUL");
            // mkfifo via libc is not available here; use the `mkfifo` binary, skipping the
            // assertion if it is absent so the gate machine never fails on a missing tool.
            // MANDATORY, not best-effort. An earlier version skipped this when the `mkfifo`
            // binary was absent — which made the guard vacuous on exactly the machines
            // lacking it, since every other assertion here passes under an `!is_dir`
            // implementation.
            //
            // `libc` is already in the lock (transitively, via notify/crossterm). If it is not
            // a DIRECT dependency, add it under `[dev-dependencies]` — a dev-only dep does not
            // touch decision 2, which governs runtime dependencies.
            let rc = unsafe { libc::mkfifo(cs.as_ptr(), 0o644) };
            assert_eq!(rc, 0, "mkfifo must succeed — this IS the guard, not an optional extra");
            assert!(!is_file_via(&RealFs, &fifo),
                "a FIFO is NOT a regular file — the one assertion an `!is_dir` implementation \
                 fails, and the reason this test exists");
        }
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
    // SAME guard as `save::fingerprint`, and for the same reason: today's
    // `std::fs::metadata(path).ok()?` FAILS for a broken symlink, so this returns None.
    // The seam's `stat` SUCCEEDS for one, so without this the session-restore staleness
    // check would receive a (mtime = 0, len = 0) identity — which matches nothing and
    // silently discards resume state, or worse matches a genuinely empty file.
    if st.broken { return None; }
    let mtime = st.mtime
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some((mtime, st.len))
}
```

   **The `broken` guard is a CLASS, not a one-off.** Every `stat` caller that previously used
   `metadata(...)` must decide what a broken symlink means, because `metadata` failed for one and
   `stat` does not. Audited, all callers in this plan:

| Caller | Broken-symlink handling |
|---|---|
| `save::fingerprint_with_limit` | explicit `if st.broken { return None; }` — matches `metadata().ok()?` |
| `state::file_identity_with_fs` | explicit `if st.broken { return None; }` — **this step** |
| `fsx::exists_via` | `Ok(st) if !st.broken` — `Path::exists()` follows, so a broken link is `false` |
| `fsx::is_file_via` | `Ok(st) if st.is_file` — broken implies `!is_file`, so `false` |
| `file::open_bounded_with_fs` size pre-check | `Ok(st) if st.is_file && …` — broken implies `!is_file`, so the pre-check is skipped and the read fails, exactly as before |
| `file::open_bounded_with_fs` `is_dir` check | `Ok(st) if st.is_dir` — broken implies `!is_dir` |
| `save_atomic_with_fs` / `save_atomic_bytes_with_fs` / `append_word_to_dict_with_fs` | `Ok(st) if st.is_symlink` — broken implies `is_symlink`, so refused; correct for a last-resort write guard |
| `fsx::resolve_write_destination` | `Ok(st) if st.broken => Err(DestError::BrokenSymlink)` |
| `file_browser_commit::classify_destination_enter` row 3 | `Ok(st) if st.is_dir` — broken implies `!is_dir` |

   Only the first two need an explicit early return; the rest are correct because `broken` implies
   `!is_file && !is_dir && is_symlink`. Add this test alongside the fingerprint one:

```rust
    #[cfg(unix)]
    #[test]
    fn file_identity_on_a_broken_symlink_is_none() {
        // Without the guard this returns Some((0, 0)) — an identity that silently discards
        // resume state, or matches a genuinely empty file.
        //
        // FAIL-VERIFY: delete the `if st.broken` line, watch this fail, then revert.
        let d = std::env::temp_dir().join(format!("wc-fid-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        let link = d.join("dangling.md");
        std::os::unix::fs::symlink(d.join("gone.md"), &link).expect("symlink");
        assert!(file_identity_with_fs(&crate::fsx::RealFs, &link).is_none(),
            "a broken symlink must yield None, exactly as metadata().ok()? did");
        let _ = std::fs::remove_dir_all(&d);
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

7. **Migrate the remaining probes.** Each is a one-line substitution.

   **`clipboard::clip_env_from_process` is NOT migrated — spec clause (g).** Its
   `dir.join(bin).is_file()` sweep asks "is this helper binary installed on `$PATH`", which is
   executable-availability detection rather than document access — the cheap form of what
   `export::probe_pandoc` does by spawning. It is also structurally unmigratable without redesigning
   an unrelated API: `ClipEnv::present` is a bare `fn(&str) -> bool` field and `on_path` is a nested
   `fn`, neither of which can capture an injected `fs`; boxing the closure would ripple through every
   `ClipEnv` literal (~10 in tests). The exemption is on the first ground; the second is why the
   alternative would have been a bad trade rather than merely more work.

| Site | Was | Becomes |
|---|---|---|
| `config::config_layer_paths` (three `p.is_file()`) | `if p.is_file()` | `if crate::fsx::is_file_via(fs, &p)` — add a leading `fs: &dyn crate::fsx::Fs` parameter and a `RealFs` wrapper |
| `app::run` (`p.is_file()`, `c.is_file()` ×2, `!p.exists()`) | as written | `crate::fsx::is_file_via(&*fs, p)` / `!crate::fsx::exists_via(&*fs, p)` — `run` holds the `Arc` from Task 5 |

**DEFERRED — four sites whose enclosing function has no `fs` until a later task.** Migrating them
here would not compile, because `exists_via` needs a seam the function does not yet receive:

| Site | Gains `fs` in | Migrated by |
|---|---|---|
| `prompts::save_as_submit` / `block_write_submit` | — | **retired entirely in Task 21**; the picker's commit arm does the existence check |
| `export::run_export` | Task 22 | Task 22, which adds the parameter |
| `export::run_pandoc` | Task 22 | Task 22 (owned `Arc` cloned into `do_export`'s thread) |
| `jobs_apply::apply_export_done` | Task 8 | Task 8, which adds the parameter |

This is the ordering rule the dependency walk enforces, applied to *parameters* rather than symbols:
a task cannot use `fs` inside a function that does not receive one yet.
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

        // ATOMICITY, actually observed. The assertion above passes identically under the OLD
        // non-atomic `OpenOptions::append` + `writeln!` — appending twice produces the same
        // bytes either way. What separates them is a FAILED write: the atomic form leaves the
        // previous contents intact, the append form leaves a torn file.
        //
        // FAIL-VERIFY: restore the append implementation, watch this fail with "alpha\nbeta\ngam".
        let ff = crate::test_support::FaultFs::new(
            crate::test_support::FaultAt::Write { after: 3 });
        let err = append_word_to_dict_with_fs(&ff, &p, "gamma")
            .expect_err("an injected mid-write failure must surface");
        let _ = err;
        assert_eq!(std::fs::read_to_string(&p).expect("read back"), "alpha\nbeta\n",
            "a FAILED append leaves the dictionary exactly as it was — no torn line. This is \
             the property `atomic_replace` buys and the old append could not.");
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

8. **Update every call site of the two signatures this task changes** (ripple map rows):

   * `jobs_apply::apply_export_done` gains `fs` → callers `app.rs:320` (`reduce_dispatch`'s
     `Msg::ExportDone` arm) and `prompts.rs:50` (`intercept`'s `Msg::ExportDone` arm) both pass
     `&*ctx.fs`; four test call sites in `jobs_apply.rs` pass `&crate::fsx::RealFs`.
   * `prompts::resolve_prompt` gains `fs` → callers `prompts.rs:40` (`intercept`) and `mouse.rs:692`
     (`mouse_prompt`) pass `&*ctx.fs`; ~15 test call sites across `prompts.rs`, `jobs_apply.rs`, and
     `app.rs` pass `&crate::fsx::RealFs`.

9. **Run — expect green:**

```
cargo test -p wordcartel --lib diagnostics_run:: file:: prompts:: swap:: jobs_apply:: mouse::
```

Expected: all pass. **These four must still pass unmodified:**
`diagnostics_run::tests::append_word_to_dict_creates_parent_dir`,
`prompts::tests::recover_loads_body_and_deletes_orphan_swap_file`,
`jobs_apply::tests::apply_export_done_rename_failure_is_a_sticky_error`,
`jobs_apply::tests::apply_export_done_toctou_target_appeared_is_a_sticky_warning`.

10. **Commit:** `refactor(c5): route durable mutations through the seam; make the dict append atomic`

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

// crate::swap — these signatures ALREADY take `fs` (Task 6 threaded all of swap.rs). This
// task changes only their BODIES: the `read_dir` loops become `list_dir` calls.
pub(crate) fn cleanable_recovery_files(fs: &dyn crate::fsx::Fs, dir: &Path,
    protected: &HashSet<PathBuf>) -> Vec<PathBuf>;              // body changes, signature does not
fn find_orphan_scratch_swap_in(fs: &dyn crate::fsx::Fs, dir: &Path)
    -> Option<(PathBuf, SwapHeader, String)>;                   // body changes, signature does not
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
pub(crate) struct DirEntryInfo { pub name: String, pub raw_name: std::ffi::OsString,
                                 pub kind: EntryKind, pub is_symlink: bool, pub broken: bool }
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
                    // The UTF-8 decision is made on the RAW name. `e.name` is already
                    // lossy-converted, so `to_str()` on a path built from it would always
                    // succeed and the invalid-UTF-8 branch would be unreachable — the plugin
                    // would be loaded under a mangled stem instead of reported (spec §5.2
                    // rider 3 requires it be reported, by name).
                    let raw = std::path::Path::new(&e.raw_name);
                    if raw.extension().and_then(|x| x.to_str()) == Some("lua") {
                        match raw.file_stem().and_then(|s| s.to_str()) {
                            Some(stem) => candidates.push((stem.to_string(), path)),
                            None => unloadable.push(LoadReport {
                                // Reported with the LOSSY name — it is all we can display,
                                // and "named, never silently dropped" is satisfied.
                                plugin: e.name.clone(), hooks: 0,
                                result: Err("plugin name is not valid UTF-8".into()),
                            }),
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

/// Modules that are WHOLLY exempt, by a clause covering every raw call in the file.
///
/// EXACTLY ONE ENTRY, and that is the honest number.
///
/// `fsx.rs` IS the seam (clause (d)) — every raw call in it is the implementation the rule
/// is defined in terms of, so a per-hit marker on each would be noise.
///
/// Deliberately NOT listed, though earlier drafts listed them:
///   * `filter.rs` and `harper_ls.rs` — verified to contain ZERO raw filesystem calls. Their
///     clause-(a) exemption covers what the CHILD PROCESS does, which this scanner cannot
///     see and does not attempt to. Listing them would imply they hold exempt raw calls.
///   * `recovery.rs` — verified to contain zero raw filesystem calls in production; the panic
///     dump goes through `swap::write_atomic` -> `fsx::atomic_replace`. It was listed as
///     "(d)-adjacent", which conflated two different things: it IS an ownership exception
///     (it cannot take an injected `Arc`, see §5.2's ownership table) but it is NOT a
///     chokepoint exception, because it never bypasses the seam.
///   * `swap.rs`, `settings.rs`, `diagnostics_run.rs`, `session_restore.rs`, `export.rs` —
///     these DO hold exempt raw calls, but only a few each, so they use per-hit markers. A
///     whole-file entry would let a new in-scope call inherit the exemption silently.
const EXEMPT_MODULES: &[(&str, &str)] = &[
    ("src/fsx.rs", "(d) the seam's own implementation"),
];

/// Per-hit exemption marker, placed on the offending line or the line directly above it:
///
/// ```ignore
/// // fs-chokepoint-allow: (b) directory provisioning for the state dir
/// std::fs::create_dir_all(&dir)?;
/// ```
///
/// WHY PER-HIT RATHER THAN PER-FILE. An earlier version of this test allow-listed whole
/// FILES, which meant a NEW in-scope raw call added to `swap.rs` or `export.rs` passed
/// silently — while the task claimed "a new raw call fails the build until routed or
/// allow-listed". Those cannot both be true, and the claim was the false one. A marker has
/// to be written deliberately, sits where the reader is, and names the clause it claims; a
/// new call has no marker and therefore fails.
const ALLOW_MARKER: &str = "fs-chokepoint-allow:";

/// A marker must name a clause — `(a)` through `(g)`. A bare marker is not an exemption,
/// it is an unexplained silence.
fn marker_names_a_clause(line: &str) -> bool {
    // Requires EXACTLY `(x)` where x is one clause letter — not merely "starts with `(` and
    // the next char is in a..=g", which would accept `(gibberish`. The mechanism must
    // validate what its name asserts.
    line.split_once(ALLOW_MARKER)
        .map(|(_, rest)| {
            let r = rest.trim_start().as_bytes();
            r.len() >= 3 && r[0] == b'(' && r[2] == b')' && (b'a'..=b'g').contains(&r[1])
        })
        .unwrap_or(false)
}

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

/// Drop the trailing `#[cfg(test)] mod tests { … }` block, and NOTHING else.
///
/// CRITICAL: strip on the module-level `#[cfg(test)]` + `mod tests` PAIR, never on a bare
/// `#[cfg(test)]` attribute. `app.rs` carries test-only `use` declarations under
/// `#[cfg(test)]` at lines 10/14/16 — an attribute-only cut discards ~99% of production
/// `app.rs`, INCLUDING the `settings::perform_settings_save` call site that is the one real
/// seam bypass in the tree. A scanner that cannot see `app.rs` cannot enforce anything about
/// the largest hub in the crate, while reporting clean.
///
/// This exact bug occurred twice during authoring — once in the sweep script used to audit
/// the tree, once here — so it is guarded by a planted sample below, not just a comment.
fn strip_test_modules(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim_start() == "#[cfg(test)]" {
            // Look ahead: only a `mod tests`-shaped item ends production code.
            if lines.peek().is_some_and(|n| n.trim_start().starts_with("mod tests")) {
                break;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn offenders_in(src: &str) -> Vec<String> {
    let prod = strip_test_modules(src);
    let lines: Vec<&str> = prod.lines().collect();
    let mut out = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("//") { continue; }
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
        let Some(what) = hit else { continue };
        // PER-HIT exemption: a clause-naming marker on this line or the one above.
        let marked_here = marker_names_a_clause(line);
        let marked_above = n > 0 && marker_names_a_clause(lines[n - 1]);
        if marked_here || marked_above { continue; }
        out.push(format!("  line {}: {what} — {}", n + 1, t));
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
        if EXEMPT_MODULES.iter().any(|(a, _)| *a == name) { continue; }
        let src = std::fs::read_to_string(&f).expect("read source");
        let hits = offenders_in(&src);
        if !hits.is_empty() {
            failures.push(format!("{name}:\n{}", hits.join("\n")));
        }
    }

    assert!(failures.is_empty(),
        "raw filesystem access with no exemption marker.\n\n{}\n\n\
         Route each through `fsx::Fs` (spec §5.2), or — if it falls under a §2.3 exemption \
         clause — put a marker on the line or directly above it naming that clause:\n\
         \x20   // fs-chokepoint-allow: (b) directory provisioning for the state dir\n\n\
         Whole-file exemption (EXEMPT_MODULES) is for files where EVERY raw call shares one \
         clause; it is not a way to silence a single new call.",
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
    // FAIL-VERIFY: drop the import-gate layer (or the UFCS pattern), watch the corresponding
    // row fail, then revert. A scanner that silently matches nothing passes every other test.
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
fn scanner_sees_production_code_below_an_early_cfg_test_attribute() {
    // THE ROUTE THAT ACTUALLY OCCURRED — twice, in two different tools.
    //
    // `app.rs` has test-only `use` declarations under `#[cfg(test)]` near the TOP of the
    // file. A scanner that cuts at the first `#[cfg(test)]` attribute discards essentially
    // all of production `app.rs` — including the one real seam bypass in the tree — and
    // reports clean.
    //
    // BIDIRECTIONAL BY CONSTRUCTION. The two planted calls are detectable by DIFFERENT
    // routes, so each direction of regression breaks a different assertion:
    //   * production `std::fs::read` — only reachable if stripping does NOT cut early;
    //   * in-test `p.symlink_metadata()` — an INHERENT call with no `use std::fs` anywhere
    //     in that block, so its only detection route is the scanner failing to strip at all.
    //
    // FAIL-VERIFY, BOTH DIRECTIONS:
    //   1. Regress `strip_test_modules` to cut at the first `#[cfg(test)]` → the first
    //      assertion fails (production code never scanned).
    //   2. Regress it to strip nothing → the third assertion fails (the in-test call leaks).
    let src = "\
#[cfg(test)]\n\
use std::collections::BTreeMap;\n\
\n\
fn production_code_far_below(p: &std::path::Path) {\n\
    let _ = std::fs::read(p);\n\
}\n\
\n\
#[cfg(test)]\n\
mod tests {\n\
    fn helper(p: &std::path::Path) { let _ = p.symlink_metadata(); }\n\
}\n";
    let hits = offenders_in(src);
    assert!(hits.iter().any(|h| h.contains("std::fs::")),
        "the scanner stopped at the test-only import and never reached production code — \
         this is the defect that hid `app.rs`'s seam bypass:\n{hits:?}");
    assert_eq!(hits.len(), 1,
        "exactly one offender: the production call, nothing from `mod tests`: {hits:?}");
    assert!(!hits.iter().any(|h| h.contains("symlink_metadata")),
        "the `mod tests` body must still be stripped — if this fires, stripping regressed to \
         doing nothing and every test helper now counts as production: {hits:?}");
}

#[test]
fn a_per_hit_marker_exempts_only_its_own_line() {
    // The per-file allow-list this replaced let a NEW raw call inherit an existing file's
    // exemption and pass silently. A marker exempts one call and nothing else.
    //
    // FAIL-VERIFY: make the marker check file-wide (or drop it), watch the second
    // assertion fail.
    let src = "\
fn provision(d: &std::path::Path) {\n\
    // fs-chokepoint-allow: (b) directory provisioning for the state dir\n\
    let _ = std::fs::create_dir_all(d);\n\
    let _ = std::fs::read(d);\n\
}\n";
    let hits = offenders_in(src);
    assert!(!hits.iter().any(|h| h.contains("create_dir_all")),
        "the marked line is exempt: {hits:?}");
    assert!(hits.iter().any(|h| h.contains("std::fs::read")),
        "the UNMARKED call on the next line must still fail — an exemption is per-hit, not \
         per-file, and this is exactly what the old whole-file list got wrong: {hits:?}");
}

#[test]
fn a_marker_without_a_clause_is_not_an_exemption() {
    // A bare marker is an unexplained silence. Every exemption names the clause it claims.
    for bad in ["trust me", "(gibberish", "(z) not a clause", "()", "(a"] {
        let src = format!(
            "fn sneaky(p: &std::path::Path) {{\n    // fs-chokepoint-allow: {bad}\n    \
             let _ = std::fs::read(p);\n}}\n");
        assert!(!offenders_in(&src).is_empty(),
            "marker {bad:?} names no valid (a)-(g) clause and must NOT silence the hit");
    }
    // …and a well-formed one does.
    let ok = "fn f(p: &std::path::Path) {\n    // fs-chokepoint-allow: (c) canonicalize\n    \
              let _ = std::fs::read(p);\n}\n";
    assert!(offenders_in(ok).is_empty(), "a clause-naming marker exempts its line");
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
   - Put a marker on the offending line, or directly above it, **naming the §2.3 clause it
     claims**:

     ```rust
     // fs-chokepoint-allow: (b) directory provisioning for the state dir
     std::fs::create_dir_all(&dir)?;
     ```

     A marker that names no clause is rejected by `marker_names_a_clause` — an unexplained
     silence is not an exemption. If no clause fits, the call is in scope and must be migrated.

     **Do NOT reach for a whole-file entry.** `EXEMPT_MODULES` is for files where EVERY raw call
     shares one clause (`fsx.rs` alone qualifies today); using it to silence a single new call
     re-creates the per-file hollowing-out the marker scheme replaced.

4. **Run — expect green:**

```
cargo test -p wordcartel --test fs_chokepoint
```

Expected: `test result: ok. 6 passed` — the production scan plus the five self-checks
(`scanner_detects_every_evasion_route`,
`scanner_sees_production_code_below_an_early_cfg_test_attribute`,
`a_per_hit_marker_exempts_only_its_own_line`, `a_marker_without_a_clause_is_not_an_exemption`,
`scanner_ignores_ordinary_code_and_test_modules`).

5. **Commit:** `test(c5): add the fs_chokepoint guard with per-hit markers and self-checks`

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
- Modify: `wordcartel/src/test_support.rs` (the shared keystroke helpers, below)

#### Shared test helpers — defined HERE, in `test_support`, reused by Tasks 13–26

Every test claiming a keystroke reaches something must drive the real entry point; a direct
call to the handler is the vacuous-guard pattern this plan has had to correct repeatedly.
These live in `test_support` rather than in one task's `mod tests` for two reasons: helpers
inside a `#[cfg(test)] mod tests` are not reachable from another module's tests at all, and
`test_support` is already the crate's sanctioned home for exactly this (`press`, `key_char`,
`install_enabled_harper`). They are `pub(crate)` and `#[cfg(test)]`-gated with the file.

**They route through `crate::file_browser::intercept`, the intercept's home as of this task.
Task 18 moves the intercept into `file_browser_intercept.rs` and updates this ONE call path** —
noted there as an explicit step, since every keystroke test in the effort depends on it.

```rust
    /// Drive ANY key through the real intercept, exactly as `reduce` would. `press_char_fb`
    /// and `press_enter_fb` are thin wrappers; tests needing Tab or Esc use this directly.
    pub(crate) fn press_key_fb(e: &mut crate::editor::Editor,
        fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
        tx: &std::sync::mpsc::Sender<crate::app::Msg>, code: crossterm::event::KeyCode)
    {
        use crossterm::event::{Event, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let ctx = crate::overlays::DispatchCtx {
            reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: tx, fs };
        let ev = Event::Key(KeyEvent { code, modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // Task 18 repoints this to `crate::file_browser_intercept::intercept`.
        let _ = crate::file_browser::intercept(crate::app::Msg::Input(ev), e, &ctx);
    }

    /// One printable character through the intercept.
    pub(crate) fn press_char_fb(e: &mut crate::editor::Editor,
        fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
        tx: &std::sync::mpsc::Sender<crate::app::Msg>, c: char)
    { press_key_fb(e, fs, tx, crossterm::event::KeyCode::Char(c)); }

    /// Enter through the intercept.
    pub(crate) fn press_enter_fb(e: &mut crate::editor::Editor,
        fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
        tx: &std::sync::mpsc::Sender<crate::app::Msg>)
    { press_key_fb(e, fs, tx, crossterm::event::KeyCode::Enter); }

    /// True when the process can read a mode-000 directory (root / CAP_DAC_OVERRIDE), which
    /// voids the premise of any chmod-based unreadability test. Tests that would otherwise
    /// assert a false negative skip loudly on this rather than passing vacuously.
    pub(crate) fn nix_privileged() -> bool {
        let d = std::env::temp_dir().join(format!("wc-priv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        if std::fs::create_dir_all(&d).is_err() { return false; }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o000));
            let readable = std::fs::read_dir(&d).is_ok();
            let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755));
            let _ = std::fs::remove_dir_all(&d);
            return readable;
        }
        #[allow(unreachable_code)] { let _ = std::fs::remove_dir_all(&d); false }
    }
```

#### Interfaces

**Consumes** (Tasks 4, 9):

```rust
// crate::fsx
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind { File, Dir, Other, Unknown }

#[derive(Clone, Debug)]
pub(crate) struct DirEntryInfo {
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
        DirEntryInfo { name: name.into(), raw_name: name.into(), kind,
                       is_symlink: false, broken: false }
    }
    fn broken(name: &str) -> DirEntryInfo {
        DirEntryInfo { name: name.into(), raw_name: name.into(),
                       kind: EntryKind::Unknown, is_symlink: true, broken: true }
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

        // Keystrokes go through the REAL intercept — mutating `fb.query` and calling
        // `rederive` by hand would prove nothing about the path a writer's typing takes, and
        // would still pass if the intercept re-fetched on every character.
        e.file_browser = Some(fb);
        for c in ['a', 'l', 'p'] { press_char_fb(&mut e, &fs_arc, &tx, c); }
        assert_eq!(fs.calls.load(Ordering::Relaxed), 1,
            "THREE keystrokes through the intercept performed ZERO additional list_dir calls");
        assert!(e.file_browser.as_ref().unwrap().entries.iter().any(|x| x.name == "alpha.md"),
            "and the filter still works");
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
        // FAIL-VERIFY: move the epoch onto FileBrowser, watch this fail, then revert.
        //
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
        let _rx_a = open_and_pump(&mut e, dir_a.clone());
        let stale_epoch = e.file_browser.as_ref().expect("open").awaiting_epoch;
        e.file_browser = None;

        // Picker #2 over dir_b.
        let _rx_b = open_and_pump(&mut e, dir_b.clone());
        let fresh_epoch = e.file_browser.as_ref().expect("reopen").awaiting_epoch;
        assert_ne!(stale_epoch, fresh_epoch, "a global epoch never reissues across close/reopen");

        // Picker #1's listing finally lands.
        let stale = crate::fsx::DirListing {
            entries: vec![crate::fsx::DirEntryInfo {
                name: "from_a.md".into(), raw_name: "from_a.md".into(),
                kind: crate::fsx::EntryKind::File, is_symlink: false, broken: false }],
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
    #[cfg(unix)]   // chmod-based unreadability is meaningless off Unix
    fn a_failed_descend_leaves_the_writer_exactly_where_they_were() {
        // `chmod 000` does not restrict root or CAP_DAC_OVERRIDE. If the listing SUCCEEDS the
        // premise is void — skip rather than assert an inverted result, because a test that
        // passes for the wrong reason is worse than one that opts out loudly.
        if nix_privileged() {
            eprintln!("skip: privileged process — chmod 000 does not restrict this test");
            return;
        }
        // The hold-pending guarantee. `fb.dir` does not move on Enter — it moves only when a
        // listing for the target SUCCEEDS. So an unreadable target costs the writer nothing:
        // not their directory, not their query, not their selection.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let dir = std::env::temp_dir().join(format!("wc-faildescend-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("keep.md"), b"x").expect("seed");
        let _rx = open_and_pump(&mut e, dir.clone());

        // Drive a REAL descend. Hand-stamping `awaiting_epoch`/`pending_dir` and calling
        // `apply_listing_done` would test only the error arm — if `file_browser_enter`'s
        // Descend arm moved `fb.dir`/`query` eagerly (the guarantee being claimed), that
        // version passes.
        //
        // FAIL-VERIFY: make the Descend arm set `fb.dir = target` and clear `fb.query` before
        // spawning, watch this fail on both the dir and the query assertion.
        let bad = dir.join("unreadable");
        std::fs::create_dir_all(&bad).expect("dir");
        #[cfg(unix)]
        std::fs::set_permissions(&bad, std::os::unix::fs::PermissionsExt::from_mode(0o000))
            .expect("chmod 000 so the listing fails");
        e.file_browser.as_mut().expect("open").query.push_str("ke");
        // Select the unreadable directory and press Enter through the real intercept.
        {
            let fb = e.file_browser.as_mut().expect("open");
            fb.entries = vec![crate::file_browser::FileEntry {
                name: "unreadable".into(), kind: crate::fsx::EntryKind::Dir,
                is_symlink: false, broken: false }];
            fb.selected = 0;
        }
        press_enter_fb(&mut e, &fs, &tx);
        // The listing fails on its thread; pump the result the run loop would deliver.
        pump_listing(&mut e, &rx);

        #[cfg(unix)]
        std::fs::set_permissions(&bad, std::os::unix::fs::PermissionsExt::from_mode(0o755)).ok();

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

**`Msg` has a hand-written exhaustive `Debug` impl** (`impl std::fmt::Debug for Msg` in `app.rs`,
matching every variant), so a new variant makes that match non-exhaustive. Add its arm:

```rust
            Msg::ListingDone { epoch, dir, .. } =>
                write!(f, "ListingDone(epoch={epoch}, dir={})", dir.display()),
```

Verified there is exactly one hand-written impl on `Msg` — `Debug`. No `Clone`/`PartialEq` to
update.

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
and `None` at every construction site.

**`Editor::open_file_browser` widens to the async shape in THIS task.** Task 12 introduced it as
`open_file_browser(&mut self, fs: &dyn Fs, dir: PathBuf)` calling the synchronous `refetch`. That
signature cannot spawn a listing — a thread needs an owned handle and a sender — so it becomes:

```rust
    pub fn open_file_browser(&mut self,
        fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
        msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
        dir: std::path::PathBuf)
    {
        crate::overlays::close_all(self);
        self.pending_keys.clear(); self.pending_mark = None;
        let mut fb = crate::file_browser::FileBrowser {
            // `fb.dir` is set immediately so the painter has a title; `start_listing` also
            // records it as the pending target, so the commit is a no-op for the initial
            // open. ONE spawn path, not two.
            dir: dir.clone(), query: String::new(),
            listing: Vec::new(), total_seen: 0, unreadable: 0, entries: Vec::new(),
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        };
        crate::file_browser::start_listing(&mut fb, dir, fs, msg_tx);
        self.file_browser = Some(fb);
    }
```

**This signature change ripples into ~10 existing call sites**, most of them tests:
`registry.rs:289`, `editor.rs:919`, three in `overlays.rs`, three in `render.rs`, two in `mouse.rs`,
two in `app.rs`, one in `session_restore.rs`, two in `file_browser.rs`. They are compile blockers for
the test suite, not just the lib. Work from the ripple map's row rather than from a single grep.

**After this task NO synchronous picker-listing path survives.** `file_browser_listing::refetch`
becomes unreachable from any picker-opening path; keep it only if a non-picker caller needs it, and
delete it otherwise. Update the `"open"` command in `registry.rs` to pass `&c.fs` and `&c.msg_tx`.

**Every test in this task drives the async path.** They construct the `Arc` and channel, call the
widened `open_file_browser`, and then **pump the `ListingDone`** to observe entries — a test that
opens a picker and immediately asserts on `fb.entries` will see an empty list, because the listing
has not landed yet. Use the same `pump_listing` helper this task adds:

```rust
    fn open_and_pump(e: &mut crate::editor::Editor, dir: std::path::PathBuf)
        -> std::sync::mpsc::Receiver<crate::app::Msg>
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_file_browser(&fs, &tx, dir);
        pump_listing(e, &rx);
        rx
    }
```

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
        rx: &std::sync::mpsc::Receiver<crate::app::Msg>) -> bool
    {
        // Fully qualified: `file_browser.rs` imports `crate::app::Msg` only inside individual
        // tests, not at module scope, so a bare `Msg` here would not resolve.
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(crate::app::Msg::ListingDone { epoch, dir, result }) => {
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

**Scope note:** this task adds the resolution primitive and nothing else. Wiring it into the
Save-As / Write-Block prompts belongs to **Task 21**, which owns that path — an earlier draft of
this task called `perform_save_as` with a signature Task 16 had not yet created, which would not
compile in order.

#### Files

- Modify: `wordcartel/src/fsx.rs` (`resolve_write_destination`, `DestError`)

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

4. **Run — expect green:**

```
cargo test -p wordcartel --lib fsx::tests::resolve_write_destination file::tests::save_through_symlink
```

Expected: the three new tests pass, and **`file::tests::save_through_symlink_refused` passes
unmodified** — proving resolution happens before the guard rather than by weakening it.

5. **Commit:** `feat(c5): resolve write destinations through symlinks before the atomic guard`

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
pub(crate) fn perform_save_as(editor: &mut Editor, chosen: std::path::PathBuf,
    resolved: std::path::PathBuf, executor: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>);
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
    fn an_injected_fault_reaches_the_save_worker() {
        // The seam extension's headline payoff, asserted. The worker-side write is
        // fault-testable for the FIRST time — it previously hardcoded RealFs inside the job
        // closure. This FAILS if an implementer calls `file::save_atomic` (the RealFs
        // wrapper) instead of `save_atomic_with_fs` with the Arc cloned from Ctx.fs.
        //
        // FAIL-VERIFY: call `file::save_atomic` here, watch this fail (the file is written
        // for real and the buffer goes clean), then revert.
        let p = scratch();
        std::fs::write(&p, b"old\n").expect("seed");
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None;
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        {
            let mut ctx = Ctx {
                editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx(),
                fs: std::sync::Arc::new(
                    crate::test_support::FaultFs::new(crate::test_support::FaultAt::Rename)),
            };
            do_save_to(&mut ctx, SaveTarget::same(p.clone()), SaveMode::Normal);
        }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(e.active().document.dirty(),
            "an injected write failure must leave the buffer dirty — if this passes as clean, \
             the worker bypassed the seam and wrote for real");
        assert_eq!(std::fs::read_to_string(&p).expect("read"), "old\n",
            "and the file on disk is untouched");
        let _ = std::fs::remove_file(&p);
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
    // OWNED handle cloned into the job closure. `jobs::Job::run` is
    // `Box<dyn FnOnce() -> JobResult + Send>`, so a borrowed `&dyn Fs` cannot cross —
    // which is exactly why Task 5 put an `Arc` on `Ctx`.
    let fs = std::sync::Arc::clone(&ctx.fs);
    ctx.editor.set_progress(crate::status::StatusTopic::Save(buffer_id, v), "Saving\u{2026}");
```

and the worker:

```rust
        run: Box::new(move || {
            let content = snap.to_string();
            // Both the write and the fingerprint use the RESOLVED path: the fingerprint must
            // describe the file actually written. (`fingerprint` follows symlinks, so this
            // agrees with `dispatch_save`'s check on Document.path whenever the link resolves.)
            //
            // BOTH go through the SEAM (`*_with_fs`), not the `RealFs` wrappers. This is the
            // single most valuable thing the seam extension buys: the worker-side save path
            // becomes fault-testable for the FIRST time. Calling `file::save_atomic` here —
            // which hardcodes `RealFs` internally — would silently discard that, and an
            // `Arc<FaultFs>` injected at `Ctx` would have no effect.
            let outcome = file::save_atomic_with_fs(&*fs, &write_path, &content);
            let new_fp = fingerprint_with_fs(&*fs, &write_path);
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
// `pub(crate)`, NOT private: `file_browser_commit::commit_destination` calls this across
// module boundaries. It was `fn` in the tree and must be widened here or the commit arm
// does not compile.
pub(crate) fn perform_save_as(editor: &mut crate::editor::Editor, chosen: std::path::PathBuf,
                   resolved: std::path::PathBuf,
                   executor: &dyn crate::jobs::Executor,
                   clock: &dyn wordcartel_core::history::Clock,
                   msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
                   // The CALLER's handle. Constructing `Arc::new(RealFs)` here would make
                   // Save-As — the most durability-critical user path in this effort —
                   // unreachable by an injected `FaultFs`, silently undoing the seam at the
                   // one place it matters most. Every caller already holds `ctx.fs`.
                   fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>) {
    let v = editor.active().document.version;
    let buffer_id = editor.active().id;
    {
        let mut ctx = crate::registry::Ctx {
            editor, clock, executor, msg_tx: msg_tx.clone(),
            fs: std::sync::Arc::clone(fs),
        };
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

**`prompts::save_as_submit` still calls `perform_save_as` at this point** (Task 21 retires it), so
its call must be updated here or the build breaks between Tasks 16 and 21. Give it the two-path form
against the path it already has — it has no resolution step yet, so both fields are the same value:

```rust
    perform_save_as(editor, target.clone(), target, executor, clock, msg_tx, fs);
```

`save_as_submit` gains an `fs` parameter from its `Ctx`-holding caller for this. It is transitional
code with a two-task lifetime, which is why it takes the simplest correct form rather than a
resolution it will never need.

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
        // FAIL-VERIFY: swap the queue for an `Option` slot, watch this fail, then revert.
        //
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
        // DRIVES THE REAL DISPATCH. An earlier version hand-enqueued (a->b, b->c) — the
        // already-correct sequence — so it passed even if `do_save_to` never captured
        // anything. It guarded nothing, which is exactly what it was split out to prevent.
        //
        // FAIL-VERIFY: move the capture back to dispatch time (bind `prior_key` before the
        // job and use it for the migration), watch this fail with an entry stranded at /b.md.
        let p_a = scratch();
        let p_b = scratch();
        let p_c = scratch();
        std::fs::write(&p_a, b"body\n").expect("seed");
        let mut e = Editor::new_from_text("body\n", Some(p_a.clone()), (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);

        // TWO Save-As dispatched from the SAME source before either merge lands. This is the
        // ordering dispatch-time capture gets wrong: it would record (a->b, a->c).
        {
            let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex,
                msg_tx: tx(), fs: std::sync::Arc::clone(&fs) };
            crate::save::do_save_to(&mut ctx,
                crate::save::SaveTarget::same(p_b.clone()), crate::save::SaveMode::SaveAs);
            crate::save::do_save_to(&mut ctx,
                crate::save::SaveTarget::same(p_c.clone()), crate::save::SaveMode::SaveAs);
        }
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        // The queue the MERGES produced — not one we wrote by hand.
        let mut s = crate::state::SessionState::default();
        let key = |p: &std::path::Path| std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().into_owned();
        s.entries.insert(key(&p_a), crate::state::StateEntry {
            cursor: 7, scroll: 0, marks: Default::default(), mtime: 1, size: 1, seq: 1,
            folds: vec![], block: None });
        let cfg = crate::config::Config::default();
        drain_session_migrations(&mut s, &mut e, &cfg);

        assert!(s.entries.contains_key(&key(&p_c)),
            "the chain must land at the FINAL path — dispatch-time capture strands it at /b");
        assert_eq!(s.entries[&key(&p_c)].cursor, 7, "carrying the original cursor through both hops");
        assert!(!s.entries.contains_key(&key(&p_a)), "no stale source entry");
        assert!(!s.entries.contains_key(&key(&p_b)), "no stranded intermediate entry");
        assert_eq!(s.entries.len(), 1, "exactly ONE entry survives");
        for f in [&p_a, &p_b, &p_c] { let _ = std::fs::remove_file(f); }
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
a decision — the rows are defined in terms of the field's contents. It carries ten tests because
this is the one surface where a design error produces the exact harm class C5 exists to eliminate:
**silent overwrite and save-to-nowhere.**

#### Files

- Create: `wordcartel/src/file_browser_commit.rs`
- Create: `wordcartel/src/file_browser_intercept.rs`
- Modify: `wordcartel/src/file_browser.rs` (`BrowseMode`, `DestinationPurpose`; move the intercept out)
- Modify: `wordcartel/src/lib.rs` (declare both new modules)
- Modify: `wordcartel/src/overlays.rs` (the `FileBrowser` row's `intercept` fn pointer)
- Modify: `wordcartel/src/mouse.rs` (`mouse_file_browser` — the click divergence)

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

// crate::file_browser
/// The shared click-commit path for `mouse::mouse_file_browser`'s `Down(Left)` arm.
///
/// SELECT mode: selects and commits (unchanged). DESTINATION mode: copies the highlighted
/// file's name into the field and returns — it does NOT commit. See decision 9.
pub(crate) fn click_commit_or_copy(editor: &mut crate::editor::Editor);

// crate::minibuffer — UTF-8-codepoint-safe field arithmetic, extracted so destination mode
// reuses it rather than growing a second hand-written cursor (a defect generator).
// These are free functions over (&mut String, &mut usize); `Minibuffer`'s own
// insert/backspace/left/right become one-line delegations to them.
pub(crate) fn text_insert(text: &mut String, cursor: &mut usize, c: char);
pub(crate) fn text_backspace(text: &mut String, cursor: &mut usize);
pub(crate) fn text_left(text: &str, cursor: &mut usize);
pub(crate) fn text_right(text: &str, cursor: &mut usize);
```

#### Steps

1. **Write the ten failing tests.** Create the test module in
   `wordcartel/src/file_browser_commit.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_browser::FileEntry;
    use crate::fsx::EntryKind;

    // The shared keystroke helpers (`press_key_fb`, `press_char_fb`, `press_enter_fb`,
    // `nix_privileged`) live in `test_support` as of Task 12 — this task's tests import them:
    use crate::test_support::{press_key_fb, press_char_fb, press_enter_fb};

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
        // The `~/` assertion is MANDATORY, not conditional. It was originally guarded by
        // `if let Some(home) = dirs::home_dir()`, which meant the entire tilde-expansion
        // branch could be missing and this test would still pass on any container without
        // a resolvable home — a vacuous pass exactly where the interesting behaviour is.
        // `dirs::home_dir()` reads $HOME on unix, so the test SETS it and owns the answer.
        //
        // FAIL-VERIFY: delete the `~/` arm from `resolve_field`, watch this fail.
        let d = tmp("resolve-abs");
        assert_eq!(resolve_field(&d, "/etc/hosts"), std::path::PathBuf::from("/etc/hosts"));

        let home = tmp("resolve-home");
        let prior = std::env::var_os("HOME");
        // Edition 2021: `set_var` is safe here (it becomes `unsafe` only in edition 2024).
        std::env::set_var("HOME", &home);
        let got = resolve_field(&d, "~/notes.md");
        match prior { Some(v) => std::env::set_var("HOME", v),
                      None    => std::env::remove_var("HOME") }
        assert_eq!(got, home.join("notes.md"),
            "`~/` expands against the home dir, unconditionally asserted");

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- The Tab gesture ----------------------------------------------------------

    #[test]
    fn tab_copies_a_name_into_the_field_and_does_not_commit() {
        // The deliberate two-step overwrite gesture: highlight, Tab (see the name land and
        // the footer show the resolved target), Enter (see the overwrite-confirm). Overwrite
        // is never one accidental keystroke, and never reachable without the target visible.
        // Driven through the REAL intercept. Calling `copy_name_into_field` directly proves
        // the helper works, not that Tab reaches it — and "does not commit" would rest on a
        // comment rather than an assertion.
        //
        // FAIL-VERIFY: remove the `KeyCode::Tab` arm from the destination branch, watch the
        // field assertion fail; wire Tab to `commit_destination`, watch the no-commit
        // assertions fail.
        let d = tmp("tab-gesture");
        std::fs::write(d.join("existing.md"), b"x").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.file_browser = Some(crate::file_browser::FileBrowser {
            dir: d.clone(), query: String::new(),
            mode: crate::file_browser::BrowseMode::Destination {
                purpose: crate::file_browser::DestinationPurpose::SaveAs,
                field: "draft".into(), field_cursor: 5,
            },
            listing: Vec::new(), total_seen: 0, unreadable: 0,
            entries: vec![FileEntry { name: "existing.md".into(), kind: EntryKind::File,
                is_symlink: false, broken: false }],
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        });

        press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Tab);

        match &e.file_browser.as_ref().expect("picker stays open").mode {
            crate::file_browser::BrowseMode::Destination { field, field_cursor, .. } => {
                assert_eq!(field, "existing.md", "Tab REPLACES the field content");
                assert_eq!(*field_cursor, "existing.md".len(), "cursor lands at the end");
            }
            other => panic!("still destination mode, got {other:?}"),
        }
        assert!(e.file_browser.is_some(), "Tab does NOT commit — the picker stays open");
        assert!(e.pending_save_overwrite.is_none(), "and raises no overwrite-confirm");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_click_on_a_file_in_destination_mode_copies_the_name_and_does_not_commit() {
        // THE CLICK DIVERGENCE (decision 9) — driven through the REAL mouse path.
        //
        // An earlier version called `click_commit_or_copy` directly. `mouse.rs` is currently
        // wired to `file_browser_enter`, so that test passed while a live click COMMITTED —
        // it guarded the safety property by asserting on a function the click never reached.
        //
        // FAIL-VERIFY: leave `mouse::mouse_file_browser`'s `Down(Left)` arm calling
        // `file_browser_enter` unconditionally (i.e. do not add the mode branch), watch this
        // fail — the picker closes and `victim.md` is overwritten.
        let d = tmp("click-divergence");
        std::fs::write(d.join("victim.md"), b"precious\n").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("draft\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        // Built from THIS task's own types — `open_destination_picker` is Task 21.
        e.file_browser = Some(crate::file_browser::FileBrowser {
            dir: d.clone(), query: String::new(),
            mode: crate::file_browser::BrowseMode::Destination {
                purpose: crate::file_browser::DestinationPurpose::SaveAs,
                field: String::new(), field_cursor: 0,
            },
            listing: Vec::new(), total_seen: 0, unreadable: 0,
            entries: vec![FileEntry {
                name: "victim.md".into(), kind: EntryKind::File, is_symlink: false, broken: false }],
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        });

        // A REAL left-click on the row the painter drew, routed through the overlay mouse
        // table exactly as `reduce` routes it.
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        // Row 0's cell, computed from the overlay rect directly. Task 20 adds
        // `chrome_geom::file_browser_row_origin`; using it here would be a forward reference,
        // and row 0 sits at `list_top` by construction so this needs no helper.
        let ov = crate::chrome_geom::palette_overlay_rect(area,
            e.file_browser.as_ref().unwrap().entries.len());
        let (col, row) = (ov.x + 1, ov.y + 2);   // +1 border column, +2 border + query row
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let ctx = crate::overlays::DispatchCtx {
            reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: &tx, fs: &fs };
        crate::mouse::mouse_file_browser(&mut e, crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: col, row, modifiers: crossterm::event::KeyModifiers::NONE,
        }, area, &ctx);

        // The name landed in the FIELD…
        match &e.file_browser.as_ref().expect("picker stays open").mode {
            crate::file_browser::BrowseMode::Destination { field, .. } =>
                assert_eq!(field, "victim.md", "a click copies the name into the field"),
            other => panic!("still destination mode, got {other:?}"),
        }
        // …and NOTHING was written or dispatched.
        assert!(e.file_browser.is_some(), "the picker must NOT close — a click does not commit");
        assert!(e.pending_save_overwrite.is_none(),
            "and it must NOT raise the overwrite-confirm — that needs a deliberate Enter");
        assert_eq!(std::fs::read_to_string(d.join("victim.md")).expect("read"), "precious\n",
            "the file on disk is untouched");
        let _ = std::fs::remove_dir_all(&d);
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

5. **Thread the dual-duty field into the listing filter — `rederive` must derive BOTH inputs from
   `fb.mode`, not take them as parameters.**

   Task 12 wrote `rederive(fb, opts)` before `BrowseMode` existed, so it filters on `fb.query` and
   takes `destination` as a field on `FilterOpts`. Left that way, the dual-duty field would filter
   the listing **only on a direct edit**: `apply_listing_done` (the initial async listing AND every
   descend) builds `FilterOpts { destination: false, … }` and re-derives from `fb.query`, which is
   empty in destination mode. Typing `chap` would not reveal existing chapter files on first open or
   after descending — losing spec §7.4's overwrite awareness, which is a **safety** property, not a
   convenience.

   Patching the three call sites would work and would drift. Instead, remove the ability to get it
   wrong: `destination` and the filter text are both **fully determined by `fb.mode`**, so `rederive`
   computes them itself and no caller passes either.

```rust
/// Re-derive `entries`/`disclosure` from the CACHED listing. The keystroke path — NO
/// filesystem access.
///
/// Takes the two EDITOR-owned options only. The filter text and the destination flag are
/// derived from `fb.mode` here, because a caller that passed them could pass the wrong ones:
/// every path that rebuilds entries (initial listing, descend, field edit, filter-toggle
/// change) would otherwise have to remember, and `apply_listing_done` did not.
pub(crate) fn rederive(fb: &mut FileBrowser, show_clutter: bool, types: FileTypeFilter) {
    let opts = FilterOpts {
        show_clutter,
        types,
        // Destination mode also shows output-format siblings so a writer sees what they
        // might clobber (spec §7.4).
        destination: fb.mode.is_destination(),
    };
    // DUAL DUTY: the field IS the filter in destination mode; the query is in the others.
    // `filter_text` is the single place that mapping lives.
    let text = fb.mode.filter_text(&fb.query).to_string();
    let at_root = fb.dir.parent().is_none();
    let (rows, d) = filter_and_rank(
        &fb.listing, at_root, &text, opts, fb.total_seen, fb.unreadable);
    fb.entries = rows;
    fb.disclosure = d;
    if fb.selected >= fb.entries.len() {
        fb.selected = fb.entries.len().saturating_sub(1);
    }
    fb.scroll_top = fb.scroll_top.min(fb.entries.len().saturating_sub(1));
}
```

   Update `refetch` the same way (`refetch(fs, fb, show_clutter, types)`), and update all three
   callers to pass the editor's two options rather than a constructed `FilterOpts`:

   * `Editor::open_file_browser` and `Editor::open_destination_picker`
   * `file_browser::apply_listing_done` — **this is the site that was hardcoding
     `destination: false`**; it now passes `editor.files_show_clutter, editor.files_type_filter`
     and the mode decides the rest
   * the destination-mode field-edit path in `file_browser_intercept`

   **Every call is `rederive(fb, show_clutter, types)`** — three arguments, no `FilterOpts`. Any
   remaining `rederive(fb, opts)` in this task's prose is stale; the whole point of the restructure
   is that no caller constructs `FilterOpts`.

   Add the test, driven from where a writer's keystrokes actually enter — **not** by calling
   `rederive`:

```rust
    #[test]
    fn typing_in_destination_mode_narrows_the_listing_to_matching_files() {
        // Spec §7.4's overwrite awareness: the field is simultaneously the filename-to-be
        // AND a live filter, so typing `chap` reveals existing chapter files a writer might
        // clobber. This failed silently when `apply_listing_done` hardcoded
        // `destination: false` and re-derived from the (empty) `query`.
        //
        // FAIL-VERIFY: make `rederive` filter on `fb.query` instead of
        // `fb.mode.filter_text(...)`, watch this fail, then revert.
        let d = std::env::temp_dir().join(format!("wc-destfilter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        for n in ["chapter-one.md", "chapter-two.md", "notes.md", "outline.md"] {
            std::fs::write(d.join(n), b"x").expect("seed");
        }
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        // Construct the destination browser from THIS task's own types and start the listing
        // directly — `Editor::open_destination_picker` belongs to Task 21, and using it here
        // would be a forward reference that blocks this task under TDD.
        let mut fb = crate::file_browser::FileBrowser {
            dir: d.clone(), query: String::new(),
            mode: crate::file_browser::BrowseMode::Destination {
                purpose: crate::file_browser::DestinationPurpose::SaveAs,
                field: String::new(), field_cursor: 0,
            },
            listing: Vec::new(), total_seen: 0, unreadable: 0, entries: Vec::new(),
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        };
        crate::file_browser::start_listing(&mut fb, d.clone(), &fs, &tx);
        e.file_browser = Some(fb);
        // The listing arrives asynchronously — pump it, exactly as the run loop would.
        pump_listing(&mut e, &rx);
        assert!(e.file_browser.as_ref().expect("open").entries.len() >= 4,
            "precondition: all four files listed before typing");

        // Type through the REAL intercept, one keystroke at a time.
        for c in ['c', 'h', 'a', 'p'] { press_char(&mut e, &fs, &tx, c); }

        let names: Vec<String> = e.file_browser.as_ref().expect("still open")
            .entries.iter().map(|r| r.name.clone()).collect();
        assert!(names.iter().any(|n| n == "chapter-one.md"),
            "existing chapter files must be REVEALED as the writer types: {names:?}");
        assert!(names.iter().any(|n| n == "chapter-two.md"), "{names:?}");
        assert!(!names.iter().any(|n| n == "notes.md"),
            "non-matching files must be filtered out: {names:?}");
        // And the field still holds what was typed — it is dual-duty, not consumed.
        match &e.file_browser.as_ref().expect("open").mode {
            crate::file_browser::BrowseMode::Destination { field, .. } =>
                assert_eq!(field, "chap", "the field is the filename-to-be as well as the filter"),
            other => panic!("expected destination mode, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }
```

   with the keystroke helper alongside `press_enter`:

```rust
    /// Feed one printable character through the real intercept.
    fn press_char(e: &mut crate::editor::Editor,
        fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
        tx: &std::sync::mpsc::Sender<crate::app::Msg>, c: char)
    {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let ctx = crate::overlays::DispatchCtx {
            reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: tx, fs };
        let ev = Event::Key(KeyEvent {
            code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let _ = crate::file_browser_intercept::intercept(crate::app::Msg::Input(ev), e, &ctx);
    }
```

6. **Extract the UTF-8 cursor arithmetic** from `minibuffer.rs` into four free functions, so
   destination mode reuses it instead of growing a second hand-written cursor:

```rust
    /// Insert `c` at `cursor`, advancing by its UTF-8 length.
    pub(crate) fn text_insert(text: &mut String, cursor: &mut usize, c: char) {
        text.insert(*cursor, c);
        *cursor += c.len_utf8();
    }

    /// Delete the codepoint before `cursor`.
    pub(crate) fn text_backspace(text: &mut String, cursor: &mut usize) {
        if *cursor == 0 { return; }
        let prev = text[..*cursor].chars().next_back().expect("cursor > 0 implies a char");
        *cursor -= prev.len_utf8();
        text.remove(*cursor);
    }

    /// Move one codepoint left.
    pub(crate) fn text_left(text: &str, cursor: &mut usize) {
        if *cursor > 0 {
            let prev = text[..*cursor].chars().next_back().expect("cursor > 0 implies a char");
            *cursor -= prev.len_utf8();
        }
    }

    /// Move one codepoint right.
    pub(crate) fn text_right(text: &str, cursor: &mut usize) {
        if *cursor < text.len() {
            let next = text[*cursor..].chars().next().expect("cursor < len implies a char");
            *cursor += next.len_utf8();
        }
    }
```

   `Minibuffer::{insert, backspace, left, right}` become one-line delegations to these, so both
   callers share one implementation. Add the multibyte test — exact bodies and offsets, since
   "covers multibyte" without asserted offsets is where UTF-8 cursor bugs hide:

```rust
    #[test]
    fn destination_field_edits_are_utf8_codepoint_safe() {
        // Byte lengths: 'a'=1, 'é'=2, '中'=3, '🙂'=4. Every assertion is on the BYTE cursor,
        // because that is what a naive `cursor += 1` gets wrong.
        let mut f = String::new();
        let mut c = 0usize;
        for ch in ['a', 'é', '中', '🙂'] { text_insert(&mut f, &mut c, ch); }
        assert_eq!(f, "aé中🙂");
        assert_eq!(c, 1 + 2 + 3 + 4, "cursor advances by UTF-8 length, not by 1 per char");

        text_left(&f, &mut c);                       // back over 🙂 (4 bytes)
        assert_eq!(c, 1 + 2 + 3);
        text_left(&f, &mut c);                       // back over 中 (3)
        assert_eq!(c, 1 + 2);
        text_right(&f, &mut c);                      // forward over 中
        assert_eq!(c, 1 + 2 + 3);

        text_backspace(&mut f, &mut c);              // delete 中
        assert_eq!(f, "aé🙂", "the codepoint BEFORE the cursor is removed whole");
        assert_eq!(c, 1 + 2);

        // Boundary: left at 0 and right at len are no-ops, never a panic or a split codepoint.
        c = 0; text_left(&f, &mut c); assert_eq!(c, 0);
        c = f.len(); text_right(&f, &mut c); assert_eq!(c, f.len());
        c = 0; text_backspace(&mut f, &mut c);
        assert_eq!(f, "aé🙂", "backspace at 0 is a no-op");
    }
```

7. **Move the intercept** into `file_browser_intercept.rs` and branch on mode. Destination mode
   routes printable characters, `Backspace`, `Left`/`Right`, and `Event::Paste` into the **field**
   via the four helpers above, and the six shared nav keys into the **selection** via
   `list_window::{list_nav_key, apply_list_nav}`. **Nav never edits the field; field edits never move
   the selection except to clamp it.** `Tab` on a highlighted `File` calls `copy_name_into_field`.
   `Esc` closes the picker (`editor.file_browser = None`) — Task 21 later REPLACES that line
   with `cancel_destination`, which adds the quit-drain abort. Keeping it plain here means this
   task depends on nothing later. Each field edit calls
   `file_browser_listing::rederive(fb, editor.files_show_clutter, editor.files_type_filter)` — the
   destination flag comes from `fb.mode`, not from the caller.

   **Repoint the shared test helper in the same step.** `test_support::press_key_fb` (Task 12)
   calls `crate::file_browser::intercept`; change that one line to
   `crate::file_browser_intercept::intercept`. Every keystroke test in Tasks 12–26 goes through
   it, so leaving it behind fails the build loudly rather than silently — but it is named here
   because "move a function" is exactly the change that forgets its test-only caller.

8. **Add the click divergence** in `file_browser.rs` and wire it to `mouse.rs`:

```rust
/// The `Down(Left)` commit path, shared by both modes — and deliberately DIFFERENT in each.
///
/// Select mode selects and commits, as it always has. Destination mode copies the
/// highlighted file's name into the field and stops: a single click must never reach a
/// write. The stakes are asymmetric — a mis-click in select mode opens the wrong file (close
/// the buffer), a mis-click in destination mode would land on the overwrite path for an
/// existing file. The inconsistency between the two modes IS the safety property; do not
/// "unify" them.
pub(crate) fn click_commit_or_copy(editor: &mut crate::editor::Editor) {
    let Some(fb) = editor.file_browser.as_mut() else { return };
    let Some(entry) = fb.entries.get(fb.selected).cloned() else { return };
    match &mut fb.mode {
        BrowseMode::Select => { /* caller invokes file_browser_enter — unchanged */ }
        BrowseMode::Destination { field, field_cursor, .. } => {
            if matches!(entry.kind, crate::fsx::EntryKind::File) {
                crate::file_browser_commit::copy_name_into_field(field, field_cursor, &entry.name);
            }
            // Dir/Other/Unknown: the click has already moved the highlight; nothing else.
        }
    }
}
```

   In `mouse::mouse_file_browser`'s `Down(Left)` arm, call `click_commit_or_copy` and invoke
   `file_browser_enter` **only** when the mode is `Select`.

9. **Declare both modules** in `lib.rs`:

```rust
pub mod file_browser_commit;
pub mod file_browser_intercept;
```

and repoint the `FileBrowser` row's `intercept` in `overlays.rs` to
`crate::file_browser_intercept::intercept`.

10. **Run — expect green:**

```
cargo test -p wordcartel --lib file_browser_commit:: file_browser_intercept:: file_browser:: mouse::
```

Expected: `test result: ok`, including all ten commit tests (nine table/field cases plus the
click divergence).

11. **Commit:** `feat(c5): add destination mode with the four-row Enter decision table`

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
    // `Path::extension()` returns `Some("")` for a TRAILING-DOT name like `notes.` — there
    // IS an embedded dot, so it is not None, and the part after it is empty. Treating that
    // as "has an extension" would take the Honoured arm and skip defaulting, leaving the
    // writer with an extensionless `notes.` file. Filter the empty case into the None arm.
    match path.extension().and_then(|e| e.to_str()).filter(|e| !e.is_empty()) {
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

// crate::chrome_geom — the inverse of `file_browser_row_at`, so tests (and any future
// caller) can address the cell the painter drew a given row at without duplicating the
// geometry. Single-sourced with `file_browser_list_h` below.
pub(crate) fn file_browser_row_origin(area: ratatui::layout::Rect, fb: &FileBrowser,
    row_index: usize) -> (u16, u16);
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

        // RESOLUTION must be visible before commit, not discovered in a confirm dialog. If
        // `footer_target` skipped `resolve_write_destination`, it would echo the symlink path
        // and both assertions above would still pass.
        //
        // FAIL-VERIFY: drop the `resolve_write_destination` call from `footer_target`, watch
        // this fail — the footer shows `link.md` instead of the target.
        #[cfg(unix)]
        {
            std::fs::write(d.join("real-target.md"), b"x").expect("seed");
            std::os::unix::fs::symlink(d.join("real-target.md"), d.join("link.md"))
                .expect("symlink");
            if let BrowseMode::Destination { field, field_cursor, .. } = &mut fb.mode {
                *field = "link.md".into(); *field_cursor = field.len();
            }
            let line = footer_target(&crate::fsx::RealFs, &fb).expect("footer");
            assert!(line.contains("real-target.md"),
                "the footer names the RESOLVED write target, not the link the writer typed: {line}");
        }
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
   in `chrome_geom.rs` that both the painter and `file_browser_row_at` call. Add this test — an assertion that the two agree at the boundary the footer moved:

```rust
    #[test]
    fn hit_testing_and_the_painter_agree_on_the_last_row_in_destination_mode() {
        // The footer consumes a row from the block's bottom edge, so the list interior
        // shrinks. If `file_browser_row_at` kept the old height, a click on the last visible
        // row would select the row BELOW the one drawn there — off-by-one on a surface where
        // the next keystroke can commit a write.
        //
        // FAIL-VERIFY: leave `file_browser_row_at` computing its own height instead of
        // calling `file_browser_list_h`, watch this fail.
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let mut fb = FileBrowser {
            dir: std::env::temp_dir(), query: String::new(),
            mode: BrowseMode::Destination {
                purpose: DestinationPurpose::SaveAs, field: "x".into(), field_cursor: 1 },
            listing: Vec::new(), total_seen: 0, unreadable: 0,
            entries: (0..12).map(|i| FileEntry {
                name: format!("f{i:02}.md"), kind: crate::fsx::EntryKind::File,
                is_symlink: false, broken: false }).collect(),
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        };
        let list_h = crate::chrome_geom::file_browser_list_h(area, &fb) as usize;
        assert!(list_h > 0 && list_h < fb.entries.len(),
            "precondition: the list is windowed, so a last visible row exists");
        let last = list_h - 1;
        let (col, row) = crate::chrome_geom::file_browser_row_origin(area, &fb, last);
        assert_eq!(crate::chrome_geom::file_browser_row_at(area, &fb, col, row), Some(last),
            "a click on the cell the painter drew row {last} at must select row {last}");
        // And one row further down is OUTSIDE the list — that cell belongs to the footer.
        assert_eq!(crate::chrome_geom::file_browser_row_at(area, &fb, col, row + 1), None,
            "the row below the last entry is the footer, not a selectable entry");
        fb.selected = last;
    }
```

   **What this guard covers, and what it does not** — following the model of Task 4's
   syscall-economy test, which discloses its own boundary:

   *Covers:* a regression where `file_browser_row_at` stops calling `file_browser_list_h` and
   recomputes its own height, so hit-testing and the windowing disagree about how many rows fit.
   The `row + 1 → None` assertion is what catches it.

   *Does NOT cover:* the painter drawing the list at different coordinates entirely. This compares
   geometry against geometry — `file_browser_row_at` against `file_browser_row_origin` — not against
   a rendered frame. Closing that would need a `TestBackend` render and a scrape of the drawn rows,
   which the e2e journey (Task 26) exercises end-to-end. Stated because a test named "the painter
   agrees" that never invokes the painter reads as more coverage than it is.

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
    pub fn open_destination_picker(&mut self,
        fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
        msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
        purpose: crate::file_browser::DestinationPurpose,
        dir: std::path::PathBuf, field: String) -> bool;
}

// crate::file_browser_commit
/// Execute a destination-mode Enter — THE single place a picker commit becomes a write.
/// Dispatches on `DestinationPurpose`, covering all three goals (SaveAs, WriteBlock, Export).
pub(crate) fn commit_destination(
    editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    executor: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>);

// crate::editor — Editor gains, so the OverwriteSaveAs prompt arm can rebuild a SaveTarget
// (it needs BOTH paths; `pending_save_overwrite` carries only the resolved one). Initialised
// to `None` in Editor's constructor.
//
// PAIRED LIFETIME: it is set and cleared in lockstep with `pending_save_overwrite`. Every
// site that abandons overwrite Save-As state must clear BOTH:
//   * `file_browser::cancel_destination`      (Esc out of the picker)
//   * `prompts::intercept`'s Esc arm          (Esc on the overwrite modal)
//   * `prompts::resolve_prompt`'s Cancel arm  (explicit Cancel)
//   * `prompts::resolve_prompt`'s OverwriteSaveAs arm (consumed via `.take()`)
// A stale `chosen` surviving cancellation would pair with a different `resolved` on a later
// round trip — a silent wrong-target write.
pub pending_save_as_chosen: Option<std::path::PathBuf>,

// crate::prompts
/// Open the Save-As destination picker, seeded at the active document's directory.
/// Returns whether it opened.
pub fn open_save_as(editor: &mut Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> bool;
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
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs,
            std::env::temp_dir(), String::new());

        // Driven through the REAL intercept. Calling `cancel_destination` directly is the
        // pattern this very task's commit-arm comment condemns: Task 18 ships a plain
        // `editor.file_browser = None` Esc arm that THIS task must replace, and a direct call
        // passes whether or not that replacement happened.
        //
        // FAIL-VERIFY: leave Task 18's plain Esc arm in place, watch the drain assertions fail.
        press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Esc);

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
    pub fn open_destination_picker(&mut self,
        fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
        msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
        purpose: crate::file_browser::DestinationPurpose,
        dir: std::path::PathBuf, field: String) -> bool
    {
        crate::overlays::close_all(self);
        self.pending_keys.clear(); self.pending_mark = None;
        let field_cursor = field.len();
        let mut fb = crate::file_browser::FileBrowser {
            dir: dir.clone(), query: String::new(),
            mode: crate::file_browser::BrowseMode::Destination { purpose, field, field_cursor },
            listing: Vec::new(), total_seen: 0, unreadable: 0, entries: Vec::new(),
            disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        };
        // ASYNC, exactly like `open_file_browser`. A synchronous `refetch` here would block
        // the input loop on the directory read and undo Task 13 for every destination
        // picker — Save-As, Write-Block, and Export. There is no synchronous listing path.
        crate::file_browser::start_listing(&mut fb, dir, fs, msg_tx);
        self.file_browser = Some(fb);
        true
    }
```

4. **Rewire `open_save_as`** in `prompts.rs`:

```rust
/// Open the Save-As destination picker, seeded at the active doc's directory.
pub fn open_save_as(editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> bool
{
    let dir = editor.active().document.path.as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    editor.open_destination_picker(fs, msg_tx,
        crate::file_browser::DestinationPurpose::SaveAs, dir, String::new())
}
```

and `blocks_marked::block_write` the same way with `DestinationPurpose::WriteBlock`.

   **Update the two `registry.rs` handlers that call them** — both are compile blockers otherwise,
   and `registry.rs` is the file most exposed to this class (see the ripple map):

```rust
        r.register("save_as", "Save As\u{2026}", Some(MenuCategory::File), |c| {
            crate::prompts::open_save_as(c.editor, &c.fs, &c.msg_tx);
            CommandResult::Handled
        });
        // …and, further down, the Block-category handler:
        r.register("block_write", "Write Block to File\u{2026}", Some(MenuCategory::Block), |c| {
            crate::blocks_marked::block_write(c.editor, &c.fs, &c.msg_tx);
            CommandResult::Handled
        });
```

   **Also update `prompts::resolve_prompt`'s `OverwriteWriteBlock` arm**, which calls
   `perform_block_write` and now needs the seam:

```rust
        PromptAction::OverwriteWriteBlock => {
            if let Some(t) = editor.pending_write_block.take() {
                if let Some(b) = editor.active().marked_block {
                    perform_block_write(editor, &t, b.start, b.end, fs);
                } else {
                    editor.set_status(crate::status::StatusKind::Info, "no marked block");
                }
            }
        }
```

5. **Remove the sniff** in `dispatch_save_then`:

Use a **private reporting core**, not a new `CommandResult` variant. `CommandResult` is
`Handled | Noop | Quit` and is returned by every command handler in the registry; widening it to
carry one call site's control-flow fact would put that blast radius on a shared enum for no gain.
The core keeps the fact local:

```rust
/// Registry `"save"` handler — unchanged public shape.
pub fn dispatch_save(ctx: &mut Ctx) -> CommandResult {
    dispatch_save_reporting(ctx);
    CommandResult::Handled
}

/// The same work, RETURNING whether it opened a Save-As destination picker.
///
/// This return value is what replaces `dispatch_save_then`'s old
/// `minibuffer.kind == SaveAs` sniff. Inferring control flow from which overlay happens to
/// be up is what made that coupling silently breakable; the fact is now produced by the
/// function that knows it.
fn dispatch_save_reporting(ctx: &mut Ctx) -> bool {
    let path = match &ctx.editor.active().document.path {
        None => {
            let opened = crate::prompts::open_save_as(ctx.editor, &ctx.fs, &ctx.msg_tx);
            return opened;
        }
        Some(p) => p.clone(),
    };
    let current_fp = fingerprint_with_fs(&*ctx.fs, &path);
    if current_fp != ctx.editor.active().document.stored_fp {
        ctx.editor.open_prompt(crate::prompt::Prompt::external_mod());
        ctx.editor.set_status_full(crate::status::StatusKind::Warning,
            "File changed on disk \u{2014} choose [R]eload or [O]verwrite",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return false;
    }
    do_save(ctx);
    false
}

pub(crate) fn dispatch_save_then(ctx: &mut crate::registry::Ctx,
    action: crate::editor::PostSaveAction)
{
    let was_unnamed = ctx.editor.active().document.path.is_none();
    let buffer_id = ctx.editor.active().id;
    let v = ctx.editor.active().document.version;
    let opened_save_as = dispatch_save_reporting(ctx);
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

6. **Add `cancel_destination`** in `file_browser.rs`, carrying the Effort-6 abort:

```rust
/// Esc out of a destination picker. Mirrors the cleanup `save_as_submit`'s empty-path arm
/// and `prompts::intercept`'s Esc arm already do — including ABORTING an in-progress quit
/// drain. Without that, backing out leaves `quit_drain` Some-but-inert.
pub(crate) fn cancel_destination(editor: &mut crate::editor::Editor) {
    editor.file_browser = None;
    editor.pending_save_as = None;
    editor.pending_save_overwrite = None;
    // Cleared HERE too: it is half of a two-field pair with `pending_save_overwrite`, and a
    // surviving `chosen` could be picked up by a LATER overwrite round trip and paired with
    // a different resolved path. Every place that abandons overwrite Save-As state clears
    // both — see the sweep below.
    editor.pending_save_as_chosen = None;
    editor.pending_write_block = None;
    if editor.quit_drain.is_some() {
        editor.quit_drain = None;
        editor.quit_drain_advance = false;
    }
}
```

Wire it to the destination-mode `Esc` arm in `file_browser_intercept`.

7. **Wire the commit arms — this is the replacement for the retired submit paths.** Without this,
   removing `save_as_submit` / `block_write_submit` leaves two of the picker's four goals with no
   executable path at all. Add to `file_browser_commit.rs`:

```rust
/// Execute a destination-mode Enter. THE single place a picker commit becomes a write.
///
/// Dispatches on `DestinationPurpose`, so adding a future destination consumer is one arm
/// the compiler demands rather than a new branch somewhere else.
pub(crate) fn commit_destination(
    editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    executor: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    let Some(fb) = editor.file_browser.as_ref() else { return };
    let crate::file_browser::BrowseMode::Destination { purpose, field, .. } = &fb.mode
        else { return };
    let purpose = purpose.clone();
    let dir = fb.dir.clone();
    let highlighted = fb.entries.get(fb.selected).cloned();

    match classify_destination_enter(&**fs, &dir, field, highlighted.as_ref()) {
        // Rows 1 and 3 — navigate, do not write. The listing lands asynchronously and
        // `apply_listing_done` commits `fb.dir` only on success.
        CommitOutcome::Descend(target) => {
            if let Some(fb) = editor.file_browser.as_mut() {
                crate::file_browser::start_listing(fb, target, fs, msg_tx);
            }
            return;
        }
        // Nothing to commit — an empty field with no usable highlight. A Sticky Warning,
        // matching what the retired `save_as_submit` empty-path arm produced.
        CommitOutcome::Nothing => {
            let noun = match purpose {
                crate::file_browser::DestinationPurpose::SaveAs => "save-as",
                crate::file_browser::DestinationPurpose::WriteBlock => "write block",
                crate::file_browser::DestinationPurpose::Export { .. } => "export",
            };
            editor.set_status_full(crate::status::StatusKind::Warning,
                format!("{noun}: empty path"),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            // Backing out of a drain's Save-As aborts the quit (Effort-6 Codex C2).
            if matches!(purpose, crate::file_browser::DestinationPurpose::SaveAs) {
                editor.pending_save_as = None;
                editor.quit_drain = None;
                editor.quit_drain_advance = false;
            }
            return;
        }
        CommitOutcome::Commit(raw) => {
            // Extension policy applies to SAVE destinations only — an export's extension is
            // fixed by the chosen format.
            let chosen = match &purpose {
                crate::file_browser::DestinationPurpose::Export { .. } => raw,
                _ => match apply_extension_policy(&raw) {
                    ExtVerdict::Defaulted(p) | ExtVerdict::Honoured(p) => p,
                    ExtVerdict::Redirect { path, ext } => {
                        // F4: refuse the save, explain, and carry the typed path into the
                        // export destination picker — advice with somewhere to go.
                        editor.set_status_full(crate::status::StatusKind::Warning,
                            format!("{ext} is an export format — opening Export instead"),
                            crate::status::StatusLifetime::Sticky,
                            crate::status::StatusSource::Host, None);
                        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or(dir);
                        let field = path.file_name()
                            .map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        editor.open_destination_picker(fs, msg_tx,
                            crate::file_browser::DestinationPurpose::Export { ext },
                            dir, field);
                        return;
                    }
                },
            };
            // Resolve through symlinks BEFORE any write is dispatched (§7.6.1).
            let resolved = match crate::fsx::resolve_write_destination(&**fs, &chosen) {
                Ok(r) => r,
                Err(crate::fsx::DestError::BrokenSymlink) => {
                    editor.set_status_full(crate::status::StatusKind::Warning,
                        format!("{}: destination symlink cannot be resolved", chosen.display()),
                        crate::status::StatusLifetime::Sticky,
                        crate::status::StatusSource::Host, None);
                    return;
                }
            };
            editor.file_browser = None;   // the picker's work is done

            // The overwrite-confirm names the RESOLVED target — the file whose bytes will
            // actually be replaced.
            let exists = crate::fsx::exists_via(&**fs, &resolved);
            match purpose {
                crate::file_browser::DestinationPurpose::SaveAs => {
                    if exists {
                        editor.pending_save_overwrite = Some(resolved.clone());
                        editor.pending_save_as_chosen = Some(chosen);
                        editor.open_prompt(crate::prompt::Prompt::save_overwrite(&resolved));
                    } else {
                        crate::prompts::perform_save_as(
                            editor, chosen, resolved, executor, clock, msg_tx, fs);
                    }
                }
                crate::file_browser::DestinationPurpose::WriteBlock => {
                    let Some(b) = editor.active().marked_block else {
                        editor.set_status(crate::status::StatusKind::Info, "no marked block");
                        return;
                    };
                    if exists {
                        editor.pending_write_block = Some(resolved);
                        editor.open_prompt(crate::prompt::Prompt::write_block_overwrite(
                            editor.pending_write_block.as_ref().expect("just set")));
                    } else {
                        crate::prompts::perform_block_write(editor, &resolved, b.start, b.end, fs);
                    }
                }
                crate::file_browser::DestinationPurpose::Export { ext } => {
                    if exists {
                        editor.pending_export = Some(crate::export::PendingExport {
                            ext, target: resolved });
                        editor.open_prompt(crate::prompt::Prompt::export_overwrite(
                            &editor.pending_export.as_ref().expect("just set").target));
                    } else {
                        // NOTE: `do_export` gains its `fs` parameter in Task 22. Until that
                        // lands, this arm calls the CURRENT arity
                        // (`do_export(editor, &ext, &resolved, msg_tx, false)`); Task 22
                        // adds the argument here as part of threading the seam into the
                        // export worker. Stated so this task compiles standalone.
                        crate::export::do_export(editor, &ext, &resolved, msg_tx, false);
                    }
                }
            }
        }
    }
}
```

   `Editor` gains `pending_save_as_chosen: Option<PathBuf>` so the `OverwriteSaveAs` prompt arm can
   reconstruct the `SaveTarget` — it needs BOTH paths, and `pending_save_overwrite` carries only the
   resolved one. Update that arm in `prompts::resolve_prompt`:

```rust
        PromptAction::OverwriteSaveAs => {
            if let (Some(resolved), Some(chosen)) =
                (editor.pending_save_overwrite.take(), editor.pending_save_as_chosen.take())
            {
                perform_save_as(editor, chosen, resolved, ex, clock, msg_tx, fs);
            }
        }
```

   **`perform_block_write` and `perform_save_as` both become `pub(crate)`** — they are `fn`
   (private) in the tree, and `file_browser_commit::commit_destination` calls them across a module
   boundary. `perform_block_write` also gains `fs` and routes through `file::save_atomic_with_fs`:

```rust
pub(crate) fn perform_block_write(editor: &mut crate::editor::Editor,
    target: &std::path::Path, start: usize, end: usize,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>)
{
    let text = editor.active().document.buffer.slice(start..end);
    match crate::file::save_atomic_with_fs(&**fs, target, &text) {
        Ok(_)  => editor.set_status(crate::status::StatusKind::Info,
                      format!("wrote block to {}", target.display())),
        Err(e) => editor.set_status_full(crate::status::StatusKind::Error, e.to_string(),
                      crate::status::StatusLifetime::Sticky,
                      crate::status::StatusSource::Host, None),
    }
}
```

   Everything else `commit_destination` reaches is already visible: `Prompt::{save_overwrite,
   write_block_overwrite, export_overwrite}` are `pub`, `export::do_export` is `pub(crate)`, and
   `MarkedBlock`'s fields are `pub`. Those were checked; these two were the only private ones.

   Wire `commit_destination` to the destination-mode `Enter` arm in `file_browser_intercept`, which
   has `DispatchCtx` and therefore `fs`, `ex`, `clock`, and `msg_tx`.

8. **Retire the minibuffer kinds.** Remove `MinibufferKind::{SaveAs, WriteBlock}` and their
   `prompts::{save_as_submit, block_write_submit}` dispatch arms — **only now that step 7 has put a
   working path in their place.** The three empty-path Sticky-Warning tests move to the picker path:
   an empty field yields `CommitOutcome::Nothing`, which sets the same Sticky Warning.
   **Their assertions on kind and lifetime must not weaken.**

9. **Add the three end-to-end commit tests** — the replacement paths must be proven reachable from
   the picker's Enter key, not merely present as functions:

```rust
    // ---- All three drive Enter through the INTERCEPT, not commit_destination -----------
    //
    // FAIL-VERIFY (all three): delete the `KeyCode::Enter` arm from
    // `file_browser_intercept`'s destination branch, watch all three fail, then revert.
    //
    // An earlier draft called `commit_destination` directly. That is the vacuous-guard
    // pattern: a missing or mis-wired Enter arm would pass every one of them, and "the
    // commit path does not exist" is the exact defect the gate caught in this task last
    // round. A test named end-to-end that skips the entry point reads as coverage while
    // proving only that a function it hand-called works.

    fn press_enter(e: &mut Editor, fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
        ex: &dyn crate::jobs::Executor, clk: &dyn wordcartel_core::history::Clock,
        tx: &std::sync::mpsc::Sender<crate::app::Msg>)
    {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx {
            reg: &reg, keymap: &km, ex, clock: clk, msg_tx: tx, fs };
        let enter = Event::Key(KeyEvent {
            code: KeyCode::Enter, modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let _ = crate::file_browser_intercept::intercept(crate::app::Msg::Input(enter), e, &ctx);
    }

    #[test]
    fn save_as_commits_end_to_end_from_enter() {
        let d = std::env::temp_dir().join(format!("wc-saveas-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        let mut e = Editor::new_from_text("chapter body\n", None, (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), "chapter one".into());

        press_enter(&mut e, &fs, &ex, &clk, &tx);
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }

        // Extension policy applied, file written, buffer rekeyed and clean.
        assert_eq!(std::fs::read_to_string(d.join("chapter one.md")).expect("written"),
            "chapter body\n");
        assert_eq!(e.active().document.path.as_deref(), Some(d.join("chapter one.md").as_path()));
        assert!(!e.active().document.dirty());
        assert!(e.file_browser.is_none(), "the picker closed on commit");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn write_block_commits_end_to_end_from_enter() {
        let d = std::env::temp_dir().join(format!("wc-wb-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        let mut e = Editor::new_from_text("alpha beta gamma\n", None, (80, 24));
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false });
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::WriteBlock, d.clone(), "excerpt".into());

        press_enter(&mut e, &fs, &ex, &clk, &tx);

        assert_eq!(std::fs::read_to_string(d.join("excerpt.md")).expect("written"), "alpha");
        assert!(e.active().document.path.is_none(),
            "write-block does NOT rekey the buffer — it exports a slice");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn export_commits_end_to_end_from_enter_through() {
        // Decision 4: a bare Enter on the PRE-SEEDED picker must reproduce today's
        // zero-decision export. Export had no Enter-through commit test at all.
        let d = std::env::temp_dir().join(format!("wc-exp-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).expect("dir");
        let src = d.join("notes.md");
        std::fs::write(&src, b"# hi\n").expect("seed");
        let mut e = Editor::new_from_text("# hi\n", Some(src), (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        // Seeded exactly as `run_export` seeds it — the Enter-through path.
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::Export { ext: "html".into() },
            d.clone(), "notes.html".into());

        press_enter(&mut e, &fs, &ex, &clk, &tx);

        // The commit arm dispatched an export for the seeded target. Assert on the DISPATCH
        // rather than on pandoc's output: pandoc may be absent on the gate machine, and the
        // wiring is what this test owns.
        assert!(e.file_browser.is_none(), "the picker closed on commit");
        // Assert the DISPATCH specifically — an `|| status contains "export"` fallback would
        // pass on any export-ish status message, including a failure, proving nothing about
        // whether Enter reached the commit arm.
        // BOUNDED RECEIVE, not `try_iter()`: `do_export` spawns a thread, so an immediate
        // drain races it and the test would pass or fail on scheduling. Same discipline the
        // listing tests use.
        let dispatched = std::iter::from_fn(|| rx.recv_timeout(
                std::time::Duration::from_secs(5)).ok())
            .take(4)
            .any(|m| matches!(m,
                crate::app::Msg::ExportDone { ref target, .. } if target == &d.join("notes.html")));
        assert!(dispatched,
            "Enter on the pre-seeded picker must dispatch an ExportDone for notes.html \
             (status was {:?})", e.status_text());
        let _ = std::fs::remove_dir_all(&d);
    }
```

10. **Run — expect green:**

```
cargo test -p wordcartel --lib save:: prompts:: blocks_marked:: file_browser:: file_browser_commit::
```

Expected: `test result: ok`. **`prompts::tests::save_and_quit_on_unnamed_buffer_does_not_arm_pending_after_save`
must still pass** — it asserts no job is dispatched and `pending_after_save` stays `None`, which the
picker path preserves.

11. **Commit:** `refactor(c5): route Save-As and Write-Block through the destination picker`

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
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>, overwrite_confirmed: bool,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>);
pub struct PendingExport { pub ext: String, pub target: PathBuf }

// crate::editor
impl Editor {
    pub fn open_destination_picker(&mut self,
        fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
        msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
        purpose: crate::file_browser::DestinationPurpose,
        dir: std::path::PathBuf, field: String) -> bool;
}
```

**Produces:**

```rust
// crate::export
pub fn run_export(editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>, ext: &str,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>);

/// `run_export` with an injectable pandoc probe — the merge gate runs on machines without
/// pandoc, so the availability check must be injected rather than detected.
pub(crate) fn run_export_with_probe(editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>, ext: &str,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    pandoc_available: impl Fn() -> bool);
```

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

        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        run_export_with_probe(&mut e, &fs, "html", &tx, || true);

        let fb = e.file_browser.as_ref().expect("export opens the destination picker");
        assert_eq!(fb.dir, d, "seeded at the SOURCE's directory");
        match &fb.mode {
            crate::file_browser::BrowseMode::Destination { purpose, field, .. } => {
                // Compare BY REFERENCE — `DestinationPurpose::Export { ext: String }` is not
                // `Copy`, so `*purpose` would move a `String` out of a borrow of `fb.mode`.
                assert_eq!(purpose, &crate::file_browser::DestinationPurpose::Export {
                    ext: "html".into() });
                assert_eq!(field, "notes.html",
                    "pre-filled with derived_export_path's file name, so bare Enter == today");
            }
            other => panic!("expected a destination picker, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn export_destination_picker_opens_without_pandoc_installed() {
        // The merge gate runs on machines with no pandoc. `run_export` probes
        // `pandoc --version` before anything else, so an environment assumption here would
        // fail the gate rather than the code. The probe is injected, not detected.
        let d = std::env::temp_dir().join(format!("wc-exp-nopandoc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        let src = d.join("notes.md");
        std::fs::write(&src, b"# hi\n").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("# hi\n", Some(src), (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        // Pandoc PRESENT (injected) → the picker opens regardless of the host machine.
        run_export_with_probe(&mut e, &fs, "html", &tx, || true);
        assert!(e.file_browser.is_some(), "an injected-present probe opens the picker");
        // Pandoc ABSENT (injected) → the refusal fires and no picker opens.
        e.file_browser = None;
        run_export_with_probe(&mut e, &fs, "html", &tx, || false);
        assert!(e.file_browser.is_none(), "an injected-absent probe opens NO picker");
        assert!(e.status_text().to_lowercase().contains("pandoc not found"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn export_still_refuses_before_opening_any_picker() {
        // The probe and the unnamed-buffer refusal stay AHEAD of the picker: there is no
        // point choosing a destination for an export that cannot run.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        run_export_with_probe(&mut e, &fs, "html", &tx, || true);
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
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    ext: &str,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    run_export_with_probe(editor, fs, ext, msg_tx, probe_pandoc)
}

/// `run_export` with an INJECTABLE pandoc probe.
///
/// The probe seam exists because the merge gate runs on machines without pandoc: a test
/// that depends on the host having it is an environment assumption that fails the gate
/// rather than the code. Production passes `probe_pandoc` (still `OnceLock`-cached);
/// tests pass a closure.
pub(crate) fn run_export_with_probe(
    editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    ext: &str,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    pandoc_available: impl Fn() -> bool,
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
    if !pandoc_available() {
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
    editor.open_destination_picker(fs, msg_tx,
        crate::file_browser::DestinationPurpose::Export { ext: ext.to_owned() }, dir, field);
}
```

4. **Thread `fs` into `do_export` and its spawned thread.** Task 7 moved `run_pandoc`'s
   `tmp.exists()` verification probe onto the seam, and that probe runs **inside** the thread
   `do_export` spawns — so the owned handle has to reach it. Pandoc's own `-o` write stays exempt
   under clause (a); the probe is ours.

```rust
pub(crate) fn do_export(
    editor: &mut crate::editor::Editor,
    ext: &str,
    target: &Path,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    overwrite_confirmed: bool,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
) {
    let sink = sink_for_ext(ext);
    let buffer_id = editor.active().id;
    let stdin = editor.active().document.buffer.to_string();
    let target = target.to_path_buf();
    let msg_tx = msg_tx.clone();
    let opts = ExportOpts {
        typography: editor.export_cfg.typography,
        pdf_engine: editor.export_cfg.pdf_engine.clone(),
    };
    // OWNED clone — the closure is `'static + Send`, so a borrowed `&dyn Fs` cannot cross.
    let fs = std::sync::Arc::clone(fs);

    std::thread::spawn(move || {
        let result = guarded_export(|| run_pandoc(sink, &stdin, &target, &opts, &fs));
        let _ = msg_tx.send(crate::app::Msg::ExportDone {
            buffer_id, target, result, overwrite_confirmed,
        });
    });
}
```

   `run_pandoc` gains a matching `fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>` parameter
   and uses it for the `tmp.exists()` probe:

```rust
            if !crate::fsx::exists_via(&**fs, &tmp) {
                return Err(FilterError::ExportWrite(
                    format!("pandoc did not write {}", tmp.display())
                ));
            }
```

   The other `do_export` caller — `prompts::resolve_prompt`'s `OverwriteExport` arm — passes its own
   `fs` (which Task 8 gave it).

5. **Wire the `Export` commit arm.** Task 21's `commit_destination` already dispatches
   `DestinationPurpose::Export`: if the resolved target exists it sets
   `editor.pending_export = Some(PendingExport { ext, target })` and opens
   `Prompt::export_overwrite`; otherwise it calls
   `do_export(editor, &ext, &resolved, msg_tx, false, fs)`. **`apply_export_done`'s TOCTOU re-check
   is unchanged.** Verify that arm compiles against the signature above.

6. **Update the four `export_*` command handlers in `registry.rs`** — compile blockers otherwise.
   All four call `run_export` and all four need the seam:

```rust
        r.register("export_html", "Export HTML", Some(MenuCategory::Export), |c| {
            crate::export::run_export(c.editor, &c.fs, "html", &c.msg_tx);
            CommandResult::Handled
        });
        // …identically for export_docx ("docx"), export_pdf ("pdf"), export_tex ("tex").
```

   Four handlers, one changed signature — the shape that made `registry.rs` the highest-risk file
   for signature ripple. Re-read the whole handler table here rather than trusting a grep for
   `run_export` alone.

7. **Run — expect green:**

```
cargo test -p wordcartel --lib export:: jobs_apply:: registry::
```

Expected: `test result: ok`. **`export::tests::export_refuses_scratch_buffer` and the three
`apply_export_done` tests must still pass unmodified.**

8. **Commit:** `feat(c5): give export a pre-seeded destination picker (Enter-through)`

---

## Phase F — Commands and closeout (Tasks 23–26)

### Task 23 — Recents: the `recents` module and `Editor::open_recents`

#### Files

- Create: `wordcartel/src/recents.rs`
- Modify: `wordcartel/src/lib.rs`

#### Interfaces

**Consumes** (Tasks 6, 7, 12):

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

pub(crate) fn open_recent(editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>);
pub(crate) fn ranked_paths(session: &crate::state::SessionState) -> Vec<std::path::PathBuf>;
```

#### Steps

1. **Write the failing test** in `recents.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::press_char_fb;

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
    // Through the SEAM, not `state::load()` — that wrapper hardcodes `RealFs`, and reaching
    // around an `fs` this function was handed is the exact defect condition in Global
    // Constraints. `state_dir()` is directory provisioning (clause (b)) and stays raw.
    let session = match crate::swap::state_dir() {
        Ok(dir) => crate::state::load_in_with_fs(fs, &dir),
        Err(_) => crate::state::SessionState::default(),
    };
    let rows = rows_from(&session, fs);
    if rows.is_empty() {
        editor.set_status(crate::status::StatusKind::Info, "No recent files");
        return;
    }
    editor.open_recents(rows);
}
```

4. **Compute availability off the UI thread.** The spec puts the existence check on the listing
   thread — a recents list spanning a hung network mount would otherwise block the input loop on
   `is_file_via` for every row, which is the exact hazard §6.3 exists to prevent. `open_recent`
   therefore spawns, exactly like a directory listing, and the rows arrive via the existing
   `Msg::ListingDone` epoch discipline:

```rust
pub(crate) fn open_recent(editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>)
{
    let session = match crate::swap::state_dir() {
        Ok(dir) => crate::state::load_in_with_fs(&**fs, &dir),
        Err(_) => crate::state::SessionState::default(),
    };
    // Paths and seq ranking are pure — computed here. Only the per-row `is_file` probe,
    // which touches the filesystem once per entry, goes to the thread.
    let paths = ranked_paths(&session);
    if paths.is_empty() {
        editor.set_status(crate::status::StatusKind::Info, "No recent files");
        return;
    }
    editor.open_recents_pending(paths.clone());       // shows rows immediately, availability TBD
    let epoch = crate::file_browser::next_epoch();
    if let Some(fb) = editor.file_browser.as_mut() { fb.awaiting_epoch = epoch; }
    let fs = std::sync::Arc::clone(fs);
    let tx = msg_tx.clone();
    std::thread::spawn(move || {
        let rows: Vec<RecentRow> = paths.into_iter()
            .map(|path| { let available = crate::fsx::is_file_via(&*fs, &path);
                          RecentRow { path, available } })
            .collect();
        let _ = tx.send(crate::app::Msg::RecentsProbed { epoch, rows });
    });
}
```

   with a `Msg::RecentsProbed { epoch, rows }` arm delegating to
   `recents::apply_recents_probed(e, epoch, rows)` — named and shaped after
   `file_browser::apply_listing_done`, since it has the identical stale-epoch obligation:

```rust
/// Apply a completed availability probe. Discards a stale epoch — a probe for a recents
/// list the writer has already closed and reopened must never re-mark the CURRENT rows.
///
/// Re-marks IN PLACE rather than calling `open_recents` again. Reopening would run
/// `close_all`, reset `query` to empty, and reset `selected` — so a probe landing a beat
/// after the writer started typing would wipe their filter and move their cursor. The probe
/// carries availability and nothing else; it must change availability and nothing else.
pub(crate) fn apply_recents_probed(editor: &mut crate::editor::Editor, epoch: u64,
    rows: Vec<RecentRow>)
{
    let Some(fb) = editor.file_browser.as_mut() else { return };
    if fb.awaiting_epoch != epoch { return; }
    let unavailable: std::collections::HashSet<String> = rows.iter()
        .filter(|r| !r.available)
        .map(|r| r.path.to_string_lossy().into_owned())
        .collect();
    for entry in fb.entries.iter_mut() {
        if unavailable.contains(&entry.name) { entry.kind = crate::fsx::EntryKind::Unknown; }
    }
}
```

   **Three consumers of the extended shapes must be updated in this task:**

   * `Msg`'s hand-written exhaustive `Debug` impl in `app.rs` gains
     `Msg::RecentsProbed { epoch, rows } => write!(f, "RecentsProbed({epoch}, {} rows)", rows.len())`
     — the same non-exhaustive-match defect `ListingDone` had in Task 13.
   * `reduce_dispatch` gains the dispatch arm beside `Msg::ListingDone`.
   * **`BrowseMode::filter_text` (Task 18) matches only `Select` and `Destination`** — adding
     `Recents` makes it non-exhaustive. Its `Recents` arm returns `query`, like `Select`:

```rust
    pub fn filter_text<'a>(&'a self, query: &'a str) -> &'a str {
        match self {
            BrowseMode::Select | BrowseMode::Recents => query,
            BrowseMode::Destination { field, .. } => field,
        }
    }
```

   **Test the probe path.** This is the newest concurrency surface in the effort, and it has the
   same stale-epoch hazard `apply_listing_done` has — with none of its coverage until now. The
   test drives `apply_recents_probed` directly for the epoch discipline (the thread itself is
   just a `map` over `is_file_via`, and spawning it would make the test scheduling-dependent for
   no added coverage), and asserts the pre-probe state that proves the open did not block.

```rust
    #[test]
    fn recents_open_unblocked_and_a_stale_probe_never_re_marks_the_rows() {
        // Two properties in one arrangement because they share it:
        //  (a) OPENING shows every row selectable immediately — the writer never waits on one
        //      `stat` per row, which is the whole reason the probe is off-thread (§6.3).
        //  (b) A STALE probe is DISCARDED. Without the epoch check, a slow probe for a list
        //      the writer already closed and reopened would re-mark the CURRENT rows with the
        //      PREVIOUS list's availability — rows greying out for the wrong files.
        //
        // FAIL-VERIFY (two): (a) make `open_recents_pending` probe inline with `is_file_via`
        // and mark rows `Unknown` — the pre-probe assertion fails; (b) delete the
        // `epoch != fb.awaiting_epoch` guard from the arm — the stale assertion fails.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let gone = std::path::PathBuf::from("/nonexistent/ch1.md");
        e.open_recents_pending(vec![gone.clone()]);

        // (a) Pre-probe: shown, and selectable — NOT yet marked unavailable.
        let fb = e.file_browser.as_ref().expect("the picker opened");
        assert_eq!(fb.entries.len(), 1, "the row is on screen before any probe lands");
        assert_eq!(fb.entries[0].kind, crate::fsx::EntryKind::File,
            "availability is UNKNOWN-but-optimistic pre-probe, so the open never blocked");
        let live_epoch = fb.awaiting_epoch;

        // (b) A probe stamped with a DIFFERENT epoch must change nothing.
        crate::recents::apply_recents_probed(&mut e, live_epoch.wrapping_sub(1),
            vec![crate::recents::RecentRow { path: gone.clone(), available: false }]);
        assert_eq!(e.file_browser.as_ref().unwrap().entries[0].kind,
            crate::fsx::EntryKind::File, "a STALE probe is discarded, not applied");

        // (c) A live probe re-marks availability and NOTHING ELSE. The writer types while the
        // probe is in flight; when it lands their filter and cursor must survive. This is why
        // the arm re-marks in place instead of calling `open_recents` again — that path runs
        // `close_all` and resets `query`, silently eating the keystrokes they just made.
        //
        // FAIL-VERIFY: implement the arm as `editor.open_recents(rows)`, watch the query
        // assertion fail while the availability one still passes.
        e.file_browser.as_mut().unwrap().query.push_str("ch1");
        crate::recents::apply_recents_probed(&mut e, live_epoch,
            vec![crate::recents::RecentRow { path: gone, available: false }]);
        let fb = e.file_browser.as_ref().unwrap();
        assert_eq!(fb.entries[0].kind, crate::fsx::EntryKind::Unknown,
            "the LIVE probe marks the row unavailable");
        assert_eq!(fb.query, "ch1", "and leaves the in-flight filter the writer typed intact");
    }
```

5. **Declare the module** in `lib.rs` and add `Editor::open_recents`.

   **Recents is a third `BrowseMode` variant, explicitly.** An earlier draft encoded "this is
   recents" as `listing.is_empty() && !entries.is_empty()` plus an early return in `rederive`. The
   gate ruled against it and was right twice over: the early return would preserve the rows but
   **could not narrow them**, so typing would silently fail to filter — worse than the problem it
   solved — and encoding a mode as an implicit state pattern is exactly what this effort has spent
   fourteen rounds removing. A variant costs one arm in each `match` and says what it means.

```rust
// crate::file_browser — BrowseMode gains a third variant (Task 18 introduced the enum).
pub enum BrowseMode {
    Select,
    Destination { purpose: DestinationPurpose, field: String, field_cursor: usize },
    /// Rows are SYNTHESIZED from the session store, not read from a directory. No listing
    /// thread is spawned, `..` is not shown, and descend is meaningless — every row is a
    /// document. Filtering ranks the rows themselves.
    Recents,
}
```

   `file_browser_listing::rederive` gains a `Recents` arm that fuzzy-ranks `fb.entries` in place
   (via `palette::fuzzy_filter` over the path strings) instead of deriving from `fb.listing`, so
   typing narrows the list exactly as it does elsewhere. Assert it: **typing must narrow the recents
   list, not clear it** — the failure mode the rejected guard would have shipped.

```rust
    /// Open the recents picker with availability NOT yet probed — every row starts `File`
    /// (selectable) and is re-marked when `Msg::RecentsProbed` lands. Splitting this from
    /// `open_recents` is what lets the probe run off-thread: the writer sees their list
    /// immediately instead of waiting on one `stat` per row.
    pub fn open_recents_pending(&mut self, paths: Vec<std::path::PathBuf>) {
        self.open_recents(paths.into_iter()
            .map(|path| crate::recents::RecentRow { path, available: true })
            .collect());
    }

    /// Present recents through the existing picker. Rows become `FileEntry` values, so nav,
    /// fuzzy ranking, windowing, mouse hover, and the painter all work unchanged.
    ///
    /// UNAVAILABLE rows carry `kind == EntryKind::Unknown`, which the picker ALREADY renders
    /// marked and refuses on Enter (Task 14) — so "greyed and not selectable" needs no new
    /// input handling, no new refusal branch, and no new painter path.
    pub fn open_recents(&mut self, rows: Vec<crate::recents::RecentRow>) {
        crate::overlays::close_all(self);
        self.pending_keys.clear(); self.pending_mark = None;
        let entries: Vec<crate::file_browser::FileEntry> = rows.iter().map(|r| {
            crate::file_browser::FileEntry {
                // Full path as the label — recents span directories, so a bare file name
                // would be ambiguous.
                name: r.path.to_string_lossy().into_owned(),
                kind: if r.available {
                    crate::fsx::EntryKind::File
                } else {
                    crate::fsx::EntryKind::Unknown
                },
                is_symlink: false,
                broken: false,
            }
        }).collect();
        let total = entries.len();
        self.file_browser = Some(crate::file_browser::FileBrowser {
            dir: std::env::current_dir().unwrap_or_default(),
            query: String::new(),
            mode: crate::file_browser::BrowseMode::Recents,
            listing: Vec::new(), total_seen: total, unreadable: 0,
            entries, disclosure: Default::default(), selected: 0, scroll_top: 0,
            awaiting_epoch: 0, pending_dir: None,
        });
    }
```

   Selecting an available row routes through the picker's ordinary `EnterOutcome::Open` arm into
   `workspace::open_as_new_buffer`, inheriting the dirty-guard and resume behaviour. An unavailable
   row hits `EnterOutcome::Refuse` and sets the Sticky Warning — no special-casing.

6. **Add the narrowing test** — the property that justified `BrowseMode::Recents` over the rejected
   implicit-state guard, and which nothing currently covers:

```rust
    #[test]
    fn typing_narrows_the_recents_list_rather_than_clearing_it() {
        // THE reason `Recents` is an explicit variant. The rejected design used an early
        // return in `rederive`, which preserved the rows but could not FILTER them — typing
        // would have silently done nothing. This is the assertion that would have caught it.
        //
        // FAIL-VERIFY (two, because there are two ways to break this): (a) make `rederive`'s
        // Recents arm return early instead of ranking — all four rows survive the filter;
        // (b) drop the printable-char arm from `file_browser_intercept`'s Recents branch —
        // the query never fills and all four survive. The keystroke goes through the REAL
        // intercept via `press_char_fb`: pushing to `fb.query` and calling `rederive` by hand
        // proves the ranker works, NOT that typing reaches it, and (b) would pass vacuously.
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        e.open_recents(vec![
            RecentRow { path: "/w/chapter-one.md".into(), available: true },
            RecentRow { path: "/w/chapter-two.md".into(), available: true },
            RecentRow { path: "/w/notes.md".into(),       available: true },
            RecentRow { path: "/w/outline.md".into(),     available: false },
        ]);
        assert_eq!(e.file_browser.as_ref().expect("open").entries.len(), 4, "precondition");

        for c in "chapter".chars() { press_char_fb(&mut e, &fs, &tx, c); }

        let names: Vec<String> = e.file_browser.as_ref().unwrap()
            .entries.iter().map(|r| r.name.clone()).collect();
        assert_eq!(names.len(), 2, "the list NARROWED, it did not clear or stay whole: {names:?}");
        assert!(names.iter().all(|n| n.contains("chapter")), "{names:?}");
    }
```

7. **Run — expect green:**

```
cargo test -p wordcartel --lib recents:: file_browser_listing::
```

8. **Commit:** `feat(c5): add open_recent sourced from the LRU session store`

---

### Task 24 — Seven commands, two persisted options, contract conformance

#### Files

- Modify: `wordcartel/src/registry.rs` (seven registrations, **before `plugin_list`**)
- Modify: `wordcartel/src/editor.rs` (the two setters)
- Modify: `wordcartel/src/settings.rs` (`SettingsSnapshot` + overrides mirror)
- Modify: `wordcartel/src/config.rs` (config seeding for both options)

#### Interfaces

**Consumes** (Tasks 12, 18, 23):

```rust
// crate::recents — the handler `open_recent` dispatches to. It EXISTS before this task
// registers a command against it; an earlier draft had the registration first, which would
// not compile.
pub(crate) fn open_recent(editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>);
pub(crate) fn ranked_paths(session: &crate::state::SessionState) -> Vec<std::path::PathBuf>;

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
        // `Registry::commands()` yields `(CommandId, &CommandMeta)` — destructure the pair.
        let ids: Vec<&str> = reg.commands().map(|(id, _)| id.0).collect();
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
            // Async form (Task 23): the availability probe runs off-thread, so this needs the
            // owned Arc and the sender, not a borrowed `&dyn Fs`.
            crate::recents::open_recent(c.editor, &c.fs, &c.msg_tx);
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
cargo test -p wordcartel --lib registry:: settings:: palette:: menu:: keymap:: overlays::
```

`overlays::` is in the list deliberately. Spec §13.4 says C5 inherits the H21 guardrails
"unchanged by not adding an `OverlayId` variant" — that premise holds (C5 adds no variant; it
reuses the FileBrowser overlay). But *inheriting* them is not the same as *running* them, and C5
adds real state to that overlay: destination mode's field, the `Recents` variant, new key arms.
`overlays::tests::every_overlay_consumes_moved_without_panic_or_data_loss` is precisely the guard
for a motion event reaching that new state — and A21's lesson was that motion regressions do not
announce themselves.

Expected: `test result: ok`, including
`overlays::tests::render_order_is_exactly_the_frame_overlays`,
`overlays::tests::every_overlay_consumes_moved_without_panic_or_data_loss`,
`settings::tests::every_persisted_setting_has_a_command`,
`palette::tests::palette_is_exhaustive_over_the_registry`,
`palette::tests::palette_is_exhaustive_over_a_plugin_loaded_registry`,
`menu::tests::parameterized_plugin_command_and_plugin_list_satisfy_law3_law4`,
`menu::tests::custom_bind_surfaces_in_menu_and_palette`,
`keymap::tests::hints_reresolve_on_preset_switch`.

7. **Commit:** `feat(c5): add the seven file-interface commands and two persisted filter options`

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

**Consumes** (Task 7) — `state::StateEntry` and `swap::SwapHeader` are pre-existing; the only
C5-produced dependency is the seam-migrated fingerprint path this task's stamping sits alongside:

```rust
// crate::save
pub(crate) fn fingerprint_with_fs(fs: &dyn crate::fsx::Fs, path: &Path)
    -> Option<FileFingerprint>;

// pre-existing in the tree, unchanged by C5 — listed because this task edits both:
// crate::state
pub struct StateEntry { pub cursor: usize, pub scroll: usize,
    pub marks: std::collections::BTreeMap<String, usize>, pub mtime: i64, pub size: u64,
    pub seq: u64, pub folds: Vec<usize>, pub block: Option<(usize, usize)> }
// crate::swap
pub struct SwapHeader { pub realpath: Option<String>, pub load_mtime_secs: Option<u64>,
    pub load_size: Option<u64>, pub content_hash: u64, pub version: u64,
    pub ts_ms: u64, pub pid: u32 }
pub fn serialize(h: &SwapHeader, body: &str) -> String;
pub fn parse(text: &str) -> Option<(SwapHeader, String)>;
```

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
        // Stability across a MUTATION, not across two reads of the same expression — the
        // latter is true unconditionally for a Copy field and asserts nothing.
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let id = e.active().document.id;
        e.active_mut().document.version += 1;
        e.active_mut().document.saved_version = Some(e.active().document.version);
        assert_eq!(e.active().document.id, id,
            "the id is minted once and survives edits/saves — it is not re-minted per version");
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
    fn swap_header_with_an_id_round_trips() {   // renamed: no old reader is exercised here;
        // backward compatibility is covered by `pre_c5_swap_without_an_id_line_still_parses`.
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

3. **Make the two field additions non-breaking BEFORE adding the field.** `StateEntry` and
   `SwapHeader` each gain `id: Option<String>`, which breaks every existing struct literal —
   `state.rs`, `session_restore.rs`, `swap.rs`, `prompts.rs`, plus literals this plan adds in Tasks
   17 and 23. Enumerating them is the third instance of the shared-type-field class
   (`DirEntryInfo::raw_name` was the last), so fix it structurally instead:

```rust
// state.rs — StateEntry already has #[serde(default)]; add the derive and it composes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateEntry { /* … existing fields …, */ pub id: Option<String> }

// swap.rs
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SwapHeader { /* … existing fields …, */ pub id: Option<String> }
```

   Then convert every existing literal to functional-update form **once**:

```rust
    StateEntry { cursor: 3, scroll: 0, mtime: 1, size: 2, seq: 1, ..Default::default() }
    SwapHeader { realpath: None, content_hash: h, version: 1, ts_ms: 5, pid: 9,
                 ..Default::default() }
```

   After this, a future field addition costs nothing at any construction site. Do this migration
   first; the `id` field lands on top of it. **This is the structural answer to a class that has now
   cost three rounds** — enumerating literals a fourth time would just defer it again.

4. **Add `DocumentId`** to `editor.rs`:

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

5. **Stamp it.** `StateEntry` gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub
   id: Option<String>`, populated by `persist_session` from the active document. `SwapHeader` gains
   `pub id: Option<String>`; `serialize` emits `id: {}` using `opt_str`, and `parse` gains an
   `"id" => id = if v == "-" { None } else { Some(v.to_string()) }` arm.

> **Nothing reads either value.** A document reopened by any route mints a fresh id; ids do not
> follow identity across routes or restarts. That is the ratified scope — §12.6 records what S3 must
> specify to make the id load-bearing.

6. **Assert the id is actually WIRED, not merely parseable.** The four tests above cover minting
   and round-tripping; every one of them passes with the stamping entirely unimplemented. These two
   close that:

```rust
    // in session_restore.rs
    #[test]
    fn persist_session_stamps_the_active_documents_id() {
        // FAIL-VERIFY: remove the `id:` assignment in `persist_session`, watch this fail.
        let p = std::env::temp_dir().join(format!("wc-idstamp-{}.md", std::process::id()));
        std::fs::write(&p, b"x\n").expect("seed");
        let e = crate::editor::Editor::new_from_text("x\n", Some(p.clone()), (80, 24));
        let expected = e.active().document.id.to_hex();
        let mut s = crate::state::SessionState::default();
        let cfg = crate::config::Config::default();
        persist_session_for_test(&mut s, &e, &cfg, 1);
        let key = std::fs::canonicalize(&p).expect("canon").to_string_lossy().into_owned();
        assert_eq!(s.entries[&key].id.as_deref(), Some(expected.as_str()),
            "the entry must carry the ACTIVE document's id, not a fresh or absent one");
        let _ = std::fs::remove_file(&p);
    }

    // in swap.rs
    #[test]
    fn build_header_stamps_the_active_documents_id() {
        // FAIL-VERIFY: drop `id` from `build_header`, watch this fail.
        let p = scratch();
        let e = crate::editor::Editor::new_from_text("body\n", Some(p.clone()), (80, 24));
        let expected = e.active().document.id.to_hex();
        let h = build_header(&e, "body\n", 1);
        assert_eq!(h.id.as_deref(), Some(expected.as_str()),
            "the swap header must carry the document's id");
        // …and it survives a serialize/parse round trip in situ.
        let (h2, _) = parse(&serialize(&h, "body\n")).expect("round-trips");
        assert_eq!(h2.id, h.id);
        let _ = std::fs::remove_file(&p);
    }
```

7. **Run — expect green:**

```
cargo test -p wordcartel --lib editor:: state:: swap:: session_restore::
```

Expected: `test result: ok`, including all four id tests. Every existing swap round-trip test must
still pass.

8. **Commit:** `feat(c5): mint and stamp a DocumentId without keying anything on it`

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
/// The recovery artifacts DELIBERATELY kept because they may hold unsaved work, with the
/// identifying detail the spec requires: the realpath the swap recorded, and when it was
/// written. Both come from the parsed `SwapHeader` that `swap_is_cleanable` already reads.
///
/// Visibility only — `cleanable_recovery_files` and its fail-closed rules are untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeptRecoverable { pub realpath: String, pub ts_ms: u64 }

pub(crate) fn kept_recoverable(fs: &dyn crate::fsx::Fs, dir: &Path,
    protected: &std::collections::HashSet<PathBuf>) -> Vec<KeptRecoverable>;

// crate::prompts — `raise_clean_recovery` is `(editor, files)` in the tree: no seam handle,
// no dir. The kept list is computed by the CALLER (`open_clean_recovery`, which already has
// both) and passed in, rather than recomputed here.
fn raise_clean_recovery(editor: &mut crate::editor::Editor,
    files: Vec<std::path::PathBuf>, kept: Vec<crate::swap::KeptRecoverable>);
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
        // Scan the dir the fixtures actually live in. `make_doc_with_swap` writes to
        // `swap_path(...)` — the process state dir — so scanning a fresh `unique_dir` would
        // have measured an empty directory and asserted about swaps that were never in it.
        let dir = state_dir().expect("state dir");
        let (p_ok, sp_ok) = make_doc_with_swap("same\n", "same\n", DEAD_PID);
        let (p_bad, sp_bad) = make_doc_with_swap("file\n", "UNSAVED\n", DEAD_PID);
        let protected = std::collections::HashSet::new();
        let cleanable = cleanable_recovery_files(&crate::fsx::RealFs, &dir, &protected);
        let kept = kept_recoverable(&crate::fsx::RealFs, &dir, &protected);
        assert!(cleanable.contains(&sp_ok), "the valueless swap is still offered");
        assert!(!cleanable.contains(&sp_bad), "the diverged swap is still NEVER offered");
        // CONTAINMENT, not an exact count: the shared state dir may hold unrelated diverged
        // swaps from other tests or a prior real session, and `kept.len() == 1` would fail
        // for reasons that have nothing to do with this behaviour.
        let real_bad = std::fs::canonicalize(&p_bad).expect("canon").display().to_string();
        let mine = kept.iter().find(|k| k.realpath.contains(&real_bad))
            .expect("the diverged one is reported so the writer knows it exists");
        assert!(!kept.iter().any(|k| k.realpath.contains(
                &std::fs::canonicalize(&p_ok).expect("canon").display().to_string())),
            "and the valueless one is NOT reported as kept");
        assert!(mine.ts_ms > 0, "and it carries the timestamp");
        for f in [&sp_ok, &sp_bad, &p_ok, &p_bad] { let _ = std::fs::remove_file(f); }
        let _ = std::fs::remove_dir_all(&dir);
    }
```

```rust
    // in e2e.rs — the whole-effort journey, driven through the real reduce loop.
    #[test]
    fn journey_open_save_export_saveas_reopen() {
        let d = std::env::temp_dir().join(format!("wc-c5-journey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        std::fs::write(d.join("existing.md"), b"already here\n").expect("seed");

        let mut h = Harness::new("first draft\n", None, (100, 30));

        // 1. OPEN the picker and confirm it lists the seeded directory.
        h.editor.borrow_mut().open_file_browser(&h.fs, &h.tx, d.clone());
        h.pump_listing();
        assert!(h.editor.borrow().file_browser.as_ref().expect("picker")
            .entries.iter().any(|e| e.name == "existing.md"),
            "the picker lists what is really there");
        h.key(KeyCode::Esc);

        // 2. FIRST SAVE through the destination picker. Type a name with NO extension and
        //    let policy append `.md`; the footer must show the post-policy target first.
        h.editor.borrow_mut().open_destination_picker(&h.fs, &h.tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), String::new());
        h.pump_listing();
        for c in "chapter one".chars() { h.key(KeyCode::Char(c)); }
        {
            let e = h.editor.borrow();
            let footer = crate::file_browser::footer_target(&*h.fs, e.file_browser.as_ref().unwrap())
                .expect("destination mode shows a target");
            assert!(footer.contains(&d.join("chapter one.md").display().to_string()),
                "the writer sees the resolved, post-policy target BEFORE committing: {footer}");
        }
        h.key(KeyCode::Enter);
        h.drain_jobs();
        assert_eq!(std::fs::read_to_string(d.join("chapter one.md")).expect("written"),
            "first draft\n");
        assert_eq!(h.editor.borrow().active().document.path.as_deref(),
            Some(d.join("chapter one.md").as_path()), "buffer rekeyed to the CHOSEN path");

        // 3. EXPORT with a destination — bare Enter reproduces the derived path.
        h.editor.borrow_mut().open_destination_picker(&h.fs, &h.tx,
            crate::file_browser::DestinationPurpose::Export { ext: "html".into() },
            d.clone(), "chapter one.html".into());
        h.pump_listing();
        h.key(KeyCode::Enter);
        // BOUNDED wait, not an immediate drain: `do_export` spawns a thread, so reading the
        // channel the instant after Enter races it and this assertion would pass or fail on
        // scheduling. Mirrors T21's `export_commits_end_to_end_from_enter_through`.
        assert!(h.await_msg(4, |m| matches!(m,
            crate::app::Msg::ExportDone { target, .. } if target == &d.join("chapter one.html"))),
            "Enter-through dispatches the export for the seeded target");

        // 4. SAVE-AS to a new name — the status must NAME the full path.
        h.editor.borrow_mut().open_destination_picker(&h.fs, &h.tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), "chapter one v2.md".into());
        h.pump_listing();
        h.key(KeyCode::Enter);
        h.drain_jobs();
        let status = h.editor.borrow().status_text().to_string();
        assert!(status.contains(&d.join("chapter one v2.md").display().to_string()),
            "a successful Save-As names where the bytes went: {status}");

        // 5. REOPEN via recents — the rescue path.
        //
        // Built from a session store this test OWNS. `crate::state::load()` would read the
        // developer's real `$XDG_STATE_HOME` — the journey never persists a session, so the
        // assertion would depend on ambient machine state and could pass or fail for reasons
        // nothing to do with C5.
        // Driven through the real COMMAND, then the real Enter — `rows_from` alone would pass
        // with a broken `open_recent`, a broken Enter arm, or missing registry wiring.
        let before = h.editor.borrow().active().document.path.clone();
        h.editor.borrow_mut().active_mut().document.path = None;   // a different buffer
        h.dispatch_command("open_recent");
        h.pump_recents();
        assert!(h.editor.borrow().file_browser.is_some(), "the command opened the recents picker");
        {
            let e = h.editor.borrow();
            let fb = e.file_browser.as_ref().unwrap();
            let idx = fb.entries.iter().position(|r| r.name.ends_with("chapter one v2.md"))
                .expect("the just-saved document is listed, ranked by seq");
            drop(e);
            h.editor.borrow_mut().file_browser.as_mut().unwrap().selected = idx;
        }
        h.key(KeyCode::Enter);
        assert_eq!(h.editor.borrow().active().document.path, before,
            "Enter on a recents row REOPENS that document — the buffer actually changed");

        let _ = std::fs::remove_dir_all(&d);
    }
```

The journey needs three small `Harness` additions, all mechanical: an `fs` field (the `Arc` built
once), a `pump_listing()` that drains one `Msg::ListingDone` into `apply_listing_done`, and an
`await_msg()` predicate wait so step 3 can assert on a dispatch that crosses a thread boundary.
`drain_jobs()` already exists.

```rust
    /// Waits up to 5s for each of at most `n` messages, returning true as soon as one
    /// satisfies `pred`. A plain `try_iter()` drain would race any thread-spawning
    /// dispatch (`do_export`) and make the assertion scheduling-dependent.
    fn await_msg(&self, n: usize, pred: impl Fn(&crate::app::Msg) -> bool) -> bool {
        std::iter::from_fn(|| self.rx.recv_timeout(std::time::Duration::from_secs(5)).ok())
            .take(n)
            .any(|m| pred(&m))
    }
```

2. **Run — expect failure**, then implement.

3. **Add `kept_recoverable`** to `swap.rs` — it reuses `recovery_file_is_cleanable`'s inverse
   over the same enumeration. **`cleanable_recovery_files`, `swap_is_cleanable`,
   `recovery_path_still_cleanable`, and their fail-closed rules are untouched**; this only counts.

4. **Surface it** in `prompts::raise_clean_recovery`'s modal text. The spec requires the kept
   entries carry **recorded realpath and timestamp**, so the message is specified exactly rather
   than left to judgement:

```rust
/// Kept-because-recoverable detail: the realpath the swap recorded, and when it was written.
/// Both come from the parsed `SwapHeader` — `realpath` and `ts_ms` — which
/// `swap_is_cleanable` already reads to reach its verdict.
pub(crate) struct KeptRecoverable { pub realpath: String, pub ts_ms: u64 }

pub(crate) fn kept_recoverable(fs: &dyn crate::fsx::Fs, dir: &Path,
    protected: &std::collections::HashSet<PathBuf>) -> Vec<KeptRecoverable>;
```

   `open_clean_recovery` — which holds `fs` and the state dir — computes both lists and passes
   them down; `raise_clean_recovery` only formats:

```rust
pub fn open_clean_recovery(editor: &mut crate::editor::Editor, fs: &dyn crate::fsx::Fs) {
    let (files, kept) = match crate::swap::state_dir() {
        Ok(dir) => {
            let protected = crate::swap::open_swap_paths(editor);
            (crate::swap::cleanable_recovery_files(fs, &dir, &protected),
             crate::swap::kept_recoverable(fs, &dir, &protected))
        }
        Err(_) => (Vec::new(), Vec::new()),
    };
    raise_clean_recovery(editor, files, kept);
}
```

   and the modal appends one line per kept entry beneath the delete count:

```rust
    let mut msg = format!("Delete {n} recovery file(s)?");
    if !kept.is_empty() {
        msg.push_str(&format!(
            "\n\nKeeping {} that may hold unsaved work:", kept.len()));
        for k in kept.iter().take(5) {
            msg.push_str(&format!("\n  {} (written {})",
                k.realpath, crate::status::format_ts(k.ts_ms)));
        }
        if kept.len() > 5 {
            msg.push_str(&format!("\n  …and {} more", kept.len() - 5));
        }
    }
```

   Assert it — with a body, like every other task:

```rust
    #[test]
    fn the_clean_recovery_modal_names_kept_recoverable_files() {
        // FAIL-VERIFY: drop the `kept` block from the message, watch the realpath assertion fail.
        let dir = state_dir().expect("state dir");
        let (p_ok, sp_ok)   = make_doc_with_swap("same\n", "same\n", DEAD_PID);   // valueless
        let (p_bad, sp_bad) = make_doc_with_swap("file\n", "UNSAVED\n", DEAD_PID); // diverged
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));

        crate::prompts::open_clean_recovery(&mut e, &crate::fsx::RealFs);

        let p = e.prompt.as_ref().expect("a confirm prompt is raised");
        let real_bad = std::fs::canonicalize(&p_bad).expect("canon").display().to_string();
        assert!(p.message.contains(&real_bad),
            "the modal must NAME the kept file by its recorded realpath: {:?}", p.message);
        assert!(p.message.to_lowercase().contains("unsaved work"),
            "and say why it is being kept: {:?}", p.message);
        assert!(!e.pending_clean.contains(&sp_bad),
            "the diverged swap is NEVER queued for deletion — the no-data-loss guarantee");
        assert!(e.pending_clean.contains(&sp_ok), "the valueless one still is");
        let _ = dir;
        for f in [&sp_ok, &sp_bad, &p_ok, &p_bad] { let _ = std::fs::remove_file(f); }
    }
```

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
| §10 recents | 23 |
| §11.1 `SaveTarget`, migration queue, drain | 16, 17 |
| §11.2 quit-drain hazard | 21 |
| §11.3 diverged-orphan visibility | 26 |
| §12 `DocumentId` | 25 |
| §13 command-surface conformance + registration order | 24 |
| §14 all asserted invariants | distributed as tabulated above |

**Placeholder scan:** no "TBD", no "similar to Task N", no "add appropriate error handling". Every
step shows code or an exact command.

**Signature consistency:** `FaultFs::new`, `FaultAt`, `FileStat`, `EntryKind`, `DirEntryInfo`,
`DirListing`, `Ctx.fs`, `DispatchCtx.fs` are declared once (Tasks 1–5) and referenced with identical
names and types in every consuming task.

**`Arc<FaultFs>` injection points** — paths fault-testable for the first time: `file::open` (T6),
`save::fingerprint` (T7), the dictionary append (T8), the browser and swap listings (T9), plugin
discovery (T10), the save worker (T16), the listing thread (T13).
