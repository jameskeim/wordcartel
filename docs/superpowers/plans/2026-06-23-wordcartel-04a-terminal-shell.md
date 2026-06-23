# Wordcartel Terminal Shell — Implementation Plan (Effort 4, Plan 4a)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the synchronous imperative shell — a runnable terminal markdown editor (`wcartel file.md`) that opens a `.md`, renders it live-concealed via the existing `wordcartel-core` engine, and supports full CUA editing, navigation, selection, clipboard, undo/redo, and atomic save.

**Architecture:** Functional core / imperative shell (§10). A new `wordcartel` crate wraps the pure `wordcartel-core` with a flat `Editor { Document, View }` state model, a single mutation channel `apply(Transaction)`, a synchronous `derive` step (incremental block-tree + per-visible-line `layout`), a pure ratatui `render`, and a crossterm input loop. Everything is synchronous in 4a; the worker/async-edges system is Plan 4b. The shell owns the **logical-line dimension** (which line the cursor is on, viewport scroll) and delegates **within-line** conceal/soft-wrap/cursor math to `wordcartel-core`'s `layout`/`ColMap`/`Cursor`.

**Tech Stack:** Rust 2021; `ratatui` + `crossterm` (rendering/terminal, §3.8); `wordcartel-core` (path dep); `thiserror` (error enums, §15.3). Headless-testable via ratatui `TestBackend` + pure state-transition tests.

