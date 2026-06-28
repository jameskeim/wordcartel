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

Today `app::run()` builds the initial buffer inline (open → `Editor::new_from_text`, which
allocates a `BufferId` via `Editor::alloc_id` and pushes the `Buffer`; an error branch maps
NotFound/Binary/Permission/IsDir/IO). A bare `Buffer::from_file(path, area)` **cannot** build
a valid `Buffer` because a `Buffer` needs an id (and ids must never be reused — job routing is
by `BufferId`+`version`). So the seam takes the id (**Codex**):

```rust
// editor.rs — Buffer construction takes a caller-supplied id.
impl Buffer {
    /// Build a Buffer from text at `path` (None = scratch). Pure construction; the
    /// caller allocates the id and positions the buffer.
    pub fn from_text(id: BufferId, text: &str, path: Option<PathBuf>, area: (u16,u16)) -> Buffer;
    /// Open `path` and build a named Buffer, mirroring run()'s open branch:
    /// Ok → named clean; NotFound → named empty "new file". Other OpenErrors propagate.
    pub fn from_file(id: BufferId, path: &Path, area: (u16,u16)) -> Result<Buffer, crate::file::OpenError>;
}
```
`Editor::new_from_text` is reimplemented on top of `from_text` (behavior unchanged), and
`app::run()`'s initial open is refactored to `alloc_id` + `Buffer::from_file` (same
error→status mapping for Binary/Permission/IsDir/IO, same "new file" for NotFound).

- In-app open uses `open_into_current(editor, path)` that:
  1. **allocates a NEW `BufferId`** (`editor.alloc_id()`) and builds via `Buffer::from_file`
     — a fresh id so any **in-flight save/swap job for the replaced buffer** (tagged with the
     old id+version) is ignored by `apply_result` instead of merging into the new file
     (close-vs-in-flight safety; the same hazard Effort 6 must handle).
  2. on `OpenError` (Binary/Permission/IsDir/IO) → set status, **do NOT replace** the buffer.
  3. replace the active buffer with the new one.
  4. **restore `state.rs` resume cursor + marks** for `path` when `editor.resume_enabled`
     (staleness-guarded via `state::file_identity`, mirroring launch — see the context note
     in §4): `open_into_current` reloads the session store (`state::load()`) and applies
     `apply_resume`/`load_marks_from_entry`. `editor.resume_enabled: bool` is a new field
     seeded in `run()` from `cfg.state.resume` (so `open_into_current` works with only
     `&mut Editor`, which is all `apply_result` has — §4).
  5. `derive::rebuild` + `nav::ensure_visible`; area = `editor.active().view.area`.
- **Effort 6 forward-compat:** `Buffer::from_file(id, …)` lets Effort 6 `alloc_id` + append a
  new buffer instead of replacing; the file-browser overlay (below) is buffer-agnostic.

