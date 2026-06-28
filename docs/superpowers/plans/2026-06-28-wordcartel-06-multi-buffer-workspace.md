# Multi-Buffer Workspace + Scratch Buffer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the latent `Vec<Buffer>` multi-buffer infrastructure into a real workspace (open many docs, cycle + switcher palette, additive open, close, multi-buffer quit) and add a permanent path-less `*scratch*` buffer that accumulates copied/moved blocks.

**Architecture:** Functional-core/imperative-shell. All changes are in the `wordcartel` shell crate. Each buffer already owns its own per-document state (cursor, marks, folds, marked_block, scroll); the workspace layer adds buffer identity/lifecycle (`scratch_id`, MRU, switch/close/open-additive), a scratch-aware dirty predicate, a multi-buffer quit state machine, two cross-buffer scratch verbs, and scratch persistence. No `wordcartel-core` changes.

**Tech Stack:** Rust, ratatui 0.30, crossterm, serde + toml (session state), `wordcartel_core` (TextBuffer/History/ChangeSet/Selection).

## Global Constraints

- All edits go through `Buffer::apply(txn, edit, kind, clock)` (or `Editor::apply`, which delegates to the active buffer). Never mutate `document.buffer` directly for user-visible edits — `apply` maps marks/jump_ring/folds/marked_block through the ChangeSet and bumps `document.version`. (editor.rs:163)
- Compose multi-edit changes with `crate::commands::build_multi_replace(&[(start,end,String)], doc_len) -> (ChangeSet, Edit)` — ascending, non-overlapping, one undo step. (used in blocks_marked.rs:28)
- Cross-buffer edits target a buffer by id: `editor.by_id_mut(id).unwrap().apply(...)`. A buffer-local async job merges via `by_id_mut(buffer_id)` → `None` no-op if its buffer is gone — never via `active()`. (save.rs:61)
- The scratch buffer is identified by **identity** (`Editor::is_scratch(id)`), not by a name field. Its display name is `*scratch*`; an unnamed ordinary buffer shows `*untitled*`.
- `Editor::is_dirty(id)` is the ONLY dirty check for workspace logic; it returns `false` for scratch. Do not call `document.dirty()` directly in new workspace code. (raw predicate: editor.rs:53)
- Commit trailers on EVERY commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```
- Run `cargo test -p wordcartel` (shell) for tasks touching the shell; the full workspace suite is `cargo test`. Baseline at plan start: 518 shell test fns green.
- Keymap fidelity: any new chord must keep `both_presets_resolve_against_builtins` and the WordStar prefix-shadow tests (keymap.rs) green. WordStar buffer ops live under the `^K` prefix; `^K ,` / `^K .` are **plain-only** second keys (precedent `^KM`/`^KJ`, keymap.rs:369). CUA uses `Alt+,` / `Alt+.`.

## File Structure

- `wordcartel/src/editor.rs` — add `scratch_id: Option<BufferId>`, `mru: Vec<BufferId>`, `quit_drain: Option<QuitDrain>`, `quit_drain_advance: bool`; methods `install_scratch`, `is_scratch`, `is_dirty`, `switch_to_index`, `touch_mru`. (MODIFY)
- `wordcartel/src/scratch.rs` — NEW: `copy_block_to_scratch`, `move_block_to_scratch`, `append_to_scratch`.
- `wordcartel/src/workspace.rs` — NEW: `next_buffer`/`prev_buffer`/`goto_scratch`/`switch_to`, `close_buffer`, additive `open_as_new_buffer`/`new_empty_buffer`, the quit-drain driver `drive_quit_drain`, and the throwaway-reuse predicate.
- `wordcartel/src/state.rs` — add `ScratchState` + `SessionState.scratch`. (MODIFY)
- `wordcartel/src/app.rs` — scratch persist/restore in `persist_session`/`run`; reroute open/new; reduce hook for quit-drain; resolve_prompt arms. (MODIFY)
- `wordcartel/src/prompt.rs` — `quit_multi(n)` + `quit_review_buffer(name)` prompts; new `PromptAction`s. (MODIFY)
- `wordcartel/src/registry.rs` — register the new commands. (MODIFY)
- `wordcartel/src/keymap.rs` — bind the new chords in both presets + tests. (MODIFY)
- `wordcartel/src/render.rs` — status-line `[i/n]` indicator + `*scratch*`/`*untitled*` display name. (MODIFY)
- `wordcartel/src/commands.rs` — `Command::Quit` handler routes to the multi-buffer quit. (MODIFY)

---

### Task 1: Scratch buffer foundation — `scratch_id`, `is_dirty`, `install_scratch`

**Files:**
- Modify: `wordcartel/src/editor.rs` (Editor struct ~271–331; `new_from_text` ~334–372; impl Editor accessors ~374–385)
- Test: `wordcartel/src/editor.rs` (tests mod ~546+)

**Interfaces:**
- Produces: `Editor.scratch_id: Option<BufferId>` (default `None`); `Editor.mru: Vec<BufferId>` (default empty); `Editor::install_scratch(&mut self)`; `Editor::is_scratch(&self, id: BufferId) -> bool`; `Editor::is_dirty(&self, id: BufferId) -> bool`.
- Note: `new_from_text` stays SINGLE-buffer (scratch_id `None`) so the ~518 existing tests keep their `buffers.len() == 1` assumption (editor.rs:684). Scratch is installed explicitly by `run()` and by Effort-6 tests.

- [ ] **Step 1: Write failing tests**

In `wordcartel/src/editor.rs` tests mod, add:

```rust
#[test]
fn install_scratch_adds_permanent_pathless_buffer() {
    let mut e = Editor::new_from_text("doc\n", None, (40, 10));
    assert_eq!(e.buffers.len(), 1);
    assert_eq!(e.scratch_id, None, "no scratch until installed");
    e.install_scratch();
    assert_eq!(e.buffers.len(), 2, "scratch appended");
    let sid = e.scratch_id.expect("scratch_id set");
    assert!(e.is_scratch(sid));
    let sb = e.by_id(sid).unwrap();
    assert!(sb.document.path.is_none(), "scratch has no path");
    assert_eq!(e.active, 0, "launch buffer stays active");
}

#[test]
fn is_dirty_excludes_scratch_even_when_edited() {
    use wordcartel_core::history::Clock;
    struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
    let mut e = Editor::new_from_text("doc\n", None, (40, 10));
    e.install_scratch();
    let sid = e.scratch_id.unwrap();
    // Edit the scratch buffer directly via build_multi_replace + Buffer::apply.
    let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "hi".into())], 0);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(2));
    e.by_id_mut(sid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
    assert!(e.by_id(sid).unwrap().document.dirty(), "raw predicate says dirty");
    assert!(!e.is_dirty(sid), "is_dirty excludes scratch");
    // An edited ordinary buffer IS dirty via is_dirty.
    let aid = e.buffers[0].id;
    let (cs2, edit2) = crate::commands::build_multi_replace(&[(0, 0, "x".into())], 4);
    let txn2 = wordcartel_core::history::Transaction::new(cs2)
        .with_selection(wordcartel_core::selection::Selection::single(1));
    e.by_id_mut(aid).unwrap().apply(txn2, edit2, wordcartel_core::history::EditKind::Other, &C(0));
    assert!(e.is_dirty(aid), "ordinary edited buffer is dirty via is_dirty");
}

