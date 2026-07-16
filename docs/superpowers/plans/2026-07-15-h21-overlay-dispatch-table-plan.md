# H21 — Input-overlay dispatch table — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the ~7 hand-parallel overlay-routing enumerations (is-active, intercept-chain, mouse, render, XOR-close) with one `overlays.rs` seam — an exhaustive `OverlayId` enum + `OVERLAYS` fn-pointer table — so adding an overlay is one row, compiler-forced to be complete.

**Architecture:** Mirror `timers.rs`'s `SUBSYSTEMS` table. A new `overlays.rs` holds `enum OverlayId`, `struct OverlayRow` (grown column-by-column across tasks), `static OVERLAYS`, `static RENDER_ORDER`, `struct DispatchCtx`, and `close_all`. Consumers (`editor.rs`, `app.rs`, `mouse.rs`, `render_overlays.rs`, `registry.rs`) collapse their hand-written 11-way chains into table folds. All behavior-preserving EXCEPT one deliberate delta: splash becomes active on the mouse path (Task 1), closing an under-splash dwell-arming quirk.

**Tech Stack:** Rust 2021, ratatui 0.30 + crossterm (shell crate `wordcartel`, binary `wcartel`). No new dependencies.

## Global Constraints

- **`#![forbid(unsafe_code)]` is CORE-only.** H21 is entirely in the `wordcartel` SHELL crate; `wordcartel-core` is untouched. No `unsafe` anywhere.
- **Workspace clippy clean is a GATE.** `cargo clippy --workspace --all-targets` must pass with `[workspace.lints.clippy] all = "deny"`. This includes `clippy::too_many_lines` (threshold 100, `clippy.toml`). A function over 100 lines needs an item-local `#[allow(clippy::too_many_lines)]` with a one-line reason. **The whole POINT of H21 is that `reduce_dispatch` / `route_overlay` / `render_overlays::paint` SHRINK** — watch these.
- **Module-budget GATE** (`wordcartel/tests/module_budgets.rs`): `src/app.rs` ≤ 1000 production lines. `render_overlays.rs` and `mouse.rs` are NOT budgeted, but their hub functions still face `too_many_lines`. The new `overlays.rs` is not budgeted.
- **No `cargo fmt`.** This repo is hand-formatted in a dense house style with NO `rustfmt.toml`. Match neighbors by hand; do not reflow untouched code. Em-dash `—` in prose comments, never `--`. snake_case fns, PascalCase types, 4-space indent.
- **`cargo test` green across all suites** (`wordcartel-core` lib + oracle, `wordcartel` lib). `cargo build` and `cargo test --no-run` warning-free for `wordcartel` (dead-code/unused warnings fail the deny gate — every struct field added must have a reader in the same task).
- **Unwrap discipline:** no `.unwrap()` on fallible/external paths; guarded unwraps get `.expect("…invariant…")`. (The moved bodies keep their existing guarded `.unwrap()`s verbatim — those sit immediately after `is_some()` guards.)
- **Command-surface contract: N/A-leaning — state it in review.** H21 touches no command registrations, no user-settable options, no keybinding hints. The `open_*` commands and the registry `"menu"` command keep their registry entries; `palette` stays both a command and an overlay and stays conformant. No contract amendment; its invariant tests remain green as-is.
- **Anchor on symbol NAMES, not line numbers** (they drift as tasks edit files). For compile/usage/signature questions on code being edited, trust `cargo`/`grep`, not an editor "unused/undefined" hint.

---

## File Structure

**New file:**
- `wordcartel/src/overlays.rs` — the seam. `enum OverlayId` (11 variants) + `impl { ALL, row() }`; `struct OverlayRow` (fields added per task: `name, id, is_active` → `+intercept` → `+close` → `+mouse` → `+render`); `static OVERLAYS`; `static RENDER_ORDER`; `struct DispatchCtx<'a>`; `enum RenderSite`; `fn any_active`; `fn close_all`; `#[cfg(test)] mod tests` (bijection + Q4 guardrail + the full open→XOR/key/click sweep). Registered in `lib.rs`.

**Files that lose hand-parallel code:**
- `wordcartel/src/editor.rs` — `has_active_input_overlay` → `crate::overlays::any_active(self)` (Task 1); the ~10 `open_*` methods' sibling-null lists → `crate::overlays::close_all(self)` (Task 3).
- `wordcartel/src/app.rs` — `reduce_dispatch`'s 12-stage intercept chain → splash-row + marks + `ALL[1..]` loop (Task 2); `dispatch_overlay_command`'s 5 nulls → `close_all` (Task 3).
- `wordcartel/src/mouse.rs` — `no_overlay_open` → `!crate::overlays::any_active` (Task 1); `route_overlay`'s 10 branches → per-overlay `mouse_*` fns + a find-active dispatch, incl. the two Down-left close arms → `close_all` and a new splash mouse slot (Task 4).
- `wordcartel/src/render_overlays.rs` — `paint`'s 8 inline overlay blocks → extracted `paint_*` fns walked in `RENDER_ORDER`; menu **bar** chrome stays a standalone step at the `Menu` slot (Task 5).
- `wordcartel/src/registry.rs` — the `"menu"` command's 9 nulls → `close_all` + preserved toggle (Task 3).
- `wordcartel/src/render.rs` — the B11 census test `has_active_input_overlay_true_for_every_surface` is retired (subsumed by the Task 6 sweep). Its `has_overlay` 5-way enumeration (production) is explicitly OUT of scope and stays.
- **NOT touched:** `save.rs::reload_from_disk` / `save.rs::load_recovered` (their `search=None; diag=None` are post-buffer-replace stale-clears, NOT XOR closes — must never migrate to `close_all`).

**Task order note (deviation from spec §sequencing):** the spec lists mouse-fold before close-fold, but `route_overlay`'s two Down-left close arms depend on `close_all`. So this plan does **close/XOR (Task 3) before mouse (Task 4)**, so `close_all` exists before its mouse-path callers. All other ordering matches the spec.

---

## Task 1: Table spine + is-active axis (+ the Q4 splash-mouse delta)

Introduces `overlays.rs` with the `is_active` column and migrates both is-active predicates. This is the ONLY task with a deliberate behavior change: `no_overlay_open` now counts `splash`, which suppresses dwell-timer arming while the splash is up (spec §3, Q4=A).

**Files:**
- Create: `wordcartel/src/overlays.rs`
- Modify: `wordcartel/src/lib.rs` (add `pub mod overlays;`)
- Modify: `wordcartel/src/editor.rs` (`has_active_input_overlay`)
- Modify: `wordcartel/src/mouse.rs` (`no_overlay_open`)
- Test: `wordcartel/src/overlays.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub(crate) enum OverlayId { Splash, Menu, Palette, ThemePicker, CursorPicker, FileBrowser, Prompt, Minibuffer, Search, Diag, Outline }` (derives `Copy, Clone, PartialEq, Eq, Debug`).
  - `impl OverlayId { pub(crate) const ALL: &'static [OverlayId]; pub(crate) fn row(self) -> &'static OverlayRow; }`
  - `pub(crate) struct OverlayRow { name: &'static str, id: OverlayId, is_active: fn(&Editor) -> bool }` (fields grow in later tasks).
  - `pub(crate) static OVERLAYS: &[OverlayRow]`
  - `pub(crate) fn any_active(editor: &Editor) -> bool`
- Consumes: `crate::editor::Editor` (the 11 overlay `Option` fields).

- [ ] **Step 1: Write the failing bijection + Q4 guardrail tests**

