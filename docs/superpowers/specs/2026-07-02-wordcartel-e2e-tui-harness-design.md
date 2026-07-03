# e2e / TUI test harness — design

**Status:** approved design (pre-spec-review)
**Date:** 2026-07-02
**Effort:** e2e/TUI harness (the campaign's one untouched frontier — nothing exercises the live `wcartel` binary's `reduce → rebuild → render` pipeline end-to-end)

## Context

The `wordcartel` shell has extensive unit coverage BELOW the app loop: `app.rs`'s
`#[cfg(test)]` module (~60 tests) drives `reduce()` with synthetic `crossterm` `KeyEvent`s +
a `TestClock` + `InlineExecutor` and asserts on `editor.*` state; `render.rs`'s tests draw to
a ratatui `TestBackend` and assert on the cell grid. But **nothing combines them** — no test
runs a real input through the FULL per-iteration loop body (`reduce → note_undo_eviction →
recompute_scrollbar_visible → derive::rebuild → reconcile-arming → render`) and asserts on the
result. That combined flow is exactly where integration regressions hide: the Resize
screen-blank Critical (a `reduce(Resize) → derive::rebuild → render` interaction, caught late
by the editing-responsiveness Fable review) was invisible to every unit test.

The map confirms the architecture already supports an IN-PROCESS harness with essentially no
refactoring: `Editor` is pure state (does NOT own the `Terminal`); `reduce(msg, editor, reg,
keymap, &dyn Executor, &dyn Clock, &Sender<Msg>) -> bool` and `render(&mut Frame, &mut Editor)`
are both terminal-independent; `TestBackend` + constructable `KeyEvent`/`Event::Resize`/
`Event::Paste` are already used in-tree. A PTY harness (spawn the real binary, scrape the
screen) is strictly worse here — non-deterministic for the 150 ms reconcile debounce, can't
assert internal state, can't inject background-job completions — its only unique coverage is
real-terminal startup, deferred.

## Goals

- A reusable in-process **`Harness`** that drives the REAL `reduce` + the REAL post-`reduce`
  loop body + `render` per injected event, and asserts on both `Editor` state AND the rendered
  `TestBackend` cell grid.
- A seed suite of the highest-value end-to-end journeys pinning the project's stated invariants
  (instant typing, no data loss, no silent UI waits, reconcile convergence) + the regression
  classes we've hit (Resize blank, reconcile staleness).
- Fidelity: the harness exercises the SAME loop body `run()` does (no re-implementation/drift),
  via one small behavior-preserving extraction.

## Non-goals

- No PTY / real-binary startup coverage (deferred — a handful of smoke tests at most, not a
  harness).
- Not the broad-scope journeys (find/replace, multi-buffer switch, mouse→caret, navigation/
  scroll, worker-panic isolation) — trivial follow-ons once the harness lands.
- No threading of the `Fs` seam (fsx.rs) into the app/editor — saves hit the real filesystem
  via `tempfile` (already a dev-dependency; matches the existing save test).
- No new shipping behavior — Component 1 is a verified-identical extraction.

## Component 1 — extract `advance` from `run()`'s loop body

`run()` (app.rs:1846) has a per-iteration body (~app.rs:2153-2175) that interleaves
state-affecting steps with terminal I/O. Extract the terminal-INDEPENDENT, state-affecting
steps into a shared function both `run()` and the harness call:

```
pub(crate) fn advance(
    editor: &mut Editor,
    clock: &dyn Clock,
    pre_id: crate::editor::BufferId,
    pre_version: u64,
) {
    editor.note_undo_eviction(pre_id, pre_version);
    recompute_scrollbar_visible(editor, clock.now_ms());
    derive::rebuild(editor);
    // reconcile-debounce arming (the existing `if maybe_stale && … { due_at = now + …}` block)
    …arm reconcile…
}
```

- **Moves into `advance`** (verbatim, same order): `note_undo_eviction(pre_id, pre_version)`,
  `recompute_scrollbar_visible(editor, clock.now_ms())`, `derive::rebuild(editor)`, and the
  reconcile-debounce arming block.
- **Stays in `run()`** (terminal-coupled or loop-control), unchanged, around the `advance`
  call: the pre-`reduce` `(pre_id, pre_version)` snapshot; `drain_clipboard_intents(…,
  backend_mut(), …)`; `reconcile_mouse_capture(…, backend_mut(), …)`; `guard.terminal().draw(…)`;
  the next-iteration `recv_timeout` deadline computation; session persistence on `saved_version`
  advance.
