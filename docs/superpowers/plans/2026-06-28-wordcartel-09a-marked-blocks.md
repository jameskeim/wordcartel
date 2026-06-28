# Persistent Marked Blocks (Effort 9A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the WordStar "mark now, act later" primitive — a single persistent, edit-tracking marked block per buffer, separate from the live selection, acted on later from anywhere (copy/move/delete/write-to-file/jump/hide), with a distinct §13.2 cue, persisted across sessions.

**Architecture:** Shell-side block state on `Buffer` (`marked_block`/`pending_block_begin`), edit-tracked by the existing `Buffer::apply` map loop; commands in a new `blocks_marked.rs` registered through the §10.4 registry; all text mutations via `editor.apply` (one undo step each). `^KW` reuses Effort 7's Save-As infra. Persistence joins `state.rs` via Effort 7's `restore_resume`. The one `wordcartel-core` change is `SemanticElement::MarkedBlock`. Spec: `docs/superpowers/specs/2026-06-28-wordcartel-09a-marked-blocks-design.md`.

**Tech Stack:** Rust, ratatui 0.30, crossterm.

## Global Constraints

- **Single** marked block per buffer; **separate** from `document.selection`.
- Block ops register in the registry (palette-reachable in every preset). **WordStar** preset binds `^K`/`^Q` (reclaiming `^KC`/`^KV` from 9B interim); **CUA** binds only `alt-b`→promote.
- **Edit-tracking:** map `marked_block.start` via `change::map_pos`, `marked_block.end` and `pending_block_begin` via `change::map_pos_before`; **collapse → clear** (`start == end` → `None`).
- **Undo/redo CLEAR the block** (they bypass `apply`; acting on stale offsets is unsafe).
- All block text edits go through `editor.apply` (undo + edit-tracking). `block_move` uses one `commands::build_multi_replace` ChangeSet (one undo step).
- **§13.2:** `MarkedBlock` mono modifier = **`reverse + bold + underline`** (the only free distinct combo — see spec §7); added to the a11y pairwise-distinct test.
- **`BLK` status indicator must NOT ride the word-count toggle** (left status segment / independent builder).
- TDD, frequent commits. Every commit ends with the trailers:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` / `Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6`

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `wordcartel/src/editor.rs` | `MarkedBlock` struct + `marked_block`/`pending_block_begin` fields; map in `apply` (start=`map_pos`, end/pending=`map_pos_before`, collapse→clear); clear on `undo`/`redo`; `from_text` init | 1 |
| `wordcartel/src/blocks_marked.rs` (new) | all `block_*` command bodies (begin/end/promote/copy/move/delete/jump/toggle-hidden/clear/write); `lib.rs` `pub mod blocks_marked` | 2,3,4 |
| `wordcartel/src/registry.rs` | register `block_*` ids (Edit) | 2,3,4 |
| `wordcartel/src/minibuffer.rs` | `MinibufferKind::WriteBlock` | 4 |
| `wordcartel/src/prompt.rs` | `PromptAction::OverwriteWriteBlock` + `Prompt::write_block_overwrite` | 4 |
| `wordcartel/src/app.rs` | `expand_path` factor; `WriteBlock` submit routing + `OverwriteWriteBlock` arm (`pending_write_block`); persist block in `persist_session`; restore in `restore_resume` | 4,5 |
| `wordcartel/src/state.rs` | `StateEntry.block: Option<(usize,usize)>` (serde default) | 5 |
| `wordcartel-core/src/theme.rs` | `SemanticElement::MarkedBlock` + `ThemeFaces.marked_block` across all touchpoints; mono=`reverse+bold+underline`; a11y test | 6 |
| `wordcartel/src/render.rs` | paint `MarkedBlock` (placed path, below Selection, fold-safe, skip hidden); `BLK`/`BLK·hidden` status segment | 6 |
| `wordcartel/src/keymap.rs` | WORDSTAR block binds (reclaim `^KC`/`^KV`); CUA `alt-b`→promote; update `wordstar_new_chords` assertions | 7 |

**Task order: 1 → 2 → 3 → 4 → 5 → 6 → 7.** Task 7 (keymap) is last — every `block_*` id must be registered first.

---

## Task 1: Block state + edit-tracking + undo-clear

**Files:**
- Modify: `wordcartel/src/editor.rs` (`MarkedBlock` + fields; `apply` mapping; `undo`/`redo` clear; `from_text` init)
- Test: `wordcartel/src/editor.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `pub struct MarkedBlock { pub start: usize, pub end: usize, pub hidden: bool }`; `Buffer.marked_block: Option<MarkedBlock>`; `Buffer.pending_block_begin: Option<usize>`.
- Consumes: `change::map_pos`, `change::map_pos_before` (already used in `apply`).

- [ ] **Step 1: Write the failing tests**

