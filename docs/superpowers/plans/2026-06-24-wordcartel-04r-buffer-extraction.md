# Wordcartel Effort 4r — Buffer Extraction (prep refactor) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reshape the flat single-document `Editor` into a thin workspace over `Vec<Buffer>` (holding exactly one buffer), with a stable `BufferId`, and thread `buffer_id` through the async job model — **without any behavior change**, so Efforts 4c/5 and the future Effort 6 build on `Buffer` instead of growing a later refactor.

**Architecture:** `Editor` keeps global app state (`buffers`, `active`, `next_buffer_id`, `register`, `status`, `prompt`, `quit`, and the save&quit await fields); each `Buffer` bundles one document's `Document` + `View` + per-document transient state. Document-mutation methods (`apply`/`undo`/`redo`) move to `impl Buffer`; `Editor` keeps thin delegating wrappers so external callers are unchanged. Jobs carry a `buffer_id` + a result-class marker; `apply_result` routes by id. With one buffer everything is **inert in effect but mechanical in code** — no path may resolve `active()` implicitly where a `buffer_id` is available.

**Tech Stack:** Rust 2021, `ropey` 1.6.1 (LF/CRLF-only), `ratatui` 0.29, `crossterm` 0.28, `std::thread`/`mpsc`, `proptest` (dev). No new dependencies.

## Global Constraints

(From `docs/superpowers/specs/2026-06-24-multi-buffer-workspace-design.md`; bind every task.)

- **Behavior-preserving:** this is a pure refactor. **The existing 136-shell + 105-core + 34-oracle suite is the gate** — every test stays green, **no test may be weakened or its assertion changed** beyond the mechanical `editor.document` → `editor.active().document` field-path rewrite. The running binary behaves identically.
- **`BufferId` is stable, monotonic, NEVER reused** for the process lifetime (`Editor.next_buffer_id` + `alloc_id()`); it is **not** the `Vec` index.
- **Mechanical routing (spec §3.4 rule 1):** every job creation, staleness check, merge, and status dispatch uses `result.buffer_id` / `by_id_mut`, **never `active()` at execution time**. A debug assertion confirms a result's `buffer_id` resolves to the intended buffer.
- **Two result classes (spec §3.4 rule 2):** *buffer-local merges* (status/saved_version/stored_fp/cadence bookkeeping) are dropped if the buffer was closed; *durability completions* (save/swap-delete/swap-write side effects) are not. With one buffer the buffer is never closed mid-flight, so the distinction is structural now and exercised by tests.
- **`len >= 1` invariant:** `Editor.buffers` is never empty; `active`/`active_mut` assert it.
- **Workspace facts:** `cargo test` from repo root; build with `cargo build --workspace` (zero warnings). Parallel stability: swap-writing tests use unique temp paths (4b-2 discipline). Binary is `wcartel`.

---

## File Structure

All changes are in the `wordcartel` shell crate; `wordcartel-core` is untouched.

- `wordcartel/src/editor.rs` *(major)* — define `Buffer` + `BufferId`; reshape `Editor`; move `apply`/`undo`/`redo` to `impl Buffer` + thin `Editor` delegators; `active()`/`active_mut()`/`by_id()`/`by_id_mut()`/`alloc_id()`; `new_from_text` builds one buffer.
- `wordcartel/src/{derive,nav,commands,render,save,app,swap}.rs` *(mechanical)* — rewrite `editor.document` / `editor.view` / `editor.<transient>` / `editor.pending_swap_*` field paths through `active()`/`active_mut()`.
- `wordcartel/src/jobs.rs` *(modify)* — `buffer_id` + `ResultClass` on `Job`/`JobResult`; `is_stale` keyed on `(buffer_id, version)`.
- `wordcartel/src/app.rs` *(modify)* — `apply_result` routes by `by_id_mut` + result class + debug assertion.
- `wordcartel/src/save.rs`, `wordcartel/src/swap.rs` *(modify)* — `dispatch_save`/`dispatch_swap_write` stamp `buffer_id = active id`; merges target the resolved buffer.

**Per-document transient fields that relocate from `Editor` onto `Buffer`:** `desired_col`, `pre_edit_rope`, `last_edit`, `last_edit_at`, `last_swap_at`, `swap_in_flight`, `pending_swap_body`, `pending_swap_path`.

