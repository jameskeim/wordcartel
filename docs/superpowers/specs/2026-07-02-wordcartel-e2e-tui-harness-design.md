# e2e / TUI test harness — design

**Status:** Codex spec-review CLEAN (round 2, ready for planning); Fable5 pass pending
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
state-affecting steps with terminal I/O. Codex-round-1 correction: the real order is
`reduce → note_undo_eviction → drain_clipboard_intents → reconcile_mouse_capture →
recompute_scrollbar_visible → derive::rebuild → reconcile-arming → draw` — i.e.
`note_undo_eviction` runs BEFORE the clipboard/mouse steps, while the other three state steps
run AFTER them (and are already CONSECUTIVE). So extract ONLY the three consecutive post-mouse
state steps (moving `note_undo_eviction` into the same function would force reordering the
state-mutating clipboard/mouse steps):

```
pub(crate) fn advance(editor: &mut Editor, clock: &dyn Clock) {
    recompute_scrollbar_visible(editor, clock.now_ms());
    derive::rebuild(editor);
    // reconcile-debounce arming (the existing `if b.reconcile.maybe_stale && … {
    //   b.reconcile.due_at = Some(now + RECONCILE_DEBOUNCE_MS); … }` block — plan-confirm 2)
    …arm reconcile…
}
```

- **Moves into `advance`** (verbatim, same order — already consecutive at app.rs ~:2160-2174):
  `recompute_scrollbar_visible(editor, clock.now_ms())`, `derive::rebuild(editor)`, the
  reconcile-debounce arming block. NO reordering.
- **Stays in `run()`, unchanged, at their exact current points:** the pre-`reduce`
  `(pre_id, pre_version)` snapshot; `editor.note_undo_eviction(pre_id, pre_version)` (a direct
  method call, BEFORE clipboard — NOT moved); `drain_clipboard_intents(…, backend_mut(), …)`;
  `reconcile_mouse_capture(…, backend_mut(), …)`; then `advance(&mut editor, &clock)`; then
  `guard.terminal().draw(…)`; the next-iteration `recv_timeout` deadline computation; session
  persistence.
- `run()`'s body becomes `snapshot → reduce → note_undo_eviction → clipboard → mouse →
  advance(editor, &clock) → draw → session`. **Byte-identical**: `advance` is a verbatim move of
  three already-consecutive lines, called from the same point.

**Harness fidelity boundary (documented):** the harness replays `snapshot → reduce →
note_undo_eviction → advance → render`, OMITTING the two terminal-output steps
(`drain_clipboard_intents`, `reconcile_mouse_capture`) that run between `note_undo_eviction`
and `advance` in `run()`. `reconcile_mouse_capture` mutates only mouse drag/capture bookkeeping
(app.rs:2222). `drain_clipboard_intents` mutates clipboard-sync state and CAN set
`editor.status = "clipboard unavailable"` — but ONLY when a pending clipboard intent is drained
against a closed clip channel (clipboard.rs:40); **none of the 7 seed journeys create a pending
clipboard intent before an assertion**, so omitting it never changes observed state (incl. the
save journey's `status()`) for this suite. Clipboard/mouse journeys (follow-on, non-goal here)
will add those steps to the harness with a mock clip channel + TestBackend.

Correctness proof: the existing ~60 `app.rs` tests + the full suite stay green (a pure move).

## Component 2 — the `Harness` (in `#[cfg(test)] mod e2e`)

A new file `wordcartel/src/e2e.rs`, declared `#[cfg(test)] mod e2e;` in `lib.rs`, so it sees
`pub(crate)` (incl. `advance`). `InlineExecutor` is already `pub` (jobs.rs:85) → reachable.
BUT (Codex round 1) `TestClock` + the key-event builders (`key_char`, `press`, …) live inside
`app.rs`'s PRIVATE `#[cfg(test)] mod tests` — a sibling `#[cfg(test)] mod e2e` cannot see them.
**Fix: lift them to a shared `#[cfg(test)] pub(crate) mod test_support`** (a new file
`wordcartel/src/test_support.rs`, `#[cfg(test)] pub(crate) mod test_support;` in `lib.rs`),
holding `TestClock` + the key-event builders; update `app.rs`'s `mod tests` to import them from
there (mechanical). The harness uses the PUBLIC `keymap::build_keymap` directly (NOT the
test-only `cua_keymap`), which returns `(KeyTrie, Vec<String>)` — destructure and drop the
warnings.

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
  `let (keymap, _warn) = keymap::build_keymap(&KeymapConfig::default(), &reg)` (destructure the
  `(KeyTrie, Vec<String>)` tuple), `InlineExecutor::default()`, `Terminal::new(TestBackend::new(
  w, h))`, an `mpsc::channel()`; `now = 0`; run the initial `derive::rebuild(&mut editor)` +
  first `render`.
