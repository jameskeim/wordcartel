# WordStar Keymap Fidelity (Effort 9B) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the opt-in `wordstar` keymap preset a faithful WordStar control-diamond experience — fix the `^A`/`^F` word-motion bug, build the handful of editor commands faithful WordStar needs (`delete_line`, `delete_to_line_end`, `save_and_quit`, numbered bookmarks, `move_screen_top/bottom`, `scroll_line_up/down`), and wire the full `^Q`/`^K` prefix tables — without touching the `cua` default.

**Architecture:** Functional-core/imperative-shell. All work is in the `wordcartel` shell. New navigation/edit commands follow the existing `Command` enum + `commands::run()` + name-keyed registry pattern; keybindings are pure data in the `WORDSTAR` preset table. `save_and_quit` reuses the already-built version-armed quit-after-save mechanism via a small DRY factor. Numbered bookmarks reuse the existing edit-tracking `marks: BTreeMap<char, usize>` store. No `wordcartel-core` change.

**Tech Stack:** Rust, ratatui 0.30, crossterm. Spec: `docs/superpowers/specs/2026-06-27-wordcartel-09b-wordstar-keymap-design.md`.

## Global Constraints

- **Opt-in only:** keybinding changes live in the `WORDSTAR` table (`keymap.rs`); the `CUA` default is **untouched**.
- **New commands are first-class registry citizens** (palette-reachable, config-bindable in any preset). Menu category matches siblings (verified): `Some(MenuCategory::Edit)` for `delete_line`/`delete_to_line_end` (like `delete_word_*` at registry.rs:106); `Some(MenuCategory::File)` for `save_and_quit` (like `save`); `None` for nav/scroll/bookmark commands (like `move_*` and `set_mark`/`jump_to_mark`, which are `None`).
- **Prefix convention:** `^Q`/`^K` second key accepted **ctrl-held *or* plain** (two rows each) — **except** `^KM`/`^KJ`, which are **plain-only** (`ctrl-k m`/`ctrl-k j`) because `^M`=Enter/`^J`=LF are terminal-reserved. Bookmarks are digit-only.
- **`^K`/`^Q` are never bound as one-chord commands** (prefix-only; a one-chord binding would shadow sub-sequences).
- **Marks track normal edits** (via `Buffer::apply` → `change::map_pos`) but **not** undo/redo — matches existing mark behavior.
- TDD, frequent commits. Every commit ends with the trailers:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` / `Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6`

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `wordcartel/src/keymap.rs` | `^A`/`^F` fix (T1); full `WORDSTAR` table + chord-collision test (T7) | 1, 7 |
| `wordcartel/src/commands.rs` | `Command::DeleteLine`/`DeleteToLineEnd` + arms (T2); `Dir::ScreenTop`/`ScreenBottom` + Move-arm match (T5) | 2, 5 |
| `wordcartel/src/registry.rs` | register `delete_line`/`delete_to_line_end` (T2); `save_and_quit` (T3); 20 bookmark ids (T4); `move_screen_top/bottom` (T5); `scroll_line_up/down` (T6) | 2–6 |
| `wordcartel/src/save.rs` | factor `dispatch_save_and_quit(ctx)` (T3) | 3 |
| `wordcartel/src/app.rs` | refactor `PromptAction::SaveAndQuit` arm to call the factor (T3) | 3 |
| `wordcartel/src/marks.rs` | factor `set_char_mark`/`jump_char_mark`; `resolve_pending` reuses (T4) | 4 |
| `wordcartel/src/nav.rs` | `move_screen_top/bottom` + `last_fully_visible_line` (T5); `clamp_caret_into_view` (T6) | 5, 6 |

**Task order: 1 → 2 → 3 → 4 → 5 → 6 → 7.** Tasks 2–5 are independent command-builders; Task 6 depends on Task 5 (`clamp_caret_into_view` reuses screen-top/bottom); Task 7 binds everything and must run last (all target ids must exist). Task 1 is the standalone `^A`/`^F` fix (roadmap exec #0) and lands first.

---

## Task 1: `^A`/`^F` word-motion fix (standalone)

**Files:**
- Modify: `wordcartel/src/keymap.rs` (`WORDSTAR` table, the two word-nav rows ~line 317-319)
- Test: `wordcartel/src/keymap.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `move_word_left`/`move_word_right` (already registered), `parse_seq`, `Registry::builtins`, `build_keymap`, `Resolution`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn wordstar_ctrl_a_f_are_word_motions() {
        let cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] };
        let (t, w) = build_keymap(&cfg, &Registry::builtins());
        assert!(w.is_empty(), "no warnings: {w:?}");
        let a = parse_chord("ctrl-a").unwrap();
        let f = parse_chord("ctrl-f").unwrap();
        assert!(matches!(t.resolve(&[a]), Resolution::Command(CommandId("move_word_left"))), "^A = word-left");
        assert!(matches!(t.resolve(&[f]), Resolution::Command(CommandId("move_word_right"))), "^F = word-right");
    }
```

- [ ] **Step 2: Run — fails** (`^A` resolves to `move_left`). `cargo test -p wordcartel wordstar_ctrl_a_f`

- [ ] **Step 3: Fix the bindings.** In `keymap.rs` `WORDSTAR`, replace:
```rust
    // Word navigation (^A left-word, ^F right-word — best-effort to char motions)
    ("ctrl-a", "move_left"),
    ("ctrl-f", "move_right"),
```
with:
```rust
    // Word navigation (^A left-word, ^F right-word)
    ("ctrl-a", "move_word_left"),
    ("ctrl-f", "move_word_right"),
