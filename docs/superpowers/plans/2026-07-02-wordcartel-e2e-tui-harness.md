# e2e / TUI Test Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An in-process `Harness` that drives the REAL `reduce → advance → render` loop body over a `TestBackend` with a virtual clock + `InlineExecutor`, plus 7 non-vacuous seed journeys pinning the project's invariants — the shell's first end-to-end coverage.

**Architecture:** One small behavior-preserving extraction (`advance` out of `run()`'s loop body) so the harness and `run()` share the state-affecting per-iteration steps; a `#[cfg(test)] pub(crate) mod test_support` holding the lifted `TestClock` + key builders; a `#[cfg(test)] mod e2e` holding the `Harness` + journeys.

**Tech Stack:** Rust, `wordcartel` shell, ratatui 0.30 `TestBackend`, crossterm 0.28, `tempfile`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-02-wordcartel-e2e-tui-harness-design.md` (Codex ×3 + Fable5 folded).
- `cargo test -p wordcartel` green; `cargo build`/`test --no-run` warning-free; **`cargo clippy --workspace --all-targets` clean (deny gate LIVE)**; NO `cargo fmt`; house style (em-dash `—`).
- `#![forbid(unsafe_code)]` unaffected (core untouched).
- New scaffolding is `#[cfg(test)]`-gated; `advance` (`pub(crate)`, app.rs) is the ONLY non-test change.
- **Never weaken an assertion to make a journey pass** — the `reason`/state assertions are the contract; if a construction doesn't hit the target path, fix the construction (or, for a genuinely unreachable path, delete + ledger-record).
- Trailers on every commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: Extract `advance` from `run()`'s loop body

**Files:** Modify `wordcartel/src/app.rs` (add `advance`; replace three loop-body lines with a call).

**Interfaces produced:** `pub(crate) fn advance(editor: &mut Editor, clock: &dyn Clock)`.

- [ ] **Step 1: Add `advance`** near `reduce` in `app.rs` (it calls `recompute_scrollbar_visible` + `derive::rebuild` + the reconcile-arming block — a verbatim move of app.rs:2157-2174, comments included):

```rust
/// The state-affecting per-iteration steps shared by `run()`'s loop and the e2e harness
/// (everything between the clipboard/mouse terminal steps and the draw). Extracted so the
/// harness exercises the REAL loop body, not a re-implementation.
pub(crate) fn advance(editor: &mut Editor, clock: &dyn Clock) {
    recompute_scrollbar_visible(editor, clock.now_ms());
    // Pre-draw rebuild: ensure the layout cache matches the final (scroll,
    // text_width) before render consumes it.  render has no on-demand fallback
    // (render.rs:132-140), so a stale cache blanks the editing rows.
    derive::rebuild(editor);
    // Arm the reconcile debounce when the tree is (possibly) stale. Re-arm only
    // when the version advanced since the last arm (so idle Ticks don't push the
    // deadline forever); arm-from-None also covers a switch to a stale buffer.
    {
        let now = clock.now_ms();
        let b = editor.active_mut();
        if b.reconcile.maybe_stale && b.reconcile.in_flight_version.is_none()
            && (b.reconcile.due_at.is_none() || b.reconcile.armed_for_version != b.document.version)
        {
            b.reconcile.due_at = Some(now.saturating_add(crate::reconcile::RECONCILE_DEBOUNCE_MS));
            b.reconcile.armed_for_version = b.document.version;
        }
    }
}
```

- [ ] **Step 2: Replace the extracted lines in `run()`** (app.rs:2157-2174) with a single call, leaving everything above (`note_undo_eviction`, `drain_clipboard_intents`, `reconcile_mouse_capture`) and below (`draw`, session persist) EXACTLY as-is:
```rust
        reconcile_mouse_capture(&mut editor, guard.terminal().backend_mut(), &mut applied_mouse);
        advance(&mut editor, &clock);
        guard.terminal().draw(|f| render::render(f, &mut editor))?;
```

**CAUTION (Codex):** replace ONLY the loop-body lines app.rs:2157-2174. Do NOT touch the
pre-first-draw block near app.rs:2059 — it additionally snaps folded cursor state + calls
`ensure_visible` and is NOT a duplicate of `advance`; leave it as-is.

- [ ] **Step 3: Run the suite — confirm the extraction is behavior-preserving.**
Run: `cargo test -p wordcartel` and `cargo clippy --workspace --all-targets`.
Expected: PASS/clean — a pure move of three consecutive lines; the 127 `app.rs` tests + full suite stay green.

- [ ] **Step 4: Commit.**
```bash
git add wordcartel/src/app.rs
git commit -m "refactor(app): extract advance() from run()'s loop body (shared with e2e harness)"   # + trailers
```

---

### Task 2: `test_support` module + the `Harness` + 2 smoke journeys

**Files:**
- Create `wordcartel/src/test_support.rs`; declare `#[cfg(test)] pub(crate) mod test_support;` in `wordcartel/src/lib.rs`.
- Modify `wordcartel/src/app.rs` (`mod tests`) — remove the moved helpers, add a `use`.
- Create `wordcartel/src/e2e.rs`; declare `#[cfg(test)] mod e2e;` in `wordcartel/src/lib.rs`.

**Interfaces produced:** `test_support::{TestClock, key_char, press}`; `e2e::Harness`.

- [ ] **Step 1: Create `test_support.rs`** — lift `TestClock` + `key_char` + `press` from `app::tests` verbatim, made `pub(crate)` (the tuple field `pub(crate)` so `TestClock(now)` works from `e2e`):

```rust
//! Shared #[cfg(test)] helpers for the shell's test modules (app::tests, e2e).
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use wordcartel_core::history::Clock;
use crate::app::Msg;

/// Deterministic virtual clock: `now_ms()` returns a fixed value.
pub(crate) struct TestClock(pub(crate) u64);
impl TestClock {
    pub(crate) fn new(ms: u64) -> Self { TestClock(ms) }
}
impl Clock for TestClock {
    fn now_ms(&self) -> u64 { self.0 }
}

/// A KeyEvent for a printable character (no modifiers, Press).
pub(crate) fn key_char(c: char) -> KeyEvent {
    KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }
}

/// A `Msg::Input` key press with explicit code + modifiers. NOTE (Codex): `press` already
/// returns `Msg` — the harness sugar passes it straight to `step`; never wrap it as
/// `Msg::Input(press(...))`.
pub(crate) fn press(code: KeyCode, mods: KeyModifiers) -> Msg {
    Msg::Input(Event::Key(KeyEvent { code, modifiers: mods, kind: KeyEventKind::Press, state: KeyEventState::NONE }))
}
```

- [ ] **Step 2: Update `app::tests`** — delete the now-moved `TestClock`, `key_char`, `press` definitions (app.rs:2306-2312, 2314-2322, 2357-2360) and add, at the top of `mod tests`:
```rust
    use crate::test_support::{TestClock, key_char, press};
```
Run `cargo test -p wordcartel` → the 127 app tests still pass (they now use the lifted helpers). If any app test used `TestClock::new` vs `TestClock(..)`, both still work.

- [ ] **Step 3: Create `e2e.rs` with the `Harness`** (`#[cfg(test)] mod e2e`). Imports + struct + constructor + the `step` core:

```rust
#![cfg(test)]
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use wordcartel_core::block_tree::{BlockTree, full_parse_rope};

use crate::app::{self, Msg, reduce};
use crate::editor::Editor;
use crate::jobs::InlineExecutor;
use crate::keymap::{self, KeyTrie};
use crate::registry::Registry;
use crate::render;
use crate::test_support::{TestClock, key_char, press};

struct Harness {
    editor: Editor,
    reg: Registry,
    keymap: KeyTrie,
    ex: InlineExecutor,
    term: Terminal<TestBackend>,
    tx: Sender<Msg>,
    _rx: Receiver<Msg>,
    now: u64,
}

impl Harness {
    fn new(text: &str, path: Option<PathBuf>, size: (u16, u16)) -> Self {
        let mut editor = Editor::new_from_text(text, path, size);
        editor.diag_cfg.enabled = false; // hermeticity: no real diagnostics thread (spec I3)
        let reg = Registry::builtins();
        let (keymap, _warn) = keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = InlineExecutor::default();
        let term = Terminal::new(TestBackend::new(size.0, size.1)).expect("test terminal");
        let (tx, _rx) = mpsc::channel();
        let mut h = Harness { editor, reg, keymap, ex, term, tx, _rx, now: 0 };
        crate::derive::rebuild(&mut h.editor);
        h.render();
        h
    }

    /// The shared production sequence: snapshot → reduce → note_undo_eviction → advance → render.
    fn step(&mut self, msg: Msg) -> bool {
        let (pre_id, pre_version) = { let b = self.editor.active(); (b.id, b.document.version) };
        let clock = TestClock(self.now);
        let keep = reduce(msg, &mut self.editor, &self.reg, &self.keymap, &self.ex, &clock, &self.tx);
        self.editor.note_undo_eviction(pre_id, pre_version);
        app::advance(&mut self.editor, &clock);
        self.render();
        keep
    }

    fn render(&mut self) {
        let editor = &mut self.editor;
        self.term.draw(|f| render::render(f, editor)).expect("draw");
    }

    // — input sugar —
    fn type_str(&mut self, s: &str) { for c in s.chars() { self.step(Msg::Input(Event::Key(key_char(c)))); } }
    fn ctrl(&mut self, c: char) -> bool { self.step(press(KeyCode::Char(c), KeyModifiers::CONTROL)) }
    fn alt(&mut self, c: char) -> bool { self.step(press(KeyCode::Char(c), KeyModifiers::ALT)) }
    fn key(&mut self, code: KeyCode) -> bool { self.step(press(code, KeyModifiers::NONE)) }
    fn resize(&mut self, w: u16, h: u16) {
        self.term.backend_mut().resize(w, h);              // sync the TestBackend cell grid
        self.step(Msg::Input(Event::Resize(w, h)));        // update the editor's buffer areas
    }
    fn advance_ms(&mut self, ms: u64) { self.now = self.now.saturating_add(ms); }
    fn tick(&mut self) -> bool { self.step(Msg::Tick) }

    // — state assertions —
    fn doc_text(&self) -> String { self.editor.active().document.buffer.to_string() }
    fn dirty(&self) -> bool { self.editor.active().document.dirty() }
    fn saved_version(&self) -> Option<u64> { self.editor.active().document.saved_version } // Option, not u64 (editor.rs:64)
    fn status(&self) -> &str { &self.editor.status }
    fn blocks(&self) -> &BlockTree { self.editor.active().document.blocks() }

    // — screen assertions —
    fn row(&self, y: u16) -> String {
        let buf = self.term.backend().buffer();
        let w = buf.area().width;
        (0..w).map(|x| buf[(x, y)].symbol()).collect()
    }
    fn screen(&self) -> Vec<String> {
        let h = self.term.backend().buffer().area().height;
        (0..h).map(|y| self.row(y)).collect()
    }
    fn screen_contains(&self, needle: &str) -> bool { self.screen().iter().any(|r| r.contains(needle)) }
}
```
(Plan-confirm 3: verify the exact accessor spellings against source — `editor.status` field, `document.buffer.to_string()`, `document.blocks()`, `document.saved_version`, `active().id`, `note_undo_eviction`, `TestBackend::resize`/`backend_mut`/`buffer()[(x,y)].symbol()`, `Editor::new_from_text`/`diag_cfg`. Adjust spellings to match; the SHAPE is fixed.)

- [ ] **Step 4: Journey — type → render.**
```rust
#[test]
fn e2e_type_shows_in_doc_and_render() {
    let mut h = Harness::new("", None, (80, 24));
    h.type_str("hello");
    assert_eq!(h.doc_text(), "hello");
    assert!(h.screen_contains("hello"), "typed text must appear on screen:\n{:#?}", h.screen());
}
```

- [ ] **Step 5: Journey — save → reload (real fs, fingerprint lifecycle).**
```rust
#[test]
fn e2e_save_writes_file_and_reloads() {
    // Create the empty tempfile BEFORE Harness::new so stored_fp == fingerprint(path)
    // (else dispatch_save raises the external-change modal instead of saving — spec I4).
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let mut h = Harness::new("", Some(path.clone()), (80, 24));
    h.type_str("hello\n");
    h.ctrl('s'); // save runs inline under InlineExecutor; reduce drains before returning
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
    assert_eq!(h.status(), "Saved");
    assert!(!h.dirty());
    // Reload: a fresh harness opening the same file round-trips.
    let h2 = Harness::new(&std::fs::read_to_string(&path).unwrap(), Some(path.clone()), (80, 24));
    assert_eq!(h2.doc_text(), "hello\n");
}
```
(Plan-confirm: the exact save trigger — `ctrl('s')` maps to the save command in the CUA keymap — and the exact `"Saved"` string, save.rs:98. If `Harness::new` from a file needs the text read separately, adjust the reload construction; the round-trip assertion is the contract.)

- [ ] **Step 6: Run + gates + commit.**
Run: `cargo test -p wordcartel` (green, incl. the 2 new journeys) + `cargo clippy --workspace --all-targets` (clean).
```bash
git add -A
git commit -m "test(e2e): test_support lift + Harness + type/save smoke journeys"   # + trailers
```

---

### Task 3: The remaining 5 seed journeys

**Files:** Modify `wordcartel/src/e2e.rs` (add 5 `#[test]`s; add any assertion helpers they need — `cursor()`, `folded()`, `reconcile` field accessors).

- [ ] **Step 1: Journey — resize → no blank (the regression).**
```rust
#[test]
fn e2e_resize_does_not_blank_the_screen() {
    let mut h = Harness::new("hello", None, (80, 24));
    assert!(h.screen_contains("hello"));
    h.resize(80, 24);  // SAME dims — the SIGWINCH class that blanked via a stale layout_key
    assert!(h.screen_contains("hello"), "same-dim resize blanked the screen:\n{:#?}", h.screen());
    h.resize(100, 30); // different dims
    assert!(h.screen_contains("hello"), "resize blanked the screen:\n{:#?}", h.screen());
}
```

- [ ] **Step 2: Journey — reconcile convergence (non-vacuous: machinery ran + divergent tree).** Add reconcile-field accessors to `Harness`, plant a wrong tree, and assert both the machinery-ran flags AND the content converged:
```rust
    // helpers on Harness:
    fn maybe_stale(&self) -> bool { self.editor.active().reconcile.maybe_stale }
    fn in_flight(&self) -> Option<u64> { self.editor.active().reconcile.in_flight_version }
    fn reconcile_blocks_version(&self) -> u64 { self.editor.active().reconcile.blocks_version }
    fn version(&self) -> u64 { self.editor.active().document.version }
    fn rope(&self) -> ropey::Rope { self.editor.active().document.buffer.snapshot() } // TextBuffer::snapshot() -> ropey::Rope (Codex)

#[test]
fn e2e_reconcile_converges_a_stale_tree() {
    let mut h = Harness::new("# A\n\nbody\n", None, (80, 24));
    // Plant a deliberately-wrong tree + mark stale (mirrors reconcile.rs:104-126).
    {
        let b = h.editor.active_mut();
        // A deliberately-wrong tree (empty), mirroring reconcile.rs:104-126's plant.
        let len = b.document.buffer.len();
        b.document.set_blocks(wordcartel_core::block_tree::empty_tree(len));
        b.reconcile.maybe_stale = true;
    }
    // Precondition: genuinely divergent from a full parse of the real text.
    let want = full_parse_rope(&h.rope());
    assert_ne!(h.blocks(), &want, "planted tree must differ from full_parse (else vacuous)");
    // Drive the debounce: one step to arm (advance sets due_at), then advance past it + tick to dispatch.
    h.tick();                                   // advance arms due_at = now + 150
    h.advance_ms(crate::reconcile::RECONCILE_DEBOUNCE_MS + 1);
    h.tick();                                   // now >= due_at → reduce dispatches reparse; InlineExecutor runs it; reduce drains
    // Machinery ran:
    assert!(!h.maybe_stale());
    assert!(h.in_flight().is_none());
    assert_eq!(h.reconcile_blocks_version(), h.version());
    // Content converged:
    assert_eq!(h.blocks(), &full_parse_rope(&h.rope()));
}
```
(Plan-confirm 4 — EMPIRICAL: verify the exact tick count to arm-then-dispatch against the real `reduce(Msg::Tick)` → reconcile-due → `dispatch_reconcile` path (it may arm+fire in one or two ticks depending on when the due check runs relative to `advance`'s arming). Adjust the `tick()`/`advance_ms` sequence until the machinery-ran assertions hold — do NOT weaken them. Confirm the `Rope` snapshot accessor + `ropey`/`text::Rope` path.)

- [ ] **Step 3: Journey — undo/redo (coalesced under the frozen clock).**
```rust
#[test]
fn e2e_undo_redo() {
    let mut h = Harness::new("", None, (80, 24));
    h.type_str("abc");                 // frozen clock → ONE coalesced revision (COALESCE_MS=500)
    assert_eq!(h.doc_text(), "abc");
    h.ctrl('z');                       // undo → reverts the whole coalesced insert
    assert_eq!(h.doc_text(), "");
    assert!(!h.screen_contains("abc"), "undone text must be gone from the screen");
    h.ctrl('y');                       // redo
    assert_eq!(h.doc_text(), "abc");
}
```
(Plan-confirm: confirm `ctrl('z')`/`ctrl('y')` are undo/redo in the CUA keymap; confirm the single-revision coalescing so undo → `""`, not `"ab"`.)

- [ ] **Step 4: Journey — quit-dirty modal (`quit_multi`, no data loss).**
```rust
#[test]
fn e2e_quit_dirty_raises_modal_not_silent_quit() {
    let mut h = Harness::new("x", None, (80, 24));
    h.type_str("y");                   // dirty
    let keep = h.ctrl('q');
    assert!(keep, "dirty Ctrl+Q must NOT quit silently");
    assert!(h.editor.prompt.is_some(), "dirty Ctrl+Q must raise the quit_multi modal");
    // Discard path: 'r' (review each) → 'd' (discard) quits.
    h.key(KeyCode::Char('r'));
    let keep2 = h.key(KeyCode::Char('d'));
    assert!(!keep2, "review→discard must quit");
}
```
(Plan-confirm 6 — EMPIRICAL: the exact modal choice keys — `quit_multi` is `[A]ll-save / [R]eview-each / [C]ancel` (prompt.rs:66), review is `[S]/[D]/[C]` (prompt.rs:78). Verify `r` then `d` drives discard-and-quit; verify `editor.prompt` is the right field. A separate test can drive `a` (all-save) → asserts the file written then quits, if a path-backed buffer is used.)

- [ ] **Step 5: Journey — fold hides lines in render.**
```rust
    // helper on Harness:
    fn folded(&self) -> &std::collections::BTreeSet<usize> { self.editor.active().folds.folded() }

#[test]
fn e2e_fold_hides_body_in_render() {
    let mut h = Harness::new("# Head\n\nsecret body line\n\n# Other\n", None, (80, 24));
    // Cursor is at the top (byte 0, inside "# Head"); Alt+Z folds that section.
    h.alt('z');
    assert!(!h.folded().is_empty(), "Alt+Z must fold the heading");
    assert!(h.screen_contains("Head"), "the heading stays visible");
    assert!(!h.screen_contains("secret body line"), "the folded body must be hidden:\n{:#?}", h.screen());
}
```
(Plan-confirm: Alt+Z = `fold_toggle` (keymap.rs:305); confirm the initial cursor is inside the first heading's section so the toggle folds it, and that `folds.folded()` is the accessor.)

- [ ] **Step 6: Run + gates + commit.**
Run: `cargo test -p wordcartel` (green, all 7 journeys) + `cargo clippy --workspace --all-targets` (clean).
```bash
git add -A
git commit -m "test(e2e): resize/reconcile/undo/quit/fold seed journeys"   # + trailers
```

---

## Self-Review

**Spec coverage:** Component 1 `advance` extraction (T1, verbatim app.rs:2157-2174, note_undo_eviction stays direct) ✓; Component 2 `test_support` lift + `Harness` (T2, spec I3 diag-off in `new`, resize dual-update, fingerprint lifecycle in the save journey) ✓; Component 3 all 7 journeys incl. the non-vacuous reconcile (machinery-ran + divergent plant, spec I1), the `quit_multi` shape (spec minor), the coalesced undo (spec minor), the resize regression, fold-in-render ✓; scheduler boundary is documented in the spec (not a code task) ✓.

**Placeholder scan:** the two EMPIRICAL steps (journey 4 tick timing, journey 6 modal keys) carry explicit "verify against source, adjust the construction, never weaken the assertion" notes — the assertions are fixed, the driving sequence is validated under cargo. Accessor spellings are flagged for plan-confirm against real signatures. No vague/TODO steps.

**Type consistency:** `advance(&mut Editor, &dyn Clock)`; `reduce(...) -> bool`; `TestClock(pub(crate) u64)`; `key_char(char) -> KeyEvent`, `press(KeyCode, KeyModifiers) -> Msg`; `build_keymap(...) -> (KeyTrie, Vec<String>)` destructured; `InlineExecutor::default()`; `full_parse_rope(&Rope) -> BlockTree` + `BlockTree: PartialEq`; `set_blocks` (editor.rs:90); `editor.diag_cfg.enabled` (on Editor, not the buffer). `TestBackend::resize`/`backend_mut()`/`buffer()[(x,y)].symbol()` per ratatui 0.30.

**Ordering:** T1 (extraction) is independent + behavior-preserving (its checkpoint proves it); T2 needs T1's `advance` + the lifted helpers; T3 needs T2's `Harness`. Each task ends green + clippy-clean.
