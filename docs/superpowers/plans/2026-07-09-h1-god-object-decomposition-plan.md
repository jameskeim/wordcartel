# H1 (round 2) — god-object decomposition: implementation plan

**Spec:** `docs/superpowers/specs/2026-07-09-h1-god-object-decomposition-design.md` (APPROVED,
Codex-clean). **Branch:** `effort-h1-god-object-decomposition`. **Facts as of:** `745698d`
(app.rs = 5,521 lines, ~1,946 production [1–1946] + tests from 1948; render.rs = 3,432 lines,
~1,048 production [1–1048] + tests from 1049).

## Goal

Reorganize the two god-objects (`app.rs`, `render.rs`) into focused files plus a
plugin-forward timer seam and a readable `reduce` skeleton — behavior-IDENTICAL, landing
before Effort P.

## Architecture

`reduce`'s ten interception stages move their bodies into per-feature modules behind a
`Handled { Done(bool) | Pass(Msg) }` protocol, leaving a thin ordered skeleton in app.rs;
the shared stage micro-epilogue becomes `fold_and_continue`. `run()`'s eight inline
deadline terms become a static `SUBSYSTEMS: &[TimedSubsystem]` fn-pointer table in a new
`timers.rs` with `next_wake`/`on_tick`/`pre_recv` free fns. Leaf helpers (theme/keymap
request-flags, chrome recompute, session persist, render geometry + status builders) move
verbatim to leaf modules; the `Input(Key)` arm and the duplicated overlay list-nav are the
two deeper cuts.

## Tech Stack

Rust (workspace: pure `wordcartel-core` + shell `wordcartel`, binary `wcartel`), ratatui
0.30, crossterm. Tests: `cargo test` (in-crate `#[cfg(test)]` modules + e2e journeys +
golden TestBackend). No new dependencies.

---

## Global Constraints (in scope for EVERY task — re-read before each)

- **House style (hand-formatted, NOT rustfmt):** snake_case fns/vars/modules, PascalCase
  types, SCREAMING_SNAKE_CASE consts; 4-space indent; ~100-col lines hand-wrapped with
  judgment (keep single-line match arms / struct literals inline where they read better);
  imports grouped by hand; em-dash `—` in prose comments, never `--`; no emoji. **Do NOT
  run `cargo fmt`** (the repo has no `rustfmt.toml`; it reflows 1000+ hunks and destroys
  blame). Match neighbors by hand; do not reflow code you did not otherwise change.
- **Behavior-IDENTICAL.** Every move is a verbatim cut-and-paste (or a mechanically
  equivalent restructuring pinned by a test written first). The load-bearing invariants
  A–L from spec §8 MUST hold — in particular: (A) stage early-returns skip the
  version-change hook; (B) deadline fire-site heterogeneity + dwell-before-visibility fire
  order; (C) the search Esc/Alt+a returns do NOT drain; (D) interception order
  pending_mark→menu→palette→theme_picker→file_browser→prompt→minibuffer→search→diag→outline;
  (E) anti-spin gates travel with their deadlines, idle → `next_wake == None`; (F)
  `InputThreadDied` intercepted pre-reduce; (G) e2e sequence `reduce →
  rebuild_keymap_if_requested → note_undo_eviction → advance → render` frozen; (H)
  per-overlay list-nav side effects; (I) outline buffer-mismatch pre-close; (J)
  prompt-modal background merge; (K) golden render parity; (L) pending_mark drains per key
  MESSAGE incl. non-Press.
- **Command-surface contract = N/A** (`docs/design/command-surface-contract.md`): this
  effort does not change the registry, palette builder, menu builder, option setters, or
  hint resolution — only code that *dispatches* commands relocates. The contract's
  invariant tests (palette-completeness, every-option-has-a-command, hint re-resolution)
  stay in place and unmodified.
- **Merge GATEs (must pass at every task boundary):** `cargo test --workspace` green;
  `cargo clippy --workspace --all-targets` clean (deny-level; item-local `#[allow]` with a
  one-line rationale only if truly needed); `cargo build` + `cargo test --no-run`
  warning-free for the crate(s) touched (this effort touches only `wordcartel`).
- **`#![forbid(unsafe_code)]`** holds in `wordcartel-core`; write no `unsafe` anywhere.
- **No re-export shims.** Callers repoint their `crate::app::…` / `crate::render::…` paths
  directly (the prior-H1 precedent). New modules are declared in `lib.rs`. Each new/refilled
  module opens with a one-line provenance doc — e.g. `//! Extracted verbatim from app.rs
  (Effort H1 round 2).`
- **Commits:** one per task (or one per module inside a task, matching prior H1). Every
  commit message ends with the project trailers verbatim per `CLAUDE.md` (the
  `Co-Authored-By:` line then a `Claude-Session:` line with the current session URL from
  the environment). Commit/push only as part of executing this plan; do not push unless
  asked.
- **PTY smoke** (`scripts/smoke/run.sh`) is mandatory-run / advisory-pass — the pre-merge
  report quotes its one-line summary; a red result is surfaced, never a silent blocker.

---

## T1 — Guardrail pins (tests only, written against CURRENT code, all green pre-refactor)

Pins the load-bearing invariants that later hub tasks must not break, so they exist BEFORE
timers/reduce land. All four new tests must PASS against the unmodified source.

**Files:**
- Modify (tests only): `wordcartel/src/app.rs` — append four `#[test]` fns inside the
  existing `mod tests` (which ends the file; add before its closing `}`). Helpers
  `cua_keymap()`, `TestClock`, `Msg`, `press`, `key_char` are already in scope
  (app.rs:1949–1959).

**Interfaces:** Consumes only existing public API (`crate::app::reduce`,
`crate::editor::Editor`, `crate::jobs::{Executor, InlineExecutor, Job, JobOutcome, JobKind}`,
`crate::palette::{Palette, PaletteRow}`, `crate::registry::{CommandId, Registry}`). Produces
no new interface.

**Steps:**

1. Add the G1 pin — palette-dispatched edit skips the version hook (§8.1-A). It sets a
   single `delete_line` palette row and presses Enter; after `reduce`, the document version
   advances but `last_edit_at` stays `None` (the stage returned at app.rs:584 before the
   :1209 hook).

   ```rust
   /// §8.1-A guardrail: a command dispatched via the palette stage returns through the
   /// stage micro-epilogue (app.rs:584) and SKIPS the version-change hook (app.rs:1209),
   /// so the edit bumps `document.version` WITHOUT setting `last_edit_at`. Do not unify
   /// the stage return with the main epilogue — this asymmetry is behavior.
   #[test]
   fn palette_dispatched_edit_skips_version_hook() {
       use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
       use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
       let mut e = Editor::new_from_text("alpha\nbeta\n", None, (80, 24));
       e.active_mut().diagnostics = crate::diagnostics_run::DiagStore::new(); // clean debounce baseline
       let before_ver = e.active().document.version;
       assert!(e.active().last_edit_at.is_none(), "precondition: no prior edit timestamp");
       // One deterministic palette row for the synchronous `delete_line` editing command.
       e.palette = Some(crate::palette::Palette::default());
       {
           let p = e.palette.as_mut().unwrap();
           p.rows = vec![crate::palette::PaletteRow {
               id: crate::registry::CommandId("delete_line"),
               label: "Delete Line".into(),
               chord: String::new(),
               buffer: None,
           }];
           p.selected = 0;
       }
       let reg = Registry::builtins(); let km = cua_keymap();
       let ex = InlineExecutor::default(); let clk = TestClock(0);
       let (tx, _rx) = std::sync::mpsc::channel();
       let enter = Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE,
           kind: KeyEventKind::Press, state: KeyEventState::NONE });
       crate::app::reduce(Msg::Input(enter), &mut e, &reg, &km, &ex, &clk, &tx);
       assert!(e.palette.is_none(), "palette dispatch closes the overlay");
       assert_ne!(e.active().document.version, before_ver, "delete_line must bump the version");
       assert!(e.active().last_edit_at.is_none(),
           "palette-dispatched edit must NOT set last_edit_at (skipped version hook — §8.1-A)");
   }
   ```