- `run()`'s body becomes `snapshot → reduce → advance(editor, &clock, pre_id, pre_version) →
  [clipboard drain / mouse capture / draw / session]`. **Byte-identical**: the extracted lines
  move verbatim, called from the same point with the same arguments.

Correctness proof: the existing ~60 `app.rs` tests + the full suite stay green (a pure move).

## Component 2 — the `Harness` (in `#[cfg(test)] mod e2e`)

A new file `wordcartel/src/e2e.rs`, declared `#[cfg(test)] mod e2e;` in `lib.rs`, so it sees
`pub(crate)` (incl. `advance`) + reuses the existing test helpers (`InlineExecutor`, the
key-event builders, `Registry::builtins`, `keymap::build_keymap`, `Msg`).

```
struct Harness {
    editor: Editor,
    reg: Registry,
    keymap: crate::keymap::KeyTrie,
    ex: InlineExecutor,                            // runs jobs synchronously; reduce drains
    term: ratatui::Terminal<ratatui::backend::TestBackend>,
    tx: std::sync::mpsc::Sender<Msg>,
    rx: std::sync::mpsc::Receiver<Msg>,            // held so background Msgs are observable
    now: u64,                                      // virtual clock (ms)
}
```

- **`Harness::new(text: &str, path: Option<PathBuf>, (w, h): (u16, u16)) -> Self`** — build
  `Editor::new_from_text(text, path, (w, h))`, `Registry::builtins()`,
  `keymap::build_keymap(&KeymapConfig::default(), &reg)`, `InlineExecutor::default()`,
  `Terminal::new(TestBackend::new(w, h))`, an `mpsc::channel()`; `now = 0`; run the initial
  `derive::rebuild(&mut editor)` + first `render`.
- **`step(&mut self, msg: Msg) -> bool`** — the core primitive, the shared production sequence:
  snapshot `(pre_id, pre_version)` from `editor.active()` → `let keep = reduce(msg, &mut
  self.editor, &self.reg, &self.keymap, &self.ex, &TestClock(self.now), &self.tx)` → `advance(
  &mut self.editor, &TestClock(self.now), pre_id, pre_version)` → render to `self.term`. Return
  `keep`. All input methods are sugar over `step`.
- **Input sugar:** `type_str(&str)`, `key(char)`, `ctrl(char)`, `alt_shift(char)`,
  `press(KeyCode, KeyModifiers)`, `resize(u16, u16)` (→ `step(Msg::Input(Event::Resize(w, h)))`),
  `paste(&str)` (→ `Event::Paste`). Each builds the `KeyEvent`/`Event` via the existing helpers
  and calls `step`.