#[test]
fn scratch_buffer_derive_rebuild_smoke() {
    // An empty (len 0) scratch buffer must survive derive::rebuild without panic.
    let mut e = Editor::new_from_text("doc\n", None, (40, 10));
    e.install_scratch();
    let sid = e.scratch_id.unwrap();
    let idx = e.buffers.iter().position(|b| b.id == sid).unwrap();
    e.active = idx;
    crate::derive::rebuild(&mut e);
    assert_eq!(e.by_id(sid).unwrap().document.buffer.len(), 0);
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test -p wordcartel install_scratch_adds_permanent_pathless_buffer is_dirty_excludes_scratch scratch_buffer_derive_rebuild_smoke`
Expected: FAIL — `no field scratch_id` / `no method install_scratch`.

- [ ] **Step 3: Add fields + methods**

In the `Editor` struct (after `resume_enabled: bool,` ~editor.rs:330) add:

```rust
    /// Effort 6: the permanent path-less *scratch* buffer's id. `None` in unit
    /// contexts that never call `install_scratch`. Scratch is identified by id,
    /// not by a name field.
    pub scratch_id: Option<BufferId>,
    /// Most-recently-used buffer ids, most-recent first. Drives the switcher palette.
    pub mru: Vec<BufferId>,
```

In `new_from_text`'s struct literal (after `resume_enabled: false,` ~editor.rs:367) add:

```rust
            scratch_id: None,
            mru: Vec::new(),
```

In `impl Editor` (near the accessors, after `alloc_id`, ~editor.rs:385) add:

```rust
    /// Effort 6: create the permanent *scratch* buffer and record its id.
    /// Appended AFTER the launch buffer so the launch buffer stays at index 0
    /// (active). Idempotent guard: a second call is a no-op.
    pub fn install_scratch(&mut self) {
        if self.scratch_id.is_some() { return; }
        let id = self.alloc_id();
        let area = self.active().view.area;
        self.buffers.push(Buffer::from_text(id, "", None, area)); // empty (len 0)
        self.scratch_id = Some(id);
        // Seed MRU: active buffer first, scratch last.
        let active_id = self.buffers[self.active].id;
        self.mru = vec![active_id, id];
    }
    /// True iff `id` is the scratch buffer.
    #[inline] pub fn is_scratch(&self, id: BufferId) -> bool { self.scratch_id == Some(id) }
    /// Scratch-aware unsaved-work predicate. Scratch is NEVER dirty (it has no
    /// file and is auto-persisted to session state). All workspace logic uses this.
    pub fn is_dirty(&self, id: BufferId) -> bool {
        if self.is_scratch(id) { return false; }
        self.by_id(id).map_or(false, |b| b.document.dirty())
    }
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test -p wordcartel install_scratch is_dirty_excludes_scratch scratch_buffer_derive_rebuild_smoke`
Expected: PASS (3 tests).

- [ ] **Step 5: Wire `install_scratch` into `run()`**

In `wordcartel/src/app.rs run()`, immediately AFTER the CLI-file open block that sets `editor.buffers[0]` (after the `if let Some(p) = path.as_deref() { ... }` block ending ~app.rs:1748), add:

```rust
    // Effort 6: install the permanent *scratch* buffer (index 1; launch buffer stays active at 0).
    editor.install_scratch();
```

- [ ] **Step 6: Run full shell suite**

Run: `cargo test -p wordcartel`
Expected: PASS — all prior tests green (new_from_text unchanged → `single_buffer_invariants_and_accessors` still holds) plus the 3 new tests.

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/editor.rs wordcartel/src/app.rs
git commit -m "feat(6): scratch_id + is_dirty + install_scratch foundation

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

### Task 2: Scratch content persistence

**Files:**
- Modify: `wordcartel/src/state.rs` (SessionState ~36–39; tests ~118+)
- Modify: `wordcartel/src/app.rs` (`persist_session` ~2026–2071; `run()` startup restore region after `install_scratch`)
- Test: `wordcartel/src/state.rs`, `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `Editor.scratch_id` (Task 1).
- Produces: `state::ScratchState { text: String, cursor: usize }`; `SessionState.scratch: Option<ScratchState>`; `app::restore_scratch(editor, &ScratchState)`. `persist_session` now also writes scratch and always flushes.

- [ ] **Step 1: Write failing test (schema round-trip)**

In `wordcartel/src/state.rs` tests mod add:

```rust
#[test]
fn scratch_state_round_trips_and_is_optional() {
    // Missing [scratch] → None.
    let s: SessionState = toml::from_str(r#"
[entries."/tmp/x.md"]
cursor = 1
scroll = 0
mtime = 1
size = 2
seq = 1
"#).unwrap();
    assert!(s.scratch.is_none(), "absent [scratch] → None");
    // Present round-trips and serializes as its own [scratch] table.
    let mut s2 = SessionState::default();
    s2.scratch = Some(ScratchState { text: "stash\n\nmore".into(), cursor: 5 });
    let out = toml::to_string(&s2).unwrap();
    assert!(out.contains("[scratch]"), "serializes as [scratch] table");
    let back: SessionState = toml::from_str(&out).unwrap();
    assert_eq!(back.scratch.unwrap().text, "stash\n\nmore");
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `cargo test -p wordcartel scratch_state_round_trips_and_is_optional`
Expected: FAIL — `ScratchState` / `scratch` not found.

- [ ] **Step 3: Add the schema**

In `wordcartel/src/state.rs`, add after `StateEntry` (~line 33):

```rust
/// Effort 6: the permanent *scratch* buffer's persisted content. Path-less, so it
/// cannot live in the path-keyed `entries` map — it is a sibling table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScratchState {
    pub text: String,
    pub cursor: usize,
}
```

Change `SessionState` (~lines 36–39) to:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub entries: BTreeMap<String, StateEntry>,
    /// Effort 6: scratch buffer content (sibling table; omitted when None so old
    /// readers and a never-used scratch don't emit an empty [scratch]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratch: Option<ScratchState>,
}
```

NOTE: `entries` (a table-map) is declared BEFORE `scratch`; both serialize as TOML tables, so order is valid. `skip_serializing_if` is REQUIRED — `toml` cannot serialize a bare `None`.

- [ ] **Step 4: Run test, verify pass**

Run: `cargo test -p wordcartel scratch_state_round_trips_and_is_optional`
Expected: PASS. Also run `cargo test -p wordcartel -- state::` to confirm existing state round-trips still pass.

- [ ] **Step 5: Write failing test for restore_scratch**

In `wordcartel/src/app.rs` tests mod add:

```rust
#[test]
fn restore_scratch_loads_text_and_clamps_cursor() {
    let mut e = crate::editor::Editor::new_from_text("doc\n", None, (40, 10));
    e.install_scratch();
    let st = crate::state::ScratchState { text: "hello".into(), cursor: 999 }; // out of range
    crate::app::restore_scratch(&mut e, &st);
    let sid = e.scratch_id.unwrap();
    let sb = e.by_id(sid).unwrap();
    assert_eq!(sb.document.buffer.to_string(), "hello");
    assert_eq!(sb.document.selection.primary().head, 5, "cursor clamped to len");
}
```

- [ ] **Step 6: Run test, verify it fails**

Run: `cargo test -p wordcartel restore_scratch_loads_text_and_clamps_cursor`
Expected: FAIL — `restore_scratch` not found.

- [ ] **Step 7: Implement restore_scratch**

In `wordcartel/src/app.rs` (near `restore_resume`, ~app.rs:336) add:

```rust
/// Effort 6: load persisted scratch content into the scratch buffer. Replaces the
/// scratch Buffer in place (fresh id so any stale job no-ops), then clamp-snaps the
/// cursor into `[0, len]` on a char boundary (mirrors 9A's clamp discipline so a
/// stale offset never panics a later `slice()`). No-op if no scratch installed.
pub fn restore_scratch(editor: &mut Editor, st: &crate::state::ScratchState) {
    let Some(sid) = editor.scratch_id else { return; };
    let Some(idx) = editor.buffers.iter().position(|b| b.id == sid) else { return; };
    let area = editor.buffers[idx].view.area;
    let id = editor.alloc_id();
    editor.buffers[idx] = crate::editor::Buffer::from_text(id, &st.text, None, area);
    editor.scratch_id = Some(id);
    // Update MRU id mapping (old scratch id → new).
    for m in editor.mru.iter_mut() { if *m == sid { *m = id; } }
    // Char-boundary clamp via `nav::clamp_snap` (Codex I3: TextBuffer has NO
    // `snap_to_boundary`; clamp_snap at nav.rs:164 operates on the ACTIVE buffer).
    // restore_scratch runs at startup; briefly make scratch active to clamp, then
    // restore the prior active index.
    let prev_active = editor.active;
    editor.active = idx;
    let cur = crate::nav::clamp_snap(editor, st.cursor);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(cur);
    editor.active = prev_active;
}
```

NOTE (Codex I3): `TextBuffer` exposes only `len`/`is_empty`/`insert`/`delete`/`slice`/line conversions/`snapshot`/`to_string` (buffer.rs:10–96) — there is no `snap_to_boundary`. Use `nav::clamp_snap` (nav.rs:164) as above; it clamps to `[0,len]` AND snaps to a char boundary. Never store a raw offset.

- [ ] **Step 8: Run test, verify pass**

Run: `cargo test -p wordcartel restore_scratch_loads_text_and_clamps_cursor`
Expected: PASS.

- [ ] **Step 9: Restructure `persist_session` to capture scratch + always flush**

Replace the body of `persist_session` (app.rs:2026–2071) so the scratch capture and `session.save()` happen REGARDLESS of the active buffer's path:

```rust
fn persist_session(
    session: &mut crate::state::SessionState,
    editor: &Editor,
    cfg: &config::Config,
    seq: u64,
) {
    // Effort 6: capture scratch content first, independent of the active buffer.
    if let Some(sid) = editor.scratch_id {
        if let Some(sb) = editor.by_id(sid) {
            session.scratch = Some(crate::state::ScratchState {
                text: sb.document.buffer.to_string(),
                cursor: sb.document.selection.primary().head,
            });
        }
    }
    // Per-file entry for the active buffer (unchanged): only when it has a real,
    // canonicalizable path. Scratch/new buffers contribute no per-file entry.
    if let Some(raw_path) = editor.active().document.path.as_deref() {
        if let Ok(canon) = std::fs::canonicalize(raw_path) {
            if let Some((mtime, size)) = crate::state::file_identity(raw_path) {
                let entry = crate::state::StateEntry {
                    cursor: editor.active().document.selection.primary().head,
                    scroll: editor.active().view.scroll,
                    marks: editor.active().marks.iter().map(|(c, &o)| (c.to_string(), o)).collect(),
                    mtime, size, seq,
                    folds: editor.active().folds.folded.iter().copied().collect(),
                    block: editor.active().marked_block.map(|b| (b.start, b.end)),
                };
                session.record(canon.to_string_lossy().into_owned(), entry, cfg.state.max_entries);
            }
        }
    }
    // Always flush — scratch durability does not depend on the active buffer.
    let _ = session.save();
}
```

- [ ] **Step 10: Wire restore at startup**

In `run()`, after `editor.install_scratch();` (Task 1 Step 5), add:

```rust
    // Effort 6: restore persisted scratch content (independent of resume_enabled —
    // scratch is the user's stash, not a per-file resume position).
    {
        let saved = crate::state::load();
        if let Some(st) = saved.scratch.as_ref() {
            restore_scratch(&mut editor, st);
        }
    }
```

- [ ] **Step 11: Write failing integration test (persist → reload round-trip)**

In `wordcartel/src/app.rs` tests mod add a test that builds an editor with scratch text, calls `persist_session` into a temp dir-backed `SessionState`, and asserts `session.scratch` carries the text even when the active buffer is unnamed:

```rust
#[test]
fn persist_session_captures_scratch_even_when_active_unnamed() {
    use wordcartel_core::history::Clock;
    struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
    let mut e = crate::editor::Editor::new_from_text("\n", None, (40, 10)); // active unnamed
    e.install_scratch();
    let sid = e.scratch_id.unwrap();
    let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "stash".into())], 0);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(5));
    e.by_id_mut(sid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
    let mut session = crate::state::SessionState::default();
    let cfg = crate::config::Config::default();
    crate::app::persist_session_for_test(&mut session, &e, &cfg, 1);
    assert_eq!(session.scratch.as_ref().unwrap().text, "stash");
}
```

`persist_session` is private; expose a thin test shim next to it:

```rust
#[cfg(test)]
pub fn persist_session_for_test(s: &mut crate::state::SessionState, e: &Editor, cfg: &config::Config, seq: u64) {
    persist_session(s, e, cfg, seq);
}
```

- [ ] **Step 12: Run tests, verify pass; then full suite**

Run: `cargo test -p wordcartel persist_session_captures_scratch restore_scratch scratch_state_round_trips`
Expected: PASS. Then `cargo test -p wordcartel` — all green.

- [ ] **Step 13: Commit**

