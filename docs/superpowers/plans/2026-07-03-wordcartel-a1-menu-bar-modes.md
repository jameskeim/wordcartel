# A1 Menu Bar Modes (hidden | auto | pinned) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `[menu] bar = "hidden"|"auto"|"pinned"` (default auto — dwell-reveal on the top row, leave-grace hide), a pinned always-visible bar with click-to-open, a `menu_bar_pin` toggle — **and the fix for the pre-existing nav.rs geometry bug** (five height reads never subtract the menu row: invisible caret at the bottom edge, under-scroll incl. typewriter, PageUp/Down overstep).

**Architecture:** `editor.menu: Option<MenuView>` keeps meaning *dropdown open*; a config-seeded `MenuBarMode` + `MouseState.{menu_reveal_due, menu_hide_due, menu_bar_revealed}` govern bar chrome. ONE accessor — `Editor::menu_bar_rows()` — replaces every `menu.is_some()` geometry read. The dwell mirrors the scrollbar transient-chrome pattern: a trivial `Moved` arm (before ALL overlay branches) arms deadlines; `recompute_menu_bar` in the shared `advance()` fires them (e2e-drivable via the virtual clock); both dues join the run-loop deadline array.

**Tech Stack:** Rust, ratatui 0.30 / crossterm 0.28 (mode-1003 motion events already enabled), serde/toml config, the e2e `Harness`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-03-wordcartel-a1-menu-bar-modes-design.md` (Codex ×3 + Fable5; two user decisions: reserve-uniformly, leave-grace). The **asymmetric-timers rule** is design, not accident: the DWELL re-arms on every row-0 motion (reveal after REST); the GRACE arms ONCE on first leave. Do not "optimize" either direction.
- `cargo test -p wordcartel-core -p wordcartel` green; `cargo build`/`test --no-run` warning-free; **`cargo clippy --workspace --all-targets` clean (deny gate LIVE)**; NO `cargo fmt`; house style (em-dash `—`).
- Never weaken a test; the nav-fix pins must FAIL on pre-fix code (verify by reasoning or a temporary revert).
- **Pre-merge report:** run `scripts/smoke/run.sh`, quote the one-line summary verbatim (advisory); the merge report states BOTH the feature and the nav-geometry bug fix.
- Trailers on every commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: State + config + the geometry accessor (incl. the nav bug fix)

**Files:**
- Modify `wordcartel/src/config.rs` (MenuBarMode, MenuConfig, RawMenu, fold, tests).
- Modify `wordcartel/src/editor.rs` (Editor fields, MouseState fields, `menu_bar_rows()`, inits).
- Modify `wordcartel/src/app.rs` (run() seeding).
- Modify `wordcartel/src/render.rs:248` (one line).
- Modify `wordcartel/src/mouse.rs` (:19, :154, :215, :228 — four geometry lines).
- Modify `wordcartel/src/nav.rs` (:90, :403, :437, :761+:759 doc, :795 — the bug fix; tests).

**Interfaces produced:** `config::MenuBarMode { Hidden, Auto, Pinned }` (Copy, PartialEq); `config::MenuConfig { bar: MenuBarMode }`; `Editor.menu_bar_mode`, `Editor.menu_bar_unpinned_mode`, `Editor::menu_bar_rows(&self) -> u16`; `MouseState.{menu_reveal_due: Option<u64>, menu_hide_due: Option<u64>, menu_bar_revealed: bool}`.

Behavior for default config is IDENTICAL after this task (`Auto` with `revealed=false` ≡ today) **except** the five nav sites now correctly subtract the menu row while the dropdown is open — the bug fix, pinned by the new tests.

- [ ] **Step 1: Config.** In `config.rs`, beside `FocusGranularity` (~:70):
```rust
/// Menu bar visibility mode (`[menu] bar`). Auto reveals on a pointer dwell at
/// the top row and hides after a leave-grace; Pinned keeps the bar always
/// visible-inactive; Hidden shows it only while the dropdown is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuBarMode { Hidden, Auto, Pinned }

#[derive(Debug, Clone)]
pub struct MenuConfig { pub bar: MenuBarMode }
impl Default for MenuConfig {
    fn default() -> Self { MenuConfig { bar: MenuBarMode::Auto } }
}
```
  `Config` (:34-42) gains `pub menu: MenuConfig,`. Beside `RawView` (~:220):
```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawMenu {
    bar: Option<String>,
}
```
  `RawConfig` (:154-162) gains `menu: RawMenu,`. The fold, beside the focus_granularity fold (:325-330):
```rust
        // menu: per-field override; enum-valued string with a warning on unknowns.
        if let Some(b) = raw.menu.bar {
            match b.as_str() {
                "hidden" => cfg.menu.bar = MenuBarMode::Hidden,
                "auto"   => cfg.menu.bar = MenuBarMode::Auto,
                "pinned" => cfg.menu.bar = MenuBarMode::Pinned,
                other => warns.push(format!("menu.bar \"{other}\" invalid; using auto")),
            }
        }