**Fields that STAY global on `Editor`:** `register`, `status`, `prompt`, `quit`, `quit_after_save`, `quit_after_save_at` (the save&quit await drives the global `quit` flag; Effort 6 generalizes it to a set), plus the new `buffers`, `active`, `next_buffer_id`.

> **Scope note:** the prep relocates `pending_swap_body`/`pending_swap_path` onto `Buffer` **as-is** (two fields). The spec's `PendingRecovery` struct + per-buffer recovery *queue* are an Effort-6 concern (they only matter with >1 buffer); folding the two fields into `PendingRecovery` is deferred to Effort 6 to keep this prep a pure mechanical relocation.

---

## Task 1: Reshape `Editor` over `Vec<Buffer>` + migrate all call sites

**This task is atomic by necessity:** changing `Editor`'s shape breaks every `editor.document`/`editor.view`/`editor.<transient>` call site at once, so the reshape and the full migration land in one commit. The change is **mechanical and compiler-gated**; the 136-test suite is the behavior gate.

**Files:**
- Modify: `wordcartel/src/editor.rs` (struct defs, methods, constructor)
- Modify: `wordcartel/src/{derive,nav,commands,render,save,app,swap}.rs` (field-path rewrites)
- Test: `wordcartel/src/editor.rs` (new accessor/alloc_id unit tests)

**Interfaces:**
- Produces:
  - `pub struct BufferId(pub u64)` deriving `Clone, Copy, PartialEq, Eq, Hash, Debug, Ord, PartialOrd`.
  - `pub struct Buffer { pub id: BufferId, pub document: Document, pub view: View, pub desired_col: Option<usize>, pub pre_edit_rope: Option<ropey::Rope>, pub last_edit: Option<block_tree::Edit>, pub last_edit_at: Option<u64>, pub last_swap_at: Option<u64>, pub swap_in_flight: bool, pub pending_swap_body: Option<String>, pub pending_swap_path: Option<PathBuf> }` (derives `Clone, Debug`).
  - `impl Buffer { pub fn apply(&mut self, txn, edit, kind, clock); pub fn undo(&mut self) -> bool; pub fn redo(&mut self) -> bool }` — moved verbatim from `Editor` (bodies reference `self.document`/`self.pre_edit_rope`/`self.last_edit` which are now `Buffer` fields; the `recovery::record_snapshot` call stays).
  - `pub struct Editor { pub buffers: Vec<Buffer>, pub active: usize, pub next_buffer_id: u64, pub register, pub status, pub quit: bool, pub prompt, pub quit_after_save: Option<u64>, pub quit_after_save_at: Option<u64> }` (derives `Clone, Debug`).
  - `impl Editor { pub fn active(&self) -> &Buffer; pub fn active_mut(&mut self) -> &mut Buffer; pub fn by_id(&self, id: BufferId) -> Option<&Buffer>; pub fn by_id_mut(&mut self, id: BufferId) -> Option<&mut Buffer>; pub fn alloc_id(&mut self) -> BufferId; pub fn apply(&mut self, txn, edit, kind, clock); pub fn undo(&mut self) -> bool; pub fn redo(&mut self) -> bool }` — `apply`/`undo`/`redo` delegate to `self.active_mut()`.
  - `Editor::new_from_text(text, path, area)` unchanged signature; builds one `Buffer` (id from `alloc_id`).

- [ ] **Step 1: Reshape the structs + methods in `editor.rs`.** Replace the `Editor` struct (current lines ~56-88), its `new_from_text` (~90-129), and the `apply`/`undo`/`redo` methods with:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Ord, PartialOrd)]
pub struct BufferId(pub u64);

#[derive(Debug, Clone)]
pub struct Buffer {
    pub id: BufferId,
    pub document: Document,
    pub view: View,
    // per-document transient state (relocated off Editor)
    pub desired_col: Option<usize>,
    pub pre_edit_rope: Option<ropey::Rope>,
    pub last_edit: Option<wordcartel_core::block_tree::Edit>,
    pub last_edit_at: Option<u64>,
    pub last_swap_at: Option<u64>,
    pub swap_in_flight: bool,
    pub pending_swap_body: Option<String>,
    pub pending_swap_path: Option<PathBuf>,
}

