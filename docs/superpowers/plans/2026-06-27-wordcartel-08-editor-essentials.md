# Editor Essentials (Effort 8) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three universal editor commands wordcartel lacks — **Select All**, **Go to line**, and an `Ln, Col` cursor-position status indicator.

**Architecture:** Functional-core/imperative-shell. A pure `caret_line_col` helper lands in `wordcartel-core` (buffer). `select_all` is a `Command` variant; `goto_line` opens the existing minibuffer overlay (extended with a `MinibufferKind` discriminant) and jumps on submit through the same fold-aware/jumplist caret path the other jumps use. The `Ln, Col` readout is a shell render-side segment that piggybacks the existing word-count status segment. Spec: `docs/superpowers/specs/2026-06-27-wordcartel-08-editor-essentials-design.md`.

**Tech Stack:** Rust, ratatui 0.30, crossterm, `unicode-segmentation` (already a `wordcartel-core` dep).

## Global Constraints

- `wordcartel-core` stays pure: no IO/ratatui/unsafe; `#![forbid(unsafe_code)]`. `caret_line_col` lives there (where `unicode-segmentation` is a dep — it is NOT a direct `wordcartel` dep).
- Commands register through the §10.4 name-keyed registry (`register(id, label, menu, handler)`), so they're palette-reachable in **every** preset. Keybindings added to the `cua` default only; **WordStar-preset bindings are deferred to Effort 9B**.
- **Column = 1-based source grapheme column**, counted over `line_to_byte(line)..caret` only (**O(line), never O(doc)**); view- and wrap-independent.
- **Go-to-line is a long-range jump:** record the jump origin (`marks::record_jump`) so `jump_back` returns; route the caret through `place_caret_visible(…, CaretPlace::UnfoldTo)` so a target inside a folded body unfolds.
- Commands that set the selection directly must **clear `desired_col` and `sel_history`** (match the normal movement/jump paths).
- `Ln, Col` rides the word-count status segment (shown only when `view_opts.word_count` is on); no separate toggle, no always-on segment.
- TDD, frequent commits. Every commit ends with the trailers:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` / `Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6`

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `wordcartel-core/src/buffer.rs` | pure `caret_line_col(&self, caret) -> (usize, usize)` (1-based line, 1-based grapheme col) | 1 |
| `wordcartel/src/commands.rs` | `Command::SelectAll` variant + `run()` arm | 2 |
| `wordcartel/src/registry.rs` | register `select_all` + `goto_line`; `cua` binds; `goto_line` opens the minibuffer | 2, 3 |
| `wordcartel/src/keymap.rs` | `cua` preset: `ctrl-a`→`select_all`, `ctrl-g`→`goto_line` | 2, 3 |
| `wordcartel/src/minibuffer.rs` | `MinibufferKind { Filter, GotoLine }` + `kind` field | 3 |
| `wordcartel/src/editor.rs` | `open_minibuffer(prompt, kind)` | 3 |
| `wordcartel/src/app.rs` | submit routes on `kind`; `goto_line_submit` jump logic | 3 |
| `wordcartel/src/render.rs` | prepend `Ln, Col` to the word-count right segment | 4 |

Task order: **1 → 2 → 3 → 4.** Task 4 consumes Task 1's helper; Tasks 2 and 3 are independent of each other.

---

## Task 1: Core — `caret_line_col` helper

**Files:**
- Modify: `wordcartel-core/src/buffer.rs` (add a method to the same `impl` as `byte_to_line`/`line_to_byte`, ~line 68)
- Test: same file `#[cfg(test)]`

**Interfaces:**
- Produces: `pub fn caret_line_col(&self, caret: BytePos) -> (usize, usize)` — returns `(1-based logical line, 1-based source grapheme column)`.
- Consumes: existing `self.byte_to_line`, `self.line_to_byte`, `self.slice(range)` (the slice helper used by `word_count`); `unicode_segmentation::UnicodeSegmentation`.

- [ ] **Step 1: Write the failing tests** (in `buffer.rs` `#[cfg(test)]`)