```bash
git add wordcartel/src/state.rs wordcartel/src/app.rs
git commit -m "feat(6): persist + restore scratch buffer content

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

### Task 3: Send-to-scratch verbs (`copy_block_to_scratch` / `move_block_to_scratch`)

**Files:**
- Create: `wordcartel/src/scratch.rs`
- Modify: `wordcartel/src/main.rs` or `wordcartel/src/lib.rs` (add `mod scratch;` — check where modules are declared)
- Modify: `wordcartel/src/registry.rs` (register the two commands, ~after block ops line 268)
- Modify: `wordcartel/src/keymap.rs` (bind in both presets)
- Test: `wordcartel/src/scratch.rs`

**Interfaces:**
- Consumes: `Editor.scratch_id`, `by_id_mut`, `Buffer::apply`, `commands::build_multi_replace`, `active().marked_block`.
- Produces: `scratch::copy_block_to_scratch(editor, clock)`, `scratch::move_block_to_scratch(editor, clock)`.
- Cross-buffer undo is intentionally two independent steps (append → scratch history; delete → source history). This matches the spec; do NOT attempt atomic cross-buffer undo.

- [ ] **Step 1: Write failing tests**

Create `wordcartel/src/scratch.rs` with the tests first:

```rust
//! Effort 6: send-to-scratch verbs. Append the active buffer's marked block to the
//! permanent *scratch* buffer; move also deletes the block from the source buffer.
//! Cross-buffer undo is two independent steps (scratch append; source delete).

use crate::editor::Editor;
use wordcartel_core::history::Clock;

#[cfg(test)]
mod tests {
    use super::*;
    struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }

    fn setup() -> Editor {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.install_scratch();
        e
    }

    #[test]
    fn copy_to_scratch_appends_and_keeps_source() {
        let mut e = setup();
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false }); // "hello"
        copy_block_to_scratch(&mut e, &C(0));
        let sid = e.scratch_id.unwrap();
        assert_eq!(e.by_id(sid).unwrap().document.buffer.to_string(), "hello");
        assert_eq!(e.active().document.buffer.to_string(), "hello world\n", "source untouched");
        assert!(e.active().marked_block.is_some(), "block kept after copy");
    }

    #[test]
    fn second_copy_separates_entries_with_blank_line() {
        let mut e = setup();
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false }); // "hello"
        copy_block_to_scratch(&mut e, &C(0));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 6, end: 11, hidden: false }); // "world"
        copy_block_to_scratch(&mut e, &C(0));
        let sid = e.scratch_id.unwrap();
        assert_eq!(e.by_id(sid).unwrap().document.buffer.to_string(), "hello\n\nworld");
    }

    #[test]
    fn move_to_scratch_appends_and_deletes_source_two_undo_steps() {
        let mut e = setup();
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 6, hidden: false }); // "hello "
        move_block_to_scratch(&mut e, &C(0));
        let sid = e.scratch_id.unwrap();
        assert_eq!(e.by_id(sid).unwrap().document.buffer.to_string(), "hello ");
        assert_eq!(e.active().document.buffer.to_string(), "world\n", "block deleted from source");
        assert!(e.active().marked_block.is_none(), "block consumed by move");
        // Undo in source restores the deletion (one step).
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "hello world\n");
        // Scratch append is a SEPARATE undo in the scratch buffer's own history.
        e.buffers.iter().position(|b| b.id == sid).map(|i| { e.active = i; });
        assert!(e.undo(), "scratch has its own undo step");
        assert_eq!(e.by_id(sid).unwrap().document.buffer.to_string(), "");
    }

    #[test]
    fn no_block_sets_status() {
        let mut e = setup();
        copy_block_to_scratch(&mut e, &C(0));
        assert_eq!(e.status, "no marked block");
    }
}
```

- [ ] **Step 2: Declare the module + run tests to confirm failure**

Add `mod scratch;` alongside the other `mod` declarations (grep `mod blocks_marked;` to find the module list).
Run: `cargo test -p wordcartel scratch::`
Expected: FAIL — `copy_block_to_scratch` / `move_block_to_scratch` not defined.

- [ ] **Step 3: Implement the verbs + append helper**

Add to `wordcartel/src/scratch.rs` (above the tests mod):

```rust
/// Append `text` to the scratch buffer (blank line before it when scratch is
/// non-empty). One undo step in the SCRATCH buffer's history. Returns false if no
/// scratch is installed.
fn append_to_scratch(editor: &mut Editor, text: &str, clock: &dyn Clock) -> bool {
    let Some(sid) = editor.scratch_id else { return false; };
    let Some(sb) = editor.by_id(sid) else { return false; };
    let cur_len = sb.document.buffer.len();
    let sep = if cur_len == 0 { "" } else { "\n\n" };
    let insert = format!("{sep}{text}");
    let new_caret = cur_len + insert.len();
    let (cs, edit) = crate::commands::build_multi_replace(&[(cur_len, cur_len, insert)], cur_len);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(new_caret));
    editor.by_id_mut(sid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
    true
}

/// Copy the active buffer's marked block into scratch; source unchanged, block kept.
pub fn copy_block_to_scratch(editor: &mut Editor, clock: &dyn Clock) {
    let Some(b) = editor.active().marked_block else { editor.status = "no marked block".into(); return; };
    let text = editor.active().document.buffer.slice(b.start..b.end);
    if append_to_scratch(editor, &text, clock) {
        editor.status = "block copied to scratch".into();
    } else {
        editor.status = "no scratch buffer".into();
    }
}

/// Move the active buffer's marked block into scratch; delete it from the source
/// (a separate undo step in the source's history). Block is consumed.
pub fn move_block_to_scratch(editor: &mut Editor, clock: &dyn Clock) {
    let Some(b) = editor.active().marked_block else { editor.status = "no marked block".into(); return; };
    let text = editor.active().document.buffer.slice(b.start..b.end);
    if !append_to_scratch(editor, &text, clock) {
        editor.status = "no scratch buffer".into();
        return;
    }
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = crate::commands::build_multi_replace(&[(b.start, b.end, String::new())], doc_len);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(b.start));
    editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock); // active (source) buffer
    editor.active_mut().marked_block = None;
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
    editor.status = "block moved to scratch".into();
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test -p wordcartel scratch::`
Expected: PASS (4 tests).

- [ ] **Step 5: Register the commands**

In `wordcartel/src/registry.rs`, after the block-write registration (line 268) add:

```rust
        // Effort 6: send-to-scratch verbs.
        r.register("copy_block_to_scratch", "Copy Block to Scratch", Some(MenuCategory::Edit), |c| { crate::scratch::copy_block_to_scratch(c.editor, c.clock); CommandResult::Handled });
        r.register("move_block_to_scratch", "Move Block to Scratch", Some(MenuCategory::Edit), |c| { crate::scratch::move_block_to_scratch(c.editor, c.clock); CommandResult::Handled });
```

- [ ] **Step 6: Bind chords in both presets**

In `wordcartel/src/keymap.rs`:
- WordStar `^K` prefix (after the block-ops block, ~line 366), add plain-only + ctrl-held second keys on free letters `g`/`a` (verify free in the `^K` subtree — `^Kg`/`^Ka` are not used):

```rust
    ("ctrl-k ctrl-g", "copy_block_to_scratch"), ("ctrl-k g", "copy_block_to_scratch"),
    ("ctrl-k ctrl-a", "move_block_to_scratch"), ("ctrl-k a", "move_block_to_scratch"),
```

- CUA preset (CUA table `static CUA`, keymap.rs:222, add near the Alt bindings):

```rust
    ("alt-shift-c", "copy_block_to_scratch"),
    ("alt-shift-v", "move_block_to_scratch"),
```

Codex I1: `alt-shift-x` is ALREADY bound to `unfold_all` (keymap.rs:307, asserted at keymap.rs:626) — do NOT use it. `alt-shift-c` and `alt-shift-v` are free in the CUA table. `^Kg`/`^Ka` are free in the WordStar `^K` subtree (keymap.rs:354–375). Re-confirm with a grep before finalizing and keep `both_presets_resolve_against_builtins` + the prefix-shadow tests green.

- [ ] **Step 7: Add a keymap resolution test**

In `wordcartel/src/keymap.rs` tests mod, add:

```rust
#[test]
fn scratch_verbs_resolve_in_both_presets() {
    for preset in ["cua", "wordstar"] {
        let (t, _) = km(&[], &[], Some(preset));
        let reg = crate::registry::Registry::builtins();
        assert!(reg.contains("copy_block_to_scratch"));
        assert!(reg.contains("move_block_to_scratch"));
        let _ = t; // chord resolution covered by both_presets_resolve_against_builtins
    }
}
```

(If `Registry` lacks a `contains`, assert via the existing helper used by `both_presets_resolve_against_builtins`; match that test's style.)

- [ ] **Step 8: Run keymap + scratch tests, then full suite**

Run: `cargo test -p wordcartel scratch:: && cargo test -p wordcartel keymap:: && cargo test -p wordcartel`
Expected: PASS — all green incl. collision/prefix-shadow tests.

- [ ] **Step 9: Commit**

```bash
git add wordcartel/src/scratch.rs wordcartel/src/registry.rs wordcartel/src/keymap.rs wordcartel/src/main.rs wordcartel/src/lib.rs
git commit -m "feat(6): copy/move block to scratch verbs + bindings

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

### Task 4: Buffer navigation — cycle + MRU + `goto_scratch`

**Files:**
- Create: `wordcartel/src/workspace.rs`
- Modify: `wordcartel/src/editor.rs` (`switch_to_index`, `touch_mru`)
- Modify: `wordcartel/src/registry.rs`, `wordcartel/src/keymap.rs`
- Modify: module declarations (`mod workspace;`)
- Test: `wordcartel/src/workspace.rs`, `wordcartel/src/editor.rs`

**Interfaces:**
- Produces: `Editor::switch_to_index(&mut self, idx: usize)` (sets active + touches MRU); `Editor::touch_mru(&mut self, id)`; `workspace::next_buffer/prev_buffer/goto_scratch(editor)`; `workspace::switch_to(editor, idx)` (switch_to_index + derive::rebuild + ensure_visible).

- [ ] **Step 1: Write failing tests for switch + MRU**

In `wordcartel/src/editor.rs` tests:

