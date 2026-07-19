# C5 — Unified file interface: grounding packet for Fable

**Your job:** ground, imagine, and scope this effort, then AUTHOR the spec. You are expected to
form your OWN recommendations. Where you disagree with the controller's recommendations or with
the human's fork answers, say so plainly and argue it — one fork (F5) is explicitly reopened by
the human and at least one other deserves scrutiny. Do not ratify by default.

---

## 0. Project context and hard constraints

**wordcartel** — markdown-first Rust terminal word processor. Binary `wcartel`.
Crates: `wordcartel-core` (pure, `#![forbid(unsafe_code)]`), `wordcartel-nlp`, `wordcartel` (shell).
ratatui 0.30 + crossterm. Functional-core / imperative-shell.

Durable constraints that bind this effort:

- **Top priority: instant typing, no data loss, no silent UI waits.**
- **No async runtime.** All background work is `std::thread` + `mpsc` via a job substrate
  (`jobs.rs`, single FIFO worker thread `wcartel-jobs`). Verified this session: **zero `async fn`
  / `.await` in all three crates**; `cargo tree -i tokio --target all` finds **no dependency path
  on any target**, and the release binary contains **0 tokio symbols**. tokio in `Cargo.lock` is a
  stale unreachable entry. `mio` is real but comes from crossterm's event polling.
- **Idle is free.** With no input and nothing animating, the input loop BLOCKS. No polling, no
  background disk writes at rest. Background work is edge-triggered by real content/state change,
  never level-triggered off wall-clock.
- **Resource behavior proportional to work.** Per-keystroke work is `O(visible)+O(edited)`, never
  `O(document)`.
- **Errors are typed enums surfaced to the status line.** Never to console (app owns the terminal).
- Hand-formatted repo, **no `cargo fmt`**. Dense house style, ~100 char lines, em-dashes in prose
  comments, no emoji in code.
- **Merge GATEs:** `cargo test` green; `cargo clippy --workspace --all-targets` clean (deny);
  `clippy::too_many_lines` threshold 100; `wordcartel/tests/module_budgets.rs` hub budgets;
  backlog-drift bijection test.
- **Advisory only, never blocking:** `cargo deny`, PTY smoke suite (`scripts/smoke/run.sh`).
- **Module structure law:** dispatchers delegate, they don't implement. New behavior enters through
  a **registration seam**, not by growing a hub. Counter-caveat: do NOT over-fragment; target is
  "one person can hold this module's single responsibility in their head."
- **Command-surface contract** (`docs/design/command-surface-contract.md`) is binding — see §5.

---

## 1. GROUNDING — filesystem access census (verified this session)

**All filesystem access lives in the `wordcartel` shell crate.** `wordcartel-core` and
`wordcartel-nlp` contain no `std::fs` usage at all. This bounds where a chokepoint must live.

### 1.1 The `Fs` seam today (`wordcartel/src/fsx.rs`)

```rust
trait Fs { create_excl, existing_mode, rename, sync_dir, remove_file }   // object-safe, pub(crate)
trait WriteSync { write_all, flush, set_mode, sync_all }
```

`RealFs` is the sole production impl; tests use `FaultFs`/`FaultHandle` to inject failures per step.

**The seam covers EXACTLY ONE operation:** `atomic_replace(fs, path, bytes, WriteOpts{mode, dir_fsync})`
— the create-temp(O_EXCL,0600) → write → set_mode → flush → fsync → rename → [dir-fsync] sequence,
with `TempGuard` RAII cleanup.

**It is write-only by design.** It does NOT cover: reads, directory listing, `canonicalize`,
`metadata`/stat, `remove_file` outside temp cleanup, or subprocess I/O.

Only three callers route through it: `file::save_atomic`/`save_atomic_bytes`, `swap::write_atomic`,
`settings::write_overrides`/`save_overrides`.

**Note: `Fs` already declares `rename` and `remove_file`, and two call sites that need them call
`std::fs` directly instead.**

### 1.2 Bounded-read helpers

- `file::bounded_read_opt(path, limit) -> Option<Vec<u8>>` — caps at `limit+1`, `None` on error/over-cap
- `swap::read_swap_capped` / `swap::read_file_capped_bytes` — same pattern, private to `swap.rs`
- Caps: `limits::MAX_OPEN_BYTES = 64 MiB`, `MAX_SESSION_BYTES = 8 MiB`, `PLUGIN_MAX_SOURCE_BYTES = 1 MiB`

### 1.3 Every touchpoint, by subsystem