```

- [ ] **Step 2: Config tests** (config.rs tests mod, mirroring the focus_granularity test at :465-475): (a) no `[menu]` → `MenuBarMode::Auto`; (b) each of `"hidden"`/`"auto"`/`"pinned"` folds to its variant; (c) `bar = "bogus"` → stays Auto + a warning containing `menu.bar`. Run: `cargo test -p wordcartel config` → PASS.

- [ ] **Step 3: Editor state + the accessor.** `editor.rs`:
  - `MouseState` (:323-336) gains three fields with doc comments in the scrollbar style:
```rust
    /// Deadline (ms) at which the auto-mode menu bar reveals (armed by a pointer
    /// dwell on row 0; re-armed on every row-0 motion — reveal fires after REST).
    pub menu_reveal_due: Option<u64>,
    /// Deadline (ms) at which a revealed auto-mode bar hides (armed ONCE on the
    /// first pointer-leave; cancelled by re-entering row 0 — the leave grace).
    pub menu_hide_due: Option<u64>,
    /// Whether the auto-mode bar is currently revealed (meaningless in other modes).
    pub menu_bar_revealed: bool,
```
  - `Editor` gains, beside `mouse_capture` (:377):
```rust
    /// Menu bar visibility mode (seeded from `[menu] bar`; mutated only by menu_bar_pin).
    pub menu_bar_mode: crate::config::MenuBarMode,
    /// The mode menu_bar_pin restores on unpin (registry handlers cannot see Config).
    pub menu_bar_unpinned_mode: crate::config::MenuBarMode,
```
  - `new_from_text` inits (beside `mouse: MouseState::default()`, :445):
    `menu_bar_mode: crate::config::MenuBarMode::Auto,` and
    `menu_bar_unpinned_mode: crate::config::MenuBarMode::Auto,`
    (MouseState derives Default — the three new fields default correctly).
  - The accessor (an `impl Editor` method near the other small accessors):
```rust
    /// Rows reserved by the menu bar at the top of the frame (0 or 1). THE single
    /// source of row-0 geometry truth — render/mouse/nav read this, never
    /// `menu.is_some()` directly (the dropdown-open checks in overlay routing are
    /// the deliberate exception: they mean "dropdown open", not geometry).
    pub fn menu_bar_rows(&self) -> u16 {
        let bar = match self.menu_bar_mode {
            crate::config::MenuBarMode::Pinned => true,
            crate::config::MenuBarMode::Auto => self.mouse.menu_bar_revealed,
            crate::config::MenuBarMode::Hidden => false,
        };
        u16::from(bar || self.menu.is_some())
    }
```

- [ ] **Step 4: Seeding.** `app.rs` `run()` (beside `export_cfg`, :1960):
```rust
    editor.menu_bar_mode = cfg.menu.bar;
    editor.menu_bar_unpinned_mode = if cfg.menu.bar == crate::config::MenuBarMode::Pinned {
        crate::config::MenuBarMode::Auto // unpin target when config itself pins
    } else {
        cfg.menu.bar
    };
```

- [ ] **Step 5: The accessor truth-table test** (editor.rs or app.rs tests):
```rust
    #[test]
    fn menu_bar_rows_truth_table() {
        use crate::config::MenuBarMode as M;
        let mut e = Editor::new_from_text("x\n", None, (20, 6));
        for (mode, revealed, open, want) in [
            (M::Hidden, false, false, 0u16), (M::Hidden, true, false, 0), (M::Hidden, false, true, 1),
            (M::Auto, false, false, 0), (M::Auto, true, false, 1), (M::Auto, false, true, 1),
            (M::Pinned, false, false, 1), (M::Pinned, true, true, 1),
        ] {
            e.menu_bar_mode = mode;
            e.mouse.menu_bar_revealed = revealed;
            e.menu = if open { Some(crate::menu::empty()) } else { None };
            assert_eq!(e.menu_bar_rows(), want, "mode={mode:?} revealed={revealed} open={open}");
        }
    }