impl Buffer {
    /// Single mutation channel for THIS buffer's document (spec §10.1).
    pub fn apply(&mut self, txn: Transaction, edit: wordcartel_core::block_tree::Edit, kind: EditKind, clock: &dyn Clock) {
        let old_rope = self.document.buffer.snapshot();
        let before = self.document.selection.clone();
        self.document.selection = self.document.history.commit_coalescing(txn, &mut self.document.buffer, before, clock, kind);
        self.document.version += 1;
        self.pre_edit_rope = Some(old_rope);
        self.last_edit = Some(edit);
        crate::recovery::record_snapshot(self.document.path.as_deref(), self.document.buffer.snapshot());
    }
    pub fn undo(&mut self) -> bool {
        match self.document.history.undo(&mut self.document.buffer) {
            Some(sel) => { self.document.selection = sel; self.document.version += 1; self.last_edit = None; self.pre_edit_rope = None; true }
            None => false,
        }
    }
    pub fn redo(&mut self) -> bool {
        match self.document.history.redo(&mut self.document.buffer) {
            Some(sel) => { self.document.selection = sel; self.document.version += 1; self.last_edit = None; self.pre_edit_rope = None; true }
            None => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub active: usize,
    pub next_buffer_id: u64,
    // global app state
    pub register: Register,
    pub status: String,
    pub quit: bool,
    pub prompt: Option<crate::prompt::Prompt>,
    pub quit_after_save: Option<u64>,
    pub quit_after_save_at: Option<u64>,
}

impl Editor {
    pub fn new_from_text(text: &str, path: Option<PathBuf>, area: (u16, u16)) -> Editor {
        let buffer = TextBuffer::from_str(text);
        let selection = Selection::single(0);
        let history = History::default();
        let blocks = block_tree::full_parse_rope(&buffer.snapshot());
        let document = Document {
            buffer, selection, history, blocks, version: 0,
            stored_fp: path.as_deref().and_then(crate::save::fingerprint),
            path, saved_version: Some(0),
        };
        let view = View { scroll: 0, scroll_row: 0, area, mode: RenderMode::LivePreview, line_layouts: BTreeMap::new() };
        // Build the workspace, then allocate the buffer's id through the single
        // id source (alloc_id) so there is no second id-assignment path (Codex review).
        let mut e = Editor {
            buffers: Vec::new(), active: 0, next_buffer_id: 0,
            register: Register::default(), status: String::new(), quit: false,
            prompt: None, quit_after_save: None, quit_after_save_at: None,
        };
        let id = e.alloc_id(); // -> BufferId(0); next_buffer_id becomes 1
        e.buffers.push(Buffer {
            id, document, view,
            desired_col: None, pre_edit_rope: None, last_edit: None,
            last_edit_at: None, last_swap_at: None, swap_in_flight: false,
            pending_swap_body: None, pending_swap_path: None,
        });
        e
    }

    #[inline] pub fn active(&self) -> &Buffer {
        debug_assert!(!self.buffers.is_empty() && self.active < self.buffers.len(), "len>=1 + active in range");
        &self.buffers[self.active]
    }
    #[inline] pub fn active_mut(&mut self) -> &mut Buffer {
        debug_assert!(!self.buffers.is_empty() && self.active < self.buffers.len(), "len>=1 + active in range");
        let i = self.active; &mut self.buffers[i]
    }
    pub fn by_id(&self, id: BufferId) -> Option<&Buffer> { self.buffers.iter().find(|b| b.id == id) }
    pub fn by_id_mut(&mut self, id: BufferId) -> Option<&mut Buffer> { self.buffers.iter_mut().find(|b| b.id == id) }
    /// Allocate a fresh, never-reused BufferId.
    pub fn alloc_id(&mut self) -> BufferId { let id = BufferId(self.next_buffer_id); self.next_buffer_id += 1; id }

