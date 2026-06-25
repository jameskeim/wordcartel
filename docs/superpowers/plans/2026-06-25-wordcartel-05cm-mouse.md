# Effort 5c-m — Mouse Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add text-first mouse support — capture lifecycle (toggleable), click/drag/shift-extend/double-word/triple-paragraph/wheel, an auto-hiding draggable scrollbar, and fully clickable overlays — by self-rendering the menu (dropping `tui-menu`) and reusing 5c's `offset_at_cell`.

**Architecture:** A new `wordcartel/src/mouse.rs` handles all mouse events from a single `Msg::Input(Event::Mouse)` reduce arm, reusing `nav::offset_at_cell`, `commands::scope_range_at`, `sel_history`, and `dispatch_overlay_command`. The menu is rewritten to a self-rendered shallow model (`MenuView{groups,open,highlighted,built}`) so both overlays are click+keyboard. Mouse capture is toggleable and reconciled in the main loop.

**Tech Stack:** Rust, `crossterm` 0.28 (`EnableMouseCapture`/`MouseEvent`), `ratatui` 0.29 (`Scrollbar`/`List`/`Clear` primitives — no interactive widgets). **Removes** `tui-menu`.

## Global Constraints

- `#![forbid(unsafe_code)]`; `wordcartel-core` stays IO/thread-free and is UNTOUCHED by this effort.
- `cargo build --workspace` zero warnings; an item unused until a later task carries a SCOPED per-item `#[allow(dead_code)] // wired in Task N` (never module-level).
- No pre-existing test weakened or deleted; `cargo test --workspace` stays green.
- No NEW dependency; `tui-menu = "=0.3.0"` is REMOVED (Task 1).
- ratatui has no clickable/interactive widgets — "clickable" means we own the `Rect` and hit-test mouse coords ourselves; render and mouse share geometry helpers so paint and hit-testing never desync.
- Offsets that become a caret are clamped+grapheme-snapped via `nav::clamp_snap` (5c).

## File Structure

| File | Responsibility | Task(s) |
|------|----------------|---------|
| `wordcartel/src/menu.rs` (rewrite) | Self-rendered shallow menu model `MenuView{groups,open,highlighted,built}` | 1 |
| `wordcartel/src/render.rs` | Self-render menu bar+dropdown; scrollbar; shared geometry helpers | 1,6,7 |
| `wordcartel/Cargo.toml` | Remove `tui-menu` | 1 |
| `wordcartel/src/app.rs` | Menu keyboard nav rewrite; mouse reduce arm; loop reconcile + scrollbar deadline | 1,2,3,6 |
| `wordcartel/src/term.rs` | `TerminalGuard::new(enable_mouse)` + `DisableMouseCapture` teardowns | 2 |
| `wordcartel/src/config.rs` | `mouse.capture` config | 2 |
| `wordcartel/src/editor.rs` | `Editor.mouse_capture`, `Editor.mouse: MouseState` | 2,3 |
| `wordcartel/src/registry.rs` | `toggle_mouse_capture` command | 2 |
| `wordcartel/src/mouse.rs` (new) | All mouse-event handling | 3,4,5,6,7 |
| `wordcartel/src/commands.rs` | `pub fn scope_range_at(editor, offset, Scope)` | 5 |

**Linear task order:** 1 (self-render menu) → 2 (capture lifecycle) → 3 (foundations + click) → 4 (drag/shift) → 5 (multi-click + wheel) → 6 (scrollbar) → 7 (overlay mouse).

---

## Task 1: Self-render the menu (drop `tui-menu`)

Behavior-preserving refactor: the F10 menu keeps its exact keyboard behavior, but is now painted by us (so Task 7 can make it clickable) and `tui-menu` is removed.

**Files:**
- Modify: `wordcartel/src/menu.rs` (rewrite `MenuView`), `wordcartel/src/render.rs` (replace the `tui_menu::Menu` paint at 245-256 + add geometry helpers), `wordcartel/src/app.rs` (rewrite the menu keyboard block at 487-533 + the `menu_select_for_test` shim ~1250), `wordcartel/Cargo.toml` (remove `tui-menu`)
- Test: `wordcartel/src/menu.rs`, `wordcartel/src/app.rs`

**Interfaces:**
- Produces: `MenuView { pub groups: Vec<(MenuCategory, Vec<(String, CommandId)>)>, pub open: usize, pub highlighted: usize, pub built: bool }`; `menu::empty()`/`menu::build(reg,keymap)`; render geometry helpers `render::menu_bar_layout(area, groups) -> Vec<(usize, ratatui::layout::Rect)>`, `render::menu_dropdown_rect(area, groups, open) -> Option<Rect>`, `render::menu_dropdown_row_at(area, groups, open, col, row) -> Option<usize>` (used by Task 7).
- Consumes: existing private `menu::grouped_commands` (unchanged).

- [ ] **Step 1: Adapt + extend the failing tests.** In `menu.rs`, the `grouped_commands` grouping test is unchanged. In `app.rs`, rewrite the `menu_select_for_test` shim and add parity tests:

```rust
    // app.rs tests — replaces the MenuState-era shim
    #[test]
    fn menu_keyboard_nav_moves_and_dispatches() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |c| Event::Key(KeyEvent { code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // F10 opens; menu hydrated with groups
        crate::app::reduce(Msg::Input(press(KeyCode::F(10))), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_some());
        let m = e.menu.as_ref().unwrap();
        assert!(!m.groups.is_empty(), "menu hydrated with groups");
        assert_eq!(m.open, 0);
        // Right moves to the next category, Down highlights a row
        crate::app::reduce(Msg::Input(press(KeyCode::Right)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.menu.as_ref().unwrap().open, 1);
        crate::app::reduce(Msg::Input(press(KeyCode::Down)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.menu.as_ref().unwrap().highlighted, 1);
    }
```

(`f10_opens_menu` / `f10_toggles_menu_closed_when_open` stay — they assert `editor.menu.is_some()`.)

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib app::tests::menu_keyboard_nav` → FAIL (groups/open/highlighted fields missing).

- [ ] **Step 3: Rewrite `menu.rs`** — drop the `tui_menu` import, `MenuState`, `menu_items_from_groups`; new struct + ctors (keep `grouped_commands`/`leaf_label`/`category_label` private, unchanged):

```rust
use crate::keymap::KeyTrie;
use crate::registry::{CommandId, MenuCategory, Registry, MENU_ORDER};

#[derive(Clone, Debug)]
pub struct MenuView {
    pub groups: Vec<(MenuCategory, Vec<(String, CommandId)>)>,
    pub open: usize,
    pub highlighted: usize,
    pub built: bool,
}

pub fn empty() -> MenuView {
    MenuView { groups: Vec::new(), open: 0, highlighted: 0, built: false }
}

pub fn build(reg: &Registry, keymap: &KeyTrie) -> MenuView {
    MenuView { groups: grouped_commands(reg, keymap), open: 0, highlighted: 0, built: true }
}
```

(Keep the existing private `grouped_commands`, `leaf_label`, `category_label` exactly as-is. Remove the custom `Debug` impl — `#[derive(Debug)]` works now. `MenuView` is `Clone`.)

- [ ] **Step 4: Rewrite the `app.rs` menu keyboard block** (replaces 487-533). The block runs only on `KeyEventKind::Press` while `editor.menu.is_some()`; guard `groups.is_empty()`:

```rust
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                let mut selected: Option<crate::registry::CommandId> = None;
                if let Some(menu) = editor.menu.as_mut() {
                    let ncat = menu.groups.len();
                    match k.code {
                        crossterm::event::KeyCode::Esc | crossterm::event::KeyCode::F(10) => { editor.menu = None; }
                        crossterm::event::KeyCode::Left if ncat > 0 => {
                            menu.open = (menu.open + ncat - 1) % ncat; menu.highlighted = 0;
                        }
                        crossterm::event::KeyCode::Right if ncat > 0 => {
                            menu.open = (menu.open + 1) % ncat; menu.highlighted = 0;
                        }
                        crossterm::event::KeyCode::Up if ncat > 0 => {
                            menu.highlighted = menu.highlighted.saturating_sub(1);
                        }
                        crossterm::event::KeyCode::Down if ncat > 0 => {
                            let n = menu.groups[menu.open].1.len();
                            if n > 0 { menu.highlighted = (menu.highlighted + 1).min(n - 1); }
                        }
                        crossterm::event::KeyCode::Enter if ncat > 0 => {
                            let leaves = &menu.groups[menu.open].1;
                            if let Some((_, id)) = leaves.get(menu.highlighted) { selected = Some(*id); }
                        }
                        _ => {}
                    }
                }
                if let Some(id) = selected {
                    dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id);
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
```

- [ ] **Step 5: Replace the render menu paint** (render.rs, the `tui_menu::Menu` block at 245-256) and add the geometry helpers:

```rust
// Shared geometry — render AND mouse (Task 7) both call these.
pub(crate) fn menu_bar_layout(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)]) -> Vec<(usize, Rect)> {
    let mut out = Vec::new();
    let mut x = area.x;
    for (i, (cat, _)) in groups.iter().enumerate() {
        let label = crate::menu::category_label_pub(*cat); // expose category_label as pub(crate)
        let wgt = label.chars().count() as u16 + 2; // 1 space padding each side
        out.push((i, Rect::new(x, area.y, wgt, 1)));
        x = x.saturating_add(wgt);
    }
    out
}
pub(crate) fn menu_dropdown_rect(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)], open: usize) -> Option<Rect> {
    let bar = menu_bar_layout(area, groups);
    let (_, label_rect) = bar.get(open)?;
    let leaves = &groups.get(open)?.1;
    if leaves.is_empty() { return None; }
    let width = leaves.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0) as u16 + 2;
    let height = leaves.len() as u16;
    Some(Rect::new(label_rect.x, area.y + 1, width.min(area.width.saturating_sub(label_rect.x - area.x)), height.min(area.height.saturating_sub(1))))
}
pub(crate) fn menu_dropdown_row_at(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::registry::CommandId)>)], open: usize, col: u16, row: u16) -> Option<usize> {
    let r = menu_dropdown_rect(area, groups, open)?;
    if col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height {
        Some((row - r.y) as usize)
    } else { None }
}
```

Then paint (replacing the old block): for each `(i, rect)` in `menu_bar_layout`, render the category label (reversed style when `i == menu.open`); if `menu_dropdown_rect` is Some, `Clear` it then render a `List` of the open group's labels with `menu.highlighted` reversed. (Expose `menu::category_label` as `pub(crate) fn category_label_pub` or inline the match — keep `grouped_commands` private.)