```rust
    // Editor has NO insert/delete helpers (Codex) — drive edits through apply, building the
    // changeset with the existing build_multi_replace. The editor.rs test module's TestClock
    // is `Cell<u64>`-based (editor.rs:522): `TestClock(std::cell::Cell::new(0))` — Codex.
    fn ap(e: &mut Editor, edits: &[(usize, usize, &str)]) {
        let doc_len = e.active().document.buffer.len();
        let owned: Vec<(usize, usize, String)> = edits.iter().map(|(a,b,s)| (*a,*b,s.to_string())).collect();
        let (cs, edit) = crate::commands::build_multi_replace(&owned, doc_len);
        let txn = wordcartel_core::history::Transaction::new(cs);
        e.apply(txn, edit, wordcartel_core::history::EditKind::Other, &TestClock(std::cell::Cell::new(0)));
    }

    #[test]
    fn marked_block_tracks_edits_and_collapses() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 6, end: 11, hidden: false }); // "world"
        ap(&mut e, &[(0, 0, "XX")]); // insert "XX" at byte 0 → block shifts right by 2
        let b = e.active().marked_block.unwrap();
        assert_eq!((b.start, b.end), (8, 13));
        let len = e.active().document.buffer.len();
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: len, hidden: false });
        ap(&mut e, &[(0, len, "")]); // delete the whole region → collapse → cleared
        assert!(e.active().marked_block.is_none(), "fully-deleted block clears");
    }

    #[test]
    fn marked_block_boundary_inserts_stay_outside() {
        let mut e = Editor::new_from_text("ab cd\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 3, end: 5, hidden: false }); // "cd"
        ap(&mut e, &[(5, 5, "X")]); // insert at end → end stays (map_pos_before), block does NOT grow
        assert_eq!(e.active().marked_block.unwrap().end, 5);
        ap(&mut e, &[(3, 3, "Y")]); // insert at start → start moves past (map_pos), block does NOT grow at front
        assert_eq!(e.active().marked_block.unwrap().start, 4);
    }

    #[test]
    fn undo_clears_marked_block() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        ap(&mut e, &[(0, 0, "Z")]); // make history non-empty
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 2, hidden: false });
        e.undo(); // Editor::undo exists (editor.rs:512)
        assert!(e.active().marked_block.is_none(), "undo clears the block (it bypasses apply mapping)");
    }
```

> Confirm `TestClock` (or the real clock type) the editor.rs `#[cfg(test)]` module uses for `apply`-based tests (e.g. `undo_redo_round_trip`), and that `Editor::undo()` returns `bool`. The block-mutation in `apply`/`undo` must compile (mutating `self.marked_block` after the existing marks/ring/fold loops — no overlapping borrow).

- [ ] **Step 2: Run — fails** (`MarkedBlock` undefined). `cargo test -p wordcartel marked_block`

- [ ] **Step 3: Implement.** In `editor.rs`:
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkedBlock { pub start: usize, pub end: usize, pub hidden: bool }
// in Buffer struct: pub marked_block: Option<MarkedBlock>, pub pending_block_begin: Option<usize>,
```
Add `marked_block: None, pending_block_begin: None,` to `Buffer::from_text`. In `Buffer::apply`, after the existing marks/ring/folds mapping (editor.rs ~162-176), add:
```rust
        // 9A: the marked block follows the text. start uses map_pos, end + pending use
        // map_pos_before → boundary inserts stay outside the half-open [start,end).
        self.pending_block_begin = self.pending_block_begin
            .map(|p| wordcartel_core::change::map_pos_before(p, &cs));
        if let Some(b) = self.marked_block.as_mut() {
            b.start = wordcartel_core::change::map_pos(b.start, &cs);
            b.end   = wordcartel_core::change::map_pos_before(b.end, &cs);
        }
        if self.marked_block.map_or(false, |b| b.start >= b.end) {
            self.marked_block = None; // collapsed → clear
        }
```
In `Buffer::undo` and `Buffer::redo` (editor.rs ~180/196), on a successful undo/redo, clear the block:
```rust
        // 9A: undo/redo bypass apply's mapping → clear the block (acting on stale offsets unsafe).
        self.marked_block = None;
        self.pending_block_begin = None;
```
(Place inside the success branch, after the buffer/selection are restored.)

- [ ] **Step 4: Run** `cargo test -p wordcartel marked_block` + `cargo test -p wordcartel undo_clears` + `cargo test -p wordcartel --lib` — green.

- [ ] **Step 5: Commit** `feat(9a): MarkedBlock state + edit-tracking (map_pos/before, collapse-clear) + undo-clear`

---

## Task 2: Creation — begin / end / promote

**Files:**
- Create: `wordcartel/src/blocks_marked.rs`; Modify `wordcartel/src/lib.rs` (`pub mod blocks_marked`)
- Modify: `wordcartel/src/registry.rs` (register `block_begin`/`block_end`/`mark_block_from_selection`)
- Test: `wordcartel/src/blocks_marked.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `block_begin(editor)`, `block_end(editor)`, `mark_block_from_selection(editor)`; registry ids `block_begin`, `block_end`, `mark_block_from_selection`.
- Consumes: `nav::head`, `editor.active().document.selection.primary().{from,to}`, `Selection::single`, `MarkedBlock`.