    // Thin delegators — external callers unchanged.
    pub fn apply(&mut self, txn: Transaction, edit: wordcartel_core::block_tree::Edit, kind: EditKind, clock: &dyn Clock) {
        self.active_mut().apply(txn, edit, kind, clock);
    }
    pub fn undo(&mut self) -> bool { self.active_mut().undo() }
    pub fn redo(&mut self) -> bool { self.active_mut().redo() }
}
```

(`new_from_text` routes the initial buffer's id through `alloc_id()` — the single id source — so the first buffer is deterministically `BufferId(0)` and `next_buffer_id` ends at 1; Effort 6 calls `alloc_id` for buffers 2..N. The transient empty `buffers: Vec::new()` exists only during construction; the returned `Editor` always satisfies `len >= 1`. The existing `Document`/`View` structs and `Document::{dirty,mark_saved}` are unchanged.)

- [ ] **Step 2: Run the build to enumerate every broken call site.**

Run: `cargo build -p wordcartel 2>&1 | grep -E "^error" | head -60`
Expected: a long list of `no field 'document' on type '&Editor'` / `'view'` / `'desired_col'` / etc. — one per call site to migrate. This list is the migration worklist.

- [ ] **Step 3: Migrate call sites mechanically, file by file**, applying this exact transformation rule everywhere the compiler flags an `Editor` field that moved to `Buffer`:

| Was | Becomes (read context) | Becomes (write / `&mut` context) |
|---|---|---|
| `editor.document` | `editor.active().document` | `editor.active_mut().document` |
| `editor.view` | `editor.active().view` | `editor.active_mut().view` |
| `editor.desired_col` | `editor.active().desired_col` | `editor.active_mut().desired_col` |
| `editor.pre_edit_rope` / `last_edit` / `last_edit_at` / `last_swap_at` / `swap_in_flight` | `editor.active().<f>` | `editor.active_mut().<f>` |
| `editor.pending_swap_body` / `pending_swap_path` | `editor.active().<f>` | `editor.active_mut().<f>` |

The same applies to other binding names for an `Editor` (`e.document`, `ed.document`, `self.document` inside `Editor` methods, `ctx.editor.document`, etc.). **The compiler picks for you:** use `active()` first; if it errors with "cannot borrow as mutable" or "cannot assign", switch that site to `active_mut()`. Mutable-borrow rule: a single statement that both reads and writes through the active buffer takes `active_mut()` once and reuses the `&mut` (e.g. `let b = editor.active_mut(); b.view.scroll = …; b.document.…`). Order the files by the Step-2 list; suggested grouping (commit once at the end, not per group): `derive.rs` → `nav.rs` → `commands.rs` → `render.rs` → `save.rs` → `swap.rs` → `app.rs`.

Concrete examples (verify against the real lines):

```rust
// derive.rs — rebuild reads buffer + writes view.line_layouts:
//   let buf = &editor.document.buffer;            ->  let buf = &editor.active().document.buffer;
//   editor.view.line_layouts.clear();             ->  editor.active_mut().view.line_layouts.clear();

// nav.rs — move_* read view + document, write desired_col:
//   editor.desired_col = Some(col);               ->  editor.active_mut().desired_col = Some(col);
//   let head = editor.document.selection.primary().head;  ->  ...editor.active().document...

// commands.rs — run() mutates document via apply (delegator unchanged) but touches view/desired_col:
//   editor.desired_col = None;                    ->  editor.active_mut().desired_col = None;
//   editor.document.selection = Selection::single(x);  ->  editor.active_mut().document.selection = ...;

// app.rs — recovery-on-open + reduce set the active buffer's transients/recovery:
//   editor.pending_swap_body = Some(body);        ->  editor.active_mut().pending_swap_body = Some(body);
//   editor.last_edit_at = Some(now);              ->  editor.active_mut().last_edit_at = Some(now);

// save.rs / swap.rs — dispatch_* read the active buffer; their MERGES are rewritten in Task 2.
//   ctx.editor.document.buffer.snapshot()         ->  ctx.editor.active().document.buffer.snapshot()
//   ctx.editor.document.stored_fp                 ->  ctx.editor.active().document.stored_fp
```

**Save/swap merges** currently do `editor.last_swap_at = …` / `editor.document.saved_version = …` inside `Box<dyn FnOnce(&mut Editor)>`. For Task 1, rewrite them to `editor.active_mut().last_swap_at = …` etc. (single-buffer correct). **Task 2 replaces this with `by_id_mut(buffer_id)` routing** — leave a `// Task 2: route by buffer_id` comment at each merge site.

**Two call sites the generic field-path recipe does NOT cover — handle explicitly (Codex review):**

