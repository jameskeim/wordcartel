# Wordcartel Effort 4b-1 — Async Substrate, Background Save & Command Registry — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move save off the keystroke path onto a general background-job substrate, wake the UI the instant a job finishes, migrate command dispatch onto a name-keyed registry, and fold in the four deferred 4a polish fixes — without changing the synchronous core.

**Architecture:** A shell-only `jobs` module provides a `Job`/`JobResult`/`Executor` contract (production `ThreadExecutor` = one worker thread + mpsc; test `InlineExecutor` = run-on-dispatch). The crossterm loop is rewritten around a single `mpsc::Receiver<Msg>` fed by an input thread, the worker's results, and a timer tick, so a finished job wakes the loop immediately. Save becomes a `JobKind::Save` job whose foreground `merge` updates status/`saved_version` version-awarely. Command dispatch becomes key→`CommandId`→`Handler`; built-in handlers delegate to the existing proven `commands::run` implementations so all 84 4a tests stay green.

**Tech Stack:** Rust 2021, `ropey` 1.6.1, `ratatui` 0.29, `crossterm` 0.28, `thiserror` 2, `std::thread` + `std::sync::mpsc` (no tokio), `proptest` (dev).

## Global Constraints

These bind every task (copied verbatim from spec §3 unless noted):

- **Responsiveness is #1** (§3.9): the foreground thread never blocks on IO. p95 keystroke < 16 ms. `status` is written *before* a job is dispatched (instant feedback), never after.
- **Functional core / imperative shell** (§10, §14.4): `wordcartel-core` stays IO-free and thread-free. All threading, IO, and OS calls live in the `wordcartel` shell crate.
- **Reconcile = discard, not rebase** (§10.3): stale job results (version moved on) are dropped. No OT/CRDT.
- **Single mutation channel** (§10.1): all **incremental document text / history** mutation flows through `editor.apply`. Handlers and job merges may touch non-document state directly; a job's only way to make an *incremental* edit to the **document** is by returning a `merge` whose body calls `editor.apply`. In 4b-1 every shipping `merge` (`Save`) touches only status/`saved_version`/fingerprint bookkeeping and never document text. **Sanctioned exception — whole-document replacement:** wholesale buffer replacement that also resets history (open, reload-from-disk, swap-recovery in 4b-2) constructs a *fresh* `Document` rather than routing through `apply`; there is no incremental delta and no history to map, so `apply` does not apply. These are the only paths permitted to assign `editor.document` directly, and each clears `view.line_layouts` + re-derives.
- **LF-only line semantics**: the shell counts lines by `\n` (and `\r\n`) only; bare `\r` and U+2028/U+2029 never split a line.
- **Plugin substrate (§18.4):** the job API must stay general enough to host a future plugin-invoked transform; the dispatch boundary must be key→ID→handler so the closed enum never becomes the extensibility boundary.
- **Workspace facts:** binary is `wcartel` (`wordcartel/src/main.rs`). Build/test the whole workspace with `cargo test` from the repo root. 4a baseline: 84 shell tests + 105 core + 34 oracle, all green. No test may be weakened to pass.

---

## File Structure

- `wordcartel/src/jobs.rs` *(new)* — `Job`, `JobResult`, `JobKind`, `Executor` trait, `InlineExecutor`, `ThreadExecutor`, worker lifecycle.
- `wordcartel/src/save.rs` *(new)* — background-save orchestration, `FileFingerprint`, the save `merge` builder.
- `wordcartel/src/registry.rs` *(new)* — `CommandId`, `Registry`, `Ctx`, default keymap, `dispatch`.
- `wordcartel/src/editor.rs` *(modify)* — replace `dirty: bool` with `saved_version: Option<u64>` + `Document::dirty()`; undo/redo no-op robustness; `stored_fp` field.
- `wordcartel/src/commands.rs` *(modify)* — CycleRenderMode `ensure_visible`; Copy-on-empty guard.
- `wordcartel/src/derive.rs` *(modify, indirectly)* — no code change; behavior shifts to LF-only via the ropey feature flip. Add a regression test.
- `wordcartel/src/app.rs` *(modify)* — unified-channel `run`, input thread, `reduce(Msg)` reducer, `apply_result`, timer plumbing.
- `wordcartel/src/input.rs` *(modify)* — `key_to_command` retargeted to produce `CommandId`s (kept as a thin shim over the registry keymap).
- `wordcartel/src/lib.rs` *(modify)* — declare `jobs`, `save`, `registry`.
- `wordcartel/Cargo.toml` and `wordcartel-core/Cargo.toml` *(modify)* — `ropey` `default-features = false, features = ["simd"]`.

---

## Task 1: LF-only line semantics (deferred 4a polish, spec §4.5)

**Why first:** lowest-risk, isolated, and unblocks nothing else but removes a latent correctness bug before the bigger refactors. ropey 1.6.1 with default features splits logical lines on bare `\r`, U+2028/29, VT, FF, NEL (verified empirically: all give `len_lines == 2`). Disabling the `unicode_lines` feature (which pulls in `cr_lines`) makes ropey's line APIs split on `\n`/`\r\n` only, matching core's `TextSource` contract. Cargo unifies features across the workspace, so the flip must happen in **both** crate manifests or the default re-enables it.

**Files:**
- Modify: `wordcartel/Cargo.toml` (the `ropey` line)
- Modify: `wordcartel-core/Cargo.toml` (the `ropey` line)
- Test: `wordcartel/src/derive.rs` (tests module)

**Interfaces:**
- Consumes: nothing.
- Produces: no API change. `derive::total_logical_lines` / `derive::line_start` / `derive::line_text` now treat only `\n` and `\r\n` as line breaks.

- [ ] **Step 1: Write the failing test** in `wordcartel/src/derive.rs`'s `#[cfg(test)] mod tests` (add if absent):

```rust
#[test]
fn unicode_line_breaks_do_not_split_logical_lines() {
    use crate::editor::Editor;
    // U+2028 (LINE SEPARATOR) and a bare CR must NOT create new logical lines.
    let e = Editor::new_from_text("a\u{2028}b\rc\n", None, (80, 24));
    // One real LF-terminated line of content + the empty trailing line = 2.
    assert_eq!(crate::derive::total_logical_lines(&e.document.buffer), 2);
    // The whole "a\u{2028}b\rc" is one logical line (its content, sans trailing \n).
    assert_eq!(crate::derive::line_text(&e.document.buffer, 0), "a\u{2028}b\rc");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel --lib derive::tests::unicode_line_breaks_do_not_split_logical_lines`
Expected: FAIL — `total_logical_lines` returns `5` (U+2028, CR, LF each split).

- [ ] **Step 3: Flip the ropey feature in both manifests.**

In `wordcartel-core/Cargo.toml`, change:
```toml
ropey = "=1.6.1"
```
to:
```toml
ropey = { version = "=1.6.1", default-features = false, features = ["simd"] }
```

In `wordcartel/Cargo.toml`, make the identical change to its `ropey` dependency line.

- [ ] **Step 4: Run the new test + the FULL suite to verify LF-only and no regression**