- [ ] **Step 1: Write the failing tests**

```rust
    use crate::editor::{Editor, MarkedBlock};
    #[test]
    fn begin_then_end_forms_normalized_block() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(11);
        crate::blocks_marked::block_begin(&mut e); // pending at 11
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(6);
        crate::blocks_marked::block_end(&mut e);    // end at 6 → normalize (6,11)
        assert_eq!(e.active().marked_block, Some(MarkedBlock { start: 6, end: 11, hidden: false }));
        assert!(e.active().pending_block_begin.is_none());
    }

    #[test]
    fn end_without_begin_is_noop() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::blocks_marked::block_end(&mut e);
        assert!(e.active().marked_block.is_none());
        assert_eq!(e.status, "set block begin first");
    }

    #[test]
    fn empty_block_rejected() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        crate::blocks_marked::block_begin(&mut e);
        crate::blocks_marked::block_end(&mut e); // begin==end==2 → reject
        assert!(e.active().marked_block.is_none());
        assert_eq!(e.status, "empty block");
    }

    #[test]
    fn promote_sets_block_and_clears_selection() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5); // "hello"
        crate::blocks_marked::mark_block_from_selection(&mut e);
        assert_eq!(e.active().marked_block, Some(MarkedBlock { start: 0, end: 5, hidden: false }));
        assert!(e.active().document.selection.primary().is_empty(), "selection converted → cleared");
    }
```

- [ ] **Step 2: Run — fails** (`blocks_marked` undefined). `cargo test -p wordcartel block_begin` etc.

- [ ] **Step 3: Implement** `blocks_marked.rs`:
```rust
use crate::editor::{Editor, MarkedBlock};
use crate::nav;

pub fn block_begin(editor: &mut Editor) {
    let at = nav::head(editor);
    editor.active_mut().pending_block_begin = Some(at);
    editor.status = "block begin set".into();
}

pub fn block_end(editor: &mut Editor) {
    let Some(begin) = editor.active().pending_block_begin else {
        editor.status = "set block begin first".into(); return;
    };
    let end = nav::head(editor);
    set_block(editor, begin, end);
    editor.active_mut().pending_block_begin = None;
}

pub fn mark_block_from_selection(editor: &mut Editor) {
    let sel = editor.active().document.selection.primary();
    let (from, to) = (sel.from(), sel.to());
    if from == to { editor.status = "no selection to mark".into(); return; }
    let caret = nav::head(editor);
    set_block(editor, from, to);
    editor.active_mut().pending_block_begin = None;
    // convert: clear the live selection
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(caret);
}

/// Normalize + reject empty; sets marked_block or a status.
fn set_block(editor: &mut Editor, a: usize, b: usize) {
    let (start, end) = (a.min(b), a.max(b));
    if start == end { editor.status = "empty block".into(); return; }
    editor.active_mut().marked_block = Some(MarkedBlock { start, end, hidden: false });
    editor.status = "block marked".into();
}
```
Register in `registry.rs` (Edit menu): `block_begin`, `block_end`, `mark_block_from_selection` → `|c| { crate::blocks_marked::block_begin(c.editor); CommandResult::Handled }` (etc.).

- [ ] **Step 4: Run** `cargo test -p wordcartel block_begin block_end empty_block promote` (focused names) + `cargo test -p wordcartel --lib` — green.

- [ ] **Step 5: Commit** `feat(9a): block create — ^KB/^KK markers + promote-selection (empty rejected)`

---

## Task 3: Operations — copy / move / delete / jump / hide / clear

