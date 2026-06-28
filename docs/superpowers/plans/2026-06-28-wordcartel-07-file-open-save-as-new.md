# File Open / Save-As / New (Effort 7) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the editor open another file, save to a new name, and start a fresh document from within the app — closing the biggest completeness gap (today files load only via the launch CLI arg; unnamed buffers can't save).

**Architecture:** All in the `wordcartel` shell. A buffer-extensible `Buffer::from_file(id,..)` seam (factored from `run()`); a `theme_picker`-style `file_browser` overlay (Open); a `MinibufferKind::SaveAs` prompt (Save-As); a `new` scratch command. The 9B `quit_after_save` mechanism is generalized to `pending_after_save { buffer_id, version, action: PostSaveAction::{Quit,Open,New}, at_ms }` so "Save then proceed" works; the save merge closure does the SaveAs re-key. No `wordcartel-core` change. Spec: `docs/superpowers/specs/2026-06-28-wordcartel-07-file-open-save-as-new-design.md`.

**Tech Stack:** Rust, ratatui 0.30, crossterm, `std::fs`.

## Global Constraints

- **No `wordcartel-core` change.** New commands register through the name-keyed registry (palette-reachable, config-bindable). cua binds `ctrl-o`→`open`, `ctrl-shift-s`→`save_as`, `ctrl-n`→`new` (all verified free); WordStar binds deferred.
- **`BufferId`s are never reused** (job routing is by `BufferId`). `Buffer::from_file` takes a caller-allocated id; `open_into_current` allocates a **fresh** id so an in-flight save/swap job for the *replaced* buffer — whose merge routes through `by_id_mut(old_id)` — finds **no** matching buffer and harmlessly no-ops (it cannot mutate the new file). (Durability jobs are not dropped by `is_stale`; the id mismatch is what protects them — Codex.)
- **Save-As failure must not mutate state:** the target path rides the job; `document.path`/`stored_fp`/`saved_version` and the prior swap change **only in the success merge**.
- **In-app swap recovery is DEFERRED** (the swap persists; next launch recovers). Resume-on-open is kept.
- **Single-overlay XOR:** `file_browser` joins the XOR set everywhere `theme_picker` is cleared.
- TDD, frequent commits. Every commit ends with the trailers:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` / `Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6`

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `wordcartel/src/editor.rs` | `Buffer::from_text(id,..)`/`from_file(id,..)`; `new_from_text` on top; new fields (`resume_enabled`, `pending_after_save`, `pending_save_as`, `pending_save_overwrite`, `file_browser`); `PostSaveAction`/`PendingAfterSave`; XOR-clear `file_browser` in every `open_*` | 1,2,3,4,5 |
| `wordcartel/src/save.rs` | `do_save`→`do_save_to(ctx,target,SaveMode)`; no-path `save`→`save_as`; SaveAs merge (re-key on success) | 3 |
| `wordcartel/src/minibuffer.rs` | `MinibufferKind::SaveAs` | 3 |
| `wordcartel/src/prompt.rs` | `PromptAction::OverwriteSaveAs`; `Prompt::save_overwrite`/`dirty_guard` constructors | 3,4 |
| `wordcartel/src/file_browser.rs` (new) | `FileBrowser` state + `rebuild_entries` (mirror `theme_picker.rs`) | 5 |
| `wordcartel/src/app.rs` | post-save generalization; `save_as_submit`; dirty-guard helpers; `open_into_current`; `file_browser` **key block** (in `reduce`) + XOR; `apply_result` PostSaveAction; `run()` seam + `resume_enabled` seed | 1–5 |
| `wordcartel/src/render.rs` | `file_browser` **render** (overlay rendering lives here, not app.rs — render.rs:686/746/792) | 5 |
| `wordcartel/src/registry.rs` | register `open`/`save_as`/`new`; clear `file_browser` in menu/`dispatch_overlay_command` | 3,4,5 |
| `wordcartel/src/keymap.rs` | cua `ctrl-o`/`ctrl-shift-s`/`ctrl-n` | 3,4,5 |
| `wordcartel/src/mouse.rs` | absorb mouse while `file_browser` open | 5 |
| `wordcartel/src/lib.rs` | `pub mod file_browser` | 5 |

**Task order: 1 → 2 → 3 → 4 → 5.** Each ends with an independently testable deliverable. Task 4 (dirty-guard) needs 2's post-save + 3's save_as; Task 5 (open) needs 4's dirty-guard + 1's seam.

---

## Task 1: Buffer construction seam + `open_into_current` + `run()` refactor + `resume_enabled`

**Files:**
- Modify: `wordcartel/src/editor.rs` (`Buffer::from_text`/`from_file`, reimplement `new_from_text`, add `resume_enabled` field)
- Modify: `wordcartel/src/app.rs` (`open_into_current` + a factored `restore_resume`; `run()` initial-open branch → `alloc_id`+`Buffer::from_file`; seed `editor.resume_enabled`)
- Test: `wordcartel/src/editor.rs` + `wordcartel/src/app.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `pub fn Buffer::from_text(id: BufferId, text: &str, path: Option<PathBuf>, area: (u16,u16)) -> Buffer`; `pub fn Buffer::from_file(id: BufferId, path: &Path, area: (u16,u16)) -> Result<Buffer, crate::file::OpenError>`; `Editor.resume_enabled: bool`; `pub fn app::open_into_current(editor: &mut Editor, path: &Path)` (used by Tasks 2/4/5).
- Consumes: `TextBuffer::from_str`, `Selection::single`, `History::default`, `block_tree::full_parse_rope`, `Document`, `View`, `crate::save::fingerprint`, `crate::file::open`, `Editor::alloc_id`, the launch resume block (`state::load`/`apply_resume`/`load_marks_from_entry`/`file_identity`).