```
  (Note `Hidden + revealed=true → 0`: the revealed flag is meaningless outside Auto by
  construction — the truth table pins that.)

- [ ] **Step 6: The geometry sweep.** Replace the `u16::from(editor.menu.is_some())` /
  `(editor.active().view.area.1 as usize).saturating_sub(1)` height reads:
  - `render.rs:248`: `let menu_rows = editor.menu_bar_rows();`
  - `mouse.rs:19` (`editing_cell`): `let menu_rows: u16 = editor.menu_bar_rows();`
  - `mouse.rs:154` (scrollbar click): `let menu_rows = editor.menu_bar_rows();`
  - `mouse.rs:215` (scrollbar drag): `let menu_rows = editor.menu_bar_rows();`
  - `mouse.rs:228` (text drag): `let menu_rows = editor.menu_bar_rows();`
  - **nav.rs — the five bug-fix reads.** Each becomes menu-aware; update the stale comments:
    - `:90` (`screen_pos`):
      `let area_height = (editor.active().view.area.1 as usize).saturating_sub(1 + editor.menu_bar_rows() as usize);`
      (comment: "…reserves the status row AND the menu bar row when visible.")
    - `:403` (`ensure_visible`, typewriter branch):
      `let edit_height = (editor.active().view.area.1 as usize).saturating_sub(1 + editor.menu_bar_rows() as usize);`
    - `:437` (`ensure_visible`, plain branch): same substitution for `area_height`.
    - `:761` (`page_step`) — the height input changes, the overlap formula is PRESERVED
      VERBATIM; the doc comment (:759, "matches nav.rs:62") is updated:
```rust
/// Page step: editing_height − 1 for one row of context overlap.
/// `editing_height` reserves the status row and the menu bar row when visible
/// (matches screen_pos/ensure_visible/last_fully_visible_line).
fn page_step(editor: &Editor) -> usize {
    let editing_height = (editor.active().view.area.1 as usize)
        .saturating_sub(1 + editor.menu_bar_rows() as usize);
    editing_height.saturating_sub(1).max(1)
}
```
    - `:795` (`last_fully_visible_line`): same substitution for `height`; update its comment.

- [ ] **Step 7: The nav bug-fix pins** (nav.rs tests mod — these FAIL pre-fix with the menu
  open; verify that by reasoning through the pre-fix arithmetic in the test comments):
```rust
    #[test]
    fn ensure_visible_accounts_for_menu_bar_row() {
        // 20x10 frame: status row + OPEN MENU -> edit_height 8. Pre-fix nav used 9,
        // under-scrolling by one: the caret's screen row landed AT edit_height and
        // render's `row < edit_height` guard left the cursor unpainted.
        let mut e = crate::editor::Editor::new_from_text(&"line\n".repeat(20), None, (20, 10));
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        e.menu = Some(crate::menu::build(&reg, &km));
        let end = e.active().document.buffer.len();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(end);
        crate::derive::rebuild(&mut e);
        ensure_visible(&mut e);
        crate::derive::rebuild(&mut e);
        let (_, row) = screen_pos(&e).expect("caret must be visible after ensure_visible");
        let edit_height = 10u16 - 1 - 1;
        assert!(row < edit_height, "caret row {row} must fit the menu-adjusted viewport (h={edit_height})");
    }

    #[test]
    fn page_step_accounts_for_menu_bar_row() {
        // 20x10 + open menu: editing_height 8 -> step 7. Pre-fix: height 9 -> step 8.
        let mut e = crate::editor::Editor::new_from_text(&"line\n".repeat(30), None, (20, 10));
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        e.menu = Some(crate::menu::build(&reg, &km));
        crate::derive::rebuild(&mut e);
        let off = move_page_down(&mut e);
        let line = editor_line_of(&e, off); // use the existing line-of helper in this mod, or
                                            // derive::line_of_offset — match local test idiom.
        assert_eq!(line, 7, "PageDown from line 0 with the bar visible steps 7 (8-row viewport, 1 overlap)");
    }
```
  (Adapt helper names to the local test idiom in nav.rs's test mod — the ASSERTED VALUES
  are the contract. If `line_of_offset` isn't the local name, find the mod's existing
  line-index helper; do not weaken the `7` / `row < edit_height` expectations.)

- [ ] **Step 8: Run + gates + commit.** `cargo test -p wordcartel-core -p wordcartel` green;
  clippy clean; `cargo test --no-run` warning-free.
```bash
git add -A
git commit -m "feat(menu): MenuBarMode config + menu_bar_rows() accessor; fix nav menu-row geometry bug"   # + trailers
```

---

### Task 2: Pinned mode — inactive-bar rendering, click-to-open, Esc nuance, the pin command

**Files:**
- Modify `wordcartel/src/menu.rs` (`empty_at`).
- Modify `wordcartel/src/app.rs` (hydrate open-preservation + the Mouse-arm hydrate call).
- Modify `wordcartel/src/render.rs` (bar block restructure + `menu_bar_layout_cats`).
- Modify `wordcartel/src/mouse.rs` (the `CellHit::MenuBar` click-to-open arm).
- Modify `wordcartel/src/registry.rs` (`menu_bar_pin`).
- Modify `wordcartel/src/e2e.rs` (journeys 3 + 4).

**Interfaces:**
- Consumes: `Editor::menu_bar_rows()`, `MenuBarMode`, the Task-1 fields.
- Produces: `menu::empty_at(order_idx: usize) -> MenuView`;
  `render::menu_bar_layout_cats(area, cats: &[MenuCategory]) -> Vec<(usize, Rect)>`
  (the groups-based `menu_bar_layout` becomes a thin wrapper); the `menu_bar_pin` command.

- [ ] **Step 1: `empty_at`** (menu.rs, beside `empty()`):
```rust
/// A placeholder opened AT a specific category (an index into `MENU_ORDER`);
/// hydration maps it to the built groups' position for that category.
pub fn empty_at(order_idx: usize) -> MenuView {
    MenuView { groups: Vec::new(), open: order_idx, highlighted: 0, built: false }
}
```

- [ ] **Step 2: Hydrate preserves + maps `open`** (app.rs:824-826). The placeholder's `open`
  is a `MENU_ORDER` index (from a bar click) or 0 (F10's `empty()` — MENU_ORDER[0] = File =
  groups[0], unchanged behavior). Map by CATEGORY, not raw index (a category with no
  commands would shift group indices):
```rust
    if let Some(v) = editor.menu.as_ref().filter(|v| !v.built) {
        let want_open = v.open;
        let want_hl = v.highlighted;
        let mut built = crate::menu::build(reg, keymap);
        // The placeholder's `open` indexes MENU_ORDER; map it to the built groups'
        // position for that category (robust even if a category has no commands).
        if let Some(cat) = crate::registry::MENU_ORDER.get(want_open) {
            if let Some(pos) = built.groups.iter().position(|g| g.0 == *cat) {
                built.open = pos;
            }
        }
        built.highlighted = want_hl.min(
            built.groups.get(built.open).map_or(0, |g| g.1.len().saturating_sub(1)),
        );
        editor.menu = Some(built);
    }