```rust
    #[test]
    fn caret_line_col_ascii() {
        let b = TextBuffer::from_str("abc\ndef\n");
        assert_eq!(b.caret_line_col(0), (1, 1));   // start of doc
        assert_eq!(b.caret_line_col(2), (1, 3));   // before 'c'
        assert_eq!(b.caret_line_col(4), (2, 1));   // start of line 2 ('d')
        assert_eq!(b.caret_line_col(6), (2, 3));   // before 'f'
    }

    #[test]
    fn caret_line_col_counts_graphemes_not_bytes() {
        // "aéb": 'é' is 2 bytes (U+00E9). Caret before 'b' is byte 3.
        let b = TextBuffer::from_str("aéb\n");
        assert_eq!(b.caret_line_col(3), (1, 3)); // graphemes a,é → col 3, NOT byte-4
    }

    #[test]
    fn caret_line_col_combining_cluster_is_one_column() {
        // "e\u{301}" = 'e' + combining acute = ONE grapheme (3 bytes), then 'x'.
        let b = TextBuffer::from_str("e\u{301}x\n");
        let before_x = "e\u{301}".len(); // byte offset of 'x'
        assert_eq!(b.caret_line_col(before_x), (1, 2)); // one grapheme before caret → col 2
    }
```

> Match the real constructor (`TextBuffer::from_str` per existing buffer tests — confirm the exact name/signature in this file and mirror it).

- [ ] **Step 2: Run — fails** (`caret_line_col` undefined). `cargo test -p wordcartel-core caret_line_col`

- [ ] **Step 3: Implement** (add to the `impl` with `byte_to_line`)

```rust
    /// 1-based logical line + 1-based **source grapheme column** of `caret`.
    /// The column counts grapheme clusters from the start of the caret's line
    /// (`line_to_byte(line)`) to `caret` — source position, NOT visual; so it is
    /// view- and wrap-independent. O(line): scans only the caret's line.
    pub fn caret_line_col(&self, caret: BytePos) -> (usize, usize) {
        use unicode_segmentation::UnicodeSegmentation;
        let line = self.byte_to_line(caret);
        let line_start = self.line_to_byte(line);
        let prefix = self.slice(line_start..caret).to_string(); // the line up to the caret
        let col = UnicodeSegmentation::graphemes(prefix.as_str(), true).count();
        (line + 1, col + 1)
    }
```

> If `self.slice(..)` already yields an owned `String`/`Cow<str>` you can drop `.to_string()` and take `.as_ref()` — match the real `slice` return type (the one `word_count_segment` passes). The behavior (grapheme count over the line prefix) is the contract.

- [ ] **Step 4: Run** `cargo test -p wordcartel-core caret_line_col` — PASS. Then `cargo test -p wordcartel-core` — green.

- [ ] **Step 5: Commit** `feat(8): core caret_line_col (1-based line + source grapheme column)`

---

## Task 2: `select_all` command

**Files:**
- Modify: `wordcartel/src/commands.rs` (`Command` enum ~line 49; `run()` match ~line 208)
- Modify: `wordcartel/src/registry.rs` (register, near `copy` ~line 116)
- Modify: `wordcartel/src/keymap.rs` (`cua` preset binds)
- Test: `wordcartel/src/commands.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `Command::SelectAll`; the `select_all` registry id.
- Consumes: `editor.active().document.buffer.len()`, `wordcartel_core::selection::Selection::range`, `nav::ensure_visible`, `editor.active_mut().{desired_col, sel_history}`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn select_all_selects_whole_buffer() {
        let mut e = Editor::new_from_text("hello\nworld\n", None, (40, 10));
        let len = e.active().document.buffer.len();
        run(Command::SelectAll, &mut e, &TestClock(0));
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (0, len));
        assert!(e.active().desired_col.is_none());
    }

    #[test]
    fn select_all_empty_buffer_is_noop_safe() {
        let mut e = Editor::new_from_text("", None, (40, 10));
        run(Command::SelectAll, &mut e, &TestClock(0));
        assert!(e.active().document.selection.primary().is_empty());
    }
```

> Mirror the existing command tests' harness (`Editor::new_from_text`, `TestClock`, the `run(Command::…, &mut e, &clk)` call shape).

- [ ] **Step 2: Run — fails** (no `Command::SelectAll`). `cargo test -p wordcartel select_all`

- [ ] **Step 3: Implement.** Add `SelectAll,` to the `Command` enum (~line 49). Add the arm to `run()`:

```rust
        Command::SelectAll => {
            let len = editor.active().document.buffer.len();
            editor.active_mut().document.selection =
                wordcartel_core::selection::Selection::range(0, len);
            editor.active_mut().desired_col = None;
            editor.active_mut().sel_history.clear();
            nav::ensure_visible(editor);
            CommandResult::Handled
        }
```