- [ ] **Step 6: Remove `tui-menu` from `Cargo.toml`** (the `tui-menu = "=0.3.0"` line). Update the editor.rs:117 comment (`MenuView` is now Clone; the "not Clone" reason is gone — adjust the note or drop it; do NOT re-derive `Editor: Clone`).

- [ ] **Step 7: Run tests + suite.** `cargo test -p wordcartel --lib menu:: app::tests::menu` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings; confirm `grep -rn "tui_menu\|tui-menu" wordcartel/` returns nothing.

- [ ] **Step 8: Commit.**

```bash
git add wordcartel/src/menu.rs wordcartel/src/render.rs wordcartel/src/app.rs wordcartel/Cargo.toml wordcartel/src/editor.rs Cargo.lock
git commit -m "refactor(menu): self-render shallow menu (drop tui-menu); shared bar/dropdown geometry helpers"
```

---

## Task 2: Mouse-capture lifecycle (config + guard + toggle + reconcile)

**Files:**
- Modify: `wordcartel/src/term.rs` (`TerminalGuard::new(enable_mouse)` + teardowns), `wordcartel/src/config.rs` (`mouse.capture`), `wordcartel/src/editor.rs` (`Editor.mouse_capture`), `wordcartel/src/app.rs` (`reconcile_mouse_capture` + loop wiring + guard call site), `wordcartel/src/registry.rs` (`toggle_mouse_capture`)
- Test: `wordcartel/src/app.rs`, `wordcartel/src/term.rs`

**Interfaces:**
- Produces: `term::TerminalGuard::new(enable_mouse: bool)`; `Editor.mouse_capture: bool`; `app::reconcile_mouse_capture(editor, backend, applied: &mut bool)`; `toggle_mouse_capture` command.
- Consumes: config layering (5a).

- [ ] **Step 1: Write the failing tests.** In `app.rs`:

```rust
    #[test]
    fn toggle_mouse_capture_flips_flag() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        assert!(e.mouse_capture, "default on");
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock::new(0);
        let id = reg.resolve_name("toggle_mouse_capture").expect("registered");
        { let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx.clone() };
          reg.dispatch(id, &mut ctx); }
        assert!(!e.mouse_capture, "toggled off");
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib app::tests::toggle_mouse_capture` → FAIL.

- [ ] **Step 3: `term.rs`** — add the param + capture calls:

```rust
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, DisableMouseCapture, EnableMouseCapture};
// ...
    pub fn new(enable_mouse: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(e) = execute!(stdout, EnterAlternateScreen) { let _ = disable_raw_mode(); return Err(e); }
        let _ = execute!(stdout, EnableBracketedPaste);
        if enable_mouse { let _ = execute!(stdout, EnableMouseCapture); }
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = match Terminal::new(backend) {
            Ok(t) => t,
            Err(e) => { let _ = disable_raw_mode();
                let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen, Show);
                return Err(e); }
        };
        Ok(Self { terminal })
    }
```

Add `DisableMouseCapture` to the `Drop` impl and the panic hook `execute!` calls (all 3 teardown sites now lead with `DisableMouseCapture`). It is harmless if capture was never enabled.

- [ ] **Step 4: `config.rs`** — add `mouse.capture` to the config (RawConfig `Option<bool>`, default `true`, merged per the existing layered pattern); expose the resolved `mouse_capture: bool` on the resolved config struct.

- [ ] **Step 5: `editor.rs`** — `pub mouse_capture: bool` on `Editor`, init from config (or `true` in `new_from_text`; the real startup seeds it from config in `app::run`).

- [ ] **Step 6: `app.rs`** — the reconcile helper + loop wiring:

```rust
pub fn reconcile_mouse_capture<W: std::io::Write>(editor: &mut Editor, backend: &mut W, applied: &mut bool) {
    if editor.mouse_capture != *applied {
        if editor.mouse_capture {
            let _ = crossterm::execute!(backend, crossterm::event::EnableMouseCapture);
        } else {
            let _ = crossterm::execute!(backend, crossterm::event::DisableMouseCapture);
            // clear drag state so no Up is awaited that won't arrive
            editor.mouse.dragging = false;
            editor.mouse.scrollbar_dragging = false;
            editor.mouse.anchor = None;
        }
        *applied = editor.mouse_capture;
    }
}
```

