# Chrome Density Presets + Overlay/Mouse Completeness — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the shipped `[theme] chrome = zen|full` color axis into a full density preset that also drives chrome *visibility*, make every overlay/modal mouse-complete to the command palette's standard, and give the menu dropdown the windowing the other overlays already have.

**Architecture:** A density preset is **data** — a `ChromeBundle` of `element → value` applied by one general `apply_bundle` routine, never `if zen {…}` branching. Two built-in bundles (`ZEN`, `FULL`); the shape is additive so config-defined bundles (L2) and named profiles (L3) are future efforts, not rewrites. A new `TransientMode { Off, Auto, On }` unifies the three reveal-on-dwell elements (menu bar keeps its existing `MenuBarMode`, mapped). Overlay mouse routing is restructured so it runs *before* menu-dwell arming, closing the no-leak gap.

**Tech Stack:** `wordcartel-core` (pure model: theme/faces, `#![forbid(unsafe_code)]`) + `wordcartel` shell (ratatui 0.30, crossterm). No new dependencies.

**Source spec:** `docs/superpowers/specs/2026-07-06-wordcartel-chrome-density-and-overlay-completeness-design.md` (Codex spec gate CLEAN, round 3).

## Global Constraints

- **No-silent-UI is inviolable.** Any status message — errors above all — must become visible regardless of density mode. Status line has **no true `Off`**: a message / prompt / search / minibuffer force-reveals the reserved bottom row even under `TransientMode::Auto`. A config `status_line = "off"` is rejected (coerced to `auto`, accumulated as a config warning surfaced at startup — `app.rs:1463`). The shared `TransientMode::Off` exists solely for the scrollbar.
- **No hot-path jank.** The status row is **always reserved**: `edit_height = h.saturating_sub(1 + menu_rows)` (`render.rs:362`) is unchanged — reveal/hide of the status line is a *paint* difference on the already-reserved row, never a reflow. Per-keystroke work stays `O(visible)`. (The menu bar retains its existing reflow-on-reveal via `menu_bar_rows()`; only the status row is reserve-not-reclaim.)
- **Individual overrides win at startup; presets clobber at runtime.** An explicit config key (`[menu] bar = pinned`, `[view] scrollbar = on`, …) overrides a preset's default at startup seed and persists via the diff-law (`settings.rs:252`). Re-selecting a preset at runtime (`toggle_chrome`) re-applies its whole bundle over unsaved runtime state (spec §1.5 — runtime-clobber).
- **Exhaustive matches** on `SemanticElement` / `ChromeDisposition` / `CanvasMode` / the new `TransientMode` — no catch-all `_` that silently absorbs a new variant.
- **House style / GATEs:** `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean (deny); `cargo build` / `cargo test --no-run` warning-free for touched crates; **no `cargo fmt`** (hand-formatted, dense style; match neighbors); em-dash (`—`) prose comments, never `--`; no decorative/emoji unicode in code (multibyte only in tests). Doc-comment every new public item. Smoke suite (`scripts/smoke/run.sh`) mandatory-run / advisory-pass.
- **Ordering requirement (spec Part 3).** The universal overlay guard must be a single route placed *before* the menu-dwell-arming block (`mouse.rs:94-119`): when any overlay is open, route the event to the overlay layer, which applies its overlay-specific behavior and returns — this PRESERVES the existing palette/menu/theme/file branches (they move ahead of dwell, they are not bypassed).

---

## File Structure

**New file:**
- `wordcartel/src/density.rs` — the `ChromeBundle` record, the `ZEN`/`FULL` constants, `apply_bundle`, and `bundle_names()`/`bundle_for`. One clear responsibility: the density-preset data model and its applier. Kept out of `editor.rs` (already large) and `registry.rs`.

**Modified files (by part):**
- Part 1 (E1): `config.rs` (`TransientMode`, `[view] scrollbar`/`status_line` fields + fold), `editor.rs` (runtime fields + MouseState dwell timers), `mouse.rs` (right-edge + bottom-row dwell arming), `app.rs` (recompute status/scrollbar, seed), `render.rs` (status idle-calm paint), `settings.rs` (snapshot + O* mirror + diff_key), `registry.rs` (`toggle_chrome` applies bundle), `density.rs` (new).
- Part 2 (E2): `registry.rs` (`CommandMeta.state`, `MenuMark`, stateful command wiring, `keymap_next`), `menu.rs` (`grouped_commands(&Editor)` + label composition), `render_overlays.rs` (attached filled-panel dropdown).
- Part 3 (overlay/mouse): `mouse.rs` (restructure + new consuming branches), `render.rs` (per-overlay row-hit-test helpers), `render_overlays.rs` (unchanged geometry reused).
- Part 4 (menu windowing): `menu.rs` (`MenuView.scroll_top`), `render.rs` (`menu_dropdown_rect` windowing + `menu_dropdown_row_at`), `render_overlays.rs` (windowed dropdown paint + indicator), `mouse.rs` (menu scroll arm).

**Task dependency order:** 1 → 2 → 3 → 4 → 5 → 6 (Part 1, sequential — each builds the runtime substrate the next needs) → 7 → 8 (Part 2) → 9 → 10 → 11 → 12 → 13 (Part 3, 9 first — it restructures `handle`) → 14 (Part 4). Task 8 (dropdown styling) and Task 14 (dropdown windowing) both touch the dropdown paint; **14 lands after 8** and re-reads the filled-panel paint.

---

## Part 1 — E1: density presets

### Task 1: `TransientMode` enum + `[view] scrollbar` / `[view] status_line` config

**Files:**
- Modify: `wordcartel/src/config.rs` (add enum near `MenuBarMode` at `:80`; `ViewConfig` at `:90`; `RawView` at `:269`; fold at `:358`)
- Test: inline `#[cfg(test)]` in `config.rs`

**Interfaces:**
- Produces: `pub enum TransientMode { Off, Auto, On }` (Copy, Eq); `pub fn transient_mode_str(TransientMode) -> &'static str`; `ViewConfig.scrollbar: TransientMode`, `ViewConfig.status_line: TransientMode`; `RawView.scrollbar: Option<String>`, `RawView.status_line: Option<String>`.

- [ ] **Step 1: Write the failing test** — parse + coercion + str round-trip.

Add to `config.rs` tests, using the **existing** test helpers `tempdir()` + `write(dir, name, body)` (Codex plan gate — there is no `write_cfg`; the real helpers are at the top of `config.rs::tests`, used e.g. by `malformed_toml_warns_and_skips_layer` at `config.rs:514`):
```rust
#[test]
fn view_transient_keys_parse_and_status_off_coerces() {
    // scrollbar accepts off/auto/on verbatim.
    let d1 = tempdir();
    let p = write(&d1, "c.toml", "[view]\nscrollbar = \"on\"\nstatus_line = \"auto\"\n");
    let (cfg, warns) = load(&[p]);
    assert_eq!(cfg.view.scrollbar, TransientMode::On);
    assert_eq!(cfg.view.status_line, TransientMode::Auto);
    assert!(warns.is_empty());
    // status_line = "off" is rejected → coerced to Auto, with a warning (no-silent-UI).
    let d2 = tempdir();
    let p2 = write(&d2, "c.toml", "[view]\nstatus_line = \"off\"\n");
    let (cfg2, warns2) = load(&[p2]);
    assert_eq!(cfg2.view.status_line, TransientMode::Auto,
        "status_line off must coerce to auto to preserve no-silent-UI");
    assert!(warns2.iter().any(|w| w.contains("status_line")),
        "coercion must warn, got {warns2:?}");
    // bogus value → default + warning.
    let d3 = tempdir();
    let p3 = write(&d3, "c.toml", "[view]\nscrollbar = \"bogus\"\n");
    let (cfg3, warns3) = load(&[p3]);
    assert_eq!(cfg3.view.scrollbar, TransientMode::Auto, "bogus → default auto");
    assert!(warns3.iter().any(|w| w.contains("scrollbar")));
}
```
(Confirm the exact `tempdir()`/`write` signatures against `config.rs::tests` when implementing — match whatever the neighbors use verbatim; the `d1`/`d2`/`d3` bindings keep each `TempDir` alive for its `load`.)

- [ ] **Step 2: Run it to verify it fails** — `cargo test -p wordcartel --lib config::tests::view_transient_keys_parse_and_status_off_coerces` → FAIL (`TransientMode` undefined).

- [ ] **Step 3: Add the enum + config fields.**

Near `config.rs:80` (below `MenuBarMode`):
```rust
/// Reveal policy for a transient chrome element (status line, scrollbar; the menu
/// bar keeps its own `MenuBarMode` mapped onto this). `Off` = never shown, `On` =
/// always shown, `Auto` = revealed on pointer dwell near the element plus a
/// context trigger (scroll activity / a status message), hidden after a leave grace.
///
/// The status line has no true `Off`: a message force-reveals it even under `Auto`
/// (no-silent-UI). Only the scrollbar uses `Off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransientMode { Off, Auto, On }
```
Add to `ViewConfig` (`config.rs:90`) after `word_count`:
```rust
    pub scrollbar: TransientMode,
    pub status_line: TransientMode,
```
Add to `ViewConfig::default()` (`config.rs:100`):
```rust
            scrollbar: TransientMode::Auto, status_line: TransientMode::Auto,
```
Add to `RawView` (`config.rs:269`):
```rust
    scrollbar: Option<String>,
    status_line: Option<String>,
```
Add to the fold (`config.rs:384`, after the `menu.bar` block or end of the `view` block near `:384` — put beside the other view keys, after the `focus_granularity` block at `:384`):
```rust
        if let Some(s) = raw.view.scrollbar {
            match s.as_str() {
                "off"  => cfg.view.scrollbar = TransientMode::Off,
                "auto" => cfg.view.scrollbar = TransientMode::Auto,
                "on"   => cfg.view.scrollbar = TransientMode::On,
                other => warns.push(format!("view.scrollbar \"{other}\" invalid; using auto")),
            }
        }
        if let Some(s) = raw.view.status_line {
            match s.as_str() {
                // No true Off: reject "off" (coerce to Auto) so a message can always paint.
                "off"  => { cfg.view.status_line = TransientMode::Auto;
                            warns.push("view.status_line \"off\" not allowed (no-silent-UI); using auto".to_string()); }
                "auto" => cfg.view.status_line = TransientMode::Auto,
                "on"   => cfg.view.status_line = TransientMode::On,
                other => warns.push(format!("view.status_line \"{other}\" invalid; using auto")),
            }
        }