> **PENDING REVISIONS — apply before executing 4a (from the Codex red-team 2026-06-23).**
> This plan was written before **Effort 3c (block_tree rope integration)** was inserted ahead of it. When revising 4a to execute, fold in ALL of these:
> 1. **(C2/C3 — decided: rope-aware first)** Task 3 `derive::rebuild` must NOT `full_parse(&buffer.to_string())` per keystroke. After 3c lands, `apply` takes an **O(1) rope snapshot** (`buffer.snapshot()`) BEFORE mutating; `derive` calls `block_tree::incremental_update_rope(&old_blocks, &old_rope, &edit, &new_rope)` (materializes only the edited region → O(visible)+O(edited), §3.9). Thread the `block_tree::Edit` + pre-edit rope through `apply`. Initial load uses `full_parse_rope(&rope)` once.
> 2. **(C1)** `History::new()` does not exist → use `History::default()`.
> 3. **(C4)** Caret offset = `selection.primary().head` (the moving end), NOT `.to()/.from()` (those are normalized bounds — reversed selections break). Use `from()/to()` only for copy/delete range bounds. `Range { anchor, head }` fields are public → in Task 9 construct directly; drop the "add `Range::new`" hedge.
> 4. **(C5)** Task 11/12 open: distinguish *new-file* (not found / no path) from **Binary/non-UTF-8 refusal** (don't open; show error; start an UNNAMED empty buffer — never attach the rejected path), permission, and is-a-directory errors.
> 5. **(I7)** `desired_col: Option<usize>`; compute it from the current `ColMap` column on the FIRST vertical move (don't seed it in tests).
> 6. **(I8)** Specify + test logical-line edge cases: clamp `line_start(L)` for `L == total_lines`; tests for `""`, `"a"`, `"a\n"`, `"\n"`, `"é\nz\n"`; pin whether `eol`/`line_to_byte` include the `\n`.
> 7. **(I9/I10)** Task 11: `atomic.rs` only does temp/write/fsync/rename/dir-fsync — Task 11 must ADD preflight `symlink_metadata` refusal, skip-unchanged read+compare, and mode preservation around it; return `SaveOutcome::{Saved, Unchanged}` and assert on that (not mtime).
> 8. **(I11 — §15.6)** Terminal-too-small handling IS in 4a: render/derive clamp width/height + a "window too small" notice; `TestBackend` cases at `1x1`, `2x1`, 0-height area.
> 9. **(I13/§13.2)** Don't over-claim render accessibility: 4a uses modifier styling (BOLD/ITALIC/UNDERLINE/CROSSED_OUT — non-color for most distinctions); a full no-color/high-contrast toggle is Effort 5. Code/Link still lean on color — note it.
> 10. **(§15.7 wording)** Self-Review should say §15 is split across 4a (terminal restore, atomic save, open/save errors) + 4b (swap file, emergency dump) — both v1 — not "§15 ✅ in 4a".
> 11. **(MINOR)** Reuse table: `incremental_update` signature is `(&BlockTree, &str, &Edit, &str) -> BlockTree` (will become the rope-typed variant after 3c).

## Global Constraints

- New crate `wordcartel` (binary name `wcartel`); cargo **workspace** with `wordcartel-core`. `#![forbid(unsafe_code)]` in the new crate too.
- Canonical position = **byte offset** (`usize` / core `BytePos`) into the document text. The selection head/anchor are global byte offsets; never store a bare offset as durable display state — every surviving position is mapped through the `ChangeSet` on the same atomic step as the edit (§10.6).
- **One mutation channel:** all text/selection/history change goes through `Editor::apply`. `render` mutates nothing (pure read). No mutation inside `draw()` (§10.6).
- **Draw synchronously on every input event** — never on a timer (§3.9). Per-keystroke foreground work stays O(visible screen) + O(edited block) (§3.9): on edit, `incremental_update` the block-tree and re-`layout` only affected/visible logical lines.
- **Document/View split** even with one window (soft-wrap is view-derived; conflating it with logical lines is what broke xi — §10.2).
- **Reconcile = discard** is a 4b concern; 4a is synchronous so there is no staleness yet. Keep `version: u64` (`+= 1` per `apply`) now so 4b can stamp jobs.
- Errors are values at the edges (`Result` + `thiserror`); the pure core does not fail. A panic restores the terminal via a guard (§15.7; the *emergency buffer dump* and swap file are 4b).
- TDD; pristine output (no warnings); frequent commits. Reuse `wordcartel-core` for ALL editing/layout/parse — the shell adds no buffer/parse/cursor math of its own beyond the logical-line bookkeeping specified here.
- **Live preview is the default render** (§3.2/§3.11). Active logical line renders raw (`is_active=true`, identity-ish col_map); all other visible lines render concealed.

---

## Reuse Posture

The shell is deliberately thin. It **consumes** the merged core API verbatim — no reimplementation:

| Need | Core API (exact signatures) |
|---|---|
| Buffer | `TextBuffer::from_str(&str)`, `.insert(at: usize, &str)`, `.delete(Range<usize>)`, `.slice(Range<usize>) -> String`, `.len() -> usize`, `.to_string() -> String`, `.snapshot() -> ropey::Rope`, `.byte_to_line(b) -> usize`, `.line_to_byte(line) -> usize` |
| Edits | `ChangeSet::insert(at, &str, doc_len) -> ChangeSet`, `ChangeSet::delete(Range, doc_len) -> ChangeSet`, `.apply(&mut TextBuffer)`, `.invert(&TextBuffer) -> ChangeSet` |
| Selection | `Selection::single(pos) -> Selection`, `.primary() -> Range`, `.map(&ChangeSet) -> Selection`; `Range::point(pos)`, `.from() -> usize`, `.to() -> usize`, `.is_empty() -> bool`, `.map(&ChangeSet) -> Range` |
| History | `History::new()`, `.commit_coalescing(txn: Transaction, buf: &mut TextBuffer, before: Selection, clock: &dyn Clock, kind: EditKind) -> Selection`, `.commit(txn, buf, before) -> Selection`, `.undo(&mut TextBuffer) -> Option<Selection>`, `.redo(&mut TextBuffer) -> Option<Selection>`; `Transaction::new(ChangeSet)`, `.with_selection(Selection)`; `EditKind::{Type, Other}`; `trait Clock { fn now_ms(&self) -> u64 }`; `COALESCE_MS` |
| Clipboard | `Register`, `register::copy(&TextBuffer, Range, &mut Register)`, `register::cut(Range, doc_len, &mut Register, &TextBuffer) -> ChangeSet`, `register::paste(at, doc_len, &Register) -> Option<ChangeSet>` |
| Block roles | `block_tree::full_parse(&str) -> BlockTree`, `block_tree::incremental_update(&BlockTree, old_text, &Edit, new_text) -> BlockTree`, `BlockTree::role_at(byte) -> BlockRole`, `block_tree::apply_edit(old_text, Range, repl) -> (String, Edit)` |
| Layout | `layout::layout(line: &str, role: BlockRole, is_active: bool, viewport_width: usize) -> (Vec<VisualRow>, ColMap)`; `VisualRow { display, width, src_span, segs: Vec<StyledSeg>, role, prefix_glyph }`; `StyledSeg { text, style, width }` |
| Cursor (per logical line) | `Cursor { offset, row, desired_col }`, `cursor_at(&ColMap, offset) -> Cursor`, `move_right/move_left/move_home/move_end(&ColMap, Cursor) -> Cursor`, `move_down_within/move_up_within(&ColMap, Cursor) -> Option<Cursor>`, `enter_from_top/enter_from_bottom(&ColMap, desired_col) -> Cursor`; `ColMap { placed, rows, eol, row_end_col, is_active }`, `.source_to_visual(offset) -> (row,col)`, `.visual_to_source(row,col) -> usize`, `.snap_to_stop(raw) -> usize`, `.col_on_row(offset,row) -> usize` |
| Style enum | `style::Style { Plain, Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link }`, `style::BlockRole { Paragraph, Heading(u8), BlockQuote, ListItem, CodeBlock, ThematicBreak, FrontMatter }` |

**Within-line vs across-line responsibility (load-bearing):** `ColMap`/`Cursor`/`move_*` operate on a SINGLE logical line. The shell holds the global selection (byte offsets) and the **active logical line index**; it converts a global offset to `(line_index, in_line_offset)` via the rope, calls the core's within-line motion, and when motion runs off the start/end of a line (`move_left` at offset 0, `move_right` at `eol`, `move_*_within` returns `None`) the SHELL performs the logical-line transition and re-enters the adjacent line's `ColMap` via `cursor_at` / `enter_from_top` / `enter_from_bottom`. The shell writes none of the conceal/wrap/column math itself.

---

## File Structure

New crate at `wordcartel/` (workspace member). All modules in the lib target; `main.rs` is a thin binary entry.

- `Cargo.toml` (workspace root, repo top) — `[workspace] members = ["wordcartel-core", "wordcartel"]`.
- `wordcartel/Cargo.toml` — `[package] name="wordcartel"`, `[[bin]] name="wcartel"`, deps: `wordcartel-core` (path), `ratatui`, `crossterm`, `thiserror`.
- `wordcartel/src/main.rs` — parse one path arg; init terminal; run `App::run`; restore terminal; print any top-level error. Thin.
- `wordcartel/src/lib.rs` — `#![forbid(unsafe_code)]`; `pub mod {editor, derive, nav, commands, input, render, term, file};` (+ re-exports used by tests).
- `wordcartel/src/editor.rs` — `Document`, `View`, `Editor`, `RenderMode`; `Editor::new_from_text`, `Editor::apply`, dirty-range tracking, `version`.
- `wordcartel/src/derive.rs` — the `derive` step: maintain `Editor.document.blocks` (BlockTree) + `Editor.view.line_layouts` cache (per visible logical line: `Vec<VisualRow>` + `ColMap`), rebuilt from truth.
- `wordcartel/src/nav.rs` — cursor placement (global offset → screen `(row,col)`), and the across-logical-line motion functions (`move_left/right/up/down/home/end/doc_home/doc_end`) returning a new global offset + desired_col, delegating within-line to core.
- `wordcartel/src/commands.rs` — `Command` enum + `fn run(cmd, &mut Editor, &dyn Clock) -> CommandResult` producing a `Transaction`/`Effect` and calling `apply`; never mutates text directly except through `apply`.
- `wordcartel/src/input.rs` — `key_to_command(KeyEvent, mode) -> Option<Command>`: the static CUA keymap (§12.3) + printable→InsertChar fallback. (Pending multi-key sequences are a stub returning `None` extra state in 4a; full KeyTrie is Effort 5.)
- `wordcartel/src/render.rs` — `render(frame: &mut Frame, editor: &Editor)`: pure paint of the viewport + status line + (optional) wrap-guide. ratatui widgets.
- `wordcartel/src/term.rs` — `TerminalGuard`: enter raw mode + alt screen on construct, restore on Drop; install a panic hook that restores the terminal before the default hook prints (§15.7 terminal-restore half).
- `wordcartel/src/file.rs` — `open(path) -> Result<String, OpenError>` (refuse binary/non-UTF-8 per §15.3, mirroring repar `is_binary`), `save_atomic(path, &str) -> Result<(), SaveError>` (same-dir O_EXCL temp `0o600` → write → fsync → rename → dir fsync → skip-unchanged → refuse symlink; ports repar `atomic.rs`, §14.3). Synchronous in 4a.

Each task ends with an independently testable deliverable.

---

## Testing Approach (read once)

- **Headless state tests** (most tasks): build an `Editor` from text, drive `apply`/commands/nav directly, assert `document.buffer.to_string()`, `selection`, `version`, dirty range, and screen-mapping outputs. No terminal needed.
- **Render tests**: ratatui `Terminal::new(TestBackend::new(w, h))`, call `render`, assert `terminal.backend().buffer()` cell contents (text + style). This exercises `render` + `derive` end-to-end without a real TTY.
- **A `TestClock`** (implements core `Clock`) drives coalescing deterministically — mirror the core's `FakeClock` pattern. No wall-clock in tests (§11.3).
- Property/golden tests where they fit (e.g. "cursor screen position round-trips through visual↔source on random multi-line/multibyte buffers"), honoring the project rule: cross directions × concealed/raw × single/sequence × ASCII/multibyte.

---

### Task 0: Workspace + `wordcartel` crate scaffold

**Files:** Create `Cargo.toml` (workspace root), `wordcartel/Cargo.toml`, `wordcartel/src/main.rs`, `wordcartel/src/lib.rs`.

**Interfaces — Produces:** a buildable `wcartel` binary that prints usage and exits; the lib target with empty module decls.

- [ ] **Step 1:** Create root `Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["wordcartel-core", "wordcartel"]
```
- [ ] **Step 2:** Create `wordcartel/Cargo.toml`:
```toml
[package]
name = "wordcartel"
version = "0.0.0"
edition = "2021"
license = "MIT"

[[bin]]
name = "wcartel"
path = "src/main.rs"

[dependencies]
wordcartel-core = { path = "../wordcartel-core" }
ratatui = "0.29"
crossterm = "0.28"
thiserror = "2"

[dev-dependencies]
proptest = "1"
```
(Pin to the latest 0.29 ratatui / 0.28 crossterm that resolve; if a newer compatible minor exists, use it and note it.)
- [ ] **Step 3:** Create `wordcartel/src/lib.rs`:
```rust
#![forbid(unsafe_code)]
//! Wordcartel terminal shell (imperative shell over wordcartel-core).
pub mod editor;
pub mod derive;
pub mod nav;
pub mod commands;
pub mod input;
pub mod render;
pub mod term;
pub mod file;
```
Create each listed module file with a `// filled in by later tasks` placeholder so the crate compiles.
- [ ] **Step 4:** Create `wordcartel/src/main.rs`:
```rust
#![forbid(unsafe_code)]
fn main() {
    let path = std::env::args().nth(1);
    match path {
        Some(p) => eprintln!("wcartel: would open {p} (loop wired in Task 12)"),
        None => eprintln!("usage: wcartel <file.md>"),
    }
}
```
- [ ] **Step 5:** Run `cargo build` (workspace) → clean; `cargo build -p wordcartel` → clean. Confirm `wordcartel-core` tests still pass: `cargo test -p wordcartel-core`.
- [ ] **Step 6:** Commit: `chore(shell): scaffold wordcartel workspace crate (wcartel bin)`

---

### Task 1: State model — Document / View / Editor

**Files:** `wordcartel/src/editor.rs`; tests in-module.

**Interfaces — Produces:**
```rust
pub enum RenderMode { LivePreview, SourceHighlighted, SourcePlain }

pub struct Document {
    pub buffer: wordcartel_core::buffer::TextBuffer,
    pub selection: wordcartel_core::selection::Selection,
    pub history: wordcartel_core::history::History,
    pub blocks: wordcartel_core::block_tree::BlockTree, // derived cache (Task 3 maintains)
    pub version: u64,
    pub path: Option<std::path::PathBuf>,
    pub dirty: bool, // unsaved changes
}

pub struct View {
    pub scroll: usize,        // first visible LOGICAL line index
    pub area: (u16, u16),     // (width, height) cells of the editing area
    pub mode: RenderMode,
    // line_layouts cache added in Task 3
}

pub struct Editor {
    pub document: Document,
    pub view: View,
    pub status: String,       // ephemeral feedback line
    pub quit: bool,
    pub desired_col: usize,    // cursor's preserved visual column for vertical motion
    pub last_edit_range: Option<std::ops::Range<usize>>, // dirty byte range for derive
}
```
`Editor::new_from_text(text: &str, path: Option<PathBuf>, area: (u16,u16)) -> Editor`: builds `TextBuffer::from_str`, `Selection::single(0)`, `History::new()`, `full_parse(text)` for `blocks`, `version=0`, `dirty=false`, `mode=LivePreview`, `scroll=0`, `status=""`.

- [ ] **Step 1: Write failing test**:
```rust
#[test]
fn new_editor_holds_text_and_clean_state() {
    let e = Editor::new_from_text("# Hi\n\nbody\n", None, (80, 24));
    assert_eq!(e.document.buffer.to_string(), "# Hi\n\nbody\n");
    assert_eq!(e.document.selection.primary().from(), 0);
    assert_eq!(e.document.version, 0);
    assert!(!e.document.dirty);
    assert!(!e.document.blocks.top_level().is_empty());
}
```
- [ ] **Step 2:** Run `cargo test -p wordcartel` → FAIL (no `Editor`).
- [ ] **Step 3:** Implement the structs + `new_from_text`. Derive `Debug` where cheap. `RenderMode` derives `Clone, Copy, PartialEq, Eq, Debug`.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(shell): Editor/Document/View state model`

---

### Task 2: `apply(Transaction)` — the single mutation channel

**Files:** `wordcartel/src/editor.rs`.

**Interfaces — Produces:** `Editor::apply(&mut self, txn: Transaction, kind: EditKind, clock: &dyn Clock)`. It is the ONLY place text/selection/history change. Behavior (§10.1):
1. Record the edit's affected byte range BEFORE applying (compute from the ChangeSet — see Step 3) into `self.last_edit_range` for the derive step.
2. `let before = self.document.selection.clone();`
3. `self.document.selection = self.document.history.commit_coalescing(txn, &mut self.document.buffer, before, clock, kind);`
4. `self.document.version += 1; self.document.dirty = true;`
5. (Derive is called by the command layer / loop AFTER apply — Task 3 — not inside apply, to keep apply about truth-mutation only.)

`undo`/`redo` are NOT routed through `apply` (they don't take a ChangeSet): add `Editor::undo(&mut self)` / `Editor::redo(&mut self)` that call `history.undo/redo(&mut buffer)`, set `selection` from the returned `Option<Selection>` (ignore `None`), bump `version`, set `dirty=true`, and set `last_edit_range = Some(0..buffer.len())` (conservative full re-derive for undo/redo in 4a; targeted range is a 4b refinement).

- [ ] **Step 1: Write failing tests**:
```rust
struct TestClock(std::cell::Cell<u64>);
impl wordcartel_core::history::Clock for TestClock {
    fn now_ms(&self) -> u64 { self.0.get() }
}
#[test]
fn apply_insert_mutates_text_selection_version() {
    let mut e = Editor::new_from_text("ab\n", None, (80,24));
    let clk = TestClock(std::cell::Cell::new(0));
    // insert "X" at offset 1 -> "aXb\n"
    let cs = ChangeSet::insert(1, "X", e.document.buffer.len());
    let txn = Transaction::new(cs).with_selection(Selection::single(2));
    e.apply(txn, EditKind::Type, &clk);
    assert_eq!(e.document.buffer.to_string(), "aXb\n");
    assert_eq!(e.document.selection.primary().from(), 2);
    assert_eq!(e.document.version, 1);
    assert!(e.document.dirty);
    assert_eq!(e.last_edit_range, Some(1..2)); // inserted span in NEW coords
}
#[test]
fn undo_redo_round_trip() {
    let mut e = Editor::new_from_text("ab\n", None, (80,24));
    let clk = TestClock(std::cell::Cell::new(0));
    let cs = ChangeSet::insert(1, "X", e.document.buffer.len());
    e.apply(Transaction::new(cs).with_selection(Selection::single(2)), EditKind::Type, &clk);
    e.undo();
    assert_eq!(e.document.buffer.to_string(), "ab\n");
    e.redo();
    assert_eq!(e.document.buffer.to_string(), "aXb\n");
}
```
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement. For `last_edit_range`: the implementer derives the new-coordinate affected range from the `ChangeSet` ops (Retain/Delete/Insert in bytes). Simplest correct approach: walk the ops tracking old-pos and new-pos; the affected new-range is `[first_changed_new .. last_changed_new]` where insertions extend by inserted length and deletions mark a zero-width point in new coords. If deriving precisely is awkward, fall back to the union covering from the first non-Retain op's new offset to the end of the last change — correctness of the *derive* only requires the range to **cover** all changed bytes (over-covering re-layouts extra lines, which is still correct, just slightly more work). Document the choice.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(shell): apply(Transaction) single mutation channel + undo/redo`

---

### Task 3: Derive step — block-tree + per-line layout cache

**Files:** `wordcartel/src/derive.rs`; add `pub line_layouts: std::collections::BTreeMap<usize, (Vec<VisualRow>, ColMap)>` to `View` (keyed by LOGICAL line index), and `pub blocks` already on `Document`.

**Interfaces — Produces:** `derive::rebuild(editor: &mut Editor)` — recompute derived caches from truth for the **visible** logical-line range. Steps:
1. **Block tree (4a rule — full parse):** recompute `editor.document.blocks = full_parse(&editor.document.buffer.to_string())` unconditionally. This is deliberately the simple, correct path for 4a: it avoids threading the pre-edit text + `Edit` through `apply`, and `full_parse` over the §3.9 corpus (≤5 MB) is within the v1-first budget. The incremental path (`incremental_update`) and its oracle already exist in core and will be wired here in Plan 4b with retained `old_text` + `Edit`. Leave a marker: `// 4b: replace full_parse with incremental_update(old_blocks, old_text, edit, new_text)`.
2. **Visible range:** `first = view.scroll`; walk logical lines accumulating visual-row heights until the editing area height is filled (+1 overscan); `last` = last line that fits. Total logical lines = `buffer.byte_to_line(buffer.len()) + 1`.
3. **Active line:** the logical line containing `selection.primary().head` (use `Range::to()` for head; primary head = `to()` when head>=anchor — store head explicitly, see Task 4 note). For each visible logical line `L`: `let text = line_text(L);` (slice `[line_to_byte(L), line_to_byte(L+1))` minus trailing `\n`), `let role = blocks.role_at(line_to_byte(L));`, `let (rows, map) = layout(&text, role, L == active_line, area.width as usize);` store in `view.line_layouts`.
4. Clear `editor.last_edit_range = None`.

Helper `derive::line_text(buf, L) -> String` and `derive::total_logical_lines(buf) -> usize` and `derive::line_start(buf, L) -> usize`.

- [ ] **Step 1: Write failing tests**:
```rust
#[test]
fn derive_lays_out_visible_lines_with_roles() {
    let mut e = Editor::new_from_text("# Title\n\nplain body\n", None, (80, 24));
    derive::rebuild(&mut e);
    let (rows0, _) = &e.view.line_layouts[&0];
    // inactive heading line -> "# " concealed -> "Title"
    assert_eq!(rows0[0].display, "Title");
    assert_eq!(rows0[0].role, BlockRole::Heading(1));
}
#[test]
fn active_line_renders_raw() {
    let mut e = Editor::new_from_text("# Title\n", None, (80, 24));
    // cursor at 0 -> line 0 active -> raw "# Title"
    derive::rebuild(&mut e);
    let (rows0, _) = &e.view.line_layouts[&0];
    assert_eq!(rows0[0].display, "# Title");
}
```
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement `rebuild` + helpers per the spec above (full_parse path for 4a).
- [ ] **Step 4:** Run → PASS; also assert a wrapped long line yields `rows.len() > 1` at small width.
- [ ] **Step 5:** Commit: `feat(shell): derive step — block roles + per-line layout cache`

---

### Task 4: Cursor placement & viewport scroll

**Files:** `wordcartel/src/nav.rs`. Add an explicit cursor head model: store the cursor as the **head** of `selection.primary()`. Add `Editor::head() -> usize` returning `selection.primary().to()` if the selection is forward else `.from()` — but since 4a starts with collapsed selections and builds selection in Task 9, **store head/anchor explicitly**: change selection usage so the cursor's caret = `selection.primary()` head. Add `nav::head(editor) -> usize` = the caret byte offset (for a collapsed selection, `primary().from() == primary().to()`).

**Interfaces — Produces:**
- `nav::caret_line(editor) -> usize` — logical line index of the caret.
- `nav::screen_pos(editor) -> Option<(u16, u16)>` — caret cell `(col, row)` within the editing area, or `None` if the caret line is scrolled off. Computed by: find caret line `L`; `in_off = head - line_start(L)`; look up `view.line_layouts[&L].1` (the active ColMap); `let (vrow, vcol) = map.source_to_visual(in_off);` then add the cumulative visual-row height of lines `[scroll..L)` to `vrow` to get the screen row; `vcol` is the screen col.
- `nav::ensure_visible(editor)` — adjust `view.scroll` so the caret line's visual rows are within the area (scroll up if caret above; scroll down if below, accounting for wrapped heights). Clamp scroll to `[0, last_line]`.

- [ ] **Step 1: Write failing tests**:
```rust
#[test]
fn screen_pos_maps_caret_to_cell() {
    let mut e = Editor::new_from_text("abc\ndef\n", None, (80, 24));
    set_caret(&mut e, 5); // 'e' in "def" (line 1, col 1)
    derive::rebuild(&mut e);
    assert_eq!(nav::screen_pos(&e), Some((1, 1)));
}
#[test]
fn ensure_visible_scrolls_caret_into_view() {
    let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let mut e = Editor::new_from_text(&text, None, (80, 10));
    set_caret(&mut e, derive::line_start(&e.document.buffer, 50));
    nav::ensure_visible(&mut e);
    derive::rebuild(&mut e);
    assert!(nav::screen_pos(&e).is_some());
    assert!(e.view.scroll <= 50 && e.view.scroll + 10 > 50);
}
```
(`set_caret` test helper sets `selection = Selection::single(off)`.)
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement. Note `source_to_visual` needs the offset to be a valid cursor stop; the caret offset is always a stop because all motion (Tasks 6–7) goes through `snap_to_stop`/core `move_*`. For the initial caret (0) and `set_caret`, snap via `map.snap_to_stop`.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(shell): caret screen placement + viewport scroll`

---

### Task 5: ratatui render (live preview) + status line

**Files:** `wordcartel/src/render.rs`.

**Interfaces — Produces:** `render::render(frame: &mut ratatui::Frame, editor: &Editor)`. Pure. Layout: editing area = full frame minus the bottom status row. For each visible logical line `L` (from `scroll`), for each `VisualRow` in `view.line_layouts[&L].0`, build a ratatui `Line` from the row's `segs` — one `Span` per `StyledSeg`, mapping `Style` → ratatui `Style` (Strong→BOLD, Emphasis→ITALIC, Code→a dim/alt color, Link→underline+color, Plain→default; StrongEmphasis→BOLD|ITALIC; Strikethrough→CROSSED_OUT). Prepend `prefix_glyph` (if `Some`, e.g. the `• ` bullet) as a leading dim span on the row. Stop when the area height is filled. Render the status line (path, `*` if dirty, `version`/mode/`status` text) on the bottom row. Set the terminal cursor to `nav::screen_pos(editor)` (caller positions the hardware cursor; render returns it or sets `frame.set_cursor_position`).

Provide `render::style_to_ratatui(Style) -> ratatui::style::Style` as a pure, separately-tested fn.

- [ ] **Step 1: Write failing tests** (TestBackend):
```rust
use ratatui::{backend::TestBackend, Terminal};
#[test]
fn renders_concealed_heading_and_cursor_on_active_line() {
    let mut e = Editor::new_from_text("# Title\n\nbody\n", None, (20, 6));
    set_caret(&mut e, 10); // somewhere in "body" so heading line is inactive/concealed
    derive::rebuild(&mut e);
    let mut term = Terminal::new(TestBackend::new(20, 6)).unwrap();
    term.draw(|f| render::render(f, &e)).unwrap();
    let buf = term.backend().buffer();
    // row 0 shows "Title" (concealed "# "), not "# Title"
    let row0: String = (0..20).map(|x| buf[(x,0)].symbol().chars().next().unwrap_or(' ')).collect();
    assert!(row0.starts_with("Title"));
}
#[test]
fn style_mapping_is_bold_for_strong() {
    assert!(render::style_to_ratatui(Style::Strong).add_modifier.contains(ratatui::style::Modifier::BOLD));
}
```
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement render + `style_to_ratatui`. Use `frame.set_cursor_position((col,row))` when `screen_pos` is `Some`.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(shell): ratatui live-preview render + status line`

---

### Task 6: Horizontal navigation across logical lines

**Files:** `wordcartel/src/nav.rs`.

**Interfaces — Produces:** `nav::move_left(editor) -> usize`, `nav::move_right(editor) -> usize`, `nav::move_home(editor) -> usize`, `nav::move_end(editor) -> usize` — each returns the NEW global caret offset and updates `editor.desired_col`. Logic for `move_right`:
- `L = caret_line; (_, map) = layout of L; in_off = head - line_start(L);` `let cur = cursor_at(&map, in_off);` `let nxt = core::move_right(&map, cur);`
- If `nxt.offset == cur.offset` AND `cur.offset == map.eol` (already at line end) AND `L` is not the last line → transition: new caret = `line_start(L+1)` snapped to that line's first stop (`cursor_at(next_map, 0).offset`).
- Else new caret = `line_start(L) + nxt.offset`.
- Update `desired_col` from the resulting line's `map.col_on_row(in_off2, row)`.
`move_left` symmetric: if `move_left` doesn't advance and `in_off == 0` and `L>0` → transition to end of line `L-1` (`cursor_at(prev_map, prev_map.eol).offset`). `move_home` = `cursor_at(map, 0)` within line (or core `move_home`); `move_end` = core `move_end`.

These return offsets; the **command layer** (Task 8/9) sets `selection = Selection::single(new)` (collapsed) or extends the selection (Task 9).

- [ ] **Step 1: Write failing tests**:
```rust
#[test]
fn right_crosses_line_boundary() {
    let mut e = Editor::new_from_text("ab\ncd\n", None, (80,24));
    set_caret(&mut e, 2); // end of "ab" (before '\n')
    derive::rebuild(&mut e);
    let n = nav::move_right(&mut e); // should land at start of "cd" (offset 3)
    assert_eq!(n, 3);
}
#[test]
fn left_crosses_line_boundary() {
    let mut e = Editor::new_from_text("ab\ncd\n", None, (80,24));
    set_caret(&mut e, 3); // start of "cd"
    derive::rebuild(&mut e);
    let n = nav::move_left(&mut e); // -> end of "ab" (offset 2)
    assert_eq!(n, 2);
}
```
(Test multibyte: `"é\nz\n"` right/left across the boundary lands on char boundaries.)
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement. The caret-line layout must be present in `line_layouts`; if a transition targets a line not in the visible cache, lay it out on demand (`layout(line_text(L±1), role, true, width)`) rather than relying on the cache.
- [ ] **Step 4:** Run → PASS (incl. multibyte).
- [ ] **Step 5:** Commit: `feat(shell): horizontal cursor nav across logical lines`

---

### Task 7: Vertical navigation across logical lines

**Files:** `wordcartel/src/nav.rs`.

**Interfaces — Produces:** `nav::move_up(editor) -> usize`, `nav::move_down(editor) -> usize`, preserving `editor.desired_col` across the motion (set on horizontal motion / first vertical motion, preserved while moving vertically — §16.2 desired_col).
- `move_down`: `(_, map) = layout(L, active); cur = cursor_at(map, in_off);` try `core::move_down_within(&map, cur)`:
  - `Some(c)` → caret stays in line `L`, new caret = `line_start(L) + c.offset` (wrapped multi-row line).
  - `None` → at the bottom visual row of line `L`. If `L` is the last line, no-op (return current head). Else lay out line `L+1` (active), `let c = core::enter_from_top(&next_map, editor.desired_col);` new caret = `line_start(L+1) + c.offset`.
- `move_up` symmetric with `move_up_within` / `enter_from_bottom` into line `L-1`.
- `desired_col`: set it from the CURRENT visual column the FIRST time a vertical motion begins (the command layer sets `desired_col` on horizontal moves; vertical moves read+preserve it). Implement by having the vertical fns take the stored `editor.desired_col` and NOT overwrite it.

- [ ] **Step 1: Write failing tests**:
```rust
#[test]
fn down_preserves_column_across_lines() {
    let mut e = Editor::new_from_text("hello\nworld\n", None, (80,24));
    set_caret(&mut e, 3); // col 3 on "hello" ('l')
    e.desired_col = 3;
    derive::rebuild(&mut e);
    let n = nav::move_down(&mut e); // col 3 on "world" -> offset 6+3 = 9
    assert_eq!(n, 9);
}
#[test]
fn down_within_wrapped_line_stays_in_line() {
    // narrow width forces "aaaaaa" to wrap; down moves to the 2nd visual row, same logical line
    let mut e = Editor::new_from_text("aaaaaa\nz\n", None, (3, 24));
    set_caret(&mut e, 0);
    e.desired_col = 0;
    derive::rebuild(&mut e);
    let n = nav::move_down(&mut e);
    assert!(n > 0 && n < 6); // still inside the first logical line's wrapped rows
}
```
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(shell): vertical cursor nav (wrapped + cross-line, desired_col)`

---

### Task 8: Editing commands — insert / backspace / delete / newline

**Files:** `wordcartel/src/commands.rs`.

**Interfaces — Produces:** `Command` enum (at least `InsertChar(char)`, `InsertNewline`, `Backspace`, `DeleteForward`, plus the nav/clipboard/undo/file variants added in later tasks) and `commands::run(cmd: Command, editor: &mut Editor, clock: &dyn Clock) -> CommandResult` where `CommandResult` is a small struct/enum (`Handled`, `Quit`, `Noop`). For edits:
- `InsertChar(c)`: `let at = nav::head(editor); let cs = ChangeSet::insert(at, &c.to_string(), buffer.len()); editor.apply(Transaction::new(cs).with_selection(Selection::single(at + c.len_utf8())), EditKind::Type, clock); derive::rebuild(editor); nav::ensure_visible(editor);`
- `InsertNewline`: same with `"\n"`, `EditKind::Type` (or `Other` to break coalescing on newline — choose `Other` so undo chunks per line; document the choice).
- `Backspace`: if caret `> 0` and selection empty, delete the char before caret: compute the previous char-boundary via the active line / rope; `cs = ChangeSet::delete(prev..at, len)`; new caret = `prev`. If a selection is non-empty (Task 9), delete the selection range instead. `EditKind::Other`.
- `DeleteForward`: delete the char at caret (next boundary), caret stays. `EditKind::Other`.
After every edit: `derive::rebuild` + `nav::ensure_visible`.

- [ ] **Step 1: Write failing tests**:
```rust
#[test]
fn insert_char_types_and_advances() {
    let mut e = Editor::new_from_text("ac\n", None, (80,24));
    set_caret(&mut e, 1);
    let clk = TestClock(0.into());
    commands::run(Command::InsertChar('b'), &mut e, &clk);
    assert_eq!(e.document.buffer.to_string(), "abc\n");
    assert_eq!(nav::head(&e), 2);
}
#[test]
fn backspace_deletes_prev_char() {
    let mut e = Editor::new_from_text("abc\n", None, (80,24));
    set_caret(&mut e, 2);
    let clk = TestClock(0.into());
    commands::run(Command::Backspace, &mut e, &clk);
    assert_eq!(e.document.buffer.to_string(), "ac\n");
    assert_eq!(nav::head(&e), 1);
}
#[test]
fn typing_coalesces_into_one_undo() {
    let mut e = Editor::new_from_text("\n", None, (80,24));
    let clk = TestClock(0.into()); // same timestamp -> within COALESCE_MS
    for c in "hi".chars() { set_caret_end(&mut e); commands::run(Command::InsertChar(c), &mut e, &clk); }
    e.undo();
    assert_eq!(e.document.buffer.to_string(), "\n"); // both chars undone together
}
```
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement. Prev/next char boundary: use `buffer.to_string()` + `char_indices`, or rope grapheme stepping; for 4a a char boundary (not grapheme cluster) is acceptable — note that grapheme-aware backspace is a refinement (the core's cursor stops are grapheme-aware; reuse the active line's `ColMap` cursor stops to find prev/next stop for correctness). Prefer: derive prev/next caret via `nav::move_left`/`nav::move_right` offsets (grapheme-correct) and delete the span between.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(shell): insert/backspace/delete/newline editing commands`

---

### Task 9: Selection + clipboard

**Files:** `wordcartel/src/commands.rs`, `wordcartel/src/nav.rs`. Add `Register` to `Editor` (field `register: Register`).

**Interfaces — Produces:** selection-extending nav (`Command::Move{dir, extend: bool}`) and clipboard commands (`Copy`, `Cut`, `Paste`). The caret model becomes a true anchor/head: store `Selection` with `anchor != head` when extending. A `Move{dir, extend:false}` collapses to a point at the new offset; `extend:true` keeps the anchor and moves the head to the new offset (`Selection::single` won't do — construct `Range { anchor, head }`; if core `Range` fields aren't public, add a `Range::new(anchor, head)` constructor to the core in a tiny core change, or use existing `Range::point` + map. **Check core**: `Range` has `point`, `from`, `to`, `map`; if no `{anchor,head}` constructor is public, add `pub fn new(anchor: usize, head: usize) -> Range` to `wordcartel-core/src/selection.rs` as part of this task (small, with its own test in core).)
- `Copy`: `register::copy(&buffer, sel.primary(), &mut register);` status "Copied".
- `Cut`: `let cs = register::cut(sel.primary(), buffer.len(), &mut register, &buffer); apply(Transaction::new(cs).with_selection(Selection::single(sel.primary().from())), Other, clock);`
- `Paste`: `if let Some(cs) = register::paste(head, buffer.len(), &register) { apply(...) }` caret after inserted text.

- [ ] **Step 1: Write failing tests**: select-right-twice then Copy yields the 2 chars in register; Cut removes them and leaves caret at range start; Paste inserts at caret. (Add a core `Range::new` test if you add the constructor.)
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement. Extend-selection nav reuses Task 6/7 offset computations for the head; anchor stays.
- [ ] **Step 4:** Run → PASS (incl. core `Range::new` test if added).
- [ ] **Step 5:** Commit: `feat(shell): selection + in-process clipboard (copy/cut/paste)`

---

### Task 10: Undo/redo commands + render-mode toggle

**Files:** `wordcartel/src/commands.rs`.

**Interfaces — Produces:** `Command::Undo`/`Redo` → `editor.undo()/redo()` + `derive::rebuild` + `ensure_visible`. `Command::CycleRenderMode` → rotate `view.mode` LivePreview→SourceHighlighted→SourcePlain→LivePreview; `derive::rebuild` must honor `view.mode`: in source modes, lay out every line with `is_active=true`-style raw rendering (markers visible). **Wire mode into derive:** pass an `is_active_effective = (L == active_line) || view.mode != LivePreview` into `layout`; for `SourcePlain` additionally skip styling (render `Style::Plain` for all segs in render). (Per §3.11, source modes are strictly cheaper — identity-ish col_map, conceal off.)

- [ ] **Step 1: Write failing tests**: undo/redo via commands round-trips; `CycleRenderMode` then `derive::rebuild` makes an inactive heading line show raw `# Title` (markers visible) in SourceHighlighted.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement (thread `view.mode` through `derive::rebuild`'s `is_active` decision).
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(shell): undo/redo commands + render-mode toggle`

---

### Task 11: File open + atomic save

**Files:** `wordcartel/src/file.rs`. Port the relevant `atomic.rs` logic from `~/projects/par-command/repar` (MIT, user's own — READ it).

**Interfaces — Produces:**
```rust
#[derive(thiserror::Error, Debug)]
pub enum OpenError { #[error("{0}: not found")] NotFound(String), #[error("{0}: not valid UTF-8 / binary")] Binary(String), #[error("{0}")] Io(String) }
#[derive(thiserror::Error, Debug)]
pub enum SaveError { #[error("no path")] NoPath, #[error("refusing to write through symlink")] Symlink, #[error("{0}")] Io(String) }
pub fn open(path: &std::path::Path) -> Result<String, OpenError>;   // read; reject NUL byte or invalid UTF-8 (repar is_binary)
pub fn save_atomic(path: &std::path::Path, content: &str) -> Result<(), SaveError>; // temp+fsync+rename+dirfsync; skip-unchanged; refuse symlink
```
Wire commands: `Command::Save` → if `path` is `Some`, `set status="Saving…"`, call `save_atomic`; on Ok `dirty=false`, status="Saved"; on Err status=the error message (buffer stays dirty). `Command::Quit` → if `dirty`, set a `pending_quit_confirm` flag + status "Unsaved changes — Ctrl+Q again to quit, Ctrl+S to save" (simple modal-via-status in 4a; a real modal dialog is Effort 5); a second `Quit` while pending sets `editor.quit=true`.

- [ ] **Step 1: Write failing tests** (tempfile via `std::env::temp_dir` + pid-unique name; no extra dep): `save_atomic` writes content and a re-`open` reads it back; `open` on a file containing a NUL byte returns `OpenError::Binary`; `save_atomic` skip-unchanged does not change mtime (write same content twice; assert second is a no-op — compare mtime or a returned "unchanged" signal). Saving clears `dirty`.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement, porting `atomic.rs`. `#![forbid(unsafe_code)]` — use `std::fs`/`std::os::unix` permission APIs (no `unsafe`). Unix-only is acceptable (§14.3); gate Unix-specific perms behind `#[cfg(unix)]` with a portable fallback that still does temp+rename.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(shell): file open (binary refusal) + atomic save`

---

### Task 12: crossterm event loop + terminal lifecycle + panic restore

**Files:** `wordcartel/src/term.rs`, `wordcartel/src/input.rs`, `wordcartel/src/main.rs`.

**Interfaces — Produces:**
- `term::TerminalGuard`: on `new()` enables raw mode + enters alt screen (`crossterm::terminal::enable_raw_mode`, `EnterAlternateScreen`) and returns a `ratatui::Terminal<CrosstermBackend<Stdout>>`; on `Drop` disables raw mode + leaves alt screen + shows cursor. Install a panic hook (once) that calls the same restore before chaining to the previous hook (§15.7 terminal-restore).
- `input::key_to_command(key: crossterm::event::KeyEvent) -> Option<Command>`: the CUA map (§12.3): printable (no/Shift modifier) → `InsertChar`; Enter→`InsertNewline`; Backspace→`Backspace`; Delete→`DeleteForward`; Left/Right/Up/Down→`Move{dir, extend: shift_held}`; Home/End→`Move{LineStart/LineEnd, extend}`; Ctrl+Z→`Undo`; Ctrl+Y (or Ctrl+Shift+Z)→`Redo`; Ctrl+C→`Copy`; Ctrl+X→`Cut`; Ctrl+V→`Paste`; Ctrl+S→`Save`; Ctrl+Q→`Quit`; F-key or Ctrl+\\→`CycleRenderMode` (pick an unused, terminal-safe combo per §12.4). Mouse/PageUp/word-nav → deferred (Effort 5 / later task).
- `main.rs` / `App::run(path)`: `open` the file (or empty buffer on error, status shows it); build `Editor` with the terminal size; loop: `derive::rebuild` (initial), `term.draw(|f| render(f, &editor))`, `crossterm::event::read()` (blocking is fine — synchronous 4a), translate via `key_to_command`, `commands::run`; on `Resize` update `view.area` + `rebuild`; exit when `editor.quit`. Restore via the guard's `Drop`.

- [ ] **Step 1: Write failing test** (headless slice — the loop's pure step): a `App::step(&mut editor, key, clock)` that does translate→run→derive→ensure_visible (no terminal), tested by feeding a sequence of synthetic `KeyEvent`s and asserting final buffer + caret + `quit`. Keep terminal IO out of `step` so it's testable; the real loop calls `step` then draws.
```rust
#[test]
fn step_processes_typing_and_quit() {
    let mut e = Editor::new_from_text("\n", None, (80,24));
    let clk = TestClock(0.into());
    for c in "hi".chars() { app::step(&mut e, key_char(c), &clk); }
    app::step(&mut e, key_ctrl('q'), &clk); // dirty -> pending confirm, not quit yet
    assert!(!e.quit);
    app::step(&mut e, key_ctrl('q'), &clk); // second -> quit
    assert!(e.quit);
    assert_eq!(e.document.buffer.to_string(), "hi\n");
}
```
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement `step`, `key_to_command`, `TerminalGuard`, and the real loop in `main`. The terminal-touching code (`TerminalGuard`, `event::read`, `term.draw`) is exercised by manual smoke test, not unit tests; keep it minimal so `step` carries the logic.
- [ ] **Step 4:** Run → PASS; then `cargo build` and a manual smoke run (`wcartel /tmp/x.md`) to confirm it launches, types, saves, quits, and restores the terminal. Record the manual check in the commit body.
- [ ] **Step 5:** Commit: `feat(shell): crossterm loop + terminal lifecycle + panic restore`

---

## Self-Review (completed during planning)

- **Spec coverage:** §10.1 cycle → Tasks 2/3/5/12; §10.2 Document/View/Editor split → Task 1; §10.4 bindings→commands→apply (static keymap; data-driven KeyTrie deferred to Effort 5) → Tasks 8–12; §3.2/§3.11 live-conceal + render modes → Tasks 3/5/10; §16 ColMap/Cursor consumption (no reimplementation) → Tasks 4/6/7; §3.9 draw-every-event + O(visible) → Task 12 (loop) / Task 3 (note: 4a uses `full_parse` in derive — incremental_update wired in 4b); §14.3 atomic save + §15.3 open/save errors → Task 11; §15.7 terminal restore (emergency dump + swap → 4b) → Task 12. **Deferred to Plan 4b:** worker/async-edges system + version-stamped discard; background save + `Saving…` off the hot path; swap/recovery file + emergency buffer dump; `filter` primitive; repar transforms; system-clipboard sync (arboard/OSC52); external-mod detection; `incremental_update` wiring in derive; word/doc/page navigation; mouse; wrap-guide ruler; real modal dialogs.
- **Reuse:** every editing/parse/layout/cursor operation calls the merged `wordcartel-core` (signatures table above); the only new core change is an optional `Range::new(anchor, head)` constructor (Task 9), added with its own core test.
- **Placeholder scan:** each task gives concrete files, exact core signatures, representative failing tests, and named implementation steps; the one deliberate simplification (derive uses `full_parse` not `incremental_update` in 4a) is called out with rationale and a 4b marker, not left vague.
- **Type consistency:** `Editor`/`Document`/`View`/`RenderMode` (Task 1) consumed unchanged by Tasks 2–12; `Command`/`CommandResult` (Task 8) extended by 9–12; `nav::*` offsets (Tasks 6/7) consumed by commands (8/9) and screen placement (4); `line_layouts` cache (Task 3) consumed by render (5) and nav (4/6/7).

## Completion

When all tasks are `- [x]` and `cargo test` (workspace) is green with no warnings, AND a manual smoke run confirms open/edit/navigate/select/save/quit with terminal restore: mark Plan 4a complete in the coverage ledger (add a 4a row; §10/§3.8/§14.3/§15.3 rows → ✅ for the synchronous shell). Then Plan 4b (async edges: worker system, background save + swap/recovery + emergency dump, filter primitive, repar transforms, system-clipboard sync, external-mod detection, incremental_update in derive, nav/mouse polish).