| Subsystem | Symbol | Notes |
|---|---|---|
| Open | `file::open(path) -> Result<String, OpenError>` | stat pre-check + `.take(limit+1)`; refuses binary/NUL/invalid-UTF8/dir/oversize. **NOT through `Fs`** (reads are raw). Bounded: yes |
| Open | `editor::Buffer::from_file` | wraps `file::open`; `NotFound` → synthesized empty buffer |
| Save | `file::save_atomic(path, content)` | symlink refusal via `symlink_metadata`, skip-unchanged compare, then `fsx::atomic_replace`. Through `Fs`: yes |
| Save | `file::save_atomic_bytes` | fixed 0600, no skip-unchanged/UTF-8 check; used by export + session/overrides |
| Save | `save::do_save_to` | dispatches background `Job` (`JobKind::Save`); worker materializes rope→String |
| Save | `save::dispatch_save` | external-mod fingerprint check before dispatch; unnamed → `prompts::open_save_as` |
| Save-As | `prompts::open_save_as` | **typed-path minibuffer**, pre-filled with current dir. NOT a picker |
| Save-As | `prompts::expand_path` | `~/`-expansion + cwd-join |
| Save-As | `prompts::save_as_submit` → `perform_save_as` | overwrite-confirm if target exists |
| Write-Block | `prompts::block_write_submit` / `perform_block_write` | synchronous `file::save_atomic` of a byte slice; same typed-path minibuffer pattern. A separate, undocumented "export a selection" path |
| Swap | `swap::state_dir()` | `$XDG_STATE_HOME/wordcartel` (0700), `create_dir_all` + `set_permissions`. **NOT through `Fs`** |
| Swap | `swap::swap_path(doc_path)` | `sanitize(basename)-fnv1a64(canonicalize(path)).swp`; `scratch-{pid}.swp` for scratch |
| Swap | `swap::write_atomic` | → `fsx::atomic_replace` (0600, no dir-fsync); background `JobKind::SwapWrite`, debounced idle 2s / max 30s |
| Swap | `swap::assess(doc_path, current_bytes) -> RecoveryDecision` | `{OpenNormally, DiscardSilently, Prompt}`; content-hash comparison at startup |
| Swap | `swap::find_orphan_scratch_swap()` | scans state dir for `scratch-{pid}.swp` with dead pid (`/proc/{pid}`) |
| Swap | `swap::delete(doc_path)` | best-effort `remove_file` |
| Swap | `swap::cleanable_recovery_files` etc. | H5 "clean recovery" enumerator; fails-closed classification |
| Swap | deletion in `prompts::resolve_prompt(CleanRecovery)` | plain `std::fs::remove_file`, **NOT through `Fs`**; TOCTOU re-verify before each delete |
| Recovery | `recovery::LAST_GOOD: Mutex<Option<(Option<PathBuf>, Rope)>>` | updated on every `Editor::apply` |
| Recovery | `recovery::write_dump` → `swap::write_atomic` | `recovered-{name}-{pid}-{seq}.md`, 0600 |
| Recovery | `dump_on_panic()` | panic-hook path, `try_lock` only (never blocks) |
| Recovery | `dump_all_dirty(editor, dir)` | controlled shutdown (`ExitReason::InputLost`) |
| Fingerprint | `save::FileFingerprint{mtime, size, hash}`, `save::fingerprint(path)` | `metadata()` + separate `bounded_read_opt` content hash (BUG-2 guard). Over-cap → metadata-only, sentinel hash 0 |
| Config | `config::config_layer_paths` | XDG, nearest `.wordcartel.toml` walking up, `--config` override; existence via `Path::is_file` |
| Config | `config::load(paths)` | plain `std::fs::read_to_string` + `toml::from_str`. **Unbounded** |
| Theme | `theme_resolve::resolve_theme` | reads `theme.file` via plain `read_to_string`. **Unbounded** |
| Settings | `settings::save_overrides`/`write_overrides` | → `Fs` seam (0600, dir-fsync, parent create_dir_all+chmod-0700) |
| Settings | startup overrides + `--config` mask read | plain `read_to_string` (`app.rs`) |
| Dictionary | load at `app.rs` | `file::bounded_read_opt`; missing/invalid → empty (silent degrade) |
| Dictionary | `diagnostics_run::append_word_to_dict(path, word)` | **`OpenOptions::create(true).append(true)` + `writeln!` — NOT atomic, NOT capped, NO symlink guard, NOT through `Fs`.** Sole writer |
| Session | `state.rs` `SessionState{entries, scratch}` at `$XDG_STATE_HOME/wordcartel/session.toml` | `save()` → `file::save_atomic_bytes` (through `Fs`); `load()` bounded |
| Session | `state::file_identity(path)` | mtime+size staleness guard for resume/marks/folds/marked-block |
| Scratch | `SessionState.scratch` (`ScratchState{text, cursor}`) | persisted via `session_restore::persist_session`; ALSO has its own swap file |
| Export | `export::probe_pandoc()` | `pandoc --version`, cached `OnceLock<bool>` |
| Export | `export::derived_export_path(source, ext)` | **`source.with_extension(ext)` — path ALWAYS derived. NO prompt of any kind** |
| Export | `export::do_export` | dedicated `std::thread::spawn`; `filter::run_subprocess` pipes through pandoc (30s timeout, 64 MiB cap, `CancelFlag`) |
| Export | HTML `Capture` sink | → `jobs_apply::apply_export_done` → `file::save_atomic_bytes` (through `Fs`) |
| Export | docx/pdf/tex `WritesOutput` sink | pandoc writes `{stem}.tmp-{pid}.{ext}` via `-o`, then **plain `std::fs::rename`, NOT through `Fs`** |
| Export | overwrite | prompts at dispatch; TOCTOU re-check at completion refuses to clobber |
| Browser | `file_browser::rebuild_entries(fb)` | **synchronous `std::fs::read_dir` on the UI thread**, inline with the keystroke. Sorted dirs-then-files, substring-filtered. **No cap, no debounce, no thread.** THE ONLY directory-listing code in the workspace |
| Plugins | `plugin::load::discover(dir, disable)` | `read_dir` scan, `bounded_read_opt` at 1 MiB, UTF-8 validated, deterministic. **No `wc.fs` API exposed to Lua** |

### 1.4 Subprocess spawns

| Spawner | Command | FS relevance |
|---|---|---|
| `export.rs` | `pandoc --version`, `pandoc -f … -o <tmp>` | pandoc writes tmp directly, outside `Fs` |
| `harper_ls.rs` | `harper-ls --stdio` | long-lived LSP child; reads/writes its own `userDictPath` **out of process** |
| `clipboard.rs` | xclip/wl-copy/etc. | stdin/stdout only |
| `filter.rs` `run_subprocess` | user-typed command via `sh -c` | **arbitrary file access, outside any chokepoint** |

