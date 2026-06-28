# Wordcartel Effort 7 — File Open / Save-As / New — Design

**Status:** design (brainstormed 2026-06-28)
**Roadmap:** Effort 7, exec #3, pre-1.0 (`docs/superpowers/plans/2026-06-22-wordcartel-coverage-ledger.md`).
**Goal:** let the editor open another file, save to a new name, and start a fresh document
**from within the app** — the biggest remaining *completeness* gap (today files load only via
the launch CLI arg, and unnamed buffers can't save: the orphaned `save.rs` "No file name"
stub). Build the open path **buffer-extensibly** so Effort 6 (multi-buffer) generalizes it
without rework.

---

## 1. Scope & philosophy

- All work is in the `wordcartel` shell. **No `wordcartel-core` change.**
- New registry commands: **`open`**, **`save_as`**, **`new`** (name-keyed registry, so
  palette-reachable + config-bindable in every preset). Default `cua` keybindings added;
  WordStar `^KR`/`^KW` are **not** wired here (`^KW` is Effort 9A; `^KR` file-read deferred).
- **Single-buffer** still: Open/New **replace** the current buffer (dirty-guarded). The
  file-load logic is factored into a **buffer-extensible seam** (`Buffer::from_file`) so
  Effort 6 can load into a *new* buffer instead of replacing — "build it buffer-extensibly,"
  not "build multi-buffer."
- Reuses existing infra: the `theme_picker`-style overlay, the `MinibufferKind` prompt
  (as `goto_line` does), the `PromptAction` modal model, `file::{open, save_atomic}`, the
  crash-safety `swap` module, and `state.rs` session state.
- **Deferred (out of scope):** recent files; `tui-popup` dep; `nucleo` fuzzy filter +
  recursive gitignore-aware finder (effort "D"); the unified save-mode browser (Save-As
  stays a minibuffer prompt); multi-buffer (Effort 6).

---

## 2. File-load seam (buffer-extensibility)

Today `app::run()` builds the initial buffer inline (open → `Editor::new_from_text`, with an
error branch for NotFound/Binary/Permission/IsDir/IO). Extract that into a reusable seam:

```rust
// editor.rs (or a small file_load.rs) — pure-ish buffer construction.
impl Buffer {
    /// Build a Buffer from a file's contents. Mirrors run()'s open branch:
    /// Ok → named clean buffer; NotFound → named empty "new file"; the buffer is
    /// caller-placed (replace current in Effort 7; push new in Effort 6).
    pub fn from_file(path: &Path, area: (u16, u16)) -> Result<Buffer, crate::file::OpenError>;
}
```

- `app::run()` is refactored to use `Buffer::from_file` for its initial open (behavior
  unchanged — same error→status mapping for Binary/Permission/IsDir/IO, same "new file"
  for NotFound).
- In-app open uses an `open_into_current(editor, path)` that:
  1. builds the buffer via `Buffer::from_file`,
  2. **restores `state.rs` resume cursor + marks** for `path` (staleness-guarded, as launch
     does — same `state::file_identity` mtime+size check),
  3. replaces the active buffer,
  4. `derive::rebuild` + `nav::ensure_visible`.
- **Effort 6 forward-compat:** `Buffer::from_file` returns a `Buffer` the caller positions;
  Effort 6 calls it to append a new buffer. The file-browser overlay (below) is likewise
  buffer-agnostic. This is the rework-avoidance the roadmap mandates.

**Crash-safety on open (parity with launch):** opening a file in-app must run the same
swap-recovery assessment launch does (`swap::assess` / orphan check) so an in-app open of a
file with a stale swap surfaces the recovery prompt — OR, if that is judged out of scope for
v1, the spec explicitly states in-app open does NOT re-run recovery and documents the gap.
**Decision (v1):** in-app open **does** check `swap::assess` for the target path and raises
the existing recovery prompt when a swap is found, matching launch semantics (no silent
divergence between launch-open and in-app-open).