Create `wordcartel/src/overlays.rs` with ONLY a `tests` module first (the code it references doesn't exist yet, so it fails to compile = "fails"):

```rust
//! Input-overlay dispatch hub — placeholder (filled in Step 3).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    /// Enum↔table bijection + Splash-first ordering. Pins the invariants the exhaustive
    /// `row()` match cannot: order and identity across ALL / OVERLAYS.
    #[test]
    fn overlay_table_is_a_bijection_splash_first() {
        assert_eq!(OverlayId::ALL.len(), OVERLAYS.len(), "ALL and OVERLAYS same length");
        assert_eq!(OverlayId::ALL[0], OverlayId::Splash, "Splash must be row 0 (§2.6 skip + precedence)");
        for (i, id) in OverlayId::ALL.iter().enumerate() {
            assert_eq!(OVERLAYS[i].id, *id, "OVERLAYS order matches ALL at {i}");
            assert_eq!(id.row().id, *id, "row() round-trips id for {id:?}");
        }
        // names unique
        let mut names: Vec<&str> = OVERLAYS.iter().map(|r| r.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n, "overlay names are unique");
    }

    /// `any_active` is true for each overlay individually (subsumes render.rs's B11 census
    /// for the is-active axis; the full sweep lands in the sweep task).
    #[test]
    fn any_active_true_for_each_overlay() {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mk = || Editor::new_from_text("hello world\n", None, (40, 12));
        let mut e = mk(); e.open_search(crate::search_overlay::Phase::Find, 0);
        assert!(any_active(&e), "search");
        let mut e = mk(); e.open_palette();
        assert!(any_active(&e), "palette");
        let mut e = mk(); e.splash = Some(crate::splash::Splash::new(&km, "0.0.0"));
        assert!(any_active(&e), "splash");
        let e = mk();
        assert!(!any_active(&e), "no overlay ⇒ false");
    }

    /// Q4 delta (spec §3): with the splash up, a mouse-Moved must NOT arm the menu-bar or
    /// scrollbar dwell timers. `no_overlay_open` now counts splash, so `mouse::handle` routes
    /// the event to the overlay path and returns before the dwell-arming block runs.
    #[test]
    fn no_dwell_arming_while_splash_is_up() {
        use crossterm::event::{MouseEvent, MouseEventKind, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut e = Editor::new_from_text("hello\n", None, (40, 12));
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        e.scrollbar_mode = crate::config::TransientMode::Auto;
        e.splash = Some(crate::splash::Splash::new(&km, "0.0.0"));
        // A move onto row 0 (menu-bar dwell region) and the right edge (scrollbar region).
        for (col, row) in [(5u16, 0u16), (39u16, 5u16)] {
            let ev = MouseEvent { kind: MouseEventKind::Moved, column: col, row, modifiers: KeyModifiers::NONE };
            crate::mouse::handle(&mut e, ev, &reg, &km, &ex, &clock, &tx);
        }
        assert!(e.mouse.menu_reveal_due.is_none(), "no menu dwell armed under splash");
        assert!(e.mouse.scrollbar_reveal_due.is_none(), "no scrollbar dwell armed under splash");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel --lib overlays:: 2>&1 | head -30`
Expected: FAIL — compile errors (`OverlayId`, `OVERLAYS`, `any_active` not found).

- [ ] **Step 3: Write the seam (OverlayId, OVERLAYS, any_active)**

Replace the placeholder header in `wordcartel/src/overlays.rs` with the seam (keep the `mod tests` from Step 1 below it):

```rust
//! Input-overlay dispatch hub. Static fn-pointer table; one row per overlay, keyed by an
//! exhaustive `OverlayId`. Collapses the hand-parallel overlay enumerations (is-active,
//! intercept-chain, mouse, render, XOR-close) into one table + delegating folds. Extracted
//! from editor.rs/app.rs/mouse.rs/render_overlays.rs (Effort H21).
//!
//! Plugin-forward (the shape `timers.rs` reserved for plugin timers, which shipped as ONE
//! static row reading dynamic `Editor::pending_plugin_timers`): a future plugin panel is ONE
//! static `OverlayId::PluginPanel` row whose slots read dynamic `editor.plugin_panel` state —
//! content submitted edge-triggered / version-stamped / capped by the P3 pump, painted by a
//! builtin Rust painter, keys forwarded to Lua as events. The row is static; the content is
//! dynamic. No `PluginPanel` variant ships in H21 (it would be dead code and defeat the
//! exhaustiveness guarantee).
use crate::editor::Editor;

/// Every input overlay, exhaustive. A new overlay is a new variant; `row()` then forces it
/// into `OVERLAYS`, and every table-derived consumer inherits it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum OverlayId {
    Splash, Menu, Palette, ThemePicker, CursorPicker, FileBrowser,
    Prompt, Minibuffer, Search, Diag, Outline,
}

impl OverlayId {
    /// All variants, in intercept-chain order (splash first — §2.6). `Splash` MUST stay
    /// index 0: the intercept loop skips `ALL[1..]` after firing the splash row, and the
    /// paint early-return keys off it.
    pub(crate) const ALL: &'static [OverlayId] = &[
        OverlayId::Splash, OverlayId::Menu, OverlayId::Palette, OverlayId::ThemePicker,
        OverlayId::CursorPicker, OverlayId::FileBrowser, OverlayId::Prompt,
        OverlayId::Minibuffer, OverlayId::Search, OverlayId::Diag, OverlayId::Outline,
    ];

    /// The table row for this id. EXHAUSTIVE match — a new variant fails to compile until it
    /// is placed here (the guarantee that closes the silent-UI leak). The `OVERLAYS[i]`
    /// indices are pinned by the bijection test.
    pub(crate) fn row(self) -> &'static OverlayRow {
        match self {
            OverlayId::Splash       => &OVERLAYS[0],
            OverlayId::Menu         => &OVERLAYS[1],
            OverlayId::Palette      => &OVERLAYS[2],
            OverlayId::ThemePicker  => &OVERLAYS[3],
            OverlayId::CursorPicker => &OVERLAYS[4],
            OverlayId::FileBrowser  => &OVERLAYS[5],
            OverlayId::Prompt       => &OVERLAYS[6],
            OverlayId::Minibuffer   => &OVERLAYS[7],
            OverlayId::Search       => &OVERLAYS[8],
            OverlayId::Diag         => &OVERLAYS[9],
            OverlayId::Outline      => &OVERLAYS[10],
        }
    }
}

/// One overlay's routing slots. Fields grow as H21 folds each axis (is_active → intercept →
/// close → mouse → render); Task 1 introduces `is_active` only.
pub(crate) struct OverlayRow {
    /// Read only by the guardrail tests today (bijection/uniqueness) and reserved as the
    /// stable plugin identity for a future panel; unread in a non-test release build.
    #[allow(dead_code)]
    pub(crate) name: &'static str,
    /// Read only by the bijection test today; reserved plugin identity. Unread in release.
    #[allow(dead_code)]
    pub(crate) id: OverlayId,
    pub(crate) is_active: fn(&Editor) -> bool,
}

/// The overlay table, in `ALL` order. Non-capturing closures coerce to the fn-pointer fields.
pub(crate) static OVERLAYS: &[OverlayRow] = &[
    OverlayRow { name: "splash",        id: OverlayId::Splash,       is_active: |e| e.splash.is_some() },
    OverlayRow { name: "menu",          id: OverlayId::Menu,         is_active: |e| e.menu.is_some() },
    OverlayRow { name: "palette",       id: OverlayId::Palette,      is_active: |e| e.palette.is_some() },
    OverlayRow { name: "theme_picker",  id: OverlayId::ThemePicker,  is_active: |e| e.theme_picker.is_some() },
    OverlayRow { name: "cursor_picker", id: OverlayId::CursorPicker, is_active: |e| e.cursor_picker.is_some() },
    OverlayRow { name: "file_browser",  id: OverlayId::FileBrowser,  is_active: |e| e.file_browser.is_some() },
    OverlayRow { name: "prompt",        id: OverlayId::Prompt,       is_active: |e| e.prompt.is_some() },
    OverlayRow { name: "minibuffer",    id: OverlayId::Minibuffer,   is_active: |e| e.minibuffer.is_some() },
    OverlayRow { name: "search",        id: OverlayId::Search,       is_active: |e| e.search.is_some() },
    OverlayRow { name: "diag",          id: OverlayId::Diag,         is_active: |e| e.diag.is_some() },
    OverlayRow { name: "outline",       id: OverlayId::Outline,      is_active: |e| e.outline.is_some() },
];

/// True iff any input overlay owns the screen — the single source for both
/// `Editor::has_active_input_overlay` and `mouse::no_overlay_open`. Includes `splash`
/// (Q4 delta): the mouse path now treats the splash as active, so dwell timers cannot arm
/// under it.
pub(crate) fn any_active(editor: &Editor) -> bool {
    OverlayId::ALL.iter().any(|id| (id.row().is_active)(editor))
}
```

- [ ] **Step 4: Register the module + migrate both predicates**

In `wordcartel/src/lib.rs`, add after `pub mod mouse;` (keep the neighbors' style):

```rust
pub mod overlays;
```

In `wordcartel/src/editor.rs`, replace the body of `has_active_input_overlay` (keep the doc comment above it):

```rust
    pub fn has_active_input_overlay(&self) -> bool {
        crate::overlays::any_active(self)
    }
```

In `wordcartel/src/mouse.rs`, replace `no_overlay_open` (keep the doc comment):

```rust
/// True when NO overlay/modal is open — the shared predicate for dwell suppression.
/// Derived from the overlay table (H21); now counts `splash` too (Q4), so a mouse move
/// under the splash routes to the overlay path instead of arming dwell timers.
fn no_overlay_open(editor: &Editor) -> bool {
    !crate::overlays::any_active(editor)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p wordcartel --lib overlays:: 2>&1 | tail -20`
Expected: PASS (3 tests). Then the full guard:
Run: `cargo test -p wordcartel --lib 2>&1 | tail -15`
Expected: PASS. (Note: `render.rs`'s `has_active_input_overlay_true_for_every_surface` still passes — the predicate is behavior-identical for those 11 constructions; only the mouse-path `splash` treatment changed.)

- [ ] **Step 6: Clippy + build-warning gate**

Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -15`
Expected: clean (no warnings). Confirms no dead `name`/`id` field warning (the `#[allow(dead_code)]`s cover them) and no unused import.

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/overlays.rs wordcartel/src/lib.rs wordcartel/src/editor.rs wordcartel/src/mouse.rs
git commit -m "feat(overlays): OverlayId table + is-active axis; splash active on mouse path (H21 T1)"
```

---

## Task 2: Input-chain fold (H10) — `DispatchCtx` + the `intercept` column

Adds `DispatchCtx` and the `intercept` slot, migrates all 12 `intercept` fns (11 overlays + `marks`) to the shared signature, and rewrites `reduce_dispatch` to `splash-row → marks → loop ALL[1..]`, preserving the real chain order.

**Files:**
- Modify: `wordcartel/src/overlays.rs` (`DispatchCtx`, `OverlayRow.intercept`, OVERLAYS column)
- Modify: `wordcartel/src/app.rs` (`reduce_dispatch` intercept section)
- Modify: `wordcartel/src/{splash,marks,menu,palette,theme_picker,cursor_picker,file_browser,prompts,minibuffer,search_ui,diag_overlay,outline_overlay}.rs` (12 `intercept` signatures + internal `ctx.*` substitutions)
- Test (migrate the 7 direct `intercept` unit-test callers — Step 5): `splash.rs` (`run_intercept`), `minibuffer.rs` (two plugin-arg tests), `prompts.rs` (two tests), `menu.rs` (`documents_section_appears_and_switches`), `plugin/host.rs` (`param_command_callback_receives_arg`) — plus a new order pin in `overlays.rs`.

**Interfaces:**
- Produces:
  - `pub(crate) struct DispatchCtx<'a> { pub(crate) reg: &'a crate::registry::Registry, pub(crate) keymap: &'a crate::keymap::KeyTrie, pub(crate) ex: &'a dyn crate::jobs::Executor, pub(crate) clock: &'a dyn wordcartel_core::history::Clock, pub(crate) msg_tx: &'a std::sync::mpsc::Sender<crate::app::Msg> }`
  - `OverlayRow.intercept: fn(crate::app::Msg, &mut Editor, &DispatchCtx) -> crate::app::Handled`
  - Each overlay module's `pub(crate) fn intercept(msg, editor, ctx: &crate::overlays::DispatchCtx) -> Handled`.
- Consumes: `crate::overlays::{OverlayId, OVERLAYS}` (Task 1); `crate::app::{Msg, Handled, fold_and_continue}`.

- [ ] **Step 1: Write a failing order-preservation test**

Add to `wordcartel/src/overlays.rs` `mod tests`:

```rust
    /// The input fold must preserve the real chain order: splash fires BEFORE marks fires
    /// BEFORE the other overlays. With BOTH a pending mark AND the splash up, a key-Press
    /// must dismiss the SPLASH (not resolve the mark) — proving splash still precedes marks.
    #[test]
    fn splash_intercept_precedes_marks() {
        use crossterm::event::{KeyEvent, KeyCode, KeyEventKind, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut e = Editor::new_from_text("hello\n", None, (40, 12));
        e.splash = Some(crate::splash::Splash::new(&km, "0.0.0"));
        e.pending_mark = Some(crate::editor::MarkPending::Set);
        let key = KeyEvent { code: KeyCode::Char('a'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: crossterm::event::KeyEventState::NONE };
        crate::app::reduce(crate::app::Msg::Input(crossterm::event::Event::Key(key)),
            &mut e, &reg, &km, &ex, &clock, &tx);
        assert!(e.splash.is_none(), "splash dismissed first");
        assert!(e.pending_mark.is_some(), "the mark was NOT consumed — splash preceded marks");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel --lib overlays::tests::splash_intercept_precedes_marks 2>&1 | tail -20`
Expected: it may PASS already (today's order is splash→marks). Run it to confirm GREEN baseline — this test guards that the Step 4 rewrite does NOT invert the order. If it does not compile yet (it should — all symbols exist), fix imports. Treat this as a regression guard, not red-first.

- [ ] **Step 3: Add `DispatchCtx` + the `intercept` column to `overlays.rs`**

Add `DispatchCtx` after the `use crate::editor::Editor;` line:

```rust
use crate::app::{Msg, Handled};

/// The non-editor dispatch context, bundled so every overlay `intercept` (and later `mouse`)
/// fn shares ONE signature. The editor is passed SEPARATELY as `&mut Editor` — deliberately
/// EXCLUDED here to avoid a `&mut` aliasing tangle in the table loop (contrast
/// `registry::Ctx`, which OWNS `editor: &mut Editor` and holds `msg_tx` by VALUE for a
/// `'static` spawned thread; `DispatchCtx` borrows `msg_tx` — it never outlives the loop).
pub(crate) struct DispatchCtx<'a> {
    pub(crate) reg: &'a crate::registry::Registry,
    pub(crate) keymap: &'a crate::keymap::KeyTrie,
    pub(crate) ex: &'a dyn crate::jobs::Executor,
    pub(crate) clock: &'a dyn wordcartel_core::history::Clock,
    pub(crate) msg_tx: &'a std::sync::mpsc::Sender<Msg>,
}
```

Widen `OverlayRow` — add the `intercept` field after `is_active`:

```rust
    pub(crate) is_active: fn(&Editor) -> bool,
    pub(crate) intercept: fn(Msg, &mut Editor, &DispatchCtx) -> Handled,
```

Add `intercept:` to each of the 11 `OVERLAYS` rows (the module `intercept` fns, migrated in Step 4). The exact per-row value:

| row | `intercept:` value |
|---|---|
| splash | `crate::splash::intercept` |
| menu | `crate::menu::intercept` |
| palette | `crate::palette::intercept` |
| theme_picker | `crate::theme_picker::intercept` |
| cursor_picker | `crate::cursor_picker::intercept` |
| file_browser | `crate::file_browser::intercept` |
| prompt | `crate::prompts::intercept` |
| minibuffer | `crate::minibuffer::intercept` |
| search | `crate::search_ui::intercept` |
| diag | `crate::diag_overlay::intercept` |
| outline | `crate::outline_overlay::intercept` |

Example (splash row) — apply the same `intercept:` addition to all 11:

```rust
    OverlayRow { name: "splash", id: OverlayId::Splash, is_active: |e| e.splash.is_some(),
        intercept: crate::splash::intercept },
```

- [ ] **Step 4: Migrate the 12 intercept signatures + internal `ctx.*` substitutions**

Each `intercept` changes from `(msg, editor, ex, clock, msg_tx)` (or the 7-arg menu/palette form) to `(msg, editor, ctx: &crate::overlays::DispatchCtx)`, and every internal use of a context param becomes `ctx.<field>`. The migration is mechanical per file:

**`splash.rs`** — new signature (body unchanged; it used none of the params, they were `_ex/_clock/_msg_tx`):
```rust
pub(crate) fn intercept(msg: Msg, editor: &mut crate::editor::Editor,
    _ctx: &crate::overlays::DispatchCtx) -> Handled {
```

**`marks.rs`** — signature + the single `fold_and_continue` call:
```rust
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
```
and inside, `crate::app::fold_and_continue(editor, ex, clock, msg_tx)` → `crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx)`.

**`diag_overlay.rs`** — signature + `diag_apply_selected(editor, clock)` → `…(editor, ctx.clock)` + `fold_and_continue(editor, ex, clock, msg_tx)` → `ctx.*`:
```rust
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
```
Substitutions: `crate::search_ui::diag_apply_selected(editor, clock)` → `crate::search_ui::diag_apply_selected(editor, ctx.clock)`; `crate::app::fold_and_continue(editor, ex, clock, msg_tx)` → `crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx)`.

**`theme_picker.rs`, `cursor_picker.rs`, `outline_overlay.rs`** — signature (below) + every `crate::app::fold_and_continue(editor, ex, clock, msg_tx)` → `…(editor, ctx.ex, ctx.clock, ctx.msg_tx)`. These use no other context params.
```rust
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
```
(cursor_picker's signature uses the bare `Msg`/`Handled` aliases — keep them: `pub(crate) fn intercept(msg: Msg, editor: &mut crate::editor::Editor, ctx: &crate::overlays::DispatchCtx) -> Handled {`.)

**`minibuffer.rs`** — signature + the `Enter` arm's `save_as_submit(editor, &mb.text, ex, clock, msg_tx)` → `…(editor, &mb.text, ctx.ex, ctx.clock, ctx.msg_tx)`, `submit_filter_line(editor, &mb.text, msg_tx)` → `…(editor, &mb.text, ctx.msg_tx)`, and the tail `fold_and_continue(editor, ex, clock, msg_tx)` → `ctx.*`:
```rust
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
```

**`search_ui.rs`, `file_browser.rs`, `prompts.rs`** — signature (below); substitute every `ex`→`ctx.ex`, `clock`→`ctx.clock`, `msg_tx`→`ctx.msg_tx` at each call site in the body (they thread these into their helpers; `grep -n "\bex\b\|\bclock\b\|\bmsg_tx\b" <file>` inside the intercept fn to enumerate — replace each with `ctx.<field>`):
```rust
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
```

**`menu.rs`** — the 7-arg form collapses to 3; `reg`/`keymap` become `ctx.reg`/`ctx.keymap`:
```rust
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
```
Substitute in the body: `reg`→`ctx.reg`, `keymap`→`ctx.keymap`, `ex`→`ctx.ex`, `clock`→`ctx.clock`, `msg_tx`→`ctx.msg_tx` (e.g. `dispatch_row_action(editor, reg, keymap, ex, clock, msg_tx, action)` → `dispatch_row_action(editor, ctx.reg, ctx.keymap, ctx.ex, ctx.clock, ctx.msg_tx, action)`).

**`palette.rs`** — same 7→3 collapse; `reg`→`ctx.reg`, `keymap`→`ctx.keymap`, `ex`→`ctx.ex`, `clock`→`ctx.clock`, `msg_tx`→`ctx.msg_tx` (e.g. `rebuild_rows(p, reg, keymap)` → `rebuild_rows(p, ctx.reg, ctx.keymap)`; `dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, row.id)` → `…(editor, ctx.reg, ctx.keymap, ctx.ex, ctx.clock, ctx.msg_tx, row.id)`):
```rust
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
```

- [ ] **Step 5: Migrate the 7 existing intercept unit-test call sites**

The signature change breaks every DIRECT `intercept(...)` caller in the test modules — they still pass the old `ex/clock/msg_tx` (or 7-arg `reg/keymap/…`) args. An exhaustive tree-wide grep (`grep -rn "intercept(" wordcartel/src`, including under `plugin/`) confirms exactly **seven** call sites in **six** modules (`file_browser.rs` and the other overlay modules have only the definition, no test caller). Each must construct a `DispatchCtx` and call `intercept(msg, &mut editor, &ctx)`. The five-arg callers (all except `menu.rs`) additionally need a `reg` + `keymap` for the ctx — `minibuffer`/`splash`/`prompts` intercepts don't READ `reg`/`keymap`, but the struct still requires them, so build a throwaway `Registry::builtins()` + keymap where the test lacks one.

**`splash.rs`** — the `run_intercept` test helper (currently `intercept(msg, e, &ex, &clk, &tx)`). Replace its body:

```rust
    fn run_intercept(msg: Msg, e: &mut Editor) -> Handled {
        let ex = InlineExecutor::default();
        let clk = TestClock::new(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: &tx };
        intercept(msg, e, &ctx)
    }
```

**`minibuffer.rs`** — the two plugin-arg tests (`plugin_arg_submit_enqueues_call_with_arg`, `plugin_arg_over_cap_is_rejected_at_submit`), each currently `intercept(msg, &mut editor, &ex, &clock, &tx)`. In each, after the existing `let ex = …; let clock = …; let (tx, _rx) = …;` add a registry + keymap + ctx and switch the call:

```rust
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clock, msg_tx: &tx };
        let msg = crate::app::Msg::Input(Event::Key(enter_key()));
        intercept(msg, &mut editor, &ctx);
```

**`prompts.rs`** — two callers. In `intercept_delivers_diag_provider_event_under_a_modal` (after `let ex = InlineExecutor::default(); let clk = TestClock(0); let (tx, _rx) = …;`):

```rust
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: &tx };
        let handled = intercept(Msg::DiagProviderEvent { source: wordcartel_core::diagnostics::DiagSource::Harper,
            event: ProviderEvent::Degraded(INSTALL_HINT.into()) }, &mut e, &ctx);
```

And in the Esc test (currently `let (ex, clk, tx) = (InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);` then `intercept(Msg::Input(esc), &mut e, &ex, &clk, &tx);`):

```rust
        let (ex, clk, tx) = (InlineExecutor::default(), TestClock(0), std::sync::mpsc::channel().0);
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: &tx };
        let esc = Event::Key(KeyEvent {
            code: KeyCode::Esc, modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        intercept(Msg::Input(esc), &mut e, &ctx);
```

**`menu.rs`** — `documents_section_appears_and_switches` (already has `reg`, `km`, `ex`, `clk`, `tx`; currently `intercept(msg, &mut ed, &reg, &km, &ex, &clk, &tx)`):

```rust
        let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: &tx };
        let _ = intercept(crate::app::Msg::Input(enter_key()), &mut ed, &ctx);
```

**`plugin/host.rs`** — `param_command_callback_receives_arg` (the 7th caller; a `crate::minibuffer::intercept` call inside a `{ let mut e = editor.borrow_mut(); … }` block, currently `crate::minibuffer::intercept(msg, &mut e, &ex, &clock, &tx)`). The test already has `reg` (a `mut Registry::builtins()`), `ex`, `clock`, `tx` in scope but NO keymap — build one from `reg`, then construct the ctx inside the block (its `&reg` borrow ends with the block, before the later `host.pump(&editor, &reg, …)` reuses `reg`):

```rust
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let msg = crate::app::Msg::Input(crossterm::event::Event::Key(enter));
        {
            let mut e = editor.borrow_mut();
            let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clock, msg_tx: &tx };
            crate::minibuffer::intercept(msg, &mut e, &ctx);
        }