Threading: `jobs::ThreadExecutor` single FIFO worker (`wcartel-jobs`); `wcartel-harper-read`;
export's ad hoc `std::thread::spawn`. 14 spawn sites total. The file-browser listing is the one
FS-adjacent operation that is neither threaded nor job-queued.

### 1.5 Gaps found (access that arguably should exist, or is non-uniform)

- **No picker for Save-As or export.** Save-As/Write-Block use a bare typed-path minibuffer.
  **Export has no prompt at all** — user cannot choose filename or directory. Export is a headline
  feature.
- **No "new file in a chosen directory"** — `workspace::new_empty_buffer` is in-memory only.
- **Personal dictionary write bypasses the seam** — the only durable write not behind `atomic_replace`.
- **Export's `rename` and CleanRecovery's `remove_file` bypass the seam** despite `Fs` declaring both.
- **Directory listing is outside any abstraction** — no seam, no cap, synchronous on the UI thread.
- **Config/theme reads are unbounded** — not subject to `MAX_OPEN_BYTES` discipline.
- **No content-hash check on load** — the external-mod guard only fires around save.

---

## 2. GROUNDING — path-change durability map (verified this session)

### 2.1 Buffer path identity

`Document::path: Option<PathBuf>`. `None` covers TWO distinct cases: an ordinary untitled buffer
(`*untitled*`) and the single permanent scratch buffer (tracked separately via
`Editor::scratch_id: Option<BufferId>` / `install_scratch` / `is_scratch(id)`, displayed `*scratch*`).

There is **no identity struct**. Durability state lives as sibling fields on `Buffer`
(`last_swap_at`, `swapped_version`, `swap_in_flight`, `pending_swap_path`) and `Document`
(`saved_version`, `stored_fp`).

Path is a **derived** key in three places, all recomputed on demand, never cached as an index:
`swap::swap_path(doc_path)`; `session_restore::persist_session` (keys `SessionState.entries:
HashMap<String, StateEntry>` by `canonicalize(path).to_string_lossy()`); and nothing else.

**The workspace explicitly PERMITS the same path open in multiple buffers.** Path is not a
uniqueness invariant anywhere.

### 2.2 Swap and the path-aware latch (a fixed bug worth preserving)

Swap filename derives from a **hash of the canonical realpath**, placed in the XDG state dir, not
next to the document. Any path change computes a completely different swap filename.

`swapped_version: Option<u64>` is **version-keyed, not path-keyed**.
`swap::pending(dirty, version, swapped_version) = dirty && swapped_version != Some(version)`.

**The race (found and FIXED, commit `fd09de3`):** a `SwapWrite` job dispatched capturing the OLD
path, in flight when a SaveAs rekeys `Document.path`. The stale merge would unconditionally set
`swapped_version = Some(version)`, and since `pending()` is version-only, the editor would believe
"already durably swapped" — suppressing a fresh swap write under the NEW path. Net effect: unsaved
content with **no recovery file at its current path** if the process crashed.

Fix, in `dispatch_swap_write`'s merge closure:

```rust
if ok && swap_path(b.document.path.as_deref()).ok().as_ref() == Some(&path) {
    b.last_swap_at = Some(ts);
    b.swapped_version = Some(version);
}
```

Regression test: `swap::tests::stale_path_swap_does_not_relatch_after_rekey`.

The stale swap under the old path is deliberately **not** deleted here — a different co-open buffer
at that same old path could legitimately own it (deleting would be a data-loss vector per an
earlier Codex round).

### 2.3 Recovery

Path/content-hash based, evaluated **once at startup**, not a scan-for-renamed-files.
`swap::assess` only ever looks at the swap file that would exist for the path being opened right
now. Content-hash = FNV-1a64 of swap body vs `fnv1a64(current_file_bytes)`. Match →
`DiscardSilently`. Mismatch / file missing / unparseable → `Prompt`.

Scratch has no fixed path, so recovery uses `find_orphan_scratch_swap()` — the one genuine
directory scan.

**A swap whose recorded realpath is never revisited sits inert in the state dir indefinitely**,
discoverable only by coincidentally reopening that path, or by the H5 `clean_recovery` sweep —
which only removes it if provably valueless. A genuinely-diverged orphan accumulates permanently.

### 2.4 Save-As — what it gets RIGHT

Save-As exists (`save_as` command; also reached by plain Save on an unnamed buffer). The rekey
happens **inside the save job's merge**, atomically with write completion:

- `Document.path` → new target (only on success)
- `saved_version`, `stored_fp` → re-established against the **new** path (`new_fp` taken from
  `write_path`, so correct)
- `Buffer.swapped_version` → cleared
- old swap (`prior_key`) deleted; new swap deleted if clean, or `last_swap_at = None` to expedite a
  fresh swap under the new path if edited-on during the write
- plugin `Save` event fires with the new path
- undo history untouched and needs no rekey

On failure: `saved_version`/`stored_fp`/`path` all left untouched (buffer stays dirty at old path).

### 2.5 Save-As — what goes STALE

1. **Session-resume entries are never migrated.** `SessionState.entries` keyed by canonicalized path
   string. Save-As strands the OLD path's entry (cursor/scroll/marks/folds) forever; the NEW path
   has none until the next `persist_session`. Reopening the old name later resurrects stale resume
   state for content that moved.