> **`open_into_current` lives here** (it is the buffer-load seam), so Tasks 2 (apply_result Open arm), 4 (dirty-guard Open), and 5 (browser) all call an already-defined fn — no forward dependency.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn buffer_from_file_ok_named_clean() {
        let p = std::env::temp_dir().join(format!("wc-fromfile-{}.md", std::process::id()));
        std::fs::write(&p, "hello\nworld\n").unwrap();
        let mut e = Editor::new_from_text("\n", None, (40, 10)); // host editor for ids
        let id = e.alloc_id();
        let b = Buffer::from_file(id, &p, (40, 10)).expect("ok");
        assert_eq!(b.id, id);
        assert_eq!(b.document.buffer.to_string(), "hello\nworld\n");
        assert_eq!(b.document.path.as_deref(), Some(p.as_path()));
        assert!(!b.document.dirty(), "freshly opened file is clean");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn buffer_from_file_not_found_is_named_new_file() {
        let p = std::env::temp_dir().join(format!("wc-missing-{}.md", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut e = Editor::new_from_text("\n", None, (40, 10));
        let id = e.alloc_id();
        let b = Buffer::from_file(id, &p, (40, 10)).expect("NotFound → named empty buffer, not Err");
        assert_eq!(b.document.path.as_deref(), Some(p.as_path()));
        assert_eq!(b.document.buffer.to_string(), "\n");
    }

    #[test]
    fn buffer_from_file_binary_is_err() {
        let p = std::env::temp_dir().join(format!("wc-bin-{}.bin", std::process::id()));
        std::fs::write(&p, [0u8, 159, 146, 150]).unwrap(); // invalid UTF-8 / NUL
        let mut e = Editor::new_from_text("\n", None, (40, 10));
        let id = e.alloc_id();
        assert!(matches!(Buffer::from_file(id, &p, (40, 10)), Err(crate::file::OpenError::Binary(_))));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn new_from_text_still_builds_one_buffer_id_zero() {
        let e = Editor::new_from_text("abc\n", None, (40, 10));
        assert_eq!(e.active().id, BufferId(0));
        assert_eq!(e.active().document.buffer.to_string(), "abc\n");
        assert!(!e.resume_enabled, "default false until run() seeds it"); // field exists, defaults false
    }
```
And in `app.rs` `#[cfg(test)]` (for `open_into_current`):
```rust
    #[test]
    fn open_into_current_replaces_with_fresh_id_and_clean() {
        use crate::editor::Editor;
        let p = std::env::temp_dir().join(format!("wc-oic-{}.md", std::process::id()));
        std::fs::write(&p, "opened\n").unwrap();
        let mut e = Editor::new_from_text("scratch\n", None, (80, 24));
        let old_id = e.active().id;
        crate::app::open_into_current(&mut e, &p);
        assert_ne!(e.active().id, old_id, "fresh id → stale in-flight jobs for old buffer are ignored");
        assert_eq!(e.active().document.buffer.to_string(), "opened\n");
        assert!(!e.active().document.dirty());
        let _ = std::fs::remove_file(&p);
    }
```

> Confirm the real `file::OpenError` variants (`NotFound`/`Binary`/`Permission`/`IsDir`/`Io`) and that `Buffer` fields match the `new_from_text` push (id, document, view, desired_col, …). Mirror that field list exactly in `from_text`.

- [ ] **Step 2: Run — fails** (`Buffer::from_text`/`from_file` undefined, no `resume_enabled`). `cargo test -p wordcartel buffer_from_file`

- [ ] **Step 3: Implement.** In `editor.rs`, extract `from_text` from `new_from_text`'s buffer construction, add `from_file`, add the field:
```rust
impl Buffer {
    /// Pure construction of a Buffer with a caller-supplied id (id source = alloc_id).
    pub fn from_text(id: BufferId, text: &str, path: Option<std::path::PathBuf>, area: (u16,u16)) -> Buffer {
        let buffer = TextBuffer::from_str(text);
        let blocks = block_tree::full_parse_rope(&buffer.snapshot());
        let document = Document {
            buffer, selection: Selection::single(0), history: History::default(), blocks, version: 0,
            stored_fp: path.as_deref().and_then(crate::save::fingerprint),
            path, saved_version: Some(0),
        };
        let view = View { scroll: 0, scroll_row: 0, area, mode: RenderMode::LivePreview, line_layouts: BTreeMap::new() };
        Buffer {
            id, document, view,
            desired_col: None, pre_edit_rope: None, last_edit: None,
            last_edit_at: None, last_swap_at: None, swap_in_flight: false,
            pending_swap_body: None, pending_swap_path: None,
            marks: Default::default(), jump_ring: Vec::new(), ring_cursor: 0, sel_history: Vec::new(),
            diagnostics: crate::diagnostics_run::DiagStore::new(),
            folds: crate::fold::FoldState::default(),
        }
    }

    /// Open `path` into a named Buffer, mirroring run()'s open branch:
    /// Ok → named clean; NotFound → named empty "new file" (`"\n"`); other errors propagate.
    pub fn from_file(id: BufferId, path: &std::path::Path, area: (u16,u16)) -> Result<Buffer, crate::file::OpenError> {
        match crate::file::open(path) {
            Ok(text) => Ok(Buffer::from_text(id, &text, Some(path.to_path_buf()), area)),
            Err(crate::file::OpenError::NotFound(_)) => Ok(Buffer::from_text(id, "\n", Some(path.to_path_buf()), area)),
            Err(e) => Err(e),
        }
    }
}
```
Reimplement `new_from_text` to use `from_text` (replace the inline `document`/`view`/`Buffer{..}` + `e.buffers.push(Buffer{..})` with `let id = e.alloc_id(); e.buffers.push(Buffer::from_text(id, text, path, area));`). Add `pub resume_enabled: bool,` to the `Editor` struct and `resume_enabled: false,` to the `new_from_text` initializer.

- [ ] **Step 4: Refactor `run()`'s initial open** (app.rs ~1351, the `match path.as_deref()` block) to use the seam without behavior change:
```rust
    let mut editor = Editor::new_from_text("\n", None, area); // scratch host; we set the real buffer below
    match path.as_deref() {
        None => { /* keep the scratch buffer */ }
        Some(p) => {
            let id = editor.active().id; // reuse slot 0's id for the launch buffer
            match crate::editor::Buffer::from_file(id, p, area) {
                Ok(b) => { editor.buffers[0] = b; }
                Err(e @ (file::OpenError::Binary(_) | file::OpenError::Permission(_)
                       | file::OpenError::IsDir(_) | file::OpenError::Io(_))) => {
                    editor.status = e.to_string(); // UNNAMED scratch kept (can't clobber)
                }
            }
            if editor.active().document.path.is_some()
               && !std::path::Path::new(p).exists() { editor.status = "new file".to_string(); }
        }
    }
```
> Match `run()`'s EXACT prior status strings (Binary/Permission/IsDir/IO → `e.to_string()`; NotFound → "new file"; rejected target stays UNNAMED). Adapt the refactor so the existing run/open behavior tests pass byte-for-byte — the deliverable is "no behavior change," verified by the existing suite. Seed `editor.resume_enabled = cfg.state.resume;` near the other `editor.*` seeds (after `editor.view_opts = …`).

- [ ] **Step 5: Implement `open_into_current` + factor `restore_resume`.** Factor the launch resume logic (run()'s resume block ~app.rs:1506 — `state::load`/`file_identity` staleness guard/`apply_resume`/`load_marks_from_entry`) into a shared `fn restore_resume(editor: &mut Editor, path: &Path)` that **reloads `state::load()`** (so it works with only `&mut Editor`, which `apply_result` has), and call it from BOTH `run()` (replacing the inline block — behavior unchanged) and `open_into_current`:
```rust
pub fn open_into_current(editor: &mut crate::editor::Editor, path: &std::path::Path) {
    let id = editor.alloc_id(); // FRESH id → an in-flight job for the old buffer merges via by_id_mut(old_id)=None (no-op)
    let area = editor.active().view.area;
    match crate::editor::Buffer::from_file(id, path, area) {
        Ok(b) => {
            let a = editor.active; editor.buffers[a] = b;
            if editor.resume_enabled { restore_resume(editor, path); }
            crate::derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
            editor.status = String::new();
        }
        Err(e) => { editor.status = e.to_string(); } // do NOT replace — keep the user's work
    }
}
```
> `restore_resume` must reuse the SAME launch resume block verbatim (app.rs ~1506-1520): the `state::file_identity` mtime+size staleness guard, `apply_resume` (cursor+scroll), `load_marks_from_entry`, **AND the fold restore + `folds.reconcile(...)`** the launch path does (Codex — don't drop folds). Factor, don't fork; keep launch behavior byte-identical (existing resume tests pass).

- [ ] **Step 6: Run** `cargo test -p wordcartel buffer_from_file` + `cargo test -p wordcartel open_into_current` + `cargo test -p wordcartel --lib` — green (all existing open/run/resume tests pass unchanged).

- [ ] **Step 7: Commit** `feat(7): Buffer seam (from_text/from_file) + open_into_current + run() refactor + resume_enabled`

---

## Task 2: Post-save generalization (`quit_after_save` → `pending_after_save`)

**Files:**
- Modify: `wordcartel/src/editor.rs` (`PostSaveAction`/`PendingAfterSave`; replace `quit_after_save`/`quit_after_save_at` fields)
- Modify: `wordcartel/src/app.rs` (`apply_result` post-merge check; the `SaveAndQuit` arm; the save-quit timeout ~1566)
- Modify: `wordcartel/src/save.rs` (`dispatch_save_and_quit` arms `pending_after_save`)
- Test: `wordcartel/src/app.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `pub enum PostSaveAction { Quit, Open(PathBuf), New }`; `pub struct PendingAfterSave { pub buffer_id: BufferId, pub version: u64, pub action: PostSaveAction, pub at_ms: u64 }`; `Editor.pending_after_save: Option<PendingAfterSave>`; `save::dispatch_save_then(ctx, action)`.
- Consumes: `apply_result`'s `(kind, version, buffer_id)`, `editor.by_id`, `SAVE_QUIT_TIMEOUT_MS`, `editor.replace_active_with_scratch` (added here), `app::open_into_current` (defined in Task 1).

> **Scope note:** only the **`Quit`** arm is exercised by a caller in this task (the existing save-and-quit). The `New`/`Open` arms are defined now (they compile — `New` → `replace_active_with_scratch`, `Open(p)` → `open_into_current`, both already exist) and are *triggered* in Tasks 4/5. No stubs needed.

- [ ] **Step 1: Write the failing test** (the existing save-and-quit behavior must hold through the field rename)

```rust
    #[test]
    fn save_and_quit_arms_pending_after_save_quit_and_exits() {
        use crate::editor::{Editor, PostSaveAction};
        use crate::jobs::{Executor, InlineExecutor};
        let p = std::env::temp_dir().join(format!("wc-pas-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None; e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        {
            let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx.clone() };
            crate::save::dispatch_save_and_quit(&mut ctx);
        }
        assert!(matches!(e.pending_after_save, Some(crate::editor::PendingAfterSave { version: 1, action: PostSaveAction::Quit, .. })));
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(e.quit, "matching save result triggers quit");
        let _ = std::fs::remove_file(&p);
    }
```

> Copy the harness from the existing `save_and_quit_command_arms_…` test (Effort 9B). Also UPDATE the existing 9B tests that referenced `quit_after_save`/`quit_after_save_at` to the new field (mechanical rename; keep their assertions equivalent — `pending_after_save.is_none()` where they asserted `quit_after_save == None`).

- [ ] **Step 2: Run — fails** (`pending_after_save` undefined). `cargo test -p wordcartel save_and_quit`

- [ ] **Step 3: Implement.** In `editor.rs` add the types + field, remove `quit_after_save`/`quit_after_save_at`:
```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostSaveAction { Quit, Open(std::path::PathBuf), New }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingAfterSave { pub buffer_id: BufferId, pub version: u64, pub action: PostSaveAction, pub at_ms: u64 }
// in Editor: pub pending_after_save: Option<PendingAfterSave>,   (init None)
```
> **`buffer_id` added (Codex advisory):** carry the originating buffer id so the action fires only for *that* buffer's save — version-only is single-buffer-only; this keeps Effort 6 from re-threading it.

Add a trivial helper used by `New` (and the New command, Task 4):
```rust
impl Editor {
    /// Replace the active buffer with a fresh unnamed scratch buffer, then relayout.
    pub fn replace_active_with_scratch(&mut self) {
        let id = self.alloc_id();
        let area = self.active().view.area;
        let a = self.active; self.buffers[a] = Buffer::from_text(id, "\n", None, area);
    }
}
```
> After calling `replace_active_with_scratch`, the caller runs `derive::rebuild(editor)` + `nav::ensure_visible(editor)` (the new scratch buffer's `line_layouts` is empty — Codex). The `apply_result` `New` arm and `request_new` both do this.

**Factor the arming into a general `dispatch_save_then` (Codex #4/#5).** In `save.rs`, replace `dispatch_save_and_quit`'s inline arming with a reusable form that goes through `dispatch_save` (so the **external-mod fingerprint check is NOT bypassed**) and arms only if a job was actually dispatched:
```rust
/// The unified "save, then do `action`" entry. Goes through `dispatch_save`
/// (external-mod-checked). Handles all three buffer states (Codex re-review):
/// - NAMED, no conflict → a save job is dispatched → arm `pending_after_save{action}`.
/// - NAMED, external-mod conflict → `dispatch_save` raised the modal → do NOT arm
///   (the user resolves the modal and re-issues).
/// - UNNAMED → `dispatch_save` opened the Save-As minibuffer → carry the action in
///   `pending_save_as` so it fires after the Save-As write completes.
pub(crate) fn dispatch_save_then(ctx: &mut Ctx, action: crate::editor::PostSaveAction) {
    let was_unnamed = ctx.editor.active().document.path.is_none();
    let buffer_id = ctx.editor.active().id;
    let v = ctx.editor.active().document.version;
    dispatch_save(ctx);
    if was_unnamed {
        // dispatch_save opened Save-As (MinibufferKind::SaveAs) for the no-path buffer.
        if ctx.editor.minibuffer.as_ref().map(|m| m.kind) == Some(crate::minibuffer::MinibufferKind::SaveAs) {
            ctx.editor.pending_save_as = Some(action);
        }
    } else if ctx.editor.active().document.path.is_some() && ctx.editor.prompt.is_none() {
        ctx.editor.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id, version: v, action, at_ms: ctx.clock.now_ms(),
        });
    }
}
pub(crate) fn dispatch_save_and_quit(ctx: &mut Ctx) { dispatch_save_then(ctx, crate::editor::PostSaveAction::Quit); }
```
In `app.rs::apply_result`, replace the `quit_after_save` block with the generalized dispatch (gated on the originating buffer id):
```rust
    if kind == crate::jobs::JobKind::Save {
        if let Some(p) = &editor.pending_after_save {
            let saved_clean = editor.by_id(buffer_id).map(|b| b.document.saved_version) == Some(Some(version));
            if p.buffer_id == buffer_id && p.version == version && saved_clean {
                let action = editor.pending_after_save.take().unwrap().action;
                match action {
                    crate::editor::PostSaveAction::Quit => editor.quit = true,
                    crate::editor::PostSaveAction::New  => { editor.replace_active_with_scratch(); crate::derive::rebuild(editor); crate::nav::ensure_visible(editor); }
                    crate::editor::PostSaveAction::Open(path) => crate::app::open_into_current(editor, &path),
                }
            }
        }
    }
```
Generalize the save-quit **timeout** (app.rs ~1558, the `quit_after_save_at`/`SAVE_QUIT_TIMEOUT_MS` block) to read/clear `editor.pending_after_save` (`p.at_ms`). Update the existing tests that reference `quit_after_save`/`quit_after_save_at` (app.rs ~2168) to the new field.

- [ ] **Step 4: Run** `cargo test -p wordcartel save_and_quit` + `cargo test -p wordcartel --lib` — green (the unnamed-no-arm test still passes; the timeout test still passes).

- [ ] **Step 5: Commit** `feat(7): generalize quit_after_save → pending_after_save (PostSaveAction)`

---

## Task 3: Save-As (`do_save_to`, `MinibufferKind::SaveAs`, `save_as`, overwrite, re-key)

**Files:**
- Modify: `wordcartel/src/save.rs` (`do_save`→`do_save_to(ctx,target,SaveMode)`; no-path `save`→`save_as`; SaveAs merge)
- Modify: `wordcartel/src/minibuffer.rs` (`MinibufferKind::SaveAs`)
- Modify: `wordcartel/src/prompt.rs` (`PromptAction::OverwriteSaveAs` + `Prompt::save_overwrite`)
- Modify: `wordcartel/src/editor.rs` (`pending_save_as: Option<PostSaveAction>`, `pending_save_overwrite: Option<PathBuf>` fields)
- Modify: `wordcartel/src/registry.rs` (`save_as` command), `wordcartel/src/keymap.rs` (cua `ctrl-shift-s`)
- Modify: `wordcartel/src/app.rs` (`save_as_submit`; minibuffer submit routes `SaveAs`; `OverwriteSaveAs` resolve arm)
- Test: `wordcartel/src/app.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `save::do_save_to(ctx, target: PathBuf, mode: SaveMode)`; `SaveMode { Normal, SaveAs }`; `app::save_as_submit(editor, text, executor, clock, msg_tx)`; registry id `save_as`; `MinibufferKind::SaveAs`; `PromptAction::OverwriteSaveAs`; fields `pending_save_as`, `pending_save_overwrite`.
- Consumes: `file::save_atomic`, `swap::delete`, `fingerprint`, `Ctx`, `open_minibuffer`, `open_prompt`, the save merge-closure pattern.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn save_as_writes_new_path_and_rekeys() {
        use crate::editor::Editor;
        use crate::jobs::{Executor, InlineExecutor};
        let dir = std::env::temp_dir().join(format!("wc-saveas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("out.md");
        let _ = std::fs::remove_file(&p);
        let mut e = Editor::new_from_text("content\n", None, (80, 24)); // UNNAMED, dirty-ish
        e.active_mut().document.version = 1; e.active_mut().document.saved_version = None;
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::save_as_submit(&mut e, p.to_str().unwrap(), &ex, &clk, &tx);
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "content\n", "file written");
        assert_eq!(e.active().document.path.as_deref(), Some(p.as_path()), "path re-keyed");
        assert!(!e.active().document.dirty(), "clean after save-as");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_on_unnamed_buffer_opens_save_as_prompt() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
        crate::save::dispatch_save(&mut ctx); // no path → opens Save-As, NOT the dead stub
        assert!(matches!(e.minibuffer.as_ref().map(|m| m.kind),
            Some(crate::minibuffer::MinibufferKind::SaveAs)), "unnamed save opens the SaveAs minibuffer");
    }

    #[test]
    fn save_as_existing_target_raises_overwrite_prompt() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        let p = std::env::temp_dir().join(format!("wc-ow-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", None, (80, 24));
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::save_as_submit(&mut e, p.to_str().unwrap(), &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "existing target → confirm modal");
        assert_eq!(e.prompt.as_ref().unwrap().action_for('o'), Some(crate::prompt::PromptAction::OverwriteSaveAs));
        assert_ne!(crate::prompt::PromptAction::OverwriteSaveAs, crate::prompt::PromptAction::Overwrite);
        let _ = std::fs::remove_file(&p);
    }
```

- [ ] **Step 2: Run — fails** (`save_as_submit`/`SaveMode`/`MinibufferKind::SaveAs`/`OverwriteSaveAs` undefined). `cargo test -p wordcartel save_as`

- [ ] **Step 3: Implement the save-job refactor + SaveAs merge.** In `save.rs`:
```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SaveMode { Normal, SaveAs }   // Copy → free to `matches!` repeatedly in the moved closure (Codex)

/// Dispatch a Save job writing `target`. SaveMode::SaveAs re-keys the buffer on success.
pub(crate) fn do_save_to(ctx: &mut Ctx, target: std::path::PathBuf, mode: SaveMode) {
    ctx.editor.status = "Saving\u{2026}".to_string();
    let snap = ctx.editor.active().document.buffer.snapshot();
    let v = ctx.editor.active().document.version;
    let buffer_id = ctx.editor.active().id;
    let prior_key = ctx.editor.active().document.path.clone(); // for SaveAs swap re-key
    let write_path = target.clone();
    ctx.executor.dispatch(Job {
        buffer_id, class: ResultClass::Durability, version: v, kind: JobKind::Save,
        run: Box::new(move || {
            let content = snap.to_string();
            let outcome = file::save_atomic(&write_path, &content);
            let new_fp = fingerprint(&write_path);
            JobResult { buffer_id, class: ResultClass::Durability, version: v, kind: JobKind::Save,
                merge: Box::new(move |editor| {
                    let mut status = String::new();
                    if let Some(b) = editor.by_id_mut(buffer_id) {
                        match outcome {
                            Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged) => {
                                if matches!(mode, SaveMode::SaveAs) { b.document.path = Some(target.clone()); }
                                b.document.saved_version = Some(v);
                                b.document.stored_fp = new_fp;
                                if b.document.version == v {
                                    status = "Saved".to_string();
                                    crate::swap::delete(b.document.path.as_deref());
                                    if matches!(mode, SaveMode::SaveAs) { crate::swap::delete(prior_key.as_deref()); }
                                } else {
                                    status = format!("Saved v{v} (still editing)");
                                    // Staged re-key (Codex): the buffer was edited during the write
                                    // (now v+1). Delete the prior/scratch swap (its v content is now
                                    // ON DISK at `target`, and leaving a scratch swap would trigger a
                                    // spurious recovery next launch) and EXPEDITE a swap under the new
                                    // path: `last_swap_at = None` makes the next `due()` fire promptly,
                                    // writing a swap for the v+1 body under `target`. Exposure for the
                                    // v→v+1 keystrokes is bounded by the normal swap cadence (the same
                                    // window normal editing has between periodic swap writes).
                                    if matches!(mode, SaveMode::SaveAs) {
                                        crate::swap::delete(prior_key.as_deref());
                                        b.last_swap_at = None;
                                    }
                                }
                            }
                            Err(e) => { status = e.to_string(); } // failure: nothing mutated (path untouched)
                        }
                    }
                    editor.status = status;
                }),
            }
        }),
    });
}
```
Change `do_save` to delegate: `fn do_save(ctx) { let p = ctx.editor.active().document.path.clone().expect("…"); do_save_to(ctx, p, SaveMode::Normal); }`. Change `dispatch_save`'s **no-path** branch from the stub to opening Save-As:
```rust
        None => { crate::app::open_save_as(ctx.editor); return CommandResult::Handled; }
```
In `editor.rs` add fields `pending_save_as: Option<PostSaveAction>` (init None), `pending_save_overwrite: Option<std::path::PathBuf>` (init None). In `minibuffer.rs` add `SaveAs` to `MinibufferKind`. In `prompt.rs` add `OverwriteSaveAs` to `PromptAction` and:
```rust
    pub fn save_overwrite(target: &std::path::Path) -> Prompt {
        Prompt { message: format!("{} exists: [O]verwrite · [C]ancel", target.display()),
            choices: vec![
                Choice { key: 'o', label: "Overwrite", action: PromptAction::OverwriteSaveAs },
                Choice { key: 'c', label: "Cancel",    action: PromptAction::Cancel },
            ] }
    }
```
In `app.rs` add `open_save_as` + `save_as_submit` + route the minibuffer Enter + the resolve_prompt `OverwriteSaveAs` arm:
```rust
pub fn open_save_as(editor: &mut crate::editor::Editor) {
    let pre = editor.active().document.path.as_ref()
        .and_then(|p| p.parent()).map(|d| format!("{}/", d.display())).unwrap_or_default();
    editor.open_minibuffer("Save as: ", crate::minibuffer::MinibufferKind::SaveAs);
    if let Some(mb) = editor.minibuffer.as_mut() { mb.text = pre.clone(); mb.cursor = pre.len(); }
}

pub fn save_as_submit(editor: &mut crate::editor::Editor, text: &str,
                      executor: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
                      msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
    let t = text.trim();
    if t.is_empty() { editor.status = "save-as: empty path".into(); editor.pending_save_as = None; return; }
    // expand_path: ~ → home; relative → joined onto cwd. Mirror the ~ handling used by the
    // dictionary/config path loaders.
    let target: std::path::PathBuf = {
        let expanded = if let Some(rest) = t.strip_prefix("~/") {
            dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| std::path::PathBuf::from(t))
        } else { std::path::PathBuf::from(t) };
        if expanded.is_absolute() { expanded }
        else { std::env::current_dir().map(|d| d.join(&expanded)).unwrap_or(expanded) }
    };
    if target.exists() {
        editor.pending_save_overwrite = Some(target.clone());
        editor.open_prompt(crate::prompt::Prompt::save_overwrite(&target));
        return;
    }
    perform_save_as(editor, target, executor, clock, msg_tx);
}

fn perform_save_as(editor: &mut crate::editor::Editor, target: std::path::PathBuf,
                   executor: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
                   msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
    let v = editor.active().document.version;
    let buffer_id = editor.active().id;
    { let mut ctx = crate::registry::Ctx { editor, clock, executor, msg_tx: msg_tx.clone() };
      crate::save::do_save_to(&mut ctx, target, crate::save::SaveMode::SaveAs); }
    if let Some(action) = editor.pending_save_as.take() {
        editor.pending_after_save = Some(crate::editor::PendingAfterSave { buffer_id, version: v, action, at_ms: clock.now_ms() });
    }
}
```
Route the minibuffer `Enter` (app.rs ~1003) — add `crate::minibuffer::MinibufferKind::SaveAs => save_as_submit(editor, &mb.text, ex, clock, msg_tx)` — **the submit site's variables are named `ex`, `clock`, `msg_tx`** (not `executor`) — Codex. Add the resolve_prompt arm: `PromptAction::OverwriteSaveAs => { if let Some(t) = editor.pending_save_overwrite.take() { perform_save_as(editor, t, ex, clock, msg_tx); } }`. **Clear the carriers on EVERY cancel/dismiss path (Codex):** `PromptAction::Cancel` clears both `pending_save_overwrite` and `pending_save_as`; the **Save-As minibuffer Esc** (the minibuffer-dismiss path) clears `pending_save_as`; the **dirty-guard Cancel** (Task 4) clears `pending_save_as`. Register `save_as` (File menu) → `|c| { crate::app::open_save_as(c.editor); CommandResult::Handled }`; cua bind `("ctrl-shift-s", "save_as")`.

- [ ] **Step 4: Run** `cargo test -p wordcartel save_as` + `cargo test -p wordcartel --lib` — green (existing save tests unaffected: `do_save`→`do_save_to(Normal)` is behavior-identical).

- [ ] **Step 5: Commit** `feat(7): Save-As (do_save_to, MinibufferKind::SaveAs, OverwriteSaveAs, swap re-key)`

---

## Task 4: New command + dirty-guard mechanism

**Files:**
- Modify: `wordcartel/src/prompt.rs` (`Prompt::dirty_guard` — Save/Discard/Cancel; reuse `SaveAndQuit`? no — add `DiscardAndProceed`/use existing actions, see below)
- Modify: `wordcartel/src/editor.rs` (no new field beyond `pending_save_as`)
- Modify: `wordcartel/src/app.rs` (`guard_then(editor, action, …)` helper; resolve_prompt dirty-guard arms), `registry.rs` (`new`), `keymap.rs` (cua `ctrl-n`)
- Test: `wordcartel/src/app.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `app::request_replace(editor, action: PostSaveAction-less intent, …)` — the dirty-guard entry; registry id `new`; `PromptAction::{SaveAndProceed, DiscardAndProceed}` (proceed = perform the pending `PostSaveAction`).
- Consumes: `dirty()`, `open_prompt`, `replace_active_with_scratch`, `open_save_as`, `do_save_to`, `pending_after_save`/`pending_save_as`.

> **Design:** the dirty-guard carries the intended `PostSaveAction` (here `New`; Task 5 reuses it with `Open(p)`). Add two prompt actions — `SaveAndProceed` and `DiscardAndProceed` — and hold the intent in `editor.pending_save_as` (the same carrier; for the **named** Save path it transfers straight to `pending_after_save`).

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn new_on_clean_buffer_replaces_with_scratch() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("kept\n", None, (80, 24)); // clean (saved_version=Some(0))
        let (ex, clk, tx) = (crate::jobs::InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        crate::app::request_new(&mut e, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "\n", "clean buffer → immediate new scratch");
        assert!(e.active().document.path.is_none());
        assert!(e.prompt.is_none(), "no modal for a clean buffer");
    }

    #[test]
    fn new_on_dirty_buffer_raises_guard_modal() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("draft\n", None, (80, 24));
        e.active_mut().document.version = 1; // dirty (saved_version=Some(0))
        let (ex, clk, tx) = (crate::jobs::InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        crate::app::request_new(&mut e, &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "dirty buffer → Save/Discard/Cancel modal");
        assert_eq!(e.prompt.as_ref().unwrap().action_for('d'), Some(crate::prompt::PromptAction::DiscardAndProceed));
        // Discard → proceeds to scratch
        // (resolve via the resolve_prompt path in the integration test below)
    }
```

- [ ] **Step 2: Run — fails** (`request_new`/`PromptAction::DiscardAndProceed` undefined). `cargo test -p wordcartel new_on_`

- [ ] **Step 3: Implement.** `prompt.rs`: add `SaveAndProceed`, `DiscardAndProceed` to `PromptAction` + `Prompt::dirty_guard()` (`[S]ave · [D]iscard · [C]ancel`). `app.rs`:
```rust
/// Dirty-guard: perform `action` now if clean, else raise the Save/Discard/Cancel modal
/// (the intent is held in pending_save_as until the choice resolves).
fn request_replace(editor: &mut crate::editor::Editor, action: crate::editor::PostSaveAction,
                   ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
                   msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
    if !editor.active().document.dirty() { perform_post_save_action(editor, action, ex, clock, msg_tx); return; }
    editor.pending_save_as = Some(action);
    editor.open_prompt(crate::prompt::Prompt::dirty_guard());
}

/// Perform a PostSaveAction immediately (no save): used for the clean path and Discard.
fn perform_post_save_action(editor: &mut crate::editor::Editor, action: crate::editor::PostSaveAction,
                            ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
                            msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
    match action {
        crate::editor::PostSaveAction::New => { editor.replace_active_with_scratch(); crate::derive::rebuild(editor); crate::nav::ensure_visible(editor); }
        crate::editor::PostSaveAction::Open(p) => open_into_current(editor, &p), // Task 5 triggers; defined Task 1
        crate::editor::PostSaveAction::Quit => editor.quit = true,
    }
}

pub fn request_new(editor: &mut crate::editor::Editor, ex: &dyn crate::jobs::Executor,
                   clock: &dyn wordcartel_core::history::Clock, msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
    request_replace(editor, crate::editor::PostSaveAction::New, ex, clock, msg_tx);
}
```
resolve_prompt arms (the dirty-guard choices):
```rust
    PromptAction::DiscardAndProceed => {
        if let Some(action) = editor.pending_save_as.take() { perform_post_save_action(editor, action, ex, clock, msg_tx); }
    }
    PromptAction::SaveAndProceed => {
        editor.prompt = None;
        // Unified: dispatch_save_then handles NAMED (save+arm pending_after_save) AND UNNAMED
        // (opens Save-As, re-carrying the action in pending_save_as). Take the intent first so
        // the named path doesn't leave a stale pending_save_as (Codex re-review).
        if let Some(action) = editor.pending_save_as.take() {
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
            crate::save::dispatch_save_then(&mut ctx, action);
        }
    }
```
> No overlapping borrow: `dispatch_save_then` captures path/version internally and owns arming. For the unnamed case it re-sets `pending_save_as` (so the eventual Save-As fires the action); for the named case `pending_save_as` stays cleared (taken here). The carrier is also cleared on the dirty-guard **Cancel** arm.
> `open_into_current` already exists (Task 1), so the `Open` arm compiles; it is *triggered* in Task 5 (this task triggers only `New`). Register `new` (File) → `|c| { crate::app::request_new(c.editor, c.executor, c.clock, &c.msg_tx); CommandResult::Handled }`; cua bind `("ctrl-n", "new")`.

- [ ] **Step 4: Run** `cargo test -p wordcartel new_on_` + an integration test that resolves Discard→scratch and Cancel→untouched + `cargo test -p wordcartel --lib` — green.

- [ ] **Step 5: Commit** `feat(7): New command + dirty-guard (Save/Discard/Cancel) mechanism`

---

## Task 5: `file_browser` overlay + `open` command

**Files:**
- Create: `wordcartel/src/file_browser.rs`; Modify: `wordcartel/src/lib.rs` (`pub mod file_browser`)
- Modify: `wordcartel/src/editor.rs` (`file_browser: Option<FileBrowser>` field + `open_file_browser` + `file_browser = None` in every other `open_*`)
- Modify: `wordcartel/src/app.rs` (`file_browser` **key block** in `reduce`; Enter-on-file → `request_replace(Open(path))`)
- Modify: `wordcartel/src/render.rs` (**file_browser render** alongside palette/outline/theme_picker at render.rs:686/746/792) — **Codex: rendering lives in render.rs, not app.rs.** **Decision:** do **NOT** add `file_browser` to `has_overlay` (render.rs:618) — `theme_picker` is likewise excluded and renders fine over the content; the status line under the overlay is harmless. (Match `theme_picker` exactly.)
- Modify: `wordcartel/src/registry.rs` (`open` + `file_browser=None` in menu/`dispatch_overlay_command`), `keymap.rs` (cua `ctrl-o`), `mouse.rs` (absorb)
- Test: `wordcartel/src/file_browser.rs` + `wordcartel/src/app.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `FileBrowser { dir, query, entries, selected }`, `FileEntry { name, is_dir }`, `file_browser::rebuild_entries(&mut FileBrowser)`; `Editor.file_browser`, `Editor::open_file_browser`; registry id `open`.
- Consumes: `std::fs::read_dir`, `app::open_into_current` (Task 1), `request_replace` (Task 4), the `theme_picker` overlay key block/render to mirror.

- [ ] **Step 1: Write the failing tests** (`file_browser.rs`)

```rust
    #[test]
    fn rebuild_entries_dirs_first_with_dotdot_and_filter() {
        let dir = std::env::temp_dir().join(format!("wc-fb-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("alpha.md"), "x").unwrap();
        std::fs::write(dir.join("beta.txt"), "x").unwrap();
        let mut fb = FileBrowser { dir: dir.clone(), query: String::new(), entries: vec![], selected: 0 };
        rebuild_entries(&mut fb);
        assert_eq!(fb.entries[0].name, "..", "parent first");
        let names: Vec<_> = fb.entries.iter().map(|e| e.name.as_str()).collect();
        let sub_i = names.iter().position(|n| *n == "sub").unwrap();
        let alpha_i = names.iter().position(|n| *n == "alpha.md").unwrap();
        assert!(sub_i < alpha_i, "directories sort before files");
        fb.query = "alpha".into(); rebuild_entries(&mut fb);
        assert!(fb.entries.iter().any(|e| e.name == "alpha.md"));
        assert!(!fb.entries.iter().any(|e| e.name == "beta.txt"), "substring filter");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_file_browser_enforces_xor() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 12));
        e.open_palette();
        e.open_file_browser(std::env::temp_dir());
        assert!(e.file_browser.is_some());
        assert!(e.palette.is_none(), "opening file_browser clears the palette (XOR)");
    }
```

An `app.rs` integration test that Enter-on-a-file opens it through the dirty-guard (clean buffer → immediate replace via `open_into_current`):
```rust
    #[test]
    fn file_browser_enter_on_file_opens_it_when_clean() {
        use crate::editor::Editor;
        let dir = std::env::temp_dir().join(format!("wc-fbopen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.md"), "loaded\n").unwrap();
        let mut e = Editor::new_from_text("clean\n", None, (80, 24)); // clean
        e.open_file_browser(dir.clone());
        // select "note.md" and simulate Enter via the browser's open path:
        crate::app::open_into_current(&mut e, &dir.join("note.md")); // the clean-path the Enter handler takes
        assert_eq!(e.active().document.buffer.to_string(), "loaded\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run — fails** (`FileBrowser`/`open_file_browser` undefined). `cargo test -p wordcartel file_browser`

- [ ] **Step 3: Implement `file_browser.rs`** (mirror `theme_picker.rs`):
```rust
use std::path::PathBuf;
#[derive(Debug, Clone)] pub struct FileEntry { pub name: String, pub is_dir: bool }
#[derive(Debug, Clone)] pub struct FileBrowser { pub dir: PathBuf, pub query: String, pub entries: Vec<FileEntry>, pub selected: usize }

/// Rebuild `entries` from `dir`: synthetic ".." first (unless at root), then directories,
/// then files, each alphabetical; substring-filtered (case-insensitive) by `query`.
pub fn rebuild_entries(fb: &mut FileBrowser) {
    let q = fb.query.to_ascii_lowercase();
    let mut dirs = Vec::new(); let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&fb.dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if !q.is_empty() && !name.to_ascii_lowercase().contains(&q) { continue; }
            if is_dir { dirs.push(name) } else { files.push(name) }
        }
    }
    dirs.sort(); files.sort();
    fb.entries = Vec::new();
    if fb.dir.parent().is_some() { fb.entries.push(FileEntry { name: "..".into(), is_dir: true }); }
    fb.entries.extend(dirs.into_iter().map(|name| FileEntry { name, is_dir: true }));
    fb.entries.extend(files.into_iter().map(|name| FileEntry { name, is_dir: false }));
    if fb.selected >= fb.entries.len() { fb.selected = fb.entries.len().saturating_sub(1); }
}
```
`lib.rs`: `pub mod file_browser;`. `editor.rs`: add `pub file_browser: Option<crate::file_browser::FileBrowser>,` (init None) + `open_file_browser(dir)` (mirror `open_theme_picker`: clear all other overlays incl `file_browser`, then set it with `rebuild_entries`), and add `self.file_browser = None;` to EVERY other `open_*` helper (open_minibuffer/open_prompt/open_palette/open_menu/open_search/open_diag/open_outline/open_theme_picker).
Add the `file_browser` **key block** in `app.rs` (mirror the `theme_picker` block ~870): printable→`query`+`rebuild_entries`; Backspace→pop+rebuild; Up/Down→`selected`; Enter→ if `entries[selected].is_dir` set `dir` (join `..`→parent / name→child), clear query, rebuild, selected=0 — else `let path = fb.dir.join(name); editor.file_browser=None; request_replace(editor, crate::editor::PostSaveAction::Open(path), ex, clock, msg_tx);` (**vars are `ex`/`clock`/`msg_tx` in `reduce`, not `executor`** — Codex; the dirty-guard from Task 4 handles clean→immediate `open_into_current` vs dirty→modal); Esc→`file_browser=None`; drain async `ClipboardPaste` (mirror the other overlays). Add the **render in `render.rs`** (mirror the `theme_picker`/outline overlay render at render.rs:686/746/792 — NOT app.rs; Codex). Do **not** add `file_browser` to `has_overlay` (match `theme_picker`, which is excluded). Register `open` (File) → `|c| { let dir = c.editor.active().document.path.as_ref().and_then(|p| p.parent()).map(|d| d.to_path_buf()).unwrap_or_else(|| std::env::current_dir().unwrap_or_default()); c.editor.open_file_browser(dir); CommandResult::Handled }`; cua bind `("ctrl-o", "open")`; clear `file_browser` in `registry.rs` menu/`dispatch_overlay_command`; absorb in `mouse.rs`.

- [ ] **Step 4: Run** `cargo test -p wordcartel file_browser` + `cargo test -p wordcartel open_into_current` + `cargo test -p wordcartel --lib` + `cargo test` (workspace) — all green.

- [ ] **Step 5: Commit** `feat(7): file_browser overlay + open command (open_into_current, dirty-guarded)`

---

## Final Verification
- [ ] `cargo test` (workspace) — all green.
- [ ] `cargo clippy -p wordcartel --lib` — no new warnings in touched files.
- [ ] Manual smoke: `Ctrl+O` opens the browser (navigate dirs with Enter/`..`, type to filter, Enter opens a file); opening with a dirty buffer prompts Save/Discard/Cancel; `Ctrl+N` new scratch (dirty-guarded); `Ctrl+Shift+S` Save-As (prompt → writes → clean → status; existing target → Overwrite confirm); `save` on the new scratch routes to Save-As; save-and-quit on a named buffer still exits.

## Self-Review Notes (coverage vs spec)
- §2 seam → Task 1 (from_text/from_file take id; run refactor; resume_enabled). §4 dirty-guard + post-save → Tasks 2 (generalization) + 4 (guard/New) + 5 (Open). §5 Save-As → Task 3 (do_save_to, MinibufferKind::SaveAs, OverwriteSaveAs, staged re-key, failure invariant). §3 browser + §6 New → Tasks 5 + 4.
- Codex folds present: from_file(id,..) + fresh-id-on-replace (T1/T5); apply_result self-contained Open (T5 open_into_current reloads session); do_save_to + SaveAs merge not reusing Overwrite (T3); pending_save_as carrier through minibuffer+overwrite (T3/T4); save-and-quit named-arms/unnamed-no-arm (T2/T3); staged swap re-key (T3); in-app swap recovery deferred (not built); full overlay XOR/mouse/lib integration (T5).
- Out of scope (not planned, per spec): recent files, tui-popup, nucleo/D recursive finder, save-mode browser, in-app swap recovery, multi-buffer (Effort 6 — seams built).