Register it (registry.rs, near `copy`):
```rust
        r.register("select_all", "Select All", Some(MenuCategory::Edit), |c| run(c, Command::SelectAll));
```
And add to the `cua` preset (keymap.rs `CUA` table): `("ctrl-a", "select_all"),`.

- [ ] **Step 4: Run** `cargo test -p wordcartel select_all` + `cargo test -p wordcartel --lib` — green. (The keymap preset-integrity test `both_presets_resolve_against_builtins` must still pass — `select_all` is now a real id.)

- [ ] **Step 5: Commit** `feat(8): Select All command (Ctrl+A, whole buffer)`

---

## Task 3: Go to line (`MinibufferKind` + `goto_line`)

**Files:**
- Modify: `wordcartel/src/minibuffer.rs` (`MinibufferKind` + `kind` field; fix the test literal ~line 61)
- Modify: `wordcartel/src/editor.rs` (`open_minibuffer(prompt, kind)` ~line 301; construct with `kind`)
- Modify: `wordcartel/src/registry.rs` (filter caller ~line 122 passes `Filter`; register `goto_line`)
- Modify: `wordcartel/src/keymap.rs` (`cua`: `ctrl-g`→`goto_line`)
- Modify: `wordcartel/src/app.rs` (submit routes on `kind`; add `goto_line_submit`; fix test callers ~2260/2283/2611)
- Modify: `wordcartel/src/render.rs` (fix test caller ~1119), `wordcartel/src/editor.rs` (fix test callers ~599/606)
- Test: `wordcartel/src/app.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `pub enum MinibufferKind { Filter, GotoLine }`; `Minibuffer.kind`; `open_minibuffer(&mut self, prompt: &str, kind: MinibufferKind)`; the `goto_line` registry id.
- Consumes: `derive::total_logical_lines`, `nav::head`, `marks::record_jump`, `registry::{place_caret_visible, CaretPlace::UnfoldTo}`, `Selection::single`, `nav::ensure_visible`.

- [ ] **Step 1: Write the failing tests** (app.rs `#[cfg(test)]`, mirroring the filter-submit test)

```rust
    #[test]
    fn goto_line_jumps_to_line_start_and_records_jumpback() {
        let mut e = Editor::new_from_text("one\ntwo\nthree\nfour\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        // start at end so the jump is a real move
        let end = e.active().document.buffer.len();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(end);
        e.open_minibuffer("Go to line: ", crate::minibuffer::MinibufferKind::GotoLine);
        // type "3" then Enter
        let (tx, _rx) = std::sync::mpsc::channel();
        for ch in "3".chars() { reduce(Msg::Input(Event::Key(KeyEvent{code:KeyCode::Char(ch),modifiers:KeyModifiers::NONE,kind:KeyEventKind::Press,state:KeyEventState::NONE})), &mut e, &Registry::builtins(), &cua_keymap(), &InlineExecutor::default(), &TestClock(0), &tx); }
        reduce(Msg::Input(Event::Key(KeyEvent{code:KeyCode::Enter,modifiers:KeyModifiers::NONE,kind:KeyEventKind::Press,state:KeyEventState::NONE})), &mut e, &Registry::builtins(), &cua_keymap(), &InlineExecutor::default(), &TestClock(0), &tx);
        // line 3 ("three") starts at byte 8
        assert_eq!(e.active().document.selection.primary().head, e.active().document.buffer.line_to_byte(2));
        assert!(e.minibuffer.is_none(), "submit closes the minibuffer");
    }

    #[test]
    fn goto_line_clamps_and_rejects_garbage() {
        let mut e = Editor::new_from_text("a\nb\nc\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        crate::app::goto_line_submit(&mut e, "999");          // clamp-high → last line
        let total = crate::derive::total_logical_lines(&e.active().document.buffer);
        assert_eq!(e.active().document.selection.primary().head, e.active().document.buffer.line_to_byte(total - 1));
        crate::app::goto_line_submit(&mut e, "0");            // clamp-low → line 1
        assert_eq!(e.active().document.selection.primary().head, 0);
        crate::app::goto_line_submit(&mut e, "xyz");          // garbage → status, no move
        assert_eq!(e.active().document.selection.primary().head, 0);
        assert_eq!(e.status, "not a line number");           // rejected input sets the status
    }
```