```

- [ ] **Step 4: Run** `cargo test -p wordcartel wordstar_ctrl_a_f` — PASS. Then `cargo test -p wordcartel --lib` — green (incl `both_presets_resolve_against_builtins`).

- [ ] **Step 5: Commit** `fix(9b): ^A/^F WordStar preset bind to word-motion (shipped bug)`

---

## Task 2: `delete_line` + `delete_to_line_end` commands

**Files:**
- Modify: `wordcartel/src/commands.rs` (`Command` enum ~line 49; `run()` match — add arms near `DeleteWord` ~line 537)
- Modify: `wordcartel/src/registry.rs` (register near `delete_word` registrations)
- Test: `wordcartel/src/commands.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `Command::DeleteLine`, `Command::DeleteToLineEnd`; registry ids `delete_line`, `delete_to_line_end`.
- Consumes: `nav::head`, `editor.active().document.buffer.{len, byte_to_line, line_to_byte, slice}`, `derive::total_logical_lines`, `ChangeSet`, `Edit`, `Transaction`, `Selection::single`, `EditKind`, `editor.apply`, `derive::rebuild`, `nav::ensure_visible`. (Mirror the `Command::DeleteWord` arm at commands.rs:537.)

- [ ] **Step 1: Write the failing tests** (mirror the existing delete-command test harness — `Editor::new_from_text`, `TestClock`, `run(Command::…, &mut e, &clk)`, `e.active().document.buffer.to_string()`)

```rust
    #[test]
    fn delete_line_removes_whole_line_including_newline() {
        let mut e = Editor::new_from_text("aaa\nbbb\nccc\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5); // in "bbb"
        run(Command::DeleteLine, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "aaa\nccc\n");
        assert_eq!(e.active().document.selection.primary().head, 4); // start of "ccc"
    }

    #[test]
    fn delete_line_last_line_without_trailing_newline_vanishes() {
        let mut e = Editor::new_from_text("aaa\nbbb", None, (40, 10)); // no trailing \n
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5); // in "bbb"
        run(Command::DeleteLine, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "aaa"); // preceding \n absorbed
    }

    #[test]
    fn delete_line_on_empty_trailing_line_removes_preceding_newline() {
        let mut e = Editor::new_from_text("aaa\n", None, (40, 10)); // logical lines: "aaa", ""
        let len = e.active().document.buffer.len();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(len); // phantom empty line
        run(Command::DeleteLine, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "aaa");
    }

    #[test]
    fn delete_line_single_line_empties_buffer() {
        let mut e = Editor::new_from_text("only line", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(3);
        run(Command::DeleteLine, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "");
        assert_eq!(e.active().document.selection.primary().head, 0);
    }

    #[test]
    fn delete_to_line_end_deletes_to_eol_keeps_newline() {
        let mut e = Editor::new_from_text("hello world\nnext\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5); // after "hello"
        run(Command::DeleteToLineEnd, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "hello\nnext\n");
    }

    #[test]
    fn delete_to_line_end_at_eol_is_noop() {
        let mut e = Editor::new_from_text("hello\nnext\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5); // at end of "hello"
        let before = e.active().document.version;
        let r = run(Command::DeleteToLineEnd, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "hello\nnext\n", "byte-identical");
        assert_eq!(e.active().document.version, before, "no changeset applied");
        assert!(matches!(r, CommandResult::Noop));
    }
```

> Confirm the real field/method names against the existing delete tests: `document.version`, `buffer.to_string()`, `Selection::single`. Adapt if they differ.

- [ ] **Step 2: Run — fails** (`Command::DeleteLine`/`DeleteToLineEnd` undefined). `cargo test -p wordcartel delete_line` then `cargo test -p wordcartel delete_to_line_end`

- [ ] **Step 3: Implement.** Add to the `Command` enum (near `DeleteWord`):
```rust
    DeleteLine,
    DeleteToLineEnd,
```
Add arms to `run()` (after the `DeleteWord` arm, mirroring its `ChangeSet`/`Edit`/`Transaction`/`apply` shape):
```rust
        Command::DeleteLine => {
            let head = nav::head(editor);
            let len = editor.active().document.buffer.len();
            if len == 0 { return CommandResult::Noop; }
            let (from, to) = {
                let buf = &editor.active().document.buffer;
                let total = derive::total_logical_lines(buf);
                let l = buf.byte_to_line(head);
                let start = buf.line_to_byte(l);
                let end = if l + 1 < total { buf.line_to_byte(l + 1) } else { len };
                if start == end {
                    // Empty line — the phantom final logical line that exists only because
                    // of a trailing '\n' (start == len). Remove the preceding newline so it
                    // disappears (Codex).
                    if start > 0 { (start - 1, end) } else { (start, end) }
                } else if end == len && buf.slice(len - 1..len) != "\n" {
                    // Final line with NO trailing newline → absorb the preceding newline too,
                    // so the line fully vanishes (slice returns String).
                    if start > 0 { (start - 1, end) } else { (start, end) }
                } else {
                    (start, end)
                }
            };
            if from == to { return CommandResult::Noop; }
            let cs = ChangeSet::delete(from..to, len);
            let edit = Edit { range: from..to, new_len: 0 };
            let txn = Transaction::new(cs).with_selection(Selection::single(from));
            editor.apply(txn, edit, EditKind::Other, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }

        Command::DeleteToLineEnd => {
            let head = nav::head(editor);
            let len = editor.active().document.buffer.len();
            let to = {
                let buf = &editor.active().document.buffer;
                let total = derive::total_logical_lines(buf);
                let l = buf.byte_to_line(head);
                let line_end = if l + 1 < total { buf.line_to_byte(l + 1) } else { len };
                // Keep the newline: stop before a trailing '\n' if present.
                if line_end > head && line_end <= len && line_end > 0 && buf.slice(line_end - 1..line_end) == "\n" {
                    line_end - 1
                } else {
                    line_end
                }
            };
            if head >= to { return CommandResult::Noop; } // at/after EOL → no empty changeset
            let cs = ChangeSet::delete(head..to, len);
            let edit = Edit { range: head..to, new_len: 0 };
            let txn = Transaction::new(cs).with_selection(Selection::single(head));
            editor.apply(txn, edit, EditKind::Other, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }
```
Register (registry.rs, near the `delete_word` registrations; `Some(MenuCategory::Edit)` to match the `delete_word_*` siblings — Codex):
```rust
        r.register("delete_line",         "Delete Line",        Some(MenuCategory::Edit), |c| run(c, Command::DeleteLine));
        r.register("delete_to_line_end",  "Delete to Line End", Some(MenuCategory::Edit), |c| run(c, Command::DeleteToLineEnd));
```