2. Add the G2 pin — the search Esc arm returns WITHOUT draining (§8.1-C). A drain-counting
   executor wraps `InlineExecutor`; search-open + Esc leaves the drain count at 0, while a
   text-insert key drains.

   ```rust
   /// §8.1-C guardrail: the search stage's Esc arm (app.rs:926) returns WITHOUT draining
   /// the executor, unlike the text-edit arms (app.rs:958). fold_and_continue (T7) must be
   /// applied ONLY to sites that drain today — never retrofit a drain onto Esc/Alt+a.
   #[test]
   fn search_esc_does_not_drain_executor() {
       use crate::editor::Editor; use crate::jobs::{Executor, InlineExecutor, Job, JobOutcome};
       use crate::registry::Registry;
       use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
       struct DrainSpy { inner: InlineExecutor, drains: std::cell::Cell<usize> }
       impl Executor for DrainSpy {
           fn dispatch(&self, job: Job) { self.inner.dispatch(job); }
           fn drain(&self) -> Vec<JobOutcome> { self.drains.set(self.drains.get() + 1); self.inner.drain() }
       }
       let ex = DrainSpy { inner: InlineExecutor::default(), drains: std::cell::Cell::new(0) };
       let mut e = Editor::new_from_text("abc\n", None, (80, 24));
       e.open_search(crate::search_overlay::Phase::Find, 0);
       let reg = Registry::builtins(); let km = cua_keymap(); let clk = TestClock(0);
       let (tx, _rx) = std::sync::mpsc::channel();
       let mk = |code: KeyCode| Event::Key(KeyEvent { code, modifiers: KeyModifiers::NONE,
           kind: KeyEventKind::Press, state: KeyEventState::NONE });
       // A text-insert key DOES drain (app.rs:958) — establishes the spy works.
       crate::app::reduce(Msg::Input(mk(KeyCode::Char('a'))), &mut e, &reg, &km, &ex, &clk, &tx);
       assert_eq!(ex.drains.get(), 1, "a search text-insert key drains once");
       // Esc returns WITHOUT draining (app.rs:926) — the count must not advance.
       crate::app::reduce(Msg::Input(mk(KeyCode::Esc)), &mut e, &reg, &km, &ex, &clk, &tx);
       assert_eq!(ex.drains.get(), 1, "search Esc must NOT drain the executor (§8.1-C)");
       assert!(e.search.is_none(), "Esc cancels the search overlay");
   }
   ```

3. Add the G3 pin — a settled, no-overlay editor produces no wake deadline (§8.1-E). This
   is the anti-spin / idle-is-free invariant, expressed against the CURRENT inline logic so
   it survives the timers move unchanged.

   ```rust
   /// §8.1-E guardrail: a clean, settled, no-overlay editor arms NO timed deadline — the
   /// run loop blocks on the 3600 s fallback (idle is free). Expressed against the current
   /// per-term gates; T8 re-expresses it as timers::next_wake(&e, now) == None.
   #[test]
   fn settled_editor_arms_no_deadline() {
       use crate::editor::Editor;
       let e = Editor::new_from_text("hello\n", None, (80, 24));
       let now = 10_000u64;
       assert!(!e.active().document.dirty(), "precondition: a fresh buffer is not dirty");
       let swap_deadline = if crate::swap::pending(
           e.active().document.dirty(), e.active().document.version, e.active().swapped_version,
       ) && !e.active().swap_in_flight {
           crate::swap::next_deadline_ms(now, e.active().last_edit_at, e.active().last_swap_at)
       } else { None };
       let sq_deadline = e.pending_after_save.as_ref().map(|p| p.at_ms.saturating_add(5_000));
       let sb_deadline = if e.mouse.scrollbar_until_ms > now { Some(e.mouse.scrollbar_until_ms) } else { None };
       let menu_deadline = e.mouse.menu_reveal_due.or(e.mouse.menu_hide_due);
       let sb_dwell = e.mouse.scrollbar_reveal_due.or(e.mouse.scrollbar_hide_due);
       let status_dwell = e.mouse.status_reveal_due.or(e.mouse.status_hide_due);
       let diag_deadline = if e.active().diagnostics.in_flight_version.is_none() {
           e.active().diagnostics.recheck_due_at } else { None };
       let reconcile_deadline = if e.active().reconcile.in_flight_version.is_none() {
           e.active().reconcile.due_at } else { None };
       let deadline = crate::diagnostics_run::next_deadline(&[
           swap_deadline, sq_deadline, sb_deadline, menu_deadline,
           sb_dwell, status_dwell, diag_deadline, reconcile_deadline,
       ]);
       assert_eq!(deadline, None, "a settled no-overlay editor must arm no deadline (idle is free — §8.1-E)");
   }
   ```

4. Add the G5 pin — outline motion does not re-query (§8.1-H). Down moves the selection and
   leaves `query` empty; a Char edit sets `query` (and re-queries). (G4 is a keep-green
   obligation on the existing `theme_picker_preview_pin_visible_row` at app.rs:4921 — no new
   test.)

   ```rust
   /// §8.1-H guardrail: outline MOTION keys (Up/Down/Page/Home/End) do NOT touch `query`
   /// or re-run set_query — only the text-edit arms (Char/Backspace) re-query. The list-nav
   /// unification (T10) must keep the query re-run OUTSIDE the shared motion helper.
   #[test]
   fn outline_motion_does_not_requery() {
       use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
       use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
       let mut e = Editor::new_from_text("# Top\nintro\n## A\nbody\n", None, (80, 24));
       crate::derive::rebuild(&mut e);
       e.open_outline();
       assert!(e.outline.as_ref().unwrap().rows.len() >= 2, "precondition: two outline rows");
       let reg = Registry::builtins(); let km = cua_keymap();
       let ex = InlineExecutor::default(); let clk = TestClock(0);
       let (tx, _rx) = std::sync::mpsc::channel();
       let mk = |code: KeyCode| Event::Key(KeyEvent { code, modifiers: KeyModifiers::NONE,
           kind: KeyEventKind::Press, state: KeyEventState::NONE });
       crate::app::reduce(Msg::Input(mk(KeyCode::Down)), &mut e, &reg, &km, &ex, &clk, &tx);
       {
           let o = e.outline.as_ref().unwrap();
           assert_eq!(o.selected, 1, "Down advances the selection");
           assert!(o.query.is_empty(), "motion must NOT populate the query (§8.1-H)");
       }
       crate::app::reduce(Msg::Input(mk(KeyCode::Char('A'))), &mut e, &reg, &km, &ex, &clk, &tx);
       assert_eq!(e.outline.as_ref().unwrap().query, "A", "a Char edit re-queries the outline");
   }
   ```

5. **Run:** `cargo test -p wordcartel palette_dispatched_edit_skips_version_hook
   search_esc_does_not_drain_executor settled_editor_arms_no_deadline
   outline_motion_does_not_requery` — expect **4 passed** (they pin CURRENT behavior).
   Then `cargo test --workspace` (green) and `cargo clippy --workspace --all-targets`
   (clean).

6. **Commit:** `git commit -m "test(app): H1 guardrail pins — version-hook skip, search no-drain, idle no-deadline, outline no-requery"` (trailers appended per Global Constraints).

---

## T2 — leaf module `theme_cmds.rs` (verbatim move)

**Files:**
- Create `wordcartel/src/theme_cmds.rs`.
- Modify `wordcartel/src/lib.rs` — add `pub mod theme_cmds;` (place after `pub mod
  theme_picker;`, line 48).
- Modify `wordcartel/src/app.rs` — DELETE `rebuild_keymap_if_requested` (:193–210),
  `rederive_theme_if_requested` (:225–245), `preview_selected_theme` (:255–274),
  `commit_theme_picker` (:279–285) from production; repoint the run-loop calls at :1677,
  :1682 to `crate::theme_cmds::…`; MOVE ONLY the seam tests that call a moved helper: the
  THREE rebuild-helper tests `switch_status_survives_the_rebuild` (:5230, calls
  `rebuild_keymap_if_requested` at :5238), `patches_survive_the_switch` (:5244, :5255),
  `rebuild_seam_swaps_the_trie_and_clears_pending` (:5262, :5275); the rederive tests
  (:5344–5400); `rederive_respects_picker_committed_theme` (:5406, `use` :5409);
  `preview_applies_ansi16_policy` (:5480, `use` :5482) — into `theme_cmds.rs`'s test module.
  **Do NOT move `save_settings_command_sets_the_request_flag` (:5287)** — it dispatches
  `CommandId("save_settings")` (:5299) and calls no moved theme_cmds helper; it stays in app.rs.
- Modify `wordcartel/src/mouse.rs` — repoint `crate::app::preview_selected_theme` (:207,
  :229) and `crate::app::commit_theme_picker` (:230) to `crate::theme_cmds::…`.
- Modify `wordcartel/src/theme_picker.rs` — repoint the test call `crate::app::preview_selected_theme`
  (:132) to `crate::theme_cmds::preview_selected_theme`.
- Modify `wordcartel/src/e2e.rs` — repoint `app::rebuild_keymap_if_requested` (:104, :129)
  to `crate::theme_cmds::rebuild_keymap_if_requested`.

**Interfaces (Produces — signatures other tasks/callers rely on, unchanged from today):**
```rust
pub(crate) fn rebuild_keymap_if_requested(editor: &mut crate::editor::Editor,
    patches: &[crate::config::KeymapPatch], reg: &crate::registry::Registry)
    -> Option<crate::keymap::KeyTrie>;
pub(crate) fn rederive_theme_if_requested(editor: &mut crate::editor::Editor,
    theme_cfg: &crate::config::ThemeConfig, env: &crate::theme_resolve::EnvSnapshot) -> bool;
pub(crate) fn preview_selected_theme(editor: &mut crate::editor::Editor);
pub(crate) fn commit_theme_picker(editor: &mut crate::editor::Editor);
```

**Steps:**