```
  (This REPLACES the current two-line `is_some_and(!built) → build` form. `MENU_ORDER` is
  already `pub` in registry.rs:42.)

- [ ] **Step 3: The Mouse-arm hydrate (spec CRITICAL C1)** — app.rs:1757-1759:
```rust
        Msg::Input(Event::Mouse(ev)) => {
            crate::mouse::handle(editor, ev, reg, keymap, ex, clock, msg_tx);
            // A click-opened menu placeholder must be built before the next render —
            // the key-dispatch path hydrates; the mouse path must too (A1 spec C1).
            hydrate_overlays(editor, reg, keymap);
        }
```

- [ ] **Step 4: Render — the bar in both states.** In render.rs, add the cats-based layout
  beside `menu_bar_layout` (:108) and make the old one a wrapper:
```rust
pub(crate) fn menu_bar_layout_cats(area: Rect, cats: &[crate::registry::MenuCategory]) -> Vec<(usize, Rect)> {
    let mut out = Vec::new();
    let mut x = area.x;
    for (i, cat) in cats.iter().enumerate() {
        let label = crate::menu::category_label_pub(*cat);
        let wgt = label.chars().count() as u16 + 2; // 1 space padding each side
        out.push((i, Rect::new(x, area.y, wgt, 1)));
        x = x.saturating_add(wgt);
    }
    out
}

pub(crate) fn menu_bar_layout(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)]) -> Vec<(usize, Rect)> {
    let cats: Vec<crate::registry::MenuCategory> = groups.iter().map(|g| g.0).collect();
    menu_bar_layout_cats(area, &cats)
}
```
  Restructure the menu paint block (:906-945): the OUTER condition becomes
  `if editor.menu_bar_rows() == 1`, the fill moves outside the groups guard, and an
  inactive-bar branch paints static labels:
```rust
    if editor.menu_bar_rows() == 1 {
        let menu_area = Rect::new(area.x, area.y, w, h.saturating_sub(1));
        // Full-width bar background: gaps between labels + the right side carry the
        // Chrome style; the per-label paints below overwrite their own rects (A2).
        let bar_row = Rect::new(area.x, area.y, w, 1);
        frame.buffer_mut().set_style(bar_row, menu_closed_style);
        match editor.menu {
            Some(ref menu) if !menu.groups.is_empty() => {
                // — the existing label loop + dropdown painting, verbatim —
                let bar = menu_bar_layout(menu_area, &menu.groups);
                /* labels with open-highlight, then the dropdown block, unchanged */
            }
            _ => {
                // Inactive bar (pinned / auto-revealed / unbuilt placeholder): static
                // labels, all closed-style, no dropdown, no highlight.
                for (i, rect) in &menu_bar_layout_cats(menu_area, crate::registry::MENU_ORDER) {
                    let label = crate::menu::category_label_pub(crate::registry::MENU_ORDER[*i]);
                    frame.render_widget(Paragraph::new(format!(" {label} ")).style(menu_closed_style), *rect);
                }
            }
        }
    }
