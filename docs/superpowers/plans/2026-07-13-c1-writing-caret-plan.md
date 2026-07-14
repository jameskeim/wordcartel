# C1 — The Writing Caret (B8 + B11): implementation plan

**Effort:** C1 · **Backlog:** absorbs **B8**, closes **B11** · **Date:** 2026-07-13 · **Author:** Fable
**Spec (source of truth, Codex-clean):** `docs/superpowers/specs/2026-07-13-c1-writing-caret-design.md`
**Process:** subagent-driven-development (fresh implementer PER TASK: failing test → impl → green →
commit; then a per-task reviewer). Right-sized so a reviewer can reject one task while approving its
neighbor.

---

## Goal

The writing caret stays the **hardware terminal cursor**. Make its **shape** (`default/block/beam/
underline`) and **blink** (`on/off`) user-settable via two orthogonal, contract-conformant options;
emit **DECSCUSR** through a new edge-triggered, latch-guarded reconcile (zero writes at rest);
preview live in a **cursor picker** cloned from the theme picker; **restore** the caret on clean exit
and panic only if we ever wrote; and fix **B11** so every modal/overlay query field shows its own
caret and none is parked in the text area beneath a modal. No `wordcartel-core` change; no layout /
hot-path change.

## Architecture

Functional-core / imperative-shell. All C1 work is in the **`wordcartel` shell**. The option state
lives on `Editor` (two direct fields, like `scrollbar_mode`), mutated only through two shared setters
(Law 6). A pure derivation `desired_caret_style(&Editor) -> Option<(CaretShape, bool)>` composes shape
× blink (returning OUR `Copy + PartialEq` pair, since crossterm's `SetCursorStyle` is not `PartialEq`);
a `reconcile_cursor_style<W: Write>` seam (sibling of `chrome::reconcile_mouse_capture`) maps it via
`to_set_cursor_style` and emits the escape edge-triggered against a `run`-local `Option<(CaretShape,
bool)>` latch, called at BOTH the pre-first-draw and in-loop sites `reconcile_mouse_capture` uses. A
process-global `AtomicBool` latch
(reachable from the `'static` panic hook) gates caret restore. The picker is a fixed-list overlay
that previews by setting the options live (the reconcile morphs a dedicated sample-cell caret) and is
registered as one new interceptor stage in `app::reduce_dispatch`.

## Tech stack

Rust; ratatui 0.30 + crossterm 0.28 (`crossterm::cursor::SetCursorStyle` = DECSCUSR 0–6); the
project's registry / settings-diff / theme-picker / list-window machinery. Tests: in-crate
`#[cfg(test)]` units, `Vec<u8>` backends for escape writes, `TestBackend::cursor_position` for caret
placement, `e2e.rs` in-process journeys, `module_budgets` + LAW-2/palette-completeness invariants.

---

## Global constraints (binding — copied from CLAUDE.md / the spec)

- **House style, hand-formatted. Do NOT run `cargo fmt`** (no `rustfmt.toml`; it would reflow the
  tree). Match neighbors by hand: snake_case fns/vars/modules, PascalCase types, SCREAMING_SNAKE
  consts; 4-space indent; ~100-col hand-wrapped; imports grouped by hand; `—` em-dash in prose
  comments never `--`; no emoji in code. Do not reflow code you did not change.
- **`#![forbid(unsafe_code)]`** governs `wordcartel-core` — **N/A here** (no core changes). No `unsafe`
  anywhere in C1 regardless.
- **Workspace clippy `all = "deny"` is a merge GATE.** `cargo clippy --workspace --all-targets` must
  be clean. A deliberate exception needs an item-local `#[allow(clippy::…)]` + one-line rationale.
- **`clippy::too_many_lines` (threshold 100) is a GATE.** Keep new fns under 100 lines or carry an
  item-local `#[allow(clippy::too_many_lines)]` with a reason (only for a genuinely flat, cohesive
  dispatch — none expected in C1).
- **`module_budgets` is a GATE:** `app.rs` ≤ 1000, `render.rs` ≤ 900 production lines
  (`wordcartel/tests/module_budgets.rs`). C1 adds ~3 lines to `app.rs` and a small guard + no net
  logic to `render.rs`; **re-check after Tasks 3 and 6** (`cargo test -p wordcartel --test
  module_budgets`).
- **Idle-free / edge-triggered law.** Background/terminal work is edge-triggered by a real state
  change, never level-triggered off wall-clock. The reconcile MUST write nothing at rest (both call
  sites edge-triggered against the shared latch). Pinned by the Task-3 no-write-at-rest guardrail.
- **Command-surface-contract conformance is a merge GATE** (see the conformance section below).
  The invariant tests — palette-completeness, `every_persisted_setting_has_a_command`, hint
  re-resolution — must stay green.
- **PTY smoke** (`scripts/smoke/run.sh`) is mandatory-run / advisory-pass; the pre-merge report
  quotes its one-line summary. S7 (panic → restore) is the advisory real-terminal eyeball for the
  caret restore (DECSCUSR bytes bypass `TestBackend`).
- **rust-analyzer lags edits.** For any compile/usage/signature question on code you are editing,
  trust `cargo build`/`check`/`clippy`/`test` + `grep`, NOT an editor "unused/undefined" hint.
- **Every commit ends with the project trailers, verbatim:**
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG
  ```
- **Exact new command ids / labels (LAW — do not rename):**
  | id | label | menu | handler |
  |---|---|---|---|
  | `caret_shape_default` | `Caret Shape: Default` | None | `set_caret_shape(CaretShape::Default)` |
  | `caret_shape_block` | `Caret Shape: Block` | None | `set_caret_shape(CaretShape::Block)` |
  | `caret_shape_beam` | `Caret Shape: Beam` | None | `set_caret_shape(CaretShape::Beam)` |
  | `caret_shape_underline` | `Caret Shape: Underline` | None | `set_caret_shape(CaretShape::Underline)` |
  | `cycle_caret_shape` | `Caret Shape` | View | cycle Default→Block→Beam→Underline→Default |
  | `caret_blink_on` | `Caret Blink: On` | None | `set_caret_blink(true)` |
  | `caret_blink_off` | `Caret Blink: Off` | None | `set_caret_blink(false)` |
  | `toggle_caret_blink` | `Caret Blink` | View | flip via `set_caret_blink` |
  | `cursor` | `Caret\u{2026}` (`Caret…`) | View | `open_cursor_picker()` |

---

## File structure

**New modules:**
- `wordcartel/src/cursor_style.rs` — `CaretShape` consumers: `desired_caret_style(&Editor) ->
  Option<(CaretShape, bool)>`, `to_set_cursor_style(CaretShape, bool) -> SetCursorStyle`,
  `reconcile_cursor_style<W: Write>(&Editor, &mut W, &mut Option<(CaretShape, bool)>)`, and `mod
  restore { mark_written / ever_wrote / restore_caret_if_written }`
  (the process-global `AtomicBool`). One responsibility: terminal caret appearance + its restore latch.
- `wordcartel/src/cursor_picker.rs` — `CursorPicker` struct, the `ROW_ACTIONS` table, `intercept`
  stage, preview/commit funnel. Cloned in shape from `theme_picker.rs`.

**Existing files touched:**
- `config.rs` — `CaretShape` enum + `caret_shape_str`; `ViewConfig.caret_shape/caret_blink` +
  defaults; `RawView.caret_shape/caret_blink`; the override-apply arms (string→enum, bool).
- `editor.rs` — `Editor.caret_shape: CaretShape`, `caret_blink: bool`; `set_caret_shape` /
  `set_caret_blink` setters; `cursor_picker: Option<CursorPicker>` field; `open_cursor_picker()`;
  `has_active_input_overlay(&self) -> bool` census predicate (Task 6). Add `cursor_picker = None` to
  the other overlays' XOR-clear blocks and the `menu` command's clear block.
- `registry.rs` — the 9 command rows (data-table growth).
- `settings.rs` — `SettingsSnapshot.view_caret_shape/view_caret_blink`; `OView.caret_shape/
  caret_blink`; `default_snapshot`/`runtime_snapshot` mappings; the two diff-law entries + `any_view`
  OR + `OView` literal; the LAW-2 destructure + assertion arms.
- `render.rs` — `place_cursor` arm-3 hide-guard (gate on `has_active_input_overlay`).
- `render_overlays.rs` — the four query-overlay caret placements + the cursor-picker render arm & its
  sample-cell caret placement.
- `mouse.rs` — add `cursor_picker` to `no_overlay_open` (so mouse routes to the picker) + a
  cursor-picker branch in `route_overlay` (wheel/click, mirror theme-picker).
- `term.rs` — `restore_caret_if_written` into the three managed restore sites.
- `app.rs` — `mod cursor_picker; mod cursor_style;` (lib.rs actually — see note); the `run`-local
  latch + the two `reconcile_cursor_style` calls; startup `set_caret_shape/blink` apply; the new
  `cursor_picker::intercept` stage in `reduce_dispatch`.
- `lib.rs` — `mod cursor_style;` + `mod cursor_picker;` declarations.
- `e2e.rs` (test module) — a small `Harness::cursor_pos(&self) -> (u16, u16)` test-infra helper
  reading the inherent `TestBackend::cursor_position()` (render.rs:849 precedent) + the cursor-picker
  journey (Task 7).

> **Note on module declaration:** modules are declared in `lib.rs` (grep `^mod ` / `^pub mod ` in
> `lib.rs` to place them alphabetically among the neighbors). Do NOT declare them in `app.rs`.

---

## Command-surface-contract conformance (merge GATE)

This effort touches commands, user-settable options, the palette, and the menu — full conformance:

- **LAW 1 (registry SSOT).** Caret state mutates only via `set_caret_shape`/`set_caret_blink`,
  reached only through registered commands and the picker (which calls the same setters).
- **LAW 2 (every option is a command).** Two new persisted fields map to commands; the
  `every_persisted_setting_has_a_command` exhaustive destructure gains two fields + two assertion
  arms (Task 4). Adding the `SettingsSnapshot` fields makes the destructure fail to compile until the
  arms exist — the intended recurrence trip.
- **LAW 3 (palette exhaustive).** All 9 commands are non-hidden → appear in the palette
  automatically; palette-completeness enforces. The picker is not the only door — the six
  set-per-state primitives guarantee palette reachability of every value.
- **LAW 4 (menu ⊆ palette).** Menu subset = `cycle_caret_shape`, `toggle_caret_blink`, `cursor`
  (View). All in the palette. The six set-per-state primitives are `menu: None`.
- **LAW 6 (one setter; profiles too).** `set_caret_shape`/`set_caret_blink` are the sole mutators —
  commands, picker preview/commit, and startup config apply all call them. No bypass.
- **RULE 8 (multi-state).** `caret_shape` (4-state) = set-per-state primitives + a `cycle`
  representative with `MenuMark::Value`; `caret_blink` (2-state) = set-per-state primitives + a
  `toggle` representative with `MenuMark::OnOff`. Mirrors scrollbar / status_line verbatim.
- **LAW 7 (hints track keymap).** C1 ships **no default chord** → hint re-resolution is inherited for
  free; the `hints_reresolve_on_preset_switch` invariant is unaffected. Any user keymap patch
  re-resolves normally.
- **RULE 10 (plugin/automation spine).** All commands nullary; set-value semantics kept clean so
  Effort P can later collapse the four `caret_shape_*` into one parameterized command.

---

## Task list (each = one implementer subagent, TDD, commit at end)

Order chosen so interfaces flow forward: types (T1) → runtime state+setters (T2) → the write seam
(T3) → commands+persistence (T4) → restore (T5) → B11 (T6) → picker (T7). T3 depends on T1+T2; T4 on
T1+T2; T5 on T3; T6 on T2 (the census predicate) and is otherwise independent; T7 on T1–T4+T6.

---

### Task 1 — Option types + config + ViewConfig + override parse

**Deliverable:** `CaretShape` enum, `caret_shape_str`, `ViewConfig` fields + defaults, `RawView`
fields, and the override-apply arms — the pure config surface, no runtime wiring yet.

**Interfaces — Produces:**
- `config::CaretShape` = `enum { Default, Block, Beam, Underline }` (`Debug, Clone, Copy, PartialEq,
  Eq`), `impl Default -> Default`.
- `config::caret_shape_str(CaretShape) -> &'static str` → `"default"/"block"/"beam"/"underline"`.
- `config::ViewConfig.caret_shape: CaretShape` (default `Default`), `.caret_blink: bool` (default
  `true`).
- `config::caret_shape_from_str(&str) -> Option<CaretShape>` (used by override parse + Task 4 diff /
  Task 7; add it now).

**Consumes:** nothing (leaf task).

**TDD steps.**

1. **Failing test** — add to `config.rs`'s `#[cfg(test)] mod tests`:
   ```rust
   #[test]
   fn caret_shape_str_roundtrips() {
       for s in [CaretShape::Default, CaretShape::Block, CaretShape::Beam, CaretShape::Underline] {
           assert_eq!(caret_shape_from_str(caret_shape_str(s)), Some(s));
       }
       assert_eq!(caret_shape_from_str("bogus"), None);
   }
   #[test]
   fn viewconfig_defaults_caret_default_blink_on() {
       let v = ViewConfig::default();
       assert_eq!(v.caret_shape, CaretShape::Default);
       assert!(v.caret_blink, "blink default on (inert under Default until a shape is chosen)");
   }
   ```
   `cargo test -p wordcartel caret_shape_str_roundtrips` → fails to compile (types absent).

2. **Impl** — beside `TransientMode` / `transient_mode_str` in `config.rs`:
   ```rust
   /// Writing-caret shape. `Default` = never emit DECSCUSR (terminal's own shape) — the shipped
   /// default. The three concrete shapes map to DECSCUSR when composed with blink (see cursor_style).
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum CaretShape { Default, Block, Beam, Underline }
   impl Default for CaretShape { fn default() -> Self { CaretShape::Default } }

   pub fn caret_shape_str(s: CaretShape) -> &'static str {
       match s { CaretShape::Default => "default", CaretShape::Block => "block",
                 CaretShape::Beam => "beam", CaretShape::Underline => "underline" }
   }
   pub fn caret_shape_from_str(s: &str) -> Option<CaretShape> {
       match s { "default" => Some(CaretShape::Default), "block" => Some(CaretShape::Block),
                 "beam" => Some(CaretShape::Beam), "underline" => Some(CaretShape::Underline),
                 _ => None }
   }
   ```
   Add to `struct ViewConfig` (after `splash`): `pub caret_shape: CaretShape,` and `pub caret_blink:
   bool,`. In `impl Default for ViewConfig`, append to the literal: `caret_shape:
   CaretShape::Default, caret_blink: true`.
   Add to `struct RawView` (after `splash`): `caret_shape: Option<String>,` and `caret_blink:
   Option<bool>,`.
   In the override-apply block (mirror the `raw.view.scrollbar` arm), add:
   ```rust
   if let Some(s) = raw.view.caret_shape {
       match crate::config::caret_shape_from_str(&s) {
           Some(cs) => cfg.view.caret_shape = cs,
           None => warns.push(format!("view.caret_shape \"{s}\" invalid; using default")),
       }
   }
   if let Some(b) = raw.view.caret_blink { cfg.view.caret_blink = b; }
   ```
   (Place these next to the existing `scrollbar`/`status_line` override arms in the same fn.)

3. **Green:** `cargo test -p wordcartel caret_shape` and `cargo test -p wordcartel viewconfig_defaults`
   → pass. `cargo build -p wordcartel` clean. `cargo clippy -p wordcartel --all-targets` clean.

4. **Commit:** `C1 T1: CaretShape option type + ViewConfig/RawView fields + override parse` (+ trailers).

**Note to implementer:** if rust-analyzer flags the new enum "unused," ignore it — it is used by
later tasks; verify with `cargo build`, not the editor hint.

---

### Task 2 — Editor runtime fields + shared setters + startup apply

**Deliverable:** the two runtime fields on `Editor`, their shared setters (Law 6), and the startup
config→editor apply in `app::run`.

**Interfaces — Produces:**
- `Editor.caret_shape: CaretShape`, `Editor.caret_blink: bool` (direct fields, like `scrollbar_mode`).
- `Editor::set_caret_shape(&mut self, s: CaretShape)` and `Editor::set_caret_blink(&mut self, on:
  bool)` — the ONLY mutators.

**Consumes:** `config::CaretShape` (T1).

**TDD steps.**

1. **Failing test** — in `editor.rs` tests:
   ```rust
   #[test]
   fn caret_setters_are_the_single_mutators() {
       let mut e = Editor::new_from_text("x\n", None, (40, 12));
       assert_eq!(e.caret_shape, crate::config::CaretShape::Default);
       assert!(e.caret_blink);
       e.set_caret_shape(crate::config::CaretShape::Beam);
       e.set_caret_blink(false);
       assert_eq!(e.caret_shape, crate::config::CaretShape::Beam);
       assert!(!e.caret_blink);
   }
   ```
   `cargo test -p wordcartel caret_setters_are_the_single_mutators` → fails to compile.

2. **Impl** — add the fields to `struct Editor` near `scrollbar_mode` (grep `pub scrollbar_mode` for
   the cluster). Initialize them in EVERY `Editor` constructor path that builds the struct literal
   (grep `scrollbar_mode:` in `editor.rs` to find each — mirror exactly, defaulting `caret_shape:
   crate::config::CaretShape::Default, caret_blink: true`). Add the setters near `set_scrollbar_mode`:
   ```rust
   /// Set the writing-caret shape. The single setter the `caret_shape_*` / `cycle_caret_shape`
   /// commands, the cursor picker, and startup config apply all call (contract law 6).
   pub fn set_caret_shape(&mut self, s: crate::config::CaretShape) { self.caret_shape = s; }
   /// Set caret blink. Inert while `caret_shape == Default` (emits nothing — see cursor_style).
   pub fn set_caret_blink(&mut self, on: bool) { self.caret_blink = on; }
   ```

3. **Startup apply** — in `app::run`, where `set_scrollbar_mode(cfg.view.scrollbar)` is called
   (grep `set_scrollbar_mode(cfg.view.scrollbar)` in `app.rs`), add adjacent:
   ```rust
   editor.set_caret_shape(cfg.view.caret_shape);
   editor.set_caret_blink(cfg.view.caret_blink);
   ```

4. **Green:** `cargo test -p wordcartel caret_setters_are_the_single_mutators` → pass. `cargo build
   -p wordcartel` clean; clippy clean.

5. **Commit:** `C1 T2: Editor caret_shape/caret_blink fields + setters + startup apply` (+ trailers).

---

### Task 3 — `cursor_style.rs`: derivation + reconcile + restore latch + the two `app::run` call sites

**Deliverable:** the new module with the DECSCUSR composition, the edge-triggered reconcile, the
process-global restore latch, and both `reconcile_cursor_style` call sites wired into `app::run`
sharing one `run`-local latch. Vec<u8> unit tests + the no-write-at-rest guardrail.

**Interfaces — Produces:**
- `cursor_style::desired_caret_style(editor: &Editor) -> Option<(CaretShape, bool)>` — returns OUR own
  `Copy + PartialEq` pair (crossterm's `SetCursorStyle` is NOT `PartialEq` — spec C-11), `None` for
  `Default`.
- `cursor_style::to_set_cursor_style(shape: CaretShape, blink: bool) ->
  crossterm::cursor::SetCursorStyle` — total mapper, invoked only at the `execute!` call.
- `cursor_style::reconcile_cursor_style<W: std::io::Write>(editor: &Editor, backend: &mut W, applied:
  &mut Option<(CaretShape, bool)>)`.
- `cursor_style::restore::{ mark_written(), ever_wrote() -> bool }` — the process-global
  `AtomicBool` latch. (`restore_caret_if_written` is Produced by **Task 5**, which owns its tests.)

**Consumes:** `Editor.caret_shape/caret_blink` (T2), `config::CaretShape` (T1).

**TDD steps.**

1. **Failing tests** — create `cursor_style.rs` with a `#[cfg(test)] mod tests` FIRST (module skeleton
   + the tests, no impl bodies), and add `mod cursor_style;` to `lib.rs`. Because the impl symbols are
   absent, the pre-impl run is a **compile error / unresolved symbol** (not an assertion failure) —
   that IS the red state. Tests (Vec<u8> backend — DECSCUSR bytes bypass `TestBackend`, so mechanism
   coverage is byte-buffer based; the latch holds OUR `(CaretShape, bool)` pair, since crossterm's
   `SetCursorStyle` is not `PartialEq` — spec C-11):
   ```rust
   use super::*;
   use crate::editor::Editor;
   use crate::config::CaretShape;
   use crossterm::cursor::SetCursorStyle;

   fn ed_with(shape: CaretShape, blink: bool) -> Editor {
       let mut e = Editor::new_from_text("x\n", None, (40, 12));
       e.set_caret_shape(shape); e.set_caret_blink(blink); e
   }

   #[test]
   fn desired_composition_table() {
       assert_eq!(desired_caret_style(&ed_with(CaretShape::Default, true)),  None);
       assert_eq!(desired_caret_style(&ed_with(CaretShape::Default, false)), None);
       assert_eq!(desired_caret_style(&ed_with(CaretShape::Block, true)),     Some((CaretShape::Block, true)));
       assert_eq!(desired_caret_style(&ed_with(CaretShape::Block, false)),    Some((CaretShape::Block, false)));
       assert_eq!(desired_caret_style(&ed_with(CaretShape::Beam, true)),      Some((CaretShape::Beam, true)));
       assert_eq!(desired_caret_style(&ed_with(CaretShape::Beam, false)),     Some((CaretShape::Beam, false)));
       assert_eq!(desired_caret_style(&ed_with(CaretShape::Underline, true)), Some((CaretShape::Underline, true)));
       assert_eq!(desired_caret_style(&ed_with(CaretShape::Underline, false)),Some((CaretShape::Underline, false)));
   }

   #[test]
   fn to_set_cursor_style_maps_all_concrete_combos() {
       assert!(matches!(to_set_cursor_style(CaretShape::Block, true),     SetCursorStyle::BlinkingBlock));
       assert!(matches!(to_set_cursor_style(CaretShape::Block, false),    SetCursorStyle::SteadyBlock));
       assert!(matches!(to_set_cursor_style(CaretShape::Beam, true),      SetCursorStyle::BlinkingBar));
       assert!(matches!(to_set_cursor_style(CaretShape::Beam, false),     SetCursorStyle::SteadyBar));
       assert!(matches!(to_set_cursor_style(CaretShape::Underline, true), SetCursorStyle::BlinkingUnderScore));
       assert!(matches!(to_set_cursor_style(CaretShape::Underline, false),SetCursorStyle::SteadyUnderScore));
   }

   #[test]
   fn default_shape_writes_nothing() {
       let e = ed_with(CaretShape::Default, true);
       let mut buf: Vec<u8> = Vec::new();
       let mut applied: Option<(CaretShape, bool)> = None;
       reconcile_cursor_style(&e, &mut buf, &mut applied);
       assert!(buf.is_empty(), "Default shape must emit no DECSCUSR");
       assert!(applied.is_none());
   }

   #[test]
   fn concrete_shape_writes_once_then_rests() {
       let e = ed_with(CaretShape::Beam, true);
       let mut buf: Vec<u8> = Vec::new();
       let mut applied: Option<(CaretShape, bool)> = None;
       reconcile_cursor_style(&e, &mut buf, &mut applied);
       assert!(!buf.is_empty(), "first reconcile writes the style");
       assert_eq!(applied, Some((CaretShape::Beam, true)));
       let n = buf.len();
       reconcile_cursor_style(&e, &mut buf, &mut applied); // idle-free guardrail
       assert_eq!(buf.len(), n, "second reconcile at rest writes nothing");
   }

   #[test]
   fn runtime_back_to_default_unmanages_once() {
       let mut e = ed_with(CaretShape::Beam, true);
       let mut buf: Vec<u8> = Vec::new();
       let mut applied: Option<(CaretShape, bool)> = None;
       reconcile_cursor_style(&e, &mut buf, &mut applied);
       e.set_caret_shape(CaretShape::Default);
       let n = buf.len();
       reconcile_cursor_style(&e, &mut buf, &mut applied);
       assert!(buf.len() > n, "→Default emits one DefaultUserShape");
       assert!(applied.is_none());
       let m = buf.len();
       reconcile_cursor_style(&e, &mut buf, &mut applied);
       assert_eq!(buf.len(), m, "then rests");
   }

   #[test]
   fn restore_latch_is_monotonic() {
       // restore::EVER_WROTE is a process-global static shared across all tests in the binary,
       // so assert only the monotonic (false→true) transition, never that it is false at start.
       restore::mark_written();
       assert!(restore::ever_wrote(), "mark_written latches ever_wrote true");
   }
   ```
   **Run (pre-impl):** `cargo test -p wordcartel --lib cursor_style` → **expected: compile error /
   unresolved symbol** (`desired_caret_style`/`reconcile_cursor_style`/`to_set_cursor_style`/`restore`
   absent). After impl: **compiles + assertions pass.**

   > Note: `restore_caret_if_written` and its Vec<u8> emit/no-emit tests are owned by **Task 5** (its
   > red→green cycle). Task 3 produces only `mark_written`/`ever_wrote` (the reconcile calls
   > `mark_written`).

2. **Impl** — `cursor_style.rs` body:
   ```rust
   //! Writing-caret appearance: DECSCUSR shape/blink composition, the edge-triggered
   //! reconcile that emits it (zero writes at rest), and the process-global restore latch
   //! the panic hook consults. Sibling of `chrome::reconcile_mouse_capture`.

   use crossterm::cursor::SetCursorStyle;
   use crate::config::CaretShape;
   use crate::editor::Editor;

   /// The caret style the caret SHOULD currently have, as OUR own `Copy + PartialEq` pair
   /// `(shape, blink)`. Global today (reads only the two options); the seam exists so a per-context
   /// map could slot in later WITHOUT rewiring the reconcile — NOT a user-facing feature. `None` ⇒
   /// Default shape ⇒ emit nothing (blink inert). crossterm's `SetCursorStyle` is not `PartialEq`
   /// (C-11), so the latch/comparison uses this pair, not the crossterm type.
   pub fn desired_caret_style(editor: &Editor) -> Option<(CaretShape, bool)> {
       match editor.caret_shape {
           CaretShape::Default => None,
           _ => Some((editor.caret_shape, editor.caret_blink)),
       }
   }

   /// Map our (shape, blink) pair to the crossterm DECSCUSR command — the ONLY place the crossterm
   /// type is produced, at the `execute!` call. Total; `Default` maps to `DefaultUserShape` (never
   /// reached via `desired_caret_style`, which returns `None` for Default — kept total so there is
   /// no unreachable arm).
   pub fn to_set_cursor_style(shape: CaretShape, blink: bool) -> SetCursorStyle {
       match (shape, blink) {
           (CaretShape::Default, _)       => SetCursorStyle::DefaultUserShape,
           (CaretShape::Block, true)      => SetCursorStyle::BlinkingBlock,
           (CaretShape::Block, false)     => SetCursorStyle::SteadyBlock,
           (CaretShape::Beam, true)       => SetCursorStyle::BlinkingBar,
           (CaretShape::Beam, false)      => SetCursorStyle::SteadyBar,
           (CaretShape::Underline, true)  => SetCursorStyle::BlinkingUnderScore,
           (CaretShape::Underline, false) => SetCursorStyle::SteadyUnderScore,
       }
   }

   /// Edge-triggered: emit a DECSCUSR escape ONLY when the desired style differs from what was last
   /// applied. Never writes at rest. Latches success into `applied` (OUR pair) and (on the first real
   /// write) into the process-global restore flag. Best-effort: a failed write leaves the latch so it
   /// is retried next change — never spun on.
   pub fn reconcile_cursor_style<W: std::io::Write>(
       editor: &Editor, backend: &mut W, applied: &mut Option<(CaretShape, bool)>,
   ) {
       match desired_caret_style(editor) {
           Some(style) if *applied != Some(style) => {
               let cs = to_set_cursor_style(style.0, style.1);
               if crossterm::execute!(backend, cs).is_ok() {
                   *applied = Some(style);
                   restore::mark_written();
               }
           }
           None if applied.is_some() => {
               if crossterm::execute!(backend, SetCursorStyle::DefaultUserShape).is_ok() {
                   *applied = None;
               }
           }
           _ => {} // desired == applied, or both "unmanaged": zero writes at rest.
       }
   }

   /// Process-global "did we ever write a concrete DECSCUSR style?" latch. It must be reachable
   /// from the `'static` panic hook (which has no `&Editor`), so it is a module static, not an
   /// Editor field. (`restore_caret_if_written` is added in Task 5.)
   pub mod restore {
       use std::sync::atomic::{AtomicBool, Ordering};
       static EVER_WROTE: AtomicBool = AtomicBool::new(false);
       /// Called by the reconcile each time it successfully writes a concrete style.
       pub fn mark_written() { EVER_WROTE.store(true, Ordering::Relaxed); }
       /// True iff the reconcile ever emitted a DECSCUSR style this process.
       pub fn ever_wrote() -> bool { EVER_WROTE.load(Ordering::Relaxed) }
   }
   ```
   `Relaxed` is sufficient: monotone one-way latch; writer is the main loop; readers are the same
   thread (Drop/rollback) or the main-thread-gated panic hook.

3. **Wire the two `app::run` call sites** — sharing ONE `run`-local latch, mirroring `applied_mouse`:
   - Declare the latch near `let mut applied_mouse = editor.mouse_capture;` (grep it in `app.rs`):
     ```rust
     let mut applied_caret: Option<(crate::config::CaretShape, bool)> = None;
     ```
   - **Pre-first-draw:** immediately after the standalone `reconcile_mouse_capture(&mut editor,
     guard.terminal().backend_mut(), &mut applied_mouse);` line (grep that exact call; it sits just
     before the first `guard.terminal().draw(...)`), add:
     ```rust
     crate::cursor_style::reconcile_cursor_style(&editor, guard.terminal().backend_mut(), &mut applied_caret);
     ```
   - **In-loop:** immediately after the in-loop `reconcile_mouse_capture(&mut e, ...)` call inside the
     `{ let mut e = editor.borrow_mut(); ... }` block, add (same block, reuse `e` and the backend):
     ```rust
     crate::cursor_style::reconcile_cursor_style(&e, guard.terminal().backend_mut(), &mut applied_caret);
     ```
     > **Borrow note:** the in-loop `reconcile_mouse_capture` block takes `&mut e =
     > editor.borrow_mut()` then `guard.terminal().backend_mut()`. `reconcile_cursor_style` needs
     > only `&e` (shared), so add it inside the SAME block after the mouse call — no new borrow
     > scope. Verify with `cargo build`, not an editor borrow hint.

4. **Green:** `cargo test -p wordcartel --lib cursor_style` → pass. `cargo build -p wordcartel` clean.
   `cargo test -p wordcartel --test module_budgets` → app.rs still ≤ 1000 (adds ~3 lines). clippy clean.

5. **Commit:** `C1 T3: cursor_style reconcile + restore latch + two app::run call sites` (+ trailers).

**Limitation recorded:** these are Vec<u8> mechanism tests; no test asserts a terminal *honored*
DECSCUSR (spec decision #6, no detection). The two `run` call sites are integration-covered by the
T7 e2e journey / smoke S7 (advisory), since `run` owns the real terminal.

---

### Task 4 — Commands + registry rows + persistence + LAW-2

**Deliverable:** the 9 registry command rows, the full `SettingsSnapshot`/`OView`/diff-law wiring, the
LAW-2 destructure + assertion extension, and command + round-trip tests.

**Interfaces — Consumes:** `set_caret_shape`/`set_caret_blink` (T2), `CaretShape`/`caret_shape_str`/
`caret_shape_from_str` (T1). **Produces:** the 9 commands (ids fixed in Global Constraints); the two
new persisted snapshot fields.

**TDD steps.**

1. **Failing tests** — in `registry.rs` tests (mirror `scrollbar_commands_set_and_cycle`):
   ```rust
   #[test]
   fn caret_shape_commands_set_and_cycle() {
       use crate::config::CaretShape;
       let mut ed = Editor::new_from_text("x\n", None, (80, 24));
       dispatch_id(&mut ed, "caret_shape_block");     assert_eq!(ed.caret_shape, CaretShape::Block);
       dispatch_id(&mut ed, "caret_shape_beam");      assert_eq!(ed.caret_shape, CaretShape::Beam);
       dispatch_id(&mut ed, "caret_shape_underline"); assert_eq!(ed.caret_shape, CaretShape::Underline);
       dispatch_id(&mut ed, "caret_shape_default");   assert_eq!(ed.caret_shape, CaretShape::Default);
       // cycle Default→Block→Beam→Underline→Default
       dispatch_id(&mut ed, "cycle_caret_shape"); assert_eq!(ed.caret_shape, CaretShape::Block);
       dispatch_id(&mut ed, "cycle_caret_shape"); assert_eq!(ed.caret_shape, CaretShape::Beam);
       dispatch_id(&mut ed, "cycle_caret_shape"); assert_eq!(ed.caret_shape, CaretShape::Underline);
       dispatch_id(&mut ed, "cycle_caret_shape"); assert_eq!(ed.caret_shape, CaretShape::Default);
       let reg = Registry::builtins();
       assert_eq!(reg.meta(CommandId("caret_shape_block")).unwrap().menu, None);
       assert_eq!(reg.meta(CommandId("cycle_caret_shape")).unwrap().menu, Some(MenuCategory::View));
   }
   #[test]
   fn caret_blink_commands_set_and_toggle() {
       let mut ed = Editor::new_from_text("x\n", None, (80, 24));
       dispatch_id(&mut ed, "caret_blink_off"); assert!(!ed.caret_blink);
       dispatch_id(&mut ed, "caret_blink_on");  assert!(ed.caret_blink);
       dispatch_id(&mut ed, "toggle_caret_blink"); assert!(!ed.caret_blink);
       dispatch_id(&mut ed, "toggle_caret_blink"); assert!(ed.caret_blink);
   }
   ```
   And extend `every_persisted_setting_has_a_command` (settings.rs) — add to the destructure and add
   the two assert arms:
   ```rust
   // in field_guard's SettingsSnapshot { ... } destructure, add:
   view_caret_shape: _, view_caret_blink: _,
   // in the assertions:
   assert!(has("cycle_caret_shape") && has("caret_shape_block"), "view_caret_shape");
   assert!(has("toggle_caret_blink") && has("caret_blink_on"), "view_caret_blink");
   ```
   Plus a diff-law round-trip test — call the REAL `settings::compute_overrides(runtime, baseline,
   existing: &OverridesFile, mask: &OverridesFile) -> OverridesFile` (verified at settings.rs; there
   is NO `build_overrides` and NO `Masked` type), mirroring `scrollbar_status_line_round_trip_via_diff_law`
   exactly (which uses `snap_with` + `OverridesFile::default()`):
   ```rust
   #[test]
   fn caret_options_round_trip_via_diff_law() {
       use crate::config::CaretShape;
       let base = snap_with(|s| { s.view_caret_shape = CaretShape::Default; s.view_caret_blink = true; });
       let mut rt = base.clone();
       rt.view_caret_shape = CaretShape::Beam;
       rt.view_caret_blink = false;
       let of = compute_overrides(&rt, &base, &OverridesFile::default(), &OverridesFile::default());
       let v = of.view.expect("view section present");
       assert_eq!(v.caret_shape.as_deref(), Some("beam"));
       assert_eq!(v.caret_blink, Some(false));
       // No divergence → no caret keys written.
       let of2 = compute_overrides(&base, &base, &OverridesFile::default(), &OverridesFile::default());
       assert!(of2.view.is_none() || of2.view.as_ref().unwrap().caret_shape.is_none());
   }
   ```
   > `compute_overrides`, `snap_with`, and `OverridesFile` are the real symbols in `settings.rs`
   > tests (verified). `OView` fields (`caret_shape: Option<String>`, `caret_blink: Option<bool>`) are
   > added in step 3. If any helper name drifts, mirror the live
   > `scrollbar_status_line_round_trip_via_diff_law` test's exact call shape.
   **Run (pre-impl):** `cargo test -p wordcartel caret_` and `-p wordcartel
   every_persisted_setting_has_a_command` → **compile error** (the destructure gains fields with no
   snapshot definition yet, and the commands/OView fields are absent). After impl: **pass.**

2. **Impl — registry rows.** In `Registry::builtins`, beside the scrollbar/status_line block:
   ```rust
   use crate::config::CaretShape;
   // Caret shape: set-per-state (palette-only) + 4-state cycle representative (View, state-in-label).
   r.register("caret_shape_default",   "Caret Shape: Default",   None, |c| { c.editor.set_caret_shape(CaretShape::Default);   CommandResult::Handled });
   r.register("caret_shape_block",     "Caret Shape: Block",     None, |c| { c.editor.set_caret_shape(CaretShape::Block);     CommandResult::Handled });
   r.register("caret_shape_beam",      "Caret Shape: Beam",      None, |c| { c.editor.set_caret_shape(CaretShape::Beam);      CommandResult::Handled });
   r.register("caret_shape_underline", "Caret Shape: Underline", None, |c| { c.editor.set_caret_shape(CaretShape::Underline); CommandResult::Handled });
   r.register_stateful("cycle_caret_shape", "Caret Shape", Some(MenuCategory::View),
       |e| MenuMark::Value(match e.caret_shape {
           CaretShape::Default => "Default", CaretShape::Block => "Block",
           CaretShape::Beam => "Beam", CaretShape::Underline => "Underline" }),
       |c| { let next = match c.editor.caret_shape {
                 CaretShape::Default => CaretShape::Block, CaretShape::Block => CaretShape::Beam,
                 CaretShape::Beam => CaretShape::Underline, CaretShape::Underline => CaretShape::Default };
             c.editor.set_caret_shape(next); CommandResult::Handled });
   // Caret blink: set-per-state (palette-only) + 2-state toggle representative (View, OnOff mark).
   r.register("caret_blink_on",  "Caret Blink: On",  None, |c| { c.editor.set_caret_blink(true);  CommandResult::Handled });
   r.register("caret_blink_off", "Caret Blink: Off", None, |c| { c.editor.set_caret_blink(false); CommandResult::Handled });
   r.register_stateful("toggle_caret_blink", "Caret Blink", Some(MenuCategory::View),
       |e| MenuMark::OnOff(e.caret_blink),
       |c| { let n = !c.editor.caret_blink; c.editor.set_caret_blink(n); CommandResult::Handled });
   ```
   The `cursor` picker-open command is registered in **Task 7** (needs `open_cursor_picker`).

3. **Impl — persistence** (`settings.rs`):
   - `struct SettingsSnapshot`: add `pub view_caret_shape: crate::config::CaretShape,` and `pub
     view_caret_blink: bool,` (place after `view_splash` to match the diff/destructure order).
   - `default_snapshot` (from config): `view_caret_shape: cfg.view.caret_shape, view_caret_blink:
     cfg.view.caret_blink,`.
   - `runtime_snapshot` (from editor): `view_caret_shape: editor.caret_shape, view_caret_blink:
     editor.caret_blink,`.
   - `struct OView`: add `#[serde(skip_serializing_if = "Option::is_none")] pub caret_shape:
     Option<String>,` and `... pub caret_blink: Option<bool>,`.
   - Diff-law (in the `compute_overrides` view section — the real fn name; mirror the `scrollbar` +
     `splash` entries):
     ```rust
     let rt_cs   = crate::config::caret_shape_str(runtime.view_caret_shape).to_string();
     let base_cs = crate::config::caret_shape_str(baseline.view_caret_shape).to_string();
     let caret_shape = diff_key(&rt_cs, &base_cs,
         ex_view.and_then(|v| v.caret_shape.as_ref()),
         mk_view.and_then(|v| v.caret_shape.as_ref()).is_some());
     let caret_blink = diff_key(&runtime.view_caret_blink, &baseline.view_caret_blink,
         ex_view.and_then(|v| v.caret_blink.as_ref()),
         mk_view.and_then(|v| v.caret_blink).is_some());
     ```
     Extend the `any_view` OR with `|| caret_shape.is_some() || caret_blink.is_some()` and add
     `caret_shape, caret_blink` to the `OView { ... }` literal.
   - Add `caret_shape`/`caret_blink` fields to `RawView` mapping if the overrides→config load path
     destructures `OView`/`RawView` exhaustively (grep for a `RawView {` or `OView {` destructure in
     the load path; T1 already added the `RawView` fields, so just ensure the override-apply reads
     them — already done in T1).
   - **Mask/round-trip guards:** grep `view_scrollbar: _` in `settings.rs` tests for any OTHER
     exhaustive destructure of `SettingsSnapshot` (besides `field_guard`) and add the two fields
     there too, or the test target won't compile.

4. **Green:** `cargo test -p wordcartel caret_shape_commands_set_and_cycle
   caret_blink_commands_set_and_toggle caret_options_round_trip_via_diff_law
   every_persisted_setting_has_a_command` → pass. Palette-completeness test (grep its name, e.g.
   `palette_lists_every_command` / the test formalized from `palette.rs`) → still green (new commands
   auto-listed). `cargo build`/`clippy -p wordcartel` clean.

5. **Commit:** `C1 T4: caret shape/blink commands + persistence + LAW-2 extension` (+ trailers).

---

### Task 5 — `restore_caret_if_written` + its tests + `term.rs` restore-site edits

**Deliverable:** the `restore_caret_if_written` writer (owned here, with its own red→green Vec<u8>
test cycle) and its wiring into the three managed `term.rs` restore sites; the fourth
(`EnterAlternateScreen`-failure) path deliberately untouched.

**Interfaces — Produces:** `cursor_style::restore::restore_caret_if_written<W: std::io::Write>(backend:
&mut W)`. **Consumes:** `cursor_style::restore::{mark_written, ever_wrote}` (T3).

**TDD steps.**

1. **Failing test (red→green cycle for the writer)** — add to `cursor_style`'s `#[cfg(test)] mod
   tests` a Vec<u8> test of `restore_caret_if_written` (the writer this task adds). The restore SITES
   themselves write to `io::stdout()` and cannot be unit-tested directly, so the Vec<u8> writer test
   + smoke S7 (advisory) is the coverage:
   ```rust
   #[test]
   fn restore_caret_if_written_gated_by_latch() {
       // Process-global latch: assert only the two directions we can force deterministically.
       // never-written direction is only assertable if nothing else in the binary latched it;
       // guard on ever_wrote() so the test is order-independent.
       let mut buf: Vec<u8> = Vec::new();
       if !restore::ever_wrote() {
           restore::restore_caret_if_written(&mut buf);
           assert!(buf.is_empty(), "never wrote → restore emits nothing");
       }
       restore::mark_written();
       assert!(restore::ever_wrote());
       let mut buf2: Vec<u8> = Vec::new();
       restore::restore_caret_if_written(&mut buf2);
       assert!(!buf2.is_empty(), "after mark_written → restore emits DefaultUserShape");
   }
   ```
   **Run (pre-impl):** `cargo test -p wordcartel --lib restore_caret_if_written_gated_by_latch` →
   **compile error / unresolved symbol** (`restore_caret_if_written` absent). After impl: **pass.**

2. **Impl — the writer** — add to `cursor_style::restore` (the `pub mod restore` from T3):
   ```rust
   use crossterm::cursor::SetCursorStyle;
   /// Emit DefaultUserShape iff we ever wrote — used by the three managed term.rs restore sites.
   pub fn restore_caret_if_written<W: std::io::Write>(backend: &mut W) {
       if ever_wrote() { let _ = crossterm::execute!(backend, SetCursorStyle::DefaultUserShape); }
   }
   ```
   (Add the `use crossterm::cursor::SetCursorStyle;` inside `mod restore` if not already present.)

3. **Impl — the three sites** — in each managed site, add the restore call adjacent to the existing
   `execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen, Show)`.
   Grep `LeaveAlternateScreen, Show` in `term.rs` — three matches at `TerminalGuard::Drop`, the
   `Terminal::new` failure arm (inside `TerminalGuard::new`), and the panic hook body. In each, insert
   BEFORE the existing `execute!` (so the caret is restored within the same teardown):
   ```rust
   crate::cursor_style::restore::restore_caret_if_written(&mut io::stdout());
   ```
   Do NOT touch the `EnterAlternateScreen`-failure arm (the one that does only `let _ =
   disable_raw_mode(); return Err(e)`) — the latch is provably false there (reconcile has not run),
   so a call would be a guaranteed no-op; leave it minimal.
   In the panic hook, place the restore call after `crate::recovery::dump_on_panic();` and before the
   existing `disable_raw_mode()`/`execute!` restore (order within teardown is immaterial; keep it
   grouped with the restore).

4. **Green:** `cargo test -p wordcartel --lib restore_caret_if_written_gated_by_latch` → pass. `cargo
   build -p wordcartel` clean; the rest of `--lib cursor_style` still green; clippy clean. Run
   `scripts/smoke/run.sh` and record the S7 line (advisory — the real-terminal restore eyeball).

5. **Commit:** `C1 T5: restore_caret_if_written + latch-aware caret restore at three term.rs sites`
   (+ trailers).

---

### Task 6 — B11: arm-3 hide-guard + four overlay query-caret placements + census predicate

**Deliverable:** `place_cursor` no longer parks the caret under a modal; the four query overlays place
their own carets; a single `Editor::has_active_input_overlay` census; a table-driven
`TestBackend::cursor_position` test over every input-owning surface.

**Interfaces — Produces:** `Editor::has_active_input_overlay(&self) -> bool`. **Consumes:** existing
`place_cursor`, `render_overlays::paint`, `palette.cursor`, overlay `query` fields.

**TDD steps.**

1. **Failing tests** — `render.rs` tests (mirror the existing `TestBackend::cursor_position`
   precedent — grep `cursor_position` in `render.rs`). All openers below are REAL and verified:
   `open_palette()`, `open_outline()`, `open_theme_picker()`, `open_file_browser(PathBuf)`,
   `open_search(Phase, origin)`, `open_minibuffer(prompt, MinibufferKind)`, `open_prompt(Prompt)`,
   `open_diag(Diagnostic)`; `menu`/`splash`/`cursor_picker` set the real `Option` field directly
   (`e.menu = Some(crate::menu::build(&reg, &km, &e))`, `e.splash = Some(crate::splash::Splash::new(&km,
   "0.0.0"))`, `e.cursor_picker = Some(...)`). NO `open_menu_or_prompt_equivalent` exists.

   **Test A — the predicate is exhaustively true for every input surface** (so an added-but-unguarded
   surface fails loudly). Helper opens a surface, asserts `has_active_input_overlay()`:
   ```rust
   #[test]
   fn has_active_input_overlay_true_for_every_surface() {
       use crate::config::CaretShape;
       let reg = crate::registry::Registry::builtins();
       let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
       let mk = || { let mut e = Editor::new_from_text("hello world\n", None, (40, 12));
                     crate::derive::rebuild(&mut e); e };
       // search
       let mut e = mk(); e.open_search(crate::search_overlay::Phase::Find, 0);
       assert!(e.has_active_input_overlay(), "search");
       // minibuffer
       let mut e = mk(); e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
       assert!(e.has_active_input_overlay(), "minibuffer");
       // palette
       let mut e = mk(); e.open_palette();       assert!(e.has_active_input_overlay(), "palette");
       // outline
       let mut e = mk(); e.open_outline();       assert!(e.has_active_input_overlay(), "outline");
       // theme_picker
       let mut e = mk(); e.open_theme_picker();  assert!(e.has_active_input_overlay(), "theme_picker");
       // file_browser
       let mut e = mk(); e.open_file_browser(std::path::PathBuf::from("."));
       assert!(e.has_active_input_overlay(), "file_browser");
       // menu (direct field — build the real MenuView)
       let mut e = mk(); e.menu = Some(crate::menu::build(&reg, &km, &e));
       assert!(e.has_active_input_overlay(), "menu");
       // prompt
       let mut e = mk(); e.open_prompt(crate::prompt::Prompt::swap_recovery());
       assert!(e.has_active_input_overlay(), "prompt");
       // splash (direct field)
       let mut e = mk(); e.splash = Some(crate::splash::Splash::new(&km, "0.0.0"));
       assert!(e.has_active_input_overlay(), "splash");
       // diag
       let mut e = mk();
       e.open_diag(wordcartel_core::diagnostics::Diagnostic {
           // fill from the real struct — grep `Diagnostic {` in diag_overlay.rs / render.rs tests
           // for the exact fields (range, message, severity, source, code, href, …).
           ..diag_fixture()
       });
       assert!(e.has_active_input_overlay(), "diag");
       // cursor_picker (T6 minimal struct)
       let mut e = mk();
       e.cursor_picker = Some(crate::cursor_picker::CursorPicker {
           selected: 1, original_shape: CaretShape::Default, original_blink: true });
       assert!(e.has_active_input_overlay(), "cursor_picker");
   }
   ```
   > `diag_fixture()` = a local helper building a valid `wordcartel_core::diagnostics::Diagnostic` —
   > copy the literal from `diag_overlay.rs`'s test (grep `Diagnostic {` there) so the fields are
   > real. `Phase`/`MinibufferKind` variants are verified (`Phase::Find`; `MinibufferKind::Filter`).

   **Test B — caret placement/suppression via `TestBackend::cursor_position`.** `cursor_position()`
   returns the last `set_cursor_position` of the frame. Assert IN-FIELD carets for the query overlays
   and SUPPRESSION (caret NOT at the editor text-area cell) for a hide surface:
   Use the EXISTING `render.rs` test helper `render_capturing_cursor(e: &mut Editor, w, h) ->
   Option<(u16, u16)>` (verified, render.rs:843) — the Task 6 tests live in the same `render.rs` test
   module, so they call it directly. Its documented convention: it returns `Some((p.x, p.y))` ALWAYS,
   and a **suppressed caret shows as `Some((0, 0))`** (the `TestBackend` default — no `set_cursor_position`
   ran). The existing `place_cursor_minibuffer_hides_caret_..._no_wraparound` test asserts exactly this.
   ```rust
   #[test]
   fn palette_query_shows_caret_mid_string() {
       let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
       crate::derive::rebuild(&mut ed);
       ed.open_palette();
       if let Some(p) = ed.palette.as_mut() { p.query = "abc".into(); p.cursor = 1; }
       let ov = crate::chrome_geom::palette_overlay_rect(
           ratatui::layout::Rect::new(0, 0, 40, 12), ed.palette.as_ref().unwrap().rows.len());
       let cur = render_capturing_cursor(&mut ed, 40, 12);
       // query_area.x == ov.x + 1; prefix "> " == 2 cols; cursor == 1 char into "abc"; query row == ov.y + 1
       assert_eq!(cur, Some((ov.x + 1 + 2 + 1, ov.y + 1)), "mid-string caret in the palette query");
   }

   #[test]
   fn menu_open_suppresses_editor_caret() {
       let reg = crate::registry::Registry::builtins();
       let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
       let mut ed = Editor::new_from_text("hello world\n", None, (40, 12));
       crate::derive::rebuild(&mut ed);
       // put the editor caret off-origin so suppression (→ (0,0)) is unambiguous:
       ed.active_mut().document.selection =
           wordcartel_core::selection::Selection::single("hello world".len());
       let baseline = render_capturing_cursor(&mut ed, 40, 12);
       assert_ne!(baseline, Some((0, 0)), "precondition: editor caret is off-origin at rest");
       // open the menu (a hide surface) — arm-3 must suppress, and menu places no caret:
       ed.menu = Some(crate::menu::build(&reg, &km, &ed));
       let cur = render_capturing_cursor(&mut ed, 40, 12);
       assert_eq!(cur, Some((0, 0)),
           "arm-3 must suppress the editor caret under a modal (B11); suppression == (0,0)");
   }
   ```
   > Add analogous in-field assertions for outline/theme_picker/file_browser (end-of-query column =
   > `ov.x + 1 + 2 + query.chars().count()`, row `ov.y + 1`) and suppression (`Some((0,0))`) for
   > prompt/splash/diag. Split across several `#[test]`s (as above) rather than one giant fn to stay
   > under `clippy::too_many_lines` (100). For `search`/`minibuffer` the caret sits on the STATUS row
   > (arms 1/2) — assert `cur` is `Some((_, status_row))` and not `(0,0)`. Verify `Selection::single`
   > and `active_mut().document.selection` against the tree (grep in `editor.rs`/`selection.rs`); if
   > the setter differs, move the caret via the real navigation command instead.
   **Run (pre-impl):** `cargo test -p wordcartel has_active_input_overlay_true_for_every_surface
   palette_query_shows_caret_mid_string menu_open_suppresses_editor_caret` → **compile error**
   (`has_active_input_overlay` absent) THEN, once the predicate exists but before the guard/placements,
   assertion failures (arm-3 parks under the modal; query overlays place no caret).

2. **Impl — census predicate** (`editor.rs`, near the overlay fields):
   ```rust
   /// True while any modal/overlay owns text input — the caret must NOT be parked in the editor
   /// text area (B11). EXHAUSTIVE by design: a new input surface must be added here (no catch-all).
   pub fn has_active_input_overlay(&self) -> bool {
       self.search.is_some() || self.minibuffer.is_some() || self.palette.is_some()
           || self.outline.is_some() || self.theme_picker.is_some() || self.file_browser.is_some()
           || self.menu.is_some() || self.prompt.is_some() || self.splash.is_some()
           || self.diag.is_some() || self.cursor_picker.is_some()
   }
   ```
   > `cursor_picker` field exists after T7; if T6 lands before T7's field, either (a) sequence T7's
   > field addition into T2, or (b) add the `cursor_picker` field (a `None`-default `Option`) in T6's
   > editor edit and let T7 fill its type. **Decision: add the `cursor_picker:
   > Option<crate::cursor_picker::CursorPicker>` field + the `mod cursor_picker;` stub in T6** so the
   > census is complete; T7 fills the struct/logic. (This keeps T6's census exhaustive and testable.)
   > If the module doesn't exist yet, T6 creates a minimal `cursor_picker.rs` with just `pub struct
   > CursorPicker { pub selected: usize, pub original_shape: crate::config::CaretShape, pub
   > original_blink: bool }` so the field type resolves; T7 extends it.

3. **Impl — arm-3 guard** (`render.rs::place_cursor`): the third arm currently
   `} else if let Some((col, row)) = nav::screen_pos(editor) { ... }`. Gate it:
   ```rust
   } else if !editor.has_active_input_overlay() {
       if let Some((col, row)) = nav::screen_pos(editor) {
           if row < edit_height && tg.text_width > 0 {
               let col = col.min((tg.text_width as usize).saturating_sub(1) as u16);
               frame.set_cursor_position(Position { x: area.x + tg.text_left + col, y: edit_top + row });
           }
       }
   }
   ```
   (Arms 1 and 2 — search/minibuffer — are unchanged; they already place correctly and their fields
   being `Some` also short-circuit arm 3 via the `else if` chain, so the guard is belt-and-suspenders
   for them and load-bearing for palette/outline/etc.)

4. **Impl — overlay caret placements** (`render_overlays.rs`): in each of the four query arms (grep
   the four `cs.ov_query` sites), AFTER the query `Paragraph` render, place the caret on the local
   `query_area`. Factor the `"> "` prefix to one const to avoid painter/caret drift:
   ```rust
   const OV_QUERY_PREFIX_COLS: u16 = 2; // "> " — SINGLE SOURCE for painter + caret
   ```
   - **palette** (mid-string): 
     ```rust
     let caret_col = query_area.x + OV_QUERY_PREFIX_COLS
         + palette.query[..palette.cursor].chars().count() as u16;
     if caret_col < query_area.x + query_area.width {
         frame.set_cursor_position(Position { x: caret_col, y: query_area.y });
     }
     ```
   - **outline / theme_picker / file_browser** (end-of-query): same but
     `+ <overlay>.query.chars().count() as u16`. (Outline also has a `cursor` field pinned to the
     end; end-of-query is equivalent — plan uses `query.chars().count()` for all three; an optional
     robustness note: outline could use `outline.cursor`.)
   Import `Position` (grep the ratatui import block at the top of `render_overlays.rs`; add
   `layout::Position` if absent).

5. **Green:** `cargo test -p wordcartel caret_placement_across_input_surfaces` (+ per-surface tests) →
   pass. `cargo test -p wordcartel --test module_budgets` → render.rs ≤ 900 (guard + placements add a
   few lines; verify). `cargo build`/`clippy` clean.

6. **Commit:** `C1 T6: B11 — arm-3 hide-guard + overlay query carets + input-surface census` (+ trailers).

---

### Task 7 — `cursor_picker.rs`: struct + ROW_ACTIONS + open/XOR + intercept + preview + render + mouse + e2e

**Deliverable:** the full cursor picker — fixed-list overlay, live sample-cell preview (Fork 5-C),
Esc-restore / Enter-commit, mouse support, the `cursor` command, and an e2e journey. Extends the
minimal `CursorPicker` stub from T6.

**Interfaces — Produces:**
- `cursor_picker::CursorPicker { selected: usize, original_shape: CaretShape, original_blink: bool }`
  (extends T6's stub).
- `cursor_picker::ROW_ACTIONS: [(&'static str /*label*/, &'static str /*glyph*/, CaretShape,
  Option<bool>); 7]` — the total `row → (shape, Option<blink>)` table (Option<blink> = None only for
  row 0 Default).
- `cursor_picker::intercept(msg, editor, ex, clock, msg_tx) -> Handled`.
- `Editor::open_cursor_picker(&mut self)`.
- `cursor_picker::preview_selected(editor: &mut Editor)` and `commit_cursor_picker(editor: &mut
  Editor)` (funnel; also called by mouse.rs).
- `Harness::cursor_pos(&self) -> (u16, u16)` — test-infra helper in `e2e.rs` reading the inherent
  `TestBackend::cursor_position()` (the render.rs:849 precedent; no `Backend` trait import). Added in
  this task for the sample-cell position assertion.

**Consumes:** T2 setters, T1 types, T6 census (`has_active_input_overlay` already includes
`cursor_picker`), the theme_picker/list_window/render_overlays precedents.

**TDD steps.**

1. **Failing tests** — `cursor_picker.rs` tests + an `e2e.rs` journey.
   ```rust
   // cursor_picker.rs
   #[test]
   fn row_actions_total_and_row0_preserves_blink() {
       assert_eq!(ROW_ACTIONS.len(), 7);
       assert!(matches!(ROW_ACTIONS[0].2, crate::config::CaretShape::Default));
       assert_eq!(ROW_ACTIONS[0].3, None, "row 0 leaves caret_blink untouched");
       assert!(ROW_ACTIONS[1..].iter().all(|r| r.3.is_some()), "all concrete rows set blink");
   }
   #[test]
   fn preview_applies_row_action_row0_keeps_blink_off() {
       let mut e = Editor::new_from_text("x\n", None, (40, 12));
       e.set_caret_blink(false);              // user prefers no blink
       e.open_cursor_picker();
       // select row 0 (Default) and preview → shape Default, blink UNCHANGED (still false)
       if let Some(p) = e.cursor_picker.as_mut() { p.selected = 0; }
       preview_selected(&mut e);
       assert_eq!(e.caret_shape, crate::config::CaretShape::Default);
       assert!(!e.caret_blink, "row 0 must not touch blink");
       // select row 2 (Block · steady) → shape Block, blink false
       if let Some(p) = e.cursor_picker.as_mut() { p.selected = 2; }
       preview_selected(&mut e);
       assert_eq!(e.caret_shape, crate::config::CaretShape::Block);
       assert!(!e.caret_blink);
   }
   #[test]
   fn open_cursor_picker_enforces_xor_and_captures_original() {
       let mut e = Editor::new_from_text("x\n", None, (40, 12));
       e.set_caret_shape(crate::config::CaretShape::Beam); e.set_caret_blink(true);
       e.open_palette();
       e.open_cursor_picker();
       assert!(e.cursor_picker.is_some());
       assert!(e.palette.is_none(), "opening cursor picker closes the palette (XOR)");
       let p = e.cursor_picker.as_ref().unwrap();
       assert_eq!(p.original_shape, crate::config::CaretShape::Beam);
       assert!(p.original_blink);
   }
   #[test]
   fn esc_restores_original_options() {
       let mut e = Editor::new_from_text("x\n", None, (40, 12));
       e.set_caret_shape(crate::config::CaretShape::Default); e.set_caret_blink(true);
       e.open_cursor_picker();
       if let Some(p) = e.cursor_picker.as_mut() { p.selected = 3; } // Beam · blinking
       preview_selected(&mut e);
       assert_eq!(e.caret_shape, crate::config::CaretShape::Beam);
       // simulate Esc: restore original then close
       let orig = (e.cursor_picker.as_ref().unwrap().original_shape,
                   e.cursor_picker.as_ref().unwrap().original_blink);
       e.set_caret_shape(orig.0); e.set_caret_blink(orig.1); e.cursor_picker = None;
       assert_eq!(e.caret_shape, crate::config::CaretShape::Default);
   }
   ```
   And a COMPLETE `e2e.rs` journey (in-crate `#[cfg(test)]` module — the test lives IN `e2e.rs`, so it
   uses the real `Harness` verified surface: `Harness::new(text, path, size)`, `.step(Msg)`, `.key(KeyCode)`,
   `.render()`, `.editor.borrow()/borrow_mut()`, and the in-module private field `.term:
   Terminal<TestBackend>`). It asserts the sample-cell caret POSITION + the option STATE (the DECSCUSR
   style byte bypasses `TestBackend`, so we assert position + option fields, never the emitted escape —
   Vec<u8> reconcile tests + smoke S7 cover the byte).

   **Test-infra addition (this task):** add one small helper to `Harness` (test infra, same TDD task) —
   there is NO existing cursor accessor on `Harness`; the INHERENT `TestBackend::cursor_position()`
   returns a `Position` directly (the render.rs:849 precedent uses exactly this — no `Backend` trait
   import, direct `.x`/`.y`):
   ```rust
   // in impl Harness (e2e.rs)
   fn cursor_pos(&self) -> (u16, u16) {
       let p = self.term.backend().cursor_position(); // TestBackend inherent method → Position
       (p.x, p.y)
   }
   ```

   **Opening the picker:** the 9 caret commands ALL start with "Caret…"/"Caret Shape…"/"Caret Blink…",
   so a palette top-row label match on "caret" is fragile (per the plan-gate guidance). Open via the
   direct opener `h.editor.borrow_mut().open_cursor_picker()` — the `cursor` command's
   registration/palette-reachability is separately covered by Task 4's palette-completeness gate and a
   Task 7 registry unit test (`cursor_command_opens_picker`, below). This keeps the journey unambiguous.
   ```rust
   #[test]
   fn cursor_picker_live_preview_sample_caret_and_commit() {
       use crate::config::CaretShape;
       let mut h = Harness::new("hello world\n", None, (60, 16));
       crate::derive::rebuild(&mut h.editor.borrow_mut());
       // user starts with blink OFF (to prove row-0 preserves it later)
       h.editor.borrow_mut().set_caret_blink(false);

       // baseline: editor caret with NO overlay (for the "sample caret is elsewhere" contrast)
       h.render();
       let editor_caret = h.cursor_pos();

       // open the cursor picker (direct opener; XOR-clears others)
       h.editor.borrow_mut().open_cursor_picker();
       h.render();
       assert!(h.editor.borrow().cursor_picker.is_some(), "picker open");
       // initial selection = blinking block (row 1) off Default (decision #4)
       assert_eq!(h.editor.borrow().cursor_picker.as_ref().unwrap().selected, 1);
       // sample-cell caret is placed INSIDE the overlay — not at the editor text-area cell:
       let sample_pos = h.cursor_pos();
       assert_ne!(sample_pos, editor_caret,
           "picker sample caret must sit in the overlay, not the editor text area");

       // arrow Down to row 2 (Block · steady) → preview applies shape=Block, blink=false
       h.key(KeyCode::Down);
       h.render();
       {
           let e = h.editor.borrow();
           assert_eq!(e.caret_shape, CaretShape::Block);
           assert!(!e.caret_blink, "row 2 steady → blink false");
           assert_eq!(e.cursor_picker.as_ref().unwrap().selected, 2);
       }
       // the sample caret is FIXED (same cell) across selection moves — it is the live-morph anchor:
       assert_eq!(h.cursor_pos(), sample_pos, "sample caret stays on the fixed sample cell");

       // Esc → restore originals (Default shape; blink stays as the user had it: false)
       h.key(KeyCode::Esc);
       h.render();
       {
           let e = h.editor.borrow();
           assert!(e.cursor_picker.is_none(), "Esc closes the picker");
           assert_eq!(e.caret_shape, CaretShape::Default, "Esc restores original shape");
           assert!(!e.caret_blink, "blink unchanged (was false)");
       }

       // reopen, move to row 0 (Default) — must NOT touch blink — then Enter to commit
       h.editor.borrow_mut().open_cursor_picker();
       h.key(KeyCode::Up);                              // from row 1 up to row 0
       h.render();
       assert_eq!(h.editor.borrow().cursor_picker.as_ref().unwrap().selected, 0);
       h.key(KeyCode::Enter);
       {
           let e = h.editor.borrow();
           assert!(e.cursor_picker.is_none(), "Enter commits + closes");
           assert_eq!(e.caret_shape, CaretShape::Default);
           assert!(!e.caret_blink, "row 0 commit preserved blink=false");
       }

       // reopen, pick row 3 (Beam · blinking), Enter, then assert the real settings snapshot persists it.
       // open_cursor_picker → initial_row_for(Default, _) == 1 (decision #4). ListNav::Down is +1
       // (list_window.rs:56), so from row 1 press Down TWICE to reach row 3 (NOT three times → row 4).
       h.editor.borrow_mut().open_cursor_picker();
       assert_eq!(h.editor.borrow().cursor_picker.as_ref().unwrap().selected, 1, "initial row = 1");
       h.key(KeyCode::Down); // 1→2 (Block · steady)
       h.key(KeyCode::Down); // 2→3 (Beam · blinking)
       assert_eq!(h.editor.borrow().cursor_picker.as_ref().unwrap().selected, 3, "landed on Beam · blinking");
       h.key(KeyCode::Enter);
       {
           let e = h.editor.borrow();
           assert_eq!(e.caret_shape, CaretShape::Beam);
           assert!(e.caret_blink, "row 3 (ROW_ACTIONS[3]) = Beam · blinking → blink true");
           // real persistence path (settings.rs:186, verified): runtime_snapshot(&Editor).
           let snap = crate::settings::runtime_snapshot(&e);
           assert_eq!(snap.view_caret_shape, CaretShape::Beam);
           assert!(snap.view_caret_blink);
       }
   }
   ```
   > **Verified real symbols:** `Harness::{new, step, key, render, editor, term}` and the `KeyCode`
   > import already in `e2e.rs`; `TestBackend::cursor_position()` inherent method (render.rs:849
   > precedent, returns `Position`); `settings::runtime_snapshot(&Editor) -> SettingsSnapshot`
   > (settings.rs:186). NO `dispatch_cmd`, NO `get_cursor_position`/`.backend().cursor_position().expect(..)`,
   > NO palette label-match for opening. The `cursor_pos` helper is the only test-infra addition.

   Plus an `e2e.rs` paste-swallow test — a bracketed paste while the picker is open must NOT reach the
   document (the intercept's `Event::Paste` arm; the picker has no query field, so it is a pure no-op):
   ```rust
   #[test]
   fn cursor_picker_swallows_paste() {
       use crossterm::event::Event;
       let mut h = Harness::new("doc\n", None, (60, 16));
       crate::derive::rebuild(&mut h.editor.borrow_mut());
       h.editor.borrow_mut().open_cursor_picker();
       assert!(h.editor.borrow().cursor_picker.is_some(), "picker open");
       h.step(Msg::Input(Event::Paste("XYZ".into())));   // bracketed paste while open
       assert_eq!(h.doc_text(), "doc\n", "paste must NOT leak into the document behind the picker");
       assert!(h.editor.borrow().cursor_picker.is_some(), "paste is a no-op; picker stays open");
   }
   ```
   > `Harness::doc_text()` is a verified accessor (e2e.rs); `Msg`/`Event` are already imported there.

   Plus a small registry unit test (in `registry.rs` or `cursor_picker.rs`) covering the command path
   the e2e journey deliberately bypasses:
   ```rust
   #[test]
   fn cursor_command_opens_picker() {
       let mut ed = Editor::new_from_text("x\n", None, (40, 12));
       dispatch_id(&mut ed, "cursor");
       assert!(ed.cursor_picker.is_some(), "the `cursor` command opens the picker");
   }
   ```
   **Run (pre-impl):** `cargo test -p wordcartel --lib cursor_picker`, `cargo test -p wordcartel
   cursor_command_opens_picker`, and the e2e tests → **compile error** (`CursorPicker`/`ROW_ACTIONS`/
   `preview_selected`/`open_cursor_picker`/`cursor` command/`cursor_pos` helper absent). After impl:
   **pass.**

2. **Impl — struct + table** (`cursor_picker.rs`, extend T6 stub):
   ```rust
   use crate::config::CaretShape;
   use crate::app::{Handled, Msg};
   use crossterm::event::Event;

   pub struct CursorPicker {
       pub selected: usize,
       pub original_shape: CaretShape,
       pub original_blink: bool,
   }

   /// Total row → (shape, Option<blink>) table. `None` blink = leave caret_blink unchanged
   /// (row 0 Default only — blink is inert under Default). Glyphs are DESCRIPTIVE (honest on a
   /// DECSCUSR-ignoring terminal). Row 1 (blinking block) is the first managed stop (decision #4).
   pub const ROW_ACTIONS: [(&str, &str, CaretShape, Option<bool>); 7] = [
       ("Default (terminal)",  " ", CaretShape::Default,   None),
       ("Block \u{00b7} blinking",   "\u{2588}", CaretShape::Block,     Some(true)),
       ("Block \u{00b7} steady",     "\u{2588}", CaretShape::Block,     Some(false)),
       ("Beam \u{00b7} blinking",    "\u{258f}", CaretShape::Beam,      Some(true)),
       ("Beam \u{00b7} steady",      "\u{258f}", CaretShape::Beam,      Some(false)),
       ("Underline \u{00b7} blinking","\u{2581}", CaretShape::Underline, Some(true)),
       ("Underline \u{00b7} steady",  "\u{2581}", CaretShape::Underline, Some(false)),
   ];

   /// Apply the selected row's action via the shared setters (the ONE code path — total over the
   /// table). None blink ⇒ leave caret_blink untouched.
   pub(crate) fn preview_selected(editor: &mut crate::editor::Editor) {
       let sel = editor.cursor_picker.as_ref().map(|p| p.selected).unwrap_or(0);
       let (_, _, shape, blink) = ROW_ACTIONS[sel.min(ROW_ACTIONS.len() - 1)];
       editor.set_caret_shape(shape);
       if let Some(b) = blink { editor.set_caret_blink(b); }
   }

   /// Enter-commit: options already hold the previewed values (set live); just close.
   pub(crate) fn commit_cursor_picker(editor: &mut crate::editor::Editor) {
       editor.cursor_picker = None;
   }
   ```
   > **Glyph note (plan-level call the spec left open):** row glyphs are `█` (U+2588 block), `▏`
   > (U+258F left bar / beam), `▁` (U+2581 lower block / underline), separator `·` (U+00B7); row 0
   > uses a space (no sample glyph — Default is "terminal's own"). These are descriptive only; the
   > REAL preview is the live sample-cell caret. Adjust if any renders poorly in the smoke terminal.

3. **Impl — open/XOR** (`editor.rs`): mirror `open_theme_picker` exactly (clear all other overlays),
   capturing originals:
   ```rust
   pub fn open_cursor_picker(&mut self) {
       self.prompt = None; self.minibuffer = None; self.menu = None;
       self.pending_keys.clear(); self.pending_mark = None;
       self.search = None; self.diag = None; self.outline = None; self.palette = None;
       self.file_browser = None; self.theme_picker = None;
       // initial selection: blinking block (row 1) when currently Default, else the matching row.
       let selected = crate::cursor_picker::initial_row_for(self.caret_shape, self.caret_blink);
       self.cursor_picker = Some(crate::cursor_picker::CursorPicker {
           selected, original_shape: self.caret_shape, original_blink: self.caret_blink,
       });
   }
   ```
   Add `cursor_picker::initial_row_for(shape, blink) -> usize` (returns 1 when shape==Default —
   decision #4; else the ROW_ACTIONS index whose (shape, Some(blink)) matches, defaulting 0). Also add
   `self.cursor_picker = None;` to the OTHER overlays' XOR-clear blocks (`open_palette`,
   `open_outline`, `open_theme_picker`, `open_buffer_switcher`, `open_file_browser`) and to the
   `menu` command's clear block in `registry.rs` (grep `theme_picker = None` for every site and add a
   sibling line).

4. **Impl — intercept stage** (`cursor_picker.rs`): shaped like `theme_picker::intercept`
   (Esc→restore+close, Enter→commit, list-nav→preview; ignore char input — fixed list) but BOTH paste
   arms are no-ops here (the picker has no query field to append to — see the arm comments below;
   theme_picker instead appends `Event::Paste` to its query):
   ```rust
   pub(crate) fn intercept(msg: Msg, editor: &mut crate::editor::Editor,
       ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
       msg_tx: &std::sync::mpsc::Sender<Msg>) -> Handled {
       if editor.cursor_picker.is_none() { return Handled::Pass(msg); }
       // Paste swallow FIRST. The async clipboard-paste-result arm mirrors theme_picker.rs:46
       // (`Msg::ClipboardPaste` → no-op drop). The bracketed-paste arm is a no-op HERE precisely
       // because the cursor picker has NO query field to append to — UNLIKE theme_picker.rs:51–55,
       // which appends `Event::Paste` text to its query. Both arms must be consumed so neither leaks
       // into the document behind the overlay (app.rs:299–307 would otherwise insert Event::Paste text).
       if matches!(&msg, Msg::ClipboardPaste { .. }) {
           return Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
       }
       if matches!(&msg, Msg::Input(Event::Paste(_))) {
           return Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
       }
       if let Msg::Input(Event::Key(k)) = &msg {
           if k.kind == crossterm::event::KeyEventKind::Press {
               use crossterm::event::KeyCode;
               match k.code {
                   KeyCode::Esc => {
                       if let Some(p) = editor.cursor_picker.take() {
                           editor.set_caret_shape(p.original_shape);
                           editor.set_caret_blink(p.original_blink);
                       }
                   }
                   KeyCode::Enter => { crate::cursor_picker::commit_cursor_picker(editor); }
                   c if crate::list_window::list_nav_key(c).is_some() => {
                       let ah = editor.active().view.area.1;
                       if let Some(p) = editor.cursor_picker.as_mut() {
                           let mut st = 0usize; // fixed list — no scroll window needed, but reuse the API
                           crate::list_window::apply_list_nav(
                               crate::list_window::list_nav_key(c).unwrap(),
                               ah, crate::cursor_picker::ROW_ACTIONS.len(), &mut p.selected, &mut st);
                       }
                       crate::cursor_picker::preview_selected(editor);
                   }
                   _ => {}
               }
           }
           return Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
       }
       Handled::Pass(msg)
   }
   ```
   > Verify `list_nav_key`/`apply_list_nav` signatures by grep (`list_window.rs`): `list_nav_key(code)
   > -> Option<ListNav>`, `apply_list_nav(nav, area_h: u16, row_count: usize, selected: &mut usize,
   > scroll_top: &mut usize)`. The 7-row list fits any pane, so `scroll_top` is a throwaway local.
   > Also verify `fold_and_continue`'s exact signature in `app.rs` and mirror the theme_picker call.
   Register the stage in `app::reduce_dispatch` immediately AFTER the `theme_picker::intercept` line
   (grep `crate::theme_picker::intercept` in `app.rs`):
   ```rust
   let msg = match crate::cursor_picker::intercept(msg, editor, ex, clock, msg_tx) {
       crate::app::Handled::Done(keep) => return keep,
       crate::app::Handled::Pass(m) => m,
   };
   ```
   (Match the exact arm shape of the neighboring interceptors — grep the `theme_picker` block.)

5. **Impl — `cursor` command** (`registry.rs`, beside the caret rows from T4):
   ```rust
   r.register("cursor", "Caret\u{2026}", Some(MenuCategory::View), |c| {
       c.editor.open_cursor_picker(); CommandResult::Handled });
   ```

6. **Impl — render + sample cell** (`render_overlays.rs`): add a picker arm mirroring the
   theme-picker arm (`palette_overlay_rect`, bordered box, `ROW_ACTIONS` rows with glyph + label via
   `list_window`), PLUS a dedicated sample cell and its caret placement:
   ```rust
   if let Some(ref cp) = editor.cursor_picker {
       let ov_rect = palette_overlay_rect(area, crate::cursor_picker::ROW_ACTIONS.len() + 1);
       // ... draw Clear + bordered Block + the 7 rows (highlight cp.selected) ...
       // Sample cell: a "Preview: X" line inside the overlay; place the caret at the X column.
       let sample_row = ov_rect.y + ov_rect.height.saturating_sub(2); // second-to-last inner row
       let sample_label = format!("Preview: {}", crate::cursor_picker::ROW_ACTIONS[cp.selected].1);
       // render sample_label at (ov_rect.x + 1, sample_row) ...
       let caret_x = ov_rect.x + 1 + "Preview: ".chars().count() as u16;
       if caret_x < ov_rect.x + ov_rect.width {
           frame.set_cursor_position(Position { x: caret_x, y: sample_row });
       }
   }
   ```
   > **Sample-cell placement (plan-level call):** the sample cell is a `Preview: <glyph>` line on the
   > overlay's second-to-last inner row; the caret sits at the `<glyph>` column. This is the sole
   > on-screen caret while the picker is open (arm-3 suppressed via T6's census, which includes
   > `cursor_picker`), so `reconcile_cursor_style` morphs THIS caret as the selection changes
   > (Fork 5-C live morph). On a DECSCUSR-ignoring terminal the glyph label still describes the
   > selection. Finalize the exact row/column against the real `palette_overlay_rect` geometry and
   > clamp to the overlay's right edge (mirror the query-caret clamp). Guard against a too-short
   > overlay (skip the sample caret if `ov_rect.height < 3`).

7. **Impl — mouse** (`mouse.rs`, TWO Modify sites):
   - **(a) `no_overlay_open` (mouse.rs:73) — MUST add `cursor_picker`.** Overlay mouse routing only
     runs when `!no_overlay_open(editor)` (the guard at mouse.rs:471 calls `route_overlay` only then).
     `no_overlay_open` currently enumerates `menu/palette/theme_picker/file_browser/outline/diag/
     prompt/minibuffer/search` — WITHOUT `cursor_picker`, the picker's wheel/click handlers would
     never fire and mouse input would fall through to the editor/dwell behavior beneath the modal. Add
     the term, mirroring how `theme_picker` is enumerated:
     ```rust
     fn no_overlay_open(editor: &Editor) -> bool {
         editor.menu.is_none() && editor.palette.is_none() && editor.theme_picker.is_none()
             && editor.file_browser.is_none() && editor.outline.is_none() && editor.diag.is_none()
             && editor.prompt.is_none() && editor.minibuffer.is_none() && editor.search.is_none()
             && editor.cursor_picker.is_none()   // ← C1: route mouse to the cursor picker
     }
     ```
   - **(b) `route_overlay` (mouse.rs) — add a `cursor_picker` branch** mirroring the `theme_picker`
     branch (grep `editor.theme_picker.is_some()` inside `route_overlay`): wheel → move `cp.selected`
     (via `list_window::apply_list_nav` or the same ± the theme-picker branch uses) + `preview_selected`;
     click on a row → set `selected` + `preview_selected`; click-to-commit → `commit_cursor_picker`;
     click-away → Esc-equivalent (restore originals + close). Reuse the shared funnel fns (no bypass of
     the setters). `route_overlay` already carries `#[allow(clippy::too_many_lines)]` (one branch per
     overlay is the sanctioned shape), so a new branch is in-budget — but keep the branch a thin
     delegation.

8. **Green:** `cargo test -p wordcartel --lib cursor_picker`, the e2e journey, and the FULL suites:
   `cargo test -p wordcartel`, `cargo test -p wordcartel --test module_budgets` (app.rs ≤ 1000,
   render.rs ≤ 900 — the picker render arm adds to `render_overlays.rs`, NOT the render.rs hub, so
   render.rs budget is unaffected; verify), palette-completeness + LAW-2 green. `cargo clippy
   --workspace --all-targets` clean. Run `scripts/smoke/run.sh`, record the summary.

9. **Commit:** `C1 T7: cursor picker — ROW_ACTIONS + live sample-cell preview + command + e2e`
   (+ trailers).

---

## Testing summary (per spec §10)

| Coverage | Where | Task |
|---|---|---|
| `desired_caret_style` composition (7 combos, our pair) + `to_set_cursor_style` mapper | `cursor_style` unit | T3 |
| Reconcile: Default→no write; concrete→write once; **no-write-at-rest guardrail**; →Default unmanage | `cursor_style` Vec<u8> | T3 |
| Restore latch monotonic (`mark_written`→`ever_wrote`) | `cursor_style::restore` | T3 |
| `restore_caret_if_written` gated by latch (emit/no-emit) | `cursor_style::restore` Vec<u8> | **T5** |
| Startup-apply (persisted shape emits on frame one) | `cursor_style` Vec<u8> (unit) + T7 e2e/smoke (integration) | T3/T7 |
| Command set/cycle/toggle | `registry` | T4 |
| LAW-2 exhaustive-destructure + assertions | `settings` | T4 |
| Diff-law round-trip (both fields) | `settings` | T4 |
| Palette-completeness (auto) | existing invariant | T4 |
| B11 `TestBackend::cursor_position` census (every input surface + sample cell) | `render` | T6/T7 |
| Row-action totality + row-0-preserves-blink | `cursor_picker` | T7 |
| e2e picker journey (sample-cell POSITION, preview/Esc/Enter, persist; nav validated vs ROW_ACTIONS) | `e2e` | T7 |
| Paste-swallow (bracketed paste while picker open does NOT modify the document) | `e2e` | T7 |
| `cursor` command opens the picker | `registry`/`cursor_picker` | T7 |
| Restore on panic (real terminal) | `scripts/smoke` S7 | advisory |

**DECSCUSR-bytes-bypass-TestBackend limitation** (spec §10): the style escape goes to the crossterm
backend, not the ratatui cell buffer. Mechanism coverage is Vec<u8> (T3); the e2e journey asserts the
sample-cell caret POSITION, not the style byte; smoke S7 is the advisory real-terminal eyeball. No
test asserts a terminal HONORED DECSCUSR (no detection — spec decision #6).

---

## Merge gates (per CLAUDE.md — all must pass)

- `cargo test` green across `wordcartel-core` (unchanged) + `wordcartel`.
- `cargo build` + `cargo test --no-run` warning-free for `wordcartel`.
- `cargo clippy --workspace --all-targets` clean (`all = "deny"`).
- `cargo test -p wordcartel --test module_budgets` green (app ≤ 1000, render ≤ 900).
- Command-surface invariants green: palette-completeness, `every_persisted_setting_has_a_command`,
  hint re-resolution.
- `scripts/smoke/run.sh` run; one-line summary (incl. S7) quoted in the pre-merge report (advisory).
- The two final gates: Fable whole-branch review + Codex pre-merge GO/NO-GO.

---

## Self-review (writing-plans checklist)

- **Spec coverage:** every spec section maps to a task — options/config (T1), runtime/setters/startup
  (T2), reconcile+latch+two call sites (T3/§4.2/§5.1), commands+persistence+LAW-2 (T4/§7),
  `restore_caret_if_written`+restore sites (T5/§5.2), B11 arm-3 guard + overlay carets + census
  (T6/§6), picker + sample cell + ROW_ACTIONS + e2e (T7/§8). Contract conformance (§7) is its own
  section + merge gate. OUT-of-scope (§12) items appear nowhere. C-1…C-11 consequences are each
  honored (arm-3 census T6; blink-in-code T3; process-global latch T3; mid-string only palette T6;
  pre-draw call-site ordering T3; →Default unmanage T3; char-count prefix const T6; 7-row table T7;
  per-arrow write T7; two call sites T3; **latch is our `(CaretShape, bool)` pair not `SetCursorStyle`
  — C-11 — T3**).
- **Placeholder / API scan:** no TODO/TBD; every snippet uses REAL, tree-verified symbols —
  `compute_overrides`/`snap_with`/`OverridesFile` (T4, NOT `build_overrides`/`Masked`); the real
  openers `open_palette`/`open_outline`/`open_theme_picker`/`open_file_browser`/`open_search`/
  `open_minibuffer`/`open_prompt`/`open_diag` + direct `menu`/`splash`/`cursor_picker` field-sets (T6,
  NOT `open_menu_or_prompt_equivalent`); `Harness`/`press`/`runtime_snapshot` (T7 e2e). The only
  plan-level calls the spec left open are made explicitly — the row glyphs (`█`/`▏`/`▁`/`·`, row-0
  blank) and the sample-cell placement (second-to-last inner row, caret at the glyph column) — both
  flagged and finalize-against-geometry-noted.
- **Type consistency across tasks (esp. the new latch type):** `CaretShape`/`caret_shape_str`/
  `caret_shape_from_str` (T1) used verbatim by T2/T3/T4/T7; `set_caret_shape`/`set_caret_blink` (T2)
  called by T3-tests/T4/T7; the latch type `Option<(CaretShape, bool)>` flows identically through
  `desired_caret_style` (T3) → the `applied_caret` `run`-local (T3) → any later reader — and crossterm
  `SetCursorStyle` appears ONLY inside `to_set_cursor_style` / the `execute!` calls, never in a latch
  or comparison; `restore::{mark_written, ever_wrote}` (T3) consumed by `restore_caret_if_written`
  (T5); `has_active_input_overlay` (T6) consumed by `place_cursor` (T6) and includes `cursor_picker`
  (field seeded in T6, filled T7); `CursorPicker`/`ROW_ACTIONS`/`preview_selected`/
  `commit_cursor_picker`/`open_cursor_picker` (T7) match the interfaces block. The `cursor_picker`
  field/module ordering across T6→T7 is explicitly resolved (T6 creates the minimal struct + field so
  its census compiles; T7 extends). The T3/T5 test split is explicit: T3 owns `mark_written`/
  `ever_wrote` + reconcile tests; T5 owns `restore_caret_if_written` + its Vec<u8> emit/no-emit tests.
- **Red-state wording:** each task's pre-impl run is labelled compile-error-vs-assertion-failure
  correctly (new modules/fields → compile error first; guard/placement tasks → assertion failure once
  symbols exist).
- **Task-7 integration wiring (the three round-3 gaps, now closed and grounded):**
  - **Nav math vs `ROW_ACTIONS` + `list_window`:** `open_cursor_picker` → `initial_row_for(Default,_)`
    == **row 1** (decision #4); `ListNav::Down` = +1 clamped (list_window.rs:56). The e2e journey's
    steps are re-derived against this: from row 1, one Down → row 2 (Block·steady, blink false — first
    assertion); Up → row 0 (Default, blink untouched — commit preserves blink=false); and to reach
    **row 3** (Beam·blinking, blink true) from the row-1 start it presses Down **twice** (1→2→3), NOT
    three times (which would land row 4 Beam·steady). A `selected == 3` assertion pins the landing.
  - **Paste-swallow (new Modify site, `cursor_picker::intercept`):** the intercept consumes BOTH
    `Msg::ClipboardPaste` (mirroring theme_picker.rs:46's no-op drop) AND `Msg::Input(Event::Paste(_))`
    — the latter a no-op HERE because the picker has no query field, UNLIKE theme_picker.rs:51–55 which
    APPENDS `Event::Paste` to its query. Both arms must be consumed so a bracketed paste cannot fall
    through to document insertion (app.rs:299–307). Covered by `cursor_picker_swallows_paste` (e2e).
  - **Mouse routing (new Modify site, `mouse.rs`):** `no_overlay_open` (mouse.rs:73) adds
    `editor.cursor_picker.is_none()`, so `!no_overlay_open` fires and `route_overlay` gets a
    `cursor_picker` branch — without this the picker's wheel/click never runs (mouse falls through to
    the editor beneath the modal). Both mirror the `theme_picker` enumeration/branch.
- **Forward-only interfaces:** each task consumes only earlier-task symbols; no back-references.