1. Create `theme_cmds.rs` with the provenance doc and the four fns cut VERBATIM from
   app.rs:193–285 (including their full doc comments — the rederive/preview docs carry the
   Ansi16-policy and picker-committed-name reasoning). Add the imports the bodies need:
   they reference `crate::editor::Editor`, `crate::config`, `crate::registry::Registry`,
   `crate::keymap`, `crate::theme_resolve`, `crate::settings`, `wordcartel_core::theme`.
   Keep them as fully-qualified `crate::…` paths exactly as in the originals (the bodies
   already spell every path out), so no new `use` is required beyond what the fns name.
2. In app.rs delete the four fns; update :1677 to `if let Some(t) =
   crate::theme_cmds::rebuild_keymap_if_requested(&mut editor, &cfg.keymap.patches, &reg)`
   and :1682 to `crate::theme_cmds::rederive_theme_if_requested(&mut editor, &cfg.theme, &env);`.
3. Move the four test blocks into a `#[cfg(test)] mod tests` in theme_cmds.rs; inside them
   repoint every `crate::app::rebuild_keymap_if_requested` /
   `crate::app::rederive_theme_if_requested` / `crate::app::preview_selected_theme` /
   `use crate::app::rederive_theme_if_requested;` (app.rs:5409) / `use crate::app::preview_selected_theme;`
   (app.rs:5482) to `crate::theme_cmds::…`. Preserve each test's helpers (they build their
   own `Editor`/`Registry`; if any uses `cua_keymap()`, inline a local `build_keymap`
   equivalent since that helper stays in app.rs's test module).
4. Repoint mouse.rs:207/:229/:230 and theme_picker.rs:132 and e2e.rs:104/:129 as listed.
5. **Run:** `cargo test -p wordcartel` (green — moved tests pass from their new home),
   `cargo clippy --workspace --all-targets` (clean), `cargo build -p wordcartel` (no warnings).
6. **Commit:** `git commit -m "refactor(app): extract theme/keymap request-flag seams to theme_cmds.rs"`.

---

## T3 — leaf module `chrome.rs` (verbatim move)

**Files:**
- Create `wordcartel/src/chrome.rs`.
- Modify `lib.rs` — add `pub mod chrome;` (after `pub mod density;`, line 38).
- Modify `app.rs` — DELETE `recompute_scrollbar_visible` (:1738–1763), `recompute_menu_bar`
  (:1767–1785), `status_line_visible` (:1790–1802), `recompute_status_line` (:1805–1820),
  `reconcile_mouse_capture` (:1827–1847) from production; repoint their in-app.rs callers:
  `advance` :1267–1269 (three `recompute_*`), run startup :1586/:1587 (scrollbar + status),
  run loop :1584/:1694 (`reconcile_mouse_capture`) → `crate::chrome::…`. MOVE the tests:
  fade/dwell recompute (:3704–3730), menu-bar recompute + reconcile (:3737–3790),
  status-line visibility (:5453–5466).
- Modify `render.rs` — repoint `crate::app::status_line_visible` (:917, :933) to
  `crate::chrome::status_line_visible`.

**Interfaces (Produces — signatures unchanged):**
```rust
pub fn recompute_scrollbar_visible(editor: &mut crate::editor::Editor, now_ms: u64);
pub fn recompute_menu_bar(editor: &mut crate::editor::Editor, now_ms: u64);
pub fn status_line_visible(editor: &crate::editor::Editor) -> bool;
pub fn recompute_status_line(editor: &mut crate::editor::Editor, now_ms: u64);
pub fn reconcile_mouse_capture<W: std::io::Write>(editor: &mut crate::editor::Editor,
    backend: &mut W, applied: &mut bool);
```

**Steps:**

1. Create `chrome.rs` with provenance doc + the five fns cut VERBATIM from app.rs:1738–1847,
   including the `recompute_scrollbar_visible` doc comment carrying the **fire-order is
   load-bearing** warning (§8.1-B). Each fn body already uses `crate::config::TransientMode`
   / `crate::config::MenuBarMode` fully qualified or via a fn-local `use crate::config::…`
   — keep those fn-local `use`s exactly as written.
2. In app.rs: delete the five fns; update `advance` (:1267–1269) to
   `crate::chrome::recompute_scrollbar_visible(editor, clock.now_ms());` etc.; update
   startup :1586/:1587 to `crate::chrome::recompute_scrollbar_visible(&mut editor,
   clock.now_ms());` / `crate::chrome::recompute_status_line(...)`; update :1584/:1694 to
   `crate::chrome::reconcile_mouse_capture(&mut editor, guard.terminal().backend_mut(),
   &mut applied_mouse);`.
3. Move the three test blocks into `chrome.rs`'s `#[cfg(test)] mod tests`, repointing every
   `crate::app::recompute_*` / `crate::app::status_line_visible` /
   `crate::app::reconcile_mouse_capture` to `crate::chrome::…`.
4. Repoint render.rs:917/:933.
5. **Run:** `cargo test -p wordcartel`, `cargo clippy --workspace --all-targets`, `cargo build -p wordcartel` — all clean.
6. **Commit:** `git commit -m "refactor(app): extract chrome recompute/visibility + mouse-capture reconcile to chrome.rs"`.

---

## T4 — micro-leaf moves: `persist_session`, `file_browser_enter`, `outline_jump_to`

**Files:**
- Modify `wordcartel/src/session_restore.rs` — ADD `persist_session` (from app.rs:1852–1894)
  and `persist_session_for_test` (app.rs:1896–1899); ADD their tests
  `persist_session_captures_scratch_even_when_active_unnamed` (app.rs:4466–4478) and
  `persist_session_clears_stale_scratch_when_oversized` (app.rs:4483–4500).
- Modify `wordcartel/src/file_browser.rs` — ADD `file_browser_enter` (from app.rs:290–324).
- Modify `wordcartel/src/outline_overlay.rs` — ADD `outline_jump_to` (from app.rs:326–334).
- Modify `app.rs` — DELETE those three fns + the `persist_session_for_test` shim from
  production/test; repoint run() calls to `persist_session` (:1701, :1718) →
  `crate::session_restore::persist_session`; repoint the interception-stage callers of
  `file_browser_enter` (:726) and `outline_jump_to` (:1051) — these move with their stages
  in T5/T6, but until then call `crate::file_browser::file_browser_enter` /
  `crate::outline_overlay::outline_jump_to`; repoint the app.rs test at :4065 that calls
  `crate::app::outline_jump_to` → `crate::outline_overlay::outline_jump_to`.
- Modify `mouse.rs` — repoint `crate::app::file_browser_enter` (:272) →
  `crate::file_browser::file_browser_enter`; `crate::app::outline_jump_to` (:323) →
  `crate::outline_overlay::outline_jump_to`.
- **No change to `session_restore.rs:151`** (`file_browser_enter_on_file_opens_it_when_clean`):
  it calls `open_into_current` directly (session_restore.rs:159), NOT `file_browser_enter`
  (Codex r2 correction) — leave it untouched.