- **Clock / async:** `advance_ms(ms)` bumps `self.now`; `tick()` = `advance_ms(…)` then
  `step(Msg::Tick)` (drives the 150 ms reconcile debounce — `InlineExecutor` runs the reparse
  inline, `reduce`'s trailing `ex.drain()` merges it); `drain_msgs()` — `while let Ok(m) =
  self.rx.try_recv() { self.step(m); }` so worker/clipboard completions are observable.
- **State assertions:** `doc_text() -> String` (`editor.active().document.buffer.to_string()`),
  `cursor()`, `dirty()`, `saved_version()`, `status() -> &str`, `prompt() -> Option<…>`,
  `folded() -> &BTreeSet<usize>`, `blocks_generation() -> u64`.
- **Screen assertions:** `row(y: u16) -> String` (collect `term.backend().buffer()[(x, y)]
  .symbol()` across the width), `screen() -> Vec<String>`, `screen_contains(&str) -> bool`,
  `nonblank_cells() -> usize` (count cells whose symbol is not a space).

## Component 3 — the 7 seed journeys

Each a `#[test]` over the `Harness`, asserting on BOTH state and the rendered buffer:

1. **type → render:** `type_str("hello")` → `doc_text() == "hello"` AND `screen_contains("hello")`.
2. **save → reload (real fs):** `Harness::new("", Some(tmp), (80,24))`; `type_str("hello\n")`;
   `ctrl('s')` → assert `fs::read_to_string(tmp) == "hello\n"`, `status()` shows saved, `!dirty()`,
   `saved_version` advanced. Then `Harness::new(open the same file)` → `doc_text() == "hello\n"`.
3. **resize → no blank (regression):** `type_str("hello")`; `render`; `resize(80, 24)` (SAME
   dims) then `resize(100, 30)` (different) → after each, assert `nonblank_cells() > 0` AND
   `screen_contains("hello")` (pins the SIGWINCH screen-blank class).
4. **reconcile convergence:** build a doc + edit that yields a stale (provisional) tree (a
   `Local`/widen case); assert the tree is provisional; `advance_ms(RECONCILE_DEBOUNCE_MS);
   tick()` → assert `editor.active().document.blocks() == &full_parse_rope(rope)` (converged)
   and the render reflects the settled tree.
5. **undo/redo:** `type_str("abc")`; `ctrl('z')` → `doc_text()` reverts + render matches;
   `ctrl('y')` → restored.
6. **quit-dirty modal (no data loss):** `type_str("x")` (dirty); `let keep = ctrl('q')` → assert
   `keep == true` (did NOT quit silently) AND `prompt()` is the dirty-quit modal; drive the
   discard choice → quits; a separate run drives the save choice → file written then quits.
7. **fold hides lines in render:** a multi-heading doc; fold a heading (the fold key) → assert
   `folded()` contains the anchor AND the folded body lines are ABSENT from `screen()` while the
   heading line is present (FoldView through render).

## Determinism

No real threads / clock / terminal: `InlineExecutor` runs jobs synchronously on `dispatch`;
the virtual `now` + `TestClock(now)` make the debounce exact; `TestBackend` is a pure cell grid.
Saves use `tempfile`. Every journey is fully deterministic and hermetic (except the real-fs
tempfile, which is isolated per test).

## Testing

The harness IS the test infrastructure; the 7 journeys are its proof + the regression net.
GATE: `cargo test -p wordcartel` green (incl. the new `e2e` module), `cargo build`/`test --no-run`
warning-free, workspace clippy clean. No production behavior change (Component 1 is verified
byte-identical by the existing suite).

## Decomposition (3 tasks)

1. **Extract `advance`** from `run()` — behavior-preserving; existing ~60 app tests + full suite
   green (the proof).
2. **Build the `Harness`** (Component 2) + 2 smoke journeys (type→render, save→reload) proving
   the driver end-to-end.
3. **The remaining 5 journeys** (resize-no-blank, reconcile-convergence, undo/redo, quit-dirty,
   fold-in-render).

## Global constraints

- `cargo test -p wordcartel` green; workspace clippy **deny** gate clean; no `cargo fmt`; house
  style (em-dash `—`); `#![forbid(unsafe_code)]` unaffected (core untouched).
- New scaffolding is `#[cfg(test)]`-gated — no shipping-build surface added; `advance` is the
  only non-test change (`pub(crate)`, in `app.rs`).

## Plan-confirms (resolve during the implementation plan, against real source)

1. The EXACT loop-body line range to extract (app.rs ~2153-2175): confirm the state-affecting
   steps (`note_undo_eviction`, `recompute_scrollbar_visible`, `derive::rebuild`, reconcile
   arming) are contiguous-enough to move verbatim into `advance`, and that the terminal-coupled
   steps (clipboard drain, mouse capture, draw, session persist) + the deadline computation stay
   in `run()` without reordering that changes behavior. Confirm `note_undo_eviction`'s signature
   + that `pre_id`/`pre_version` are the values it needs.
2. Confirm the reconcile-arming block's exact shape (the `if b.reconcile.maybe_stale && … {
   b.reconcile.due_at = Some(now + RECONCILE_DEBOUNCE_MS); … }`) and that moving it into `advance`
   (which computes `now` via `clock.now_ms()`) leaves the deadline computation in `run()` intact.
3. `reduce`/`render`/`Editor::new_from_text`/`keymap::build_keymap`/`InlineExecutor`/`TestClock`/
   the key-event builder helpers — confirm exact signatures + visibility from a new
   `#[cfg(test)] mod e2e` (they're used by the existing `app.rs`/`render.rs` test modules; confirm
   `e2e.rs` as a sibling `#[cfg(test)] mod` can reach them, or whether any are private to
   `app::tests` and need lifting to a shared test-support location).
4. The fold key + the reconcile-provisional condition for journeys 7 and 4 — confirm the exact
   key (Alt+Shift+Z per the map's fold_toggle/fold_all) and an edit that reliably yields a
   provisional/stale tree that the tick converges (a `Local`-reason edit).
5. `Registry`/`KeyTrie`/`KeymapConfig`/`Msg`/`BufferId` import paths + whether `render::render`
   and `advance` are reachable un-`pub`-ified from `e2e.rs` (same crate).