**Files:**
- Modify: `wordcartel/src/blocks_marked.rs` (the act-on-block ops); `wordcartel/src/registry.rs` (register)
- Test: `wordcartel/src/blocks_marked.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `block_copy`/`block_move`/`block_delete`/`block_jump_begin`/`block_jump_end`/`block_toggle_hidden`/`block_clear` (each `fn(&mut Editor, &dyn Clock)` where an edit is involved; jumps/toggle/clear take `&mut Editor`).
- Consumes: `buffer.slice`, `commands::build_multi_replace`, `Transaction::new(..).with_selection`, `editor.apply`, `EditKind::Other`, `derive::rebuild`, `nav::{head, ensure_visible, clamp_snap}`, `marks::record_jump`, `registry::{place_caret_visible, CaretPlace::UnfoldTo}`.

- [ ] **Step 1: Write the failing tests**

```rust
    // blocks_marked.rs is a NEW module — define a local clock (Codex); the block ops that edit
    // take `&dyn wordcartel_core::history::Clock`.
    struct TestClock(u64);
    impl wordcartel_core::history::Clock for TestClock { fn now_ms(&self) -> u64 { self.0 } }

    #[test]
    fn block_copy_inserts_at_caret_and_keeps_block() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false }); // "hello"
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(11); // before "\n"
        crate::blocks_marked::block_copy(&mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "hello worldhello\n");
        assert!(e.active().marked_block.is_some(), "block stays after copy");
        assert_eq!(e.active().document.selection.primary().head, 16, "caret at end of inserted text");
    }

    #[test]
    fn block_move_relocates_and_clears_one_undo() {
        let mut e = Editor::new_from_text("AAA BBB\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false }); // "AAA "
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(7); // end (before \n)
        crate::blocks_marked::block_move(&mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "BBBAAA \n"); // "AAA " moved to caret
        assert!(e.active().marked_block.is_none(), "block consumed by move");
        let before = e.active().document.buffer.to_string();
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "AAA BBB\n", "one undo step restores");
        let _ = before;
    }

    #[test]
    fn block_move_into_itself_is_noop() {
        let mut e = Editor::new_from_text("hello\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false });
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2); // inside
        crate::blocks_marked::block_move(&mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "hello\n");
        assert_eq!(e.status, "can't move a block into itself");
    }

    #[test]
    fn block_delete_removes_and_clears() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 5, end: 11, hidden: false }); // " world"
        crate::blocks_marked::block_delete(&mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "hello\n");
        assert!(e.active().marked_block.is_none());
    }

    #[test]
    fn ops_with_no_block_status() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::blocks_marked::block_copy(&mut e, &TestClock(0));
        assert_eq!(e.status, "no marked block");
    }
```

- [ ] **Step 2: Run — fails.** `cargo test -p wordcartel block_copy block_move block_delete`

- [ ] **Step 3: Implement** in `blocks_marked.rs`:
```rust
use wordcartel_core::history::Clock;
fn block(editor: &Editor) -> Option<crate::editor::MarkedBlock> { editor.active().marked_block }

pub fn block_copy(editor: &mut Editor, clock: &dyn Clock) {
    let Some(b) = block(editor) else { editor.status = "no marked block".into(); return; };
    let text = editor.active().document.buffer.slice(b.start..b.end);
    let caret = nav::head(editor);
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = crate::commands::build_multi_replace(&[(caret, caret, text.to_string())], doc_len);
    let new_caret = caret + text.len();
    apply_edit(editor, cs, edit, new_caret, clock);
    // block stays — its endpoints map through the insertion via apply.
    editor.status = "block copied".into();
}

pub fn block_move(editor: &mut Editor, clock: &dyn Clock) {
    let Some(b) = block(editor) else { editor.status = "no marked block".into(); return; };
    let caret = nav::head(editor);
    if caret >= b.start && caret < b.end {
        editor.status = "can't move a block into itself".into(); return;
    }
    let text = editor.active().document.buffer.slice(b.start..b.end).to_string();
    let doc_len = editor.active().document.buffer.len();
    // ascending, non-overlapping edits (build_multi_replace requires order)
    let (edits, new_caret) = if caret < b.start {
        (vec![(caret, caret, text.clone()), (b.start, b.end, String::new())], caret + text.len())
    } else { // caret >= b.end (inside guarded above)
        (vec![(b.start, b.end, String::new()), (caret, caret, text.clone())], caret - (b.end - b.start) + text.len())
    };
    let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
    apply_edit(editor, cs, edit, new_caret, clock);
    editor.active_mut().marked_block = None; // consumed
    editor.status = "block moved".into();
}

pub fn block_delete(editor: &mut Editor, clock: &dyn Clock) {
    let Some(b) = block(editor) else { editor.status = "no marked block".into(); return; };
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = crate::commands::build_multi_replace(&[(b.start, b.end, String::new())], doc_len);
    apply_edit(editor, cs, edit, b.start, clock);
    editor.active_mut().marked_block = None;
    editor.status = "block deleted".into();
}

fn apply_edit(editor: &mut Editor, cs: wordcartel_core::change::ChangeSet,
              edit: wordcartel_core::block_tree::Edit, new_caret: usize, clock: &dyn Clock) {
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(new_caret));
    editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock); // Codex: EditKind is in history, not crate::editor
    crate::derive::rebuild(editor);
    nav::ensure_visible(editor);
    editor.active_mut().desired_col = None;
}

pub fn block_jump_begin(editor: &mut Editor) { block_jump(editor, true); }
pub fn block_jump_end(editor: &mut Editor)   { block_jump(editor, false); }
fn block_jump(editor: &mut Editor, to_start: bool) {
    let Some(b) = block(editor) else { editor.status = "no marked block".into(); return; };
    let target = if to_start { b.start } else { b.end };
    let pre = nav::head(editor);
    crate::marks::record_jump(editor.active_mut(), pre);
    let off = nav::clamp_snap(editor, target);
    let off = crate::registry::place_caret_visible(editor, off, crate::registry::CaretPlace::UnfoldTo);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
    crate::derive::rebuild(editor);
    nav::ensure_visible(editor);
}