Run: `cargo test`
Expected: the new test PASSES; **all 84 shell + 105 core + 34 oracle tests still pass** (core's `TextSource` is already LF-only, so disabling unicode line breaks aligns ropey with it). If any core test regresses, it was asserting Unicode-line behavior that contradicts the LF-only contract — stop and report, do not weaken it.

- [ ] **Step 5: Commit**

```bash
git add wordcartel/Cargo.toml wordcartel-core/Cargo.toml wordcartel/src/derive.rs Cargo.lock
git commit -m "fix(shell): LF/CRLF-only logical lines (disable ropey unicode_lines)"
```

---

## Task 2: undo/redo no-op robustness (deferred 4a polish, spec §4.5)

**Problem:** `Editor::undo`/`redo` unconditionally bump `version`, set `dirty`, and (via the command layer) the caller resets `desired_col` — even when history is empty and nothing changed. An empty-history undo must be a true no-op: no version bump, no dirty, no `desired_col` reset.

**Files:**
- Modify: `wordcartel/src/editor.rs:112-132` (the `undo`/`redo` methods)
- Modify: `wordcartel/src/commands.rs` (the `Command::Undo`/`Command::Redo` arms, ~339-351)
- Test: `wordcartel/src/editor.rs` (tests module)

**Interfaces:**
- Consumes: nothing.
- Produces: `Editor::undo(&mut self) -> bool` and `Editor::redo(&mut self) -> bool` — return `true` iff something changed. (Was `-> ()`.)

- [ ] **Step 1: Write the failing test** in `wordcartel/src/editor.rs` tests module:

```rust
#[test]
fn undo_on_empty_history_is_true_noop() {
    let mut e = Editor::new_from_text("ab\n", None, (80, 24));
    let v0 = e.document.version;
    e.desired_col = Some(3);
    let changed = e.undo();
    assert!(!changed, "undo with empty history must report no change");
    assert_eq!(e.document.version, v0, "version must not move on a no-op undo");
    assert!(!e.document.dirty, "a no-op undo must not dirty the buffer");
    assert_eq!(e.desired_col, Some(3), "a no-op undo must not reset desired_col");
}
```

(Note: this test reads `e.document.dirty`. Task 4 converts `dirty` to a method; when that lands, update this assertion to `e.document.dirty()`. Until then the field exists.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel --lib editor::tests::undo_on_empty_history_is_true_noop`
Expected: FAIL — `version` advanced and `dirty == true`.

- [ ] **Step 3: Make undo/redo conditional** in `wordcartel/src/editor.rs`:

```rust
    /// Undo the last revision. Returns `true` iff the buffer changed.
    /// On a no-op (empty history) it leaves version/dirty/derive-hints untouched.
    pub fn undo(&mut self) -> bool {
        match self.document.history.undo(&mut self.document.buffer) {
            Some(sel) => {
                self.document.selection = sel;
                self.document.version += 1;
                self.document.dirty = true;
                self.last_edit = None;
                self.pre_edit_rope = None;
                true
            }
            None => false,
        }
    }

    /// Redo the next revision. Returns `true` iff the buffer changed.
    pub fn redo(&mut self) -> bool {
        match self.document.history.redo(&mut self.document.buffer) {
            Some(sel) => {
                self.document.selection = sel;
                self.document.version += 1;
                self.document.dirty = true;
                self.last_edit = None;
                self.pre_edit_rope = None;
                true
            }
            None => false,
        }
    }
```

- [ ] **Step 4: Guard the command arms** in `wordcartel/src/commands.rs` so a no-op undo/redo does not re-derive, reset `desired_col`, or report `Handled`:

```rust
        Command::Undo => {
            if !editor.undo() {
                return CommandResult::Noop;
            }
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.desired_col = None;
            CommandResult::Handled
        }

        Command::Redo => {
            if !editor.redo() {
                return CommandResult::Noop;
            }
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.desired_col = None;
            CommandResult::Handled
        }
```

- [ ] **Step 5: Run tests to verify pass (incl. existing undo/redo tests)**

Run: `cargo test -p wordcartel --lib editor:: commands::tests::undo`
Expected: PASS — new no-op test green; `undo_redo_round_trip`, `undo_command_restores_buffer`, `redo_command_reapplies_change`, `undo_redo_roundtrip_via_commands` still green.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/editor.rs wordcartel/src/commands.rs
git commit -m "fix(shell): undo/redo on empty history is a true no-op"
```

---

## Task 3: command-layer polish — CycleRenderMode visibility + Copy-on-empty guard (spec §4.5)

**Two independent 4a deferrals in `commands.rs`:** (a) a render-mode change can alter layout/scroll, so the caret must be re-revealed; (b) `Copy` with an empty selection currently overwrites the register with `""` — it must be a no-op that preserves the register.

**Files:**
- Modify: `wordcartel/src/commands.rs` (`Command::CycleRenderMode` arm ~353-361; `Command::Copy` arm ~279-284)
- Test: `wordcartel/src/commands.rs` (tests module)

**Interfaces:**
- Consumes: `nav::ensure_visible` (existing). `register::copy` (existing).
- Produces: no signature change; `Command::Copy` on an empty selection now returns `CommandResult::Noop`.

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/commands.rs` tests:

```rust
#[test]
fn copy_on_empty_selection_preserves_register() {
    let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
    // Pre-load the register with "seed".
    let mut src = Editor::new_from_text("seed\n", None, (80, 24));
    src.document.selection = Selection {
        ranges: [wordcartel_core::selection::Range { anchor: 0, head: 4 }].into_iter().collect(),
        primary: 0,
    };
    run(Command::Copy, &mut src, &TestClock(0));
    e.register = src.register;
    // Now Copy with a COLLAPSED selection must NOT clobber "seed" with "".
    set_caret(&mut e, 1);
    let r = run(Command::Copy, &mut e, &TestClock(0));
    assert_eq!(r, CommandResult::Noop, "Copy on empty selection is a no-op");
    assert_eq!(e.register.get(), Some("seed"), "register must be preserved");
}

#[test]
fn cycle_render_mode_keeps_caret_visible() {
    // A tall document scrolled so the caret sits near the bottom; toggling mode
    // must call ensure_visible so the caret stays on-screen. We assert the cheap
    // observable: the command re-runs ensure_visible without panicking and the
    // caret's logical line remains within the laid-out range.
    let mut e = Editor::new_from_text(&"x\n".repeat(100), None, (20, 5));
    set_caret(&mut e, 180); // deep into the doc
    derive::rebuild(&mut e);
    nav::ensure_visible(&mut e);
    let r = run(Command::CycleRenderMode, &mut e, &TestClock(0));
    assert_eq!(r, CommandResult::Handled);
    let caret_line = e.document.buffer.snapshot().byte_to_line(nav::head(&e));
    assert!(e.view.line_layouts.contains_key(&caret_line),
        "caret's logical line must be laid out (visible) after a mode change");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib commands::tests::copy_on_empty_selection_preserves_register commands::tests::cycle_render_mode_keeps_caret_visible`
Expected: FAIL — Copy clobbers the register with `""`; the caret line is not in `line_layouts` after a mode change (no `ensure_visible`).

- [ ] **Step 3: Add the Copy guard** in `wordcartel/src/commands.rs`:

```rust
        Command::Copy => {
            let r = editor.document.selection.primary();
            if r.is_empty() {
                // Copy-on-empty must NOT overwrite the register with "".
                return CommandResult::Noop;
            }
            register::copy(&editor.document.buffer, r, &mut editor.register);
            editor.status = "Copied".to_string();
            CommandResult::Handled
        }
```

- [ ] **Step 4: Add ensure_visible to CycleRenderMode** in `wordcartel/src/commands.rs`:

```rust
        Command::CycleRenderMode => {
            editor.view.mode = match editor.view.mode {
                RenderMode::LivePreview       => RenderMode::SourceHighlighted,
                RenderMode::SourceHighlighted => RenderMode::SourcePlain,
                RenderMode::SourcePlain       => RenderMode::LivePreview,
            };
            derive::rebuild(editor);
            nav::ensure_visible(editor); // a mode change can alter layout/scroll (§4.5)
            CommandResult::Handled
        }
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p wordcartel --lib commands::tests`
Expected: PASS — both new tests green; `cycle_render_mode_rotates_through_modes` and `source_highlighted_makes_inactive_heading_show_raw` still green.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/commands.rs
git commit -m "fix(shell): CycleRenderMode re-reveals caret; Copy-on-empty preserves register"
```

---

## Task 4: saved_version / dirty model (spec §4.3)

**Replace the drift-prone `dirty: bool` flag with `saved_version: Option<u64>`.** "Is there unsaved work?" becomes a pure function of versions: `dirty == (Some(version) != saved_version)`. `None` means never saved (new/scratch). This is a behavior-preserving refactor: every place that read/wrote `document.dirty` is updated to the derived method / to set `saved_version`.

**Files:**
- Modify: `wordcartel/src/editor.rs` (`Document` struct + `apply`/`undo`/`redo` + `new_from_text`)
- Modify: `wordcartel/src/commands.rs` (`Command::Save` and `Command::Quit` arms)
- Modify: `wordcartel/src/app.rs` (the new-file branch sets status, not dirty — already fine; verify no `dirty` writes)
- Test: `wordcartel/src/editor.rs`, and update existing `dirty`-reading tests across the crate.

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `Document.saved_version: Option<u64>` (replaces `dirty: bool`).
  - `Document::dirty(&self) -> bool { Some(self.version) != self.saved_version }`.
  - `Document::mark_saved(&mut self, v: u64) { self.saved_version = Some(v); }`.
  - `Editor::apply`/`undo`/`redo` no longer set any dirty flag — dirtiness is implied by `version != saved_version`.

- [ ] **Step 1: Write the failing test** in `wordcartel/src/editor.rs` tests:

```rust
#[test]
fn dirty_is_a_function_of_versions() {
    let mut e = Editor::new_from_text("ab\n", None, (80, 24));
    assert!(!e.document.dirty(), "fresh buffer (saved_version=Some(0)) is clean");
    let clk = TestClock(std::cell::Cell::new(0));
    let cs = wordcartel_core::change::ChangeSet::insert(1, "X", e.document.buffer.len());
    e.apply(
        Transaction::new(cs).with_selection(Selection::single(2)),
        wordcartel_core::block_tree::Edit { range: 1..1, new_len: 1 },
        EditKind::Type, &clk,
    );
    assert!(e.document.dirty(), "after an edit, version != saved_version → dirty");
    e.document.mark_saved(e.document.version);
    assert!(!e.document.dirty(), "after mark_saved at current version → clean");
}
```

- [ ] **Step 2: Run test to verify it fails (does not compile yet)**

Run: `cargo test -p wordcartel --lib editor::tests::dirty_is_a_function_of_versions`
Expected: FAIL — `dirty()` / `mark_saved` do not exist.

- [ ] **Step 3: Change the `Document` struct and constructor** in `wordcartel/src/editor.rs`. Replace `pub dirty: bool,` with `pub saved_version: Option<u64>,` and add methods:

```rust
#[derive(Debug, Clone)]
pub struct Document {
    pub buffer: TextBuffer,
    pub selection: Selection,
    pub history: History,
    pub blocks: BlockTree,
    pub version: u64,
    pub path: Option<PathBuf>,
    /// The document version last written to disk. `None` = never saved
    /// (new/scratch). `dirty()` is derived from this — no separate flag.
    pub saved_version: Option<u64>,
}

impl Document {
    /// Unsaved-work predicate (spec §4.3): clean iff the on-disk version
    /// equals the current version.
    pub fn dirty(&self) -> bool {
        Some(self.version) != self.saved_version
    }
    /// Record that version `v` is now on disk.
    pub fn mark_saved(&mut self, v: u64) {
        self.saved_version = Some(v);
    }
}
```

In `new_from_text`, set the initial state clean at version 0: replace `dirty: false,` with `saved_version: Some(0),`.

- [ ] **Step 4: Drop the `dirty = true` writes** in `apply`, `undo`, `redo` (they are now redundant — dirtiness is derived). In `Editor::apply` remove the line `self.document.dirty = true;`. In the `Some(sel)` arms of `undo`/`redo` (from Task 2) remove the `self.document.dirty = true;` lines. (Version still bumps, so `dirty()` becomes true automatically.)

- [ ] **Step 5: Update the Save and Quit arms** in `wordcartel/src/commands.rs`. Save records the saved version; Quit reads `dirty()`:

```rust
        Command::Save => {
            match &editor.document.path {
                None => {
                    editor.status = "No file name (save-as is Effort 5)".to_string();
                }
                Some(p) => {
                    let path = p.clone();
                    let v = editor.document.version;
                    editor.status = "Saving\u{2026}".to_string();
                    let content = editor.document.buffer.to_string();
                    match file::save_atomic(&path, &content) {
                        Ok(file::SaveOutcome::Saved) => {
                            editor.document.mark_saved(v);
                            editor.status = "Saved".to_string();
                        }
                        Ok(file::SaveOutcome::Unchanged) => {
                            editor.document.mark_saved(v);
                            editor.status = "(unchanged)".to_string();
                        }
                        Err(e) => { editor.status = e.to_string(); }
                    }
                }
            }
            CommandResult::Handled
        }

        Command::Quit => {
            if editor.document.dirty() && !editor.pending_quit {
                editor.pending_quit = true;
                editor.status =
                    "Unsaved changes \u{2014} Ctrl+Q again to quit, Ctrl+S to save".to_string();
                CommandResult::Handled
            } else {
                editor.quit = true;
                CommandResult::Quit
            }
        }
```

- [ ] **Step 6: Update every other `document.dirty` reader/writer to the method/`mark_saved`.** Search and fix:

Run: `rg -n "document\.dirty\b|\.dirty\s*=|\.dirty," wordcartel/src`
Update each call site:
- `wordcartel/src/commands.rs` tests: `assert!(!e.document.dirty)` → `assert!(!e.document.dirty())`; `e.document.dirty = true;` (Save test setup) → leave the buffer naturally dirty by editing, or set `e.document.saved_version = None;`.
- `wordcartel/src/file.rs` test `save_command_clears_dirty`: `e.document.dirty = true;` → `e.document.saved_version = None;`, and `assert!(!e.document.dirty, …)` → `assert!(!e.document.dirty(), …)`.
- `wordcartel/src/editor.rs` tests: `assert!(e.document.dirty)` / `assert!(!e.document.dirty)` → `…dirty()`.
- The Task 2 test's `assert!(!e.document.dirty)` → `assert!(!e.document.dirty())`.

These are mechanical, behavior-preserving edits (same assertion, derived accessor).

- [ ] **Step 7: Run the FULL suite**

Run: `cargo test`
Expected: PASS — the new test plus all previously-green tests (now reading `dirty()`).

- [ ] **Step 8: Commit**

```bash
git add wordcartel/src/editor.rs wordcartel/src/commands.rs wordcartel/src/file.rs
git commit -m "refactor(shell): replace dirty bool with saved_version/dirty() model"
```

---

## Task 5: job substrate types + InlineExecutor (spec §4.1)

**The general, plugin-ready job contract** plus the deterministic test executor. No real threads here — `InlineExecutor` runs the job on `dispatch` and buffers the result for `drain`, mirroring 4a's `Clock` injection pattern.

**Files:**
- Create: `wordcartel/src/jobs.rs`
- Modify: `wordcartel/src/lib.rs` (add `pub mod jobs;`)
- Test: in `wordcartel/src/jobs.rs`

**Interfaces:**
- Produces (normative names from spec §4.1):
  - `pub struct Job { pub version: u64, pub kind: JobKind, pub run: Box<dyn FnOnce() -> JobResult + Send> }`
  - `pub struct JobResult { pub version: u64, pub kind: JobKind, pub merge: Box<dyn FnOnce(&mut Editor) + Send> }`
  - `pub enum JobKind { Save, SwapWrite, #[cfg(test)] CoalesceProbe }` (derives `Clone, Copy, PartialEq, Eq, Hash, Debug`)
  - `pub trait Executor { fn dispatch(&self, job: Job); fn drain(&self) -> Vec<JobResult>; }`
  - `pub struct InlineExecutor { … }` implementing `Executor` + `Default`.
  - `pub fn is_stale(kind: JobKind, result_version: u64, current_version: u64) -> bool` — one-shot kinds (`Save`, `SwapWrite`) are never stale; coalescible kinds are stale when versions differ. This is the single staleness predicate both executors and the loop consult.

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/jobs.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    #[test]
    fn inline_executor_runs_on_dispatch_and_buffers_for_drain() {
        let ex = InlineExecutor::default();
        ex.dispatch(Job {
            version: 1,
            kind: JobKind::Save,
            run: Box::new(|| JobResult {
                version: 1,
                kind: JobKind::Save,
                merge: Box::new(|e: &mut Editor| e.status = "merged".into()),
            }),
        });
        let mut results = ex.drain();
        assert_eq!(results.len(), 1);
        assert!(ex.drain().is_empty(), "drain must consume buffered results");
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        (results.remove(0).merge)(&mut e);
        assert_eq!(e.status, "merged");
    }

    #[test]
    fn one_shot_kinds_are_never_stale() {
        assert!(!is_stale(JobKind::Save, 1, 99));
        assert!(!is_stale(JobKind::SwapWrite, 1, 99));
    }

    #[test]
    fn coalescible_kind_is_stale_when_version_moved() {
        assert!(is_stale(JobKind::CoalesceProbe, 1, 2));
        assert!(!is_stale(JobKind::CoalesceProbe, 2, 2));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib jobs::tests`
Expected: FAIL — module `jobs` does not exist.

- [ ] **Step 3: Declare the module** in `wordcartel/src/lib.rs` by adding `pub mod jobs;` after `pub mod file;`.

- [ ] **Step 4: Write `wordcartel/src/jobs.rs`:**

```rust
//! General background-job substrate (spec §4.1). Shell-only: the core stays
//! thread-free. One worker thread (production) gives FIFO result ordering for
//! free; `InlineExecutor` gives deterministic, thread-free tests.

use std::cell::RefCell;
use crate::editor::Editor;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum JobKind {
    Save,      // one-shot, user-initiated: always applies
    SwapWrite, // one-shot housekeeping: always applies (status only)
    #[cfg(test)]
    CoalesceProbe, // test-only stand-in for a future coalescible kind
}

/// A unit of background work, dispatched for a document version, run on a
/// worker, merged back on the foreground.
pub struct Job {
    pub version: u64,
    pub kind: JobKind,
    /// Runs on the worker thread; must not touch the Editor directly.
    pub run: Box<dyn FnOnce() -> JobResult + Send>,
}

/// What a worker hands back: its own foreground merge effect.
pub struct JobResult {
    pub version: u64,
    pub kind: JobKind,
    /// Applied on the foreground before the next draw. By contract this touches
    /// only non-document bookkeeping; any document-text change must route
    /// through `editor.apply`.
    pub merge: Box<dyn FnOnce(&mut Editor) + Send>,
}

/// The single staleness predicate (spec §4.1 staleness policy).
pub fn is_stale(kind: JobKind, result_version: u64, current_version: u64) -> bool {
    match kind {
        JobKind::Save | JobKind::SwapWrite => false, // one-shot: always applies
        #[cfg(test)]
        JobKind::CoalesceProbe => result_version != current_version,
    }
}

pub trait Executor {
    /// Enqueue a job for the worker.
    fn dispatch(&self, job: Job);
    /// Non-blocking: collect any results ready now (consumes them).
    fn drain(&self) -> Vec<JobResult>;
}

/// Deterministic test executor: runs `job.run()` immediately on `dispatch`,
/// buffers the result for `drain`. No threads, no flake.
#[derive(Default)]
pub struct InlineExecutor {
    pending: RefCell<Vec<JobResult>>,
}

impl Executor for InlineExecutor {
    fn dispatch(&self, job: Job) {
        let result = (job.run)();
        self.pending.borrow_mut().push(result);
    }
    fn drain(&self) -> Vec<JobResult> {
        self.pending.borrow_mut().drain(..).collect()
    }
}
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p wordcartel --lib jobs::tests`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/jobs.rs wordcartel/src/lib.rs
git commit -m "feat(jobs): job/JobResult/JobKind contract + InlineExecutor + staleness predicate"
```

---

## Task 6: ThreadExecutor + worker thread (spec §4.1)

**The production executor:** a single worker thread consuming an mpsc `job_rx`, pushing each `JobResult` onto a result channel whose `Sender` is cloned into the unified `Msg` channel later (Task 8). A single worker gives FIFO ordering for free. `drain` is a non-blocking `try_recv` loop. One focused integration test exercises the real thread with a deterministic handshake (no sleeps, no timing).

**Files:**
- Modify: `wordcartel/src/jobs.rs` (add `ThreadExecutor`)
- Test: `wordcartel/src/jobs.rs`

**Interfaces:**
- Consumes: `Job`, `JobResult`, `Executor` (Task 5).
- Produces:
  - `pub struct ThreadExecutor { job_tx: Sender<Job>, result_rx: Receiver<JobResult>, _worker: JoinHandle<()> }`
  - `pub fn new() -> (ThreadExecutor, Sender<JobResult>)` — returns the executor **and a clone of the result `Sender`**, so Task 8 can hand the worker a sender that also feeds the unified `Msg` channel. (Implementation detail: internally the worker sends on a `Sender<JobResult>`; `new` keeps one `Receiver` for `drain` and exposes a `Sender` clone for the loop's forwarder.)
  - On `Drop`, the `job_tx` is dropped → the worker's `recv()` returns `Err` → the worker thread exits; `Drop` joins it.

- [ ] **Step 1: Write the failing test** in `wordcartel/src/jobs.rs` tests:

```rust
#[test]
fn thread_executor_runs_job_on_worker_and_drains_result() {
    use std::sync::mpsc;
    // Deterministic handshake: the job signals completion on a channel the test
    // blocks on — no sleeps, no timing assumptions.
    let (done_tx, done_rx) = mpsc::channel::<u64>();
    let (ex, _forward) = ThreadExecutor::new();
    ex.dispatch(Job {
        version: 7,
        kind: JobKind::Save,
        run: Box::new(move || {
            done_tx.send(7).unwrap();
            JobResult {
                version: 7,
                kind: JobKind::Save,
                merge: Box::new(|e: &mut crate::editor::Editor| e.status = "worker".into()),
            }
        }),
    });
    assert_eq!(done_rx.recv().unwrap(), 7, "worker must run the job");
    // The result is now en route; block until drain sees it.
    let mut results = Vec::new();
    while results.is_empty() {
        results = ex.drain();
    }
    assert_eq!(results[0].version, 7);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel --lib jobs::tests::thread_executor_runs_job_on_worker_and_drains_result`
Expected: FAIL — `ThreadExecutor` does not exist.

- [ ] **Step 3: Add `ThreadExecutor`** to `wordcartel/src/jobs.rs`:

```rust
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

/// Production executor: one worker thread, FIFO. The worker pushes each
/// `JobResult` onto `result_tx`; `new` returns a clone of that sender so the
/// main loop can forward results into its unified `Msg` channel (Task 8).
pub struct ThreadExecutor {
    job_tx: Option<Sender<Job>>,
    result_rx: Receiver<JobResult>,
    worker: Option<JoinHandle<()>>,
}

impl ThreadExecutor {
    pub fn new() -> (ThreadExecutor, Sender<JobResult>) {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<JobResult>();
        let forward = result_tx.clone();
        let worker = std::thread::Builder::new()
            .name("wcartel-jobs".into())
            .spawn(move || {
                // FIFO: process jobs in dispatch order. Exit when job_tx drops.
                while let Ok(job) = job_rx.recv() {
                    let result = (job.run)();
                    // If the result receiver is gone, the app is shutting down.
                    if result_tx.send(result).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn jobs worker");
        (
            ThreadExecutor { job_tx: Some(job_tx), result_rx, worker: Some(worker) },
            forward,
        )
    }
}

impl Executor for ThreadExecutor {
    fn dispatch(&self, job: Job) {
        if let Some(tx) = &self.job_tx {
            // A send failure means the worker died; the next drain will surface
            // nothing and the UI stays responsive. Dropping the job is safe.
            let _ = tx.send(job);
        }
    }
    fn drain(&self) -> Vec<JobResult> {
        let mut out = Vec::new();
        while let Ok(r) = self.result_rx.try_recv() {
            out.push(r);
        }
        out
    }
}

impl Drop for ThreadExecutor {
    fn drop(&mut self) {
        // Drop job_tx so the worker's recv() returns Err and the loop exits.
        self.job_tx = None;
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p wordcartel --lib jobs::tests`
Expected: PASS — 4 tests (the thread test plus the 3 from Task 5).

- [ ] **Step 5: Commit**

```bash
git add wordcartel/src/jobs.rs
git commit -m "feat(jobs): ThreadExecutor (single FIFO worker, drainable, joined on drop)"
```

---

## Task 7: command registry — CommandId, Ctx, keymap migration (spec §4.4, §10.4)

**Migrate dispatch to key→`CommandId`→`Handler`** so the closed `Command` enum stops being the extensibility boundary. To keep all 84 tests green with zero churn, built-in handlers **delegate to the existing `commands::run(Command, …)` implementations** — the enum becomes shared built-in implementation, not the dispatch seam. `Ctx` bundles the editor, clock, and executor, so handlers that will dispatch jobs (save, in Task 9) have what they need.

**Files:**
- Create: `wordcartel/src/registry.rs`
- Modify: `wordcartel/src/lib.rs` (add `pub mod registry;`)
- Modify: `wordcartel/src/input.rs` (add `key_to_command_id` producing `(CommandId, …)`; keep `key_to_command` for tests)
- Test: `wordcartel/src/registry.rs`

**Interfaces:**
- Consumes: `commands::{Command, run, CommandResult, Dir}`, `editor::Editor`, `jobs::Executor`, `history::Clock`.
- Produces:
  - `pub struct CommandId(pub &'static str);` (derives `Clone, Copy, PartialEq, Eq, Hash, Debug`)
  - `pub struct Ctx<'a> { pub editor: &'a mut Editor, pub clock: &'a dyn Clock, pub executor: &'a dyn Executor }`
  - `pub type Handler = fn(&mut Ctx) -> CommandResult;`
  - `pub struct Registry { map: HashMap<CommandId, Handler> }` with `pub fn builtins() -> Registry`, `pub fn dispatch(&self, id: CommandId, ctx: &mut Ctx) -> CommandResult` (unknown id → surfaces `editor.status = "unknown command: <id>"` and returns `Noop`, never a silent no-op, §12.5).
  - `input::key_to_command_id(key: KeyEvent) -> Option<KeyAction>` where `pub enum KeyAction { Id(CommandId), Insert(char) }` — printable keys map to `Insert(c)` (the literal-insert fallthrough), everything else to a `CommandId`.

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/registry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::jobs::InlineExecutor;
    use wordcartel_core::history::Clock;

    struct Z;
    impl Clock for Z { fn now_ms(&self) -> u64 { 0 } }

    #[test]
    fn dispatch_save_id_runs_save_handler() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex };
        let r = reg.dispatch(CommandId("save"), &mut ctx);
        // No path → save handler reports the no-name status (delegates to run()).
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert!(e.status.contains("No file name"));
    }

    #[test]
    fn unknown_command_surfaces_status_not_silent() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex };
        let r = reg.dispatch(CommandId("nope"), &mut ctx);
        assert_eq!(r, crate::commands::CommandResult::Noop);
        assert!(e.status.contains("unknown command"), "must surface, never silent (§12.5)");
    }

    #[test]
    fn keymap_printable_is_insert_fallthrough_and_arrows_are_ids() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let a = KeyEvent { code: KeyCode::Char('a'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE };
        assert!(matches!(crate::input::key_to_command_id(a), Some(crate::input::KeyAction::Insert('a'))));
        let shift_left = KeyEvent { code: KeyCode::Left, modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press, state: KeyEventState::NONE };
        assert!(matches!(crate::input::key_to_command_id(shift_left),
            Some(crate::input::KeyAction::Id(CommandId("select_left")))));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib registry::tests input::`
Expected: FAIL — `registry` / `key_to_command_id` do not exist.

- [ ] **Step 3: Declare the module** in `wordcartel/src/lib.rs`: add `pub mod registry;` after `pub mod input;`.

- [ ] **Step 4: Write `wordcartel/src/registry.rs`.** Handlers delegate to `commands::run`, which keeps every 4a behavior identical:

```rust
//! Name-keyed command registry (spec §4.4 / §10.4). key → CommandId → Handler.
//! Built-in handlers delegate to the proven `commands::run` implementations so
//! the closed `Command` enum is shared built-in *implementation*, not the
//! dispatch boundary. Plugins (Effort P) register CommandId→Handler here without
//! touching the enum.

use std::collections::HashMap;

use crate::commands::{self, Command, CommandResult, Dir};
use crate::editor::Editor;
use crate::jobs::Executor;
use wordcartel_core::history::Clock;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CommandId(pub &'static str);

/// Everything a handler may touch. The executor is here so job-dispatching
/// handlers (save, swap) have it; today's built-ins ignore it.
pub struct Ctx<'a> {
    pub editor: &'a mut Editor,
    pub clock: &'a dyn Clock,
    pub executor: &'a dyn Executor,
}

pub type Handler = fn(&mut Ctx) -> CommandResult;

pub struct Registry {
    map: HashMap<CommandId, Handler>,
}

impl Registry {
    pub fn builtins() -> Registry {
        let mut map: HashMap<CommandId, Handler> = HashMap::new();
        // Motions (collapse selection).
        map.insert(CommandId("move_left"),  |c| run(c, Command::Move { dir: Dir::Left,  extend: false }));
        map.insert(CommandId("move_right"), |c| run(c, Command::Move { dir: Dir::Right, extend: false }));
        map.insert(CommandId("move_up"),    |c| run(c, Command::Move { dir: Dir::Up,    extend: false }));
        map.insert(CommandId("move_down"),  |c| run(c, Command::Move { dir: Dir::Down,  extend: false }));
        map.insert(CommandId("move_line_start"), |c| run(c, Command::Move { dir: Dir::LineStart, extend: false }));
        map.insert(CommandId("move_line_end"),   |c| run(c, Command::Move { dir: Dir::LineEnd,   extend: false }));
        // Selecting motions (extend).
        map.insert(CommandId("select_left"),  |c| run(c, Command::Move { dir: Dir::Left,  extend: true }));
        map.insert(CommandId("select_right"), |c| run(c, Command::Move { dir: Dir::Right, extend: true }));
        map.insert(CommandId("select_up"),    |c| run(c, Command::Move { dir: Dir::Up,    extend: true }));
        map.insert(CommandId("select_down"),  |c| run(c, Command::Move { dir: Dir::Down,  extend: true }));
        map.insert(CommandId("select_line_start"), |c| run(c, Command::Move { dir: Dir::LineStart, extend: true }));
        map.insert(CommandId("select_line_end"),   |c| run(c, Command::Move { dir: Dir::LineEnd,   extend: true }));
        // Editing.
        map.insert(CommandId("insert_newline"), |c| run(c, Command::InsertNewline));
        map.insert(CommandId("backspace"),      |c| run(c, Command::Backspace));
        map.insert(CommandId("delete_forward"), |c| run(c, Command::DeleteForward));
        // Clipboard / history / view.
        map.insert(CommandId("copy"),  |c| run(c, Command::Copy));
        map.insert(CommandId("cut"),   |c| run(c, Command::Cut));
        map.insert(CommandId("paste"), |c| run(c, Command::Paste));
        map.insert(CommandId("undo"),  |c| run(c, Command::Undo));
        map.insert(CommandId("redo"),  |c| run(c, Command::Redo));
        map.insert(CommandId("cycle_render_mode"), |c| run(c, Command::CycleRenderMode));
        // Save / quit. (Save becomes a job-dispatcher in Task 9; for now it
        // delegates to the synchronous Command::Save arm.)
        map.insert(CommandId("save"), |c| run(c, Command::Save));
        map.insert(CommandId("quit"), |c| run(c, Command::Quit));
        Registry { map }
    }

    /// Dispatch by id. Unknown ids surface a status (never a silent no-op, §12.5).
    pub fn dispatch(&self, id: CommandId, ctx: &mut Ctx) -> CommandResult {
        match self.map.get(&id) {
            Some(handler) => handler(ctx),
            None => {
                ctx.editor.status = format!("unknown command: {}", id.0);
                CommandResult::Noop
            }
        }
    }
}

/// Thin adapter: run a built-in `Command` against the Ctx's editor+clock.
fn run(ctx: &mut Ctx, cmd: Command) -> CommandResult {
    commands::run(cmd, ctx.editor, ctx.clock)
}
```

- [ ] **Step 5: Add `key_to_command_id` to `wordcartel/src/input.rs`.** Keep the existing `key_to_command` (the 84 tests use it). Add the registry-facing mapping:

```rust
use crate::registry::CommandId;

/// What a key resolves to: a named command, or a literal character insert
/// (the §10.4 printable fallthrough — not a registered command).
pub enum KeyAction {
    Id(CommandId),
    Insert(char),
}

/// Registry-facing keymap: key → CommandId (or literal insert).
pub fn key_to_command_id(key: KeyEvent) -> Option<KeyAction> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let id = |s| Some(KeyAction::Id(CommandId(s)));
    match key.code {
        KeyCode::Char('z') if ctrl && !shift => id("undo"),
        KeyCode::Char('y') if ctrl           => id("redo"),
        KeyCode::Char('Z') if ctrl && shift  => id("redo"),
        KeyCode::Char('c') if ctrl           => id("copy"),
        KeyCode::Char('x') if ctrl           => id("cut"),
        KeyCode::Char('v') if ctrl           => id("paste"),
        KeyCode::Char('s') if ctrl           => id("save"),
        KeyCode::Char('q') if ctrl           => id("quit"),
        KeyCode::Char('\\') if ctrl          => id("cycle_render_mode"),

        KeyCode::Left  => id(if shift { "select_left" } else { "move_left" }),
        KeyCode::Right => id(if shift { "select_right" } else { "move_right" }),
        KeyCode::Up    => id(if shift { "select_up" } else { "move_up" }),
        KeyCode::Down  => id(if shift { "select_down" } else { "move_down" }),
        KeyCode::Home  => id(if shift { "select_line_start" } else { "move_line_start" }),
        KeyCode::End   => id(if shift { "select_line_end" } else { "move_line_end" }),

        KeyCode::Enter     => id("insert_newline"),
        KeyCode::Backspace => id("backspace"),
        KeyCode::Delete    => id("delete_forward"),
        KeyCode::F(1)      => id("cycle_render_mode"),

        KeyCode::Char(c) if !ctrl && !alt => Some(KeyAction::Insert(c)),
        _ => None,
    }
}
```

- [ ] **Step 6: Run tests to verify pass**

Run: `cargo test -p wordcartel --lib registry::tests input::`
Expected: PASS — the 3 new tests plus all existing `input::`/`app::` keymap tests (untouched `key_to_command`).

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/registry.rs wordcartel/src/input.rs wordcartel/src/lib.rs
git commit -m "feat(registry): key→CommandId→Handler dispatch; literal-insert fallthrough"
```

---

## Task 8: unified-channel main loop + input thread (spec §4.2)

**Rewrite `app::run`** around a single `mpsc::Receiver<Msg>` fed by (a) a detached input thread looping on `crossterm::event::read()`, (b) the worker's forwarded `JobResult`s, and (c) a timer `Tick`. A finished job now wakes the loop instantly instead of waiting for the next keypress. The pure, testable part is a new `reduce(Msg)` reducer; `run` is the thin IO wrapper. Save is still synchronous here (Task 9 converts it) — at this point no job kinds are dispatched in production, but the `JobDone` plumbing and reducer are exercised by a unit test using `InlineExecutor`.

**Files:**
- Modify: `wordcartel/src/app.rs` (replace `run`; add `Msg`, `reduce`, `apply_result`, `dispatch_key`)
- Test: `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `registry::{Registry, Ctx, CommandId}`, `input::{key_to_command_id, KeyAction}`, `jobs::{Executor, JobResult, is_stale}`, `commands` (for InsertChar), `derive`, `render`, `term`.
- Produces:
  - `pub enum Msg { Input(crossterm::event::Event), JobDone(JobResult), Tick }`
  - `pub fn reduce(msg: Msg, editor: &mut Editor, reg: &Registry, ex: &dyn Executor, clock: &dyn Clock) -> bool` — process one message; returns `true` while the app should keep running. Handles `Input(Key)` via the registry (or literal insert), `JobDone(r)` via `apply_result`, `Tick` (no-op in 4b-1; swap cadence is 4b-2), then drains and merges any further ready results.
  - `pub fn apply_result(r: JobResult, editor: &mut Editor)` — if `!is_stale(r.kind, r.version, editor.document.version)`, run `(r.merge)(editor)`; else drop it.

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/app.rs` tests:

```rust
#[test]
fn reduce_handles_typing_via_registry() {
    use crate::editor::Editor;
    use crate::jobs::InlineExecutor;
    use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("\n", None, (80, 24));
    let reg = Registry::builtins();
    let ex = InlineExecutor::default();
    let clk = TestClock(0);
    for c in "hi".chars() {
        let ev = Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        assert!(crate::app::reduce(crate::app::Msg::Input(ev), &mut e, &reg, &ex, &clk));
    }
    assert_eq!(e.document.buffer.to_string(), "hi\n");
}

#[test]
fn apply_result_merges_fresh_and_drops_stale() {
    use crate::editor::Editor;
    use crate::jobs::{JobResult, JobKind};
    let mut e = Editor::new_from_text("\n", None, (80, 24));
    e.document.version = 5;
    // Fresh one-shot (Save is never stale): merges.
    crate::app::apply_result(JobResult { version: 3, kind: JobKind::Save,
        merge: Box::new(|ed: &mut Editor| ed.status = "saved".into()) }, &mut e);
    assert_eq!(e.status, "saved");
    // Stale coalescible: dropped.
    crate::app::apply_result(JobResult { version: 3, kind: JobKind::CoalesceProbe,
        merge: Box::new(|ed: &mut Editor| ed.status = "STALE".into()) }, &mut e);
    assert_eq!(e.status, "saved", "stale coalescible result must be dropped");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib app::tests::reduce_handles_typing_via_registry app::tests::apply_result_merges_fresh_and_drops_stale`
Expected: FAIL — `reduce`/`apply_result`/`Msg` do not exist.

- [ ] **Step 3: Add the reducer, `apply_result`, and `Msg`** to `wordcartel/src/app.rs` (keep the existing `step` for the legacy keymap tests):

```rust
use crate::jobs::{is_stale, Executor, JobResult};
use crate::registry::{Ctx, Registry};
use crate::input::KeyAction;

pub enum Msg {
    Input(Event),
    JobDone(JobResult),
    Tick,
}

/// Merge a finished job's effect on the foreground, honoring staleness (§10.3).
pub fn apply_result(r: JobResult, editor: &mut Editor) {
    if is_stale(r.kind, r.version, editor.document.version) {
        return; // version moved on: discard, don't rebase
    }
    (r.merge)(editor);
}

/// Process one message. Returns true while the app should keep running.
pub fn reduce(
    msg: Msg,
    editor: &mut Editor,
    reg: &Registry,
    ex: &dyn Executor,
    clock: &dyn Clock,
) -> bool {
    match msg {
        Msg::Input(Event::Key(key)) => {
            match crate::input::key_to_command_id(key) {
                Some(KeyAction::Id(id)) => {
                    let mut ctx = Ctx { editor, clock, executor: ex };
                    reg.dispatch(id, &mut ctx);
                }
                Some(KeyAction::Insert(c)) => {
                    commands::run(commands::Command::InsertChar(c), editor, clock);
                }
                None => {}
            }
        }
        Msg::Input(Event::Resize(w, h)) => {
            editor.view.area = (w, h);
            derive::rebuild(editor);
        }
        Msg::Input(_) => {}
        Msg::JobDone(r) => apply_result(r, editor),
        Msg::Tick => { /* swap cadence wired in Effort 4b-2 */ }
    }
    // Fold any other results that became ready while handling this message.
    for r in ex.drain() {
        apply_result(r, editor);
    }
    !editor.quit
}
```

- [ ] **Step 4: Rewrite `run`** in `wordcartel/src/app.rs` to use the unified channel + input thread. Replace the `loop { … crossterm::event::read() … }` body (lines ~100-126) with:

```rust
    let reg = Registry::builtins();
    let (executor, forward) = crate::jobs::ThreadExecutor::new();

    // Unified message channel. The input thread, the worker forwarder, and the
    // timer all feed `msg_tx`; the loop blocks on `msg_rx` (zero idle CPU).
    let (msg_tx, msg_rx) = std::sync::mpsc::channel::<Msg>();

    // Worker → unified channel: a small forwarder thread turns each JobResult
    // into Msg::JobDone. (The worker pushes onto `forward`; we relay it here so
    // the loop only ever reads one channel.)
    {
        let msg_tx = msg_tx.clone();
        let (_relay_tx, relay_rx) = (forward, ()); // `forward` is the JobResult Sender clone
        // NOTE: see Task 8 step 5 — the relay reads from a JobResult Receiver.
        let _ = (msg_tx, relay_rx);
    }

    // Input thread: blocks on read(); forwards every event. Detached — it dies
    // with the process on quit (read() can't be interrupted portably).
    {
        let msg_tx = msg_tx.clone();
        std::thread::Builder::new()
            .name("wcartel-input".into())
            .spawn(move || {
                while let Ok(ev) = crossterm::event::read() {
                    if msg_tx.send(Msg::Input(ev)).is_err() {
                        break; // receiver dropped → app exiting
                    }
                }
            })
            .expect("spawn input thread");
    }

    let clock = SystemClock;
    // Initial draw.
    guard.terminal().draw(|f| render::render(f, &editor))?;
    for msg in msg_rx {
        let keep = reduce(msg, &mut editor, &reg, &executor, &clock);
        guard.terminal().draw(|f| render::render(f, &editor))?;
        if !keep {
            break;
        }
    }
    Ok(())
```

- [ ] **Step 5: Resolve the worker→loop forwarder cleanly.** The placeholder relay above is wrong; replace `ThreadExecutor::new()`'s second return with a proper relay. Change the forwarder block to spawn a relay that moves `JobResult`s from the executor's result stream into `Msg::JobDone`. Simplest correct shape: have `ThreadExecutor::new()` return `(ThreadExecutor, Receiver<JobResult>)` is *not* possible (the executor owns the receiver for `drain`). Instead, the loop relies on **`reduce`'s `ex.drain()`** to fold results — so the worker does not need a push relay at all for correctness, **but** a result completing while the loop is blocked on `msg_rx` would not wake it. To guarantee instant wake, give the worker a clone of `msg_tx`:

Modify `ThreadExecutor::new` to accept the unified sender: change its signature to `pub fn new(on_done: Sender<Msg>) -> ThreadExecutor` — **but `Msg` lives in `app`, which would couple `jobs` to `app`.** Avoid the cycle: have the worker send `JobResult` on its own channel AND a unit "wake" on a `Sender<()>` the loop selects on. Concretely, keep `ThreadExecutor` as in Task 6 (it owns `result_rx` for `drain`) and add a **wake sender**:

In `wordcartel/src/jobs.rs`, change `ThreadExecutor::new` to:
```rust
    pub fn new(wake: Sender<()>) -> ThreadExecutor {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<JobResult>();
        let worker = std::thread::Builder::new()
            .name("wcartel-jobs".into())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    let result = (job.run)();
                    if result_tx.send(result).is_err() { break; }
                    let _ = wake.send(()); // nudge the loop to drain
                }
            })
            .expect("spawn jobs worker");
        ThreadExecutor { job_tx: Some(job_tx), result_rx, worker: Some(worker) }
    }
```
Update Task 6's test to pass a throwaway `wake` sender: `let (w, _) = mpsc::channel(); let ex = ThreadExecutor::new(w);` and drop the `_forward` binding.

Then in `app::run`, model the wake as a `Msg`: create `let (wake_tx, wake_rx) = mpsc::channel::<()>();`, spawn a relay thread that converts each wake `()` into `msg_tx.send(Msg::Tick)` **only as a drain nudge** — but a cleaner unification is to send `Msg::JobDone` is impossible without the result. Resolution adopted: the worker sends results on `result_rx` (drained by `reduce`), and a **relay thread** owns nothing but forwards wakes:
```rust
    let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
    let executor = crate::jobs::ThreadExecutor::new(wake_tx);
    {
        let msg_tx = msg_tx.clone();
        std::thread::spawn(move || {
            while wake_rx.recv().is_ok() {
                // Nudge the loop; reduce()'s ex.drain() collects the actual results.
                if msg_tx.send(Msg::Tick).is_err() { break; }
            }
        });
    }
```
`Msg::Tick` already triggers `reduce`'s trailing `ex.drain()`, so the merge happens on wake. (4b-2 repurposes `Tick` for swap cadence; the drain-on-tick behavior is shared and harmless.) Remove the broken placeholder relay block from Step 4.

- [ ] **Step 6: Run the loop tests + full suite**

Run: `cargo test`
Expected: PASS — `reduce_*`/`apply_result_*` green; the updated Task 6 thread test green; all prior tests green. `app::run` is not unit-tested directly (it owns the terminal), but `reduce` covers its logic.

- [ ] **Step 7: Manual smoke (optional but recommended).** Build and run the binary on a scratch file; confirm typing, arrows, Ctrl+S (sync save still), Ctrl+\\ mode toggle, and Ctrl+Q twice all work:

Run: `cargo run -p wordcartel -- /tmp/wcartel-smoke.md`
Expected: editor opens, edits render live, quit restores the terminal.

- [ ] **Step 8: Commit**

```bash
git add wordcartel/src/app.rs wordcartel/src/jobs.rs
git commit -m "feat(app): unified-channel loop + input thread; reduce() reducer wakes on jobs"
```

---

## Task 9: background save (spec §4.3)

**Convert save into a `JobKind::Save` job.** The foreground writes `"Saving…"` and captures an O(1) rope snapshot + version + path, then dispatches; the worker materializes the snapshot to a `String` and calls `file::save_atomic(&path, &content)`; the returned `merge` updates status/`saved_version` **version-awarely** (only marks the buffer "Saved" if the document is still at the saved version) and refreshes the stored fingerprint. The save handler in the registry stops delegating to the synchronous `Command::Save` and calls `save::dispatch_save(ctx)`.

**Files:**
- Create: `wordcartel/src/save.rs`
- Modify: `wordcartel/src/lib.rs` (add `pub mod save;`)
- Modify: `wordcartel/src/editor.rs` (add `stored_fp: Option<FileFingerprint>` to `Document`; capture at load)
- Modify: `wordcartel/src/registry.rs` (the `"save"` handler calls `save::dispatch_save`)
- Modify: `wordcartel/src/file.rs` (the `save_command_clears_dirty` test → background-save shape)
- Test: `wordcartel/src/save.rs`

**Interfaces:**
- Consumes: `jobs::{Job, JobResult, JobKind, Executor}`, `file::{save_atomic, SaveOutcome, SaveError}`, `editor::Editor`, `registry::Ctx`.
- Produces:
  - `pub struct FileFingerprint { pub mtime: Option<std::time::SystemTime>, pub size: u64 }` (derives `Clone, Copy, PartialEq, Eq, Debug`).
  - `pub fn fingerprint(path: &std::path::Path) -> Option<FileFingerprint>` — `None` if the path does not exist.
  - `pub fn dispatch_save(ctx: &mut Ctx) -> CommandResult` — the registry `"save"` handler.
  - `Document.stored_fp: Option<FileFingerprint>` — last-known on-disk fingerprint (captured at load, refreshed by the save merge). (External-mod *prompting* on a pre-save stat mismatch is added in Effort 4b-2; 4b-1 captures/refreshes the fingerprint and, on a detected mismatch, refuses with a status message.)

- [ ] **Step 1: Write the failing tests** in `wordcartel/src/save.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::jobs::{Executor, InlineExecutor};
    use crate::registry::Ctx;
    use wordcartel_core::history::Clock;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Z; impl Clock for Z { fn now_ms(&self) -> u64 { 0 } }
    static SEQ: AtomicU32 = AtomicU32::new(0);
    fn scratch() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("wcartel-bgsave-{}-{}.md",
            std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)))
    }

    #[test]
    fn background_save_clears_dirty_at_saved_version() {
        let p = scratch();
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.document.saved_version = None; // simulate an unsaved edit
        e.document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        {
            let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex };
            dispatch_save(&mut ctx);
        }
        assert_eq!(e.status, "Saving\u{2026}", "status set before dispatch (§3.9)");
        // InlineExecutor already ran the job; apply the buffered merge.
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(!e.document.dirty(), "version==saved_version after save → clean");
        assert_eq!(e.status, "Saved");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new\n");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn background_save_result_for_old_version_does_not_mark_clean() {
        let p = scratch();
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("v1\n", Some(p.clone()), (80, 24));
        e.document.saved_version = None;
        e.document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; dispatch_save(&mut ctx); }
        // User edits on to version 2 BEFORE the merge applies.
        e.document.version = 2;
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        // saved_version recorded v1, but the buffer is at v2 → still dirty.
        assert_eq!(e.document.saved_version, Some(1));
        assert!(e.document.dirty(), "edited-on buffer stays dirty after a stale-version save");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn background_save_failure_keeps_dirty_and_status() {
        // Save through a symlink is refused by save_atomic → merge must keep dirty.
        let real = scratch();
        let link = scratch();
        std::fs::write(&real, "real\n").unwrap();
        #[cfg(unix)] std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))] { let _ = &link; return; }
        let mut e = Editor::new_from_text("x\n", Some(link.clone()), (80, 24));
        e.document.saved_version = None;
        e.document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex }; dispatch_save(&mut ctx); }
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(e.document.dirty(), "failed save must leave the buffer dirty");
        assert!(e.document.saved_version.is_none());
        assert!(e.status.to_lowercase().contains("symlink"));
        let _ = std::fs::remove_file(&link); let _ = std::fs::remove_file(&real);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib save::tests`
Expected: FAIL — module `save` does not exist.

- [ ] **Step 3: Add `stored_fp` to `Document`** in `wordcartel/src/editor.rs`: add field `pub stored_fp: Option<crate::save::FileFingerprint>,` and initialize it in `new_from_text` to `crate::save::fingerprint_of_path(path.as_deref())` (a helper that returns `None` for `None`/missing). To avoid a construction-time path stat in unit tests that pass real temp paths, define the helper to be cheap and tolerant:

In `editor.rs` `new_from_text`, after computing `path`, set:
```rust
            stored_fp: path.as_deref().and_then(crate::save::fingerprint),
```
and add `stored_fp` to the `Document { … }` literal.

- [ ] **Step 4: Declare the module** in `wordcartel/src/lib.rs`: add `pub mod save;` after `pub mod jobs;`.

- [ ] **Step 5: Write `wordcartel/src/save.rs`:**

```rust
//! Background save (spec §4.3). The foreground captures an O(1) rope snapshot +
//! version + path and dispatches a JobKind::Save job; the worker materializes
//! the snapshot off the keystroke path and atomically writes it; the merge
//! updates status/saved_version version-awarely.

use std::path::Path;
use std::time::SystemTime;

use crate::commands::CommandResult;
use crate::file::{self, SaveOutcome, SaveError};
use crate::jobs::{Job, JobKind, JobResult};
use crate::registry::Ctx;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileFingerprint {
    pub mtime: Option<SystemTime>,
    pub size: u64,
}

/// Fingerprint a path, or `None` if it does not exist / cannot be stat'd.
pub fn fingerprint(path: &Path) -> Option<FileFingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    Some(FileFingerprint { mtime: meta.modified().ok(), size: meta.len() })
}

/// Registry `"save"` handler: dispatch a background save.
pub fn dispatch_save(ctx: &mut Ctx) -> CommandResult {
    let path = match &ctx.editor.document.path {
        None => {
            ctx.editor.status = "No file name (save-as is Effort 5)".to_string();
            return CommandResult::Handled;
        }
        Some(p) => p.clone(),
    };

    // External-mod check (§4.3 step 2): cheap stat; if the on-disk fingerprint
    // diverged from what we last wrote, refuse and surface a status. (4b-2 turns
    // this into a modal R/O/S prompt.)
    let current_fp = fingerprint(&path);
    if current_fp != ctx.editor.document.stored_fp {
        ctx.editor.status =
            "File changed on disk \u{2014} not saved (reload or overwrite — Effort 4b-2)".to_string();
        return CommandResult::Handled;
    }

    // §3.9: status BEFORE dispatch. O(1) snapshot; version captured now.
    ctx.editor.status = "Saving\u{2026}".to_string();
    let snap = ctx.editor.document.buffer.snapshot(); // O(1) ropey clone
    let v = ctx.editor.document.version;

    ctx.executor.dispatch(Job {
        version: v,
        kind: JobKind::Save,
        run: Box::new(move || {
            // Worker: materialize the snapshot off the keystroke path, then write.
            let content = snap.to_string();
            let outcome = file::save_atomic(&path, &content);
            let new_fp = fingerprint(&path);
            JobResult {
                version: v,
                kind: JobKind::Save,
                merge: Box::new(move |editor| {
                    match outcome {
                        Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged) => {
                            // Record what is now on disk at version v (always).
                            editor.document.saved_version = Some(v);
                            editor.document.stored_fp = new_fp;
                            // Only "Saved" if the buffer is now clean; otherwise the
                            // user edited on and the buffer is still dirty (§4.3).
                            if editor.document.version == v {
                                editor.status = "Saved".to_string();
                            } else {
                                editor.status = format!("Saved v{v} (still editing)");
                            }
                        }
                        Err(e) => {
                            // Failure: leave saved_version/stored_fp untouched
                            // (buffer stays dirty); surface the error.
                            editor.status = match e {
                                SaveError::Symlink => "refusing to write through symlink".into(),
                                other => other.to_string(),
                            };
                        }
                    }
                }),
            }
        }),
    });
    CommandResult::Handled
}
```

- [ ] **Step 6: Point the registry `"save"` handler at the background save.** In `wordcartel/src/registry.rs` `builtins()`, replace the `"save"` insert:

```rust
        map.insert(CommandId("save"), |c| crate::save::dispatch_save(c));
```

- [ ] **Step 7: Update the `file.rs` save test to the background shape.** In `wordcartel/src/file.rs`, replace `save_command_clears_dirty` with a background-save version that dispatches via `InlineExecutor` and applies the merge (same assertion: a save clears dirty):

```rust
    #[test]
    fn background_save_command_clears_dirty() {
        use crate::editor::Editor;
        use crate::jobs::{Executor, InlineExecutor};
        use crate::registry::Ctx;
        use wordcartel_core::history::Clock;
        struct Z; impl Clock for Z { fn now_ms(&self) -> u64 { 0 } }

        let p = scratch_path("cmd-save");
        fs::write(&p, "initial\n").expect("pre-write");
        let mut e = Editor::new_from_text("hello\n", Some(p.clone()), (80, 24));
        e.document.saved_version = None; // unsaved edit
        e.document.version = 1;
        let ex = InlineExecutor::default();
        let clk = Z;
        { let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex };
          crate::save::dispatch_save(&mut ctx); }
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(!e.document.dirty(), "dirty must be cleared after a successful background save");
        let _ = fs::remove_file(&p);
    }
```

- [ ] **Step 8: Run the full suite**

Run: `cargo test`
Expected: PASS — `save::tests` (3) green; `background_save_command_clears_dirty` green; all prior tests green.

- [ ] **Step 9: Manual smoke.** Edit a file, Ctrl+S; status flashes "Saving…" then "Saved"; the file on disk updates; editing again and saving still works:

Run: `cargo run -p wordcartel -- /tmp/wcartel-smoke.md`

- [ ] **Step 10: Commit**

```bash
git add wordcartel/src/save.rs wordcartel/src/editor.rs wordcartel/src/registry.rs wordcartel/src/file.rs wordcartel/src/lib.rs
git commit -m "feat(save): background save job with version-aware merge + fingerprint"
```

---

## Self-Review (4b-1)

**Spec coverage (§4):**
- §4.1 job substrate (Job/JobResult/JobKind, Executor, InlineExecutor, ThreadExecutor, staleness, CoalesceProbe) → Tasks 5, 6. ✅
- §4.2 unified-channel loop + input thread + instant wake → Task 8. ✅
- §4.3 saved_version/dirty model + background save + version-aware merge + fingerprint capture → Tasks 4, 9. ✅
- §4.4 name-keyed registry (key→ID→handler), literal-insert fallthrough, unknown-id surfaced, 84 tests preserved → Task 7. ✅
- §4.5 four polish items: undo/redo no-op (Task 2), CycleRenderMode ensure_visible + Copy-on-empty (Task 3), LF-only lines (Task 1). ✅

**Deferred to 4b-2 (out of this plan's scope, by design):** swap/recovery file, panic dump, modal-prompt infra, the external-mod *prompt* (4b-1 detects+refuses-with-status), save&quit bounded wait, swap cadence on `Tick`. Tracked in `2026-06-23-wordcartel-04b2-crash-safety.md`.

**Type consistency:** `Executor::{dispatch, drain}`, `JobKind::{Save, SwapWrite, CoalesceProbe}`, `is_stale(kind, result_version, current_version)`, `Ctx { editor, clock, executor }`, `CommandId(&'static str)`, `KeyAction::{Id, Insert}`, `Document::{dirty(), mark_saved(), saved_version, stored_fp}`, `FileFingerprint { mtime, size }`, `save::{fingerprint, dispatch_save}`, `app::{Msg, reduce, apply_result}` — names are used identically across tasks. `ThreadExecutor::new(wake: Sender<()>)` finalized in Task 8 Step 5 (Task 6's test updated to match).

**Known seams the executor must hold:** Task 8's worker→loop wake uses a `Sender<()>` nudge + `reduce`'s `ex.drain()`; this avoids a `jobs`→`app` type cycle. If, during execution, the relay proves racy under load, the fallback is to have the loop also `drain()` after every `Input` (already the case via `reduce`'s trailing drain), so a missed wake only delays a merge to the next event, never drops it.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-23-wordcartel-04b1-async-substrate.md`. (Effort 4b-2 — crash safety — is a separate plan that builds on this one.)