2. **Orphan swap litter** (see §2.3).
3. `Buffer.pending_swap_path` is declared but never populated/read — dormant field, latent trap.
4. Status messaging does not name the new path on a successful Save-As ("Saved" / "Saved v{v}
   (still editing)" — identical wording to a normal save).

### 2.6 Undo / snapshots

`Document.history: History` carries **no path or filename reference anywhere**. Undo is pure
content/selection state. A path change is a complete no-op for undo. `Buffer.undo_evicted_pending`
is buffer-scoped.

**There is NO snapshots/checkpoint feature today.** The only checkpoint-like things: the periodic
swap file, and `recovery::LAST_GOOD` + `dump_on_panic`/`dump_all_dirty` emergency `.md` dumps —
both content-only, keyed by path at snapshot time purely for naming the dump file.

---

## 3. GROUNDING — overlay/picker infrastructure (verified this session)

### 3.1 The H21 overlay dispatch table (`overlays.rs`) — the registration seam

`OverlayId`: exhaustive enum, 11 variants — `Splash, Menu, Palette, ThemePicker, CursorPicker,
FileBrowser, Prompt, Minibuffer, Search, Diag, Outline`. `Splash` MUST be index 0.

`OverlayId::ALL: &'static [OverlayId]` — canonical intercept-chain order.
`OverlayId::row(self) -> &'static OverlayRow` — exhaustive match into `OVERLAYS`; **adding a variant
fails to compile until a row is added.** This is the compile-time seam preventing silent UI leaks.

`OverlayRow` fields: `name: &'static str`, `id: OverlayId`,
`is_active: fn(&Editor) -> bool`, `intercept: fn(Msg, &mut Editor, &DispatchCtx) -> Handled`,
`close: fn(&mut Editor)`, `mouse: fn(&mut Editor, MouseEvent, Rect, &DispatchCtx)`,
`render: RenderSite`.

`RenderSite` = `Frame(fn(&mut Frame, &mut Editor, &ChromeStyles))` or `StatusRow` (painted inline in
`render.rs`; used only by `Prompt`/`Minibuffer`/`Search`).

```rust
pub(crate) struct DispatchCtx<'a> {   // deliberately excludes &mut Editor (aliasing)
    reg: &'a crate::registry::Registry,
    keymap: &'a crate::keymap::KeyTrie,
    ex: &'a dyn crate::jobs::Executor,
    clock: &'a dyn wordcartel_core::history::Clock,
    msg_tx: &'a std::sync::mpsc::Sender<Msg>,
}
```

`RENDER_ORDER` — a DISTINCT permutation from `ALL`, containing only `Frame`-site overlays, grounded
against `render_overlays::paint`'s literal sequence.

`any_active()` / `close_all()` are single folds over `OVERLAYS`.

**To register a new overlay:** add the variant to `OverlayId` + `ALL` (position = intercept
precedence); add the arm to `row()` (compiler forces); add the `OverlayRow` literal at the matching
index; if `Frame`, add to `RENDER_ORDER` (enforced by `render_order_is_exactly_the_frame_overlays`).
Everything else — `any_active`, `close_all`, mouse routing, XOR-single-overlay, and the guardrail
suite (bijection, render coverage, active-XOR, key/click/hover consumption, close-all-clears) — is
inherited because it iterates the table.

The module doc **explicitly reserves this shape for a future `OverlayId::PluginPanel`** — the
intended precedent for exactly this kind of new overlay.

### 3.2 The existing file browser (`file_browser.rs`)

```rust
pub struct FileEntry { pub name: String, pub is_dir: bool }
pub struct FileBrowser {
    pub dir: PathBuf, pub query: String,
    pub entries: Vec<FileEntry>, pub selected: usize, pub scroll_top: usize,
}
```

`rebuild_entries` — `std::fs::read_dir`, dirs-then-files alphabetical, synthetic `".."` when
`dir.parent().is_some()`. Filter: **case-insensitive substring** (`name.to_ascii_lowercase().contains(&q)`)
— **not fuzzy**. **No hidden-file filtering. No extension filtering.**

`file_browser_enter()` — shared Enter path (keyboard + mouse click). Directory → `read_dir(&target).is_ok()`
check, then descend (Sticky/Error status and no mutation if unreadable). File → close browser +
`workspace::open_as_new_buffer`.

Keys: `Esc` close; `Enter`; the six shared nav keys via `list_window::list_nav_key`/`apply_list_nav`;
`Backspace` pops query; printable `Char` appends; `Event::Paste` appends. Every key consumes.

Render: `render_overlays::paint_file_browser` — bordered `Block` titled `" Open: {dir} "`, bottom
windowed indicator.

Mouse: `mouse::mouse_file_browser` — `Moved` hovers via `chrome_geom::file_browser_row_at`; wheel via
`list_window::wheel_list`/`WHEEL_STEP`; `Down(Left)` selects AND commits; click-away closes.

Opened via `Editor::open_file_browser(dir)`, from the `"open"` command (seeds dir from active doc's
parent, falling back to `current_dir()`).

### 3.3 Other list overlays and shared code

`Palette` (also serves the buffer switcher via `PaletteKind::Buffers` — there is **no separate
buffer-list overlay**), `ThemePicker`, `CursorPicker`, `FileBrowser`, `Outline`, `Menu`, `Diag`.

**Shared:** `list_window.rs` — `list_h_for`, `keep_visible`/`app::keep_overlay_visible`,
`wheel_scroll`, `clamp_into_window`, `wheel_list`, `WHEEL_STEP`, `ListNav`/`list_nav_key`/
`apply_list_nav` (palette, theme picker, file browser, outline; menu and diag opt out).
`palette::fuzzy_filter<T>` — generic **nucleo-matcher**-based fuzzy ranker, shared by palette and
outline. **The file browser does NOT use it.** `chrome_geom.rs` — per-overlay hit-testing, keeping
geometry single-sourced between mouse and painter.

**Hand-parallel (not shared):** each overlay's state struct, `intercept`, painter, mouse fn. H21
unified *dispatch*, not the data model or rendering.

A21 introduced the `list_window` wheel trio + a dedupe-guarded live-preview pattern on
ThemePicker/CursorPicker. Guardrail: `every_overlay_consumes_moved_without_panic_or_data_loss`.

### 3.4 Text input (`minibuffer.rs`)

```rust
pub enum MinibufferKind { Filter, GotoLine, SaveAs, WriteBlock, WrapColumn,
                          PluginArg { id: crate::registry::CommandId } }
pub struct Minibuffer { pub prompt: String, pub text: String, pub cursor: usize, pub kind: MinibufferKind }
```

`insert/backspace/left/right` — UTF-8-codepoint-safe. `RenderSite::StatusRow`.
Enter dispatches by `kind` to `prompts::submit_filter_line` / `goto_line_submit` / `save_as_submit` /
`block_write_submit` / `wrap_column_submit`, or enqueues a `PluginCall`.

`Prompt` (`prompt.rs`) is a separate fixed-choice modal — `Prompt { message, choices: Vec<Choice> }`
/ `PromptAction`, used for external_mod, transform_chooser, clean_recovery, quit confirm.

### 3.5 Existing file-related commands (`registry.rs`)

| Command | Category | Behavior |
|---|---|---|
| `new` | File | `prompts::request_new` → `workspace::new_empty_buffer` |
| `open` | File | `editor.open_file_browser(dir)` |
| `save` | File | `save::dispatch_save` — no path → `open_save_as`; else fingerprint check → external-mod prompt or `do_save` |
| `save_as` | File | `prompts::open_save_as` → `MinibufferKind::SaveAs`, pre-filled with current dir |
| `save_and_quit` | File | `save::dispatch_save_and_quit` |
| `quit` | File | `Command::Quit` |
| `clean_recovery` | File | count-confirm modal over orphaned recovery/swap files |
| `export_html` / `export_docx` / `export_pdf` / `export_tex` | Export | `export::run_export(editor, "<fmt>", &msg_tx)` |
| `block_write` | Block | `blocks_marked::block_write` → `MinibufferKind::WriteBlock` |
| `save_settings` | Settings | writes settings/config |

**Only Open uses the FileBrowser. No save/write/export command opens it.**

---

## 4. GROUNDING — Helix study + dependency verdicts (verified this session)

### 4.1 Helix's picker architecture

`Picker<T: Send + Sync, D>` is fully generic — `T` = item type, `D` = shared editor-data context.
`Column<T, D>` is a per-column formatter. A cheap `Clone`-able `Injector<T, D>` lets **any producer,
from any thread**, call `injector.push(item)`. A `version`/`picker_version: Arc<AtomicUsize>` pair
makes `push` return `Err(InjectorShutdown)` once the picker is dismissed — a still-running producer
harmlessly no-ops instead of needing explicit cancellation plumbing.

**No polling.** `Nucleo::new(Config::DEFAULT, Arc::new(helix_event::request_redraw), None, cols)` —
the matcher owns worker threads and fires a **redraw-request callback** when results land. The
render path does `self.matcher.tick(10)` (10 ms budget) per frame to drain. Background computes,
callback wakes UI, UI drains with a bounded per-frame budget.

**Directory walking: the `ignore` crate**, via `WalkBuilder` with `.hidden()`, `.parents()`,
`.ignore()`, `.follow_links()`, `.git_ignore()`, `.git_global()`, `.git_exclude()`,
`.sort_by_file_name()`, `.max_depth()`, `.filter_entry()`, `.add_custom_ignore_filename()`,
`.types()`. Critically it calls **`.build()` — the SEQUENTIAL `Walk`, not `build_parallel()`** —
for the plain file picker. `filter_picker_entry` hardcodes skipping `.git`/`.pijul`/`.jj`/`.hg`/`.svn`
even when ignore is off, and dedupes symlinks pointing back inside the root.

`FilePickerConfig` defaults: `hidden: true` (hidden files hidden), all gitignore layers respected.
`FileExplorerConfig` defaults the OPPOSITE way — everything shown, nothing filtered.

`global_search` (content search) is the ONE place using `build_parallel()`.

**Large-directory handling — the reusable pattern:**

```rust
let injector = picker.injector();
let timeout = std::time::Instant::now() + std::time::Duration::from_millis(30);
let mut hit_timeout = false;
for file in &mut files {
    if injector.push(file).is_err() { break; }
    if std::time::Instant::now() >= timeout { hit_timeout = true; break; }
}
if hit_timeout {
    std::thread::spawn(move || { for file in files { if injector.push(file).is_err() { break; } } });
}
```

No debounce, no polling loop, no batch cap. Small repos finish inline with zero thread-spawn
overhead. Only a slow walk falls back to one bare `std::thread::spawn`.

**Fuzzy matching:** the full `nucleo` crate (deps: `nucleo-matcher`, `parking_lot`, **`rayon`**) —
Helix needs the background reindexer. The pure `nucleo-matcher` crate has NO threading (`memchr` +
optional `unicode-segmentation`), MPL-2.0.

**NO tree view.** `type FileExplorer = Picker<(PathBuf, bool), (PathBuf, Style)>` — the SAME generic
Picker, seeded with one directory level (`max_depth(Some(1))`). Selecting a directory **pushes a
brand-new picker** rooted at the child, with a synthetic `..` at index 0 and single-child directories
auto-flattened via `get_child_if_single_dir`. No tree state, no indentation/collapse logic.

**Nuance:** Helix's `main.rs` is `#[tokio::main]` and `global_search`'s re-query is wrapped in
`async move`, but that is because Helix's job system is tokio-shaped for LSP/DAP. `file_picker` /
`file_explorer` themselves use **zero `async`/`.await`** — plain sync code plus one
`std::thread::spawn`. tokio is NOT why Helix's picker is fast.

### 4.2 Dependency verdicts

- **`tokio` 1.53.0, MIT — DO NOT ADOPT.** `rt,rt-multi-thread,fs,macros` = 10 crates; `full` = ~21.
  Controller note (verified independently, and this CORRECTS the research agent's claim that tokio
  rides in under `harper-brill`/`burn`): `cargo tree -i tokio --target all` finds **no path on any
  target**, and the release binary has **0 tokio symbols**. It is a stale lockfile entry. Adopting
  it means standing up a runtime from scratch. Critically, **`tokio::fs` is not real async file IO**
  — there is no portable async file IO on Linux short of io_uring, so it is blocking IO dispatched
  to a thread pool via `spawn_blocking`. We already have blocking IO on a worker thread.
- **`jwalk` 0.8.1, MIT — SKIP.** Depends on `rayon` + `crossbeam`. rayon lazily spins
  `num_cpus::get()` workers that **park on a condvar when idle** — no CPU spin, but a fixed number
  of OS threads alive for process lifetime, a second uncoordinated concurrency substrate beside
  `jobs.rs`. Its selling point (streamed + sorted parallel results) is something a `std::thread` +
  `mpsc` already gives us.
- **`ignore` 0.4.30, Unlicense OR MIT — ADOPT.** Deps: `crossbeam-deque`, `globset`, `log`, `memchr`,
  `same-file`, `walkdir`, `regex-automata`, `winapi-util`(win). **This repo's lock already contains
  every one except `globset`** (via mlua, arboard, crossterm, harper-brill). Net new: ≈`ignore` +
  `globset`. `WalkParallel` is built on **crossbeam-deque, NOT rayon** — no independent global thread
  pool; driveable from our own `std::thread`. Brings gitignore/hidden/VCS-dir handling. License
  already in `deny.toml` allow-list.
- **`walkdir` 2.5.0 — comes bundled with `ignore`; don't take standalone.**
- **`notify` 8.2.0, CC0-1.0 — HOLD, not for this feature.** Linux deps: `notify-types`, `libc`,
  `log`, `mio`, `inotify`, `inotify-sys`. Genuinely event-driven (inotify via epoll) — the watcher
  thread blocks in-kernel, **zero CPU at rest, satisfies "idle is free" exactly**. Published 8.2.0
  has no tokio dep (9.0.0-rc has it optional/feature-gated, off by default). License allow-listed.
  Architecturally clean, but a picker is a one-shot listing, not a live-watch problem. Its real home
  is the external-mod fingerprint item.
- **`tui-tree-widget` 0.24.0, MIT — COMPATIBLE with ratatui 0.30** (depends on `ratatui-core 0.1` /
  `ratatui-widgets 0.3`, exactly what ratatui 0.30 splits into). The controller had flagged version
  incompatibility as a likely blocker; that was **wrong**. Not recommended anyway on
  don't-build-a-second-stateful-widget grounds.
- **`nucleo-matcher` 0.3.x, MPL-2.0 — ALREADY a direct dependency of `wordcartel`.** Zero marginal
  cost. Already reviewed/allow-listed in `deny.toml`. No threading of its own — correct for a
  project that owns its own job substrate.

---

## 5. The command-surface contract (binding)

`docs/design/command-surface-contract.md`. Three surfaces (palette, menu, keybindings) + a future
plugin actor all route through ONE command registry, which is also the plugin/automation API.

Laws, each with an enforcing test:
1. Registry is the single source of truth.
2. **Every user-settable option is a command.** Test: `settings.rs::every_persisted_setting_has_a_command`
   — compile-time exhaustive destructure of `SettingsSnapshot` + per-field `reg.resolve_name(...)`.
3. **Palette is exhaustive.** Tests: `palette.rs::palette_is_exhaustive_over_the_registry` and
   `palette_is_exhaustive_over_a_plugin_loaded_registry`.
4. **Menu ⊆ palette** (dynamic-section rows are data and exempt). Test:
   `menu.rs::parameterized_plugin_command_and_plugin_list_satisfy_law3_law4`.
5. Every mouse affordance has a keyboard path.
6. **One setter per option** — presets/profiles call the SAME setter the command calls.
7. **Hints track the active keymap.** Tests: `keymap.rs::hints_reresolve_on_preset_switch`,
   `menu.rs::custom_bind_surfaces_in_menu_and_palette`.
8. **Multi-state option ⇒ set-per-state palette-only primitives + ONE stateful toggle/cycle menu
   representative.**
9. A preset is never the only door to an option.
10. Commands are nullary today; parameterized set-value commands are an Effort-P concern.

**Dynamic menu sections:** `DYNAMIC_SECTIONS` data table of `fn(&Editor) -> Vec<(label, action)>`
providers — rows are data, not registry commands, but their *actions* must call a shared setter a
registered command also uses. Precedent: the Documents section + `switch_buffer`/`next_buffer`/
`goto_scratch` all route through `workspace::switch_to`.

**Any effort touching commands/options/palette/menu/hints MUST state in BOTH spec and plan how it
conforms.** The invariant tests are merge GATEs.

---

## 6. The forks, as put to the human, and the human's answers

The controller resolved five forks with the human, one at a time, plain-text A/B/C with a
recommendation each time. Below: the options as presented, the controller's recommendation, and the
human's answer. **You are invited to disagree with any of these.**

### F1 — Browsing UI model

Context given: Helix has NO tree widget; its file explorer is the same flat Picker re-rooted per
directory with a synthetic `..` and single-child flattening. But Helix's users are developers who
know their repo; the human's stated motivation is *"many writers may not have as much experience
navigating or using filesystems from the terminal"* — a persistent tree gives spatial orientation a
re-rooted picker structurally cannot.

- **A.** Re-rooted flat picker (Helix's pattern). Extend existing `FileBrowser`, reuse for save/export.
- **B.** Persistent collapsible tree (`tui-tree-widget`). Best orientation; costs a second stateful
  widget, new keybindings, new render path, a dependency.
- **C.** Split by job: picker for *transactional* moments (open / save-as / export destination);
  the tree is a *binder* problem and belongs to **S2 (directory-as-binder)**, already on the backlog.
  Building the tree in C5 risks building it twice.

Controller recommended **C**, with the caveat that if S2 is far off, writers get no orientation aid
for a long time, and B-inside-C5 would then be defensible provided the tree is designed as the
binder's component from day one.

**HUMAN ANSWERED: C — picker for C5, tree lands in S2.**

### F2 — Chokepoint width

Context given: `Fs` is write-only, one operation, three callers; reads/listing/delete/rename all raw
`std::fs`; `Fs` already declares unused `rename`/`remove_file`. Some access genuinely cannot be
unified (shell filter, harper-ls, pandoc's `-o`) and the spec must say so.

- **A.** UI-only. One picker for destinations + fix listing responsiveness. Leave the seam.
- **B.** UI + durable writes. Adopt the unused `rename`/`remove_file`; fix the dictionary append to
  be atomic and capped. Reads/listing get a shared entry point but not full fault-injection.
- **C.** Full unification. One module owns read/write/stat/list/rename/delete; `Fs` extended so all
  of it is fault-injectable, capped, testable. Config/theme reads come under cap discipline.

Controller recommended **C**, arguing that this codebase's durability bugs have consistently been
*seam* bugs (swap-latch race, BUG-2 fingerprint, swap idle-thrash) and the testable ones are exactly
those already behind `Fs` — extending the seam extends what's testable, and gives S3 snapshots a
surface with a fault-injection story. Caveat stated: C is the difference between a medium and a
large effort, and most of its work is migration rather than user-visible improvement.

**HUMAN ANSWERED: C — full unification.**

### F3 — Visibility / filter model

Constraint noted: contract law 8 means each state needs a palette-reachable command. Principle
proposed and accepted: **the filter must never silently lie** — a footer disclosure count whenever
entries are withheld (follows from the project's no-silent-UI rule).

- **A.** Two orthogonal toggles: "clutter" (dotfiles + VCS dirs + gitignored — all the same intent,
  noise the writer didn't create) and "file type" (documents vs everything).
- **B.** One preset ladder: Writer → All files → Everything. Fewer knobs but collapses two
  independent axes; "dotfiles but only .md" becomes inexpressible.
- **C.** Per-invocation override with fixed defaults. Nothing to misconfigure, nothing sticks.

Controller recommended **A**, and separately argued the documents filter must **not** mean
markdown-only — a writer importing a `.docx` or opening a `.txt` would find an apparently empty
directory. Proposed a generous definition: `.md`, `.markdown`, `.txt`, `.rst`, plus pandoc-ingestible
formats.

**HUMAN ANSWERED: A — two toggles, generous documents definition.**

### F4 — "Make sure we are saving files as markdown"

Context given: Save-As takes a typed path with **no extension validation at all** — a writer can
type `chapter one` and get an extensionless file, or `chapter.docx` and get markdown text under a
name Word will choke on. Save-vs-export is precisely the distinction this audience gets wrong, and
the extension is where the confusion surfaces.

- **A.** Default-and-redirect. Missing extension → append `.md`. Recognized OUTPUT extension
  (`.docx`/`.pdf`/`.html`/`.tex`) → refuse, explain, offer Export (which now has a destination
  prompt). Any other extension → honored silently.
- **B.** Default only. Append `.md`; never comment. Preserves the broken-`.docx` trap.
- **C.** Strict. Only markdown extensions for save; everything else via Export. Blocks legitimate
  `.txt`/`.rst`/dotfile-note cases.

Controller recommended **A**, noting it is only possible *because* export gains a destination prompt
— before this effort "use Export instead" would have been advice with nowhere to go.

**HUMAN ANSWERED: A — default-and-redirect.**

### F5 — Path-as-identity  ⚠️ **THE HUMAN HAS REOPENED THIS. TREAT IT AS UNRESOLVED.**

Two stale-state gaps (§2.5) plus a deeper question: should a buffer have a **stable identity**
independent of path, so renames stop orphaning things and **S3 snapshots inherit a durable key**?

- **A.** Fix symptoms, keep path as key. Migrate the session-restore entry on Save-As; extend the
  recovery sweep to diverged orphans. Small, contained, no new persistent state.
- **B.** Introduce a stable buffer identity. Persist an id per document; key session state,
  snapshots, and swap on it. Renames free everywhere; S3 lands on solid ground.
- **C.** A now, identity question recorded as S3's to answer.

Controller recommended **A**, on this specific reasoning: the swap filename is *derived* from the
canonical realpath by hash, which means **recovery needs no index** — on startup, opening file X, we
compute exactly where its swap would be and look. Keying swaps on a buffer id would require a
path→id mapping on disk, and that index becomes a single point of failure for crash recovery: if
lost or corrupted, swap files exist but are no longer findable. That trades a derivable key for an
indexed one in the one subsystem where "works when everything else is lost" is the entire point.
Controller also noted session state is *already* an index file so an id there would be cheap, but
that doing it for session-only while swap stays path-derived yields two identity models.

**HUMAN ANSWERED: A — fix symptoms, keep path-derived swap.**

**BUT THE HUMAN THEN SAID, verbatim:** *"some questions like F5 were troubling because I would like
a good surface for future efforts like snapshots."*

**So F5 is explicitly reopened for you.** The human accepted A reluctantly and wants a better answer
if one exists. Specifically consider — and do not feel bound by the controller's framing:

- Is the "index is a single point of failure" argument actually decisive, or is it overstated? (E.g.
  could an id be embedded IN the swap header, which is already read at recovery time, making the
  mapping self-describing and index-free? The swap header already carries `content_hash` and a
  recorded `realpath`.)
- Could swap remain path-derived (index-free recovery preserved) while session state AND snapshots
  key on a stable id — and is "two identity models" actually a cost, or is it correct layering,
  given the two have genuinely different failure requirements?
- What does S3 (snapshots — durable revision checkpoints) actually need from an identity surface?
  What would it regret if C5 ships A? Is there a cheap forward-compatible move now (e.g. mint and
  persist an id without yet keying anything on it) that costs little and de-risks S3?
- Is there a fourth option the controller did not present?

---

## 7. The controller's draft design (for you to critique, not to implement as-is)

One decision the controller made unilaterally and flagged: **the picker navigates ONE directory
level at a time**, with fuzzy filtering within the level; recursive project-wide fuzzy-find was
assigned to S2. The human has not objected but also has not been asked directly.

1. **Chokepoint.** One shell module owns every in-process FS operation; `fsx` pattern extends rather
   than gets replaced (free functions taking `&dyn Fs`; trait carries injectable primitives). `Fs`
   grows bounded-read, directory-listing, stat. `FaultFs` gains coverage for each. Migration sites:
   `file::open`, `config::load`, `theme_resolve`, dictionary read + append, `state.rs` load,
   export's `rename`, `clean_recovery`'s `remove_file`, `file_browser`'s `read_dir`. Out of scope,
   stated explicitly: `!` shell filter, harper-ls's own access, pandoc's `-o`.
2. **Listing off the hot path.** Helix's 30 ms time-box + `std::thread` fallback, riding `jobs.rs` +
   `mpsc`, merged behind a version check like S8's `PosSweep`. `ignore::WalkBuilder` with
   `max_depth(1)`. Entry count capped with visible disclosure.
3. **One picker, two modes.** Select mode (Open) chooses an existing entry. Destination mode
   (Save-As, Export, Write-Block, New) navigates AND types a filename — the picker carries a
   filename field. Destination mode replaces the typed-path minibuffer and gives export a
   destination for the first time.
4. **Filters.** Two orthogonal toggles per F3, each a command with set-per-state + cycle per law 8.
   Defaults: clutter hidden, documents-only (generous). Footer disclosure count whenever withheld.
5. **Extension policy** per F4.
6. **Durability.** Save-As migrates the session-restore entry. `clean_recovery` sweep extends to
   diverged orphans. The path-aware swap latch stays exactly as is; its regression test is a merge
   gate; the spec records WHY path-derived naming is deliberate. Stable-identity tradeoff written
   down for S3.
7. **Command-surface conformance** per §5.
8. **Testing.** `FaultFs` coverage per new seam op; listing cap + disclosure; extension-policy cases;
   the three contract invariants; an e2e journey open → save-as → export-with-destination; module
   budgets (`file_browser.rs` will grow; the two modes are a natural split point).
9. **Dependencies.** `ignore` + existing `nucleo-matcher`. No tokio, no jwalk, no tree widget.
   `notify` deferred.
10. **Risks.** Broad migration touching durability-critical code (mitigation: seam extension is
    purely additive; call sites migrate incrementally, green at each step). `ignore`'s gitignore
    semantics could hide a writer's own drafts if they keep a manuscript under an aggressive ignore
    file — the disclosure count is what keeps this from being silent, but it is a real hazard worth
    naming rather than assuming gitignore-respecting is right for a non-developer audience.
    **Effort size: large** — sections 1 and 3 are each substantial alone.

---

## 8. What to produce

Work in three passes and return them in one report:

**GROUND.** State what you verified vs took on faith from this packet. Anchor on SYMBOL NAMES, not
line numbers — line anchors drift. If a claim here is load-bearing and you can cheaply check it
against the real source, check it and say so. Flag anything in this packet you believe is wrong.

**IMAGINE.** What SHOULD this be, for a writer who is not confident at a terminal? Push on the
design. Where the controller's draft is merely adequate, say what would be better. Consider
explicitly: what does a writer actually experience the first time they save a file, and the first
time they export one? Is "destination mode" the right shape, or is there something better? Is
one-level navigation right, or does a writer need project-wide find? What is the failure mode when
someone cannot find their file, and does the design actually rescue them?

**SCOPE.** Decompose into tasks sized for TDD subagents. Identify what is genuinely hard vs merely
tedious. Call out anything that should be deferred or split out, and anything the human should
decide before a spec is written. Give an honest size estimate and name the single highest-risk part.

**And resolve F5 with a real recommendation and an argument** — including, if you conclude the
controller was wrong, saying so directly.

Do not write the spec yet. Return ground/imagine/scope; the human reviews, then you author.