pub fn block_toggle_hidden(editor: &mut Editor) {
    match editor.active_mut().marked_block.as_mut() {
        Some(b) => { b.hidden = !b.hidden; let h = b.hidden; editor.status = if h { "block hidden".into() } else { "block shown".into() }; }
        None => editor.status = "no marked block".into(),
    }
}
pub fn block_clear(editor: &mut Editor) {
    editor.active_mut().marked_block = None;
    editor.active_mut().pending_block_begin = None;
    editor.status = "block cleared".into();
}
```
> Confirm the real `Transaction`/`EditKind`/`slice` (returns `String`) APIs against the `DeleteWord` arm (commands.rs:537) and `build_multi_replace`'s return; adapt the `apply_edit` helper to match exactly. Register all seven ids in `registry.rs` (Edit). Jump/toggle/clear handlers take only `c.editor`; copy/move/delete pass `c.clock`.

- [ ] **Step 4: Run** `cargo test -p wordcartel block_` + `cargo test -p wordcartel --lib` — green.

- [ ] **Step 5: Commit** `feat(9a): block ops — copy/move/delete (build_multi_replace, one undo) + jump/hide/clear`

---

## Task 4: `^KW` write block → file

**Files:**
- Modify: `wordcartel/src/app.rs` (`expand_path` factor from `save_as_submit`; `block_write` open + submit + `OverwriteWriteBlock` arm; minibuffer routing); `wordcartel/src/minibuffer.rs` (`WriteBlock`); `wordcartel/src/prompt.rs` (`OverwriteWriteBlock` + `write_block_overwrite`); `wordcartel/src/editor.rs` (`pending_write_block` field); `wordcartel/src/blocks_marked.rs` (`block_write` opens the prompt); `wordcartel/src/registry.rs` (register `block_write`)
- Test: `wordcartel/src/app.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `MinibufferKind::WriteBlock`; `PromptAction::OverwriteWriteBlock`; `Editor.pending_write_block: Option<PathBuf>`; `app::block_write_submit`; `app::expand_path`.
- Consumes: `file::save_atomic`, `buffer.slice`, the Save-As minibuffer/resolve_prompt pattern.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn block_write_writes_block_text_only_doc_unchanged() {
        use crate::editor::Editor;
        let p = std::env::temp_dir().join(format!("wc-blkw-{}.md", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false }); // "hello"
        let before_doc = e.active().document.buffer.to_string();
        crate::app::block_write_submit(&mut e, p.to_str().unwrap());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello", "block text written");
        assert_eq!(e.active().document.buffer.to_string(), before_doc, "document unchanged");
        assert!(e.active().marked_block.is_some(), "block stays after write");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn block_write_existing_target_raises_overwrite() {
        use crate::editor::Editor;
        let p = std::env::temp_dir().join(format!("wc-blkw-ow-{}.md", std::process::id()));
        std::fs::write(&p, "old").unwrap();
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 3, hidden: false });
        crate::app::block_write_submit(&mut e, p.to_str().unwrap());
        assert_eq!(e.prompt.as_ref().unwrap().action_for('o'), Some(crate::prompt::PromptAction::OverwriteWriteBlock));
        let _ = std::fs::remove_file(&p);
    }