```

- [ ] **Step 6: Rewrite `reduce_dispatch`'s intercept section**

In `wordcartel/src/app.rs`, replace the 12 `let msg = match crate::X::intercept(...) { … };` lines (the block from `crate::splash::intercept` through `crate::outline_overlay::intercept`) with:

```rust
    // Overlay/modal input dispatch (H21). Real chain order: splash row FIRST, then the
    // `marks` chord pre-stage, then the remaining overlay rows in ALL order. `marks` is NOT
    // a table row (chord state, no overlay struct) — it sits between the splash row and the
    // rest to preserve today's `splash → marks → others` precedence (spec §2.6).
    let ctx = crate::overlays::DispatchCtx { reg, keymap, ex, clock, msg_tx };
    let mut msg = msg;
    msg = match (crate::overlays::OverlayId::Splash.row().intercept)(msg, editor, &ctx) {
        crate::app::Handled::Done(k) => return k, crate::app::Handled::Pass(m) => m };
    msg = match crate::marks::intercept(msg, editor, &ctx) {
        crate::app::Handled::Done(k) => return k, crate::app::Handled::Pass(m) => m };
    for id in &crate::overlays::OverlayId::ALL[1..] {
        msg = match (id.row().intercept)(msg, editor, &ctx) {
            crate::app::Handled::Done(k) => return k,
            crate::app::Handled::Pass(m) => m,
        };
    }