- **`save::reload_from_disk` / `save::load_recovered`** (`save.rs:~109-139`) currently build `let fresh = Editor::new_from_text(...)` then assign `editor.document = fresh.document; editor.view.line_layouts.clear(); …`. After the reshape, `fresh` is a whole workspace and `fresh.document` no longer exists. Migrate by **replacing the active buffer's contents from `fresh`'s single buffer while preserving the active buffer's `id`** (this is the spec's sanctioned whole-document replacement; keeping `id` stable keeps in-flight job routing valid):
  ```rust
  let fresh = Editor::new_from_text(&text, Some(path.clone()), area);
  let new_buf = fresh.buffers.into_iter().next().expect("new_from_text yields one buffer");
  let id = editor.active().id;                 // preserve THIS buffer's id
  *editor.active_mut() = Buffer { id, ..new_buf };
  // then the existing follow-ups, now on the active buffer:
  editor.active_mut().view.line_layouts.clear();
  derive::rebuild(editor);
  nav::ensure_visible(editor);
  editor.active_mut().document.stored_fp = fingerprint(&path); // (load_recovered: saved_version = None)
  ```
  (`Buffer { id, ..new_buf }` keeps the stable id and takes fresh document/view/reset-transients from `new_buf`. `load_recovered` then sets `saved_version = None` on the active buffer as today.)
- **`swap::build_header(editor: &Editor, …)`** (`swap.rs:~220-239`) reads `editor.document.path` / `editor.document.version` / `editor.document.stored_fp`. The generic recipe applies (`editor.document` → `editor.active().document`); just confirm every read inside it is rewritten. (No signature change needed for the prep.)

- [ ] **Step 4: Migrate the existing tests' field paths** in the same mechanical way. Tests across `editor.rs`, `commands.rs`, `app.rs`, `save.rs`, `swap.rs`, `render.rs`, `nav.rs` that read `e.document.*` / `e.view.*` / `e.desired_col` / `e.pending_swap_*` / `e.last_*` / `e.swap_in_flight` become `e.active().…` / `e.active_mut().…`. **This is the only permitted test change — the assertion values are identical.** Do NOT alter any asserted value, only the field path. (Tests that set `e.document.selection = …` use `active_mut()`; tests that read `e.document.buffer.to_string()` use `active()`.)

- [ ] **Step 5: Add accessor + alloc_id unit tests** to `editor.rs`'s test module:

```rust
#[test]
fn single_buffer_invariants_and_accessors() {
    let mut e = Editor::new_from_text("hi\n", None, (80, 24));
    assert_eq!(e.buffers.len(), 1);
    assert_eq!(e.active, 0);
    assert_eq!(e.active().id, BufferId(0));
    assert_eq!(e.active().document.buffer.to_string(), "hi\n");
    // by_id resolves the active buffer; a bogus id is None.
    let id = e.active().id;
    assert!(e.by_id(id).is_some());
    assert!(e.by_id_mut(BufferId(999)).is_none());
}

#[test]
fn alloc_id_is_monotonic_and_never_reuses() {
    let mut e = Editor::new_from_text("\n", None, (80, 24));
    // id 0 is taken by the initial buffer; next allocations are 1, 2, 3...
    let a = e.alloc_id();
    let b = e.alloc_id();
    assert_eq!(a, BufferId(1));
    assert_eq!(b, BufferId(2));
    assert_ne!(a, e.active().id); // never collides with the existing buffer's id
}
```

- [ ] **Step 6: Build + run the FULL suite (the behavior gate).**

Run: `cargo build --workspace 2>&1 | grep -iE "error|warning"; echo "---"; cargo test`
Expected: zero errors, zero warnings; **136 shell + 105 core + 34 oracle + integration all pass**, plus the 2 new editor tests (138 shell). If ANY pre-existing test fails, the migration changed behavior — STOP, find the non-mechanical edit, and fix it; do not adjust the test.

- [ ] **Step 7: Confirm parallel stability + binary builds.**

Run: `for i in 1 2 3; do cargo test -p wordcartel --lib 2>&1 | grep "test result:" | head -1; done; cargo build -p wordcartel`
Expected: 138 passed each run; binary builds.

- [ ] **Step 8: Commit.**