```

- [ ] **Step 2: Run — fails.** `cargo test -p wordcartel block_write`

- [ ] **Step 3: Implement.** Factor `pub fn expand_path(text: &str) -> std::path::PathBuf` in `app.rs` from `save_as_submit`'s inline `~`/relative logic; make `save_as_submit` call it (no behavior change). Add `pending_write_block: Option<PathBuf>` to `Editor` (init None). `minibuffer.rs`: `WriteBlock` variant. `prompt.rs`: `OverwriteWriteBlock` + `Prompt::write_block_overwrite(target)` (mirror `save_overwrite`). `blocks_marked.rs`: `block_write` opens the prompt:
```rust
pub fn block_write(editor: &mut Editor) {
    if editor.active().marked_block.is_none() { editor.status = "no marked block".into(); return; }
    let pre = editor.active().document.path.as_ref()
        .and_then(|p| p.parent()).map(|d| format!("{}/", d.display())).unwrap_or_default();
    editor.open_minibuffer("Write block to: ", crate::minibuffer::MinibufferKind::WriteBlock);
    if let Some(mb) = editor.minibuffer.as_mut() { mb.cursor = pre.len(); mb.text = pre; }
}
```
`app.rs`:
```rust
pub fn block_write_submit(editor: &mut crate::editor::Editor, text: &str) {
    let Some(b) = editor.active().marked_block else { editor.status = "no marked block".into(); return; };
    let t = text.trim();
    if t.is_empty() { editor.status = "write block: empty path".into(); return; }
    let target = expand_path(t);
    if target.exists() { editor.pending_write_block = Some(target.clone());
        editor.open_prompt(crate::prompt::Prompt::write_block_overwrite(&target)); return; }
    perform_block_write(editor, &target, b.start, b.end);
}
fn perform_block_write(editor: &mut crate::editor::Editor, target: &std::path::Path, start: usize, end: usize) {
    let text = editor.active().document.buffer.slice(start..end).to_string();
    match crate::file::save_atomic(target, &text) {
        Ok(_)  => editor.status = format!("wrote block to {}", target.display()),
        Err(e) => editor.status = e.to_string(),
    }
}
```
Route the minibuffer `Enter` (app.rs ~1003): `MinibufferKind::WriteBlock => block_write_submit(editor, &mb.text)`. `resolve_prompt` arm: `OverwriteWriteBlock => { if let Some(t) = editor.pending_write_block.take() { if let Some(b) = editor.active().marked_block { perform_block_write(editor, &t, b.start, b.end); } } }`. **Clear `pending_write_block` on EVERY dismiss (Codex):** `PromptAction::Cancel` clears it, AND the **raw-`Esc` modal branch** (which today directly clears `pending_export`/`pending_save_overwrite`/`pending_save_as`) must also set `editor.pending_write_block = None` — else it stays armed after Esc. Register `block_write` (File) → `block_write`. cua/WordStar bind deferred to Task 7.

- [ ] **Step 4: Run** `cargo test -p wordcartel block_write` + `cargo test -p wordcartel save_as` (expand_path factor didn't regress) + `cargo test -p wordcartel --lib` — green.

- [ ] **Step 5: Commit** `feat(9a): ^KW block_write (Save-As-style prompt, OverwriteWriteBlock, doc untouched)`

---

## Task 5: Persistence across sessions

**Files:**
- Modify: `wordcartel/src/state.rs` (`StateEntry.block`); `wordcartel/src/app.rs` (`persist_session` records block; `restore_resume` restores it)
- Test: `wordcartel/src/app.rs` / `state.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `StateEntry.block: Option<(usize, usize)>`.
- Consumes: `persist_session`, `restore_resume`, `file_identity` staleness guard.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn marked_block_persists_and_restores_under_matching_identity() {
        // Mirror an existing restore_resume test: write a file, persist a session entry with a
        // block, then open the SAME unchanged file and assert the block restores; then change the
        // file and assert the block (and entry) is discarded.
        // (Adapt to the real restore_resume/persist_session test harness — see app.rs ~3459.)
    }
```

> Use the real persistence-test harness in `app.rs`/`state.rs` (the existing resume/marks-restore tests). The assertions are the contract: a block round-trips under a matching mtime+size; a mismatch discards it; `hidden` is `false` on restore.

- [ ] **Step 2: Run — fails** (no `block` field). `cargo test -p wordcartel marked_block_persists`

- [ ] **Step 3: Implement.** `state.rs`: add `#[serde(default)] pub block: Option<(usize, usize)>,` to `StateEntry`. Update EVERY `StateEntry { … }` literal in tests (app.rs ~3459/3562, state.rs tests) with `block: None`. In `persist_session` (app.rs:1987), add to the entry: `block: editor.active().marked_block.map(|b| (b.start, b.end)),`. In `restore_resume` (app.rs:315, the accepted-entry branch after folds): 
```rust
                    if let Some((s, en)) = entry.block {
                        editor.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: s, end: en, hidden: false });
                    }
```

- [ ] **Step 4: Run** `cargo test -p wordcartel marked_block_persists` + `cargo test -p wordcartel state` + `cargo test -p wordcartel --lib` — green (existing session.toml loads via serde default).

- [ ] **Step 5: Commit** `feat(9a): persist the marked block across sessions (state.rs + restore_resume, staleness-guarded)`

---

## Task 6: Theme cue (`MarkedBlock`) + render paint + `BLK` indicator

**Files:**
- Modify: `wordcartel-core/src/theme.rs` (`SemanticElement::MarkedBlock` + `ThemeFaces.marked_block` everywhere; a11y test)
- Modify: `wordcartel/src/render.rs` (paint the block; `BLK` status segment)
- Test: `wordcartel-core/src/theme.rs` + `wordcartel/src/render.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `SemanticElement::MarkedBlock`; `ThemeFaces.marked_block`.
- Consumes: the placed-path compose layering; the status-line assembly.

- [ ] **Step 1: Write the failing tests**

```rust
    // theme.rs: MarkedBlock has a distinct mono modifier (reverse+bold+underline) and is in ALL_ELEMENTS.
    #[test]
    fn marked_block_mono_modifier_is_distinct() {
        let t = no_color();
        let mb = t.face(SemanticElement::MarkedBlock);
        assert_eq!((mb.reverse, mb.bold, mb.underline), (Some(true), Some(true), Some(true)));
        // distinct from selection (reverse+underline), search_current (bold+reverse), diag_spelling (bold+underline)
        assert_ne!(mb, t.face(SemanticElement::Selection));
    }