```
Add the str helper near `config.rs` (top-level fn, mirrors `settings::menu_bar_str`):
```rust
/// "off"/"auto"/"on" — round-trips `TransientMode` for the overrides mirror.
pub fn transient_mode_str(m: TransientMode) -> &'static str {
    match m { TransientMode::Off => "off", TransientMode::Auto => "auto", TransientMode::On => "on" }
}
```

- [ ] **Step 4: Run tests to verify pass** — same command → PASS. Also `cargo test -p wordcartel --lib config::` green.

- [ ] **Step 5: Commit** — `git add wordcartel/src/config.rs && git commit -m "feat(config): TransientMode enum + [view] scrollbar/status_line keys"` (+ trailers).

---

### Task 2: Runtime fields + MouseState dwell timers + startup seed

**Files:**
- Modify: `wordcartel/src/editor.rs` (`MouseState` at `:324`; `Editor` fields at `:399`; `Editor::default`/`new` seed at `:485`)
- Modify: `wordcartel/src/app.rs` (seed region `:1347-1356`)
- Test: inline in `editor.rs`

**Interfaces:**
- Consumes: `TransientMode` (Task 1).
- Produces: `Editor.scrollbar_mode: TransientMode`, `Editor.status_line_mode: TransientMode`; `MouseState.scrollbar_reveal_due/scrollbar_hide_due/scrollbar_revealed: Option<u64>/Option<u64>/bool`, `MouseState.status_reveal_due/status_hide_due/status_revealed` likewise. Seeded from `cfg.view.scrollbar`/`cfg.view.status_line` at startup.

- [ ] **Step 1: Write the failing test** — default modes + seed.
```rust
#[test]
fn editor_seeds_transient_modes_and_mouse_dwell_defaults() {
    let e = Editor::new_from_text("x\n", None, (40, 8));
    assert_eq!(e.scrollbar_mode, crate::config::TransientMode::Auto);
    assert_eq!(e.status_line_mode, crate::config::TransientMode::Auto);
    assert_eq!(e.mouse.scrollbar_reveal_due, None);
    assert!(!e.mouse.status_revealed);
}
```

- [ ] **Step 2: Run it to verify it fails** — FAIL (fields undefined).

- [ ] **Step 3: Add the fields.**

`MouseState` (`editor.rs:344`, after `menu_bar_revealed`) — mirror the menu-dwell trio for scrollbar and status:
```rust
    /// Right-edge dwell deadline for the Auto-mode scrollbar (armed on rest at col w-1).
    pub scrollbar_reveal_due: Option<u64>,
    /// Leave-grace deadline for the Auto-mode scrollbar (armed once on leave).
    pub scrollbar_hide_due: Option<u64>,
    /// Whether the Auto-mode scrollbar is currently dwell-revealed (independent of
    /// `scrollbar_until_ms`, which is the scroll-activity channel).
    pub scrollbar_revealed: bool,
    /// Bottom-row dwell deadline for the Auto-mode status line.
    pub status_reveal_due: Option<u64>,
    /// Leave-grace deadline for the Auto-mode status line.
    pub status_hide_due: Option<u64>,
    /// Whether the Auto-mode status line is currently dwell-revealed.
    pub status_revealed: bool,
```
(These extend the `#[derive(Default)]` `MouseState`, so no manual default needed.)

`Editor` struct (`editor.rs:399`, after `menu_bar_mode`):
```rust
    pub scrollbar_mode: crate::config::TransientMode,
    pub status_line_mode: crate::config::TransientMode,
```
`Editor::default`/constructor seed (`editor.rs:485`, after `menu_bar_mode: …Auto`):
```rust
            scrollbar_mode: crate::config::TransientMode::Auto,
            status_line_mode: crate::config::TransientMode::Auto,
```

`app.rs` seed (`app.rs:1348`, right after `editor.view_opts = cfg.view.clone();`):
```rust
    editor.scrollbar_mode = cfg.view.scrollbar;
    editor.status_line_mode = cfg.view.status_line;
```

- [ ] **Step 4: Run tests to verify pass** — PASS; `cargo build -p wordcartel` warning-free.

- [ ] **Step 5: Commit** — `feat(editor): scrollbar/status_line runtime modes + dwell timers + seed`.

---

### Task 3: Scrollbar visibility honors mode + right-edge dwell

**Files:**
- Modify: `wordcartel/src/app.rs` (`recompute_scrollbar_visible` at `:1665`)
- Modify: `wordcartel/src/mouse.rs` (dwell-arming region; add scrollbar right-edge arm)
- Test: inline in `app.rs` (visibility truth table) + `mouse.rs` (arm)

**Interfaces:**
- Consumes: `Editor.scrollbar_mode`, MouseState scrollbar dwell fields (Task 2), `TransientMode`.
- Produces: `recompute_scrollbar_visible` now sets `scrollbar_visible` per mode; a right-edge dwell arm mirroring the menu arm.

- [ ] **Step 1: Write the failing tests.**

`app.rs` tests:
```rust
#[test]
fn scrollbar_visible_respects_mode() {
    use crate::config::TransientMode;
    let mut e = Editor::new_from_text("x\n", None, (40, 8));
    // On: always visible regardless of activity/dwell.
    e.scrollbar_mode = TransientMode::On;
    recompute_scrollbar_visible(&mut e, 10_000);
    assert!(e.mouse.scrollbar_visible, "On → always visible");
    // Off: never visible even with fresh activity.
    e.scrollbar_mode = TransientMode::Off;
    e.mouse.scrollbar_until_ms = 20_000;
    recompute_scrollbar_visible(&mut e, 10_000);
    assert!(!e.mouse.scrollbar_visible, "Off → never visible");
    // Auto: visible while activity OR dwell holds; hidden once both lapse.
    e.scrollbar_mode = TransientMode::Auto;
    e.mouse.scrollbar_until_ms = 20_000; e.mouse.scrollbar_revealed = false;
    recompute_scrollbar_visible(&mut e, 10_000);
    assert!(e.mouse.scrollbar_visible, "Auto + activity → visible");
    e.mouse.scrollbar_until_ms = 0; e.mouse.scrollbar_revealed = true;
    recompute_scrollbar_visible(&mut e, 10_000);
    assert!(e.mouse.scrollbar_visible, "Auto + dwell-revealed → visible");
    e.mouse.scrollbar_revealed = false;
    recompute_scrollbar_visible(&mut e, 10_000);
    assert!(!e.mouse.scrollbar_visible, "Auto + neither → hidden");
}
```

- [ ] **Step 2: Run to verify fail** — FAIL (current impl ignores mode).

- [ ] **Step 3: Implement.**

Replace `recompute_scrollbar_visible` (`app.rs:1665`). **Fire the dwell deadlines FIRST, then compute `scrollbar_visible`** (Codex plan gate Critical — computing visibility before firing the deadline delays a reveal that lands exactly on `now_ms` by a frame):
```rust
pub fn recompute_scrollbar_visible(editor: &mut crate::editor::Editor, now_ms: u64) {
    use crate::config::TransientMode;
    // Fire the Auto dwell/grace deadlines FIRST (armed by the mouse Moved arm), so a
    // deadline landing exactly on `now_ms` flips `scrollbar_revealed` BEFORE we read it.
    if editor.scrollbar_mode == TransientMode::Auto {
        if editor.mouse.scrollbar_reveal_due.is_some_and(|d| now_ms >= d) {
            editor.mouse.scrollbar_reveal_due = None;
            editor.mouse.scrollbar_revealed = true;
        }
        if editor.mouse.scrollbar_hide_due.is_some_and(|d| now_ms >= d) {
            editor.mouse.scrollbar_hide_due = None;
            editor.mouse.scrollbar_revealed = false;
        }
    } else {
        editor.mouse.scrollbar_reveal_due = None;
        editor.mouse.scrollbar_hide_due = None;
        editor.mouse.scrollbar_revealed = false;
    }
    editor.mouse.scrollbar_visible = match editor.scrollbar_mode {
        TransientMode::On  => true,
        TransientMode::Off => false,
        // Auto: scroll activity (the existing channel) OR a live right-edge dwell.
        TransientMode::Auto => now_ms < editor.mouse.scrollbar_until_ms
            || editor.mouse.scrollbar_revealed,
    };
}
```