```bash
git add wordcartel/src
git commit -m "refactor(editor): reshape Editor over Vec<Buffer> (vec-of-one); relocate per-doc transients to Buffer"
```

---

## Task 2: Thread `buffer_id` + result class through the job model

Make every job carry the `BufferId` it was dispatched for and a result-class marker; route `apply_result` by id (drop buffer-local merges for a missing buffer; run durability completions regardless). With one buffer this is inert in effect but mechanical in code (spec §3.4).

**Files:**
- Modify: `wordcartel/src/jobs.rs` (`Job`/`JobResult` fields; `ResultClass`; `is_stale`)
- Modify: `wordcartel/src/save.rs`, `wordcartel/src/swap.rs` (`dispatch_*` stamp `buffer_id`; merges target `by_id_mut`)
- Modify: `wordcartel/src/app.rs` (`apply_result` routing + debug assertion)
- Test: `wordcartel/src/jobs.rs` + `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `editor::{BufferId, Editor}`, `editor::Editor::by_id_mut`.
- Produces:
  - `pub enum ResultClass { BufferLocal, Durability }` (derives `Clone, Copy, PartialEq, Eq, Debug`).
  - `Job` and `JobResult` gain `pub buffer_id: BufferId` and `pub class: ResultClass`.
  - `JobResult.merge` stays `Box<dyn FnOnce(&mut Editor) + Send>` (it resolves its own buffer via `by_id_mut`); the merge captures its `buffer_id`.
  - `is_stale(class, kind, buffer_id, result_version, editor) -> bool` — true if `BufferLocal` and the buffer is missing OR (coalescible kind && version moved); `Durability` is never stale.
  - `apply_result(r, editor)` routes per class.

- [ ] **Step 1: Write the failing routing tests** in `wordcartel/src/app.rs` tests:

```rust
#[test]
fn buffer_local_result_for_missing_buffer_is_dropped() {
    use crate::editor::{Editor, BufferId};
    use crate::jobs::{JobResult, JobKind, ResultClass};
    let mut e = Editor::new_from_text("\n", None, (80, 24));
    // A buffer-local merge for a non-existent buffer must NOT run.
    crate::app::apply_result(JobResult {
        buffer_id: BufferId(999), class: ResultClass::BufferLocal,
        version: 1, kind: JobKind::Save,
        merge: Box::new(|ed: &mut Editor| ed.status = "SHOULD NOT RUN".into()),
    }, &mut e);
    assert_ne!(e.status, "SHOULD NOT RUN", "buffer-local merge for a missing buffer is dropped");
}

#[test]
fn buffer_local_result_for_live_buffer_merges() {
    use crate::editor::Editor;
    use crate::jobs::{JobResult, JobKind, ResultClass};
    let mut e = Editor::new_from_text("\n", None, (80, 24));
    let id = e.active().id;
    crate::app::apply_result(JobResult {
        buffer_id: id, class: ResultClass::BufferLocal,
        version: 1, kind: JobKind::Save,
        merge: Box::new(|ed: &mut Editor| ed.status = "merged".into()),
    }, &mut e);
    assert_eq!(e.status, "merged");
}