```
And in `render.rs`:
```rust
    #[test]
    fn marked_block_paints_and_status_shows_blk() {
        let mut e = Editor::new_from_text("hello world\n", None, (60, 6));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false });
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 60, 6);
        // the block cells carry a non-default style distinct from unselected cells (assert a modifier present)
        // and the status row contains "BLK"
        assert!(row_string(&buf, 5).contains("BLK"), "status shows BLK indicator");
    }

    #[test]
    fn hidden_block_status_reads_blk_hidden_and_not_painted() {
        let mut e = Editor::new_from_text("hello\n", None, (60, 6));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: true });
        crate::derive::rebuild(&mut e);
        assert!(row_string(&render_to_buffer(&mut e, 60, 6), 5).contains("BLK·hidden"));
    }
```

> Use the real render test helpers (`render_to_buffer`/`row_string`); adapt the painted-cell assertion to how the existing selection-paint test asserts a styled cell (e.g. `matches!(cell.modifier …)` or the placed-style check).

- [ ] **Step 2: Run — fails** (`MarkedBlock` undefined in theme). `cargo test -p wordcartel-core marked_block_mono` + `cargo test -p wordcartel marked_block_paints`

- [ ] **Step 3: Implement theme.** In `theme.rs` add `MarkedBlock` to `SemanticElement`; add `marked_block: Face` to `ThemeFaces`; add the arm to `face()` (and `face_mut()`), `element_from_key()` (`"marked_block" => MarkedBlock`), and `ALL_ELEMENTS` (now 32). Give it a value in EVERY constructor (mirror how `selection`/`front_matter` are set): `mono_faces()` → `m(true /*bold*/, false, true /*underline*/, false, true /*reverse*/)` (= **reverse+bold+underline**); `default()`/`tokyo_night()`/`from_base16()`/phosphor/HSL builtins → a tinted bg `+ reverse + bold + underline` (a distinct, lighter-than-selection bg). Add `MarkedBlock` to the a11y pairwise-distinct test set (it must pass).

- [ ] **Step 4: Implement render.** In `render.rs`, paint `MarkedBlock` on the **placed path** (mirror the selection paint that layers `face_to_ratatui(theme.face(Selection))`). **`use_placed` refactor (Codex):** `use_placed` is computed ONCE before the row loop as an immutable bool; a visible non-hidden `marked_block` must force the placed path too — either fold the block's presence into that precompute (`use_placed = has_selection || has_search || has_diag || (marked_block.is_some() && !hidden)`) or compute a per-row `row_use_placed`. Do **not** only paint inside the existing placed branch (it would silently skip the block when no selection/search/diag is active). Compose `MarkedBlock` **below** Selection/Search/Diag (base → MarkedBlock → Selection → …). Map the block's source bytes → visual cells via the same `ColMap` the selection uses; **only paint visible cells** (fold-safe; never index hidden lines). Add the **`BLK`** status segment to the **left** status text (NOT the word-count-gated right segment): when `marked_block` is `Some`, append ` · BLK` (or ` · BLK·hidden` when `hidden`), themed `Chrome`.

- [ ] **Step 5: Run** `cargo test -p wordcartel-core marked_block` + `cargo test -p wordcartel marked_block` + `cargo test -p wordcartel-core` (a11y/coverage tests pass with the new element) + `cargo test -p wordcartel --lib` — green.

- [ ] **Step 6: Commit** `feat(9a): MarkedBlock cue (reverse+bold+underline) + render paint + BLK status indicator`

---

## Task 7: Keymap — WordStar binds + CUA promote

**Files:**
- Modify: `wordcartel/src/keymap.rs` (WORDSTAR block binds, reclaim `^KC`/`^KV`; CUA `alt-b`; update `wordstar_new_chords` test)
- Test: `wordcartel/src/keymap.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: all `block_*` ids (registered in Tasks 2–4).

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn wordstar_block_chords_resolve() {
        let cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] };
        let (t, w) = build_keymap(&cfg, &Registry::builtins());
        assert!(w.is_empty(), "{w:?}");
        let cmd = |s: &str| t.resolve(&parse_seq(s).unwrap());
        // both the plain AND ctrl-held second-key forms (9B prefix convention)
        assert!(matches!(cmd("ctrl-k b"), Resolution::Command(CommandId("block_begin"))));
        assert!(matches!(cmd("ctrl-k ctrl-b"), Resolution::Command(CommandId("block_begin"))));
        assert!(matches!(cmd("ctrl-k k"), Resolution::Command(CommandId("block_end"))));
        assert!(matches!(cmd("ctrl-k c"), Resolution::Command(CommandId("block_copy"))), "^KC reclaimed from copy");
        assert!(matches!(cmd("ctrl-k ctrl-c"), Resolution::Command(CommandId("block_copy"))), "^K^C reclaimed too");
        assert!(matches!(cmd("ctrl-k v"), Resolution::Command(CommandId("block_move"))), "^KV reclaimed from paste");
        assert!(matches!(cmd("ctrl-k ctrl-v"), Resolution::Command(CommandId("block_move"))), "^K^V reclaimed too");
        assert!(matches!(cmd("ctrl-k y"), Resolution::Command(CommandId("block_delete"))));
        assert!(matches!(cmd("ctrl-k w"), Resolution::Command(CommandId("block_write"))));
        assert!(matches!(cmd("ctrl-k h"), Resolution::Command(CommandId("block_toggle_hidden"))));
        assert!(matches!(cmd("ctrl-q b"), Resolution::Command(CommandId("block_jump_begin"))));
        assert!(matches!(cmd("ctrl-q k"), Resolution::Command(CommandId("block_jump_end"))));
        // remaining ctrl-held forms (lock the both-forms contract — Codex completeness)
        assert!(matches!(cmd("ctrl-k ctrl-k"), Resolution::Command(CommandId("block_end"))));
        assert!(matches!(cmd("ctrl-k ctrl-y"), Resolution::Command(CommandId("block_delete"))));
        assert!(matches!(cmd("ctrl-k ctrl-w"), Resolution::Command(CommandId("block_write"))));
        assert!(matches!(cmd("ctrl-k ctrl-h"), Resolution::Command(CommandId("block_toggle_hidden"))));
        assert!(matches!(cmd("ctrl-q ctrl-b"), Resolution::Command(CommandId("block_jump_begin"))));
        assert!(matches!(cmd("ctrl-q ctrl-k"), Resolution::Command(CommandId("block_jump_end"))));
    }

    #[test]
    fn cua_alt_b_promotes() {
        let (t, _) = build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        assert!(matches!(t.resolve(&parse_seq("alt-b").unwrap()), Resolution::Command(CommandId("mark_block_from_selection"))));
    }