---

## 3. Open + file-browser overlay (open mode)

**Command `open`** (cua: `ctrl-o`) opens the **`file_browser`** overlay.

**Overlay structure** (mirror `theme_picker.rs`):
```rust
pub struct FileBrowser {
    pub dir: PathBuf,        // current directory being listed
    pub query: String,       // type-to-filter (substring, case-insensitive)
    pub entries: Vec<FileEntry>, // rebuilt on dir change / query change
    pub selected: usize,     // clamped index into the filtered view
}
pub struct FileEntry { pub name: String, pub is_dir: bool } // ".." synthesized as is_dir
```
- **Listing:** `std::fs::read_dir(dir)`; sort **directories first, then files**, each
  alphabetical; prepend a synthetic `..` entry (unless at filesystem root). Hidden dotfiles
  shown (v1; a toggle is a later nicety). Unreadable dir → status message, stay in prior dir.
- **Filter:** `query` substring-filters `entries` case-insensitively (same approach as the
  palette/theme-picker), `selected` clamped to the filtered list.
- **Keys** (mirror palette/theme-picker key block in `app.rs`): printable → append to
  `query`; Backspace → pop; Up/Down → move `selected`; **Enter** → if selected is a
  directory (incl `..`) **descend** (set `dir`, clear `query`, rebuild, reset `selected`);
  if a file → **open** it (the dirty-guard runs first, §4); **Esc** → close. Async
  `ClipboardPaste` while open is drained (same guard the other overlays use).
- **XOR:** `file_browser` joins the single-overlay XOR set — opening it clears
  prompt/minibuffer/palette/menu/search/diag/outline/theme_picker, and each of those clears
  `file_browser` (add to all the existing XOR sites + `dispatch_overlay_command`/mouse guard,
  exactly as `theme_picker` did).
- **Starting dir:** the active document's parent dir, else `std::env::current_dir()`.
- **Render:** mirror the `theme_picker`/outline overlay render (themed border via
  `compose([SE::Chrome])`, tiny-area/empty-list guards, `§13.2`-safe faces).

---

## 4. Dirty-guard + post-save chaining

Open (a chosen file) and New both **replace** the current buffer, so they guard unsaved work.

**Flow:** the action (open path `P` / new) checks `editor.active().document.dirty()`:
- **clean** → perform immediately.
- **dirty** → raise a `PromptAction` modal **Save / Discard / Cancel**:
  - **Discard** → perform the replace, edits lost.
  - **Cancel** → abort; buffer untouched.
  - **Save** → **arm a post-save action** and start the save; on write **success** perform
    the pending action; on failure/timeout → abort, keep the buffer (no data loss, no
    surprise replace).