#[test]
fn durability_result_for_missing_buffer_still_runs() {
    use crate::editor::{Editor, BufferId};
    use crate::jobs::{JobResult, JobKind, ResultClass};
    let mut e = Editor::new_from_text("\n", None, (80, 24));
    // A durability completion runs even though its buffer is gone (e.g. closed).
    crate::app::apply_result(JobResult {
        buffer_id: BufferId(999), class: ResultClass::Durability,
        version: 1, kind: JobKind::SwapWrite,
        merge: Box::new(|ed: &mut Editor| ed.status = "durability ran".into()),
    }, &mut e);
    assert_eq!(e.status, "durability ran");
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p wordcartel --lib app::tests::buffer_local_result_for_missing_buffer_is_dropped app::tests::durability_result_for_missing_buffer_still_runs`
Expected: FAIL to compile — `ResultClass`, the `buffer_id`/`class` fields don't exist.

- [ ] **Step 3: Add `ResultClass` + fields to `jobs.rs`.** Extend `Job` and `JobResult`:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResultClass {
    /// Mutates buffer-local state (status/saved_version/cadence); dropped if the buffer is gone.
    BufferLocal,
    /// External filesystem side effect that must complete even if the buffer was closed.
    Durability,
}

pub struct Job {
    pub buffer_id: crate::editor::BufferId,
    pub class: ResultClass,
    pub version: u64,
    pub kind: JobKind,
    pub run: Box<dyn FnOnce() -> JobResult + Send>,
}

pub struct JobResult {
    pub buffer_id: crate::editor::BufferId,
    pub class: ResultClass,
    pub version: u64,
    pub kind: JobKind,
    pub merge: Box<dyn FnOnce(&mut Editor) + Send>,
}
```

Update `is_stale` to take the buffer context (it currently takes `(kind, result_version, current_version)`); new signature and body:

```rust
/// Staleness now consults the result class + whether the buffer still exists.
pub fn is_stale(r: &JobResult, editor: &Editor) -> bool {
    match r.class {
        ResultClass::Durability => false, // must always complete
        ResultClass::BufferLocal => match editor.by_id(r.buffer_id) {
            None => true, // buffer closed -> drop the buffer-local merge
            Some(b) => match r.kind {
                JobKind::Save | JobKind::SwapWrite => false, // one-shot: never stale
                #[cfg(test)]
                JobKind::CoalesceProbe => r.version != b.document.version,
            },
        },
    }
}
```

(`is_stale`'s callers — only `apply_result` and the jobs unit tests — change to the new signature. Keep the existing `CoalesceProbe` staleness semantics, now per-buffer.)

- [ ] **Step 4: Rewrite `apply_result` in `app.rs`** to route by class + id with the debug assertion:

```rust
pub fn apply_result(r: JobResult, editor: &mut Editor) {
    if crate::jobs::is_stale(&r, editor) {
        return; // buffer-local merge for a closed buffer, or a stale coalescible
    }
    let (kind, version, buffer_id, class) = (r.kind, r.version, r.buffer_id, r.class);
    // Mechanical-routing assertion (spec §3.4): a buffer-local merge must resolve
    // to a live buffer here; durability merges may target a now-missing buffer.
    debug_assert!(
        class == crate::jobs::ResultClass::Durability || editor.by_id(buffer_id).is_some(),
        "buffer-local result for a missing buffer slipped past is_stale"
    );
    (r.merge)(editor);
    // Save & quit: exit once the awaited save version lands clean for that buffer.
    if kind == crate::jobs::JobKind::Save
        && editor.quit_after_save == Some(version)
        && editor.by_id(buffer_id).map(|b| b.document.saved_version) == Some(Some(version))
    {
        editor.quit = true;
    }
}
```

(The merge closures themselves resolve `by_id_mut(buffer_id)` — see Step 5 — so the buffer-local ones are safe; `apply_result` only gates whether to run them. The save&quit check now reads the target buffer's `saved_version` via `by_id`.)

- [ ] **Step 5: Stamp `buffer_id`/`class` at dispatch and route merges by id** in `save.rs` and `swap.rs`. At each `executor.dispatch(Job { … })`, add `buffer_id: ctx.editor.active().id` and the class (`Save`/`SwapWrite` are `Durability` — their merges have the filesystem side effect and bookkeeping; the bookkeeping part is buffer-targeted via `by_id_mut`). The merge closures (the `// Task 2: route by buffer_id` sites from Task 1) become:

```rust
// save.rs dispatch_save's merge — capture buffer_id, route by id:
let buffer_id = ctx.editor.active().id;
// ... in the JobResult:
JobResult {
    buffer_id, class: ResultClass::Durability, version: v, kind: JobKind::Save,
    merge: Box::new(move |editor| {
        // Assemble the (global) status in a local so the `b` mutable borrow ends
        // before we touch editor.status. swap::delete reads b.document.path under
        // the &mut borrow (an immutable field read), which is allowed.
        let mut status = String::new();
        if let Some(b) = editor.by_id_mut(buffer_id) {
            match outcome {
                Ok(_) => {
                    b.document.saved_version = Some(v);
                    b.document.stored_fp = new_fp;
                    if b.document.version == v {
                        status = "Saved".to_string();
                        crate::swap::delete(b.document.path.as_deref());
                    } else {
                        status = format!("Saved v{v} (still editing)");
                    }
                }
                Err(e) => {
                    // Preserve the current DRY form (4b-2 final-review touch-up
                    // collapsed the per-variant match to e.to_string()); do NOT
                    // re-introduce a SaveError::Symlink arm (it would need an import
                    // and regress that cleanup). `e.to_string()` for Symlink still
                    // contains "symlink", satisfying the existing failure test.
                    status = e.to_string();
                }
            }
        }
        editor.status = status;
    }),
}
```

```rust
// swap.rs dispatch_swap_write's merge:
JobResult {
    buffer_id, class: ResultClass::Durability, version, kind: JobKind::SwapWrite,
    merge: Box::new(move |editor| {
        if let Some(b) = editor.by_id_mut(buffer_id) {
            b.swap_in_flight = false;
            if ok { b.last_swap_at = Some(ts); }
        }
        if !ok { editor.status = "swap write failed".to_string(); } // status global
    }),
}
```

Note the borrow split: `b = editor.by_id_mut(buffer_id)` borrows `editor` mutably; set buffer-local fields through `b`, then set the global `editor.status` **after** `b` is dropped (separate statements, as shown) to satisfy the borrow checker. **With one buffer `by_id_mut(active id)` always resolves** — behavior is identical; the routing is now mechanical.

- [ ] **Step 6: Update the jobs unit tests** for the new `is_stale` signature + the new `Job`/`JobResult` fields (mechanical: add `buffer_id` + `class` to each constructed `Job`/`JobResult`; call `is_stale(&r, &editor)`). Keep the `CoalesceProbe` staleness assertions (now via a live buffer's version). Any existing `save::tests`/`swap::tests`/`app::tests` that construct a `JobResult` literal add the two fields.

- [ ] **Step 7: Run the failing routing tests + the FULL suite.**

Run: `cargo test`
Expected: the 3 new routing tests pass; **all prior tests green** (138 shell + 105 core + 34 oracle + integration). `cargo build --workspace` zero warnings.

- [ ] **Step 8: Parallel stability + binary smoke.**

Run: `for i in 1 2 3; do cargo test -p wordcartel --lib 2>&1 | grep "test result:" | head -1; done && cargo run -p wordcartel -- /tmp/wcartel-4r-smoke.md`
Expected: stable pass counts; the binary opens, edits render, Ctrl+S saves, swap cadence/recovery/panic dump behave exactly as before (one buffer).

- [ ] **Step 9: Commit.**

```bash
git add wordcartel/src
git commit -m "feat(jobs): thread buffer_id + result class through the job model; route apply_result by id"
```

---

## Self-Review (4r)

**Spec coverage (§6.1):**
- Task 1 → Buffer/BufferId/new Editor shape (vec-of-one), accessors, `alloc_id`/`next_buffer_id`, transient relocation, `new_from_text`, full call-site migration, `apply`/`undo`/`redo` → `Buffer`. ✅
- Task 2 → `buffer_id` + result class through the job model, `apply_result` routes via `by_id_mut` (drop buffer-local if closed; durability completes), `is_stale` keyed on `(buffer_id, version)`, debug assertion. ✅
- Behavior-preserving: existing suite is the gate (Step 6/Task 1; Step 7/Task 2). ✅
- Deferred (documented): `PendingRecovery` struct + per-buffer recovery queue → Effort 6; save&quit await stays global (generalized to a set in Effort 6).

**Placeholder scan:** the migration uses a transformation *recipe* + concrete examples rather than reproducing all ~330 lines verbatim — appropriate for a compiler-gated mechanical refactor (the Step-2 error list is the exhaustive worklist; the compiler verifies completeness). No "TBD"/"handle edge cases"; the structs, accessors, `is_stale`, `apply_result`, and both merges are shown in full.

**Type consistency:** `BufferId`, `Buffer`, `Editor::{active,active_mut,by_id,by_id_mut,alloc_id}`, `ResultClass::{BufferLocal,Durability}`, `Job`/`JobResult` fields `buffer_id`/`class`, `is_stale(&r, editor)`, `apply_result(r, editor)` — names used identically across both tasks. `apply`/`undo`/`redo` exist on both `Buffer` (real) and `Editor` (delegating), same signatures.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-24-wordcartel-04r-buffer-extraction.md`. (The Effort 6 feature plan is written separately when you're closer to it — post-1.0.)