**Interfaces (Produces — visibilities set for cross-module callers):**
```rust
// session_restore.rs
pub(crate) fn persist_session(session: &mut crate::state::SessionState,
    editor: &crate::editor::Editor, cfg: &crate::config::Config, seq: u64);
#[cfg(test)] pub fn persist_session_for_test(...); // signature verbatim from app.rs:1897
// file_browser.rs
pub(crate) fn file_browser_enter(editor: &mut crate::editor::Editor);
// outline_overlay.rs
pub fn outline_jump_to(editor: &mut crate::editor::Editor, byte: usize);
```
(Both `persist_session` and `file_browser_enter` are `pub(crate)`; app.rs `run()` and
`reduce`'s stages call in-crate. `outline_jump_to` stays `pub` as today.)

**Steps:**

1. Move `persist_session` + `persist_session_for_test` into session_restore.rs. Their
   bodies reference `crate::state`, `crate::limits`, `crate::config`, `crate::editor::Editor`
   — session_restore.rs already imports the restore side; add whatever `use` the moved
   bodies name (they spell `crate::state::…`, `crate::limits::…` fully, so likely only the
   `Editor`/`config` names). Change `persist_session`'s signature to `pub(crate)` and the
   `Editor`/`config::Config` references to `crate::editor::Editor` / `crate::config::Config`.
   Move the two tests, repointing `crate::app::persist_session_for_test` (app.rs:4478 and
   :4500) → `crate::session_restore::persist_session_for_test`.
2. Move `file_browser_enter` into file_browser.rs (VERBATIM incl. the `§3` readability
   comment). Move `outline_jump_to` into outline_overlay.rs (VERBATIM incl. its
   `record_jump`/`unfold_ancestors_of`/`ensure_visible` calls); repoint the test at
   app.rs:4065.
3. In app.rs: delete the three fns; repoint :1701/:1718 (persist), keep the stage callers
   at :726/:1051 pointing at the new modules (they relocate in T5/T6).
4. Repoint mouse.rs:272/:323.
5. **Run:** `cargo test -p wordcartel`, clippy, build — clean.
6. **Commit:** `git commit -m "refactor(app): relocate persist_session, file_browser_enter, outline_jump_to to their domain modules"`.

---

## T5 — reduce stages, overlay group (menu / palette / theme_picker / file_browser)

Introduce the `Handled` protocol and move the four overlay interception bodies VERBATIM.
Inline `for o in ex.drain() {…}` sites stay inline in this task (the `fold_and_continue`
helper lands in T7). The reduce skeleton begins forming.

**Files:**
- Modify `app.rs` — ADD the `Handled` enum next to `Msg`; in `reduce`, REPLACE the four
  stage blocks (menu :383–451, palette :456–588, theme_picker :592–700, file_browser
  :704–798) with match-on-`intercept` skeleton lines; keep every OTHER stage inline for now.
- Modify `menu.rs`, `palette.rs`, `theme_picker.rs`, `file_browser.rs` — ADD an `intercept`
  fn each (bodies moved from the deleted app.rs blocks).

**Interfaces (Produces):**
```rust
// app.rs, alongside Msg
pub(crate) enum Handled { Done(bool), Pass(Msg) }

// menu.rs
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    reg: &crate::registry::Registry, keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled;
// palette.rs — identical signature to menu.rs (needs reg + keymap).
pub(crate) fn intercept(msg, editor, reg, keymap, ex, clock, msg_tx) -> crate::app::Handled;
// theme_picker.rs — no reg/keymap:
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled;
// file_browser.rs — same shape as theme_picker.rs (no reg/keymap).
pub(crate) fn intercept(msg, editor, ex, clock, msg_tx) -> crate::app::Handled;
```

**Steps:**

1. In app.rs, add the enum immediately after the `Msg` `Debug` impl (before
   `keep_overlay_visible`, ~app.rs:116):
   ```rust
   /// One interception stage's verdict. `Done(keep)` — the stage consumed the message and
   /// `reduce` returns `keep` (= `!editor.quit`). `Pass(msg)` — fall through; ownership of
   /// the message returns to the chain (by value, because the palette Paste arm and the
   /// prompt stage bind `msg` by value today).
   pub(crate) enum Handled { Done(bool), Pass(Msg) }
   ```
2. `menu.rs::intercept`: transpose the wrapper `if editor.menu.is_some() { … }` (app.rs:383)
   to an opening guard `if editor.menu.is_none() { return crate::app::Handled::Pass(msg); }`,
   then paste the body (:384–450) VERBATIM. Each consuming `return !editor.quit;` becomes
   `return crate::app::Handled::Done({ for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); } !editor.quit });` — i.e. keep the exact inline drain that preceded each return, wrapped in `Done(...)`. The trailing `// Non-key msg falls through` becomes `crate::app::Handled::Pass(msg)`. The two paste-drop early returns (:384–393) and the dispatch call to `dispatch_overlay_command` (:443, which stays `pub(crate)` in app.rs) are preserved verbatim.
3. `palette.rs::intercept`: same transposition of :456–587. Note the Paste arm binds
   `Msg::Input(Event::Paste(text)) = msg` by value (:463) — with `msg: Msg` owned by the
   fn, this compiles unchanged. Preserve the `dispatch_overlay_command` (:492) and
   buffer-switcher (:484–489) arms verbatim.
4. `theme_picker.rs::intercept`: transpose :592–699; the Paste arm (:601) and the motion/
   text arms call `crate::theme_cmds::preview_selected_theme(editor)` (repointed in T2) at
   the SAME points. `commit_theme_picker` (:620) is `crate::theme_cmds::commit_theme_picker`.
5. `file_browser.rs::intercept`: transpose :704–797; Enter calls
   `crate::file_browser::file_browser_enter(editor)` (the in-module fn from T4 — call it
   as a bare `file_browser_enter(editor)` since it now lives here).
6. In `reduce`, replace each of the four deleted blocks with:
   ```rust
   let msg = match crate::menu::intercept(msg, editor, reg, keymap, ex, clock, msg_tx) {
       crate::app::Handled::Done(k) => return k, crate::app::Handled::Pass(m) => m };
   let msg = match crate::palette::intercept(msg, editor, reg, keymap, ex, clock, msg_tx) {
       crate::app::Handled::Done(k) => return k, crate::app::Handled::Pass(m) => m };
   let msg = match crate::theme_picker::intercept(msg, editor, ex, clock, msg_tx) {
       crate::app::Handled::Done(k) => return k, crate::app::Handled::Pass(m) => m };
   let msg = match crate::file_browser::intercept(msg, editor, ex, clock, msg_tx) {
       crate::app::Handled::Done(k) => return k, crate::app::Handled::Pass(m) => m };
   ```
   placed in the SAME position and order the inline blocks occupied (after the pending_mark
   block, before the theme_picker→…→prompt inline blocks that remain for T6). Because
   pending_mark is still an inline `if` block at this point, keep it where it is; the four
   new skeleton lines sit exactly between the pending_mark block and the (still-inline)
   prompt block, matching today's order (§8.1-D).
7. **Run:** `cargo test -p wordcartel` — the starvation/paste-drop family (:3440–3487,
   :3620, :2875, :4001, :4017) and the T1 pins stay green WITHOUT edits. clippy + build clean.
8. **Commit:** `git commit -m "refactor(reduce): extract menu/palette/theme_picker/file_browser interception via Handled protocol"`.

---

## T6 — reduce stages, modal/input group (pending_mark / prompt / minibuffer / search / diag / outline)

Move the remaining six interception bodies VERBATIM behind `intercept` fns; the reduce
skeleton is now fully formed above the normal match.

**Files:**
- Modify `app.rs` — REPLACE the six inline blocks (pending_mark :365–378, prompt :804–849,
  minibuffer :854–900, search :906–963, diag :968–983, outline :985–1088) with skeleton
  match-lines, in the SAME order.
- Modify `marks.rs`, `prompts.rs`, `minibuffer.rs`, `search_ui.rs`, `diag_overlay.rs`,
  `outline_overlay.rs` — ADD an `intercept` fn each.

**Interfaces (Produces — uniform signature, none needs reg/keymap):**
```rust
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled;
```

**Steps:**

1. `marks.rs::intercept` (from :365–378): guard `if editor.pending_mark.is_none() { return
   Pass(msg); }`; the body drains-and-returns for ANY Key MESSAGE incl. non-Press (§8.1-L)
   — preserve the exact structure: the `if let Msg::Input(Event::Key(k)) = &msg` block ends
   with the inline drain + `Done(!editor.quit)`; a non-key message falls through to
   `Pass(msg)`.
2. `prompts.rs::intercept` (from :804–849): guard `if editor.prompt.is_none() { return
   Pass(msg); }`; the `match msg { … }` consumes EVERY message (Key + the five background
   result arms + `_ => {}`), then ALWAYS `for o in ex.drain() {…}` and `Done(!editor.quit)`
   — it never returns `Pass` once admitted (§8.1-J). Note it matches `msg` by value; with
   `msg: Msg` owned this is unchanged. The Key arm calls `crate::prompts::resolve_prompt`
   (already in this module — bare call).
3. `minibuffer.rs::intercept` (from :854–900): guard on `editor.minibuffer.is_none()`; Key
   input only → drain + `Done`; ALL non-key messages → `Pass(msg)` (§8.1-D starvation).
   The Enter arm's `match mb.kind { … }` calls `crate::prompts::…` submit fns verbatim.
4. `search_ui.rs::intercept` (from :906–963): guard on `editor.search.is_none()`. Preserve
   the Stepping-phase early block (:914–924) and its inline drain + `Done`. **The Esc arm
   (:926) and Alt+a arm (:929) return WITHOUT draining** — transcribe them as
   `KeyCode::Esc => { crate::search_ui::search_cancel(editor); return crate::app::Handled::Done(!editor.quit); }`
   and the Alt+a equivalent — do NOT add a drain (§8.1-C; G2 stays green). The Alt+Enter arm
   (:930–935) keeps its inline drain. Other key arms fall to the shared post-match
   `search_sync` + inline drain + `Done`. Non-key → `Pass(msg)`. (These are calls to sibling
   fns in the same module — use bare `search_cancel` / `search_sync` / `search_step` etc.)
5. `diag_overlay.rs::intercept` (from :968–983): guard on `editor.diag.is_none()`; Key only
   → `d.up()`/`d.down()`/close/`crate::search_ui::diag_apply_selected` + drain + `Done`;
   non-key → `Pass(msg)`.
6. `outline_overlay.rs::intercept` (from :985–1088): the buffer-mismatch pre-close
   (:985–988) is the FIRST statement, BEFORE the `is_none()` guard (§8.1-I):
   ```rust
   pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut Editor, ...) -> crate::app::Handled {
       if editor.outline.is_some()
           && editor.outline.as_ref().map(|o| o.buffer_id) != Some(editor.active().id) {
           editor.outline = None;
       }
       if editor.outline.is_none() { return crate::app::Handled::Pass(msg); }
       // … body :989–1087 verbatim …
   }
   ```
   Preserve the Enter arm's stale-version inner return (:1041–1046) as an inner
   `return crate::app::Handled::Done({ for o in ex.drain(){…} !editor.quit });`. The Enter
   jump calls `outline_jump_to(editor, target)` (the in-module fn from T4 — bare call). The
   Backspace/Char arms re-snapshot `(blocks, rope)` and call `o.set_query` verbatim.
7. In `reduce`, replace the six deleted blocks with the skeleton lines, in order:
   `marks::intercept` FIRST (before the T5 menu line), then after the T5 four-line block:
   `prompts::intercept`, `minibuffer::intercept`, `search_ui::intercept`,
   `diag_overlay::intercept`, `outline_overlay::intercept` — matching §8.1-D exactly. All
   six use the no-reg/keymap signature; the reduce skeleton now reads as ten
   `let msg = match …::intercept(...) { Done(k) => return k, Pass(m) => m };` lines followed
   by `let before = editor.active().document.version;` (:1090) and the normal match.
8. **Run:** `cargo test -p wordcartel` — G2 (`search_esc_does_not_drain_executor`) and all
   starvation tests green unmodified; clippy + build clean.
9. **Commit:** `git commit -m "refactor(reduce): extract pending_mark/prompt/minibuffer/search/diag/outline interception; reduce is now a stage skeleton"`.

---

## T7 — factor `fold_and_continue`

Replace the repeated inline `for o in ex.drain() {…} !editor.quit` in the ten stage
handlers with one helper. The two non-stage production drains — `dispatch_overlay_command`
(app.rs:170) and the main reduce epilogue (app.rs:1218) — are NOT rewritten (see below),
and the two search no-drain returns stay verbatim. (`menu_select_for_test` has no drain of
its own — it just calls `dispatch_overlay_command`, app.rs:184–186.)

**Files:**
- Modify `app.rs` — ADD `fold_and_continue`. Leave `dispatch_overlay_command`'s drain
  (:170) inline — it drains then HYDRATES (:172), so it is not a pure drain-then-continue.
  Leave the main epilogue drain (:1217–1221) inline — it runs AFTER the version-change hook
  and returns `!editor.quit`; keeping it inline avoids entangling the epilogue's hook
  ordering.
- Modify `marks.rs`, `menu.rs`, `palette.rs`, `theme_picker.rs`, `file_browser.rs`,
  `prompts.rs`, `minibuffer.rs`, `search_ui.rs`, `diag_overlay.rs`, `outline_overlay.rs` —
  rewrite each `Done({ for o in ex.drain() {…} !editor.quit })` to
  `Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx))`.

**Interfaces (Produces):**
```rust
pub(crate) fn fold_and_continue(editor: &mut crate::editor::Editor,
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> bool;
```

**Steps:**

1. Add to app.rs (near `hydrate_overlays`):
   ```rust
   /// The shared stage micro-epilogue: drain ready executor results, fold them into the
   /// editor, and report keep-running. Factored from the 21 verbatim repetitions. NOT used
   /// where a stage returns without draining (the search Esc/Alt+a arms — §8 invariant C).
   pub(crate) fn fold_and_continue(editor: &mut crate::editor::Editor, ex: &dyn crate::jobs::Executor,
       clock: &dyn wordcartel_core::history::Clock, msg_tx: &std::sync::mpsc::Sender<Msg>) -> bool {
       for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
       !editor.quit
   }
   ```
2. In each of the ten stage modules, replace every `Done({ for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); } !editor.quit })` with `Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx))`. Do NOT touch `search_ui`'s two no-drain `Done(!editor.quit)` returns (Esc :926, Alt+a :929) or its Alt+Enter/Stepping/normal drains — wait, those DO drain, so they convert too; the ONLY exclusions are the two returns that today have NO `for o in ex.drain()` before them. Verify by grepping each module for `Done(!editor.quit)` after the change: exactly two remain, both in search_ui.
3. Optionally simplify `dispatch_overlay_command`'s inline drain (:170) — it drains then
   hydrates, so it is NOT a pure `fold_and_continue` (hydrate follows); leave it verbatim.
   Leave the main reduce epilogue (:1217–1221) verbatim (it drains then returns after the
   version hook — the helper's drain-then-`!quit` matches, but keeping it inline avoids
   entangling the epilogue's version-hook ordering; do not change it).
4. **Run:** `cargo test -p wordcartel` — G1/G2 and starvation family green; clippy + build clean.
5. **Commit:** `git commit -m "refactor(reduce): factor the stage drain micro-epilogue into fold_and_continue"`.

---

## T8 — `timers.rs` hub (the anti-regrowth seam)

Move the save-timeout seam, add the eight gate-embedded deadline fns + the static table +
`next_wake`/`on_tick`/`pre_recv`, and rewire run()'s loop top + reduce's Tick arm.

**Files:**
- Create `wordcartel/src/timers.rs`.
- Modify `lib.rs` — add `pub mod timers;` (after `pub mod swap;`, line 23).
- Modify `app.rs` — DELETE `SAVE_QUIT_TIMEOUT_MS` (:1908) + `save_timeout_tick` (:1912–1941)
  from production and MOVE their tests (:5036–5095) to timers.rs; DELETE the inline deadline
  block (:1607–1668, keeping the `let now`/`recv_timeout`/`Msg::Tick`-map shape) and REPLACE
  with `pre_recv` + `next_wake`; REPLACE reduce's `Msg::Tick` arm body (:1175–1202) with a
  call to `timers::on_tick`.

**Interfaces (Produces):**
```rust
pub(crate) struct TimedSubsystem {
    pub(crate) name: &'static str,
    pub(crate) deadline: fn(&crate::editor::Editor, u64) -> Option<u64>,
}
pub(crate) static SUBSYSTEMS: &[TimedSubsystem];
pub(crate) fn next_wake(editor: &crate::editor::Editor, now: u64) -> Option<u64>;
pub(crate) fn on_tick(editor: &mut crate::editor::Editor, ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock, msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>);
pub(crate) fn pre_recv(editor: &mut crate::editor::Editor, now: u64);
pub(crate) const SAVE_QUIT_TIMEOUT_MS: u64 = 5_000;
pub(crate) fn save_timeout_tick(editor: &mut crate::editor::Editor, now: u64);
```

**Steps:**

1. Create timers.rs with provenance doc and imports:
   ```rust
   //! Timed-subsystem hub. Static fn-pointer table; each subsystem's `deadline` embeds its
   //! own in-flight/pending gate so a de-gated past-due Some can never reach recv_timeout(0)
   //! (the swap-thrash / A3-spin class). Extracted from app.rs run()/reduce (Effort H1 r2).
   //! Plugin-forward: the static slice upgrades to a `Vec<TimedSubsystem>` when Effort P needs
   //! dynamic (plugin) timer registration; builtins stay plain fn pointers.
   use crate::editor::Editor;
   use crate::jobs::Executor;
   use crate::registry::Ctx;
   use crate::app::Msg;
   use wordcartel_core::history::Clock;
   ```
2. Move `SAVE_QUIT_TIMEOUT_MS` and `save_timeout_tick` VERBATIM (the compiler-exhaustive
   `PostSaveAction` match at :1920–1938 is load-bearing — keep it). Change visibility to
   `pub(crate)`.
3. Add the eight deadline fns, each a verbatim transplant of its run()-loop term + gate:
   ```rust
   fn swap_deadline(e: &Editor, now: u64) -> Option<u64> {
       if crate::swap::pending(e.active().document.dirty(), e.active().document.version,
           e.active().swapped_version) && !e.active().swap_in_flight {
           crate::swap::next_deadline_ms(now, e.active().last_edit_at, e.active().last_swap_at)
       } else { None }
   }
   fn sq_deadline(e: &Editor, _now: u64) -> Option<u64> {
       e.pending_after_save.as_ref().map(|p| p.at_ms.saturating_add(SAVE_QUIT_TIMEOUT_MS))
   }
   fn sb_deadline(e: &Editor, now: u64) -> Option<u64> {
       if e.mouse.scrollbar_until_ms > now { Some(e.mouse.scrollbar_until_ms) } else { None }
   }
   fn menu_deadline(e: &Editor, _now: u64) -> Option<u64> {
       e.mouse.menu_reveal_due.or(e.mouse.menu_hide_due)
   }
   fn sb_dwell_deadline(e: &Editor, _now: u64) -> Option<u64> {
       e.mouse.scrollbar_reveal_due.or(e.mouse.scrollbar_hide_due)
   }
   fn status_dwell_deadline(e: &Editor, _now: u64) -> Option<u64> {
       e.mouse.status_reveal_due.or(e.mouse.status_hide_due)
   }
   fn diag_deadline(e: &Editor, _now: u64) -> Option<u64> {
       if e.active().diagnostics.in_flight_version.is_none() {
           e.active().diagnostics.recheck_due_at } else { None }
   }
   fn reconcile_deadline(e: &Editor, _now: u64) -> Option<u64> {
       if e.active().reconcile.in_flight_version.is_none() {
           e.active().reconcile.due_at } else { None }
   }
   ```
   Carry over, as fn doc comments, the load-bearing rationale from the deleted loop block:
   the swap idle-thrash note (app.rs:1609–1612) onto `swap_deadline`; the "at most one Some"
   note (:1628–1630) onto `menu_deadline`; the A3-spin exclusion note (:1635–1640) onto
   `diag_deadline`.
4. Add the table (order = today's fold order) and the three free fns:
   ```rust
   pub(crate) static SUBSYSTEMS: &[TimedSubsystem] = &[
       TimedSubsystem { name: "swap",         deadline: swap_deadline },
       TimedSubsystem { name: "save_quit",    deadline: sq_deadline },
       TimedSubsystem { name: "scrollbar",    deadline: sb_deadline },
       TimedSubsystem { name: "menu_dwell",   deadline: menu_deadline },
       TimedSubsystem { name: "sb_dwell",     deadline: sb_dwell_deadline },
       TimedSubsystem { name: "status_dwell", deadline: status_dwell_deadline },
       TimedSubsystem { name: "diagnostics",  deadline: diag_deadline },
       TimedSubsystem { name: "reconcile",    deadline: reconcile_deadline },
   ];
   pub(crate) fn next_wake(editor: &Editor, now: u64) -> Option<u64> {
       SUBSYSTEMS.iter().filter_map(|s| (s.deadline)(editor, now)).min()
   }
   pub(crate) fn pre_recv(editor: &mut Editor, now: u64) { save_timeout_tick(editor, now); }
   ```
   `on_tick` is the reduce Tick-arm body (app.rs:1175–1202) VERBATIM:
   ```rust
   pub(crate) fn on_tick(editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock,
       msg_tx: &std::sync::mpsc::Sender<Msg>) {
       let now = clock.now_ms();
       if crate::swap::pending(editor.active().document.dirty(), editor.active().document.version,
           editor.active().swapped_version)
           && !editor.active().swap_in_flight
           && crate::swap::due(now, editor.active().last_edit_at, editor.active().last_swap_at)
       {
           editor.active_mut().swap_in_flight = true;
           let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
           crate::swap::dispatch_swap_write(&mut ctx);
       }
       let version = editor.active().document.version;
       if editor.diag_cfg.enabled
           && crate::diagnostics_run::diag_due(&editor.active().diagnostics, now, version)
       {
           let ignore_words = std::sync::Arc::new(
               editor.dictionary.iter().chain(editor.session_ignores.iter()).cloned()
                   .collect::<std::collections::HashSet<String>>());
           let diag_cfg = editor.diag_cfg.clone();
           crate::diagnostics_run::dispatch_diagnostics(editor, &diag_cfg, ignore_words, msg_tx.clone());
       }
       if crate::reconcile::reconcile_due(&editor.active().reconcile, now) {
           crate::reconcile::dispatch_reconcile(editor, ex);
       }
   }
   ```
5. In app.rs `reduce`, replace the `Msg::Tick => { … }` arm (:1175–1202) with
   `Msg::Tick => crate::timers::on_tick(editor, ex, clock, msg_tx),`.
6. In app.rs `run`, rewrite the loop top: keep `let now = clock.now_ms();` (:1607), replace
   `save_timeout_tick(&mut editor, now);` (:1608) with `crate::timers::pre_recv(&mut editor,
   now);`, DELETE the eight-term block + `next_deadline` fold (:1609–1660), and set
   ```rust
   let timeout = crate::timers::next_wake(&editor, now)
       .map(|d| std::time::Duration::from_millis(d.saturating_sub(now)))
       .unwrap_or(std::time::Duration::from_secs(3600));
   ```
   The `recv_timeout(timeout)` match (:1664–1668) and everything below (InputThreadDied
   intercept, epilogue) are UNCHANGED.
7. Move the C4 save-timeout tests (app.rs:5036–5095) into timers.rs, repointing
   `crate::app::save_timeout_tick` / `crate::app::SAVE_QUIT_TIMEOUT_MS` →
   `crate::timers::…`.
8. Add two new timers guardrails in timers.rs's test module:
   ```rust
   /// Idle-is-free: a clean, settled, no-overlay editor arms no wake (§8.1-E). This is the
   /// timers-native form of app.rs's settled_editor_arms_no_deadline pin.
   #[test]
   fn next_wake_none_when_settled() {
       let e = crate::editor::Editor::new_from_text("hello\n", None, (80, 24));
       assert!(!e.active().document.dirty());
       assert_eq!(crate::timers::next_wake(&e, 10_000), None);
   }
   /// Each named subsystem's in-flight/pending gate yields None when gated — generalizes
   /// diag_deadline_excluded_when_in_flight across the whole table (§8.1-E). CRITICAL: each
   /// subsystem is ARMED so its deadline would be Some WITHOUT its gate — the test must FAIL
   /// if a gate is deleted, not pass vacuously.
   #[test]
   fn gated_subsystems_yield_none() {
       let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
       // diagnostics: past-due recheck ARMED, but in-flight → excluded. (Without the gate,
       // diag_deadline would be Some(0).)
       e.active_mut().diagnostics.recheck_due_at = Some(0);
       e.active_mut().diagnostics.in_flight_version = Some(1);
       // reconcile: past-due ARMED, but in-flight → excluded. (Without the gate, Some(0).)
       e.active_mut().reconcile.due_at = Some(0);
       e.active_mut().reconcile.in_flight_version = Some(1);
       // swap: make the buffer DIRTY (version 1 != saved_version Some(0)) with swapped_version
       // None so swap::pending == true (swap.rs:79), and last_edit_at Some so next_deadline_ms
       // (swap.rs:63) would return Some WITHOUT the gate — then a write in flight is the ONLY
       // reason swap yields None. new_from_text seeds saved_version Some(0)/version 0 (a clean
       // buffer), so without this arming swap::pending is already false and the gate is a no-op.
       e.active_mut().document.version = 1;          // dirty: 1 != saved_version Some(0)
       e.active_mut().last_edit_at = Some(0);         // arm next_deadline_ms
       // swapped_version stays None → pending true.
       assert!(crate::swap::pending(e.active().document.dirty(), e.active().document.version,
           e.active().swapped_version), "precondition: swap work is pending (else the gate is vacuous)");
       assert!(crate::swap::next_deadline_ms(10_000, e.active().last_edit_at, e.active().last_swap_at).is_some(),
           "precondition: WITHOUT the !swap_in_flight gate the swap deadline would be Some");
       e.active_mut().swap_in_flight = true;          // the gate under test
       for s in crate::timers::SUBSYSTEMS {
           if matches!(s.name, "diagnostics" | "reconcile") {
               assert_eq!((s.deadline)(&e, 10_000), None, "{} must be None while in-flight", s.name);
           }
       }
       assert_eq!((crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "swap").unwrap().deadline)(&e, 10_000),
           None, "swap must be None while a write is in flight (§8.1-E — the !swap_in_flight gate)");
   }
   ```
   Keep `diag_deadline_excluded_when_in_flight` (app.rs:4249), `idle_buffer_does_not_thrash_the_swap_file`
   (:2037), and `continuous_editing_checkpoints_but_stays_bounded` (:2071) UNMODIFIED in
   app.rs — they still pass (they drive `reduce`/`swap` directly, not the deleted loop block).
9. **Run:** `cargo test -p wordcartel next_wake_none_when_settled gated_subsystems_yield_none`
   (2 passed), then `cargo test -p wordcartel` full (green incl. the three unmodified swap/diag
   guardrails + T1's `settled_editor_arms_no_deadline`), clippy, build — clean.
10. **Commit:** `git commit -m "refactor(run): extract the timed-subsystem hub to timers.rs (static fn-pointer table)"`.

---

## T9 — deeper cut: `Input(Key)` arm → `input::handle_key`

**Files:**
- Modify `wordcartel/src/input.rs` — ADD `handle_key` (body from app.rs:1092–1136).
- Modify `app.rs` — REPLACE the `Msg::Input(Event::Key(k)) if k.kind == …Press => { … }`
  arm body with a call.

**Interfaces (Produces):**
```rust
pub(crate) fn handle_key(k: crossterm::event::KeyEvent, editor: &mut crate::editor::Editor,
    reg: &crate::registry::Registry, keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>);
```

**Steps:**

1. In input.rs add `handle_key`, pasting the arm body (:1098–1135) VERBATIM incl. the
   Esc-precedence comment (:1093–1097). It references `crate::keymap`, `crate::commands`,
   `crate::registry::Ctx`, and `crate::app::hydrate_overlays` (which stays `pub(crate)` in
   app.rs). The dispatch block builds `Ctx { editor, clock, executor: ex, msg_tx:
   msg_tx.clone() }` then `reg.dispatch(id, &mut ctx); crate::app::hydrate_overlays(editor,
   reg, keymap);` exactly as today.
2. In reduce's normal `match msg`, replace the arm with:
   ```rust
   Msg::Input(Event::Key(k)) if k.kind == crossterm::event::KeyEventKind::Press =>
       crate::input::handle_key(k, editor, reg, keymap, ex, clock, msg_tx),
   ```
   Note `handle_key` runs INSIDE the normal match, so the version-capture `before` (:1090)
   and epilogue hook (:1209) still see key-driven edits — the §8.1-A asymmetry is undisturbed
   (G1 stays green).
3. **Run:** `cargo test -p wordcartel` (green; the many normal-key tests exercise this path),
   clippy, build — clean.
4. **Commit:** `git commit -m "refactor(reduce): extract the normal-mode key arm to input::handle_key"`.

---

## T10 — deeper cut: overlay list-nav unification → `list_window`

Unify the four identical six-key motion blocks (palette/theme_picker/file_browser/outline)
behind `apply_list_nav`, keeping per-overlay side effects OUTSIDE the helper.

**Files:**
- Modify `wordcartel/src/list_window.rs` — ADD `ListNav`, `list_nav_key`, `apply_list_nav`.
- Modify `palette.rs`, `theme_picker.rs`, `file_browser.rs`, `outline_overlay.rs` — route
  the six motion keys in each `intercept` through the helper.

**Interfaces (Produces):**
```rust
pub(crate) enum ListNav { Up, Down, PageUp, PageDown, Home, End }
pub(crate) fn list_nav_key(code: crossterm::event::KeyCode) -> Option<ListNav>;
/// Apply a motion to (selected, scroll_top) over `row_count` rows in an `area_h`-tall
/// buffer area — the exact math of the four duplicated blocks.
pub(crate) fn apply_list_nav(nav: ListNav, area_h: u16, row_count: usize,
    selected: &mut usize, scroll_top: &mut usize);
```

**Steps:**

1. Add to list_window.rs:
   ```rust
   pub(crate) enum ListNav { Up, Down, PageUp, PageDown, Home, End }
   pub(crate) fn list_nav_key(code: crossterm::event::KeyCode) -> Option<ListNav> {
       use crossterm::event::KeyCode;
       match code {
           KeyCode::Up => Some(ListNav::Up), KeyCode::Down => Some(ListNav::Down),
           KeyCode::PageUp => Some(ListNav::PageUp), KeyCode::PageDown => Some(ListNav::PageDown),
           KeyCode::Home => Some(ListNav::Home), KeyCode::End => Some(ListNav::End),
           _ => None,
       }
   }
   pub(crate) fn apply_list_nav(nav: ListNav, area_h: u16, row_count: usize,
       selected: &mut usize, scroll_top: &mut usize) {
       let lh = list_h_for(row_count, area_h);
       match nav {
           ListNav::Up => *selected = selected.saturating_sub(1),
           ListNav::Down => *selected = (*selected + 1).min(row_count.saturating_sub(1)),
           ListNav::PageDown => *selected = (*selected + lh.max(1)).min(row_count.saturating_sub(1)),
           ListNav::PageUp => *selected = selected.saturating_sub(lh.max(1)),
           ListNav::Home => *selected = 0,
           ListNav::End => *selected = row_count.saturating_sub(1),
       }
       keep_visible(*selected, row_count, lh, scroll_top);
   }
   ```
   This reproduces exactly today's per-key math: Up/Down are `saturating_sub(1)` /
   `min(len-1)`; Page steps use `list_h_for` (== the inline `lh`) then `min`/`saturating_sub`;
   Home/End are `0` / `len-1`; every arm ends with `keep_visible` (the `keep_overlay_visible`
   inline call, which is `list_h_for` + `keep_visible` — app.rs:121–124). **Verify against
   the source:** the inline `keep_overlay_visible(ah, sel, rows, &mut scroll)` uses
   `list_h_for(row_count, area_h)` for BOTH the page step and the visibility clamp, so the
   single `lh` here is correct for both.
2. `palette.rs::intercept` motion arms (was app.rs:496–540): replace the six arms with
   ```rust
   c if crate::list_window::list_nav_key(c).is_some() => {
       let ah = editor.active().view.area.1;
       if let Some(p) = editor.palette.as_mut() {
           crate::list_window::apply_list_nav(crate::list_window::list_nav_key(c).unwrap(),
               ah, p.rows.len(), &mut p.selected, &mut p.scroll_top);
       }
   }
   ```
   placed BEFORE the Enter/Backspace/Left/Right/Char arms (motion keys are distinct codes,
   so ordering among arms is safe). The Enter, Backspace, Left, Right, Char (text-edit /
   dispatch) arms stay VERBATIM — they are not motion. (The guard `c if …is_some()` binds
   `k.code`; keep the outer `match k.code` and add this arm.)
3. `theme_picker.rs::intercept` motion arms (was :621–671): same routing over
   `tp.rows.len()` / `tp.selected` / `tp.scroll_top`, then — CRITICAL (§8.1-H) — call
   `crate::theme_cmds::preview_selected_theme(editor)` AFTER the borrow drops, exactly as
   the six original arms each did:
   ```rust
   c if crate::list_window::list_nav_key(c).is_some() => {
       let ah = editor.active().view.area.1;
       if let Some(tp) = editor.theme_picker.as_mut() {
           crate::list_window::apply_list_nav(crate::list_window::list_nav_key(c).unwrap(),
               ah, tp.rows.len(), &mut tp.selected, &mut tp.scroll_top);
       }
       crate::theme_cmds::preview_selected_theme(editor);
   }
   ```
   The Esc/Enter/Backspace/Char arms stay verbatim (Char/Backspace still call
   `rebuild_rows` + preview; Enter commits).
4. `file_browser.rs::intercept` motion arms (was :727–771): same routing over
   `fb.entries.len()` / `fb.selected` / `fb.scroll_top`; NO preview/re-query side effect
   (file browser motion had none). Enter/Backspace/Char stay verbatim.
5. `outline_overlay.rs::intercept` motion arms (was :995–1039): same routing over
   `o.rows.len()` / `o.selected` / `o.scroll_top`; NO re-query on motion (§8.1-H — G5 pins
   this). The Enter (jump/stale-close), Backspace, Char arms stay VERBATIM (they re-snapshot
   `(blocks, rope)` + `set_query`).
6. **Run:** `cargo test -p wordcartel` — G4 (`theme_picker_preview_pin_visible_row` :4921)
   and G5 (`outline_motion_does_not_requery`) green; the palette/file-browser windowing
   tests green; clippy + build clean.
7. **Commit:** `git commit -m "refactor(overlays): unify the four overlay list-nav blocks via list_window::apply_list_nav"`.

---

## T11 — render leaf `chrome_geom.rs` (geometry + hit-testing, verbatim move)

**Files:**
- Create `wordcartel/src/chrome_geom.rs`.
- Modify `lib.rs` — add `pub mod chrome_geom;` (after `pub mod render_overlays;`, line 11).
- Modify `render.rs` — DELETE the 13 geometry fns (`menu_bar_layout_cats` :140,
  `menu_bar_layout` :153, `menu_dropdown_rect` :158, `menu_dropdown_row_at` :172,
  `menu_area` :197, `windowed_indicator` :203, `palette_overlay_rect` :216, `palette_row_at`
  :230, `theme_picker_row_at` :245, `file_browser_row_at` :257, `outline_row_at` :269,
  `diag_row_at` :282, `prompt_choice_at` :306–340); MOVE their pure-geometry tests; repoint
  render.rs's own remaining call sites (render() at :159/:173 etc. are inside the moved fns;
  render_overlays is a separate module — see below).
- Modify `render_overlays.rs` — split the `use crate::render::{…}` import (:15–18):
  `ChromeStyles` stays from `crate::render`; `menu_bar_layout`, `menu_bar_layout_cats`,
  `menu_dropdown_rect`, `palette_overlay_rect`, `windowed_indicator` come from
  `crate::chrome_geom`. Repoint the body call at :295 (`crate::render::menu_area`) →
  `crate::chrome_geom::menu_area`.
- Modify `mouse.rs` — repoint all `crate::render::` geometry refs to `crate::chrome_geom::`:
  production `:112, :118, :156, :168, :172, :178, :213, :217, :256, :260, :296, :300, :349,
  :353, :380, :498`; tests `:772, :805, :912, :1205, :1285, :1324, :1398, :1574`.

**Interfaces (Produces — all `pub(crate)`, signatures verbatim from render.rs):**
`menu_bar_layout_cats`, `menu_bar_layout`, `menu_dropdown_rect`, `menu_dropdown_row_at`,
`menu_area`, `windowed_indicator`, `palette_overlay_rect`, `palette_row_at`,
`theme_picker_row_at`, `file_browser_row_at`, `outline_row_at`, `diag_row_at`,
`prompt_choice_at` (exact signatures at render.rs:140/153/158/172/197/203/216/230/245/257/269/282/306).

**Steps:**

1. Create chrome_geom.rs with provenance doc + imports the fns need:
   ```rust
   use ratatui::layout::Rect;
   ```
   (the bodies also reference `crate::registry`, `crate::menu`, `crate::list_window`,
   `crate::palette`, `crate::theme_picker`, `crate::file_browser`, `crate::outline_overlay`,
   `crate::diag_overlay`, `crate::prompt` — all fully qualified in the fn bodies, so no
   extra `use` beyond `Rect`). Cut the 13 fns VERBATIM, including the twinning-invariant doc
   comments (`menu_area` :192–196, `menu_dropdown_row_at` :176–180) — they document the
   paint/hit-test parity that must not drift.
2. In render.rs, delete the 13 fns. `render()`'s body does NOT call any of them directly
   (it delegates overlay/menu painting to `render_overlays::paint`), so render.rs's own
   production code needs no repoint — verify by grepping render.rs:475–996 for the 13 names
   (expect none).

   **Test disposition — COMPLETE, source-verified list.** These are ALL render.rs test-mod
   fns that reference any of the 13 moved fns (no other moved-fn has a render.rs test;
   `*_row_at`/`prompt_choice_at` are tested in mouse.rs, which only repoints). The rule:
   a test that invokes `render` / `render_to_buffer` (the helper at render.rs:1066, which
   calls `term.draw(super::render)`) STAYS in render.rs and only repoints the moved-fn name
   to `crate::chrome_geom::`; a pure geometry/hit-test test (zero render invocation) MOVES.

   **MOVE (verified render-free — each body opened and confirmed):**
   - `palette_overlay_rect_sizes_to_row_count` (fn at render.rs:1309) — Rect + two
     `palette_overlay_rect` asserts, no helper, no render. Self-contained.
   - `menu_dropdown_windows_a_tall_category` (:3265) — `menu_dropdown_rect` on
     `tall_menu_groups(20)`, no render.
   - `dropdown_indicator_row_hit_test_returns_none` (:3395) — `menu_dropdown_rect` +
     `menu_dropdown_row_at` on `tall_menu_groups(20)`, no render.
   - **Also move the test-only helper `tall_menu_groups` (render.rs:3253)** — it is used
     ONLY by the two menu movers above (grep: refs at :3267 and :3399, nowhere else), so it
     moves with them into chrome_geom.rs's `#[cfg(test)] mod tests`; no staying test loses it.

   **STAY + repoint (verified render-driving — each calls `render_to_buffer`/`render`):**
   - `palette_windowed_slice_shows_scrolled_rows` (:1334, `render_to_buffer` :1339).
   - `palette_indicator_only_when_scrollable` (:1357, `render_to_buffer` :1362/:1386).
   - `outline_windowed_slice_and_indicator` (:2434, `render_to_buffer` :2445/:2470).
   - `theme_picker_windowed_slice_and_indicator` (:2482, `render_to_buffer` :2501/:2521).
   - `file_browser_windowed_slice_and_indicator` (:2533, `render_to_buffer` :2551/:2572).
   - `tokyo_overlay_interior_is_themed` (:2970, `render_to_buffer` :2979).
   - `dropdown_fills_whole_rect_with_muted_panel_bg` (:3210, `render` :3239).
   - `dropdown_indicator_row_carries_panel_bg` (:3280, `render` :3309).
   - `dropdown_highlight_never_hidden_in_overflow` (:3327, `render` :3356).
3. Repoint render_overlays.rs (import split + :295) and mouse.rs (all 16 production + 8 test
   sites listed above).
4. **Run:** `cargo test -p wordcartel`, clippy, build — clean. Golden render tests unchanged.
5. **Commit:** `git commit -m "refactor(render): extract shared geometry + hit-testing to chrome_geom.rs"`.

---

## T12 — render leaf `render_status.rs` (status builders, verbatim move)

**Files:**
- Create `wordcartel/src/render_status.rs`.
- Modify `lib.rs` — add `pub mod render_status;` (after `pub mod render;`, line 10, or
  adjacent to chrome_geom).
- Modify `render.rs` — MOVE `status_left_text` (:349–371), `word_count_segment` (:378–393),
  `format_search_bar` (:1002–1029, changing visibility `fn` → `pub(crate) fn`); repoint
  `render()`'s calls: `format_search_bar(s)` :901, `status_left_text(editor)` :918,
  `word_count_segment(editor)` :935 → `crate::render_status::…`; MOVE their tests
  (`word_count_segment_selection_aware` :1523–1532, the `status_left_text` cases
  :3044–3055). **`fold_marker_for` (:1037–1043) STAYS in render.rs** (row-loop helper used
  at :648 — spec §7.2 correction).

**Interfaces (Produces — `pub(crate)`):**
```rust
pub(crate) fn status_left_text(editor: &crate::editor::Editor) -> String;
pub(crate) fn word_count_segment(editor: &crate::editor::Editor) -> Option<String>;
pub(crate) fn format_search_bar(s: &crate::search_overlay::SearchState) -> String;
```

**Steps:**

1. Create render_status.rs with provenance doc + imports: `use crate::editor::Editor;` and
   `use wordcartel_core::count;` (used by `word_count_segment` :388–392); `format_search_bar`
   references `crate::search_overlay::Phase`, `wordcartel_core::search`, `crate::limits`
   (fully qualified — no extra `use`). Cut the three fns VERBATIM; make `format_search_bar`
   `pub(crate)`.
2. In render.rs repoint the three call sites (:901, :918, :935) to `crate::render_status::…`;
   move the two test blocks, repointing `crate::render::status_left_text` /
   `crate::render::word_count_segment` → `crate::render_status::…`. Leave the
   `fold_marker_for` test (:1518–1519) and `fold_marker_for` itself in render.rs.
3. **Run:** `cargo test -p wordcartel`, clippy, build — clean.
4. **Commit:** `git commit -m "refactor(render): extract status-line text builders to render_status.rs"`.

---

## Final gates (after T12)

1. `cargo test --workspace` — full green.
2. `cargo clippy --workspace --all-targets` — clean.
3. `cargo build --workspace` + `cargo test --workspace --no-run` — warning-free.
4. `scripts/smoke/run.sh` — run; quote the one-line summary verbatim in the pre-merge
   report (advisory-pass; a red result is surfaced, not a blocker).
5. Then the standard pipeline's two gates: Fable whole-branch review (§8 invariant
   checklist, compiling probes) + Codex pre-merge GO/NO-GO. Merge `--no-ff` only after both
   pass; delete the branch; push only when asked.

---

## Self-review

- **Spec-coverage:** every spec §10 task maps to exactly one plan task — T1↔§10.1 (G1/G2/
  G3=settled/G5 pins; G4 = keep-green), T2↔theme_cmds, T3↔chrome, T4↔micro-leaves, T5↔reduce
  overlay stages, T6↔reduce modal/input stages, T7↔fold_and_continue, T8↔timers, T9↔input
  key arm, T10↔list-nav unification, T11↔chrome_geom, T12↔render_status. (Spec §10 T1's G3 is
  named `settled_editor_arms_no_deadline` here; the timers-native re-expression is T8's
  `next_wake_none_when_settled`.)
- **Placeholder scan:** no "similar to Task N", "add appropriate X", or TODO — every code
  and test step shows complete content or an exact verbatim-move source range with the
  precise repoint list.
- **Type/signature consistency:** `Handled { Done(bool), Pass(Msg) }` (by value) is produced
  in T5 and consumed by every `intercept` (T5/T6) and the reduce skeleton; the overlay
  `intercept` signatures split into two shapes (menu/palette take `reg, keymap`; the other
  eight do not) — matched at every call site in the skeleton. `fold_and_continue` (T7) and
  `TimedSubsystem`/`SUBSYSTEMS`/`next_wake`/`on_tick`/`pre_recv` (T8) and
  `ListNav`/`list_nav_key`/`apply_list_nav` (T10) and the chrome_geom/render_status fn
  signatures (T11/T12) are stated once in Interfaces and used consistently. Migration-site
  lists carry the Codex-r2 completeness corrections (theme_cmds app.rs:5409/5482;
  persist_session app.rs:4478/4500; chrome_geom mouse.rs:1285/1324/1398/1574;
  file_browser_enter — NO session_restore.rs:151 repoint).

## History

- 2026-07-09 — Initial plan (Fable), grounded on `745698d`; spec APPROVED + Codex-clean.
- 2026-07-09 — Round-3: folded Codex plan-gate fixes — (T8) armed the `gated_subsystems_yield_none`
  swap case so `swap::pending == true` and the deadline would be Some without the gate (was a
  vacuous no-op on a clean fresh buffer — verified against swap.rs:79 + editor.rs saved_version
  seed); (T11) re-scoped the render test-split so only pure hit-test/windowing tests move and
  paint tests (`render`/`render_to_buffer`) stay + repoint; (T2) narrowed the keymap-test move to
  the three rebuild-helper tests, leaving `save_settings_command_sets_the_request_flag` in app.rs;
  (T7) corrected the "two glue sites" wording — the two non-stage drains are
  `dispatch_overlay_command` + the main epilogue; `menu_select_for_test` has no own drain.
- 2026-07-09 — Round-4: reclassified the three `*_windowed_slice_and_indicator` tests
  (render.rs:2434/2482/2533) from MOVE → STAY+repoint — each calls `render_to_buffer` (I had
  them backwards in round-3). Did a COMPLETE re-verification of the entire T11 test-move list:
  opened every render.rs test referencing a moved geometry fn and classified by presence of a
  `render`/`render_to_buffer` call. Final move list = exactly three pure tests
  (`palette_overlay_rect_sizes_to_row_count`, `menu_dropdown_windows_a_tall_category`,
  `dropdown_indicator_row_hit_test_returns_none`) + the `tall_menu_groups` helper (used only by
  the two menu movers); all nine render-driving tests STAY + repoint.