```

(The subsequent `let before = editor.active().document.version;` line and the `match msg { … }` tail are unchanged; they use `reg/keymap/ex/clock/msg_tx` directly, whose immutable borrows via `ctx` end at the loop.)

- [ ] **Step 7: Run tests to verify green**

Run: `cargo test -p wordcartel --lib 2>&1 | tail -20`
Expected: PASS — including `overlays::tests::splash_intercept_precedes_marks`, the app.rs interceptor tests (`*_interceptor Handled::Done path*`), and the migrated splash/prompts/menu/minibuffer intercept unit tests (Step 5).

- [ ] **Step 8: Clippy + budget gate**

Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -15 && cargo test -p wordcartel --test module_budgets 2>&1 | tail -8`
Expected: clippy clean; `app_rs_stays_a_thin_dispatch_hub` passes (reduce_dispatch SHRANK).

- [ ] **Step 9: Commit**

```bash
git add wordcartel/src/overlays.rs wordcartel/src/app.rs wordcartel/src/splash.rs wordcartel/src/marks.rs wordcartel/src/menu.rs wordcartel/src/palette.rs wordcartel/src/theme_picker.rs wordcartel/src/cursor_picker.rs wordcartel/src/file_browser.rs wordcartel/src/prompts.rs wordcartel/src/minibuffer.rs wordcartel/src/search_ui.rs wordcartel/src/diag_overlay.rs wordcartel/src/outline_overlay.rs wordcartel/src/plugin/host.rs
git commit -m "refactor(overlays): fold the 12-stage intercept chain into the table (H10; H21 T2)"
```

---

## Task 3: Close/XOR fold — the `close` column + `close_all`

Adds the `close` slot and `close_all`, then migrates the sibling-null lists in every `open_*`, in `dispatch_overlay_command`, and in the registry `"menu"` command. (Done before the mouse fold because `route_overlay`'s Down-left close arms in Task 4 depend on `close_all`.)

**Files:**
- Modify: `wordcartel/src/overlays.rs` (`OverlayRow.close`, OVERLAYS column, `close_all`)
- Modify: `wordcartel/src/editor.rs` (the 10 `open_*` methods)
- Modify: `wordcartel/src/app.rs` (`dispatch_overlay_command`)
- Modify: `wordcartel/src/registry.rs` (the `"menu"` command)
- Test: `wordcartel/src/overlays.rs` (`close_all` clears all)

**Interfaces:**
- Produces:
  - `OverlayRow.close: fn(&mut Editor)`
  - `pub(crate) fn close_all(editor: &mut Editor)`
- Consumes: `crate::overlays::{OVERLAYS}` (Task 1).

- [ ] **Step 1: Write the failing `close_all` test**

Add to `wordcartel/src/overlays.rs` `mod tests`:

```rust
    /// `close_all` clears EVERY overlay (the XOR-close axis). Opens all 11 in turn (each via a
    /// real `open_*` or field set), asserts it was active, then asserts `close_all` clears it.
    #[test]
    fn close_all_clears_every_overlay() {
        let diag_fixture = || wordcartel_core::diagnostics::Diagnostic {
            range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "m".into(), suggestions: Vec::new(),
        };
        let openers: Vec<(&str, Box<dyn Fn(&mut Editor)>)> = vec![
            ("search",        Box::new(|e: &mut Editor| e.open_search(crate::search_overlay::Phase::Find, 0))),
            ("minibuffer",    Box::new(|e: &mut Editor| e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter))),
            ("palette",       Box::new(|e: &mut Editor| e.open_palette())),
            ("outline",       Box::new(|e: &mut Editor| e.open_outline())),
            ("theme_picker",  Box::new(|e: &mut Editor| e.open_theme_picker())),
            ("file_browser",  Box::new(|e: &mut Editor| e.open_file_browser(std::path::PathBuf::from(".")))),
            ("prompt",        Box::new(|e: &mut Editor| e.open_prompt(crate::prompt::Prompt::swap_recovery()))),
            ("cursor_picker", Box::new(|e: &mut Editor| e.open_cursor_picker())),
            ("diag",          Box::new(move |e: &mut Editor| e.open_diag(diag_fixture()))),
            ("menu",          Box::new(|e: &mut Editor| { e.menu = Some(crate::menu::empty()); })),
            ("splash",        Box::new(|e: &mut Editor| { e.splash = Some(crate::splash::Splash::new(
                &crate::keymap::KeyTrie::default(), "0.0.0")); })),
        ];
        for (name, open) in openers {
            let mut e = Editor::new_from_text("x\n", None, (40, 12));
            crate::derive::rebuild(&mut e);
            open(&mut e);
            assert!(any_active(&e), "{name}: precondition — overlay open");
            close_all(&mut e);
            assert!(!any_active(&e), "{name}: close_all cleared it");
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel --lib overlays::tests::close_all_clears_every_overlay 2>&1 | tail -15`
Expected: FAIL — `close_all` not found.

- [ ] **Step 3: Add the `close` column + `close_all`**

Widen `OverlayRow` — add after `intercept`:

```rust
    pub(crate) close: fn(&mut Editor),
```

Add `close:` to each of the 11 `OVERLAYS` rows (each nulls its own field). Values:

| row | `close:` value |
|---|---|
| splash | `\|e\| e.splash = None` |
| menu | `\|e\| e.menu = None` |
| palette | `\|e\| e.palette = None` |
| theme_picker | `\|e\| e.theme_picker = None` |
| cursor_picker | `\|e\| e.cursor_picker = None` |
| file_browser | `\|e\| e.file_browser = None` |
| prompt | `\|e\| e.prompt = None` |
| minibuffer | `\|e\| e.minibuffer = None` |
| search | `\|e\| e.search = None` |
| diag | `\|e\| e.diag = None` |
| outline | `\|e\| e.outline = None` |

Example (splash row, now with all four columns):

```rust
    OverlayRow { name: "splash", id: OverlayId::Splash, is_active: |e| e.splash.is_some(),
        intercept: crate::splash::intercept, close: |e| e.splash = None },
```

Add `close_all` after `any_active`:

```rust
/// Close every overlay (hold the single-overlay XOR invariant). Replaces the sibling-null
/// lists in every `open_*`, in `dispatch_overlay_command`, in the registry `"menu"` command,
/// and (Task 4) `route_overlay`'s Down-left close arms. NOT the `save.rs` post-buffer-replace
/// stale-clears — those clear only `search`/`diag` for content staleness, not the XOR set.
pub(crate) fn close_all(editor: &mut Editor) {
    for row in OVERLAYS { (row.close)(editor); }
}
```

- [ ] **Step 4: Migrate the 10 `open_*` methods (`editor.rs`)**

In each `open_*`, replace the run of `self.<overlay> = None;` sibling-null lines with a single `crate::overlays::close_all(self);`, KEEPING the `self.pending_keys.clear();` and `self.pending_mark = None;` lines (not overlay fields) and the final `self.<own> = Some(...)` construction. The `debug_assert!` in `open_minibuffer` stays. Concretely, each method's null-run becomes:

```rust
        crate::overlays::close_all(self);
        self.pending_keys.clear();
        self.pending_mark = None;
```

followed by the method's own `Some(...)` assignment (and any post-build call like `rebuild_rows`/`rebuild_entries`). Apply to: `open_minibuffer`, `open_prompt`, `open_palette`, `open_search`, `open_diag`, `open_outline`, `open_theme_picker`, `open_buffer_switcher`, `open_file_browser`, `open_cursor_picker`. (Net effect is identical for the 10 input overlays; for `splash`, `close_all` additionally nulls it — a benign superset, since splash is startup-only and already dismissed before any `open_*` runs. `open_prompt`'s explicit `self.splash = None;` line is now redundant and is removed — `close_all` covers it.)