- [ ] **Step 4: Run** `cargo test -p wordcartel delete_line` + `cargo test -p wordcartel delete_to_line_end` + `cargo test -p wordcartel --lib` — green.

- [ ] **Step 5: Commit** `feat(9b): delete_line + delete_to_line_end commands`

---

## Task 3: `save_and_quit` (factor + command)

**Files:**
- Modify: `wordcartel/src/save.rs` (add `dispatch_save_and_quit`)
- Modify: `wordcartel/src/app.rs` (`PromptAction::SaveAndQuit` arm ~line 284 calls the factor)
- Modify: `wordcartel/src/registry.rs` (register `save_and_quit`)
- Test: `wordcartel/src/app.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `pub(crate) fn dispatch_save_and_quit(ctx: &mut crate::registry::Ctx)`; registry id `save_and_quit`.
- Consumes: `crate::save::dispatch_save`, `Ctx { editor, clock, executor, msg_tx }`, `editor.active().document.{version, path}`, `editor.prompt`, `editor.quit_after_save`, `editor.quit_after_save_at`, `clock.now_ms()`. (Mirror app.rs:284-295.)

- [ ] **Step 1: Write the failing tests** (harness copied verbatim from the existing `save_and_quit_*` tests at app.rs:2176/2196 — only the dispatch call differs: the **command** path `dispatch_save_and_quit(&mut ctx)` instead of the **prompt** path `resolve_prompt(PromptAction::SaveAndQuit, …)`)

```rust
    #[test]
    fn save_and_quit_command_arms_quit_after_save_like_prompt() {
        // The save_and_quit registry command must reach the SAME armed state as the
        // PromptAction::SaveAndQuit path (proves the DRY factor).
        use crate::editor::Editor;
        use crate::jobs::{Executor, InlineExecutor};
        let p = std::env::temp_dir().join(format!("wc-savequit-cmd-{}.md", std::process::id()));
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
        assert_eq!(e.quit_after_save, Some(1), "command path arms quit_after_save");
        assert!(!e.quit, "not yet — waiting for the save result");
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(e.quit, "matching save result triggers quit");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn save_and_quit_command_on_unnamed_buffer_does_not_arm() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("scratch\n", None, (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        {
            let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx.clone() };
            crate::save::dispatch_save_and_quit(&mut ctx);
        }
        assert_eq!(e.quit_after_save, None, "no path → not armed");
        assert!(!e.quit);
    }
```

> The block scope around the `Ctx` ends its `&mut e` borrow before the later `e.quit_after_save`/`apply_result` reads (mirrors how the prompt arm builds a transient `Ctx`).

- [ ] **Step 2: Run — fails** (`dispatch_save_and_quit` undefined). `cargo test -p wordcartel save_and_quit_command`

- [ ] **Step 3: Implement the factor.** In `save.rs`:
```rust
/// Save, then arm quit-after-save so the editor exits when the save completes.
/// Arms ONLY if a save job was actually dispatched (path present, no modal raised) —
/// otherwise leaves dispatch_save's status and stays open. Shared by the quit-confirm
/// prompt (PromptAction::SaveAndQuit) and the `save_and_quit` command (Effort 9B).
pub(crate) fn dispatch_save_and_quit(ctx: &mut crate::registry::Ctx) {
    let v = ctx.editor.active().document.version;
    dispatch_save(ctx);
    if ctx.editor.active().document.path.is_some() && ctx.editor.prompt.is_none() {
        ctx.editor.quit_after_save = Some(v);
        ctx.editor.quit_after_save_at = Some(ctx.clock.now_ms());
    }
}
```
Refactor the `app.rs` `PromptAction::SaveAndQuit` arm (~line 284) to reuse it (keep the prompt-dismissal, which is prompt-specific):
```rust
        PromptAction::SaveAndQuit => {
            editor.prompt = None; // dismiss the quit-confirm modal first
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
            crate::save::dispatch_save_and_quit(&mut ctx);
            return; // prompt handled; must NOT clear an external-mod modal
        }
```
Register the command (registry.rs, File menu, near `save`):
```rust
        r.register("save_and_quit", "Save and Quit", Some(MenuCategory::File), |c| {
            crate::save::dispatch_save_and_quit(c);
            CommandResult::Handled
        });
```

- [ ] **Step 4: Run** `cargo test -p wordcartel save_and_quit` (covers both the existing prompt tests and the new command tests) + `cargo test -p wordcartel --lib` — green.

- [ ] **Step 5: Commit** `feat(9b): save_and_quit command (factored from SaveAndQuit prompt arm)`

---

## Task 4: numbered bookmarks (marks helpers + 20 commands)

**Files:**
- Modify: `wordcartel/src/marks.rs` (factor `set_char_mark`/`jump_char_mark`; `resolve_pending` reuses)
- Modify: `wordcartel/src/registry.rs` (register 20 bookmark ids via a local macro)
- Test: `wordcartel/src/marks.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `pub fn set_char_mark(editor: &mut Editor, ch: char)` (inserts mark at caret; **no status**); `pub fn jump_char_mark(editor: &mut Editor, ch: char) -> bool` (jumps if found, returns whether found; **no status**, fold-aware + records jump-back); registry ids `set_bookmark_0..9`, `jump_bookmark_0..9`.
- Consumes: `nav::head`, `editor.active_mut().marks`, `record_jump`, `nav::clamp_snap`, `place_caret_visible`, `CaretPlace::UnfoldTo`, `derive::rebuild`, `nav::ensure_visible`. (The status string lives in each caller, so interactive marks keep "mark …" wording and bookmarks use "bookmark …".)

- [ ] **Step 1: Write the failing tests** (mirror existing `marks.rs` tests)

```rust
    #[test]
    fn bookmark_set_and_jump_round_trips() {
        let mut e = Editor::new_from_text("line0\nline1\nline2\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(6); // line1
        super::set_char_mark(&mut e, '3');
        assert_eq!(e.active().marks.get(&'3'), Some(&6));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert!(super::jump_char_mark(&mut e, '3'), "found");
        assert_eq!(e.active().document.selection.primary().head, 6);
    }

    #[test]
    fn jump_unset_bookmark_returns_false_and_does_not_move() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        assert!(!super::jump_char_mark(&mut e, '7'), "unset → false");
        assert_eq!(e.active().document.selection.primary().head, 2, "no move");
    }

    #[test]
    fn bookmark_shares_slot_with_interactive_char_mark() {
        let mut e = Editor::new_from_text("0123456789\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5);
        super::set_mark(&mut e);            // interactive
        super::resolve_pending(&mut e, '5'); // stores under '5'
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert!(super::jump_char_mark(&mut e, '5'), "bookmark 5 == char-mark '5'");
        assert_eq!(e.active().document.selection.primary().head, 5);
    }

    #[test]
    fn jump_bookmark_into_fold_reveals_target() {
        // Mirror jump_to_mark_into_fold_reveals_target with set_char_mark/jump_char_mark.
        let doc = "# Top\nintro\n## A\nbody1\nbody2\n## B\n";
        let mut e = Editor::new_from_text(doc, None, (80, 24));
        crate::derive::rebuild(&mut e);
        let a = doc.find("## A").unwrap();
        let target = doc.find("body2").unwrap();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(target);
        super::set_char_mark(&mut e, '1');
        e.active_mut().folds.toggle(a);
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert!(super::jump_char_mark(&mut e, '1'));
        assert_eq!(e.active().document.selection.primary().head, target);
        assert!(!e.active().folds.folded.contains(&a));
    }
```

- [ ] **Step 2: Run — fails** (`set_char_mark`/`jump_char_mark` undefined). `cargo test -p wordcartel bookmark`

- [ ] **Step 3: Factor the helpers.** In `marks.rs`, add the status-free cores and route `resolve_pending` through them:
```rust
/// Store a mark at the caret under `ch` (no status — caller sets wording).
/// Clears `sel_history` to match the interactive `set_mark` path (marks.rs:8) so a
/// numbered-bookmark set resets the expand-selection ladder identically (Codex).
pub fn set_char_mark(editor: &mut Editor, ch: char) {
    editor.active_mut().sel_history.clear();
    let at = nav::head(editor);
    editor.active_mut().marks.insert(ch, at);
}

/// Jump to mark `ch` if set (fold-aware, records jump-back). Returns whether it existed.
/// No status — caller sets wording.
pub fn jump_char_mark(editor: &mut Editor, ch: char) -> bool {
    editor.active_mut().sel_history.clear();
    let raw = editor.active().marks.get(&ch).copied();
    let Some(raw) = raw else { return false; };
    let pre = nav::head(editor);
    record_jump(editor.active_mut(), pre);
    let off = nav::clamp_snap(editor, raw);
    let off = place_caret_visible(editor, off, CaretPlace::UnfoldTo);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
    crate::derive::rebuild(editor);
    nav::ensure_visible(editor);
    true
}
```
Rewrite `resolve_pending`'s two arms to reuse them (keeping the existing "mark …" status wording):
```rust
pub fn resolve_pending(editor: &mut Editor, ch: char) {
    match editor.pending_mark.take() {
        Some(MarkPending::Set) => {
            set_char_mark(editor, ch);
            editor.status = format!("mark {ch} set");
        }
        Some(MarkPending::Jump) => {
            if jump_char_mark(editor, ch) {
                editor.status = format!("jumped to mark {ch}");
            } else {
                editor.status = format!("no mark {ch}");
            }
        }
        None => {}
    }
}
```

- [ ] **Step 4: Register the 20 commands** via a local macro in `registry.rs` (non-capturing closures with literal digits — `Handler` is a `fn` pointer, so a runtime loop can't capture `ch`):
```rust
        macro_rules! register_bookmarks {
            ($r:expr, $($d:literal => $ch:literal),+ $(,)?) => {$(
                $r.register(concat!("set_bookmark_", $d), concat!("Set Bookmark ", $d), None,
                    |c| { crate::marks::set_char_mark(c.editor, $ch);
                          c.editor.status = concat!("bookmark ", $d, " set").to_string();
                          CommandResult::Handled });
                $r.register(concat!("jump_bookmark_", $d), concat!("Jump to Bookmark ", $d), None,
                    |c| { if crate::marks::jump_char_mark(c.editor, $ch) {
                              c.editor.status = concat!("jumped to bookmark ", $d).to_string();
                          } else {
                              c.editor.status = concat!("no bookmark ", $d).to_string();
                          }
                          CommandResult::Handled });
            )+};
        }
        register_bookmarks!(r,
            "0" => '0', "1" => '1', "2" => '2', "3" => '3', "4" => '4',
            "5" => '5', "6" => '6', "7" => '7', "8" => '8', "9" => '9');
```

- [ ] **Step 5: Run** `cargo test -p wordcartel bookmark` + `cargo test -p wordcartel marks` (the existing `set_then_jump_mark_round_trips`/`jump_to_mark_into_fold_reveals_target` must still pass — `resolve_pending` behavior unchanged) + `cargo test -p wordcartel --lib` — green.

- [ ] **Step 6: Commit** `feat(9b): numbered bookmarks ^K0-9/^Q0-9 (shared char-mark store)`

---

## Task 5: viewport nav — `move_screen_top` / `move_screen_bottom`

**Files:**
- Modify: `wordcartel/src/commands.rs` (`Dir` enum ~line 30; `Command::Move` match ~line 363)
- Modify: `wordcartel/src/nav.rs` (add `move_screen_top`/`move_screen_bottom` + `last_fully_visible_line`)
- Modify: `wordcartel/src/registry.rs` (register `move_screen_top`/`move_screen_bottom`)
- Test: `wordcartel/src/nav.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `Dir::ScreenTop`, `Dir::ScreenBottom`; `pub fn move_screen_top(editor: &mut Editor) -> usize`; `pub fn move_screen_bottom(editor: &mut Editor) -> usize`; `pub(crate) fn last_fully_visible_line(editor: &Editor) -> usize`; registry ids `move_screen_top`/`move_screen_bottom`.
- Consumes: `head`, `move_up`/`move_down` (return new head, preserve `desired_col`), `editor.active().view.{scroll, scroll_row, area}`, `rows_of_line`, `fold_view().next_visible`, `editor.active().document.buffer.byte_to_line`. (Model `move_screen_top/bottom` on `move_page_up/down` at nav.rs:726 — a bounded `move_up/down` loop.)

- [ ] **Step 1: Write the failing tests** (a tall-enough doc so top≠bottom; set `view.scroll` and area explicitly)

```rust
    #[test]
    fn move_screen_top_lands_on_first_visible_line() {
        let mut e = Editor::new_from_text("l0\nl1\nl2\nl3\nl4\nl5\nl6\nl7\n", None, (20, 4));
        crate::derive::rebuild(&mut e);
        e.active_mut().view.scroll = 2;     // first visible logical line = l2
        e.active_mut().view.scroll_row = 0;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(e.active().document.buffer.line_to_byte(4)); // caret on l4
        let off = crate::nav::move_screen_top(&mut e);
        assert_eq!(e.active().document.buffer.byte_to_line(off), 2, "caret pulled to top visible line");
    }

    #[test]
    fn move_screen_bottom_lands_on_last_fully_visible_line() {
        let mut e = Editor::new_from_text("l0\nl1\nl2\nl3\nl4\nl5\nl6\nl7\n", None, (20, 4)); // editing height = 4-1 = 3
        crate::derive::rebuild(&mut e);
        e.active_mut().view.scroll = 1;     // visible logical lines l1,l2,l3 (height 3, no wrap)
        e.active_mut().view.scroll_row = 0;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(e.active().document.buffer.line_to_byte(1));
        let off = crate::nav::move_screen_bottom(&mut e);
        assert_eq!(e.active().document.buffer.byte_to_line(off), 3, "caret to last fully-visible line");
    }
```

> `view.area` is `(width, height)` (a tuple — use `area.1`, NOT `area.height`); the **editing** height reserves one row for the status bar (`(area.1 as usize).saturating_sub(1)`), so `(20, 4)` gives 3 visible rows. Adjust expected line indices if soft-wrap of the `lN` strings changes the visible-row math.

- [ ] **Step 2: Run — fails** (`move_screen_top` undefined). `cargo test -p wordcartel move_screen`

- [ ] **Step 3: Implement nav fns.** In `nav.rs`:
```rust
/// Last logical line whose final visual row still fits fully within the viewport,
/// walking visible (fold-aware) lines from (view.scroll, view.scroll_row).
pub(crate) fn last_fully_visible_line(editor: &Editor) -> usize {
    let (top, skip) = { let v = &editor.active().view; (v.scroll, v.scroll_row) };
    // view.area is (width, height); the editing region reserves one row for the
    // status bar (matches nav.rs:90/403/page_step).
    let height = (editor.active().view.area.1 as usize).saturating_sub(1);
    if height == 0 { return top; }
    let fv = fold_view(editor);
    let mut line = top;
    let mut used = 0usize;
    let mut last_full = top;
    let mut first = true;
    loop {
        let rows = rows_of_line(editor, line);
        let contrib = if first { rows.saturating_sub(skip) } else { rows };
        if used + contrib > height { break; }   // this line's last row is clipped
        used += contrib;
        last_full = line;
        first = false;
        match fv.next_visible(line) { Some(n) => line = n, None => break }
        if used >= height { break; }
    }
    last_full
}

/// Move the caret to a target logical line, column preserved, **bidirectionally**
/// (up or down depending on the current side). Bidirectional so it works both for
/// `^QE`/`^QX` (caret already on-screen) AND for the scroll caret-clamp (Task 6), where
/// the caret may be above OR below the viewport (Codex).
fn move_caret_to_line(editor: &mut Editor, target: usize) -> usize {
    let mut off = head(editor);
    loop {
        let cur = editor.active().document.buffer.byte_to_line(off);
        if cur == target { break; }
        let next = if cur > target { move_up(editor) } else { move_down(editor) };
        if next == off { break; } // hit doc bound
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(next);
        off = next;
    }
    off
}

/// Move the caret to the first visible logical line (view.scroll), column preserved.
pub fn move_screen_top(editor: &mut Editor) -> usize {
    let top = editor.active().view.scroll;
    move_caret_to_line(editor, top)
}

/// Move the caret to the last fully-visible logical line, column preserved.
pub fn move_screen_bottom(editor: &mut Editor) -> usize {
    let bottom = last_fully_visible_line(editor);
    move_caret_to_line(editor, bottom)
}
```
Add `Dir::ScreenTop, Dir::ScreenBottom` to the `Dir` enum (commands.rs) and to the `Command::Move` match (after `Dir::PageUp`/`PageDown`):
```rust
                Dir::ScreenTop     => nav::move_screen_top(editor),
                Dir::ScreenBottom  => nav::move_screen_bottom(editor),
```
Register (registry.rs, `None` menu like other `move_*`):
```rust
        r.register("move_screen_top",    "Move to Screen Top",    None, |c| run(c, Command::Move { dir: Dir::ScreenTop,    extend: false }));
        r.register("move_screen_bottom", "Move to Screen Bottom", None, |c| run(c, Command::Move { dir: Dir::ScreenBottom, extend: false }));
```

- [ ] **Step 4: Run** `cargo test -p wordcartel move_screen` + `cargo test -p wordcartel --lib` — green.

- [ ] **Step 5: Commit** `feat(9b): move_screen_top/bottom (^QE/^QX) viewport-relative nav`

---

## Task 6: `scroll_line_up` / `scroll_line_down` commands

**Files:**
- Modify: `wordcartel/src/nav.rs` (add `clamp_caret_into_view`)
- Modify: `wordcartel/src/registry.rs` (register `scroll_line_up`/`scroll_line_down`)
- Test: `wordcartel/src/nav.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `pub fn clamp_caret_into_view(editor: &mut Editor)`; registry ids `scroll_line_up`/`scroll_line_down`.
- Consumes: `scroll_up_one`/`scroll_down_one` (existing, viewport-only, by visual row), `move_screen_top`/`move_screen_bottom` (Task 5), `last_fully_visible_line`, `head`, `editor.active().view.scroll`, `buffer.byte_to_line`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn scroll_line_down_moves_viewport_and_keeps_caret_visible() {
        let mut e = Editor::new_from_text("l0\nl1\nl2\nl3\nl4\nl5\nl6\nl7\n", None, (20, 4));
        crate::derive::rebuild(&mut e);
        e.active_mut().view.scroll = 0;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0); // caret on l0
        crate::nav::scroll_line_down(&mut e); // viewport down one; l0 scrolls off-top
        assert!(e.active().view.scroll >= 1, "viewport advanced");
        // caret was on l0 (now above viewport) → nudged down to the new first visible line
        let caret_line = e.active().document.buffer.byte_to_line(e.active().document.selection.primary().head);
        assert!(caret_line >= e.active().view.scroll, "caret stays within viewport");
    }

    #[test]
    fn scroll_line_up_does_not_move_caret_when_still_visible() {
        let mut e = Editor::new_from_text("l0\nl1\nl2\nl3\nl4\nl5\nl6\nl7\n", None, (20, 4));
        crate::derive::rebuild(&mut e);
        e.active_mut().view.scroll = 4;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(e.active().document.buffer.line_to_byte(4));
        let before = e.active().document.selection.primary().head;
        crate::nav::scroll_line_up(&mut e); // viewport up; caret line still within view
        assert_eq!(e.active().document.selection.primary().head, before, "caret unmoved while visible");
    }
```

> `scroll_line_up`/`down` are thin wrappers exposed for testing; if the project prefers testing through the registry command, drive them via `run`/registry instead — but a direct `nav::scroll_line_*` fn keeps the test simple. Adjust expected scroll deltas to the real visual-row math.

- [ ] **Step 2: Run — fails** (`scroll_line_down` undefined). `cargo test -p wordcartel scroll_line`

- [ ] **Step 3: Implement.** In `nav.rs`:
```rust
/// After a viewport scroll, pull the caret back inside the visible range (WordStar
/// keeps the caret on screen). No-op if already visible. Relies on `move_screen_top`/
/// `move_screen_bottom` being **bidirectional** (Task 5): when the caret is ABOVE the
/// viewport (`cl < top`), `move_screen_top` moves it DOWN to `top`; when BELOW
/// (`cl > bottom`), `move_screen_bottom` moves it UP to `bottom`.
pub fn clamp_caret_into_view(editor: &mut Editor) {
    let top = editor.active().view.scroll;
    let bottom = last_fully_visible_line(editor);
    let cl = editor.active().document.buffer.byte_to_line(head(editor));
    if cl < top {
        move_screen_top(editor);
    } else if cl > bottom {
        move_screen_bottom(editor);
    }
}

/// WordStar ^W: scroll viewport up one row, keep caret visible.
pub fn scroll_line_up(editor: &mut Editor) {
    scroll_up_one(editor);
    clamp_caret_into_view(editor);
}

/// WordStar ^Z: scroll viewport down one row, keep caret visible.
pub fn scroll_line_down(editor: &mut Editor) {
    scroll_down_one(editor);
    clamp_caret_into_view(editor);
}
```
Register (registry.rs, `None` menu — view-only commands; they don't go through `Command`/`run`):
```rust
        r.register("scroll_line_up",   "Scroll Line Up",   None, |c| { crate::nav::scroll_line_up(c.editor);   CommandResult::Handled });
        r.register("scroll_line_down", "Scroll Line Down", None, |c| { crate::nav::scroll_line_down(c.editor); CommandResult::Handled });
```

- [ ] **Step 4: Run** `cargo test -p wordcartel scroll_line` + `cargo test -p wordcartel --lib` — green (existing `scroll_down_one_steps_over_hidden_lines` must still pass — `scroll_down_one` unchanged).

- [ ] **Step 5: Commit** `feat(9b): scroll_line_up/down (^W/^Z) viewport scroll, caret-stays-visible`

---

## Task 7: wire the full `WORDSTAR` table + chord-collision test

**Files:**
- Modify: `wordcartel/src/keymap.rs` (`WORDSTAR` table — full replacement; add the collision/prefix-shadow test)
- Test: `wordcartel/src/keymap.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: all ids registered in Tasks 1–6 + existing ids. `preset_bindings("wordstar")`, `parse_seq`, `Resolution`, `Registry::builtins`, `build_keymap`.

- [ ] **Step 1: Write the failing tests** (resolution of new chords + the dedicated collision/shadow guard)

```rust
    #[test]
    fn wordstar_new_chords_resolve() {
        let cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] };
        let (t, w) = build_keymap(&cfg, &Registry::builtins());
        assert!(w.is_empty(), "no warnings: {w:?}");
        let seq = |s: &str| parse_seq(s).unwrap();
        let cmd = |s: &str| t.resolve(&seq(s));
        // diamond extensions
        assert!(matches!(cmd("ctrl-r"), Resolution::Command(CommandId("move_page_up"))));
        assert!(matches!(cmd("ctrl-c"), Resolution::Command(CommandId("move_page_down"))));
        assert!(matches!(cmd("ctrl-w"), Resolution::Command(CommandId("scroll_line_up"))));
        assert!(matches!(cmd("ctrl-z"), Resolution::Command(CommandId("scroll_line_down"))));
        assert!(matches!(cmd("ctrl-y"), Resolution::Command(CommandId("delete_line"))));
        assert!(matches!(cmd("ctrl-t"), Resolution::Command(CommandId("delete_word_forward"))));
        assert!(matches!(cmd("ctrl-g"), Resolution::Command(CommandId("delete_forward"))));
        assert!(matches!(cmd("ctrl-u"), Resolution::Command(CommandId("undo"))));
        assert!(matches!(cmd("ctrl-shift-u"), Resolution::Command(CommandId("redo"))));
        // ^Q quick, both forms
        assert!(matches!(cmd("ctrl-q ctrl-s"), Resolution::Command(CommandId("move_line_start"))));
        assert!(matches!(cmd("ctrl-q s"),      Resolution::Command(CommandId("move_line_start"))));
        assert!(matches!(cmd("ctrl-q e"),      Resolution::Command(CommandId("move_screen_top"))));
        assert!(matches!(cmd("ctrl-q x"),      Resolution::Command(CommandId("move_screen_bottom"))));
        assert!(matches!(cmd("ctrl-q f"),      Resolution::Command(CommandId("find"))));
        assert!(matches!(cmd("ctrl-q y"),      Resolution::Command(CommandId("delete_to_line_end"))));
        assert!(matches!(cmd("ctrl-q 0"),      Resolution::Command(CommandId("jump_bookmark_0"))));
        assert!(matches!(cmd("ctrl-q 9"),      Resolution::Command(CommandId("jump_bookmark_9"))));
        // ^K block/file, both forms + bookmarks
        assert!(matches!(cmd("ctrl-k ctrl-s"), Resolution::Command(CommandId("save"))));
        assert!(matches!(cmd("ctrl-k s"),      Resolution::Command(CommandId("save"))));
        assert!(matches!(cmd("ctrl-k x"),      Resolution::Command(CommandId("save_and_quit"))));
        assert!(matches!(cmd("ctrl-k 5"),      Resolution::Command(CommandId("set_bookmark_5"))));
        // ^KM / ^KJ plain-only; the ctrl-form must NOT be bound
        assert!(matches!(cmd("ctrl-k m"), Resolution::Command(CommandId("set_mark"))));
        assert!(matches!(cmd("ctrl-k j"), Resolution::Command(CommandId("jump_to_mark"))));
        assert!(matches!(cmd("ctrl-k ctrl-m"), Resolution::None), "^KM ctrl-form reserved, not bound");
    }

    #[test]
    fn wordstar_has_no_chord_collisions_or_prefix_shadows() {
        let rows = preset_bindings("wordstar").unwrap();
        // (a) no duplicate chord maps to two ids
        let mut seen: std::collections::HashMap<Vec<KeyChord>, &str> = std::collections::HashMap::new();
        for (chord, id) in rows {
            let seq = parse_seq(chord).unwrap();
            if let Some(prev) = seen.insert(seq, id) {
                assert_eq!(prev, *id, "duplicate chord {chord} maps to {prev} AND {id}");
            }
        }
        // (b) no bound sequence is a strict prefix of another (would shadow it on exact-match)
        let seqs: Vec<Vec<KeyChord>> = rows.iter().map(|(c, _)| parse_seq(c).unwrap()).collect();
        for a in &seqs {
            for b in &seqs {
                if a.len() < b.len() && b.starts_with(a) {
                    panic!("chord {a:?} is a strict prefix of {b:?} — would shadow it");
                }
            }
        }
    }
```

- [ ] **Step 2: Run — fails** (new chords resolve to `None` / collision test may pass trivially until table grows). `cargo test -p wordcartel wordstar_new_chords` then `cargo test -p wordcartel wordstar_has_no_chord`

- [ ] **Step 3: Replace the `WORDSTAR` table** with the full faithful set:
```rust
static WORDSTAR: &[(&str, &str)] = &[
    // Cursor diamond
    ("ctrl-e", "move_up"),
    ("ctrl-x", "move_down"),
    ("ctrl-s", "move_left"),
    ("ctrl-d", "move_right"),
    ("ctrl-a", "move_word_left"),   // Task 1 fix
    ("ctrl-f", "move_word_right"),  // Task 1 fix
    ("ctrl-r", "move_page_up"),
    ("ctrl-c", "move_page_down"),
    ("ctrl-w", "scroll_line_up"),
    ("ctrl-z", "scroll_line_down"),
    // Delete / undo / redo
    ("ctrl-g", "delete_forward"),
    ("ctrl-t", "delete_word_forward"),
    ("ctrl-y", "delete_line"),
    ("ctrl-u", "undo"),
    ("ctrl-shift-u", "redo"),
    // ^Q "quick" prefix (ctrl-held OR plain second key)
    ("ctrl-q ctrl-s", "move_line_start"), ("ctrl-q s", "move_line_start"),
    ("ctrl-q ctrl-d", "move_line_end"),   ("ctrl-q d", "move_line_end"),
    ("ctrl-q ctrl-r", "move_doc_start"),  ("ctrl-q r", "move_doc_start"),
    ("ctrl-q ctrl-c", "move_doc_end"),    ("ctrl-q c", "move_doc_end"),
    ("ctrl-q ctrl-e", "move_screen_top"), ("ctrl-q e", "move_screen_top"),
    ("ctrl-q ctrl-x", "move_screen_bottom"), ("ctrl-q x", "move_screen_bottom"),
    ("ctrl-q ctrl-f", "find"),    ("ctrl-q f", "find"),
    ("ctrl-q ctrl-a", "replace"), ("ctrl-q a", "replace"),
    ("ctrl-q ctrl-l", "find_next"), ("ctrl-q l", "find_next"),
    ("ctrl-q ctrl-p", "jump_back"), ("ctrl-q p", "jump_back"),
    ("ctrl-q ctrl-y", "delete_to_line_end"), ("ctrl-q y", "delete_to_line_end"),
    ("ctrl-q 0", "jump_bookmark_0"), ("ctrl-q 1", "jump_bookmark_1"),
    ("ctrl-q 2", "jump_bookmark_2"), ("ctrl-q 3", "jump_bookmark_3"),
    ("ctrl-q 4", "jump_bookmark_4"), ("ctrl-q 5", "jump_bookmark_5"),
    ("ctrl-q 6", "jump_bookmark_6"), ("ctrl-q 7", "jump_bookmark_7"),
    ("ctrl-q 8", "jump_bookmark_8"), ("ctrl-q 9", "jump_bookmark_9"),
    // ^K "block/file" prefix (ctrl-held OR plain, except ^KM/^KJ plain-only)
    ("ctrl-k ctrl-s", "save"), ("ctrl-k s", "save"),
    ("ctrl-k ctrl-d", "save"), ("ctrl-k d", "save"),
    ("ctrl-k ctrl-x", "save_and_quit"), ("ctrl-k x", "save_and_quit"),
    ("ctrl-k ctrl-q", "quit"), ("ctrl-k q", "quit"),
    ("ctrl-k ctrl-c", "copy"),  ("ctrl-k c", "copy"),   // interim (9A reclaims ^KC for block copy)
    ("ctrl-k ctrl-v", "paste"), ("ctrl-k v", "paste"),  // interim (9A reclaims ^KV for block move)
    ("ctrl-k m", "set_mark"),       // plain-only (^M reserved)
    ("ctrl-k j", "jump_to_mark"),   // plain-only (^J reserved)
    ("ctrl-k 0", "set_bookmark_0"), ("ctrl-k 1", "set_bookmark_1"),
    ("ctrl-k 2", "set_bookmark_2"), ("ctrl-k 3", "set_bookmark_3"),
    ("ctrl-k 4", "set_bookmark_4"), ("ctrl-k 5", "set_bookmark_5"),
    ("ctrl-k 6", "set_bookmark_6"), ("ctrl-k 7", "set_bookmark_7"),
    ("ctrl-k 8", "set_bookmark_8"), ("ctrl-k 9", "set_bookmark_9"),
    // Kept modern keys (arrows / Home / End / Shift-select / editing)
    ("backspace", "backspace"),
    ("del",       "delete_forward"),
    ("enter",     "insert_newline"),
    ("left",  "move_left"),
    ("right", "move_right"),
    ("up",    "move_up"),
    ("down",  "move_down"),
    ("home",  "move_line_start"),
    ("end",   "move_line_end"),
    ("shift-left",  "select_left"),
    ("shift-right", "select_right"),
    ("shift-up",    "select_up"),
    ("shift-down",  "select_down"),
    ("shift-home",  "select_line_start"),
    ("shift-end",   "select_line_end"),
];
```

- [ ] **Step 4: Run** `cargo test -p wordcartel wordstar` (all wordstar tests incl `both_presets_resolve_against_builtins`, the new resolution test, and the collision/shadow test) then `cargo test -p wordcartel --lib` and `cargo test` (workspace) — all green.

- [ ] **Step 5: Commit** `feat(9b): wire full faithful WordStar preset table + chord-collision test`

---

## Final Verification
- [ ] `cargo test` (workspace) — all green.
- [ ] `cargo clippy -p wordcartel --lib` — no new warnings in touched files.
- [ ] Manual smoke (in `wordstar` preset): `^A`/`^F` move by word; `^QR`/`^QC` jump doc start/end; `^QE`/`^QX` jump screen top/bottom; `^W`/`^Z` scroll keeping caret on screen; `^Y` deletes a line, `^QY` deletes to EOL; `^U` undo / `ctrl-shift-u` redo; `^K5` set bookmark 5, `^Q5` jump to it (survives an edit; unfolds if folded); `^KX` saves then exits (stays open on no-filename); `^KS`/`^KD` save, `^KQ` quit. CUA preset unchanged.

## Self-Review Notes (coverage vs spec)
- §2 binding map → Task 7 (table) + Task 1 (the `^A`/`^F` fix). Both prefix forms + `^KM`/`^KJ` plain-only exception + removed stale `ctrl-z`/`ctrl-y` rows + interim `^KC`/`^KV` all present.
- §3.1 editing → Task 2 (delete_line byte-range edges; delete_to_line_end no empty changeset). §3.2 save_and_quit → Task 3 (reuse existing `quit_after_save`, factored). §3.3 screen nav → Task 5 (`Dir` variants; `last_fully_visible_line` for soft-wrap). §3.4 scroll → Task 6 (reuse `scroll_*_one` visual-row; caret-clamp). §3.5 bookmarks → Task 4 (factored helpers; macro-registered; shared slot; fold-aware).
- §5 testing → the collision/prefix-shadow guard (Task 7), edit edge cases (Task 2), screen-bottom soft-wrap exclusion (Task 5), caret-stays-visible (Task 6), command-vs-prompt arming parity (Task 3).
- Out of scope (not planned, per spec): block ops (9A), file-read/save-as (7), print/format/dot/help, overtype.