```
  (Move the existing loop/dropdown code into the `Some(...)` arm unchanged. NOTE the
  unbuilt-placeholder case intentionally falls to the inactive branch — between a click and
  the hydrate call it never renders, but the total function stays panic-free for any state.
  Confirm `MENU_ORDER`'s type — if it's `[MenuCategory; 5]`, pass `&MENU_ORDER[..]`.)

- [ ] **Step 5: Click-to-open** (mouse.rs, in the `Down(Left)` match arm's `editing_cell`
  result — add a `CellHit::MenuBar` arm before `CellHit::Scrollbar`; it is reachable ONLY
  with the bar visible and the dropdown CLOSED, because the open-dropdown case returns in
  the overlay block above):
```rust
            if let CellHit::MenuBar = hit {
                // Inactive bar: open the dropdown AT the clicked category (hydrated
                // by reduce's post-handle hydrate_overlays call).
                let cats_hit = crate::render::menu_bar_layout_cats(area, crate::registry::MENU_ORDER)
                    .into_iter()
                    .find(|(_, r)| ev.column >= r.x && ev.column < r.x + r.width && ev.row == r.y)
                    .map(|(i, _)| i);
                if let Some(order_idx) = cats_hit {
                    editor.menu = Some(crate::menu::empty_at(order_idx));
                }
                // A row-0 click OFF the labels does nothing (the fill area is inert).
            } else if let CellHit::Scrollbar = hit {
```
  (Adjust the surrounding `if let` chain to the local idiom — the current code uses
  sequential `if let` on `hit`; a `match hit` restructure is fine if cleaner. The
  behavioral contract: MenuBar+label → `empty_at(idx)`; MenuBar off-label → no-op.)

- [ ] **Step 6: The `menu_bar_pin` command** (registry.rs, beside the other View toggles ~:374):
```rust
        r.register("menu_bar_pin", "Pin Menu Bar", Some(MenuCategory::View), |c| {
            use crate::config::MenuBarMode;
            if c.editor.menu_bar_mode == MenuBarMode::Pinned {
                c.editor.menu_bar_mode = c.editor.menu_bar_unpinned_mode;
            } else {
                c.editor.menu_bar_unpinned_mode = c.editor.menu_bar_mode;
                c.editor.menu_bar_mode = MenuBarMode::Pinned;
            }
            // Mode-transition hygiene: stale auto-state must not survive (spec M2).
            c.editor.mouse.menu_reveal_due = None;
            c.editor.mouse.menu_hide_due = None;
            c.editor.mouse.menu_bar_revealed = false;
            CommandResult::Handled
        });
```

- [ ] **Step 7: Unit tests** (app.rs / menu.rs / mouse.rs test mods, matching local idiom):
  1. `hydrate_preserves_and_maps_open`: `editor.menu = Some(menu::empty_at(2))` (Format) →
     `hydrate_overlays` → `menu.open` == the built groups' position of Format; `built`.
  2. `hydrate_clamps_highlighted`: placeholder with `highlighted = 999` → clamped to the
     open group's last index.
  3. `pin_toggle_round_trips_and_clears_auto_state`: mode Auto + revealed=true + a pending
     due → dispatch `menu_bar_pin` → Pinned, all three fields cleared; dispatch again →
     Auto restored.
  4. `editing_cell_row0_is_menubar_only_when_bar_visible`: Pinned + menu None → `MenuBar`;
     Hidden + menu None → NOT MenuBar (Text/Scrollbar per column).
  5. `render_paints_inactive_bar_labels` (render tests): Pinned + menu None → row 0
     contains `" File "`, `" Edit "`… in Chrome style, and NO dropdown on row 1.
  6. `click_on_inactive_bar_opens_that_category` (mouse or app tests): Pinned, menu None;
     `mouse::handle(Down(Left) at the Format label's column, row 0)` → `menu` is Some
     placeholder with `open == 3`?? — NO: `open` == the MENU_ORDER index of Format (= 2:
     File, Edit, Format…). Assert `open == 2` and `!built`; then `hydrate_overlays` →
     `built` with the mapped group open. (Compute the label column from
     `menu_bar_layout_cats` in the test — don't hardcode.)
- [ ] **Step 8: e2e journeys 3 + 4** (e2e.rs, following the existing journey style):
```rust
    /// A1 journey 3: pinned — the bar is always there; Esc closes the dropdown ONLY.
    #[test]
    fn journey_pinned_bar_persists_across_dropdown_close() {
        let mut h = Harness::new("hello world\n", None, (40, 8));
        h.editor.menu_bar_mode = crate::config::MenuBarMode::Pinned;
        h.tick(); // render with the mode applied
        assert!(h.row(0).contains(" File "), "pinned bar visible before any menu use");
        assert!(h.row(1).contains("hello"), "text shifted below the bar");
        h.key(KeyCode::F(10));
        assert!(h.editor.menu.is_some(), "F10 opens the dropdown");
        h.key(KeyCode::Esc);
        assert!(h.editor.menu.is_none(), "Esc closes the dropdown");
        assert!(h.row(0).contains(" File "), "the bar PERSISTS after Esc (the state split)");
    }

    /// A1 journey 4: hidden — the dwell is mode-gated (non-vacuous form, spec M4).
    #[test]
    fn journey_hidden_never_reveals_on_dwell() {
        let mut h = Harness::new("hello world\n", None, (40, 8));
        h.editor.menu_bar_mode = crate::config::MenuBarMode::Hidden;
        h.mouse_move(5, 0);
        h.advance_ms(crate::mouse::MENU_DWELL_MS + 1);
        h.tick();
        assert!(!h.editor.mouse.menu_bar_revealed, "Hidden mode must never arm/reveal");
        assert!(h.row(0).contains("hello"), "row 0 is still text");
        h.key(KeyCode::F(10));
        assert!(h.row(0).contains(" File "), "F10 still opens");
        h.key(KeyCode::Esc);
        assert!(h.row(0).contains("hello"), "Esc closes FULLY in hidden mode");
    }
```
  Add the harness sugar (beside `resize`):
```rust
    fn mouse_move(&mut self, col: u16, row: u16) {
        self.step(Msg::Input(Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column: col, row, modifiers: KeyModifiers::NONE,
        })));
    }
    fn mouse_down(&mut self, col: u16, row: u16) {
        self.step(Msg::Input(Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: col, row, modifiers: KeyModifiers::NONE,
        })));
    }
```
  (Journey 4 references `MENU_DWELL_MS` — Task 3 defines it; if Task 2 lands first, use the
  literal `250 + 1` with a comment, and Task 3 switches it to the const. NOTE: journey 4's
  dwell-injection assertion is inert until Task 3 adds the arm — it passes trivially here
  and becomes load-bearing when Task 3 lands; keep it in this task so the pinned/hidden
  journeys ship together, and Task 3's report confirms it's now non-vacuous.)
- [ ] **Step 9: Run + gates + commit** (same gates as Task 1).
```bash
git commit -m "feat(menu): pinned mode — inactive bar, click-to-open, Esc nuance, menu_bar_pin"   # + trailers
```

---

### Task 3: Auto mode — the dwell machinery

**Files:**
- Modify `wordcartel/src/mouse.rs` (consts + the Moved arm + predicate tests).
- Modify `wordcartel/src/app.rs` (`recompute_menu_bar` in advance(), the deadline array,
  `reconcile_mouse_capture` clears).
- Modify `wordcartel/src/e2e.rs` (journeys 1, 2, 5).

**Interfaces:**
- Consumes: everything from Tasks 1-2.
- Produces: `mouse::MENU_DWELL_MS`, `mouse::MENU_LEAVE_GRACE_MS`, `app::recompute_menu_bar`.

- [ ] **Step 1: Consts** (mouse.rs, top):
```rust
/// Pointer must REST on row 0 this long before the auto-mode bar reveals.
pub(crate) const MENU_DWELL_MS: u64 = 250;
/// A revealed bar survives leaving row 0 this long (aim-wobble forgiveness).
pub(crate) const MENU_LEAVE_GRACE_MS: u64 = 400;
```

- [ ] **Step 2: The Moved arm** — in `handle()`, immediately after the universal Up-clear
  (:85-88), BEFORE `let (w, h) = …` and ALL overlay branches (every overlay block below
  returns unconditionally; leave-bookkeeping must run even while one is open — spec I1):
```rust
    // A1 auto-mode dwell tracking. Runs on every motion frame — keep it trivial
    // (integer compares + stores only; the reveal/hide fire later in advance()).
    // The two timers are deliberately ASYMMETRIC: the dwell re-arms on every
    // row-0 motion (reveal after REST), the grace arms ONCE on the first leave.
    if editor.menu_bar_mode == crate::config::MenuBarMode::Auto {
        if let MouseEventKind::Moved = ev.kind {
            if ev.row > 0 {
                editor.mouse.menu_reveal_due = None;
                if editor.mouse.menu_bar_revealed && editor.mouse.menu_hide_due.is_none() {
                    editor.mouse.menu_hide_due = Some(clock.now_ms() + MENU_LEAVE_GRACE_MS);
                }
            } else {
                editor.mouse.menu_hide_due = None; // re-entry cancels a pending hide
                if editor.menu.is_none()
                    && editor.palette.is_none()
                    && editor.theme_picker.is_none()
                    && editor.file_browser.is_none()
                    && !editor.mouse.dragging
                    && !editor.mouse.scrollbar_dragging
                    && !editor.mouse.menu_bar_revealed
                {
                    editor.mouse.menu_reveal_due = Some(clock.now_ms() + MENU_DWELL_MS);
                }
            }
        }
    }
```

- [ ] **Step 3: `recompute_menu_bar`** (app.rs, beside `recompute_scrollbar_visible` :2242):
```rust
/// Fire the auto-mode menu-bar deadlines (armed by the mouse Moved arm). Gated on
/// Auto — a stale due must never fire in Pinned/Hidden (spec M2).
pub fn recompute_menu_bar(editor: &mut crate::editor::Editor, now_ms: u64) {
    if editor.menu_bar_mode != crate::config::MenuBarMode::Auto {
        return;
    }
    if editor.mouse.menu_reveal_due.is_some_and(|d| now_ms >= d) {
        editor.mouse.menu_reveal_due = None;
        editor.mouse.menu_bar_revealed = true;
    }
    if editor.mouse.menu_hide_due.is_some_and(|d| now_ms >= d) {
        editor.mouse.menu_hide_due = None;
        editor.mouse.menu_bar_revealed = false;
    }
}
```
  Call it in `advance()` immediately after `recompute_scrollbar_visible(editor, clock.now_ms());`:
  `recompute_menu_bar(editor, clock.now_ms());`

- [ ] **Step 4: The deadline array** (app.rs :2156-2183) — add beside `sb_deadline`:
```rust
        // Menu-bar dwell/grace: at most one is Some by construction (the Moved arm
        // clears the other side); recompute_menu_bar clears a fired due, so a past
        // deadline cannot persist and spin the loop.
        let menu_deadline = editor.mouse.menu_reveal_due.or(editor.mouse.menu_hide_due);
```
  and add `menu_deadline,` to the `next_deadline(&[…])` array.

- [ ] **Step 5: Capture-off stranding fix** (spec I2) — `reconcile_mouse_capture`'s disable
  branch (app.rs:2257-2263), beside the drag clears:
```rust
            editor.mouse.menu_reveal_due = None;
            editor.mouse.menu_hide_due = None;
            editor.mouse.menu_bar_revealed = false;
```

- [ ] **Step 6: The predicate-table unit tests** (mouse.rs tests, driving `handle()` directly
  with `TestClock` — follow the local test idiom for constructing Editor/reg/keymap/executor):
  1. `dwell_arms_on_row0_rest`: Moved(5,0) at t=0 → `menu_reveal_due == Some(MENU_DWELL_MS)`.
  2. `dwell_rearm_tracks_last_motion` (the asymmetry, side 1): Moved(5,0)@0, Moved(6,0)@100
     → due == Some(100 + MENU_DWELL_MS).
  3. `dwell_never_arms_during_drag_or_overlay`: dragging=true → no arm; palette open → no
     arm; dropdown open → no arm (each case).
  4. `leave_arms_grace_once` (the asymmetry, side 2): revealed=true; Moved(5,5)@0 → hide ==
     Some(MENU_LEAVE_GRACE_MS); Moved(6,5)@100 → hide UNCHANGED (still Some(GRACE), not 100+GRACE).
  5. `reentry_cancels_grace`: revealed=true, Moved(5,5)@0 (hide armed), Moved(5,0)@100 →
     hide == None, still revealed.
  6. `leave_bookkeeping_runs_while_dropdown_open` (the I1 pin): revealed=true + menu open;
     Moved(5,5) → hide armed (the arm sits before the overlay return).
  7. `recompute_fires_and_is_mode_gated` (app tests): reveal due in the past + Auto →
     revealed; same + Pinned → untouched; hide due past + Auto → unrevealed.
  8. `capture_disable_clears_menu_bar_state` (app tests): revealed + a pending due →
     `reconcile_mouse_capture` with `mouse_capture=false` over a `Vec<u8>` backend → all
     three cleared.

- [ ] **Step 7: e2e journeys 1, 2, 5** (e2e.rs):
```rust
    /// A1 journey 1: dwell-reveal (rest), grace-hide (leave), and grace-cancel (return).
    #[test]
    fn journey_auto_dwell_reveal_and_grace_hide() {
        let mut h = Harness::new("hello world\n", None, (40, 8));
        // default mode is Auto; row 0 is text while unrevealed
        assert!(h.row(0).contains("hello"));
        h.mouse_move(5, 0);
        h.advance_ms(crate::mouse::MENU_DWELL_MS + 1);
        h.tick();
        assert!(h.row(0).contains(" File "), "bar revealed after the dwell");
        assert!(h.row(1).contains("hello"), "text reserved down one row");
        // leave: grace, not instant
        h.mouse_move(5, 5);
        assert!(h.row(0).contains(" File "), "still revealed during the grace");
        h.advance_ms(crate::mouse::MENU_LEAVE_GRACE_MS + 1);
        h.tick();
        assert!(h.row(0).contains("hello"), "hidden after the grace; text back on row 0");
        // reveal again, then leave-and-return WITHIN the grace: the bar survives
        h.mouse_move(5, 0);
        h.advance_ms(crate::mouse::MENU_DWELL_MS + 1);
        h.tick();
        h.mouse_move(5, 5);
        h.advance_ms(100); // < grace
        h.mouse_move(5, 0);
        h.advance_ms(crate::mouse::MENU_LEAVE_GRACE_MS + 1);
        h.tick();
        assert!(h.row(0).contains(" File "), "return within the grace keeps the bar");
    }

    /// A1 journey 2: a drag across row 0 never arms the dwell.
    #[test]
    fn journey_drag_never_reveals() {
        let mut h = Harness::new("hello world\nmore text here\n", None, (40, 8));
        h.mouse_down(2, 1);            // start a text drag (dragging = true)
        h.mouse_move(2, 0);            // motion onto row 0 mid-drag
        h.advance_ms(crate::mouse::MENU_DWELL_MS + 10);
        h.tick();
        assert!(!h.editor.mouse.menu_bar_revealed, "drag must not arm the dwell");
        assert!(h.row(0).contains("hello"), "row 0 stays text");
    }

    /// A1 journey 5: a row-0 click while unrevealed is a TEXT click.
    #[test]
    fn journey_row0_click_unrevealed_edits_text() {
        let mut h = Harness::new("hello world\n", None, (40, 8));
        h.mouse_down(4, 0);
        assert!(h.editor.menu.is_none(), "no menu opened");
        assert_eq!(h.editor.active().document.selection.primary().head(), 4,
            "the click placed the caret in the text");
    }
```
  (Journey 2 note: real terminals send `Drag` while a button is held; the journey pins the
  belt-and-braces `!dragging` guard for lost-Up cases — construct it exactly as written.
  Adapt the `head()` accessor to the Selection API's real name — `head()`/`.head` per
  selection.rs; the asserted OFFSET 4 is the contract. Also flip Task 2's journey-4 literal
  to the consts now that they exist, and confirm in the report that its dwell-injection
  assertion is now load-bearing.)

- [ ] **Step 8: Run + gates + commit** (same gates).
```bash
git commit -m "feat(menu): auto mode — dwell reveal + leave grace via the deadline machinery"   # + trailers
```

---

## Pre-merge checklist (beyond the standard gates)

1. `scripts/smoke/run.sh` — quote the one-line summary verbatim (advisory; motion injection
   is out of smoke's reach — documented boundary; the e2e journeys are the dwell coverage).
2. The merge report states BOTH: the A1 feature set AND the pre-existing nav-geometry bug
   fix (invisible bottom-edge caret / under-scroll incl. typewriter / PageUp/Down overstep
   whenever the menu was open).
3. A quick live sanity in tmux (tui-interact) of PINNED mode (config `[menu] bar = "pinned"`:
   bar visible, click a label, Esc keeps the bar) — dwell needs a real pointer and stays a
   user-verified item post-merge.

## Self-Review

**Spec coverage:** config+fold+tests ✓; the three MouseState fields + two Editor fields +
seeding (incl. the unpinned-mode fallback) ✓; `menu_bar_rows()` + truth table ✓; the
geometry sweep — render :248, mouse :19/:154/:215/:228, nav's FIVE reads with page_step's
overlap preserved + stale comments fixed ✓ + the two failing-pre-fix pins ✓; the two-sided
split (overlay reads untouched — no task touches mouse.rs:115/app.rs:1137/registry.rs:221
dropdown checks) ✓; inactive-bar render from `MENU_ORDER` via `menu_bar_layout_cats` (+ the
old signature preserved as a wrapper for existing callers/tests) ✓; click-to-open with
category-mapped hydration + clamp (spec C1's mouse-arm hydrate call) ✓; Esc/F10 nuance
verified by journey 3 (no code change needed — app.rs:1153-1154 stays) ✓; `menu_bar_pin`
with the REQUIRED Editor field + hygiene clears ✓; the Moved arm verbatim from the spec
(placement before ALL overlay branches, the widened arming gate, both asymmetric timers)
✓; `recompute_menu_bar` mode-gated in `advance()` ✓; the deadline-array slot with the
no-spin rationale ✓; the I2 capture-off clears ✓; journeys 1-5 (4 made non-vacuous per M4)
+ the 8-case predicate/unit table ✓; degradation is by-construction (no capture → handle
early-returns; the journeys don't need to pin it beyond the capture-clear test) ✓.

**Placeholder scan:** the two spots where local idiom varies (nav test helpers, the
`if let` chain shape in mouse.rs Step 5) name the CONTRACT values that may not be adapted
away; everything else is complete code.

**Type consistency:** `MenuBarMode` lives in config.rs and is referenced as
`crate::config::MenuBarMode` everywhere; `menu_bar_rows() -> u16`;
`empty_at(usize)`; `menu_bar_layout_cats(Rect, &[MenuCategory]) -> Vec<(usize, Rect)>`
(the wrapper keeps the old signature); consts are `pub(crate)` in mouse.rs (the spec says
`pub` — `pub(crate)` satisfies the intent, all consumers are in-crate; e2e.rs references
them as `crate::mouse::MENU_DWELL_MS`). `Selection::primary().head()` — confirm the real
accessor name at implementation time (the offset contract is fixed).

**Ordering:** T1 (pure state/geometry, behavior-identical except the bug fix) → T2 (pinned +
render + commands; journey 4's dwell line inert until T3) → T3 (the dwell + journeys,
flips journey 4 load-bearing). Sequential; T2/T3 both touch e2e.rs and mouse.rs.