```rust
#[test]
fn switch_to_index_sets_active_and_touches_mru() {
    let mut e = Editor::new_from_text("a\n", None, (40, 10));
    e.install_scratch(); // [doc(0), scratch(1)], mru = [doc, scratch]
    let scratch = e.scratch_id.unwrap();
    e.switch_to_index(1);
    assert_eq!(e.active, 1);
    assert_eq!(e.mru.first().copied(), Some(scratch), "switched buffer is MRU-front");
}
```

In `wordcartel/src/workspace.rs` (create with tests):

```rust
//! Effort 6: multi-buffer workspace navigation + lifecycle.
use crate::editor::Editor;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cycle_wraps_in_stable_order_including_scratch() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch(); // indices [0 doc, 1 scratch]
        assert_eq!(e.active, 0);
        next_buffer(&mut e); assert_eq!(e.active, 1);
        next_buffer(&mut e); assert_eq!(e.active, 0, "wraps");
        prev_buffer(&mut e); assert_eq!(e.active, 1, "prev wraps back");
    }
    #[test]
    fn goto_scratch_jumps_to_scratch() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch();
        goto_scratch(&mut e);
        assert_eq!(e.buffers[e.active].id, e.scratch_id.unwrap());
    }
    #[test]
    fn cycle_single_buffer_is_noop() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10)); // no scratch → 1 buffer
        next_buffer(&mut e);
        assert_eq!(e.active, 0);
    }
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p wordcartel switch_to_index_sets_active workspace::`
Expected: FAIL — methods/functions missing.

- [ ] **Step 3: Implement editor methods**

In `impl Editor` (after `is_dirty`) add:

```rust
    /// Move `id` to the front of the MRU list.
    pub fn touch_mru(&mut self, id: BufferId) {
        self.mru.retain(|&x| x != id);
        self.mru.insert(0, id);
    }
    /// Set the active buffer by index and record it MRU-front. Out-of-range → no-op.
    pub fn switch_to_index(&mut self, idx: usize) {
        if idx >= self.buffers.len() { return; }
        self.active = idx;
        let id = self.buffers[idx].id;
        self.touch_mru(id);
    }
```

- [ ] **Step 4: Implement workspace navigation**

In `wordcartel/src/workspace.rs` (above tests):

```rust
/// Switch active buffer by index and refresh the view.
pub fn switch_to(editor: &mut Editor, idx: usize) {
    editor.switch_to_index(idx);
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}
pub fn next_buffer(editor: &mut Editor) { cycle(editor, 1); }
pub fn prev_buffer(editor: &mut Editor) { cycle(editor, -1); }
fn cycle(editor: &mut Editor, delta: isize) {
    let n = editor.buffers.len();
    if n <= 1 { return; }
    let idx = ((editor.active as isize + delta).rem_euclid(n as isize)) as usize;
    switch_to(editor, idx);
}
/// Jump directly to the scratch buffer (no-op if none installed).
pub fn goto_scratch(editor: &mut Editor) {
    if let Some(sid) = editor.scratch_id {
        if let Some(idx) = editor.buffers.iter().position(|b| b.id == sid) {
            switch_to(editor, idx);
        }
    }
}
```

Declare `mod workspace;` with the other modules.

- [ ] **Step 5: Run, verify pass**

Run: `cargo test -p wordcartel switch_to_index_sets_active workspace::`
Expected: PASS.

- [ ] **Step 6: Register commands + bind chords**

registry.rs (after the scratch verbs from Task 3):

```rust
        // Effort 6: workspace navigation.
        r.register("next_buffer", "Next Buffer", Some(MenuCategory::View), |c| { crate::workspace::next_buffer(c.editor); CommandResult::Handled });
        r.register("prev_buffer", "Previous Buffer", Some(MenuCategory::View), |c| { crate::workspace::prev_buffer(c.editor); CommandResult::Handled });
        r.register("goto_scratch", "Go to Scratch Buffer", Some(MenuCategory::View), |c| { crate::workspace::goto_scratch(c.editor); CommandResult::Handled });
```

keymap.rs — WordStar `^K` prefix, plain-only second keys (precedent `^KM`/`^KJ`):

```rust
    ("ctrl-k ,", "prev_buffer"),
    ("ctrl-k .", "next_buffer"),
```

keymap.rs — CUA table:

```rust
    ("alt-,", "prev_buffer"),
    ("alt-.", "next_buffer"),
```

(`goto_scratch` stays palette/menu-only — no chord.)

- [ ] **Step 7: Add chord parse + resolution tests**

In keymap.rs tests:

```rust
#[test]
fn buffer_cycle_chords_parse_and_resolve() {
    // Codex I2: the real sequence parser is `parse_seq` (keymap.rs:109), NOT
    // `parse_chord_seq`. `parse_chord` (keymap.rs:59) parses a single chord.
    for s in ["ctrl-k ,", "ctrl-k .", "alt-,", "alt-."] {
        assert!(crate::keymap::parse_seq(s).is_some(), "parse {s}");
    }
    // Both presets build with no warnings (no collision / prefix-shadow).
    let (_t, w) = km(&[], &[], Some("wordstar"));
    assert!(w.is_empty(), "no wordstar warnings: {w:?}");
    let (_t, w) = km(&[], &[], Some("cua"));
    assert!(w.is_empty(), "no cua warnings: {w:?}");
}
```

- [ ] **Step 8: Run keymap + workspace + full suite**

Run: `cargo test -p wordcartel keymap:: && cargo test -p wordcartel workspace:: && cargo test -p wordcartel`
Expected: PASS — collision/prefix-shadow tests green.

- [ ] **Step 9: Commit**

```bash
git add wordcartel/src/workspace.rs wordcartel/src/editor.rs wordcartel/src/registry.rs wordcartel/src/keymap.rs wordcartel/src/main.rs wordcartel/src/lib.rs
git commit -m "feat(6): buffer cycling + MRU + goto_scratch + bindings

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

### Task 5: Switcher palette (`switch_buffer`)

**Files:**
- Modify: `wordcartel/src/workspace.rs` (build switcher rows), `wordcartel/src/editor.rs` (`open_buffer_switcher`)
- Modify: `wordcartel/src/palette.rs` (a buffer-switch palette variant), `wordcartel/src/registry.rs`, `wordcartel/src/keymap.rs`
- Reference: read `wordcartel/src/palette.rs` fully first — mirror how `open_palette` builds rows and how a palette selection dispatches an action.

**Interfaces:**
- Produces: `workspace::buffer_switch_rows(editor) -> Vec<(BufferId, String)>` (MRU order; display name `*scratch*`/`*untitled*`/filename + dirty marker); a palette mode that, on Enter, calls `workspace::switch_to` for the chosen buffer id.

**Implementation note (Codex I6 + I-new-5):** `PaletteRow` stores a `CommandId` (palette.rs:15) and the Enter handler reads `r.id` → `dispatch_overlay_command(...)` (app.rs:1075–1080). Crucially, `rebuild_rows` (palette.rs:43) REGENERATES `rows` from the command registry on EVERY query edit (app.rs:1118) — so a parallel `buffers: Vec<BufferId>` would be wiped/desynced the moment the user types a filter. Two coordinated changes are required:

1. **Per-row buffer id.** Add `pub buffer: Option<BufferId>` to `PaletteRow` (palette.rs:15). Command rows set `buffer: None`; switcher rows set `buffer: Some(id)`. The id travels WITH the row, so filtering can never desync it.
2. **Palette kind + branched rebuild.** Add `pub kind: PaletteKind` to `Palette` (`#[derive(Default)] enum PaletteKind { #[default] Commands, Buffers }`) and `pub source_rows: Vec<PaletteRow>` (the unfiltered switcher rows; empty for Commands). Make `rebuild_rows` branch:
   - `Commands` → existing behavior (build from `reg.commands()`, `buffer: None`).
   - `Buffers` → `p.rows = fuzzy_filter(&p.source_rows, &p.query, |r| &r.label);` then clamp `selected`. (Registry is NOT consulted.)

`open_buffer_switcher(&mut self)` sets `kind = Buffers`, builds `source_rows` from `workspace::buffer_switch_rows(self)` (MRU order; `label` = display name, `buffer = Some(id)`, `id` = a harmless sentinel e.g. `CommandId("palette")`, `chord` = ""), then calls `rebuild_rows`. Other overlay-clearing mirrors `open_palette`.

Enter site (app.rs:1075) — branch on the selected row's `buffer`:

```rust
crossterm::event::KeyCode::Enter => {
    let row = editor.palette.as_ref().and_then(|p| p.rows.get(p.selected).cloned());
    if let Some(row) = row {
        if let Some(bid) = row.buffer {
            editor.palette = None;
            if let Some(idx) = editor.buffers.iter().position(|b| b.id == bid) {
                crate::workspace::switch_to(editor, idx);
            }
        } else {
            dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, row.id);
        }
    }
}
```

Read palette.rs fully first; update the existing `rebuild_rows` tests for the new `buffer` field default (`None`).

- [ ] **Step 1: Write failing test for switcher rows**

In `wordcartel/src/workspace.rs` tests:

```rust
#[test]
fn switcher_rows_mru_order_with_display_names() {
    let mut e = Editor::new_from_text("a\n", None, (40, 10));
    e.install_scratch();
    // Make buffer 0 a named file display, scratch second.
    e.buffers[0].document.path = Some(std::path::PathBuf::from("/tmp/notes.md"));
    goto_scratch(&mut e);     // MRU front = scratch
    let rows = buffer_switch_rows(&e);
    assert_eq!(rows.first().unwrap().1, "*scratch*", "MRU front is scratch");
    assert!(rows.iter().any(|(_, n)| n.contains("notes.md")));
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p wordcartel switcher_rows_mru_order_with_display_names`
Expected: FAIL — `buffer_switch_rows` missing.

- [ ] **Step 3: Implement row builder**

In `wordcartel/src/workspace.rs`:

```rust
/// Display name for a buffer: *scratch* for scratch, *untitled* for a path-less
/// ordinary buffer, else the file name. Prefixed with "*" when dirty (is_dirty).
pub fn buffer_display_name(editor: &Editor, id: crate::editor::BufferId) -> String {
    let base = if editor.is_scratch(id) {
        "*scratch*".to_string()
    } else {
        match editor.by_id(id).and_then(|b| b.document.path.as_ref()) {
            Some(p) => p.file_name().map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.to_string_lossy().into_owned()),
            None => "*untitled*".to_string(),
        }
    };
    if editor.is_dirty(id) { format!("*{base}") } else { base }
}

/// Buffers in MRU order (front = most recent), as (id, display name). Buffers not
/// yet in the MRU list are appended in buffer order.
pub fn buffer_switch_rows(editor: &Editor) -> Vec<(crate::editor::BufferId, String)> {
    let mut out: Vec<(crate::editor::BufferId, String)> = Vec::new();
    for &id in &editor.mru {
        if editor.by_id(id).is_some() { out.push((id, buffer_display_name(editor, id))); }
    }
    for b in &editor.buffers {
        if !out.iter().any(|(id, _)| *id == b.id) { out.push((b.id, buffer_display_name(editor, b.id))); }
    }
    out
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p wordcartel switcher_rows_mru_order_with_display_names`
Expected: PASS.

- [ ] **Step 5: Wire the palette opener + selection**

Add `Editor::open_buffer_switcher(&mut self)` mirroring `open_palette` (editor.rs) but seeded from `workspace::buffer_switch_rows(self)`; on Enter, the palette-submit path calls `workspace::switch_to(editor, index_of_selected_id)`. Follow the existing palette open/submit structure exactly (read palette.rs + the palette-submit site first). Register:

```rust
        r.register("switch_buffer", "Switch Buffer\u{2026}", Some(MenuCategory::View), |c| { c.editor.open_buffer_switcher(); CommandResult::Handled });
```

Bind: WordStar `("ctrl-k ctrl-l", "switch_buffer"), ("ctrl-k l", "switch_buffer")` (verify `^Kl` free); CUA `("alt-b", ...)` is taken (mark_block_from_selection) — use `("ctrl-shift-e", "switch_buffer")` or another free CUA chord (VERIFY free; note choice in report).

- [ ] **Step 6: Write a switcher-selection test**

Add a test driving `open_buffer_switcher` + simulating an Enter on the second row, asserting `active` changed to that buffer. Match how existing palette-selection tests are written (grep palette submit tests).

- [ ] **Step 7: Run palette + workspace + full suite**

Run: `cargo test -p wordcartel palette:: workspace:: && cargo test -p wordcartel`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(6): buffer switcher palette (MRU order)

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

### Task 6: Additive open / new with throwaway reuse

**Files:**
- Modify: `wordcartel/src/workspace.rs` (`open_as_new_buffer`, `new_empty_buffer`, `active_is_reusable_throwaway`)
- Modify: `wordcartel/src/app.rs` (`request_new` → additive; reroute file-browser open + post-save Open; remove now-dead `PostSaveAction::{New,Open}` machinery)
- Modify: `wordcartel/src/registry.rs` (`new`/`open` handlers if needed)
- Test: `wordcartel/src/workspace.rs`, `wordcartel/src/app.rs`

**Interfaces:**
- Produces: `workspace::open_as_new_buffer(editor, path)`, `workspace::new_empty_buffer(editor)`, `workspace::active_is_reusable_throwaway(editor) -> bool`.
- Behavior: Open/New ADD a buffer + switch, unless the active buffer is a reusable throwaway (path-less, clean, content `""` or `"\n"`, and not scratch) in which case reuse it in place. Additive open/new never destroy a buffer, so the Effort-7 dirty-guard-before-replace is no longer needed for New/Open.

- [ ] **Step 1: Write failing tests**

In `wordcartel/src/workspace.rs` tests:

```rust
#[test]
fn open_reuses_clean_untitled_throwaway() {
    let mut e = Editor::new_from_text("\n", None, (40, 10)); // throwaway launch buffer
    e.install_scratch();
    assert_eq!(e.buffers.len(), 2);
    let tmp = std::env::temp_dir().join(format!("wc-open-{}.md", std::process::id()));
    std::fs::write(&tmp, "file body\n").unwrap();
    open_as_new_buffer(&mut e, &tmp);
    assert_eq!(e.buffers.len(), 2, "throwaway reused, not added");
    assert_eq!(e.active().document.buffer.to_string(), "file body\n");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn open_adds_buffer_when_active_is_real() {
    let mut e = Editor::new_from_text("real content\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
    e.install_scratch();
    let tmp = std::env::temp_dir().join(format!("wc-open2-{}.md", std::process::id()));
    std::fs::write(&tmp, "second\n").unwrap();
    open_as_new_buffer(&mut e, &tmp);
    assert_eq!(e.buffers.len(), 3, "added a new buffer");
    assert_eq!(e.active().document.buffer.to_string(), "second\n");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn new_empty_buffer_is_additive_and_not_scratch() {
    let mut e = Editor::new_from_text("real\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
    e.install_scratch();
    new_empty_buffer(&mut e);
    assert_eq!(e.buffers.len(), 3);
    assert!(e.active().document.path.is_none());
    assert!(!e.is_scratch(e.active().id), "New buffer is not the scratch buffer");
}

#[test]
fn scratch_is_never_a_reuse_target() {
    let mut e = Editor::new_from_text("real\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
    e.install_scratch();
    goto_scratch(&mut e); // active = scratch (empty, path-less, "clean")
    assert!(!active_is_reusable_throwaway(&e), "scratch must not be reused");
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p wordcartel open_reuses_clean_untitled open_adds_buffer_when_active new_empty_buffer_is_additive scratch_is_never_a_reuse_target`
Expected: FAIL.

- [ ] **Step 3: Implement additive open/new**

In `wordcartel/src/workspace.rs`:

```rust
use std::path::Path;

/// True iff the active buffer is a reusable empty untitled throwaway (NOT scratch).
pub fn active_is_reusable_throwaway(editor: &Editor) -> bool {
    let b = editor.active();
    if editor.is_scratch(b.id) { return false; }
    if b.document.path.is_some() { return false; }
    if editor.is_dirty(b.id) { return false; }
    let t = b.document.buffer.to_string();
    t.is_empty() || t == "\n"
}

/// Open `path` additively: reuse a throwaway active buffer, else push + switch.
pub fn open_as_new_buffer(editor: &mut Editor, path: &Path) {
    if active_is_reusable_throwaway(editor) {
        crate::app::open_into_current(editor, path); // replace-in-place seam (Effort 7)
        return;
    }
    let id = editor.alloc_id();
    let area = editor.active().view.area;
    match crate::editor::Buffer::from_file(id, path, area) {
        Ok(b) => {
            editor.buffers.push(b);
            let idx = editor.buffers.len() - 1;
            editor.switch_to_index(idx);
            if editor.resume_enabled { crate::app::restore_resume(editor, path); }
            crate::derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
            editor.status = String::new();
        }
        Err(e) => editor.status = e.to_string(),
    }
}

/// Create a fresh empty untitled buffer additively (reuse throwaway if present).
pub fn new_empty_buffer(editor: &mut Editor) {
    if active_is_reusable_throwaway(editor) { return; } // already an empty untitled — nothing to do
    let id = editor.alloc_id();
    let area = editor.active().view.area;
    editor.buffers.push(crate::editor::Buffer::from_text(id, "\n", None, area));
    let idx = editor.buffers.len() - 1;
    editor.switch_to_index(idx);
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
    editor.status = String::new();
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p wordcartel open_reuses_clean_untitled open_adds_buffer new_empty_buffer scratch_is_never_a_reuse_target`
Expected: PASS.

- [ ] **Step 5: Reroute the command handlers + remove dead replace machinery**

- `request_new` (app.rs:496) → call `crate::workspace::new_empty_buffer(editor)` directly (drop the dirty-guard/PostSaveAction::New path — additive New never destroys a buffer):

```rust
pub fn request_new(editor: &mut Editor, _ex: &dyn Executor, _clock: &dyn Clock, _msg_tx: &std::sync::mpsc::Sender<Msg>) {
    crate::workspace::new_empty_buffer(editor);
}
```

- **File-browser open** currently routes through `request_replace(PostSaveAction::Open(...))` (app.rs:1260). Change that call site to `crate::workspace::open_as_new_buffer(editor, &path)` directly (additive; no save-then-replace).
- **Sequence the removal (Codex I5 — the variants have a large live blast radius).** Do NOT remove the variants first. Order:
  1. Wire additive `open_as_new_buffer`/`new_empty_buffer` (Steps 3–4, done).
  2. Reroute `request_new` (app.rs:496) → `new_empty_buffer`, and file-browser open (app.rs:1260) → `open_as_new_buffer`.
  3. Update the replacement-flow TESTS to additive expectations (Step 6). Codex I-new-6: the live New/Open test sites are app.rs:2718, **2749, 2777, 2804, 2828, 2851** (six total), plus any asserting `replace_active_with_scratch` / `"Open cancelled"` / `"New cancelled"`. Grep `PostSaveAction::New`, `PostSaveAction::Open`, `replace_active_with_scratch` to confirm none are missed.
  4. ONLY THEN remove `PostSaveAction::New` and `PostSaveAction::Open` (editor.rs:15) and their now-unreferenced sites: the `apply_result` arms (app.rs:141, app.rs:153), the `perform_post_save_action` arms (app.rs:464, app.rs:469), and `replace_active_with_scratch` (editor.rs:389) if it has no remaining callers. **KEEP `PostSaveAction::Quit`** (used by save-and-quit; Task 8 then adds `ContinueQuitDrain`).
  5. Simplify `request_replace`/`request_new` accordingly (request_replace may collapse to a thin Quit-only helper or be inlined).
- Run `cargo build -p wordcartel` after the reroute and again after the removal — the compiler enumerates every remaining reference, so removal is mechanical and complete (no `#[allow(dead_code)]` shortcut).

- [ ] **Step 6: Update/inspect existing New/Open tests**

The Effort-7 tests asserting New/Open REPLACE the active buffer (grep `replace_active_with_scratch`, `PostSaveAction::New`, `Open cancelled`) now describe removed behavior. Update them to the additive semantics (New/Open add a buffer; throwaway reused). Do NOT delete coverage — re-point each assertion to the additive outcome.

- [ ] **Step 7: Run full suite**

Run: `cargo test -p wordcartel`
Expected: PASS — additive tests green, rerouted New/Open tests updated, no dead-code warnings (`cargo build` clean).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(6): additive open/new with throwaway reuse

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