**Deadline-array wake-up (Codex plan gate Critical — shared with Task 4).** The run loop's deadline computation (`app.rs:1557-1567`) currently mins only `scrollbar_until_ms` (`sb_deadline`) and `menu_reveal_due.or(menu_hide_due)` (`menu_deadline`). The new dwell timers must also wake the loop, or a reveal/hide never fires until an unrelated event. Add to that block:
```rust
        let sb_dwell_deadline = editor.mouse.scrollbar_reveal_due.or(editor.mouse.scrollbar_hide_due);
        let status_dwell_deadline = editor.mouse.status_reveal_due.or(editor.mouse.status_hide_due);
```
and fold `sb_dwell_deadline` and `status_dwell_deadline` into the same `min`-of-`Some` reduction the loop already applies to `sb_deadline`/`menu_deadline` (match the existing combinator at `app.rs:~1568-1580` — grep the line that mins the deadlines into the `poll` timeout).
Add the right-edge dwell arm in `mouse.rs`, in the `Moved` handling. Extend the existing dwell block (`mouse.rs:98-119`) with a sibling arm gated on `scrollbar_mode == Auto`. Place it **after** the menu-bar arm, inside the same `if let MouseEventKind::Moved = ev.kind` (restructure the menu arm to share the Moved guard). Concretely, after the menu-bar `if editor.menu_bar_mode == …Auto { … }` block, add:
```rust
    // Scrollbar right-edge dwell (mirror of the menu-bar dwell; col w-1 is the track).
    if editor.scrollbar_mode == crate::config::TransientMode::Auto {
        if let MouseEventKind::Moved = ev.kind {
            let w = editor.active().view.area.0;
            let at_right_edge = ev.column == w.saturating_sub(1);
            if at_right_edge {
                editor.mouse.scrollbar_hide_due = None;
                if !editor.mouse.scrollbar_revealed
                    && editor.mouse.scrollbar_reveal_due.is_none()
                    && no_overlay_open(editor)
                    && !editor.mouse.dragging && !editor.mouse.scrollbar_dragging
                {
                    editor.mouse.scrollbar_reveal_due = Some(clock.now_ms() + MENU_DWELL_MS);
                }
            } else {
                editor.mouse.scrollbar_reveal_due = None;
                if editor.mouse.scrollbar_revealed && editor.mouse.scrollbar_hide_due.is_none() {
                    editor.mouse.scrollbar_hide_due = Some(clock.now_ms() + MENU_LEAVE_GRACE_MS);
                }
            }
        }
    }
```
Add a private helper in `mouse.rs` (also used by Task 9's guard):
```rust
/// True when NO overlay/modal is open — the shared predicate for dwell suppression.
fn no_overlay_open(editor: &Editor) -> bool {
    editor.menu.is_none() && editor.palette.is_none() && editor.theme_picker.is_none()
        && editor.file_browser.is_none() && editor.outline.is_none() && editor.diag.is_none()
        && editor.prompt.is_none() && editor.minibuffer.is_none() && editor.search.is_none()
}
```
Reuse `no_overlay_open` to replace the menu-arm's inline `editor.menu.is_none() && editor.palette.is_none() && …` gate (`mouse.rs:107-110`) so the two dwell arms share one predicate.

Add a `mouse.rs` arm test:
```rust
#[test]
fn scrollbar_dwell_arms_on_right_edge_rest() {
    let mut e = Editor::new_from_text("hello\n", None, (40, 8));
    crate::derive::rebuild(&mut e);
    e.scrollbar_mode = crate::config::TransientMode::Auto;
    let (reg, ex, _, tx, km) = ctx();
    handle(&mut e, moved(39, 4), &reg, &km, &ex, &TestClock(0), &tx); // col w-1 = 39
    assert_eq!(e.mouse.scrollbar_reveal_due, Some(MENU_DWELL_MS));
}
```

- [ ] **Step 4: Run tests to verify pass** — both new tests PASS; existing `mouse::tests::dwell_*` still green (the menu arm is unchanged behaviorally — verify `dwell_never_arms_during_drag_or_overlay` still passes since `no_overlay_open` now covers more overlays, which only tightens the gate).

- [ ] **Step 5: Commit** — `feat(scrollbar): mode-aware visibility + right-edge dwell (Auto)`.

---

### Task 4: Status-line visibility (Auto = dwell OR message-force) + idle-calm paint

**Files:**
- Modify: `wordcartel/src/app.rs` (add `recompute_status_line`; call it beside `recompute_menu_bar` in the run loop / reduce deadline path — grep for the `recompute_menu_bar(` call site and add the sibling call)
- Modify: `wordcartel/src/mouse.rs` (bottom-row dwell arm, mirror of Task 3)
- Modify: `wordcartel/src/render.rs` (status section `:753-799` — idle-calm branch)
- Test: inline in `app.rs` + `render.rs`

**Interfaces:**
- Consumes: `Editor.status_line_mode`, MouseState status dwell fields, `TransientMode`.
- Produces: `pub fn status_line_visible(&Editor) -> bool` (helper for render); `recompute_status_line(&mut Editor, now_ms)`; a bottom-row dwell arm.

**Design note (no-silent-UI):** "visible" here means *the idle info line* (`[i/n] name [mode]`) is painted. A message (`!editor.status.is_empty()`), an active `prompt`/`search`/`minibuffer` ALWAYS paint regardless of mode — those branches already exist in `render.rs:753-769` and are untouched. Only the final `else` (normal idle info line) becomes conditional.

- [ ] **Step 1: Write the failing tests.**

`app.rs`:
```rust
#[test]
fn status_line_visible_forces_on_message_even_in_auto() {
    use crate::config::TransientMode;
    let mut e = Editor::new_from_text("x\n", None, (40, 8));
    e.status_line_mode = TransientMode::Auto;
    e.mouse.status_revealed = false;
    e.status.clear();
    assert!(!status_line_visible(&e), "Auto idle + no message → info line hidden (calm)");
    e.status = "saved".into();
    assert!(status_line_visible(&e), "a message force-reveals even under Auto (no-silent-UI)");
    e.status.clear();
    e.mouse.status_revealed = true;
    assert!(status_line_visible(&e), "Auto + dwell-revealed → visible");
    e.status_line_mode = TransientMode::On;
    e.mouse.status_revealed = false;
    assert!(status_line_visible(&e), "On → always visible");
}
```

- [ ] **Step 2: Run to verify fail** — FAIL (`status_line_visible` undefined).

- [ ] **Step 3: Implement.**

Add to `app.rs` (near `recompute_menu_bar`):
```rust
/// Whether the NORMAL idle status info line should paint. A message / prompt /
/// search / minibuffer force it regardless of mode (no-silent-UI) — those are
/// handled in render.rs before this is consulted; this governs only the idle line.
pub fn status_line_visible(editor: &crate::editor::Editor) -> bool {
    use crate::config::TransientMode;
    match editor.status_line_mode {
        TransientMode::On  => true,
        // Off is never assigned to status (coerced to Auto at parse); treat defensively as Auto.
        TransientMode::Off | TransientMode::Auto =>
            !editor.status.is_empty()
                || editor.mouse.status_revealed
                || editor.prompt.is_some()
                || editor.search.is_some()
                || editor.minibuffer.is_some(),
    }
}

/// Fire the Auto-mode status dwell/grace deadlines (armed by the mouse Moved arm).
pub fn recompute_status_line(editor: &mut crate::editor::Editor, now_ms: u64) {
    use crate::config::TransientMode;
    if editor.status_line_mode != TransientMode::Auto {
        editor.mouse.status_reveal_due = None;
        editor.mouse.status_hide_due = None;
        return;
    }
    if editor.mouse.status_reveal_due.is_some_and(|d| now_ms >= d) {
        editor.mouse.status_reveal_due = None;
        editor.mouse.status_revealed = true;
    }
    if editor.mouse.status_hide_due.is_some_and(|d| now_ms >= d) {
        editor.mouse.status_hide_due = None;
        editor.mouse.status_revealed = false;
    }
}
```
Wire `recompute_status_line(editor, now_ms)` at BOTH recompute sites: beside `recompute_menu_bar` at `app.rs:1238` (reduce), and beside `recompute_scrollbar_visible` at the startup call `app.rs:1533` (that startup path currently calls only `recompute_scrollbar_visible` — add `recompute_status_line` there so the first frame's status state is correct).

Bottom-row dwell arm in `mouse.rs` (mirror Task 3, gated on `status_line_mode == Auto`; `at_bottom = ev.row == h-1`):
```rust
    if editor.status_line_mode == crate::config::TransientMode::Auto {
        if let MouseEventKind::Moved = ev.kind {
            let h = editor.active().view.area.1;
            let at_bottom = h > 0 && ev.row == h - 1;
            if at_bottom {
                editor.mouse.status_hide_due = None;
                if !editor.mouse.status_revealed
                    && editor.mouse.status_reveal_due.is_none()
                    && no_overlay_open(editor)
                {
                    editor.mouse.status_reveal_due = Some(clock.now_ms() + MENU_DWELL_MS);
                }
            } else {
                editor.mouse.status_reveal_due = None;
                if editor.mouse.status_revealed && editor.mouse.status_hide_due.is_none() {
                    editor.mouse.status_hide_due = Some(clock.now_ms() + MENU_LEAVE_GRACE_MS);
                }
            }
        }
    }
```

Render idle-calm branch (`render.rs:753-799`). The chain picks `(status_text, status_style)` from search / minibuffer / prompt / normal (`:769`), then composes a right-aligned word-count segment in the `!has_overlay` branch (`:776-787`). **Two changes (Codex plan gate Critical — the calm branch must use a base-canvas style, NOT `cs.menu_closed` chrome, and must suppress the word-count segment too):**

1. Replace the final `else` (normal-state, `:768-770`) to pick a calm base-canvas style when hidden:
```rust
        } else {
            // Normal state. Under zen/Auto idle with no message, the reserved row renders
            // as calm canvas (base bg); visible reveal via On / dwell / message force.
            if crate::app::status_line_visible(editor) {
                (status_left_text(editor), cs.menu_closed) // visible: [Chrome] panel bg
            } else {
                // Calm canvas: the same bg-only fill the edit band uses — NOT chrome.
                let mut calm = compose::base_canvas(&editor.theme, editor.depth);
                calm.fg = None;
                (String::new(), calm)
            }
        };
```
2. Suppress the word-count composer when hidden, so `Ln/Col · words` does not paint over the calm row. Gate the composer (`:776`):
```rust
        let status_hidden = !has_overlay && !crate::app::status_line_visible(editor);
        let composed = if !has_overlay && !status_hidden {
            /* existing word_count_segment compose block, unchanged */
        } else {
            status_text.chars().take(w as usize).collect()
        };
```
`compose::base_canvas` is the same helper the opaque edit band uses at `render.rs:380`. `edit_height` (`render.rs:362`) is untouched — the row stays reserved; only its paint changes (zero reflow).

- [ ] **Step 4: Run tests to verify pass** — new tests PASS; add a render test asserting the idle row is blank under Auto with no message but the info line shows under On (drive `render()` against a `TestBackend`, read the bottom row string — mirror `renders_active_prompt_on_status_row` at `render.rs:1051`).

- [ ] **Step 5: Commit** — `feat(status): Auto reveal (dwell+message-force) + idle-calm reserved row`.

---

### Task 5: `ChromeBundle` + `ZEN`/`FULL` + `apply_bundle`; `toggle_chrome` applies the bundle

**Files:**
- Create: `wordcartel/src/density.rs`
- Modify: `wordcartel/src/main.rs` or `wordcartel/src/lib.rs` (add `mod density;` — grep for the module list)
- Modify: `wordcartel/src/registry.rs` (`toggle_chrome` fn at `:544` — apply the bundle)
- Test: inline in `density.rs`

**Interfaces:**
- Consumes: `ChromeDisposition`, `MenuBarMode`, `TransientMode`, `Editor` runtime fields.
- Produces: `pub struct ChromeBundle { chrome_disposition, menu_bar, status_line, scrollbar, measure, word_count }`; `pub const ZEN`/`FULL: ChromeBundle`; `pub fn apply_bundle(&mut Editor, &ChromeBundle)`; `pub fn bundle_for(ChromeDisposition) -> &'static ChromeBundle`; `pub fn bundle_names() -> [&'static str; 2]`.

**Design note (L2/L3 readiness — reviewer-checkable):** `apply_bundle` takes `&ChromeBundle` regardless of origin; fields are additive (L3 adds `theme`/`keymap` without touching the applier's mechanism). No `if zen {…}` branching anywhere — the two presets differ ONLY in the `const` data.

- [ ] **Step 1: Write the failing tests.**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::config::{MenuBarMode, TransientMode};
    use wordcartel_core::theme::ChromeDisposition;

    #[test]
    fn zen_and_full_bundles_match_the_table() {
        assert_eq!(ZEN.chrome_disposition, ChromeDisposition::Zen);
        assert_eq!(ZEN.menu_bar, MenuBarMode::Auto);
        assert_eq!(ZEN.status_line, TransientMode::Auto);
        assert_eq!(ZEN.scrollbar, TransientMode::Auto);
        assert!(ZEN.measure);
        assert!(!ZEN.word_count);
        assert_eq!(FULL.chrome_disposition, ChromeDisposition::Full);
        assert_eq!(FULL.menu_bar, MenuBarMode::Pinned);
        assert_eq!(FULL.status_line, TransientMode::On);
        assert_eq!(FULL.scrollbar, TransientMode::On);
        assert!(!FULL.measure);
        assert!(FULL.word_count);
    }

    #[test]
    fn apply_bundle_sets_every_owned_field_and_clears_menu_dwell() {
        let mut e = Editor::new_from_text("x\n", None, (40, 8));
        e.menu_bar_mode = MenuBarMode::Hidden;
        e.mouse.menu_bar_revealed = true; // stale auto-state
        apply_bundle(&mut e, &FULL);
        assert_eq!(e.menu_bar_mode, MenuBarMode::Pinned);
        assert_eq!(e.status_line_mode, TransientMode::On);
        assert_eq!(e.scrollbar_mode, TransientMode::On);
        assert!(!e.view_opts.measure);
        assert!(e.view_opts.word_count);
        assert_eq!(e.chrome_disposition, ChromeDisposition::Full);
        assert!(!e.mouse.menu_bar_revealed, "bundle apply must clear stale menu dwell state");
    }
}
```

- [ ] **Step 2: Run to verify fail** — FAIL (module absent).

- [ ] **Step 3: Implement `density.rs`.**
```rust
//! Chrome density presets (E1). A preset is DATA — a `ChromeBundle` of
//! `element → value` applied by one general `apply_bundle` routine, never
//! `if zen {…}` branching. Two built-ins (`ZEN`, `FULL`); the shape is additive
//! so config-defined bundles (L2) and named profiles that also set theme/keymap
//! (L3) are future efforts, not rewrites.

use crate::config::{MenuBarMode, TransientMode};
use crate::editor::Editor;
use wordcartel_core::theme::ChromeDisposition;

/// The resolved density target for each preset-owned chrome element. Fields are
/// additive: L3 may add `theme`/`keymap` without changing `apply_bundle`.
#[derive(Debug, Clone, Copy)]
pub struct ChromeBundle {
    pub chrome_disposition: ChromeDisposition,
    pub menu_bar: MenuBarMode,
    pub status_line: TransientMode,
    pub scrollbar: TransientMode,
    pub measure: bool,
    pub word_count: bool,
}

/// Zen density: muted chrome, everything transient, centered measure on, word count off.
pub const ZEN: ChromeBundle = ChromeBundle {
    chrome_disposition: ChromeDisposition::Zen,
    menu_bar: MenuBarMode::Auto,
    status_line: TransientMode::Auto,
    scrollbar: TransientMode::Auto,
    measure: true,
    word_count: false,
};

/// Full density: elevated chrome, everything pinned/on, no centered measure, word count on.
pub const FULL: ChromeBundle = ChromeBundle {
    chrome_disposition: ChromeDisposition::Full,
    menu_bar: MenuBarMode::Pinned,
    status_line: TransientMode::On,
    scrollbar: TransientMode::On,
    measure: false,
    word_count: true,
};

/// The built-in bundle for a disposition — the single lookup the density command uses.
pub fn bundle_for(disp: ChromeDisposition) -> &'static ChromeBundle {
    match disp { ChromeDisposition::Zen => &ZEN, ChromeDisposition::Full => &FULL }
}

/// Enumerable selectable-bundle names (today `["zen", "full"]`); L2/L3 extend this.
pub fn bundle_names() -> [&'static str; 2] { ["zen", "full"] }

/// Apply `bundle` to `editor`, setting every preset-owned element via its existing
/// runtime field. Origin-agnostic (built-in const vs future config/registry entry).
/// Does NOT itself request a theme re-derive — the caller (`toggle_chrome`) owns
/// that so the flip + re-derive stay a single honest transition.
pub fn apply_bundle(editor: &mut Editor, bundle: &ChromeBundle) {
    editor.chrome_disposition = bundle.chrome_disposition;
    editor.menu_bar_mode = bundle.menu_bar;
    editor.status_line_mode = bundle.status_line;
    editor.scrollbar_mode = bundle.scrollbar;
    editor.view_opts.measure = bundle.measure;
    editor.view_opts.word_count = bundle.word_count;
    // Mode-transition hygiene: stale auto-state must not survive a preset change
    // (mirrors menu_bar_pin at registry.rs:446).
    editor.mouse.menu_reveal_due = None;
    editor.mouse.menu_hide_due = None;
    editor.mouse.menu_bar_revealed = false;
    editor.mouse.scrollbar_reveal_due = None;
    editor.mouse.scrollbar_hide_due = None;
    editor.mouse.scrollbar_revealed = false;
    editor.mouse.status_reveal_due = None;
    editor.mouse.status_hide_due = None;
    editor.mouse.status_revealed = false;
}
```
Register the module (`mod density;` beside the other `mod`s — grep `mod menu;` in `wordcartel/src/main.rs`/`lib.rs`).

Modify `toggle_chrome` (`registry.rs:544`): after the monochrome arm returns and the new disposition is computed (`registry.rs:553-557`), replace the bare `editor.chrome_disposition = new_disp;` with a bundle apply, keeping the existing `theme_rederive = true` and status arms:
```rust
    // Apply the whole density bundle for the new disposition (color + visibility),
    // then request the re-derive. Re-selecting a preset re-applies its bundle over
    // unsaved runtime state (spec §1.5 — runtime-clobber). rebuild for measure.
    crate::density::apply_bundle(editor, crate::density::bundle_for(new_disp));
    editor.theme_rederive = true;
    crate::derive::rebuild(editor); // measure change affects layout
```
(Keep the remaining status-message arms of `toggle_chrome` at `:559+` intact — they already set `editor.status` for the four cases. Verify the `label` line still reads `new_disp`.)

- [ ] **Step 4: Run tests to verify pass** — density tests PASS; `registry::tests::toggle_chrome_flips_and_requests_rederive` still green (it asserts flip + rederive — both still hold; the added bundle fields don't break it). If that test also asserts *only* disposition changed, update it to accept the bundle side-effects (it should assert disposition + rederive; leave visibility assertions to density tests).

- [ ] **Step 5: Commit** — `feat(density): ChromeBundle + ZEN/FULL + apply_bundle; toggle_chrome applies bundle`.

---

### Task 6: Persistence — `scrollbar`/`status_line` round-trip (diff-law)

**Files:**
- Modify: `wordcartel/src/settings.rs` (`SettingsSnapshot` `:33`; `OView` `:101`; `snapshot_of` `:142`; `runtime_snapshot` `:161`; `compute_overrides` view block `:339-374`)
- Test: inline in `settings.rs`

**Interfaces:**
- Consumes: `TransientMode`, `transient_mode_str` (Task 1), `Editor.scrollbar_mode`/`status_line_mode`.
- Produces: `SettingsSnapshot.view_scrollbar/view_status_line: TransientMode`; `OView.scrollbar/status_line: Option<String>`; two new `diff_key` entries folded into `OView`.

- [ ] **Step 1: Write the failing test** — a runtime change round-trips; an unchanged one stays absent.
```rust
#[test]
fn scrollbar_status_line_round_trip_via_diff_law() {
    use crate::config::TransientMode;
    let base = snap_with(|s| { s.view_scrollbar = TransientMode::Auto; s.view_status_line = TransientMode::Auto; });
    // Runtime diverges: scrollbar → On, status_line → On.
    let mut rt = base.clone();
    rt.view_scrollbar = TransientMode::On;
    rt.view_status_line = TransientMode::On;
    let of = compute_overrides(&rt, &base, &OverridesFile::default(), &OverridesFile::default());
    let v = of.view.expect("view section present");
    assert_eq!(v.scrollbar.as_deref(), Some("on"));
    assert_eq!(v.status_line.as_deref(), Some("on"));
    // No divergence → no keys written.
    let of2 = compute_overrides(&base, &base, &OverridesFile::default(), &OverridesFile::default());
    assert!(of2.view.is_none() || of2.view.as_ref().unwrap().scrollbar.is_none());
}
```
`snap_with` helper: clone a full valid `SettingsSnapshot` (mirror the existing `settings.rs:504` test literal that constructs one) and mutate via the closure. If a helper exists, reuse it; else add the literal from `:504` extended with the two new fields.

- [ ] **Step 2: Run to verify fail** — FAIL (fields undefined).

- [ ] **Step 3: Implement.**

`SettingsSnapshot` (`settings.rs:41`, after `view_wrap_column`):
```rust
    pub view_scrollbar: crate::config::TransientMode,
    pub view_status_line: crate::config::TransientMode,
```
`OView` (`settings.rs:107`, after `wrap_column`):
```rust
    #[serde(skip_serializing_if = "Option::is_none")] pub scrollbar:   Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub status_line: Option<String>,
```
`snapshot_of` (`settings.rs:152`, after `view_wrap_column`):
```rust
        view_scrollbar:  cfg.view.scrollbar,
        view_status_line: cfg.view.status_line,
```
`runtime_snapshot` (`settings.rs:170`, after `view_wrap_column`):
```rust
        view_scrollbar:  editor.scrollbar_mode,
        view_status_line: editor.status_line_mode,
```
`compute_overrides` view block (`settings.rs:367`, after `wrap_column` diff): add two `diff_key`s over the string form (mirror `menu_bar_str` usage at `:377`):
```rust
    let rt_sb = crate::config::transient_mode_str(runtime.view_scrollbar).to_string();
    let base_sb = crate::config::transient_mode_str(baseline.view_scrollbar).to_string();
    let scrollbar = diff_key(&rt_sb, &base_sb,
        ex_view.and_then(|v| v.scrollbar.as_ref()),
        mk_view.and_then(|v| v.scrollbar.as_ref()).is_some());
    let rt_sl = crate::config::transient_mode_str(runtime.view_status_line).to_string();
    let base_sl = crate::config::transient_mode_str(baseline.view_status_line).to_string();
    let status_line = diff_key(&rt_sl, &base_sl,
        ex_view.and_then(|v| v.status_line.as_ref()),
        mk_view.and_then(|v| v.status_line.as_ref()).is_some());
```
Extend `any_view` (`:372`) and the `OView` literal (`:374`):
```rust
    let any_view = typewriter.is_some() || focus.is_some() || measure.is_some()
        || wrap_guide.is_some() || word_count.is_some() || wrap_column.is_some()
        || scrollbar.is_some() || status_line.is_some();
    let view = some_if(OView { typewriter, focus, measure, wrap_guide, word_count, wrap_column, scrollbar, status_line }, any_view);
```
Update any other `OView { … }` literal / `SettingsSnapshot { … }` literal in tests (`settings.rs:504`) to include the new fields (compiler will flag each).

- [ ] **Step 4: Run tests to verify pass** — new test PASS; full `cargo test -p wordcartel --lib settings::` green (fix any literal-construction compile errors the new fields introduce).

- [ ] **Step 5: Commit** — `feat(settings): persist [view] scrollbar/status_line via diff-law`.

---

## Part 2 — E2: visual polish

### Task 7: State-in-label menu items (`CommandMeta.state` + `MenuMark`)

**Files:**
- Modify: `wordcartel/src/registry.rs` (`CommandMeta` `:45`; `register` `:64`; add a `register_stateful`; wire stateful commands; add `keymap_next`; demote `keymap_cua`/`keymap_wordstar` to `menu: None`)
- Modify: `wordcartel/src/menu.rs` (`grouped_commands` `:30` → take `&Editor`; label composition; callers `build`/`empty` in `menu.rs`, `app.rs::hydrate_overlays:138`)
- Test: inline in `registry.rs` + `menu.rs`

**Interfaces:**
- Produces: `pub enum MenuMark { OnOff(bool), Value(&'static str) }`; `CommandMeta.state: Option<fn(&Editor) -> MenuMark>`; `grouped_commands(reg, keymap, editor)`; `menu::build(reg, keymap, editor)`.
- Consumes: live `Editor` state.

**Design decision (radio-collapse mechanism — flagged for the Codex plan gate):** the spec's "radio groups collapse to a single `Keymap: CUA` row" is implemented by adding ONE cycle command `keymap_next` (Settings menu, state `Value(active_preset_upper)`) that advances to the next preset in `["cua","wordstar"]`, and setting `keymap_cua`/`keymap_wordstar` to `menu: None` (they remain palette + keybinding commands, just not menu rows). This keeps the 1-command-per-row model intact. Stateful commands wired: `toggle_measure`, `toggle_wrap_guide`, `toggle_word_count` (OnOff); `toggle_chrome`, `toggle_canvas`, `keymap_next`, `menu_bar_pin` (Value).

- [ ] **Step 1: Write the failing tests.**

`registry.rs`:
```rust
#[test]
fn stateful_commands_report_live_state() {
    let reg = Registry::builtins();
    let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
    ed.view_opts.word_count = false;
    let m = reg.meta(CommandId("toggle_word_count")).unwrap();
    let f = m.state.expect("toggle_word_count has a state fn");
    assert!(matches!(f(&ed), MenuMark::OnOff(false)));
    ed.view_opts.word_count = true;
    assert!(matches!(f(&ed), MenuMark::OnOff(true)));
    // Chrome is a Value mark.
    let cm = reg.meta(CommandId("toggle_chrome")).unwrap().state.unwrap();
    ed.chrome_disposition = wordcartel_core::theme::ChromeDisposition::Zen;
    assert!(matches!(cm(&ed), MenuMark::Value("Zen")));
}

#[test]
fn keymap_group_collapses_to_one_cycle_row() {
    let reg = Registry::builtins();
    // keymap_cua/keymap_wordstar are palette-only now (menu: None).
    assert_eq!(reg.meta(CommandId("keymap_cua")).unwrap().menu, None);
    assert_eq!(reg.meta(CommandId("keymap_next")).unwrap().menu, Some(MenuCategory::Settings));
}
```

`menu.rs`:
```rust
#[test]
fn menu_leaf_shows_state_in_label() {
    let reg = crate::registry::Registry::builtins();
    let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
    let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
    ed.view_opts.word_count = true;
    let groups = grouped_commands(&reg, &km, &ed);
    let view = groups.iter().find(|(c, _)| *c == crate::registry::MenuCategory::View).unwrap();
    assert!(view.1.iter().any(|(label, _)| label.starts_with("Word Count: On")),
        "stateful toggle renders 'Word Count: On', got {:?}", view.1);
}
```

- [ ] **Step 2: Run to verify fail** — FAIL.

- [ ] **Step 3: Implement.**

`registry.rs` — `MenuMark` + `CommandMeta.state`:
```rust
/// The live-state mark a stateful menu command interpolates into its row label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuMark { OnOff(bool), Value(&'static str) }

#[derive(Clone, Copy)]
pub struct CommandMeta {
    pub label: &'static str,
    pub menu: Option<MenuCategory>,
    /// Optional live-state provider — evaluated at menu-build time against `&Editor`.
    /// `None` for stateless commands (their static label renders unchanged).
    pub state: Option<fn(&crate::editor::Editor) -> MenuMark>,
}
```
Update `register` (`:64`) to set `state: None`, and add a `register_stateful`:
```rust
    fn register(&mut self, id: &'static str, label: &'static str, menu: Option<MenuCategory>, handler: Handler) {
        let cid = CommandId(id);
        self.index.insert(cid, self.entries.len());
        self.entries.push(CommandEntry { id: cid, handler, meta: CommandMeta { label, menu, state: None } });
    }
    fn register_stateful(&mut self, id: &'static str, label: &'static str, menu: Option<MenuCategory>,
                         state: fn(&crate::editor::Editor) -> MenuMark, handler: Handler) {
        let cid = CommandId(id);
        self.index.insert(cid, self.entries.len());
        self.entries.push(CommandEntry { id: cid, handler, meta: CommandMeta { label, menu, state: Some(state) } });
    }
```
Convert the stateful registrations. Examples:
```rust
        r.register_stateful("toggle_measure", "Toggle Centered Measure", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.measure),
            |c| { c.editor.view_opts.measure = !c.editor.view_opts.measure; crate::derive::rebuild(c.editor); CommandResult::Handled });
        r.register_stateful("toggle_wrap_guide", "Toggle Wrap Guide", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.wrap_guide),
            |c| { c.editor.view_opts.wrap_guide = !c.editor.view_opts.wrap_guide; CommandResult::Handled });
        r.register_stateful("toggle_word_count", "Toggle Word Count", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.word_count),
            |c| { c.editor.view_opts.word_count = !c.editor.view_opts.word_count; CommandResult::Handled });
        r.register_stateful("toggle_chrome", "Chrome: Full/Zen", Some(MenuCategory::Settings),
            |e| MenuMark::Value(match e.chrome_disposition {
                wordcartel_core::theme::ChromeDisposition::Full => "Full",
                wordcartel_core::theme::ChromeDisposition::Zen => "Zen" }),
            |c| { toggle_chrome(c.editor); CommandResult::Handled });
        r.register_stateful("toggle_canvas", "Canvas: Opaque/Transparent", Some(MenuCategory::Settings),
            |e| MenuMark::Value(match e.canvas {
                wordcartel_core::theme::CanvasMode::Opaque => "Opaque",
                wordcartel_core::theme::CanvasMode::Transparent => "Transparent" }),
            |c| { toggle_canvas(c.editor); CommandResult::Handled });
        r.register_stateful("menu_bar_pin", "Pin Menu Bar", Some(MenuCategory::View),
            |e| MenuMark::Value(match e.menu_bar_mode {
                crate::config::MenuBarMode::Pinned => "Pinned",
                crate::config::MenuBarMode::Auto => "Auto",
                crate::config::MenuBarMode::Hidden => "Hidden" }),
            |c| { /* existing menu_bar_pin body */ CommandResult::Handled });
```
(Keep the existing `menu_bar_pin` handler body verbatim inside the closure.)

Demote keymap radios + add cycle:
```rust
        r.register("keymap_cua", "Keymap: CUA", None, |c| { switch_keymap_preset(c.editor, "cua"); CommandResult::Handled });
        r.register("keymap_wordstar", "Keymap: WordStar", None, |c| { switch_keymap_preset(c.editor, "wordstar"); CommandResult::Handled });
        r.register_stateful("keymap_next", "Keymap", Some(MenuCategory::Settings),
            |e| MenuMark::Value(if e.active_keymap_preset == "wordstar" { "WordStar" } else { "CUA" }),
            |c| {
                let next = if c.editor.active_keymap_preset == "cua" { "wordstar" } else { "cua" };
                switch_keymap_preset(c.editor, next);
                CommandResult::Handled
            });
```

`menu.rs` — thread `&Editor` and compose labels:
```rust
pub fn build(reg: &Registry, keymap: &KeyTrie, editor: &crate::editor::Editor) -> MenuView {
    MenuView { groups: grouped_commands(reg, keymap, editor), open: 0, highlighted: 0, built: true }
}

fn grouped_commands(reg: &Registry, keymap: &KeyTrie, editor: &crate::editor::Editor)
    -> Vec<(MenuCategory, Vec<(String, CommandId)>)> {
    let mut groups = Vec::new();
    for cat in MENU_ORDER {
        let mut leaves: Vec<(String, CommandId)> = reg.commands()
            .filter_map(|(id, meta)| {
                if meta.menu == Some(cat) && id != CommandId("palette") {
                    Some((menu_leaf_label(meta, editor, keymap.chord_for(id)), id))
                } else { None }
            })
            .collect();
        if cat == MenuCategory::View && reg.meta(CommandId("palette")).is_some() {
            leaves.push((leaf_label("Command Palette...", keymap.chord_for(CommandId("palette"))), CommandId("palette")));
        }
        if !leaves.is_empty() { groups.push((cat, leaves)); }
    }
    groups
}

/// Compose a menu leaf label, interpolating live state for stateful commands.
/// Stateless → the static label + chord. Stateful → `"{base}: {value}"` + chord,
/// where `base` strips a leading "Toggle " and any "…: variants" suffix.
fn menu_leaf_label(meta: &crate::registry::CommandMeta,
                   editor: &crate::editor::Editor, chord: Option<String>) -> String {
    use crate::registry::MenuMark;
    let text = match meta.state {
        None => meta.label.to_string(),
        Some(f) => {
            let base = meta.label.strip_prefix("Toggle ").unwrap_or(meta.label);
            let base = base.split(':').next().unwrap_or(base).trim();
            match f(editor) {
                MenuMark::OnOff(b) => format!("{base}: {}", if b { "On" } else { "Off" }),
                MenuMark::Value(v) => format!("{base}: {v}"),
            }
        }
    };
    leaf_label(&text, chord)
}
```
Update callers of `build`/`grouped_commands`: `app.rs::hydrate_overlays` (`:138`) `crate::menu::build(reg, keymap)` → `crate::menu::build(reg, keymap, editor)` (the `&Editor` is available there — note `hydrate_overlays` borrows `editor` mutably for the palette arm; compute the built menu against an immutable borrow first, as Codex advised: build the `MenuView` from `&*editor` before the `editor.menu = Some(built)` assignment). Adjust the menu-build block:
```rust
    if editor.menu.as_ref().is_some_and(|v| !v.built) {
        let (want_open, want_hl) = { let v = editor.menu.as_ref().unwrap(); (v.open, v.highlighted) };
        let mut built = crate::menu::build(reg, keymap, editor); // immutable borrow of editor
        if let Some(cat) = crate::registry::MENU_ORDER.get(want_open) {
            if let Some(pos) = built.groups.iter().position(|g| g.0 == *cat) { built.open = pos; }
        }
        built.highlighted = want_hl.min(built.groups.get(built.open).map_or(0, |g| g.1.len().saturating_sub(1)));
        editor.menu = Some(built);
    }
```
Update `menu.rs` test helpers (`grouped_commands(...)` calls at `:76`/`:83`, `build(&reg,&keymap)` at `:100`) to pass a throwaway `&Editor`.

- [ ] **Step 4: Run tests to verify pass** — new tests PASS; `menu::tests::build_groups_by_category_in_order_with_chords_and_palette_entry` updated + green; `registry::tests` for the keymap/settings menu categories updated (the Settings-menu label list at `registry.rs:801` now includes `keymap_next`, excludes `keymap_cua`/`keymap_wordstar` — update that assertion).

- [ ] **Step 5: Commit** — `feat(menu): state-in-label rows + keymap radio-collapse (keymap_next)`.

---

### Task 8: Two-archetype styling — attached filled-panel dropdown

**Files:**
- Modify: `wordcartel/src/render_overlays.rs` (menu dropdown paint `:316-332`)
- Test: inline in `render_overlays.rs` (or an `e2e`-style `TestBackend` assertion)

**Interfaces:**
- Consumes: `ChromeStyles` (`menu_norm`/`menu_sel` = ChromeMuted/ChromeSelected), `menu_dropdown_rect`.
- Produces: dropdown painted as a filled elevated panel (Muted bg fill, no box border), reading as extending down from the bar. Floating overlays are UNCHANGED (still bordered — they use `overlay_border`).

**Design note:** No new styling primitives (E3 shipped the six-face family). The dropdown already renders a `List` with `menu_norm`/`menu_sel` styles onto `drop_rect` after `Clear`. This task makes the fill explicit (a `set_style(drop_rect, cs.menu_norm)` over the whole rect after `Clear`, so gaps read as one panel) and ensures no border block is drawn. The indicator row from Task 14 lands on the bottom row later.

- [ ] **Step 1: Write the failing test** — the dropdown rect's background is the ChromeMuted panel bg (filled), not the raw terminal default.

Drive a small `render()` (or the dropdown paint directly) against a `TestBackend`, open a menu category, and assert a cell inside `drop_rect` but past the last leaf label carries the panel bg (`cs.menu_norm.bg`). Mirror the `TestBackend` setup in `render.rs` tests (`renders_active_prompt_on_status_row` at `render.rs:1051`). Assert:
```rust
// after painting a menu with an open category on an 80x24 backend:
let bg_at = |x: u16, y: u16| backend.buffer().get(x, y).style().bg;
assert_eq!(bg_at(drop_x, drop_bottom), panel_bg,
    "dropdown paints a filled panel to its bottom row (no unfilled gap)");
```

- [ ] **Step 2: Run to verify fail** — FAIL (only per-item styles paint; the rect isn't filled edge-to-edge).

- [ ] **Step 3: Implement.** In the dropdown paint (`render_overlays.rs:316`), after `Clear` and before/around the `List`:
```rust
                if let Some(drop_rect) = menu_dropdown_rect(menu_area, &menu.groups, menu.open) {
                    frame.render_widget(Clear, drop_rect);
                    // Attached filled panel: fill the whole rect with the Muted panel bg so
                    // the dropdown reads as one elevated surface extending from the bar (no box).
                    frame.buffer_mut().set_style(drop_rect, cs.menu_norm);
                    let leaves = &menu.groups[menu.open].1;
                    /* existing item list build, unchanged */
                    frame.render_widget(List::new(items), drop_rect);
                }
```
Confirm no `Block::default().borders(...)` is added to the dropdown (it never was — the border archetype belongs to floating overlays only). Leave the floating overlays (palette/theme/file/outline/diag) bordered as-is.

- [ ] **Step 4: Run tests to verify pass** — PASS; visually confirm via smoke (advisory) that the dropdown looks like a filled panel.

- [ ] **Step 5: Commit** — `feat(render): attached filled-panel dropdown (two-archetype styling)`.

---

## Part 3 — overlay/mouse completeness

### Task 9: Universal no-leak guard + dwell ordering (mouse.rs restructure)

**Files:**
- Modify: `wordcartel/src/mouse.rs` (`handle` `:72-231` — restructure)
- Test: inline in `mouse.rs`

**Interfaces:**
- Consumes: `no_overlay_open` (Task 3).
- Produces: an overlay-open route placed BEFORE the dwell-arming block; new consuming branches for `prompt`/`minibuffer`/`outline`/`diag`/`search` (behavior added in Tasks 10-13; this task adds the *consume* so nothing leaks and dwell is suppressed).

**Design (spec ordering requirement):** Restructure `handle` so the sequence is: (1) `pending_mark`/`!mouse_capture` early return (unchanged, `:81`); (2) universal Up(Left) drag-clear (unchanged, `:90`); (3) **overlay routing** — `if !no_overlay_open(editor) { route_overlay(...); return; }` placed BEFORE (4) the dwell-arming block; (5) the editor fall-through match. The dwell-arming block therefore only runs when NO overlay is open — so no modal can arm the menu-bar/scrollbar/status dwell. Move the existing palette/menu/theme_picker/file_browser branch bodies into `route_overlay`, then add the new overlay arms.

- [ ] **Step 1: Write the failing tests** — dwell never arms with a prompt/minibuffer/search/outline/diag open; a click over each is consumed (caret does not move).
```rust
#[test]
fn dwell_never_arms_under_any_modal() {
    let modal_setups: Vec<(&str, fn(&mut Editor))> = vec![
        ("prompt", |e| e.prompt = Some(crate::prompt::Prompt::quit_confirm())),
        ("minibuffer", |e| e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter)),
        ("search", |e| e.search = Some(crate::search_overlay::SearchState::open(
            crate::search_overlay::Phase::Find, 0, crate::editor::BufferId(1)))),
        ("outline", |e| e.open_outline()),
        // diag: construct a DiagOverlay directly (mirror diag_overlay.rs tests).
    ];
    for (name, setup) in modal_setups {
        let mut e = Editor::new_from_text("hello\n", None, (40, 8));
        crate::derive::rebuild(&mut e);
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        setup(&mut e);
        let (reg, ex, _, tx, km) = ctx();
        handle(&mut e, moved(5, 0), &reg, &km, &ex, &TestClock(0), &tx);
        assert!(e.mouse.menu_reveal_due.is_none(), "{name}: modal open must suppress menu dwell");
    }
}

#[test]
fn click_under_prompt_is_consumed_not_leaked_to_editor() {
    let mut e = Editor::new_from_text("abcdef\n", None, (40, 8));
    crate::derive::rebuild(&mut e);
    e.prompt = Some(crate::prompt::Prompt::quit_confirm());
    let (reg, ex, clk, tx, km) = ctx();
    handle(&mut e, down(3, 4), &reg, &km, &ex, &clk, &tx);
    assert_eq!(crate::nav::head(&e), 0, "click must not move the caret while a prompt is open");
}
```

- [ ] **Step 2: Run to verify fail** — FAIL (prompt/search have no branch → click leaks to editor, moves caret; dwell arms).

- [ ] **Step 3: Implement.** Extract the four existing overlay branch bodies (`:122-230`) into:
```rust
/// Route a mouse event to the open overlay layer. PRECONDITION: at least one overlay
/// is open (`!no_overlay_open`). Consumes the event (the caller returns unconditionally
/// after this). Text-input modals (minibuffer/search/prompt for non-choice clicks)
/// consume without acting; list overlays scroll/click/click-away (Tasks 10-13).
fn route_overlay(editor: &mut Editor, ev: MouseEvent, area: ratatui::layout::Rect,
                 reg: &crate::registry::Registry, keymap: &crate::keymap::KeyTrie,
                 ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
                 msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
    if editor.palette.is_some() { /* existing palette body (:122-172, minus the trailing `return`) */ return; }
    if editor.menu.is_some() { /* existing menu body */ return; }
    if editor.theme_picker.is_some() { /* Task 10 body */ return; }
    if editor.file_browser.is_some() { /* Task 10 body */ return; }
    if editor.outline.is_some() { /* Task 11 body */ return; }
    if editor.diag.is_some() { /* Task 12 body */ return; }
    if editor.prompt.is_some() { /* Task 13 body */ return; }
    // Text-input modals: consume, no row action (you type).
    if editor.minibuffer.is_some() || editor.search.is_some() { return; }
}
```
In `handle`, after the Up(Left) drag-clear (`:93`) and BEFORE the dwell block (`:98`):
```rust
    let (w, h) = editor.active().view.area;
    let area = ratatui::layout::Rect::new(0, 0, w, h);
    if !no_overlay_open(editor) {
        route_overlay(editor, ev, area, reg, keymap, ex, clock, msg_tx);
        return;
    }
    // ── from here down: no overlay open — dwell arming + editor gestures ──
```
Delete the now-migrated overlay branches (`:122-230`) and the duplicate `let (w, h) …; let area …;` at `:120-121`. The dwell block (`:98-119`) and the editor match (`:231`) now run only when `no_overlay_open`. This lets Task 3/4's `no_overlay_open()` gate in the dwell arms simplify (they're now only reached when no overlay is open — but keep the `no_overlay_open` guard in those arms as defense-in-depth; it's cheap and keeps them correct if the ordering ever changes). NOTE: preserve the universal Up(Left) drag-clear at `:90` ABOVE the overlay route so `overlay_open_mid_drag_up_clears_drag_state` (`:673`) still passes.

- [ ] **Step 4: Update the one intentionally-changed dwell test.** Moving overlay routing before the dwell block changes ONE existing invariant (Codex plan gate Critical): `leave_bookkeeping_runs_while_dropdown_open` (Case 6, `mouse.rs:796-806`) asserts the menu-bar leave-grace arms *while the dropdown is open*. Under the new ordering, dwell does NOT run while any overlay (incl. the dropdown) is open — that is exactly the no-leak guarantee, and it self-heals (closing the dropdown re-enables dwell; the next row>0 move arms the grace). **Replace** Case 6 with an assertion of the NEW invariant:
```rust
/// New invariant (overlay-route-before-dwell): leave-bookkeeping does NOT run while
/// the dropdown is open — dwell is suppressed for every open overlay (no-leak guard).
#[test]
fn dwell_suppressed_while_dropdown_open() {
    let mut e = Editor::new_from_text("hello\n", None, (40, 8));
    crate::derive::rebuild(&mut e);
    e.menu_bar_mode = crate::config::MenuBarMode::Auto;
    e.mouse.menu_bar_revealed = true;
    e.menu = Some(crate::menu::empty_at(0)); // dropdown open
    let (reg, ex, _, tx, km) = ctx();
    handle(&mut e, moved(5, 5), &reg, &km, &ex, &TestClock(0), &tx);
    assert!(e.mouse.menu_hide_due.is_none(),
        "dwell (incl. leave-bookkeeping) must not run while an overlay is open");
}
```

- [ ] **Step 5: Run tests to verify pass** — the new Task 9 tests PASS; the REST of `mouse::tests` green (palette dispatch, buffer-switch, theme-picker/file-browser wheel, drag-clear, and the rest of the dwell table — Cases 1-5 + 9 are unaffected because they exercise the no-overlay-open path; only Case 6 changes, updated above). `overlay_open_mid_drag_up_clears_drag_state` (`mouse.rs:673`) MUST still pass — the universal Up(Left) drag-clear stays ABOVE the overlay route.

- [ ] **Step 6: Commit** — `refactor(mouse): overlay route before dwell; consume-guard all modals (no leak)`.

---

### Task 10: Theme picker + file browser — click-to-commit + click-away

**Files:**
- Modify: `wordcartel/src/render.rs` (add `theme_picker_row_at`, `file_browser_row_at` hit-tests, mirroring `palette_row_at` `:198`)
- Modify: `wordcartel/src/mouse.rs` (`route_overlay` theme_picker/file_browser arms — add Down(Left))
- Test: inline in `mouse.rs`

**Interfaces:**
- Consumes: `palette_overlay_rect`, `list_h_for`, the picker structs' `scroll_top`/`selected`/`rows`/`entries`.
- Produces: `render::theme_picker_row_at(area, tp, col, row) -> Option<usize>`; `render::file_browser_row_at(area, fb, col, row) -> Option<usize>`.

- [ ] **Step 1: Write the failing tests** — click a theme row applies it; click a dir entry enters it; click outside closes.
```rust
#[test]
fn click_theme_row_applies_and_closes() {
    let mut e = Editor::new_from_text("# H\n\n", None, (80, 24));
    crate::derive::rebuild(&mut e);
    e.open_theme_picker();
    let names = wordcartel_core::theme::Theme::builtin_names();
    let target = names.iter().position(|n| *n == "tokyo-night").unwrap();
    let area = ratatui::layout::Rect::new(0, 0, 80, 24);
    let rect = crate::render::palette_overlay_rect(area, e.theme_picker.as_ref().unwrap().rows.len());
    let click_row = rect.y + 2 + target as u16; // list starts ov_y+2 (scroll_top 0)
    let (reg, ex, clk, tx, km) = ctx();
    let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: click_row, modifiers: KeyModifiers::NONE };
    handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
    assert!(e.theme_picker.is_none(), "picker closes on row click");
    assert_eq!(e.theme.name, "tokyo-night", "clicked theme applied");
}
```
(Add a symmetric `click_outside_theme_picker_closes` and a file-browser `click_dir_enters` test — file browser: assert `fb.dir` changed into the clicked directory.)

- [ ] **Step 2: Run to verify fail** — FAIL (theme_picker arm is scroll-only).

- [ ] **Step 3: Implement.** Theme picker + file browser ALREADY have `scroll_top` + windowed render (`render_overlays.rs:158`/`:230` — Codex plan gate) — this task is **hit-test + mouse-arm only**, no render groundwork. Add hit-tests in `render.rs` (mirror `palette_row_at`):
```rust
pub(crate) fn theme_picker_row_at(area: Rect, tp: &crate::theme_picker::ThemePicker, col: u16, row: u16) -> Option<usize> {
    let r = palette_overlay_rect(area, tp.rows.len());
    let list_top = r.y.saturating_add(2);
    let list_h = crate::list_window::list_h_for(tp.rows.len(), area.height) as u16;
    if col >= r.x.saturating_add(1) && col < r.x.saturating_add(r.width).saturating_sub(1)
        && row >= list_top && row < list_top.saturating_add(list_h) {
        Some((row - list_top) as usize + tp.scroll_top)
    } else { None }
}
// file_browser_row_at: identical shape over fb.entries.len()/fb.scroll_top.
```
Extend the theme_picker arm in `route_overlay` with a Down(Left) case that applies the clicked row (set `selected`, call `crate::app::preview_selected_theme`, then commit the identity exactly as the keyboard Enter arm does — grep the Enter arm in `app.rs` that consumes `tp.previewed`) and closes; and a click-away (outside `palette_overlay_rect`) that restores `tp.original` and closes (mirror the Esc path). File browser Down(Left): apply the clicked entry via the same code path the keyboard Enter uses (grep the file-browser Enter handler — it enters a dir or opens a file, including the unreadable-dir guard tested at `file_browser.rs:84`). Reuse that handler; do not duplicate the fs logic.

- [ ] **Step 4: Run tests to verify pass** — new tests PASS; existing `tp_wheel_*`/`fb_wheel_*` green.

- [ ] **Step 5: Commit** — `feat(mouse): theme-picker + file-browser click-to-commit + click-away`.

---

### Task 11: Outline — scroll + click + click-away

**Files:**
- Modify: `wordcartel/src/render.rs` (`outline_row_at`)
- Modify: `wordcartel/src/mouse.rs` (`route_overlay` outline arm — full set)
- Test: inline in `mouse.rs`

**Interfaces:**
- Consumes: `OutlineOverlay` (`scroll_top`/`selected`/`rows`), the outline Enter handler (jump-to-heading).
- Produces: `render::outline_row_at`; outline arm with ScrollUp/Down + Down(Left) commit + click-away.

- [ ] **Step 1: Write the failing test** — click an outline row jumps the caret to that heading + closes; wheel moves selection.
```rust
#[test]
fn click_outline_row_jumps_and_closes() {
    let mut e = Editor::new_from_text("# A\n\n## B\n\nbody\n", None, (80, 24));
    crate::derive::rebuild(&mut e);
    e.open_outline();
    let area = ratatui::layout::Rect::new(0, 0, 80, 24);
    let rows_len = e.outline.as_ref().unwrap().rows.len();
    let rect = crate::render::palette_overlay_rect(area, rows_len);
    let click_row = rect.y + 2 + 1; // second heading "## B"
    let target_byte = e.outline.as_ref().unwrap().rows[1].byte;
    let (reg, ex, clk, tx, km) = ctx();
    let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: click_row, modifiers: KeyModifiers::NONE };
    handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
    assert!(e.outline.is_none(), "outline closes on click");
    assert_eq!(crate::nav::head(&e), target_byte, "caret jumps to the clicked heading");
}
```

- [ ] **Step 2: Run to verify fail** — FAIL (outline has no mouse arm).

- [ ] **Step 3: Implement.** Outline ALREADY has `scroll_top` + windowed render (`render_overlays.rs:118`) — this task is **mouse-only** (no render groundwork). Add `outline_row_at` in `render.rs` (same shape over `outline.rows.len()`/`outline.scroll_top`). Outline arm in `route_overlay`: ScrollUp/Down (move `selected`, `keep_overlay_visible`), Down(Left) inside → set `selected`, then jump via `crate::app::outline_jump_to(editor, byte)` (`app.rs:276`). **`outline_jump_to` does NOT itself version-guard** (Codex plan gate) — replicate the keyboard Enter arm's stale guard (`app.rs:~1013`, compares `outline.opened_version` to the live `document.version` and refuses if changed) BEFORE calling `outline_jump_to`; then close. Down(Left) outside → close.

- [ ] **Step 4: Run tests to verify pass** — new test + a wheel test PASS.

- [ ] **Step 5: Commit** — `feat(mouse): outline scroll + click-to-jump + click-away`.

---

### Task 12: Diag — add `scroll_top` + windowing + scroll/click/click-away

**Files:**
- Modify: `wordcartel/src/diag_overlay.rs` (`DiagOverlay.scroll_top` + `up`/`down` keep-visible awareness)
- Modify: `wordcartel/src/render_overlays.rs` (diag paint `:364-390` — window by `scroll_top`)
- Modify: `wordcartel/src/render.rs` (`diag_row_at`)
- Modify: `wordcartel/src/mouse.rs` (`route_overlay` diag arm — full set)
- Test: inline in `diag_overlay.rs` + `mouse.rs`

**Interfaces:**
- Produces: `DiagOverlay.scroll_top: usize`; `render::diag_row_at`; diag arm with scroll/click/click-away. Windowed paint replaces the inline `.min(15)` cap.

- [ ] **Step 1: Write the failing tests** — a tall diag scrolls (selection stays visible) and a click applies the row.
```rust
// diag_overlay.rs
#[test]
fn diag_down_keeps_selection_in_window() {
    let mut d = tall_diag(30); // helper: a Diagnostic with 28 suggestions → 30 rows
    d.scroll_top = 0;
    for _ in 0..20 { d.down(24); } // down(area_h) keeps window
    assert!(d.selected.saturating_sub(d.scroll_top) < crate::list_window::list_h_for(d.row_count(), 24),
        "selection stays inside the window after scrolling");
}
```
(Note: change `up`/`down` to take `area_h` and call `keep_visible`, OR keep them arg-free and window in the paint + mouse layer via `keep_overlay_visible`. The cleaner choice — matching the other overlays — is to window in the paint/mouse layer via `keep_overlay_visible`, leaving `up`/`down` unchanged. Adopt THAT: no signature change to `up`/`down`; the paint calls `keep_overlay_visible(h, selected, row_count, &mut scroll_top)` like the palette does at `render_overlays.rs:45`. Rewrite the test accordingly to drive the paint or `keep_overlay_visible` directly.)

- [ ] **Step 2: Run to verify fail** — FAIL (`scroll_top` undefined).

- [ ] **Step 3: Implement.** Diag is the ONE list overlay that still lacks `scroll_top` + windowing (outline/theme/file already window — Codex plan gate). Add `scroll_top: usize` to `DiagOverlay` (`:10`) + its constructor (`:21`). In the diag paint (`render_overlays.rs:364`), call `crate::app::keep_overlay_visible(h, diag_ov.selected, row_count, &mut diag_ov.scroll_top)` at the top of the diag block (requires `editor.diag` as `&mut` — the `paint` fn already takes `&mut Editor`; mirror the palette arm at `:45`), replace the inline `.min(15)` list slice with a windowed slice `[scroll_top..end]` (mirror the palette slice at `render_overlays.rs:88`), and set `ListState` select to `selected - scroll_top`. `diag_row_at` in `render.rs` mirrors `palette_row_at`. Diag arm in `route_overlay`: ScrollUp/Down (move `selected`, `keep_overlay_visible`), Down(Left) inside → set `selected`, then apply via `crate::search_ui::diag_apply_selected(editor, clock)` (`search_ui.rs:133` — it ALREADY contains the `opened_version` stale guard tested at `app.rs:4029`; reuse it directly, do NOT reimplement); outside → close.

- [ ] **Step 4: Run tests to verify pass** — new tests PASS; existing diag apply tests green.

- [ ] **Step 5: Commit** — `feat(diag): scroll_top windowing + mouse scroll/click/click-away`.

---

### Task 13: Prompt choices clickable

**Files:**
- Modify: `wordcartel/src/render.rs` (add `prompt_choice_at` — hit-test over the status-row choice regions)
- Modify: `wordcartel/src/mouse.rs` (`route_overlay` prompt arm)
- Test: inline in `mouse.rs`

**Interfaces:**
- Consumes: `Prompt.choices` (`prompt.rs:36` — each `Choice { key, label, action }`), the status-row prompt render.
- Produces: `render::prompt_choice_at(area, prompt, col, row) -> Option<PromptAction>`; prompt arm dispatching the chosen `PromptAction` via the same resolver the keyboard uses.

**Design note:** the prompt renders its message on the status row (bottom). The clickable regions are the `[S]ave`/`[D]iscard`/`[C]ancel` tokens within that row. `prompt_choice_at` computes each choice's column span from the rendered message layout. Simplest robust approach: since the keyboard path maps a typed key → `Prompt::action_for(ch)`, and the message contains `[S]`, `[D]`, `[C]` markers, compute each choice's span by locating its `[X]` marker substring in the rendered status text and mapping the click column into it. Fall back to: a click anywhere on the status row while a prompt is open, over a choice marker, activates that choice; a click NOT over any marker is consumed (no-op) — the prompt stays (Esc/`C` cancels).

- [ ] **Step 1: Write the failing test** — clicking the `[Q]uit anyway` region dispatches `QuitAnyway`.
```rust
#[test]
fn click_prompt_choice_dispatches_action() {
    let mut e = Editor::new_from_text("x\n", None, (80, 8));
    crate::derive::rebuild(&mut e);
    e.prompt = Some(crate::prompt::Prompt::quit_confirm());
    let area = ratatui::layout::Rect::new(0, 0, 80, 8);
    // Locate the '[Q]' marker column in the rendered status text.
    let msg = e.prompt.as_ref().unwrap().message.clone();
    let q_col = msg.find("[Q]").expect("quit marker present") as u16;
    let (reg, ex, clk, tx, km) = ctx();
    let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: q_col + 1, row: 7, modifiers: KeyModifiers::NONE };
    handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
    assert!(e.quit, "clicking [Q]uit anyway must trigger the QuitAnyway action");
}
```
(The prompt message is rendered left-aligned at column 0 on the status row, so message byte-index ≈ column for ASCII markers. If the render offsets the message, adjust `prompt_choice_at` to add that offset; document it.)

- [ ] **Step 2: Run to verify fail** — FAIL (prompt is consume-only from Task 9).

- [ ] **Step 3: Implement.** `prompt_choice_at` in `render.rs`:
```rust
/// Map a click on the status row to a prompt choice by locating each choice's
/// `[K]` marker in the rendered message. Returns the `PromptAction` when the click
/// column falls within a choice's marker+label span; `None` otherwise.
pub(crate) fn prompt_choice_at(area: Rect, prompt: &crate::prompt::Prompt, col: u16, row: u16)
    -> Option<crate::prompt::PromptAction> {
    if row != area.y + area.height.saturating_sub(1) { return None; } // status row only
    let rel = col.saturating_sub(area.x); // message renders at column area.x (Codex plan gate)
    let msg = &prompt.message;
    for choice in &prompt.choices {
        let marker = format!("[{}]", choice.key.to_ascii_uppercase());
        if let Some(byte_idx) = msg.find(&marker) {
            let start = byte_idx as u16; // ASCII markers → byte index == column offset
            // span = the marker plus its trailing label word up to the next '·' separator.
            let rest = &msg[byte_idx..];
            let span_len = rest.find('·').unwrap_or(rest.len()) as u16;
            if rel >= start && rel < start + span_len { return Some(choice.action); }
        }
    }
    None
}
```
Prompt arm in `route_overlay`: on Down(Left), `if let Some(action) = crate::render::prompt_choice_at(area, prompt, ev.column, ev.row) { dispatch it via the same resolver app.rs uses for keyboard prompt actions }`; else consume (no-op). Grep the keyboard prompt resolver in `app.rs` (it matches `PromptAction` → effect) and call it — do not duplicate the action semantics.

- [ ] **Step 4: Run tests to verify pass** — new test PASS.

- [ ] **Step 5: Commit** — `feat(mouse): clickable prompt choices on the status row`.

---

## Part 4 — menu windowing

### Task 14: Menu dropdown windowing + indicator

**Files:**
- Modify: `wordcartel/src/menu.rs` (`MenuView.scroll_top`)
- Modify: `wordcartel/src/render.rs` (`menu_dropdown_rect` windowing `:151`; `menu_dropdown_row_at` `:162`)
- Modify: `wordcartel/src/render_overlays.rs` (windowed dropdown paint `:316` + indicator)
- Modify: `wordcartel/src/mouse.rs` (menu arm — ScrollUp/Down + keyboard keep-visible)
- Test: inline in `render.rs` + `mouse.rs`

**Interfaces:**
- Consumes: `list_window::{list_h_for, keep_visible}`, `windowed_indicator` (`render.rs:171`).
- Produces: `MenuView.scroll_top: usize`; `menu_dropdown_rect` windows height via `list_h_for` against the space below the label; the dropdown scrolls rather than truncating.

**Design note:** lands AFTER Task 8 — re-read the filled-panel dropdown paint. The indicator `" n/total "` renders on the dropdown's bottom row when the open category overflows the window.

- [ ] **Step 1: Write the failing tests** — a tall category windows; keyboard ↓ past the window scrolls; the indicator appears.
```rust
// render.rs
#[test]
fn menu_dropdown_windows_a_tall_category() {
    // Build groups where one category has > available height leaves.
    let area = Rect::new(0, 0, 80, 8); // h-1 status, h-... → small window
    let groups = tall_menu_groups(20); // helper: a category with 20 leaves
    let rect = menu_dropdown_rect(area, &groups, 0).expect("dropdown rect");
    let avail = crate::list_window::list_h_for(20, area.height);
    assert_eq!(rect.height as usize, avail.min(20),
        "dropdown height is windowed by list_h_for, not the raw leaf count");
}
```
```rust
// mouse.rs
#[test]
fn menu_wheel_scrolls_dropdown() {
    let mut e = Editor::new_from_text("x\n", None, (80, 8));
    crate::derive::rebuild(&mut e);
    e.menu = Some(crate::menu::empty_at(4)); // a category with many leaves (Settings)
    let (reg, ex, clk, tx, km) = ctx();
    crate::app::hydrate_overlays(&mut e, &reg, &km);
    let before = e.menu.as_ref().unwrap().scroll_top;
    let wheel = MouseEvent { kind: MouseEventKind::ScrollDown, column: 2, row: 3, modifiers: KeyModifiers::NONE };
    for _ in 0..10 { handle(&mut e, wheel, &reg, &km, &ex, &clk, &tx); }
    assert!(e.menu.as_ref().unwrap().highlighted > 0, "wheel moves the highlight");
    let _ = before;
}
```

- [ ] **Step 2: Run to verify fail** — FAIL (`scroll_top` undefined; `menu_dropdown_rect` uses raw `leaves.len()`).

- [ ] **Step 3: Implement.**

`MenuView` (`menu.rs:4`): add `pub scroll_top: usize`; set `scroll_top: 0` in `empty`/`empty_at`/`build` literals.

`menu_dropdown_rect` (`render.rs:151`): window the height. Replace `let height = leaves.len() as u16;` and the `Rect::new(...)` height term with a `list_h_for`-based height against the space below the label (`area.height - 1` rows are available below row 0; the dropdown starts at `area.y + 1`):
```rust
    let avail_below = area.height.saturating_sub(1); // rows under the bar
    let list_h = crate::list_window::list_h_for(leaves.len(), area.height.saturating_add(4)).min(avail_below as usize) as u16;
    let height = list_h.max(1);
    Some(Rect::new(label_rect.x, area.y + 1,
        width.min(area.width.saturating_sub(label_rect.x.saturating_sub(area.x))),
        height))
```
(Adjust the `+4` fudge: `list_h_for` subtracts 4 for palette chrome; the dropdown has no query bar, so pass an `area_h` that yields `min(leaves, 15, avail_below)`. Concretely define a local `list_h = leaves.len().min(15).min(avail_below as usize)` — do NOT reuse `list_h_for`'s `-4` here; the dropdown budget is `avail_below`, not `h-4`. Use `leaves.len().min(15).min(avail_below as usize)`.)

`menu_dropdown_row_at` (`render.rs:162`): return an ABSOLUTE index accounting for `scroll_top` — it needs the `scroll_top`, so change its signature to accept it: `menu_dropdown_row_at(area, groups, open, scroll_top, col, row)` → `Some((row - r.y) as usize + scroll_top)`. Update the caller in `mouse.rs:186`.

Dropdown paint (`render_overlays.rs:316`, post-Task-8): call `crate::list_window::keep_visible(menu.highlighted, leaves.len(), drop_rect.height as usize, &mut scroll_top)` (needs `editor.menu` as `&mut` in the paint — the fn already has `&mut Editor`; take the `scroll_top` mutably like the palette does), slice `leaves[scroll_top..end]`, and render the indicator via `windowed_indicator(menu.highlighted, leaves.len(), drop_rect.height as usize)` on the dropdown's bottom row when it overflows.

Menu arm in `route_overlay` (`mouse.rs`): add `ScrollUp/Down` → move `highlighted` within `[0, leaves_len)` and `keep_visible`. Keyboard handlers in `app.rs`: the Up/Down arms (`app.rs:358-361`) call `list_window::keep_visible(highlighted, len, len.min(15), &mut menu.scroll_top)` after moving — so the highlight is dragged into view. The **Left/Right category-change arms (`app.rs:356-357`) must ALSO reset `menu.scroll_top = 0`** (not just `highlighted = 0`), or a stale scroll window carries into a shorter category (Codex plan gate). (For `list_h` in the keyboard path where no frame geometry is known, use `leaves.len().min(15)` as the budget, consistent with the paint's clamp; the paint re-windows against the true frame `list_h` every frame — the `list_window` two-layer invariant.)

- [ ] **Step 4: Run tests to verify pass** — new tests PASS; existing menu tests (`click_on_inactive_bar_opens_that_category`, dropdown dispatch) green.

- [ ] **Step 5: Commit** — `feat(menu): dropdown windowing + n/total indicator + wheel scroll`.

---

## Testing & gates (whole-effort)

- Per-task: the TDD tests above (Arrange-Act-Assert), each committed with its task.
- **New local test helpers.** Names used in the task sketches — `tall_menu_groups(n)`, `tall_diag(n)`, `panel_bg`/`drop_x`/`drop_bottom` geometry locals — do NOT exist yet; each is a small `#[cfg(test)]` helper the implementer adds in the same module (Codex plan gate). Match the existing test helper style in that file.
- Cross-cutting regression tests to confirm before the final review:
  - **No-silent-UI:** a status message reveals the reserved row even under zen/`Auto` (Task 4 test covers `status_line_visible`; add an `e2e.rs` journey asserting a save message paints on the bottom row in zen).
  - **No-jank:** `edit_height` is byte-identical before/after — assert `render.rs:362` is unchanged in the final review (no task edits it).
  - **Persistence round-trip:** the new `[view] scrollbar`/`status_line` keys (Task 6).
- GATEs: `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean; `cargo build` / `cargo test --no-run` warning-free for touched crates. Smoke `scripts/smoke/run.sh` mandatory-run (quote the one-line summary in the pre-merge report).
- Pipeline gates: Codex plan review (loop clean) → subagent execution → Codex pre-merge + Fable whole-branch → merge `--no-ff`.

## Grounded reuse targets (exact anchors — do NOT duplicate logic)

These are the concrete functions/sites the mouse arms and recompute calls reuse. Verified against the branch:

1. **Prompt resolver (Task 13):** `crate::prompts::resolve_prompt(action, editor, ex, clock, msg_tx)` (`prompts.rs:99`, signature `(PromptAction, &mut Editor, &dyn Executor, &dyn Clock, &Sender<Msg>)`). The mouse prompt arm calls this with the hit `PromptAction`, then clears `editor.prompt`.
2. **Outline jump (Task 11):** `crate::app::outline_jump_to(editor, byte)` (`app.rs:276`) — the mouse arm sets `selected`, reads `rows[selected].byte`, calls `outline_jump_to`, closes the overlay. (The `opened_version` stale guard lives in the keyboard Enter path; if `outline_jump_to` does not itself re-check version, replicate the guard the keyboard arm uses.)
3. **Theme-picker Enter commit (Task 10):** the keyboard arm at `app.rs:~540-560` takes `theme_picker`, on Esc applies `tp.original`, on commit consumes `tp.previewed` to set `theme_identity`. The mouse commit arm must run `preview_selected_theme` then the SAME `previewed`-consume identity commit; extract that inline commit into a small helper (e.g. `commit_theme_picker(editor)`) and call it from both the keyboard Enter arm and the mouse arm.
4. **Diag apply (Task 12):** `crate::search_ui::diag_apply_selected(editor, clock)` (`search_ui.rs:133`) ALREADY exists and includes the `opened_version` stale guard (tested at `app.rs:4029`) — the mouse arm sets `selected` then calls it directly. Do NOT reimplement. **File-browser Enter (Task 10):** the file-browser open/descend logic (incl. the unreadable-dir guard tested at `file_browser.rs:84`) lives in the reduce Enter arm; if it is not already a named fn, extract a `file_browser_enter(editor, …)` helper and call it from BOTH the keyboard arm and the mouse arm — the plan forbids copying the fs logic.
5. **Menu highlight ↑/↓ (Task 14):** the keyboard handler is at `app.rs:358-361` (Up = `highlighted.saturating_sub(1)`, Down = `(highlighted+1).min(n-1)`). Add `crate::list_window::keep_visible(menu.highlighted, n, n.min(15), &mut menu.scroll_top)` right after each move (inside the scoped `menu` borrow).
6. **Recompute sites (Tasks 3, 4):** `recompute_scrollbar_visible` + `recompute_menu_bar` are called together at `app.rs:1237-1238` (reduce) and `recompute_scrollbar_visible` again at `app.rs:1533` (run loop). Add `recompute_status_line(editor, clock.now_ms())` beside `recompute_menu_bar` at `:1238`, and beside the `:1533` call. (Scrollbar already recomputes at both — Task 3 only changes its body.)