- [ ] **Step 5: Migrate `dispatch_overlay_command` (`app.rs`)**

Replace its 5 explicit nulls:

```rust
    editor.palette = None;
    editor.menu = None;
    editor.theme_picker = None;
    editor.cursor_picker = None;
    editor.file_browser = None;
```

with:

```rust
    crate::overlays::close_all(editor);
```

(Safe widening: XOR guarantees the other overlays are already `None` at this call site.)

- [ ] **Step 6: Migrate the registry `"menu"` command (`registry.rs`) — preserve the toggle**

Replace the 9 sibling nulls + toggle. The command currently nulls 9 fields, clears `pending_keys`/`pending_mark`, then toggles `menu`. Because `close_all` also nulls `menu`, capture the pre-state first:

```rust
        r.register("menu", "Menu Bar", None, |c| {
            let was_open = c.editor.menu.is_some();
            crate::overlays::close_all(c.editor);
            c.editor.pending_keys.clear();
            c.editor.pending_mark = None;
            c.editor.menu = if was_open { None } else { Some(crate::menu::empty()) };
            CommandResult::Handled
```

(keep the rest of the closure/registration unchanged after `CommandResult::Handled`).

- [ ] **Step 7: Run tests + gates**

Run: `cargo test -p wordcartel --lib 2>&1 | tail -20`
Expected: PASS — `close_all_clears_every_overlay`, `open_search_clears_siblings_and_open_others_clear_search`, `open_prompt_clears_any_pending_splash`, `open_buffer_switcher_yields_buffers_kind_palette`, and the menu-command tests all green.
Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -10`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add wordcartel/src/overlays.rs wordcartel/src/editor.rs wordcartel/src/app.rs wordcartel/src/registry.rs
git commit -m "refactor(overlays): fold the XOR-close lists into close_all (H21 T3)"
```

---

## Task 4: Mouse fold — the `mouse` column + `route_overlay` collapse + splash slot

Adds the `mouse` slot, extracts each `route_overlay` branch into a per-overlay `mouse_*` fn, adds a splash mouse slot (consumes all mouse events), migrates the palette/menu Down-left close arms to `close_all` (guards preserved verbatim), and collapses `route_overlay` to find-active + one call.

**Files:**
- Modify: `wordcartel/src/overlays.rs` (`OverlayRow.mouse`, OVERLAYS column)
- Modify: `wordcartel/src/mouse.rs` (`route_overlay` → dispatcher; new `mouse_*` fns; `handle` builds `DispatchCtx`)
- Modify: `wordcartel/src/splash.rs` (new `pub(crate) fn mouse` slot)
- Test: `wordcartel/src/overlays.rs` (click-consumed under each overlay — partial; full sweep in Task 6)

**Interfaces:**
- Produces:
  - `OverlayRow.mouse: fn(&mut Editor, crossterm::event::MouseEvent, ratatui::layout::Rect, &DispatchCtx)`
  - `wordcartel/src/mouse.rs`: `fn mouse_palette/mouse_menu/mouse_theme_picker/mouse_cursor_picker/mouse_file_browser/mouse_outline/mouse_diag/mouse_prompt/mouse_minibuffer/mouse_search(&mut Editor, MouseEvent, Rect, &DispatchCtx)`.
  - `wordcartel/src/splash.rs`: `pub(crate) fn mouse(editor: &mut Editor, ev: MouseEvent, _area: Rect, _ctx: &DispatchCtx)`.
- Consumes: `crate::overlays::{OverlayId, OVERLAYS, DispatchCtx, close_all}` (Tasks 1–3).

- [ ] **Step 1: Write a failing "click consumed under overlay" test**

Add to `wordcartel/src/overlays.rs` `mod tests`:

```rust
    /// A mouse Down under an open list overlay is consumed by the overlay's mouse slot —
    /// it must NOT fall through to an editor gesture (no click-through while a modal is up).
    #[test]
    fn click_under_overlay_does_not_move_caret() {
        use crossterm::event::{MouseEvent, MouseEventKind, MouseButton, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut e = Editor::new_from_text("hello world\ntwo\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.open_palette();
        let before = crate::nav::head(&e);
        // A click well outside the palette rect (bottom-left) — with no palette open this
        // would move the caret; under the palette it is consumed (close-away or no-op).
        let ev = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 0, row: 11,
            modifiers: KeyModifiers::NONE };
        crate::mouse::handle(&mut e, ev, &reg, &km, &ex, &clock, &tx);
        // Either the palette closed (click-away) or stayed; in NEITHER case did the editor
        // caret jump to the clicked cell — the event never reached the editor gesture path.
        assert_eq!(crate::nav::head(&e), before, "click under palette did not move the caret");
    }
```

- [ ] **Step 2: Run to verify current behavior (baseline green)**

Run: `cargo test -p wordcartel --lib overlays::tests::click_under_overlay_does_not_move_caret 2>&1 | tail -15`
Expected: PASS today (route_overlay already consumes). This is a regression guard for the refactor. If it does not compile, fix imports; do not proceed until it is a GREEN baseline.

- [ ] **Step 3: Add the splash mouse slot (`splash.rs`)**

Add after `splash::intercept` (imports `MouseEvent`, `Rect` as needed — `use ratatui::layout::Rect;` and `crossterm::event::MouseEvent` if not present):

```rust
/// Splash mouse slot (H21 Q4). While the splash owns the screen it consumes ALL mouse events
/// so nothing leaks to dwell timers or the editor. A `Down` dismisses (mirrors `intercept`'s
/// Down arm); moves/scroll are swallowed no-ops. In practice `Down` is already consumed by
/// `intercept` before `mouse::handle` runs, so this slot mainly swallows Moved/Scroll — but it
/// dismisses defensively to match the intercept semantics.
pub(crate) fn mouse(editor: &mut crate::editor::Editor, ev: crossterm::event::MouseEvent,
    _area: ratatui::layout::Rect, _ctx: &crate::overlays::DispatchCtx) {
    if matches!(ev.kind, crossterm::event::MouseEventKind::Down(_)) {
        editor.splash = None;
    }
}
```

- [ ] **Step 4: Extract each `route_overlay` branch into a `mouse_*` fn (`mouse.rs`)**

For each overlay, move the BODY of its `if editor.X.is_some() { … }` branch (verbatim) into a free fn, dropping the outer `if editor.X.is_some()` guard (the dispatcher already selected this overlay) and turning the branch's trailing `return;` into the fn's natural end (interior `return;`s stay). Each fn has the signature `fn mouse_X(editor: &mut crate::editor::Editor, ev: crossterm::event::MouseEvent, area: ratatui::layout::Rect, ctx: &crate::overlays::DispatchCtx)`. Apply the `ctx.*` substitutions where the body used `reg/keymap/ex/clock/msg_tx`:

- `mouse_palette` — from the `if editor.palette.is_some()` block. Substitute `dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id)` → `crate::app::dispatch_overlay_command(editor, ctx.reg, ctx.keymap, ctx.ex, ctx.clock, ctx.msg_tx, id)`. **Close-arm migration:** the `else if !inside { editor.palette = None; editor.search = None; editor.diag = None; }` arm becomes `else if !inside { crate::overlays::close_all(editor); }` — the `hit == None && !inside` guard is preserved verbatim.
- `mouse_menu` — from `if editor.menu.is_some()`. Substitute `dispatch_row_action(editor, reg, keymap, ex, clock, msg_tx, action)` → `crate::menu::dispatch_row_action(editor, ctx.reg, ctx.keymap, ctx.ex, ctx.clock, ctx.msg_tx, action)`. **Close-arm migration:** the final `else { editor.menu = None; editor.search = None; editor.diag = None; }` becomes `else { crate::overlays::close_all(editor); }` — the `bar_hit == None && row_action == None` guard is preserved verbatim (this fires for non-action cells INSIDE the dropdown too, e.g. the overflow indicator; do NOT geometry-gate it).
- `mouse_theme_picker` — from `if editor.theme_picker.is_some()` (this branch lives above line where earlier reads showed cursor_picker; use `grep -n "if editor.theme_picker.is_some" mouse.rs` to locate). Uses no ctx params → `_ctx`.
- `mouse_cursor_picker` — from `if editor.cursor_picker.is_some()`. No ctx params → `_ctx`.
- `mouse_file_browser` — from `if editor.file_browser.is_some()`. No ctx params → `_ctx`.
- `mouse_outline` — from `if editor.outline.is_some()`. No ctx params → `_ctx`.
- `mouse_diag` — from `if editor.diag.is_some()`. Substitute `diag_apply_selected(editor, clock)` → `crate::search_ui::diag_apply_selected(editor, ctx.clock)`.
- `mouse_prompt` — from `if editor.prompt.is_some()`. Substitute `resolve_prompt(action, editor, ex, clock, msg_tx)` → `crate::prompts::resolve_prompt(action, editor, ctx.ex, ctx.clock, ctx.msg_tx)`.
- `mouse_minibuffer` — from `if editor.minibuffer.is_some()`. No ctx params → `_ctx`.
- `mouse_search` — from `if editor.search.is_some()` (the tail block; it has no trailing `return`). No ctx params → `_ctx`.

Each extracted fn keeps a `#[allow(clippy::too_many_lines)]` ONLY if its own body exceeds 100 lines (menu/palette may; add the attribute with a one-line reason like `// overlay mouse routing — one region per branch` if clippy flags it).

- [ ] **Step 5: Collapse `route_overlay` to a dispatcher**

Replace the entire old `route_overlay` body (the 10 `if editor.X.is_some()` branches) with:

```rust
/// Route a mouse event to the active overlay's mouse slot. PRECONDITION: an overlay is open
/// (`!no_overlay_open`). Consumes the event (the caller returns unconditionally after this).
fn route_overlay(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
                 ctx: &crate::overlays::DispatchCtx) {
    if let Some(id) = crate::overlays::OverlayId::ALL.iter()
        .find(|id| (id.row().is_active)(editor))
    {
        (id.row().mouse)(editor, ev, area, ctx);
    }
}
```

Update its ONE caller in `mouse::handle` — build a `DispatchCtx` and pass it. Replace the call site:

```rust
    if !no_overlay_open(editor) {
        let ctx = crate::overlays::DispatchCtx { reg, keymap, ex, clock, msg_tx };
        route_overlay(editor, ev, area, &ctx);
        return;
    }
```

(`handle` keeps its `reg, keymap, ex, clock, msg_tx` params — the dwell/gesture code below still uses `clock`; the `ctx` immutable borrows end when `route_overlay` returns.)

Add the `mouse` column to `overlays.rs` `OverlayRow` (after `close`) and to each OVERLAYS row:

```rust
    pub(crate) mouse: fn(&mut Editor, crossterm::event::MouseEvent, ratatui::layout::Rect, &DispatchCtx),
```

| row | `mouse:` value |
|---|---|
| splash | `crate::splash::mouse` |
| menu | `crate::mouse::mouse_menu` |
| palette | `crate::mouse::mouse_palette` |
| theme_picker | `crate::mouse::mouse_theme_picker` |
| cursor_picker | `crate::mouse::mouse_cursor_picker` |
| file_browser | `crate::mouse::mouse_file_browser` |
| prompt | `crate::mouse::mouse_prompt` |
| minibuffer | `crate::mouse::mouse_minibuffer` |
| search | `crate::mouse::mouse_search` |
| diag | `crate::mouse::mouse_diag` |
| outline | `crate::mouse::mouse_outline` |

The `mouse_*` fns must be visible to `overlays.rs` — make each `pub(crate) fn mouse_X(...)` in `mouse.rs`. Add `crossterm::event::MouseEvent` / `ratatui::layout::Rect` imports to `overlays.rs` as needed.

- [ ] **Step 6: Run tests + gates**

Run: `cargo test -p wordcartel --lib 2>&1 | tail -25`
Expected: PASS — `click_under_overlay_does_not_move_caret`, the mouse.rs overlay-click tests (palette/menu/theme_picker/cursor_picker/file_browser/outline/diag/prompt/minibuffer/search click + click-away tests), and `no_dwell_arming_while_splash_is_up` all green.
Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -15`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/overlays.rs wordcartel/src/mouse.rs wordcartel/src/splash.rs
git commit -m "refactor(overlays): fold route_overlay into the table + splash mouse slot (H21 T4)"
```

---

## Task 5: Render fold — `RenderSite` + `RENDER_ORDER` + `paint_*` extraction

Adds the `render` slot and the `RENDER_ORDER` permutation (distinct from `OVERLAYS`/intercept order), extracts each `render_overlays::paint` overlay block into a `paint_*` fn, and rewrites `paint` to splash-early-return + a `RENDER_ORDER` walk. The always-on menu **bar** chrome stays a standalone step at the `Menu` slot (spec §2.3.1/§2.3.2).

**Files:**
- Modify: `wordcartel/src/overlays.rs` (`RenderSite`, `OverlayRow.render`, OVERLAYS column, `RENDER_ORDER`)
- Modify: `wordcartel/src/render_overlays.rs` (`paint` → dispatcher; new `paint_*` fns; menu bar/dropdown split)
- Test: `wordcartel/src/overlays.rs` (render-coverage assertion)

**Interfaces:**
- Produces:
  - `pub(crate) enum RenderSite { Frame(fn(&mut ratatui::Frame, &mut Editor, &crate::render::ChromeStyles)), StatusRow }`
  - `OverlayRow.render: RenderSite`
  - `pub(crate) static RENDER_ORDER: &[OverlayId]` (the 8 Frame-site ids in paint order).
  - `wordcartel/src/render_overlays.rs`: `pub(crate) fn paint_splash/paint_palette/paint_outline/paint_theme_picker/paint_cursor_picker/paint_file_browser/paint_menu_dropdown/paint_diag(frame, editor, cs)` (the `Frame`-site painters) + `pub(crate) fn paint_menu_bar(frame, editor, cs)` (standalone bar chrome, out of the table).
- Consumes: `crate::overlays::{OverlayId, RenderSite}`.

- [ ] **Step 1: Write the failing render-coverage test**

Add to `wordcartel/src/overlays.rs` `mod tests`:

```rust
    /// Render-axis coverage: every OverlayId has a RenderSite (exhaustive by `row()`), and
    /// RENDER_ORDER contains EXACTLY the ids whose RenderSite is Frame — Splash first, no
    /// StatusRow overlay in the paint walk, no Frame overlay missing from it.
    #[test]
    fn render_order_is_exactly_the_frame_overlays() {
        assert_eq!(RENDER_ORDER[0], OverlayId::Splash, "paint early-return keys off Splash first");
        let frame_ids: Vec<OverlayId> = OverlayId::ALL.iter().copied()
            .filter(|id| matches!(id.row().render, RenderSite::Frame(_)))
            .collect();
        let mut walk = RENDER_ORDER.to_vec();
        let mut frame_sorted = frame_ids.clone();
        walk.sort_by_key(|id| format!("{id:?}"));
        frame_sorted.sort_by_key(|id| format!("{id:?}"));
        assert_eq!(walk, frame_sorted, "RENDER_ORDER == the set of Frame-site overlays");
        // The StatusRow trio must NOT appear in the paint walk.
        for id in [OverlayId::Prompt, OverlayId::Minibuffer, OverlayId::Search] {
            assert!(matches!(id.row().render, RenderSite::StatusRow), "{id:?} is StatusRow");
            assert!(!RENDER_ORDER.contains(&id), "{id:?} not in RENDER_ORDER");
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel --lib overlays::tests::render_order_is_exactly_the_frame_overlays 2>&1 | tail -15`
Expected: FAIL — `RenderSite`, `RENDER_ORDER` not found.

- [ ] **Step 3: Add `RenderSite`, the `render` column, and `RENDER_ORDER`**

Add to `overlays.rs`:

```rust
/// Where an overlay paints. Every OverlayId answers this axis (the render-coverage test
/// asserts it) WITHOUT forcing false uniformity: frame-owned surfaces carry a painter fn; the
/// status-row trio carry a marker (their painting stays in render.rs, untouched).
/// `Copy` so the render loop / coverage test can read `id.row().render` out of the
/// `&'static OverlayRow` by value (both variants — a fn pointer and a unit — are Copy).
#[derive(Clone, Copy)]
pub(crate) enum RenderSite {
    /// Painted by `render_overlays`. The paint SEQUENCE is `RENDER_ORDER` — a permutation
    /// distinct from OVERLAYS/intercept order (§2.3.2).
    Frame(fn(&mut ratatui::Frame, &mut Editor, &crate::render::ChromeStyles)),
    /// Painted on the shared status row inside `render.rs` (search bar / minibuffer / prompt).
    /// NOT relocated by H21 — the marker exists only so the axis is exhaustive (absent from
    /// RENDER_ORDER, which covers only the Frame overlays).
    StatusRow,
}

/// Frame-paint order — a permutation over the Frame-site overlays ONLY (the StatusRow trio
/// are absent; they paint in render.rs). DISTINCT from OVERLAYS/intercept order. Grounded
/// verbatim against `render_overlays::paint`'s block sequence: splash, palette, outline,
/// theme_picker, cursor_picker, file_browser, menu DROPDOWN, diag. (The always-on menu BAR
/// chrome is NOT in this walk — it is painted by a standalone step pinned at the `Menu` slot;
/// only the dropdown is the `Menu` row's Frame painter — spec §2.3.1/§2.3.2.)
pub(crate) static RENDER_ORDER: &[OverlayId] = &[
    OverlayId::Splash, OverlayId::Palette, OverlayId::Outline, OverlayId::ThemePicker,
    OverlayId::CursorPicker, OverlayId::FileBrowser, OverlayId::Menu, OverlayId::Diag,
];
```

Widen `OverlayRow` — add after `mouse`:

```rust
    pub(crate) render: RenderSite,
```

Add `render:` to each OVERLAYS row:

| row | `render:` value |
|---|---|
| splash | `RenderSite::Frame(crate::render_overlays::paint_splash)` |
| menu | `RenderSite::Frame(crate::render_overlays::paint_menu_dropdown)` |
| palette | `RenderSite::Frame(crate::render_overlays::paint_palette)` |
| theme_picker | `RenderSite::Frame(crate::render_overlays::paint_theme_picker)` |
| cursor_picker | `RenderSite::Frame(crate::render_overlays::paint_cursor_picker)` |
| file_browser | `RenderSite::Frame(crate::render_overlays::paint_file_browser)` |
| prompt | `RenderSite::StatusRow` |
| minibuffer | `RenderSite::StatusRow` |
| search | `RenderSite::StatusRow` |
| diag | `RenderSite::Frame(crate::render_overlays::paint_diag)` |
| outline | `RenderSite::Frame(crate::render_overlays::paint_outline)` |

The `Menu` row's Frame painter is `paint_menu_dropdown` (DROPDOWN only); the always-on menu **bar** chrome is `paint_menu_bar`, painted as a standalone step at the Menu slot in `paint` (Step 5) — it is NOT a table row (chrome, out of scope, painted whether or not `menu` is `Some`). `paint_splash` is a thin wrapper (Step 4). `ratatui::Frame` is fully qualified in the `RenderSite` type, so no new import is required.

- [ ] **Step 4: Extract the paint blocks into `paint_*` fns (`render_overlays.rs`)**

Each overlay's block in `paint` (a re-window `if let Some(x) = editor.X.as_mut() { keep_overlay_visible... }` followed by `if let Some(ref x) = editor.X { ...paint... }`) moves into a `pub(crate) fn paint_X(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles)`. **The moved bodies reference the outer locals `area` and `h` (`let area = frame.area(); let h = area.height;` in today's `paint`) — every extracted painter MUST re-derive them at the top** or it won't compile. Because each block already self-gates on `if let Some`, calling the fn when the overlay is inactive is a no-op — behavior-identical to today's unconditional sequential blocks.

The six list-overlay painters share the identical two-line prologue, then the verbatim block. For example, `paint_palette`:

```rust
#[allow(clippy::too_many_lines)] // single overlay's paint block, extracted verbatim
pub(crate) fn paint_palette(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    let area = frame.area();
    let h = area.height;
    // ↓ the "Command palette overlay" block moves here VERBATIM (the
    //   `if let Some(p) = editor.palette.as_mut() { keep_overlay_visible(h, ...) }`
    //   re-window + the `if let Some(ref palette) = editor.palette { ... }` paint, which use
    //   `area` and `h`). No character of the block body changes.
}
```

Extract, each with the SAME `let area = frame.area(); let h = area.height;` prologue then the block verbatim:

- `paint_palette` — the "Command palette overlay" block. (uses `area`, `h`)
- `paint_outline` — the outline block. (uses `area`, `h`)
- `paint_theme_picker` — the theme_picker block. (uses `area`, `h`)
- `paint_cursor_picker` — the cursor_picker block. (uses `area`, `h`)
- `paint_file_browser` — the file_browser block. (uses `area`, `h`)
- `paint_diag` — the "Diagnostic quick-fix overlay" block. (uses `area`, `h`)

Add `#[allow(clippy::too_many_lines)]` (reason `// single overlay's paint block, extracted verbatim`) to any painter whose body exceeds 100 lines (palette/diag likely; add only if clippy flags it).