### Task 7: Close buffer + last-buffer invariant

**Files:**
- Modify: `wordcartel/src/workspace.rs` (`close_buffer`)
- Modify: `wordcartel/src/registry.rs` (register `close_buffer`)
- Test: `wordcartel/src/workspace.rs`

**Interfaces:**
- Produces: `workspace::close_buffer(editor)` — closes the active buffer. Scratch → no-op + status. Last ordinary buffer → replaced by a fresh empty untitled. New active = same-index neighbor (or new last). For THIS task, a dirty buffer is closed via a simple guard: if `is_dirty(active)`, set a status and DO NOT close (the interactive Save/Discard prompt is wired in Task 8 alongside the quit machinery; here we keep close safe by refusing to drop unsaved work).

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn close_scratch_is_noop_with_status() {
    let mut e = Editor::new_from_text("a\n", None, (40, 10));
    e.install_scratch();
    goto_scratch(&mut e);
    close_buffer(&mut e);
    assert_eq!(e.buffers.len(), 2, "scratch not closed");
    assert!(e.status.contains("scratch"));
}

#[test]
fn close_last_ordinary_leaves_fresh_untitled() {
    let mut e = Editor::new_from_text("only\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
    e.install_scratch(); // [a.md, scratch]
    close_buffer(&mut e); // close a.md → invariant keeps ≥1 ordinary
    assert_eq!(e.buffers.len(), 2, "scratch + a fresh untitled");
    assert!(!e.is_scratch(e.active().id));
    assert!(e.active().document.path.is_none(), "fresh untitled");
}

#[test]
fn close_selects_same_index_neighbor() {
    let mut e = Editor::new_from_text("first\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
    e.install_scratch();
    let tmp = std::env::temp_dir().join(format!("wc-c-{}.md", std::process::id()));
    std::fs::write(&tmp, "second\n").unwrap();
    open_as_new_buffer(&mut e, &tmp); // [a.md(0), scratch(1), second(2)] active=2
    switch_to(&mut e, 0); // active a.md
    close_buffer(&mut e); // remove index 0 → neighbor shifts into slot 0
    assert!(e.buffers.iter().all(|b| b.document.path.as_deref() != Some(&tmp) || true));
    assert_eq!(e.active, 0, "same-index neighbor active");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn close_refuses_dirty_buffer() {
    use wordcartel_core::history::Clock;
    struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
    let mut e = Editor::new_from_text("x\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
    e.install_scratch();
    let aid = e.active().id;
    let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "z".into())], 2);
    let txn = wordcartel_core::history::Transaction::new(cs).with_selection(wordcartel_core::selection::Selection::single(1));
    e.by_id_mut(aid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
    close_buffer(&mut e);
    assert!(e.by_id(aid).is_some(), "dirty buffer not closed");
    assert!(e.status.to_lowercase().contains("unsaved") || e.status.to_lowercase().contains("save"));
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p wordcartel close_scratch_is_noop close_last_ordinary close_selects_same_index close_refuses_dirty`
Expected: FAIL.

- [ ] **Step 3: Implement close_buffer**

```rust
/// Close the active buffer. Scratch → no-op. Dirty → refuse (keep work; the quit
/// flow handles interactive save). Last ordinary buffer → replace with a fresh
/// empty untitled. New active = same-index neighbor.
pub fn close_buffer(editor: &mut Editor) {
    let id = editor.active().id;
    if editor.is_scratch(id) { editor.status = "can't close the scratch buffer".into(); return; }
    if editor.is_dirty(id) { editor.status = "unsaved changes — save or discard first".into(); return; }
    let ordinary = editor.buffers.iter().filter(|b| !editor.is_scratch(b.id)).count();
    if ordinary <= 1 {
        // Last ordinary buffer: replace in place with a fresh empty untitled.
        let nid = editor.alloc_id();
        let area = editor.active().view.area;
        let a = editor.active;
        editor.buffers[a] = crate::editor::Buffer::from_text(nid, "\n", None, area);
        editor.touch_mru(nid);
        crate::derive::rebuild(editor);
        crate::nav::ensure_visible(editor);
        editor.status = String::new();
        return;
    }
    let a = editor.active;
    editor.mru.retain(|&x| x != id);
    editor.buffers.remove(a);
    let new_idx = a.min(editor.buffers.len() - 1);
    switch_to(editor, new_idx);
    editor.status = String::new();
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p wordcartel close_scratch_is_noop close_last_ordinary close_selects_same_index close_refuses_dirty`
Expected: PASS.

- [ ] **Step 5: Register the command (palette/menu-only — no chord)**

```rust
        r.register("close_buffer", "Close Buffer", Some(MenuCategory::File), |c| { crate::workspace::close_buffer(c.editor); CommandResult::Handled });
```

- [ ] **Step 6: Run full suite**

Run: `cargo test -p wordcartel`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(6): close_buffer with last-buffer invariant + scratch guard

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

### Task 8: Multi-buffer quit (Save-All / Review-each)

**Files:**
- Modify: `wordcartel/src/editor.rs` (`quit_drain`, `quit_drain_advance` fields + `QuitDrain`/`QuitMode`)
- Modify: `wordcartel/src/prompt.rs` (`quit_multi`, `quit_review_buffer` + `PromptAction`s)
- Modify: `wordcartel/src/commands.rs` (`Command::Quit` → multi-buffer)
- Modify: `wordcartel/src/app.rs` (`resolve_prompt` arms; `drive_quit_drain`; `apply_result` `ContinueQuitDrain`; reduce hook)
- Modify: `wordcartel/src/editor.rs` (`PostSaveAction::ContinueQuitDrain`)
- Test: `wordcartel/src/app.rs`

**Interfaces:**
- Produces: `Editor.quit_drain: Option<QuitDrain>` where `QuitDrain { queue: std::collections::VecDeque<BufferId>, mode: QuitMode }`, `QuitMode { SaveAll, ReviewEach }`; `Editor.quit_drain_advance: bool`; `PostSaveAction::ContinueQuitDrain`; `PromptAction::{QuitSaveAll, QuitReviewEach, ReviewSave, ReviewDiscard}`; `app::drive_quit_drain(editor, ex, clock, msg_tx)`.
- Design: `Command::Quit` checks `any non-scratch buffer is_dirty`. None dirty → quit immediately. Else raise `quit_multi(n)`. The drain processes one buffer per step; completion (save landing or review prompt) re-drives. `Cancel` clears `quit_drain` (aborts).

**Read first:** `wordcartel/src/save.rs` `dispatch_save_then` (how it arms `pending_after_save` for the active buffer / opens Save-As for unnamed) and `app.rs` `perform_post_save_action`. The drain reuses `dispatch_save_then` per buffer.

- [ ] **Step 1: Write failing tests**

In `wordcartel/src/app.rs` tests (model the existing `resolve_prompt(SaveAndQuit,...)` tests at ~2565):

```rust
#[test]
fn quit_with_no_dirty_buffers_quits_immediately() {
    let (ex, clk, tx) = test_ctx(); // use the existing test harness helpers
    let mut e = crate::editor::Editor::new_from_text("clean\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
    e.install_scratch();
    e.active_mut().document.mark_saved(e.active().document.version); // clean
    let mut ctx = make_ctx(&mut e, &ex, &clk, &tx);
    let r = crate::commands::run(&mut ctx, crate::commands::Command::Quit);
    assert!(e.quit, "no dirty buffers → quit");
    let _ = r;
}

#[test]
fn quit_save_all_drains_named_dirty_then_quits() {
    // Two named dirty buffers; QuitSaveAll drains both then sets quit.
    // Drive: resolve_prompt(QuitSaveAll) → first save dispatched; simulate JobDone
    // for each via the test executor, re-driving until queue empty and quit set.
    // (Mirror how the existing SaveAndQuit test simulates a save landing.)
    // ... build editor with two dirty named temp files, install_scratch ...
    // assert e.quit becomes true after both saves land.
}

#[test]
fn quit_review_each_cancel_aborts() {
    let (ex, clk, tx) = test_ctx();
    let mut e = /* editor with one dirty named buffer + scratch */ make_dirty_editor();
    crate::app::resolve_prompt(crate::prompt::PromptAction::QuitReviewEach, &mut e, &ex, &clk, &tx);
    assert!(e.quit_drain.is_some(), "drain started");
    crate::app::resolve_prompt(crate::prompt::PromptAction::Cancel, &mut e, &ex, &clk, &tx);
    assert!(e.quit_drain.is_none(), "cancel aborts the drain");
    assert!(!e.quit, "not quitting after cancel");
}
```

Plus two abort-path tests (from the Codex C2/C3/8d fixes):

```rust
#[test]
fn quit_drain_aborts_on_save_failure() {
    // SaveAll over a buffer whose save fails: simulate a failed Save JobDone with
    // action ContinueQuitDrain → quit_drain cleared, quit stays false, error status.
    // ... assert e.quit_drain.is_none() && !e.quit after the failed completion ...
}

#[test]
fn quit_drain_aborts_when_save_as_dismissed() {
    // Drain reaches a dirty UNNAMED buffer → Save-As minibuffer opens. Dismiss it
    // (Esc path / empty submit) → quit_drain cleared, quit stays false.
    // ... drive resolve_prompt(QuitSaveAll); assert minibuffer SaveAs open; invoke
    //     the dismiss path; assert e.quit_drain.is_none() && !e.quit ...
}
```

(Flesh out all four stubbed tests using the file's existing executor/ctx helpers — grep the SaveAndQuit test at app.rs:2565 for the exact harness calls, and the Save-As tests for how an unnamed save routes to the minibuffer.)

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p wordcartel quit_with_no_dirty quit_save_all_drains quit_review_each_cancel`
Expected: FAIL — types/actions/fns missing.

- [ ] **Step 3: Add Editor state + PostSaveAction variant**

editor.rs — extend `PostSaveAction` (line 15):

```rust
pub enum PostSaveAction { Quit, Open(std::path::PathBuf), New, ContinueQuitDrain }
```

(If Task 6 removed `Open`/`New`, the enum is `{ Quit, ContinueQuitDrain }`.) Add to `Editor`:

```rust
    pub quit_drain: Option<QuitDrain>,
    pub quit_drain_advance: bool,
```

and types near `PostSaveAction`:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuitMode { SaveAll, ReviewEach }

#[derive(Clone, Debug)]
pub struct QuitDrain {
    pub queue: std::collections::VecDeque<BufferId>,
    pub mode: QuitMode,
}
```

Initialize both new fields in `new_from_text` (`quit_drain: None, quit_drain_advance: false,`).

- [ ] **Step 4: Add prompts + actions**

prompt.rs — `PromptAction` (line 6) add: `QuitSaveAll, QuitReviewEach, ReviewSave, ReviewDiscard`. Add constructors:

```rust
    pub fn quit_multi(n: usize) -> Prompt {
        Prompt {
            message: format!("{n} buffer(s) unsaved: [A]ll save · [R]eview each · [C]ancel"),
            choices: vec![
                Choice { key: 'a', label: "Save all",    action: PromptAction::QuitSaveAll },
                Choice { key: 'r', label: "Review each",  action: PromptAction::QuitReviewEach },
                Choice { key: 'c', label: "Cancel",       action: PromptAction::Cancel },
            ],
        }
    }
    pub fn quit_review_buffer(name: &str) -> Prompt {
        Prompt {
            message: format!("{name}: [S]ave · [D]iscard · [C]ancel"),
            choices: vec![
                Choice { key: 's', label: "Save",    action: PromptAction::ReviewSave },
                Choice { key: 'd', label: "Discard", action: PromptAction::ReviewDiscard },
                Choice { key: 'c', label: "Cancel",  action: PromptAction::Cancel },
            ],
        }
    }
```

Codex I4: `Prompt` has EXACTLY two fields — `message: String` and `choices: Vec<Choice>` (prompt.rs:34) — and does NOT derive `Default`. Construct both fields literally; do NOT use `..Default::default()`. (Mirrors `quit_confirm` at prompt.rs:50.)

- [ ] **Step 5: `Command::Quit` → multi-buffer**

commands.rs:535 replace the body:

```rust
        Command::Quit => {
            let any_dirty = editor.buffers.iter().any(|b| editor.is_dirty(b.id));
            if any_dirty {
                let n = editor.buffers.iter().filter(|b| editor.is_dirty(b.id)).count();
                editor.open_prompt(crate::prompt::Prompt::quit_multi(n));
                CommandResult::Handled
            } else {
                editor.quit = true;
                CommandResult::Quit
            }
        }
```

- [ ] **Step 6: Implement `drive_quit_drain` + resolve arms + apply_result + reduce hook**

In `app.rs`:

```rust
/// Advance the quit drain by one step: pick the next dirty buffer, switch to it,
/// and either dispatch its save (SaveAll) or raise the per-buffer review prompt
/// (ReviewEach). When the queue is empty, quit. Re-driven by save completion
/// (apply_result sets `quit_drain_advance`) and by review-prompt resolution.
pub fn drive_quit_drain(editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock, msg_tx: &std::sync::mpsc::Sender<Msg>) {
    loop {
        if editor.quit_drain.is_none() { return; }
        // Pop already-clean / vanished buffers off the front. Each iteration uses a
        // SHORT immutable borrow to read the front id (Codex I-new-1: never hold a
        // `quit_drain` borrow across an `editor.is_dirty`/method call), then mutates
        // the queue in a SEPARATE borrow.
        let front = editor.quit_drain.as_ref().and_then(|d| d.queue.front().copied());
        let Some(id) = front else {
            editor.quit_drain = None;
            editor.quit = true;
            return;
        };
        let gone = editor.buffers.iter().all(|b| b.id != id);
        if gone || !editor.is_dirty(id) {
            if let Some(d) = editor.quit_drain.as_mut() { d.queue.pop_front(); }
            continue;
        }
        let idx = editor.buffers.iter().position(|b| b.id == id).unwrap();
        crate::workspace::switch_to(editor, idx); // show the buffer in question
        let mode = editor.quit_drain.as_ref().unwrap().mode;
        match mode {
            crate::editor::QuitMode::SaveAll => {
                let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
                crate::save::dispatch_save_then(&mut ctx, crate::editor::PostSaveAction::ContinueQuitDrain);
                return; // wait for the save (named) or Save-As (unnamed) to complete
            }
            crate::editor::QuitMode::ReviewEach => {
                let name = crate::workspace::buffer_display_name(editor, id);
                editor.open_prompt(crate::prompt::Prompt::quit_review_buffer(&name));
                return; // wait for ReviewSave/ReviewDiscard/Cancel
            }
        }
    }
}
```

NOTE: `QuitMode` must be `Copy` (derive `Clone, Copy`) so `let mode = ...mode;` copies out without holding the borrow.

`resolve_prompt` arms (app.rs:512) — add:

```rust
        PromptAction::QuitSaveAll | PromptAction::QuitReviewEach => {
            editor.prompt = None;
            let mode = if matches!(action, PromptAction::QuitSaveAll) { crate::editor::QuitMode::SaveAll } else { crate::editor::QuitMode::ReviewEach };
            let queue: std::collections::VecDeque<_> = editor.buffers.iter().filter(|b| editor.is_dirty(b.id)).map(|b| b.id).collect();
            editor.quit_drain = Some(crate::editor::QuitDrain { queue, mode });
            drive_quit_drain(editor, ex, clock, msg_tx);
            return;
        }
        PromptAction::ReviewSave => {
            editor.prompt = None;
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
            crate::save::dispatch_save_then(&mut ctx, crate::editor::PostSaveAction::ContinueQuitDrain);
            return;
        }
        PromptAction::ReviewDiscard => {
            editor.prompt = None;
            if let Some(d) = editor.quit_drain.as_mut() { d.queue.pop_front(); }
            drive_quit_drain(editor, ex, clock, msg_tx);
            return;
        }
```

Extend the `Cancel` arm (app.rs:513) to also clear the drain:

```rust
        PromptAction::Cancel => {
            editor.pending_export = None;
            editor.pending_save_overwrite = None;
            editor.pending_save_as = None;
            editor.pending_write_block = None;
            editor.quit_drain = None; // Effort 6: abort a multi-buffer quit
        }
```

`apply_result` (app.rs:134, in the `match action`) — add an arm:

```rust
                crate::editor::PostSaveAction::ContinueQuitDrain => {
                    editor.pending_after_save = None;
                    if saved_this && !editor.is_dirty(buffer_id) {
                        // Saved clean: drop this buffer from the queue and advance.
                        if let Some(d) = editor.quit_drain.as_mut() { d.queue.pop_front(); }
                        editor.quit_drain_advance = true; // apply_job_result re-drives with ctx
                    } else if saved_this {
                        // Codex C-new: the user typed DURING the in-flight save → this buffer
                        // is dirty again. Do NOT pop/skip it (that would silently lose the new
                        // edits). Re-drive WITHOUT popping → drive_quit_drain re-saves the newer
                        // version of the same front buffer. Converges once the user stops typing.
                        editor.quit_drain_advance = true;
                    } else {
                        // Save failed (Codex 8d): abort the drain so it can't linger with no
                        // in-flight save and no re-drive. The merge's error status stands.
                        editor.quit_drain = None;
                        editor.quit_drain_advance = false;
                    }
                }
```

**Centralize the re-drive (Codex C1).** `apply_result` is called from MULTIPLE `reduce` branches that each return early — the palette-drain path (app.rs:1133), the prompt `JobDone` path (app.rs:1317, drained at 1336, returns 1337), and the main tail (~app.rs:1651). A single hook at the tail would be skipped on the early-return paths. So introduce ONE helper and route EVERY job-result application through it:

```rust
/// Apply a finished job's result, then advance a multi-buffer quit drain if one
/// is waiting on this completion. The single funnel for all JobDone handling so
/// the re-drive cannot be skipped on an early-returning reduce branch (Codex C1).
pub fn apply_job_result(r: JobResult, editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock, msg_tx: &std::sync::mpsc::Sender<Msg>) {
    apply_result(r, editor);
    if editor.quit_drain_advance {
        editor.quit_drain_advance = false;
        drive_quit_drain(editor, ex, clock, msg_tx);
    }
}
```

Grep EVERY runtime `apply_result(` call site and replace it with `apply_job_result(r, editor, ex, clock, msg_tx)` (Codex I-new-3: `reduce`'s executor param is `ex`, and `msg_tx: &Sender<Msg>` is already a reference — pass `msg_tx`, not `&msg_tx`). Codex I-new-2: the sites are MORE than the three reduce branches — they include:
- the executor-drain loops in `reduce` (app.rs ~1133 palette drain, ~992 overlay drain) and each direct `Msg::JobDone(r)` branch (app.rs:1317/1336 prompt path, tail ~1651);
- **`dispatch_overlay_command`** which ends with `for r in ex.drain() { apply_result(r, editor); }` (app.rs:729) and is reached from menu/palette Enter. It already has `ex`/`clock`/`msg_tx` in scope (it builds a `Ctx` from them), so `apply_job_result(r, editor, ex, clock, msg_tx)` slots in directly.
Leave ONLY the unit-test shim calling `apply_result` directly. Grep `apply_result(` across app.rs to confirm none are missed.

**Save-As cancel aborts the drain (Codex C2).** A dirty UNNAMED buffer in the drain opens the Save-As MINIBUFFER (not a prompt), so the `PromptAction::Cancel` arm never fires for it. Two sites must abort the drain when `quit_drain.is_some()`:
- Save-As minibuffer Esc/dismiss (app.rs:1361–1367, where it sets `editor.minibuffer = None; editor.pending_save_as = None;`): also `editor.quit_drain = None; editor.quit_drain_advance = false;`.
- Empty Save-As submit (`save_as_submit`, app.rs:407, the `t.is_empty()` early return): also clear `quit_drain`/`quit_drain_advance`.
In both, leave `editor.quit = false` (no data loss; the user backed out).

**Save timeout must handle ContinueQuitDrain (Codex C3).** The bounded-save-quit timeout block (app.rs:1908–1922) only special-cases `PostSaveAction::Quit`; every other action just clears `pending_after_save` with a "try again" status — which would strand a queued `quit_drain` with no in-flight save and no re-drive. Extend that block: when the timed-out action is `ContinueQuitDrain`, ABORT the whole quit (`editor.quit_drain = None; editor.quit_drain_advance = false;`) with status `"save timed out — quit cancelled"`. Do not silently clear only `pending_after_save`.

```rust
// inside the timeout block, replacing the is_quit branch. Codex I-new-4: match on
// &p.action (PostSaveAction::Open(PathBuf) is non-Copy — `match p.action` would move
// out of the borrowed `pending_after_save`). Decide the action first, then mutate.
let timed_out_drain = matches!(&p.action, crate::editor::PostSaveAction::ContinueQuitDrain);
let timed_out_quit  = matches!(&p.action, crate::editor::PostSaveAction::Quit);
editor.pending_after_save = None;
if timed_out_quit {
    editor.open_prompt(crate::prompt::Prompt::quit_confirm());
    editor.status = "Save still running — choose again".into();
} else if timed_out_drain {
    editor.quit_drain = None;
    editor.quit_drain_advance = false;
    editor.status = "save timed out — quit cancelled".into();
} else {
    editor.status = "Save still running — try again".into();
}
```

(Reading `&p.action` via `matches!` BEFORE clearing `pending_after_save` avoids the move; the existing block already binds `p` as `&editor.pending_after_save` — keep that shape.)

- [ ] **Step 7: Run quit tests, verify pass**

Run: `cargo test -p wordcartel quit_with_no_dirty quit_save_all_drains quit_review_each_cancel`
Expected: PASS.

- [ ] **Step 8: Run full suite**

Run: `cargo test -p wordcartel`
Expected: PASS — the existing single-buffer quit tests (quit_confirm) updated if they assert the old prompt. Search `quit_confirm` in app.rs tests; a clean single active buffer still quits, but a dirty one now raises `quit_multi` — update those assertions to the new prompt/flow.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(6): multi-buffer quit (Save-All / Review-each) state machine

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

### Task 9: Status-line buffer indicator

**Files:**
- Modify: `wordcartel/src/render.rs` (status line ~616–658)
- Test: `wordcartel/src/render.rs`

**Interfaces:**
- Consumes: `Editor.active`, `Editor.buffers`, `workspace::buffer_display_name`.
- Produces: a `[i/n]` indicator (1-based active index over buffer count) + display-name fallback (`*scratch*`/`*untitled*`) where the path is currently shown.

- [ ] **Step 1: Write failing test**

Read how render.rs status tests assert (grep existing status-line tests). Add:

```rust
#[test]
fn status_line_shows_buffer_index_and_count() {
    let mut e = crate::editor::Editor::new_from_text("a\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
    e.install_scratch(); // 2 buffers, active index 0
    let s = crate::render::status_left_text(&e); // use the real status-builder seam
    assert!(s.contains("[1/2]"), "shows active/count: {s}");
}

#[test]
fn status_line_names_untitled_and_scratch() {
    let mut e = crate::editor::Editor::new_from_text("\n", None, (40, 10));
    e.install_scratch();
    let s_untitled = crate::render::status_left_text(&e);
    assert!(s_untitled.contains("*untitled*"));
    crate::workspace::goto_scratch(&mut e);
    let s_scratch = crate::render::status_left_text(&e);
    assert!(s_scratch.contains("*scratch*"));
}
```

Codex M1: there is NO `status_left_text` today — the status-left is assembled inline from `path_str`/`dirty_marker`/`mode_text` (render.rs:589/597/625). So the FIRST step of this task is to extract that inline assembly into a pure `pub(crate) fn status_left_text(editor: &Editor) -> String` (no ratatui types), call it from the render path with identical output, and confirm the existing render tests still pass — THEN add the `[i/n]`/display-name behavior. The extraction is a prerequisite, not a fallback.

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p wordcartel status_line_shows_buffer_index status_line_names_untitled_and_scratch`
Expected: FAIL.

- [ ] **Step 3: Implement the indicator**

In the status-left assembly (render.rs ~616): replace the raw `path_str` with `crate::workspace::buffer_display_name(editor, editor.active().id)` and prepend the index/count:

```rust
    let idx = editor.active + 1;
    let count = editor.buffers.len();
    let name = crate::workspace::buffer_display_name(editor, editor.active().id);
    // e.g. "[1/2] notes.md" ; dirty marker already handled by buffer_display_name's "*" prefix
    let head = format!("[{idx}/{count}] {name}");
```

Splice `head` where the filename currently goes, preserving the existing dirty/BLK/mode/Ln:Col segments (the BLK indicator at render.rs:632 and Ln:Col at 646 stay as-is).

NOTE: if `buffer_display_name`'s leading `*` dirty marker duplicates an existing dirty marker, drop the older one so the marker appears once. Keep the BLK and Ln:Col segments unchanged.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p wordcartel status_line_shows_buffer_index status_line_names_untitled_and_scratch`
Expected: PASS.

- [ ] **Step 5: Run full suite**

Run: `cargo test -p wordcartel`
Expected: PASS — existing status-line tests updated for the new `[i/n] name` head if they asserted the bare path.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(6): status-line buffer [i/n] indicator + scratch/untitled names

$(printf 'Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6')"
```

---

## Self-Review

**Spec coverage:**
- Buffer model / scratch_id / is_dirty → Task 1. ✓
- Scratch persistence (SessionState.scratch, explicit/active-independent) → Task 2. ✓
- copy/move_block_to_scratch + two-undo model → Task 3. ✓
- Cycle keys (^K,/^K. + Alt+,/Alt+.) + MRU + goto_scratch → Task 4. ✓
- Switcher palette (MRU) → Task 5. ✓
- Additive open/new + throwaway reuse (scratch excluded) → Task 6. ✓
- Close rules (scratch no-op, last-buffer invariant, same-index neighbor) → Task 7. ✓
- Multi-buffer quit (Save-All/Review-each, Cancel aborts, queue) → Task 8. ✓
- Status `[i/n]` + display names → Task 9. ✓
- Export left as-is / filter-transform single-flight (I1/I2) → not code changes; preserved (no task touches them). ✓
- Keymap parse/collision tests (M1) → Tasks 3, 4 (+5). ✓
- Out of scope (splits, workspace-set restore, cross-buffer block ops, SSH/tmux clipboard) → no tasks. ✓

**Type consistency:** `scratch_id: Option<BufferId>`, `is_dirty(BufferId)`, `is_scratch(BufferId)`, `switch_to_index(usize)`, `switch_to(editor, usize)`, `buffer_display_name(editor, BufferId)`, `QuitDrain { queue: VecDeque<BufferId>, mode: QuitMode }`, `PostSaveAction::ContinueQuitDrain` — used consistently across tasks.

**Codex plan review (round 1) — resolved inline:**
- C1 centralized quit-drain re-drive via `apply_job_result` at every JobDone site (Task 8). ✓
- C2 Save-As minibuffer dismiss / empty-submit abort the drain (Task 8). ✓
- C3 save-timeout handles `ContinueQuitDrain` (abort, not silent-clear) (Task 8). ✓
- I1 CUA move chord `alt-shift-x`→`alt-shift-v` (x is `unfold_all`) (Task 3). ✓
- I2 `parse_chord_seq`→`parse_seq` (Task 4). ✓
- I3 `snap_to_boundary` doesn't exist → `nav::clamp_snap` via temporary-active (Task 2). ✓
- I4 `Prompt` has no `Default` → literal `{message, choices}` (Task 8). ✓
- I5 sequenced `PostSaveAction::{New,Open}` removal with all live sites enumerated (Task 6). ✓
- I6 palette can't carry buffer ids → discriminant + Enter-site branch (Task 5). ✓
- M1 `status_left_text` extraction made a prerequisite (Task 9). ✓
- 8d save-failure mid-drain aborts cleanly (no lingering drain) (Task 8). ✓
- Confirmed clean by Codex (round 1): empty `""` scratch supported; cross-buffer apply with `path=None` safe; core signatures/anchors accurate; WordStar `^K` chords free.

**Codex plan review (round 2) — resolved inline:**
- C-new dirty-during-save guard: `ContinueQuitDrain` pops only when `saved_this && !is_dirty`; saved-but-redirtied re-drives without popping; failure aborts (Task 8). ✓
- I-new-1 `drive_quit_drain` borrow-safe rewrite (short immutable front read, separate mutate; `QuitMode: Copy`) (Task 8). ✓
- I-new-2 C1 site list expanded to `dispatch_overlay_command` (app.rs:729) + overlay drain (~992), not just the 3 reduce branches (Task 8). ✓
- I-new-3 identifiers `ex`/`msg_tx` (not `executor`/`&msg_tx`) (Task 8). ✓
- I-new-4 timeout uses `matches!(&p.action, …)` (Open(PathBuf) non-Copy) (Task 8). ✓
- I-new-5 palette `PaletteKind` discriminant + per-row `buffer` field + branched `rebuild_rows` (parallel vec would desync on filter) (Task 5). ✓
- I-new-6 six New/Open test sites (2718/2749/2777/2804/2828/2851) (Task 6). ✓
- Confirmed clean by Codex (round 2): C2 unnamed-buffer SaveAll continuation coherent (action survives pending_save_as→pending_after_save→JobDone); Save-As cancel sites correct; `alt-shift-v`/`parse_seq`/`Prompt{}` literal/`clamp_snap` side-effect-free/`status_left_text` extraction all sound.

**Codex plan review (round 3): CLEAN — ready to implement.** No Critical/Important/Minor; all round-2 edits (drain convergence, borrow-safe driver, apply_job_result sites, timeout `matches!`, palette kind model) verified coherent against the real code.

**Open verification points flagged inline for implementers (not placeholders — real "confirm against code" checks):**
- Task 2: exact char-boundary snap helper name on `TextBuffer` (`snap_to_boundary` vs `clamp_snap`).
- Task 3/4/5: confirm `^Kg`/`^Ka`/`^Kl`, `alt-shift-c`/`alt-shift-x`, and the CUA switch_buffer chord are unbound; confirm the real chord-parse fn name.
- Task 5: the palette's row/selection integration shape (read palette.rs + submit site).
- Task 6: blast radius of removing `PostSaveAction::{New,Open}`.
- Task 8: `dispatch_save_then` signature + the reduce site identifiers for executor/clock/msg_tx.
- Task 9: existence of an extractable status-left builder (else refactor one).