**Crash-safety swap recovery on in-app open — DEFERRED (v1).** Launch runs `swap::assess`
and may raise a recovery modal; doing the same from the in-app open path (especially when the
open fires from inside the save-completion handler, §4) would mean opening a modal while
other overlay state is in play — real sequencing/reentrancy complexity (**Codex**). For v1,
in-app open does **not** run swap recovery: the swap file is left on disk untouched, so the
**next launch** of that file still recovers it (no data loss — only a deferred prompt). This
is a documented gap (§10), a clean follow-on once the open path is settled.

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
// editor.pending_after_save: Option<PendingAfterSave>  (replaces quit_after_save/_at;
//   keeps the SAME version-match + SAVE_QUIT_TIMEOUT_MS timeout machinery)
```
- `apply_result`, on a save result whose version matches `pending_after_save.version`,
  performs the action: `Quit` → `editor.quit = true` (today's behavior); `Open(p)` →
  `open_into_current(editor, p)`; `New` → replace with scratch. The existing **timeout**
  clears `pending_after_save` for all actions.
- **`apply_result` context (Codex).** `apply_result(r, editor)` has only `&mut Editor` — no
  `clock`/`msg_tx`/`session`. The actions are chosen to need nothing more: `Open(p)` =
  `open_into_current` (self-contained — reloads `state::load()`, uses `editor.view.area`,
  gates resume on `editor.resume_enabled`; **swap recovery deferred**, §2); `New`/`Quit`
  trivial. So no `apply_result` signature change is required.

**Chained "Save then act" — explicit state carriers (Codex).** The pending action can be
armed as `PendingAfterSave { version, action }` **only once a save job is dispatched** (the
version is known then). For the dirty-guard `Save` choice:
- **Named buffer:** dispatch `save` and arm `pending_after_save { version, action }` directly.
- **Unnamed buffer:** there is no path, so `Save` opens **Save-As** (§5) carrying the action
  in a new field **`editor.pending_save_as: Option<PostSaveAction>`** (the *target path* is
  the minibuffer text; the *action* is what to do after). The carrier survives the Save-As
  minibuffer AND a possible `OverwriteSaveAs` confirm; when the Save-As write is finally
  dispatched, it transfers into `pending_after_save { version, action }`. Esc/Cancel anywhere
  in the chain clears `pending_save_as` (abort, buffer untouched). Chain:
  dirty-guard → Save-As minibuffer → [overwrite confirm] → write dispatched (arm) → result.

**Save-and-quit reconciliation (Codex).** `PromptAction::SaveAndQuit` now arms
`PostSaveAction::Quit`:
- **Named buffer** save-and-quit is **behavior-identical to 9B** (arm `{version, Quit}` on the
  real save dispatch). The existing `save_and_quit_sets_quit_after_save_…` test passes (now
  asserting `pending_after_save == Some({version, Quit})` instead of `quit_after_save`).
- **Unnamed buffer** save-and-quit does **not arm** at the prompt-resolution point (no save is
  dispatched — it opens Save-As carrying `Quit`). The existing
  `save_and_quit_on_unnamed_buffer_does_not_arm` test still holds (nothing is armed when it
  checks). After the user names+saves via Save-As, the `Quit` fires.

---

## 5. Save-As

**Command `save_as`** (cua: `ctrl-shift-s`), and the first `save` of any **unnamed** buffer,
route here. Replaces the `save.rs:90` / `:125` stub.

- **Prompt:** a `MinibufferKind::SaveAs` minibuffer ("Save as: ", pre-filled with the active
  document's dir or cwd, trailing separator) — same overlay `goto_line` uses, submit routes
  on `mb.kind`.
- **Save job API refactor (Codex).** Today `do_save` reads `document.path` and **panics if
  absent**, and the worker writes that captured path. Save-As cannot ride that. Refactor into
  an explicit-target form:
  ```rust
  enum SaveMode { Normal, SaveAs }      // SaveAs carries the prior swap key in the merge
  fn do_save_to(ctx, target: PathBuf, mode: SaveMode)   // dispatches a Save job for `target`
  ```
  `do_save` (named, `Normal`) becomes `do_save_to(ctx, document.path.clone().unwrap(), Normal)`
  — behavior unchanged. Save-As calls `do_save_to(ctx, P, SaveAs)`.
- **Submit** (`save_as_submit(editor, text)`):
  1. Resolve the typed path (expand `~`, make absolute relative to cwd) → target `P`. Empty
     input → no-op + status (a carried `pending_save_as` action is cleared).
  2. **Overwrite confirm:** if `P` exists, raise a **new** `PromptAction::OverwriteSaveAs`
     modal — **NOT** `PromptAction::Overwrite` (that means *external-mod* overwrite and calls
     `save::overwrite_save`, the wrong path; Export already needed its own `OverwriteExport`).
     A `pending_save_overwrite: Option<PathBuf>` holds `P` across the confirm (mirror
     `pending_export`); on **OverwriteSaveAs** → proceed to write `P`, on **Cancel** → abort
     (clear `pending_save_overwrite` AND any carried `pending_save_as` action).
  3. **Write:** `do_save_to(ctx, P, SaveAs)`. The target `P` rides the job; the buffer's
     `document.path` is **not** mutated yet. If a `pending_save_as` action was carried (the
     dirty-guard/save-and-quit chain, §4), transfer it now into
     `pending_after_save { version, action }`.
  4. **Re-key, on write SUCCESS only (correctness crux):** in the `apply_result` SaveAs merge:
     capture the **prior** swap key (the unnamed scratch key `None`, or the previous path),
     set `document.path = Some(P)`, refresh `stored_fp = fingerprint(P)` and
     `saved_version = v`. Then **staged swap re-key (Codex stale-save hazard):** delete the
     prior-key swap, and **if the buffer is now dirty** (`version != v` — edited during the
     save) **immediately write a fresh swap under `P`** (`swap::write_atomic`/
     `dispatch_swap_write`) so crash protection is never dropped for the new content. (Mirrors
     normal save's "only delete swap when `version == v`," extended to also protect `P` when
     the user kept typing.)
- **Failure rule (no state loss):** because `P` rides the job and `document.path` is mutated
  only in the success merge, a failed Save-As write → status message, the buffer stays
  **exactly as it was** (unnamed buffers stay `path == None`, still dirty; the prior/scratch
  swap is **not** deleted and still protects the work), and any `pending_after_save` armed
  against this write is cleared.

**`save` (named buffer)** is unchanged (`do_save_to(.., Normal)`). The no-path branch changes:
instead of the dead stub, `save` on an unnamed buffer **invokes `save_as`** (opens the prompt).

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
| `wordcartel/src/editor.rs` | `Buffer::from_text(id,..)` + `Buffer::from_file(id, path, area)`; reimplement `new_from_text` on `from_text`; generalize `quit_after_save`/`_at` → `pending_after_save: Option<PendingAfterSave>` + `PostSaveAction`/`PendingAfterSave`; new fields `resume_enabled: bool`, `pending_save_as: Option<PostSaveAction>`, `pending_save_overwrite: Option<PathBuf>`, `file_browser: Option<FileBrowser>`; `new`-scratch + replace-active helpers; add `file_browser=None` to every `open_*` XOR helper |
| `wordcartel/src/file_browser.rs` (new) | `FileBrowser` state + `rebuild_entries` (read_dir, dirs-first, `..`, substring filter) — mirrors `theme_picker.rs`; `lib.rs` `pub mod file_browser` |
| `wordcartel/src/registry.rs` | register `open`, `save_as`, `new`; clear `file_browser` in the menu-command + `dispatch_overlay_command` overlay-clearing paths (as `theme_picker` is cleared) |
| `wordcartel/src/keymap.rs` | cua binds `ctrl-o`→`open`, `ctrl-shift-s`→`save_as`, `ctrl-n`→`new` (verified free; WordStar binds deferred) |
| `wordcartel/src/minibuffer.rs` | `MinibufferKind::SaveAs` |
| `wordcartel/src/prompt.rs` | `PromptAction::OverwriteSaveAs` (distinct from `Overwrite`/`OverwriteExport`); a `Prompt::save_overwrite(P)` constructor + the dirty-guard `Save/Discard/Cancel` prompt constructor |
| `wordcartel/src/app.rs` | `file_browser` key block + render (mirror the `theme_picker` block ~app.rs:870 + the other overlay blocks at 729/780/935/990/1021/1083/1105); submit routes `SaveAs`; dirty-guard helpers; `open_into_current`; `save_as_submit`; `apply_result` performs `PostSaveAction` + the SaveAs merge/staged re-key; generalize the `PromptAction::SaveAndQuit` arm + add `OverwriteSaveAs` arm; `file_browser` XOR additions across the overlay blocks; refactor `run()` to `alloc_id`+`Buffer::from_file` and seed `editor.resume_enabled = cfg.state.resume` |
| `wordcartel/src/save.rs` | refactor `do_save` → `do_save_to(ctx, target, SaveMode)`; no-path `save` → invoke `save_as`; SaveAs merge (re-key on success) |
| `wordcartel/src/mouse.rs` | absorb mouse events while `file_browser` is open (mirror the palette/menu/theme_picker special-cases at mouse.rs:91/118/143) |
| `wordcartel/src/state.rs` | (reuse) `open_into_current` reloads `state::load()` + `apply_resume`/`load_marks_from_entry` gated on `resume_enabled` |

---

## 8. Error handling

- **Open** (in-app, via browser → existing entries): Binary/Permission/IsDir/IO → status
  message, **buffer NOT replaced** (you keep your work); NotFound shouldn't occur from the
  browser, but `Buffer::from_file` still maps it to a named "new file" for the launch caller.
- **Save-As** write failure → status; the buffer is **unchanged** (`document.path` is not
  mutated on failure — §5 failure rule; an unnamed buffer stays unnamed/dirty); any armed
  `pending_after_save` is cleared.
- **Dirty-guard Cancel / Esc** → always safe abort, buffer untouched; clears `pending_save_as`.
- **Unreadable directory** in the browser → status, remain in the previous dir.
- **Swap on in-app open: DEFERRED (§2).** In-app open does not run `swap::assess`; the swap
  file is left on disk and the next launch of that file recovers it (no data loss, deferred
  prompt). Launch-open behavior is unchanged.

---

## 9. Testing

- **`Buffer::from_file(id, …)`:** Ok (named, clean, content matches, carries the given id);
  NotFound → named empty "new file"; Binary/Permission/IsDir/IO → the right `OpenError`.
  `run()` still maps each to the same status as before (regression). `open_into_current`
  allocates a **fresh id** (assert the replaced buffer's id changed → stale in-flight results
  for the old id are ignored by `apply_result`).
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
  under the old key); existing-target → **`OverwriteSaveAs`** modal (distinct from
  `Overwrite`/`OverwriteExport`; Overwrite writes, Cancel aborts); empty input → no-op.
- **save_as staged re-key (dirty-during-save):** if the buffer is edited while the Save-As
  write is in flight (`version != v` at merge), after re-key a fresh swap exists under the new
  path `P` (crash protection not dropped).
- **save_as (failure invariant):** a write that fails leaves the buffer **unchanged** —
  unnamed buffer stays `path == None`, still dirty, and its scratch swap is **not** deleted
  (still protects the unsaved work); `pending_after_save` cleared.
- **save (unnamed) → save_as:** `save` on a path-less buffer opens the SaveAs minibuffer
  (no longer the dead stub).
- **new:** `new` replaces with an unnamed clean scratch buffer; first `save` opens SaveAs;
  dirty-guard runs when the current buffer is dirty.
- **post-save generalization:** the named `save_and_quit_sets_…` test now asserts
  `pending_after_save == Some({version, Quit})`; the unnamed `save_and_quit_…_does_not_arm`
  test still holds (nothing armed at the resolution point — it opens Save-As carrying `Quit`);
  timeout clears `pending_after_save`.

---

## 10. Out of scope (explicitly deferred)

- **Recent files** — `state.rs` already LRU-keys opened paths, so a recents list is a cheap
  later add; no picker UI for it now.
- **`tui-popup`** dependency — the existing `PromptAction` modal model handles all dialogs.
- **`nucleo` fuzzy filter + recursive gitignore-aware ("D") finder** — adopt with that
  feature; v1 uses substring filtering of one directory at a time.
- **Unified save-mode browser** — Save-As is a minibuffer prompt in v1.
- **Swap recovery on in-app open** (Codex) — deferred to avoid raising a recovery modal from
  the save-completion/open path; the swap persists and the next launch recovers it. Clean
  follow-on once the open path is settled.
- **WordStar `^KR` file-read / `^KW` write-block** — `^KW` is Effort 9A; `^KR` deferred.
- **Multi-buffer (Effort 6)** — not built here, but `Buffer::from_file` + the buffer-agnostic
  browser are the seams it will reuse.