```

- [ ] **Step 2: Run — fails.** `cargo test -p wordcartel wordstar_block cua_alt_b`

- [ ] **Step 3: Implement.** In `keymap.rs` `WORDSTAR`: **replace** the interim rows
`("ctrl-k ctrl-c","copy"),("ctrl-k c","copy")` and `("ctrl-k ctrl-v","paste"),("ctrl-k v","paste")`
with the block binds (both ctrl-held and plain second-key forms per the 9B convention):
`^KB`→`block_begin`, `^KK`→`block_end`, `^KC`→`block_copy`, `^KV`→`block_move`, `^KY`→`block_delete`,
`^KW`→`block_write`, `^KH`→`block_toggle_hidden`, `^QB`→`block_jump_begin`, `^QK`→`block_jump_end`.
In `CUA`: add `("alt-b", "mark_block_from_selection")`. **Codex:** the existing
`wordstar_new_chords_resolve` test does **not** currently assert `^KC`/`^KV` → copy/paste, so
there is nothing to "update" — instead **add** the block-chord assertions (Step 1 above) and
make sure no *other* wordstar test asserts `ctrl-k c`/`ctrl-k v` resolve to `copy`/`paste`
(remove/adjust any that do).

- [ ] **Step 4: Run** `cargo test -p wordcartel wordstar` (incl `both_presets_resolve_against_builtins` + the collision/prefix-shadow test) + `cargo test -p wordcartel --lib` + `cargo test` (workspace) — all green.

- [ ] **Step 5: Commit** `feat(9a): wire WordStar block keymap (reclaim ^KC/^KV) + CUA alt-b promote`

---

## Final Verification
- [ ] `cargo test` (workspace) — all green.
- [ ] `cargo clippy -p wordcartel-core -p wordcartel --lib` — no new warnings in touched files.
- [ ] Manual smoke (wordstar preset): `^KB`…move…`^KK` marks a block (distinct highlight, `BLK` in status); promote via `alt-b` (cua) from a shift-selection; `^KC` copies it to the caret (block stays), `^KV` moves it (one undo restores), `^KY` deletes it; `^KW` writes it to a file (doc unchanged); `^QB`/`^QK` jump (unfold); `^KH` hides the highlight (`BLK·hidden`); edit around the block (it tracks); undo clears it; close + reopen the unchanged file → the block is restored.

## Self-Review Notes (coverage vs spec)
- §2 state/edit-track/undo-clear → T1. §3 creation (incl empty-reject + promote-clears-selection) → T2. §4 ops (copy-stays, move-one-undo + inside-guard, delete, jump, hide, clear) → T3. §5 ^KW → T4. §6 persistence → T5. §7 cue + render + BLK → T6. §8 keymap → T7.
- Codex folds present: map_pos/map_pos_before split (T1); undo-clears (T1); build_multi_replace (T3); expand_path factor (T4); persist_session/restore_resume + serde-default + literal churn (T5); reverse+bold+underline + a11y test + use_placed + fold-safe paint + BLK-not-word-count-gated (T6); reclaim ^KC/^KV + update wordstar test (T7).
- Out of scope (not planned, per spec): multiple blocks; column/rectangular; block indent/case/format; `^KP` print; persisting hidden/pending.