> Adapt the harness to the real `reduce(...)` test signature used by `minibuffer_routing_and_submit_dispatches_filter` (~app.rs:2256) — mirror its `Registry`/`keymap`/`Executor`/`Clock`/`tx` setup and the `cua_keymap()` helper. For the clamp test, call `goto_line_submit` directly (it's `pub(crate)`). Confirm `editor.status` is the real status field name/path and adjust if it differs.

- [ ] **Step 2: Run — fails** (`MinibufferKind`/`goto_line_submit` undefined). `cargo test -p wordcartel goto_line`

- [ ] **Step 3: Implement the minibuffer kind.** In `minibuffer.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinibufferKind { Filter, GotoLine }

#[derive(Debug, Clone)]
pub struct Minibuffer {
    pub prompt: String,
    pub text: String,
    pub cursor: usize,
    pub kind: MinibufferKind,   // NEW
}
```
Update the test literal (~line 61) to `Minibuffer { prompt: "> ".into(), text: String::new(), cursor: 0, kind: MinibufferKind::Filter }`.

- [ ] **Step 4: Thread `kind` through `open_minibuffer`.** In `editor.rs`:
```rust
    pub fn open_minibuffer(&mut self, prompt: &str, kind: crate::minibuffer::MinibufferKind) {
        // …existing XOR clears unchanged…
        self.minibuffer = Some(crate::minibuffer::Minibuffer {
            prompt: prompt.into(), text: String::new(), cursor: 0, kind,
        });
    }
```
Update EVERY caller (compiler-driven): production `registry.rs:122` (filter) → `c.editor.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);`; test callers `editor.rs:599/606`, `app.rs:2260/2283/2611`, `render.rs:1119` → add `crate::minibuffer::MinibufferKind::Filter`.

- [ ] **Step 5: Register `goto_line` + bind.** In `registry.rs` (near the `palette`/`find` registrations):
```rust
        r.register("goto_line", "Go to Line\u{2026}", Some(MenuCategory::View), |c| {
            c.editor.open_minibuffer("Go to line: ", crate::minibuffer::MinibufferKind::GotoLine);
            CommandResult::Handled
        });
```
Add to the `cua` `CUA` table: `("ctrl-g", "goto_line"),`.

- [ ] **Step 6: Route submit + the jump.** In `app.rs`, change the minibuffer `Enter` arm (~line 989):
```rust
                    crossterm::event::KeyCode::Enter => {
                        let mb = editor.minibuffer.take().unwrap();
                        match mb.kind {
                            crate::minibuffer::MinibufferKind::Filter   => submit_filter_line(editor, &mb.text, msg_tx),
                            crate::minibuffer::MinibufferKind::GotoLine => goto_line_submit(editor, &mb.text),
                        }
                    }
```
Add `goto_line_submit` near `submit_filter_line`:
```rust
/// Submit a minibuffer line as a go-to-line target (Effort 8). 1-based, clamped;
/// records a jump origin (jump-back), unfolds to the target, lands at column 1.
pub(crate) fn goto_line_submit(editor: &mut crate::editor::Editor, text: &str) {
    let n: usize = match text.trim().parse() {
        Ok(n) => n,
        Err(_) => { editor.status = "not a line number".to_string(); return; }
    };
    let total = crate::derive::total_logical_lines(&editor.active().document.buffer);
    let line_index = n.max(1).min(total) - 1;            // 1-based clamp → 0-based index
    let pre = crate::nav::head(editor);
    crate::marks::record_jump(editor.active_mut(), pre); // jump-back support
    let target = editor.active().document.buffer.line_to_byte(line_index);
    let caret = crate::registry::place_caret_visible(editor, target, crate::registry::CaretPlace::UnfoldTo);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(caret);
    editor.active_mut().desired_col = None;
    editor.active_mut().sel_history.clear();
    crate::nav::ensure_visible(editor);
}
```

- [ ] **Step 7: Run** `cargo test -p wordcartel goto_line minibuffer` + `cargo test -p wordcartel --lib` — green. The existing `minibuffer_routing_and_submit_dispatches_filter` test must still pass (it now constructs/opens with `Filter`).

- [ ] **Step 8: Commit** `feat(8): Go to line (Ctrl+G) — minibuffer kind, clamp, jumplist, fold-aware`

---

## Task 4: `Ln, Col` status indicator

**Files:**
- Modify: `wordcartel/src/render.rs` (the right-segment assembly, `if let Some(right) = word_count_segment(editor)` ~line 619)
- Test: `wordcartel/src/render.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `editor.active().document.buffer.caret_line_col` (Task 1), `nav::head`, `word_count_segment`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn status_shows_ln_col_when_word_count_on() {
        let mut e = Editor::new_from_text("hello\nworld\n", None, (60, 6));
        e.view_opts.word_count = true;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(8); // line 2, col 3 ("wo|rld")
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 60, 6);
        let status = row_string(&buf, 5); // bottom row
        assert!(status.contains("Ln 2, Col 3"), "got: {status}");
        assert!(status.contains("words"), "still shows the count: {status}");
    }

    #[test]
    fn status_hides_ln_col_when_word_count_off() {
        let mut e = Editor::new_from_text("hello\n", None, (60, 6));
        e.view_opts.word_count = false;
        crate::derive::rebuild(&mut e);
        let status = row_string(&render_to_buffer(&mut e, 60, 6), 5);
        assert!(!status.contains("Ln "), "position rides word-count; off → hidden: {status}");
    }

    #[test]
    fn ln_col_is_view_independent() {
        // same caret → same Ln,Col in LivePreview and SourcePlain (decision A)
        let mk = |mode| {
            let mut e = Editor::new_from_text("# Heading\n\n**bold** text\n", None, (60, 8));
            e.view_opts.word_count = true;
            e.active_mut().view.mode = mode;
            e.active_mut().document.selection = wordcartel_core::selection::Selection::single(14); // inside "**bold**"
            crate::derive::rebuild(&mut e);
            row_string(&render_to_buffer(&mut e, 60, 8), 7)
        };
        let live = mk(crate::editor::RenderMode::LivePreview);
        let src  = mk(crate::editor::RenderMode::SourcePlain);
        let pick = |s: &str| s.split_once("Ln ").map(|(_, r)| format!("Ln {}", r.split(" ·").next().unwrap_or(r))).unwrap_or_default();
        assert_eq!(pick(&live), pick(&src), "Ln,Col identical across views");
    }
```

> Use the real private render test helpers `render_to_buffer`/`row_string`. Adapt the caret byte offsets to the real strings; the assertions (Ln/Col present with word-count on, absent with it off, identical across views) are the contract.

- [ ] **Step 2: Run — fails** (no position in status). `cargo test -p wordcartel status_shows_ln_col status_hides_ln_col ln_col_is_view_independent`

- [ ] **Step 3: Implement.** In `render.rs`, in the right-segment branch (~line 619-620), prepend the position to the word-count text:
```rust
            if let Some(wc) = word_count_segment(editor) {
                let caret = crate::nav::head(editor);
                let (l, c) = editor.active().document.buffer.caret_line_col(caret);
                let right = format!("Ln {l}, Col {c} · {wc}");
                // …existing flush-right reserve/pad/truncate logic, now using `right`…
            }
```
Keep the rest of the flush-right + truncation logic exactly as-is (it already guards tiny terminals and is suppressed under overlays). Only the `right` string gains the `Ln {l}, Col {c} · ` prefix.

- [ ] **Step 4: Run** `cargo test -p wordcartel render:: status_shows status_hides ln_col` + `cargo test -p wordcartel --lib` — green. No other status/render golden should change (position only appears when word_count is on, which the existing render tests don't enable unless they assert the count).

- [ ] **Step 5: Commit** `feat(8): Ln,Col cursor-position status indicator (rides word-count segment)`

---

## Final Verification
- [ ] `cargo test` (workspace) — all green.
- [ ] `cargo clippy -p wordcartel-core -p wordcartel --lib` — no new warnings in touched files.
- [ ] Manual smoke: `Ctrl+A` selects the whole doc; `Ctrl+G` → type a line → caret lands at that line's start, `jump_back` (alt-left) returns; a `Ctrl+G` into a folded section unfolds it; with word-count on, the status shows `Ln N, Col N · …`; the Col is the same number in live-preview and source view.

## Self-Review Notes (coverage vs spec)
- §1 Select All → Task 2. §2 Go to line (minibuffer kind, clamp, jumplist, fold-unfold, side-state) → Task 3. §3 `Ln, Col` (source grapheme col, view-independent, word-count piggyback) → Tasks 1 + 4.
- All Codex spec-review folds present: fold-aware placement (T3 `place_caret_visible(UnfoldTo)`), `record_jump` (T3), `desired_col`/`sel_history` clear (T2 + T3), `derive::total_logical_lines` + clamp (T3), core `caret_line_col` + O(line) scan (T1), the `Minibuffer` literal test fix (T3 Step 3).
- Out of scope (not planned, per spec): `line:col`/relative goto, separate position toggle, selection-size readout, WordStar binds (9B).