`paint_splash` — the thin wrapper (no `area`/`h` needed — `splash::paint` re-derives its own):

```rust
/// Splash painter (RENDER_ORDER[0]). Owns the whole frame; `paint` early-returns after this
/// so no other overlay paints while the splash is up.
pub(crate) fn paint_splash(frame: &mut Frame, editor: &mut Editor, _cs: &ChromeStyles) {
    crate::splash::paint(frame, editor);
}
```

**The menu bar/dropdown split (spec §2.3.1 — the delicate one).** Today the whole
`if editor.menu_bar_rows() == 1 { … }` block fuses the always-on BAR chrome (background +
labels, painted even when `editor.menu` is `None`) with the DROPDOWN (only when a category is
open). Split into TWO fns. The dropdown sub-block is exactly the
`// Paint the dropdown for the open category` / `if let Some(drop_rect) = menu_dropdown_rect(...)`
region (the block whose only use of the outer `menu` binding is
`menu_dropdown_rect(menu_area, &menu.groups, menu.open)`; everything after re-borrows
`editor.menu.as_ref()/as_mut()` internally).

`paint_menu_bar` — the standalone bar CHROME (out of the table). It is the whole
`if editor.menu_bar_rows() == 1` block MINUS the dropdown sub-block: the bar-row background,
the active-menu bar-label loop, and the inactive-bar `_` arm:

```rust
/// Always-on menu BAR chrome (out of the overlay table — painted whether or not `menu` is
/// `Some`). Pinned at the `Menu` slot of the RENDER_ORDER walk (spec §2.3.1). The DROPDOWN is
/// painted separately by `paint_menu_dropdown`.
pub(crate) fn paint_menu_bar(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    if editor.menu_bar_rows() != 1 { return; }
    let area = frame.area();
    let menu_area = crate::chrome_geom::menu_area(area);
    // Full-width bar background: gaps between labels + the right side carry the Chrome style;
    // the per-label paints below overwrite their own rects (A2).
    let bar_row = Rect::new(area.x, area.y, area.width, 1);
    frame.buffer_mut().set_style(bar_row, cs.menu_closed);
    match editor.menu {
        Some(ref menu) if !menu.groups.is_empty() => {
            // Paint the menu bar (one label per category)
            let bar = menu_bar_layout(menu_area, &menu.groups);
            for (i, rect) in &bar {
                let cat = menu.groups[*i].0;
                let label = crate::menu::category_label_pub(cat);
                let text = format!(" {label} ");
                let style = if *i == menu.open {
                    cs.menu_open
                } else {
                    cs.menu_closed
                };
                frame.render_widget(Paragraph::new(text).style(style), *rect);
            }
        }
        _ => {
            // Inactive bar (pinned / auto-revealed / unbuilt placeholder): static labels,
            // all closed-style, no dropdown, no highlight.
            for (i, rect) in &menu_bar_layout_cats(menu_area, &crate::registry::MENU_ORDER) {
                let label = crate::menu::category_label_pub(crate::registry::MENU_ORDER[*i]);
                frame.render_widget(Paragraph::new(format!(" {label} ")).style(cs.menu_closed), *rect);
            }
        }
    }
}
```

`paint_menu_dropdown` — the `Menu` row's Frame painter (DROPDOWN only, self-gated). It
re-derives `area`/`menu_area`, re-establishes the open-menu guard, computes `drop_rect` (the
outer `menu` binding's sole role), then runs the dropdown body VERBATIM (lines 446–505 of
today's block, which use `editor.menu.as_mut()/as_ref()` and `drop_rect`):

```rust
#[allow(clippy::too_many_lines)] // the menu dropdown paint block, extracted verbatim
pub(crate) fn paint_menu_dropdown(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    if editor.menu_bar_rows() != 1 { return; }
    // Guard: only an OPEN, non-empty menu has a dropdown (the outer `match editor.menu`
    // Some-arm condition in today's block).
    let open_nonempty = matches!(editor.menu, Some(ref m) if !m.groups.is_empty());
    if !open_nonempty { return; }
    let area = frame.area();
    let menu_area = crate::chrome_geom::menu_area(area);
    // The outer `menu` binding's only role: compute the dropdown rect.
    let drop = {
        let menu = editor.menu.as_ref().unwrap();
        menu_dropdown_rect(menu_area, &menu.groups, menu.open)
    };
    let Some(drop_rect) = drop else { return; };
    // ↓ verbatim from today's dropdown block (the body inside `if let Some(drop_rect) = …`):
    //   the `let scroll_top = { … editor.menu.as_mut().unwrap() … };` re-window, the Clear +
    //   fill, the items build from `editor.menu.as_ref().unwrap()`, the item_rect render, and
    //   the overflow n/total indicator. No character of that body changes.
    let scroll_top = {
        let m = editor.menu.as_mut().unwrap();
        let leaves_len = m.groups[m.open].1.len();
        let list_h = drop_rect.height as usize;
        let overflows = leaves_len > list_h;
        let keep_h = if overflows { list_h.saturating_sub(1) } else { list_h };
        crate::list_window::keep_visible(m.highlighted, leaves_len, keep_h, &mut m.scroll_top);
        m.scroll_top
    };
    frame.render_widget(Clear, drop_rect);
    frame.buffer_mut().set_style(drop_rect, cs.menu_norm);
    let (highlighted, leaves_len) = {
        let m = editor.menu.as_ref().unwrap();
        (m.highlighted, m.groups[m.open].1.len())
    };
    let list_h = drop_rect.height as usize;
    let overflows = leaves_len > list_h;
    let item_rows = if overflows { list_h.saturating_sub(1) } else { list_h };
    let end = (scroll_top + item_rows).min(leaves_len);
    let leaves = &editor.menu.as_ref().unwrap().groups[editor.menu.as_ref().unwrap().open].1;
    let items: Vec<ListItem> = leaves[scroll_top..end]
        .iter()
        .enumerate()
        .map(|(row_in_window, (label, _))| {
            let abs_row = scroll_top + row_in_window;
            let style = if abs_row == highlighted {
                cs.menu_sel
            } else {
                cs.menu_norm
            };
            ListItem::new(format!(" {label} ")).style(style)
        })
        .collect();
    let item_rect = if overflows && list_h > 0 {
        Rect::new(drop_rect.x, drop_rect.y, drop_rect.width, item_rows as u16)
    } else {
        drop_rect
    };
    frame.render_widget(List::new(items), item_rect);
    if overflows && list_h > 0 {
        if let Some(ind) = windowed_indicator(highlighted, leaves_len, list_h) {
            let ind_y = drop_rect.y + drop_rect.height - 1;
            let ind_rect = Rect::new(drop_rect.x, ind_y, drop_rect.width, 1);
            frame.render_widget(
                Paragraph::new(ind).style(cs.menu_norm),
                ind_rect,
            );
        }
    }
}
```

- [ ] **Step 5: Rewrite `paint` to the RENDER_ORDER walk (bar chrome pinned at the Menu slot)**

Replace the body of `render_overlays::paint` (everything from the splash early-return through the diag block) with:

```rust
pub(crate) fn paint(frame: &mut Frame, editor: &mut Editor, cs: &ChromeStyles) {
    // Splash owns the frame (RENDER_ORDER[0]) — paint it and return, exactly as before.
    if editor.splash.is_some() {
        crate::render_overlays::paint_splash(frame, editor, cs);
        return;
    }
    // Walk the remaining Frame overlays in paint order. Each painter self-gates on its own
    // `if let Some(..)`, so an inactive overlay is a no-op (byte-identical to the old
    // sequential blocks). The always-on menu BAR chrome is NOT a table row — it is painted as
    // a standalone step pinned at the `Menu` slot, before the menu-dropdown painter, so it
    // sits at the same z-position it held today (after file_browser, before diag): palette/
    // outline/theme_picker/cursor_picker/file_browser paint UNDER the bar, diag OVER it.
    for id in &crate::overlays::RENDER_ORDER[1..] {
        if *id == crate::overlays::OverlayId::Menu {
            paint_menu_bar(frame, editor, cs); // chrome (out of table), pinned here
        }
        if let crate::overlays::RenderSite::Frame(f) = id.row().render {
            f(frame, editor, cs);
        }
    }
}
```

(The old `#[allow(clippy::too_many_lines)]` on `paint` is removed — the new body is well under 100 lines. The `OV_QUERY_PREFIX_COLS` const and helpers used by the extracted painters stay in the module.)

- [ ] **Step 6: Run tests + gates**

Run: `cargo test -p wordcartel --lib 2>&1 | tail -20`
Expected: PASS — `render_order_is_exactly_the_frame_overlays` plus all render/overlay paint tests and the e2e journeys (render output byte-identical).
Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -15 && cargo test -p wordcartel --test module_budgets 2>&1 | tail -8`
Expected: clippy clean; render budget holds.

- [ ] **Step 7: Verify render output is unchanged (visual regression via e2e)**

Run: `cargo test -p wordcartel --lib e2e:: 2>&1 | tail -15`
Expected: PASS — the in-process `reduce → advance → render` journeys against `TestBackend` confirm the paint sequence (incl. menu bar vs overlay z-order) is byte-identical.

- [ ] **Step 8: Commit**

```bash
git add wordcartel/src/overlays.rs wordcartel/src/render_overlays.rs
git commit -m "refactor(overlays): fold paint into RENDER_ORDER walk; menu bar/dropdown split (H21 T5)"
```

---

## Task 6: Completeness sweep + retire the B11 census

Adds the full per-overlay sweep (open → exactly-one-active XOR + key-Press-consumed + click-consumed) and retires the now-subsumed `render.rs` B11 census test.

**Files:**
- Modify: `wordcartel/src/overlays.rs` (`#[cfg(test)] mod tests` — the sweep)
- Modify: `wordcartel/src/render.rs` (delete `has_active_input_overlay_true_for_every_surface`)
- Test: `wordcartel/src/overlays.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–5 (`any_active`, `close_all`, the intercept/mouse table, `reduce`, `mouse::handle`).

- [ ] **Step 1: Write the failing sweep test**

Add to `wordcartel/src/overlays.rs` `mod tests`. It opens each of the 11 overlays via a real constructor, then asserts XOR + key-Press-consumed + click-consumed:

```rust
    /// The completeness sweep (subsumes render.rs's B11 census). For EACH overlay: open it,
    /// assert exactly one row is_active (XOR); a key-Press routed through `reduce` is consumed
    /// (buffer version unchanged — no keystroke leak); and a mouse Down-left in the text band
    /// routed through `mouse::handle` is consumed by the overlay's mouse slot (the caret does
    /// NOT jump to the clicked cell — no click-through while a modal is up).
    #[test]
    fn every_overlay_is_active_xor_and_consumes_key_and_click() {
        use crossterm::event::{Event, KeyEvent, KeyCode, KeyEventKind, KeyEventState,
            MouseEvent, MouseEventKind, MouseButton, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let diag_fixture = || wordcartel_core::diagnostics::Diagnostic {
            range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "m".into(), suggestions: Vec::new(),
        };
        // (name, opener) — every OverlayId, opened via a real path.
        let openers: Vec<(&str, Box<dyn Fn(&mut Editor)>)> = vec![
            ("search",        Box::new(|e: &mut Editor| e.open_search(crate::search_overlay::Phase::Find, 0))),
            ("minibuffer",    Box::new(|e: &mut Editor| e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter))),
            ("palette",       Box::new(|e: &mut Editor| e.open_palette())),
            ("outline",       Box::new(|e: &mut Editor| e.open_outline())),
            ("theme_picker",  Box::new(|e: &mut Editor| e.open_theme_picker())),
            ("file_browser",  Box::new(|e: &mut Editor| e.open_file_browser(std::path::PathBuf::from(".")))),
            ("prompt",        Box::new(|e: &mut Editor| e.open_prompt(crate::prompt::Prompt::swap_recovery()))),
            ("diag",          Box::new(move |e: &mut Editor| e.open_diag(diag_fixture()))),
            ("cursor_picker", Box::new(|e: &mut Editor| e.open_cursor_picker())),
            ("menu",          Box::new(|e: &mut Editor| { e.menu = Some(crate::menu::empty()); })),
            ("splash",        Box::new(|e: &mut Editor| { e.splash = Some(crate::splash::Splash::new(
                &crate::keymap::KeyTrie::default(), "0.0.0")); })),
        ];
        for (name, open) in openers {
            let mut e = Editor::new_from_text("hello world\nsecond line here\n", None, (40, 12));
            crate::derive::rebuild(&mut e);
            open(&mut e);
            // (a) exactly one active
            let active = OverlayId::ALL.iter().filter(|id| (id.row().is_active)(&e)).count();
            assert_eq!(active, 1, "{name}: exactly one overlay active (XOR)");
            // (b) a key-Press is consumed — the buffer version must not change (every overlay
            // intercept returns Handled::Done for ALL key messages, so 'z' never reaches the buffer).
            let v0 = e.active().document.version;
            let key = KeyEvent { code: KeyCode::Char('z'), modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press, state: KeyEventState::NONE };
            crate::app::reduce(crate::app::Msg::Input(Event::Key(key)), &mut e, &reg, &km,
                &ex, &clock, &tx);
            assert_eq!(e.active().document.version, v0, "{name}: key-Press did not edit the buffer");
            // (c) a Down-left in the text band is consumed by the overlay's mouse slot — the
            // caret does NOT move to the clicked cell (with NO overlay this click WOULD move it).
            // `mouse_capture` defaults true, so `handle` routes through `route_overlay`. Re-open
            // in case the key above dismissed the overlay (e.g. splash Press dismisses it).
            let mut e = Editor::new_from_text("hello world\nsecond line here\n", None, (40, 12));
            crate::derive::rebuild(&mut e);
            open(&mut e);
            let before = crate::nav::head(&e);
            let click = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left),
                column: 6, row: 9, modifiers: KeyModifiers::NONE };
            crate::mouse::handle(&mut e, click, &reg, &km, &ex, &clock, &tx);
            assert_eq!(crate::nav::head(&e), before,
                "{name}: text-band click consumed by the overlay — caret did not jump");
        }
    }