In `app::run`: construct the guard as `TerminalGuard::new(editor.mouse_capture)?`; declare `let mut applied_mouse = editor.mouse_capture;`; call `reconcile_mouse_capture(&mut editor, guard.terminal().backend_mut(), &mut applied_mouse)` once before the first draw AND each loop iteration next to `drain_clipboard_intents` (app.rs:1040). (`MouseState`'s `dragging`/`scrollbar_dragging`/`anchor` are added in Task 3 — if Task 2 lands first, stub those lines behind the fields or land the `MouseState` struct here; recommended: land the `MouseState` struct + `Editor.mouse` field in Task 2 so reconcile compiles, with the gesture logic in Task 3.)

- [ ] **Step 7: `registry.rs`** — register the command (palette-only, no chord):

```rust
        r.register("toggle_mouse_capture", "Toggle Mouse Capture", Some(MenuCategory::View), |c| { c.editor.mouse_capture = !c.editor.mouse_capture; CommandResult::Handled });
```

- [ ] **Step 8: Run tests + suite.** `cargo test -p wordcartel --lib` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings.

- [ ] **Step 9: Commit.**

```bash
git add wordcartel/src/term.rs wordcartel/src/config.rs wordcartel/src/editor.rs wordcartel/src/app.rs wordcartel/src/registry.rs
git commit -m "feat(mouse): capture lifecycle — TerminalGuard(enable_mouse), mouse.capture config, toggle command, loop reconcile"
```

---

## Task 3: Mouse foundations + click→caret

**Files:**
- Modify: `wordcartel/src/editor.rs` (`MouseState` if not landed in Task 2), `wordcartel/src/app.rs` (the `Msg::Input(Event::Mouse)` arm), create `wordcartel/src/mouse.rs`, `wordcartel/src/lib.rs` (`pub mod mouse;`)
- Test: `wordcartel/src/mouse.rs`

**Interfaces:**
- Produces: `MouseState { anchor: Option<usize>, last_click: Option<ClickRecord>, dragging: bool, scrollbar_dragging: bool, scrollbar_until_ms: u64, scrollbar_visible: bool }`, `ClickRecord { cell: (u16,u16), at_ms: u64, count: u8 }`; `mouse::handle(editor, ev: MouseEvent, reg, keymap, ex, clock, msg_tx)`; `mouse::editing_cell(editor, col, row) -> CellHit`.
- Consumes: `nav::offset_at_cell`, `nav::clamp_snap`, `editor.pending_mark`.

- [ ] **Step 1: Write the failing tests** in `mouse.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::jobs::InlineExecutor; use crate::registry::Registry;
    use crossterm::event::{MouseEvent, MouseEventKind, MouseButton, KeyModifiers};
    // app's TestClock is private to its test module — define a local one here.
    struct TestClock(u64);
    impl wordcartel_core::history::Clock for TestClock { fn now_ms(&self) -> u64 { self.0 } }
    fn ctx() -> (Registry, InlineExecutor, TestClock, std::sync::mpsc::Sender<crate::app::Msg>, crate::keymap::KeyTrie) {
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let (tx, _rx) = std::sync::mpsc::channel();
        (reg, InlineExecutor::default(), TestClock(0), tx, km)
    }
    fn down(col: u16, row: u16) -> MouseEvent {
        MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: col, row, modifiers: KeyModifiers::NONE }
    }
    #[test]
    fn click_places_caret_at_cell_offset() {
        let mut e = Editor::new_from_text("abc\ndef\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        // cell (1,1) = 'e' in "def" → offset 5 (no menu, so screen row == editing row)
        handle(&mut e, down(1, 1), &reg, &km, &ex, &clk, &tx);
        assert_eq!(crate::nav::head(&e), 5);
    }
    #[test]
    fn click_below_content_goes_to_doc_end() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, down(0, 10), &reg, &km, &ex, &clk, &tx); // row past content
        assert_eq!(crate::nav::head(&e), e.active().document.buffer.len());
    }
    #[test]
    fn mouse_ignored_during_pending_mark() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.pending_mark = Some(crate::editor::MarkPending::Set);
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, down(1, 0), &reg, &km, &ex, &clk, &tx);
        assert_eq!(crate::nav::head(&e), 0, "click ignored while mark capture pending");
        assert!(e.pending_mark.is_some());
    }
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib mouse::` → FAIL.

- [ ] **Step 3: `editor.rs`** — `MouseState`/`ClickRecord` (if not landed in Task 2) + `pub mouse: MouseState` (init `Default`). Derive `Default` for both.

- [ ] **Step 4: `mouse.rs`** — coord translation + the handler skeleton (click only for now):

```rust
use crossterm::event::{MouseEvent, MouseEventKind, MouseButton, KeyModifiers};
use crate::editor::Editor;

pub enum CellHit { Text { col: u16, erow: u16 }, MenuBar, Status, Scrollbar, Outside }

pub fn editing_cell(editor: &Editor, col: u16, row: u16) -> CellHit {
    let (w, h) = editor.active().view.area;
    let menu_rows: u16 = u16::from(editor.menu.is_some());
    if h == 0 { return CellHit::Outside; }
    if row == h - 1 { return CellHit::Status; }
    if menu_rows == 1 && row == 0 { return CellHit::MenuBar; }
    if editor.mouse.scrollbar_visible && col == w.saturating_sub(1) { return CellHit::Scrollbar; }
    let erow = row.saturating_sub(menu_rows);
    let edit_height = h.saturating_sub(1 + menu_rows);
    if erow < edit_height { CellHit::Text { col, erow } } else { CellHit::Outside }
}

pub fn handle(
    editor: &mut Editor, ev: MouseEvent,
    _reg: &crate::registry::Registry, _keymap: &crate::keymap::KeyTrie,
    _ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    _msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    if editor.pending_mark.is_some() || !editor.mouse_capture { return; }
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let CellHit::Text { col, erow } = editing_cell(editor, ev.column, ev.row) {
                let off = crate::nav::offset_at_cell(editor, col, erow)
                    .unwrap_or_else(|| crate::nav::clamp_snap(editor, editor.active().document.buffer.len()));
                editor.active_mut().sel_history.clear();
                editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
                editor.mouse.anchor = Some(off);
                editor.mouse.dragging = true;
                let _ = clock; // multi-click timing wired in Task 5
                crate::derive::rebuild(editor);
                crate::nav::ensure_visible(editor);
            }
        }
        _ => {} // drag/wheel/up wired in Tasks 4-5
    }
}
```

- [ ] **Step 5: `app.rs`** — add the mouse arm at the BOTTOM of `reduce` (below the key/paste interceptors, near `Msg::Input(_) => {}` at app.rs:764). Replace/precede that catch-all:

```rust
        Msg::Input(Event::Mouse(ev)) => {
            crate::mouse::handle(editor, ev, reg, keymap, ex, clock, msg_tx);
        }
        Msg::Input(_) => {}
```

`lib.rs`: `pub mod mouse;`.

- [ ] **Step 6: Run tests + suite.** `cargo test -p wordcartel --lib mouse::` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings (some `MouseState` fields are consumed in Tasks 4-6 → scoped `#[allow(dead_code)] // wired in Task N` per-field as needed).

- [ ] **Step 7: Commit.**

```bash
git add wordcartel/src/mouse.rs wordcartel/src/lib.rs wordcartel/src/editor.rs wordcartel/src/app.rs
git commit -m "feat(mouse): MouseState + Event::Mouse reduce arm + click→caret (offset_at_cell), pending_mark guard"
```

---

## Task 4: Drag-select, Shift+click extend, edge auto-scroll

**Files:** Modify `wordcartel/src/mouse.rs`. Test: `wordcartel/src/mouse.rs`.

**Interfaces:** Consumes `MouseState.anchor`/`dragging` (Task 3), `nav::offset_at_cell`/`ensure_visible`, the existing scroll fields.

- [ ] **Step 1: Write the failing tests** in `mouse.rs`:

```rust
    #[test]
    fn drag_selects_range_from_anchor() {
        let mut e = Editor::new_from_text("abcdef\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, down(1, 0), &reg, &km, &ex, &clk, &tx); // anchor at offset 1
        let drag = MouseEvent { kind: MouseEventKind::Drag(MouseButton::Left), column: 4, row: 0, modifiers: KeyModifiers::NONE };
        handle(&mut e, drag, &reg, &km, &ex, &clk, &tx); // head at offset 4
        let r = e.active().document.selection.primary();
        assert_eq!((r.from(), r.to()), (1, 4));
    }
    #[test]
    fn shift_click_extends_keeping_anchor() {
        let mut e = Editor::new_from_text("abcdef\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        let (reg, ex, clk, tx, km) = ctx();
        let shift_down = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 4, row: 0, modifiers: KeyModifiers::SHIFT };
        handle(&mut e, shift_down, &reg, &km, &ex, &clk, &tx);
        let r = e.active().document.selection.primary();
        assert_eq!((r.from(), r.to()), (1, 4), "extends from existing anchor to click");
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib mouse::tests::drag mouse::tests::shift_click` → FAIL.

- [ ] **Step 3: Implement** — in the `Down(Left)` arm, handle the SHIFT modifier (extend); add the `Drag(Left)` arm with edge auto-scroll:

```rust
        MouseEventKind::Down(MouseButton::Left) => {
            if let CellHit::Text { col, erow } = editing_cell(editor, ev.column, ev.row) {
                let off = crate::nav::offset_at_cell(editor, col, erow)
                    .unwrap_or_else(|| editor.active().document.buffer.len());
                let off = crate::nav::clamp_snap(editor, off);
                editor.active_mut().sel_history.clear();
                if ev.modifiers.contains(KeyModifiers::SHIFT) {
                    let anchor = editor.active().document.selection.primary().anchor;
                    editor.active_mut().document.selection = wordcartel_core::selection::Selection::range(anchor, off);
                    editor.mouse.anchor = Some(anchor);
                } else {
                    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
                    editor.mouse.anchor = Some(off);
                    // (multi-click in Task 5)
                }
                editor.mouse.dragging = true;
                crate::derive::rebuild(editor); crate::nav::ensure_visible(editor);
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if !editor.mouse.dragging { return; }
            let (_w, h) = editor.active().view.area;
            let menu_rows = u16::from(editor.menu.is_some());
            let edit_top = menu_rows;
            let edit_bottom = h.saturating_sub(1); // status row excluded
            // edge auto-scroll
            if ev.row < edit_top { crate::nav::scroll_up_one(editor); }
            else if ev.row >= edit_bottom { crate::nav::scroll_down_one(editor); }
            let erow = ev.row.clamp(edit_top, edit_bottom.saturating_sub(1)).saturating_sub(menu_rows);
            let head = crate::nav::offset_at_cell(editor, ev.column, erow)
                .unwrap_or_else(|| editor.active().document.buffer.len());
            let head = crate::nav::clamp_snap(editor, head);
            if let Some(anchor) = editor.mouse.anchor {
                editor.active_mut().document.selection = wordcartel_core::selection::Selection::range(anchor, head);
                crate::derive::rebuild(editor);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => { editor.mouse.dragging = false; }
```

Add tiny `nav::scroll_up_one`/`scroll_down_one` helpers if not present (adjust `view.scroll`/`scroll_row` by one logical row, clamped) — reuse the existing scroll arithmetic in `nav.rs`.

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib mouse::` → PASS; `cargo test --workspace` → green; zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/mouse.rs wordcartel/src/nav.rs
git commit -m "feat(mouse): drag-select with edge auto-scroll; Shift+click extend"
```

---

## Task 5: Multi-click (word/paragraph) + wheel scroll

**Files:** Modify `wordcartel/src/mouse.rs`, `wordcartel/src/commands.rs` (`scope_range_at`). Test: `wordcartel/src/mouse.rs`, `wordcartel/src/commands.rs`.

**Interfaces:** Produces `pub fn commands::scope_range_at(editor: &Editor, offset: usize, scope: Scope) -> (usize, usize)`. Consumes `MouseState.last_click`, `clock.now_ms()`, the scroll fields.

- [ ] **Step 1: Write the failing tests.** In `commands.rs`:

```rust
    #[test]
    fn scope_range_at_word_at_offset() {
        let mut e = Editor::new_from_text("alpha beta", None, (80, 24));
        derive::rebuild(&mut e);
        // offset 7 is inside "beta" (6..10)
        assert_eq!(super::scope_range_at(&e, 7, Scope::Word), (6, 10));
    }
```

In `mouse.rs`:

```rust
    #[test]
    fn double_click_selects_word_triple_selects_paragraph() {
        let mut e = Editor::new_from_text("alpha beta\n\ngamma\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        // two Downs on the same cell within 400ms (TestClock fixed at 0)
        handle(&mut e, down(7, 0), &reg, &km, &ex, &clk, &tx);
        handle(&mut e, down(7, 0), &reg, &km, &ex, &clk, &tx);
        let r = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(r.from()..r.to()), "beta");
        handle(&mut e, down(7, 0), &reg, &km, &ex, &clk, &tx); // triple → paragraph
        let r2 = e.active().document.selection.primary();
        assert!(e.active().document.buffer.slice(r2.from()..r2.to()).starts_with("alpha beta"));
        assert!(!e.active().sel_history.is_empty(), "multi-click seeds the expand ladder");
    }
    #[test]
    fn wheel_scrolls_view_not_caret() {
        let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 10));
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        let before = crate::nav::head(&e);
        let wheel = MouseEvent { kind: MouseEventKind::ScrollDown, column: 0, row: 0, modifiers: KeyModifiers::NONE };
        handle(&mut e, wheel, &reg, &km, &ex, &clk, &tx);
        assert!(e.active().view.scroll > 0, "view scrolled");
        assert_eq!(crate::nav::head(&e), before, "caret unchanged");
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib commands::tests::scope_range_at mouse::tests::double_click mouse::tests::wheel` → FAIL.

- [ ] **Step 3: `commands.rs`** — extract `scope_range_at`; make the private `scope_range` delegate:

```rust
pub fn scope_range_at(editor: &Editor, h: usize, scope: Scope) -> (usize, usize) {
    let buf = &editor.active().document.buffer;
    let blocks = &editor.active().document.blocks;
    match scope {
        Scope::Word => { /* SAME body as current scope_range but using the `h` param instead of nav::head(editor) */ }
        Scope::Sentence => { /* ... */ }
        Scope::Paragraph => nav::paragraph_range_at(blocks, buf, h),
        Scope::Document => (0, buf.len()),
    }
}
fn scope_range(editor: &Editor, scope: Scope) -> (usize, usize) {
    scope_range_at(editor, nav::head(editor), scope)
}
```

(Move the existing `scope_range` body verbatim into `scope_range_at`, replacing the internal `let h = nav::head(editor);` with the `h` parameter.)

- [ ] **Step 4: `mouse.rs`** — multi-click in the non-shift `Down(Left)` branch + the wheel arms:

```rust
        // inside Down(Left), non-shift branch, replacing the single-click placement:
        let now = clock.now_ms();
        let cell = (ev.column, ev.row);
        let count = match editor.mouse.last_click {
            Some(ref lc) if now.saturating_sub(lc.at_ms) <= 400 && lc.cell == cell => (lc.count % 3) + 1,
            _ => 1,
        };
        editor.mouse.last_click = Some(ClickRecord { cell, at_ms: now, count });
        match count {
            2 => { let (f, t) = crate::commands::scope_range_at(editor, off, crate::commands::Scope::Word);
                   seed_and_select(editor, f, t); }
            3 => { let (f, t) = crate::commands::scope_range_at(editor, off, crate::commands::Scope::Paragraph);
                   seed_and_select(editor, f, t); }
            _ => { editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(off);
                   editor.mouse.anchor = Some(off); }
        }
```

```rust
        MouseEventKind::ScrollDown => { crate::nav::scroll_down_one(editor); editor.mouse.scrollbar_until_ms = clock.now_ms() + 1200; }
        MouseEventKind::ScrollUp   => { crate::nav::scroll_up_one(editor);   editor.mouse.scrollbar_until_ms = clock.now_ms() + 1200; }
```

`seed_and_select` pushes the current selection onto `sel_history` (clone-to-local first) then sets `Selection::range(f,t)` + rebuild + ensure_visible — so `Ctrl+W` grows from the mouse selection.

- [ ] **Step 5: Run tests + suite.** `cargo test -p wordcartel --lib` → PASS; `cargo test --workspace` → green; zero warnings.

- [ ] **Step 6: Commit.**

```bash
git add wordcartel/src/mouse.rs wordcartel/src/commands.rs
git commit -m "feat(mouse): double-click word / triple-click paragraph (seeds ladder); wheel scrolls view"
```

---

## Task 6: Auto-hiding draggable scrollbar

**Files:** Modify `wordcartel/src/render.rs` (scrollbar paint), `wordcartel/src/app.rs` (loop deadline + scrollbar_visible recompute), `wordcartel/src/mouse.rs` (scrollbar drag). Test: `wordcartel/src/app.rs`, `wordcartel/src/mouse.rs`.

**Interfaces:** Consumes `MouseState.scrollbar_until_ms`/`scrollbar_visible`/`scrollbar_dragging`; `derive::total_logical_lines`; the loop deadline.

- [ ] **Step 1: Write the failing tests.** In `mouse.rs`:

```rust
    #[test]
    fn scrollbar_drag_scrubs_view() {
        let text: String = (0..100).map(|i| format!("l{i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 12));
        e.mouse.scrollbar_visible = true;
        crate::derive::rebuild(&mut e);
        let (reg, ex, clk, tx, km) = ctx();
        // Down on the scrollbar column (w-1 = 79), mid-track row
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 79, row: 6, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.mouse.scrollbar_dragging);
        assert!(e.active().view.scroll > 0, "scrubbed to a lower position");
    }
```

In `app.rs`, a Tick/deadline test:

```rust
    #[test]
    fn scrollbar_visible_recomputed_against_clock() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.mouse.scrollbar_until_ms = 1000;
        crate::app::recompute_scrollbar_visible(&mut e, 500); // before deadline
        assert!(e.mouse.scrollbar_visible);
        crate::app::recompute_scrollbar_visible(&mut e, 1200); // after
        assert!(!e.mouse.scrollbar_visible);
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib mouse::tests::scrollbar_drag app::tests::scrollbar_visible` → FAIL.

- [ ] **Step 3: Implement.**
  - `app.rs`: `pub fn recompute_scrollbar_visible(editor: &mut Editor, now_ms: u64) { editor.mouse.scrollbar_visible = now_ms < editor.mouse.scrollbar_until_ms; }`. Call it at the top of the loop (with `clock.now_ms()`) so render reads a fresh bool. Add `editor.mouse.scrollbar_until_ms` to the loop **deadline** computation (app.rs:1023-1033): include it (when `> now` and `scrollbar_visible`) in the `deadline` min, so the loop wakes at fade time, recomputes the bool false, and redraws.
  - `render.rs`: when `editor.mouse.scrollbar_visible`, render a `Scrollbar(ScrollbarOrientation::VerticalRight)` on the rightmost editing column with `ScrollbarState::new(total_logical_lines).position(view.scroll)` over the editing-area Rect.
  - `mouse.rs`: in `Down(Left)`/`Drag(Left)`, when `editing_cell` is `CellHit::Scrollbar`, set `scrollbar_dragging = true` and map the row within the track to `view.scroll = (erow_in_track / edit_height) * max_scroll` (clamped); set `scrollbar_until_ms = now + 1200`. `Up` clears `scrollbar_dragging`.

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib` → PASS; `cargo test --workspace` → green; zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/render.rs wordcartel/src/app.rs wordcartel/src/mouse.rs
git commit -m "feat(mouse): auto-hiding draggable scrollbar (loop-deadline fade, proportional scrub)"
```

---

## Task 7: Mouse on overlays (palette + menu)

**Files:** Modify `wordcartel/src/app.rs` (`dispatch_overlay_command` → `pub(crate)`), `wordcartel/src/render.rs` (palette geometry helpers), `wordcartel/src/mouse.rs` (overlay routing). Test: `wordcartel/src/mouse.rs`.

**Interfaces:** Consumes `dispatch_overlay_command` (now `pub(crate)`), `render::{palette_overlay_rect, palette_row_at, menu_bar_layout, menu_dropdown_row_at}` (menu helpers from Task 1).

- [ ] **Step 1: Write the failing tests** in `mouse.rs`:

```rust
    #[test]
    fn click_palette_row_dispatches_and_closes() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3);
        e.palette = Some(crate::palette::Palette::default());
        let (reg, ex, clk, tx, km) = ctx();
        crate::app::hydrate_overlays(&mut e, &reg, &km); // fill rows (5b helper)
        // find the row index of "copy" and click it
        let rows = &e.palette.as_ref().unwrap().rows;
        let idx = rows.iter().position(|r| r.id == crate::registry::CommandId("copy")).unwrap();
        let rect = crate::render::palette_overlay_rect((80,24).into());
        let click_row = rect.y + 2 + idx as u16; // list starts at ov_y+2
        let d = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: click_row, modifiers: KeyModifiers::NONE };
        handle(&mut e, d, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_none(), "palette closed after click");
        assert_eq!(e.register.get(), Some("abc"), "clicked Copy dispatched");
    }
    #[test]
    fn click_outside_palette_closes_it() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.palette = Some(crate::palette::Palette::default());
        let (reg, ex, clk, tx, km) = ctx();
        handle(&mut e, down(0, 0), &reg, &km, &ex, &clk, &tx); // top-left, outside the centered overlay
        assert!(e.palette.is_none());
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib mouse::tests::click_palette mouse::tests::click_outside` → FAIL.

- [ ] **Step 3: Implement.**
  - `app.rs`: change `fn dispatch_overlay_command` (app.rs:421) AND `fn hydrate_overlays` (app.rs:408) to `pub(crate)` — both are private today; the mouse handler needs `dispatch_overlay_command` and the T7 test needs `hydrate_overlays`.
  - `render.rs`: extract `pub(crate) fn palette_overlay_rect(area: Rect) -> Rect` (the ov_x/ov_y/ov_w/ov_h computation at render.rs:193-196) and `pub(crate) fn palette_row_at(area: Rect, palette: &Palette, col: u16, row: u16) -> Option<usize>` (list starts at `ov_y+2`, height = visible rows); render's palette paint calls them so geometry is shared.
  - `mouse.rs`: at the TOP of `handle` (after the pending_mark/capture guards), route overlays BEFORE the text area:

```rust
    if editor.palette.is_some() {
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            let area = ratatui::layout::Rect::new(0, 0, editor.active().view.area.0, editor.active().view.area.1);
            if let Some(idx) = crate::render::palette_row_at(area, editor.palette.as_ref().unwrap(), ev.column, ev.row) {
                if let Some(id) = editor.palette.as_ref().unwrap().rows.get(idx).map(|r| r.id) {
                    crate::app::dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id);
                }
            } else {
                editor.palette = None; // click outside closes
            }
        }
        return;
    }
    if editor.menu.is_some() {
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            let area = ratatui::layout::Rect::new(0, 0, editor.active().view.area.0, editor.active().view.area.1);
            let groups = &editor.menu.as_ref().unwrap().groups;
            if let Some((cat, _)) = crate::render::menu_bar_layout(area, groups).into_iter().find(|(_, r)| ev.column >= r.x && ev.column < r.x + r.width && ev.row == r.y) {
                let m = editor.menu.as_mut().unwrap(); m.open = cat; m.highlighted = 0;
            } else if let Some(row) = crate::render::menu_dropdown_row_at(area, groups, editor.menu.as_ref().unwrap().open, ev.column, ev.row) {
                let open = editor.menu.as_ref().unwrap().open;
                if let Some((_, id)) = editor.menu.as_ref().unwrap().groups.get(open).and_then(|g| g.1.get(row)).copied().map(|x| ((), x.1)) {
                    crate::app::dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id);
                }
            } else {
                editor.menu = None; // outside → close
            }
        }
        return;
    }
```

(Wire the real `reg`/`keymap`/`ex`/`msg_tx` params through `handle` — they were `_`-prefixed in Task 3; un-prefix them now.)

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib mouse::` → PASS; `cargo test --workspace` → green; `cargo build --workspace` → zero warnings.

- [ ] **Step 5: Commit.**

```bash
git add wordcartel/src/app.rs wordcartel/src/render.rs wordcartel/src/mouse.rs
git commit -m "feat(mouse): clickable overlays — palette row dispatch + menu bar/dropdown click + outside-dismiss"
```

---

## Self-Review

**Spec coverage:** §2 deps (T1 drops tui-menu; no new dep) ✅; §3 modules (mouse.rs T3, menu rewrite T1, render helpers T1/6/7, commands::scope_range_at T5, term/config/editor/registry T2) ✅; §4 capture lifecycle incl. config-honored startup + reconcile + clear-on-off (T2) ✅; §5 coord translation `editing_cell` + scrollbar-column (T3/T6) ✅; §6 click/drag/shift/multi-click/wheel (T3/T4/T5) ✅; §7 auto-hiding scrollbar + loop-deadline fade + scrub (T6) ✅; §8 palette full-click + self-rendered menu click + outside-dismiss + dispatch_overlay_command pub(crate) (T1/T7) ✅; §9 pending_mark guard + None→doc-end + clear-on-off + groups bounds-guard (T3/T2/T1) ✅; §10 tests (per task) ✅; §11 summary + prerequisites (dispatch_overlay_command pub(crate) T7, scope_range_at T5, TerminalGuard param T2) ✅.

**Placeholder scan:** the `scope_range_at` Word/Sentence arms in T5 Step 3 say "SAME body as current scope_range" — that is a deliberate move-verbatim instruction (the body exists in the repo), not a placeholder; the implementer copies the real arms and swaps `nav::head(editor)` for the `h` param. The `menu category_label` exposure in T1 (`category_label_pub` or inline) is a concrete either/or. No TBD/empty steps.

**Type consistency:** `MouseState`/`ClickRecord` fields, `CellHit`, `mouse::handle`/`editing_cell`, `MenuView{groups,open,highlighted,built}`, `render::{menu_bar_layout,menu_dropdown_rect,menu_dropdown_row_at,palette_overlay_rect,palette_row_at}`, `commands::scope_range_at`, `app::{reconcile_mouse_capture,recompute_scrollbar_visible,dispatch_overlay_command(pub(crate))}`, `term::TerminalGuard::new(enable_mouse)`, `nav::{scroll_up_one,scroll_down_one,clamp_snap,offset_at_cell}` are used identically across the tasks that define and consume them.