- **`step(&mut self, msg: Msg) -> bool`** — the core primitive, the shared production sequence:
  snapshot `(pre_id, pre_version)` from `editor.active()` → `let keep = reduce(msg, &mut
  self.editor, &self.reg, &self.keymap, &self.ex, &TestClock(self.now), &self.tx)` →
  `self.editor.note_undo_eviction(pre_id, pre_version)` (the direct method call `run()` makes
  before clipboard/mouse) → `advance(&mut self.editor, &TestClock(self.now))` → render to
  `self.term`. Return `keep`. All input methods are sugar over `step`.
- **Input sugar:** `type_str(&str)`, `key(char)`, `ctrl(char)`, `alt(char)`, `alt_shift(char)`,
  `press(KeyCode, KeyModifiers)`, `paste(&str)` (→ `Event::Paste`). Each builds the
  `KeyEvent`/`Event` via the `test_support` builders and calls `step`.
- **`resize(&mut self, w: u16, h: u16)` (Codex round 1 — MUST update both):** call
  `self.term.backend_mut().resize(w, h)` (so the `TestBackend` cell grid matches) AND
  `self.step(Msg::Input(Event::Resize(w, h)))` (so the editor's buffer areas update). Omitting
  either desyncs the editor geometry from the rendered buffer.
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
   `ctrl('s')` (the save job runs inline under `InlineExecutor`, drained by `reduce` before the
   step returns — save.rs:71/98, app.rs:1797) → assert `fs::read_to_string(tmp) == "hello\n"`,
   `status() == "Saved"` (exact string, save.rs:98), `!dirty()`, `saved_version` advanced. Then
   `Harness::new(open the same file)` → `doc_text() == "hello\n"`.
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
7. **fold hides lines in render:** a multi-heading doc; fold ONE heading via **Alt+Z**
   (`fold_toggle`; keymap.rs:305 — NOT Alt+Shift+Z, which is `fold_all`) → assert `folded()`
   contains the anchor AND the folded body lines are ABSENT from `screen()` while the heading
   line is present (FoldView through render).

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

1. **Extract `advance`** (the 3 consecutive post-mouse state steps) from `run()` — behavior-
   preserving; existing ~60 app tests + full suite green (the proof).
2. **Lift `TestClock` + the key-event builders to `#[cfg(test)] pub(crate) mod test_support`**
   (update `app::tests` to import them) + **build the `Harness`** (Component 2) + 2 smoke
   journeys (type→render, save→reload) proving the driver end-to-end.
3. **The remaining 5 journeys** (resize-no-blank, reconcile-convergence, undo/redo, quit-dirty,
   fold-in-render).

## Global constraints

- `cargo test -p wordcartel` green; workspace clippy **deny** gate clean; no `cargo fmt`; house
  style (em-dash `—`); `#![forbid(unsafe_code)]` unaffected (core untouched).
- New scaffolding is `#[cfg(test)]`-gated — no shipping-build surface added; `advance` is the
  only non-test change (`pub(crate)`, in `app.rs`).

## Plan-confirms (resolve during the implementation plan, against real source)

1. The EXACT lines to extract (app.rs ~:2160-2174): confirm `recompute_scrollbar_visible`,
   `derive::rebuild`, and the reconcile-arming block are the three CONSECUTIVE post-mouse steps
   (Codex round 1 confirmed this order), so `advance` is a verbatim move with NO reordering;
   `note_undo_eviction(pre_id, pre_version)` stays a direct call BEFORE the clipboard/mouse steps.
   Confirm the deadline computation for the next `recv_timeout` reads `b.reconcile.due_at` AFTER
   `advance` sets it (so leaving it in `run()` post-`advance` is correct).
2. Confirm the reconcile-arming block's exact shape (the `if b.reconcile.maybe_stale && … {
   b.reconcile.due_at = Some(now + RECONCILE_DEBOUNCE_MS); … }`) and that moving it into `advance`
   (which computes `now` via `clock.now_ms()`) leaves the deadline computation in `run()` intact.
3. RESOLVED (Codex round 1): `InlineExecutor` is `pub` (jobs.rs:85); `TestClock` + the
   key-event builders are private to `app::tests` and MUST be lifted to `#[cfg(test)] pub(crate)
   mod test_support` (Task 2). Confirm the exact set of helpers `app::tests` uses so the lift
   updates every reference, and that `build_keymap` (public) suffices for the harness keymap
   (no `cua_keymap` needed).
4. The fold key + the reconcile-provisional condition for journeys 7 and 4 — confirm the exact
   key (Alt+Shift+Z per the map's fold_toggle/fold_all) and an edit that reliably yields a
   provisional/stale tree that the tick converges (a `Local`-reason edit).
5. `Registry`/`KeyTrie`/`KeymapConfig`/`Msg`/`BufferId` import paths + whether `render::render`
   and `advance` are reachable un-`pub`-ified from `e2e.rs` (same crate).