```

- [ ] **Step 2: Run to verify it passes**

Run: `cargo test -p wordcartel --lib overlays::tests::every_overlay_is_active_xor_and_consumes_key_and_click 2>&1 | tail -25`
Expected: PASS. (Notes: `crate::menu::empty()` is a valid unbuilt placeholder that `is_active` sees as `Some`, and `menu::intercept` consumes the key; the click at `(col 6, row 9)` is in the text band — every overlay's mouse slot consumes it or click-aways-closes without moving the caret, whereas with no overlay it would move the caret to that cell.)

- [ ] **Step 3: Retire the subsumed B11 census test**

In `wordcartel/src/render.rs`, delete the test `has_active_input_overlay_true_for_every_surface` (and its now-unused `diag_fixture` helper IF no other test in that module uses it — `grep -n "diag_fixture" render.rs` first; keep it if a sibling test references it). The sweep in `overlays.rs` is strictly stronger (XOR + key-consumed, not just the predicate).

- [ ] **Step 4: Full test + gate sweep**

Run: `cargo test -p wordcartel 2>&1 | tail -20`
Expected: PASS (lib + integration).
Run: `cargo clippy --workspace --all-targets 2>&1 | tail -15`
Expected: clean.
Run: `cargo test -p wordcartel-core 2>&1 | tail -8`
Expected: PASS (untouched, sanity).

- [ ] **Step 5: PTY smoke suite (mandatory-run, advisory-pass)**

Run: `scripts/smoke/run.sh 2>&1 | tail -5`
Expected: quote the one-line summary verbatim (e.g. `smoke: 8/8 PASS`, or `smoke: SKIP — …` on a tmux-less machine). A red result is an advisory finding to surface, not a merge blocker.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/overlays.rs wordcartel/src/render.rs
git commit -m "test(overlays): completeness sweep (XOR+key+click); retire B11 census (H21 T6)"
```

---

## Self-Review

**1. Spec coverage:**
- `overlays.rs` with `OverlayId`/`OVERLAYS`/`RENDER_ORDER`/`DispatchCtx`/`RenderSite`/`close_all` — Tasks 1–5. ✓
- Fold both is-active predicates (`has_active_input_overlay`, `no_overlay_open`) — Task 1. ✓
- Fold H10 (intercept chain), splash→marks→rest order, `&ALL[1..]` — Task 2. ✓
- Signature heterogeneity via `DispatchCtx` (editor separate; 7→3 collapse for menu/palette); all 7 existing direct `intercept` unit-test callers migrated (splash/minibuffer×2/prompts×2/menu/plugin-host) — Task 2 Steps 4–5. ✓
- Fold mouse routing + splash mouse slot + the two Down-left close arms (exact guards) — Task 4. ✓
- Fold XOR close (open_* + dispatch_overlay_command + registry "menu" toggle) — Task 3; save.rs exclusion stated in File Structure + `close_all` doc. ✓
- Render: RENDER_ORDER walk, RenderSite (`#[derive(Clone, Copy)]`, Frame/StatusRow), menu **bar** = standalone `paint_menu_bar` chrome pinned at the Menu slot + `Menu` row = dropdown-only `paint_menu_dropdown`, each extracted painter re-derives `area`/`h`, StatusRow trio untouched — Task 5. ✓
- Q4 delta + guardrail (no dwell under splash) — Task 1. ✓
- Three completeness tests: bijection+Splash-first (T1) + render-coverage (T5); per-overlay XOR+key-consumed+click-consumed sweep (T6); Q4 guardrail (T1). `close_all` test opens all 11 (T3). ✓
- Plugin-panel doc-only (no PluginPanel variant) — `overlays.rs` module header (T1 Step 3). ✓
- Command-surface contract N/A-leaning — Global Constraints. ✓
- `#![forbid(unsafe_code)]` core-only; clippy/`too_many_lines`/module-budget gates; no `cargo fmt` — Global Constraints; per-task gate steps. ✓

**2. Placeholder scan:** No "TBD/TODO/similar to Task N/add error handling." The large verbatim-move bodies (mouse_* / paint_*) are specified by exact source block + the required `let area = frame.area(); let h = area.height;` prologue + the precise `ctx.*` substitutions + fn signature — complete, not hand-waved. The menu bar/dropdown split and the sweep's per-overlay click assertion show full code.

**3. Type consistency:** `DispatchCtx` fields (`reg, keymap, ex, clock, msg_tx`) match `reduce_dispatch`/`mouse::handle` params and are used identically in every migrated body. `fn(Msg, &mut Editor, &DispatchCtx) -> Handled` is the single intercept type across Task 2 and the table. `close_all`/`any_active`/`OverlayId::row`/`RENDER_ORDER`/`RenderSite::{Frame,StatusRow}` names are consistent across tasks. `OverlayRow` grows monotonically (is_active → intercept → close → mouse → render), each new field with a same-task reader (no dead-field warning). The `paint_*` fn signature `(&mut Frame, &mut Editor, &ChromeStyles)` matches `RenderSite::Frame`'s fn-pointer type.

---

Plan complete and saved to `docs/superpowers/plans/2026-07-15-h21-overlay-dispatch-table-plan.md`. Two execution options:

1. **Subagent-Driven (recommended)** — a fresh subagent per task + two-stage review between tasks.
2. **Inline Execution** — batch execution in this session with checkpoints.