**Post-save mechanism — generalize the existing `quit_after_save`.** 9B's save-and-quit
arms `editor.quit_after_save: Option<u64>` (a version) and the `Msg::JobDone`→`apply_result`
path quits when the completed save's version matches, with a timeout that clears it. Effort 7
generalizes this single-purpose field into:
```rust
pub enum PostSaveAction { Quit, Open(PathBuf), New }
pub struct PendingAfterSave { pub version: u64, pub action: PostSaveAction }
// editor.pending_after_save: Option<PendingAfterSave>   (replaces quit_after_save/_at,
//   OR runs alongside — implementer's call; existing save-and-quit tests MUST stay green)
```
- `apply_result`, on a save result whose version matches `pending_after_save.version`,
  performs the action: `Quit` → `editor.quit = true` (today's behavior); `Open(p)` →
  `open_into_current(editor, p)`; `New` → replace with scratch.
- The existing **timeout** (`SAVE_QUIT_TIMEOUT_MS`) clears `pending_after_save` if the save
  never lands — generalized to all actions.
- **Unnamed dirty buffer + Save:** there is no path to save to, so "Save" routes into
  **Save-As** (§5); the post-save action is armed against the Save-As write's version, so
  after the Save-As completes the open/new proceeds. (Chain: dirty-guard → Save-As prompt →
  write → pending action fires.)

**Constraint:** the `Quit` path must remain behavior-identical to 9B (the two existing
`save_and_quit_*` tests pass unchanged); the `PromptAction::SaveAndQuit` arm now arms
`PostSaveAction::Quit`.

---

## 5. Save-As

**Command `save_as`** (cua: `ctrl-shift-s`), and the first `save` of any **unnamed** buffer,
route here. Replaces the `save.rs:90` / `:125` stub.

- **Prompt:** a `MinibufferKind::SaveAs` minibuffer ("Save as: ", pre-filled with the active
  document's dir or cwd, trailing separator) — same overlay `goto_line` uses, submit routes
  on `mb.kind`.
- **Submit** (`save_as_submit(editor, text)`):
  1. Resolve the typed path (expand `~`, make absolute relative to cwd) → target `P`. Empty
     input → no-op + status.
  2. **Overwrite confirm:** if `P` exists, raise the existing `PromptAction::Overwrite`
     modal first (a `pending_save_as: Option<PathBuf>` holds `P` across the confirm, mirror
     `pending_export`); on **Overwrite** → proceed, on **Cancel** → abort + clear.
  3. **Write:** dispatch the async save **targeting `P`** (reuse the `JobKind::Save` path;
     the target `P` is carried with the job so it writes `P` regardless of the buffer's
     current `path` — see the failure rule below), `file::save_atomic`.
  4. **Re-key, on write SUCCESS only (correctness crux):** in `apply_result`, when the
     Save-As write lands: capture the **prior** swap key (the unnamed buffer's scratch key,
     i.e. `None`, or the previous path), then set `document.path = Some(P)`, refresh
     `stored_fp = fingerprint(P)` and `saved_version`, and `swap::delete(prior_key)` so no
     orphan swap is left under the old key. Future swaps + the `state.rs` resume entry use
     `P`.
- **Failure rule (no state loss):** the buffer's `path` and the prior swap are **not**
  mutated until the write succeeds. A failed Save-As write → status message, the buffer stays
  exactly as it was (unnamed buffers stay unnamed, scratch swap intact and still protecting
  the unsaved work), and any `pending_after_save` armed against this write is cleared. This
  is why the target `P` rides the job rather than being written into `document.path` up front.

**`save` (named buffer)** is unchanged (9B/5a behavior). Only the no-path branch changes:
instead of the dead stub, `save` on an unnamed buffer **invokes `save_as`** (opens the
prompt).

---

## 6. New

**Command `new`** (cua: `ctrl-n`): dirty-guard (§4) → replace the active buffer with a fresh
**unnamed scratch** buffer (`"\n"`, path `None`, `saved_version = Some(0)` so it starts
clean), `derive::rebuild`, reset view/selection. Its first `save` routes through Save-As
(§5). Mirrors `run()`'s no-path scratch construction.

---

## 7. Files touched

| File | Change |
|---|---|
| `wordcartel/src/editor.rs` | `Buffer::from_file(path, area) -> Result<Buffer, OpenError>`; generalize `quit_after_save`/`quit_after_save_at` → `pending_after_save: Option<PendingAfterSave>` + `PostSaveAction`/`PendingAfterSave` types; `new`-scratch helper; `pending_save_as` field |
| `wordcartel/src/file_browser.rs` (new) | `FileBrowser` overlay state + `rebuild_entries` (read_dir, dirs-first, `..`, substring filter) — mirrors `theme_picker.rs` |
| `wordcartel/src/registry.rs` | register `open`, `save_as`, `new`; XOR clears for `file_browser` |
| `wordcartel/src/keymap.rs` | cua binds `ctrl-o`→`open`, `ctrl-shift-s`→`save_as`, `ctrl-n`→`new` (WordStar binds deferred) |
| `wordcartel/src/minibuffer.rs` | `MinibufferKind::SaveAs` |
| `wordcartel/src/app.rs` | `file_browser` key block + render (mirror theme_picker); submit routes `SaveAs`; dirty-guard helpers; `open_into_current`; `save_as_submit`; `apply_result` performs `PostSaveAction`; generalize the `SaveAndQuit` arm; XOR additions; refactor `run()` to use `Buffer::from_file` |
| `wordcartel/src/save.rs` | no-path `save` → invoke `save_as`; Save-As write + swap re-key; overwrite-via-`pending_save_as` |
| `wordcartel/src/state.rs` | (reuse) resume restore on `open_into_current` |

---

## 8. Error handling

- **Open** (in-app, via browser → existing entries): Binary/Permission/IsDir/IO → status
  message, **buffer NOT replaced** (you keep your work); NotFound shouldn't occur from the
  browser, but `Buffer::from_file` still maps it to a named "new file" for the launch caller.
- **Save-As** write failure → status; buffer retains the typed path but stays dirty; any
  armed `pending_after_save` cleared.
- **Dirty-guard Cancel / Esc** → always safe abort, buffer untouched.
- **Unreadable directory** in the browser → status, remain in the previous dir.
- **Swap on open:** in-app open runs `swap::assess` for the target and raises the recovery
  prompt when a swap exists (launch parity).

---

## 9. Testing

- **`Buffer::from_file`:** Ok (named, clean, content matches); NotFound → named empty
  "new file"; Binary/Permission/IsDir/IO → the right `OpenError`. `run()` still maps each to
  the same status as before (regression).
- **file_browser:** `rebuild_entries` (dirs-first ordering, `..` present except at root,
  substring filter case-insensitive, `selected` clamped); descend into a subdir and ascend
  via `..`; XOR (opening file_browser clears the other overlays and vice-versa); unreadable
  dir → status, dir unchanged.
- **open flow:** open a file into a **clean** buffer replaces content + sets path/clean +
  restores resume cursor; open into a **dirty** buffer raises the Save/Discard/Cancel modal;
  Discard replaces; Cancel aborts (buffer + path unchanged); **Save** arms
  `PostSaveAction::Open` and the open fires on the matching save result, and is aborted on a
  save failure.
- **save_as (success):** submit writes to the new path (`file::open` reads it back), sets
  `path`/`stored_fp`/`saved_version`, buffer clean; **old/scratch swap deleted** (no orphan
  under the old key); existing-target → Overwrite modal (Overwrite writes, Cancel aborts);
  empty input → no-op.
- **save_as (failure invariant):** a write that fails leaves the buffer **unchanged** —
  unnamed buffer stays `path == None`, still dirty, and its scratch swap is **not** deleted
  (still protects the unsaved work); `pending_after_save` cleared.
- **save (unnamed) → save_as:** `save` on a path-less buffer opens the SaveAs minibuffer
  (no longer the dead stub).
- **new:** `new` replaces with an unnamed clean scratch buffer; first `save` opens SaveAs;
  dirty-guard runs when the current buffer is dirty.
- **post-save generalization:** the two existing `save_and_quit_*` tests pass unchanged
  (`PostSaveAction::Quit` path identical); timeout clears `pending_after_save`.

---

## 10. Out of scope (explicitly deferred)

- **Recent files** — `state.rs` already LRU-keys opened paths, so a recents list is a cheap
  later add; no picker UI for it now.
- **`tui-popup`** dependency — the existing `PromptAction` modal model handles all dialogs.
- **`nucleo` fuzzy filter + recursive gitignore-aware ("D") finder** — adopt with that
  feature; v1 uses substring filtering of one directory at a time.
- **Unified save-mode browser** — Save-As is a minibuffer prompt in v1.
- **WordStar `^KR` file-read / `^KW` write-block** — `^KW` is Effort 9A; `^KR` deferred.
- **Multi-buffer (Effort 6)** — not built here, but `Buffer::from_file` + the buffer-agnostic
  browser are the seams it will reuse.
