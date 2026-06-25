# Wordcartel 5b — Command Palette + Hideable Menu Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a fuzzy command palette (`Ctrl+P`) and a hideable menu bar (`F10`), both as view layers over the 4b command registry, so every command is discoverable + reachable with its chord shown in place.

**Architecture:** Add per-command display metadata to an ordered registry; register the transforms as discrete commands; reverse-look-up chords from the 5a keymap. The palette is a hand-rolled centered overlay + `nucleo-matcher` fuzzy; the menu uses `tui-menu` 0.3.0 driven by our crossterm keys. Both **precompute their display data in the reduce path** (which holds `&reg`+`&keymap`) into `Editor` state; `render(&Editor)` paints from it. `wordcartel-core` is untouched.

**Tech Stack:** Rust; `nucleo-matcher = "0.3"` (palette fuzzy, MPL-2.0); `tui-menu = "=0.3.0"` (ratatui-0.29-safe pin, MIT/Apache); ratatui 0.29 `Clear`/`List` for the overlay; the 4b `Registry`, 5a `KeyTrie`/keymap, the `Msg`/`reduce` loop.

**Spec:** `docs/superpowers/specs/2026-06-25-wordcartel-05b-palette-menu-design.md` (Codex-reviewed: 1 crit + 6 imp applied).

## Global Constraints

- `#![forbid(unsafe_code)]`; `wordcartel-core` untouched. New deps in `wordcartel/Cargo.toml` only: `nucleo-matcher = "0.3"`, `tui-menu = "=0.3.0"` (pin exactly — 0.3.1 needs ratatui 0.30).
- **Both surfaces are VIEW LAYERS over the registry** — they render + route a `CommandId` back; the registry owns the command list.
- **Ordered registry:** commands stored in stable insertion order (`Vec<CommandEntry>` + `HashMap<CommandId,usize>` index); palette/menu enumerate deterministically.
- **Render reads precomputed state:** `render(&Editor)` is UNCHANGED — the palette/menu store their display rows/tree (labels + reverse-looked-up chords) built in reduce; render paints from `Editor.palette`/`Editor.menu`. (5a `mem::take`s the keymap into a `run()` local, so render can't reach it.)
- **XOR:** at most one of {prompt, minibuffer, palette, menu} active; overlays open only in normal mode; opening one clears `pending_keys`.
- **Key-only interception:** palette/menu reduce blocks intercept only `Msg::Input(Event::Key(_))` and return early; non-key `Msg`s (Job/Filter/Transform/Export Done, Tick, Resize) fall through.
- **Esc precedence:** active overlay (palette/menu) > prompt > minibuffer > pending-cancel > filter-cancel > keymap.
- **Shared dispatch:** `dispatch_overlay_command` closes the active overlay, then runs the normal `Ctx` build + `reg.dispatch` + `ex.drain` (no duplication; a command that opens a modal opens it cleanly after the overlay closes).
- `cargo build --workspace` zero warnings; not-yet-wired items scoped `#[allow(dead_code)]`. No prior test weakened.

---

## File Structure

- **Create:** `wordcartel/src/palette.rs` (palette state + nucleo ranking + row build), `wordcartel/src/menu.rs` (build the tui-menu tree from the registry + route).
- **Modify:** `wordcartel/Cargo.toml` (deps), `wordcartel/src/lib.rs` (modules), `registry.rs` (ordered store + `CommandMeta`/`MenuCategory` + `commands()`/`meta()` + transform commands), `keymap.rs` (`chord_for`), `editor.rs` (`palette`/`menu` fields), `app.rs` (palette/menu commands + reduce blocks + hydrate-on-open + `dispatch_overlay_command` + Esc), `render.rs` (palette overlay + menu bar), the keymap presets (bind `palette`=Ctrl+P, `menu`=F10).

---

### Task 1: Ordered registry + command metadata + transform commands

**Files:**
- Modify: `wordcartel/src/registry.rs`, `wordcartel/src/transform.rs`
- Test: `wordcartel/src/registry.rs`

**Interfaces:**
- Produces:
  - `pub struct CommandMeta { pub label: &'static str, pub menu: Option<MenuCategory> }`
  - `pub enum MenuCategory { File, Edit, Format, View, Export }` (derive `Clone, Copy, PartialEq, Eq, Debug`) + `pub const MENU_ORDER: [MenuCategory; 5] = [File, Edit, Format, View, Export]`.
  - `Registry { entries: Vec<CommandEntry>, index: HashMap<CommandId, usize> }`, `struct CommandEntry { id: CommandId, handler: Handler, meta: CommandMeta }`.
  - `Registry::register(&mut self, id: &'static str, label: &'static str, menu: Option<MenuCategory>, handler: Handler)`.
  - `pub fn dispatch(&self, id: CommandId, ctx: &mut Ctx) -> CommandResult` (via index); `pub fn resolve_name(&self, name: &str) -> Option<CommandId>` (via index, `Borrow<str>`); `pub fn commands(&self) -> impl Iterator<Item = (CommandId, &CommandMeta)>` (registration order); `pub fn meta(&self, id: CommandId) -> Option<&CommandMeta>`.

- [ ] **Step 1: Write failing tests** in `registry.rs`:
```rust
    #[test]
    fn commands_iterate_in_registration_order_with_meta() {
        let reg = Registry::builtins();
        let ids: Vec<&str> = reg.commands().map(|(id, _)| id.0).collect();
        // deterministic + stable across calls
        let ids2: Vec<&str> = reg.commands().map(|(id, _)| id.0).collect();
        assert_eq!(ids, ids2);
        // every command has a non-empty label
        assert!(reg.commands().all(|(_, m)| !m.label.is_empty()));
        // a known command's meta
        let cut = reg.meta(CommandId("cut")).unwrap();
        assert_eq!(cut.label, "Cut");
        assert_eq!(cut.menu, Some(MenuCategory::Edit));
    }

    #[test]
    fn transforms_are_registered_commands_in_format_category() {
        let reg = Registry::builtins();
        for (id, cat) in [("reflow","Reflow"), ("unwrap","Unwrap"), ("ventilate","Ventilate")] {
            let m = reg.meta(CommandId(id)).unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(m.menu, Some(MenuCategory::Format));
            assert_eq!(m.label, cat);
            assert!(reg.resolve_name(id).is_some());
        }
    }

    #[test]
    fn resolve_name_and_dispatch_still_work_after_refactor() {
        let reg = Registry::builtins();
        assert_eq!(reg.resolve_name("save"), Some(CommandId("save")));
        assert_eq!(reg.resolve_name("nope"), None);
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib registry::` → FAIL.

- [ ] **Step 3: Restructure `Registry`** to the ordered store + metadata:
```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuCategory { File, Edit, Format, View, Export }
pub const MENU_ORDER: [MenuCategory; 5] =
    [MenuCategory::File, MenuCategory::Edit, MenuCategory::Format, MenuCategory::View, MenuCategory::Export];

#[derive(Clone, Copy)]
pub struct CommandMeta { pub label: &'static str, pub menu: Option<MenuCategory> }

struct CommandEntry { id: CommandId, handler: Handler, meta: CommandMeta }

pub struct Registry { entries: Vec<CommandEntry>, index: HashMap<CommandId, usize> }

impl Registry {
    fn register(&mut self, id: &'static str, label: &'static str, menu: Option<MenuCategory>, handler: Handler) {
        let cid = CommandId(id);
        self.index.insert(cid, self.entries.len());
        self.entries.push(CommandEntry { id: cid, handler, meta: CommandMeta { label, menu } });
    }
    pub fn dispatch(&self, id: CommandId, ctx: &mut Ctx) -> CommandResult {
        match self.index.get(&id) { Some(&i) => (self.entries[i].handler)(ctx), None => CommandResult::Noop }
    }
    pub fn resolve_name(&self, name: &str) -> Option<CommandId> {
        self.index.get_key_value(name).map(|(id, _)| *id) // CommandId: Borrow<str> (5a)
    }
    pub fn meta(&self, id: CommandId) -> Option<&CommandMeta> {
        self.index.get(&id).map(|&i| &self.entries[i].meta)
    }
    pub fn commands(&self) -> impl Iterator<Item = (CommandId, &CommandMeta)> {
        self.entries.iter().map(|e| (e.id, &e.meta))
    }
}
```
(Confirm `CommandResult` has a `Noop` variant — it does, per 4b; if the no-command case should differ, match the existing `dispatch` behavior. `index.get_key_value::<str>(name)` works because `CommandId: Borrow<str>` from 5a.)

- [ ] **Step 4: Rewrite `builtins()`** to use `register(id, label, menu, handler)` for EVERY command — preserving today's handlers, adding a human label + category. Transcribe all existing commands (motions/selection = `menu: None` palette-only with labels like `"Move Left"`; the menu-worthy ones get categories). Representative mapping (fill in ALL current commands):
```rust
    pub fn builtins() -> Registry {
        let mut r = Registry { entries: Vec::new(), index: HashMap::new() };
        // motions / selection — palette-only (menu: None)
        r.register("move_left",  "Move Left",  None, |c| run(c, Command::Move { dir: Dir::Left,  extend: false }));
        // … all move_*/select_* …
        // Edit
        r.register("undo",  "Undo",  Some(MenuCategory::Edit), |c| run(c, Command::Undo));
        r.register("redo",  "Redo",  Some(MenuCategory::Edit), |c| run(c, Command::Redo));
        r.register("cut",   "Cut",   Some(MenuCategory::Edit), |c| run(c, Command::Cut));
        r.register("copy",  "Copy",  Some(MenuCategory::Edit), |c| run(c, Command::Copy));
        r.register("paste", "Paste", Some(MenuCategory::Edit), |c| run(c, Command::Paste));
        r.register("filter","Filter…", Some(MenuCategory::Edit), /* existing filter handler */);
        // File
        r.register("save", "Save", Some(MenuCategory::File), /* existing save handler */);
        r.register("quit", "Quit", Some(MenuCategory::File), /* existing quit handler */);
        // View
        r.register("cycle_render_mode", "Cycle Render Mode", Some(MenuCategory::View), |c| run(c, Command::CycleRenderMode));
        // Export (already commands from 4c-1)
        r.register("export_html", "Export HTML", Some(MenuCategory::Export), /* existing */);
        r.register("export_docx", "Export DOCX", Some(MenuCategory::Export), /* existing */);
        r.register("export_pdf",  "Export PDF",  Some(MenuCategory::Export), /* existing */);
        // transform (the Ctrl+T chooser command) stays; + the 3 discrete transform commands (Step 5)
        r
    }
```
**Implementer note:** read the CURRENT `builtins()` and migrate EVERY `map.insert` to a `register(...)` call with a label + category — do not drop any command. Choose labels/categories sensibly; `move_*`/`select_*` are `menu: None`. The `commands_iterate…` test guards that all have labels.

- [ ] **Step 5: Register the 3 transform commands** (`transform.rs` is where dispatch lives; register in `builtins()`):
```rust
        r.register("reflow",    "Reflow",    Some(MenuCategory::Format),
            |c| { crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Reflow,    c.clock, &c.msg_tx); CommandResult::Handled });
        r.register("unwrap",    "Unwrap",    Some(MenuCategory::Format),
            |c| { crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Unwrap,    c.clock, &c.msg_tx); CommandResult::Handled });
        r.register("ventilate", "Ventilate", Some(MenuCategory::Format),
            |c| { crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Ventilate, c.clock, &c.msg_tx); CommandResult::Handled });
```
(`dispatch_transform(editor, kind, clock, msg_tx)` — `Ctx` carries `editor`/`clock`/`msg_tx`. Same dispatch the `Ctrl+T` chooser uses, so behavior is identical.)

- [ ] **Step 6: Run tests + suite.** `cargo test -p wordcartel --lib registry::` then `cargo test --workspace` → all pass; `cargo build --workspace` zero warnings. (`commands()`/`meta()` unused in production until Tasks 3/4 → scoped `#[allow(dead_code)]`.)

- [ ] **Step 7: Commit.**
```bash
git add wordcartel/src/registry.rs wordcartel/src/transform.rs
git commit -m "feat(registry): ordered store + CommandMeta (label/category) + iteration; register reflow/unwrap/ventilate commands"
```

---

### Task 2: `keymap::chord_for` reverse lookup

**Files:**
- Modify: `wordcartel/src/keymap.rs`
- Test: `wordcartel/src/keymap.rs`

**Interfaces:**
- Consumes: `KeyTrie` internal `map: HashMap<Vec<KeyChord>, CommandId>`, `chords_display(&[KeyChord]) -> String` (exists).
- Produces: `pub fn chord_for(&self, id: CommandId) -> Option<String>` on `KeyTrie`.

- [ ] **Step 1: Write the failing test** in `keymap.rs`:
```rust
    #[test]
    fn chord_for_returns_shortest_and_blank_when_unbound() {
        let (t, _) = build_keymap(&crate::config::KeymapConfig::default(), &crate::registry::Registry::builtins());
        assert_eq!(t.chord_for(crate::registry::CommandId("cut")).as_deref(), Some("ctrl-x"));
        // a command with no default binding → None
        assert_eq!(t.chord_for(crate::registry::CommandId("ventilate")), None);
    }
```
(Use ids that the CUA preset binds / doesn't bind; adjust the expected chord to the real CUA binding for `cut`.)

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib keymap::tests::chord_for` → FAIL.

- [ ] **Step 3: Implement `chord_for`** on `KeyTrie`:
```rust
    /// Reverse-lookup: a display chord bound to `id`, or None if unbound.
    /// Shortest sequence wins; ties broken by the rendered string (KeyChord isn't Ord).
    pub fn chord_for(&self, id: CommandId) -> Option<String> {
        self.map.iter()
            .filter(|(_, v)| **v == id)
            .map(|(seq, _)| chords_display(seq))
            .min_by(|a, b| a.chars().count().cmp(&b.chars().count()).then_with(|| a.cmp(b)))
    }
```
(`min_by` on (length, then string) gives the shortest, deterministic on ties.)

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib keymap::` → pass; `cargo test --workspace` green; zero warnings. (`chord_for` unused until Task 3 → scoped `#[allow(dead_code)]`.)

- [ ] **Step 5: Commit.**
```bash
git add wordcartel/src/keymap.rs
git commit -m "feat(keymap): chord_for(CommandId) reverse lookup for in-place shortcut display"
```

---

### Task 3: Command palette (overlay mode + nucleo fuzzy + shared dispatch)

**Files:**
- Modify: `wordcartel/Cargo.toml`, `wordcartel/src/lib.rs`, `wordcartel/src/editor.rs`, `wordcartel/src/app.rs`, `wordcartel/src/render.rs`, keymap presets
- Create: `wordcartel/src/palette.rs`
- Test: `wordcartel/src/palette.rs`, `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `Registry::{commands, resolve_name}`, `keymap::chord_for`, `reg.dispatch`.
- Produces:
  - `palette.rs`: `pub struct PaletteRow { pub id: CommandId, pub label: String, pub chord: String }`; `pub struct Palette { pub query: String, pub cursor: usize, pub rows: Vec<PaletteRow>, pub selected: usize }`; `pub fn rebuild_rows(p: &mut Palette, reg: &Registry, keymap: &KeyTrie)` (rank all commands by `query` via nucleo → rows with label+chord; empty query → all in registration order; updates `selected` clamp).
  - `Editor.palette: Option<Palette>` (init `None`).
  - `app.rs`: the `palette` command (sets `editor.palette = Some(Palette::default())`); `fn hydrate_overlays(editor, reg, keymap)` (build rows for a freshly-opened palette / menu tree for a freshly-opened menu); `fn dispatch_overlay_command(editor, reg, ex, clock, msg_tx, id)` (close overlay, build Ctx, dispatch, drain).
  - `Ctrl+P` bound to `palette` in the keymap presets.

- [ ] **Step 1: Add the dep + module.** `wordcartel/Cargo.toml`: `nucleo-matcher = "0.3"`. `lib.rs`: `pub mod palette;`. `cargo build -p wordcartel` to confirm it resolves.

- [ ] **Step 2: Write failing tests.** In `palette.rs`:
```rust
    #[test]
    fn rebuild_rows_empty_query_lists_all_in_order_with_chords() {
        let reg = crate::registry::Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = Palette::default();
        rebuild_rows(&mut p, &reg, &keymap);
        assert_eq!(p.rows.len(), reg.commands().count(), "empty query → all commands");
        let cut = p.rows.iter().find(|r| r.id == crate::registry::CommandId("cut")).unwrap();
        assert_eq!(cut.label, "Cut");
        assert_eq!(cut.chord, "ctrl-x"); // its CUA chord
    }

    #[test]
    fn rebuild_rows_fuzzy_filters_and_ranks() {
        let reg = crate::registry::Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = Palette { query: "refl".into(), ..Default::default() };
        rebuild_rows(&mut p, &reg, &keymap);
        assert!(p.rows.iter().any(|r| r.id == crate::registry::CommandId("reflow")), "fuzzy 'refl' matches Reflow");
        assert!(p.rows.iter().all(|r| r.label.to_lowercase().contains('r')));
        // no match → empty
        let mut p2 = Palette { query: "zzzzzz".into(), ..Default::default() };
        rebuild_rows(&mut p2, &reg, &keymap);
        assert!(p2.rows.is_empty());
    }
```
In `app.rs`:
```rust
    #[test]
    fn ctrl_p_opens_palette_and_enter_dispatches_selected() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |c, m| Event::Key(KeyEvent { code: c, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // Ctrl+P opens + hydrates
        crate::app::reduce(Msg::Input(press(KeyCode::Char('p'), KeyModifiers::CONTROL)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_some());
        assert!(!e.palette.as_ref().unwrap().rows.is_empty(), "palette hydrated with all commands on open");
        // type "copy", select first, Enter → dispatches copy (register gets the selection)
        for ch in "copy".chars() { crate::app::reduce(Msg::Input(press(KeyCode::Char(ch), KeyModifiers::NONE)), &mut e, &reg, &km, &ex, &clk, &tx); }
        crate::app::reduce(Msg::Input(press(KeyCode::Enter, KeyModifiers::NONE)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_none(), "Enter closes the palette");
        assert_eq!(e.register.get(), Some("abc"), "selected command (Copy) dispatched");
    }

    #[test]
    fn palette_esc_closes_and_nonkey_falls_through() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.palette = Some(crate::palette::Palette::default());
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        // a non-key Msg while palette open still applies (falls through) — e.g. a transform result
        let bid = e.active().id;
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("X".into()) }, &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_some(), "palette still open");
        assert_eq!(e.active().document.buffer.to_string(), "Xab\n", "non-key msg fell through while palette open");
        // Esc closes the palette
        let esc = Event::Key(KeyEvent { code: KeyCode::Esc, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(esc), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_none());
    }
```

- [ ] **Step 3: Run to verify failure.** `cargo test -p wordcartel --lib palette:: ctrl_p_opens palette_esc` → FAIL.

- [ ] **Step 4: Implement `palette.rs`:**
```rust
use crate::registry::{Registry, CommandId};
use crate::keymap::KeyTrie;
use nucleo_matcher::{Matcher, Config};
use nucleo_matcher::pattern::{Pattern, CaseMatching, Normalization};

#[derive(Default)]
pub struct Palette { pub query: String, pub cursor: usize, pub rows: Vec<PaletteRow>, pub selected: usize }
pub struct PaletteRow { pub id: CommandId, pub label: String, pub chord: String }

/// Rebuild the (precomputed) rows from the registry, ranked by `query`. Empty
/// query → all commands in registration order. Updates `selected` (clamped).
pub fn rebuild_rows(p: &mut Palette, reg: &Registry, keymap: &KeyTrie) {
    let all: Vec<(CommandId, &str)> = reg.commands().map(|(id, m)| (id, m.label)).collect();
    let ranked: Vec<CommandId> = if p.query.is_empty() {
        all.iter().map(|(id, _)| *id).collect()
    } else {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pat = Pattern::parse(&p.query, CaseMatching::Ignore, Normalization::Smart);
        // score each label; keep matches; sort by score desc, then registration order (stable).
        let mut scored: Vec<(usize, u32, CommandId)> = all.iter().enumerate()
            .filter_map(|(i, (id, label))| {
                let mut buf = Vec::new();
                let hay = nucleo_matcher::Utf32Str::new(label, &mut buf);
                pat.score(hay, &mut matcher).map(|s| (i, s, *id))
            }).collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scored.into_iter().map(|(_, _, id)| id).collect()
    };
    p.rows = ranked.into_iter().map(|id| PaletteRow {
        id,
        label: reg.meta(id).map(|m| m.label.to_string()).unwrap_or_default(),
        chord: keymap.chord_for(id).unwrap_or_default(),
    }).collect();
    if p.selected >= p.rows.len() { p.selected = p.rows.len().saturating_sub(1); }
}
```
**Implementer note:** confirm the exact `nucleo-matcher` 0.3 scoring call (`Pattern::score(Utf32Str, &mut Matcher) -> Option<u32>`, or `Pattern::match_list(items, &mut matcher) -> Vec<(T,u32)>`). If `match_list` is cleaner, use it (it returns matches sorted desc) and recover registration order as the tiebreak. Reuse ONE `Matcher`.

- [ ] **Step 5: Editor field + commands + reduce wiring** (`editor.rs`, `app.rs`):
  - `editor.rs`: `pub palette: Option<crate::palette::Palette>,` init `None` in `new_from_text`; also clear it where prompts/minibuffer open (XOR) and in `open_minibuffer`.
  - `app.rs` register the `palette` command in `builtins()` (Task 1 file, but the handler is defined where Editor is in scope): `r.register("palette", "Command Palette…", Some(MenuCategory::View), |c| { c.editor.palette = Some(crate::palette::Palette::default()); CommandResult::Handled });`
  - **Hydrate-on-open:** add `fn hydrate_overlays(editor, reg, keymap)` and call it in reduce immediately AFTER any command dispatch (normal-mode arm + `dispatch_overlay_command`): if `editor.palette` is Some with empty rows AND empty query → `rebuild_rows(...)`. (A just-opened palette has empty rows; this fills them so the first render shows all commands.)
  - **`dispatch_overlay_command(editor, reg, ex, clock, msg_tx, id)`:** `editor.palette = None; editor.menu = None;` then `let mut ctx = Ctx{editor, clock, executor: ex, msg_tx: msg_tx.clone()}; reg.dispatch(id, &mut ctx); for r in ex.drain() { apply_result(r, editor); }`.
  - **Palette reduce block** (ABOVE the prompt block, since overlays are top-level): `if editor.palette.is_some() { if let Msg::Input(Event::Key(k)) = &msg { if Press { match k.code: Esc → editor.palette=None; Enter → let id = selected row id; dispatch_overlay_command(.., id) (if any rows); Up/Down → move selected (clamp); Backspace/Left/Right/Char → edit query then rebuild_rows(.., reg, keymap) } } for r in ex.drain(){apply_result(r,editor);} return !editor.quit; } /* non-key falls through */ }`.
  - Esc precedence: the palette block returns early for keys, so its Esc is handled before prompt/minibuffer/pending — matches the spec stack.
  - Keymap preset: add `("ctrl-p","palette")` to the CUA preset (and confirm Ctrl+P is free).

- [ ] **Step 6: Render the palette overlay** (`render.rs`): when `editor.palette.is_some()`, after the normal draw, render a centered overlay — `Clear` over a centered `Rect`, a `Paragraph` (the query line, with caret) on the first row, and a `List` of rows below (each `"{label}"` left + `"{chord}"` right-aligned; highlight `selected`). Clamp the overlay to the area if the terminal is small. (Paint from `editor.palette` state only — no `&reg`/`&keymap` needed in render.)

- [ ] **Step 7: Run tests + suite.** `cargo test -p wordcartel --lib` then `cargo test --workspace` → all pass (palette unit + app integration + prior, with `&km` already threaded from 5a); `cargo build --workspace` zero warnings.

- [ ] **Step 8: Commit.**
```bash
git add wordcartel/Cargo.toml wordcartel/src/lib.rs wordcartel/src/palette.rs wordcartel/src/editor.rs wordcartel/src/app.rs wordcartel/src/registry.rs wordcartel/src/render.rs
git commit -m "feat(palette): Ctrl+P fuzzy command palette (nucleo) — overlay mode, hydrate-on-open, shared dispatch, render"
```

---

### Task 4: Hideable menu bar (`tui-menu`, view over registry)

**Files:**
- Modify: `wordcartel/Cargo.toml`, `wordcartel/src/lib.rs`, `wordcartel/src/editor.rs`, `wordcartel/src/app.rs`, `wordcartel/src/render.rs`, `wordcartel/src/registry.rs` (the `menu` command), keymap presets
- Create: `wordcartel/src/menu.rs`
- Test: `wordcartel/src/menu.rs`, `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `Registry::{commands, meta}`, `keymap::chord_for`, `MENU_ORDER`, `dispatch_overlay_command` (Task 3), `tui_menu::{Menu, MenuItem, MenuState, MenuEvent}`.
- Produces:
  - `menu.rs`: `pub struct MenuView { pub state: tui_menu::MenuState<CommandId>, pub items: Vec<tui_menu::MenuItem<CommandId>> }`; `pub fn build(reg: &Registry, keymap: &KeyTrie) -> MenuView` (group `commands()` with `menu == Some(cat)` by `MENU_ORDER`, omit empty categories; each leaf label = `"{label}    {chord}"`, payload = `CommandId`; add a "Command Palette…" leaf under View routing the `palette` command id).
  - `Editor.menu: Option<MenuView>` (init `None`).
  - the `menu` command (`F10`) sets `editor.menu = Some(MenuView::default-empty)`, hydrated on open.

- [ ] **Step 1: Add the dep + module.** `Cargo.toml`: `tui-menu = "=0.3.0"` (exact pin). `lib.rs`: `pub mod menu;`. `cargo build -p wordcartel` to confirm `tui-menu` 0.3.0 resolves against ratatui 0.29. **Re-confirm the `tui-menu` 0.3.0 API** against its docs/source (`MenuItem::item(label, data)` / `MenuItem::group(label, children)`, `Menu::new(items)`, `MenuState<T>`, `state.drain_events() -> impl Iterator<Item = MenuEvent<T>>`, `MenuEvent::Selected(T)`, and the navigation methods to drive from key events). If any name differs, adapt — report exact names in the report.

- [ ] **Step 2: Write failing tests** in `menu.rs`:
```rust
    #[test]
    fn build_groups_by_category_in_order_with_chords_and_palette_entry() {
        let reg = crate::registry::Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let view = build(&reg, &keymap);
        // top-level groups follow MENU_ORDER, only non-empty categories
        let tops = top_level_labels(&view); // helper: the group labels in order
        assert!(tops.contains(&"Edit".to_string()) && tops.contains(&"Format".to_string()));
        assert!(!tops.contains(&"Insert".to_string()), "empty/absent categories omitted");
        // a Format leaf carries its chord baked into the label (if bound) or just the label
        let fmt = group_items(&view, "Format"); // helper: leaf (label, CommandId) pairs
        assert!(fmt.iter().any(|(label, id)| *id == crate::registry::CommandId("reflow") && label.starts_with("Reflow")));
        // View contains the palette cross-link
        let view_items = group_items(&view, "View");
        assert!(view_items.iter().any(|(_, id)| *id == crate::registry::CommandId("palette")));
    }
```
(Write the small `top_level_labels`/`group_items` helpers against the real `MenuItem` shape — they read the tree you built. If `tui-menu`'s item type doesn't expose its children for inspection, assert against an intermediate `Vec<(MenuCategory, Vec<(String, CommandId)>)>` you build BEFORE converting to `MenuItem`s — structure `build` so the grouping is testable without depending on `tui-menu` internals.)

And in `app.rs`:
```rust
    #[test]
    fn f10_opens_menu_and_selected_event_dispatches() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        // Simulate a menu selection routing: open, then feed a synthesized Selected(copy) via the menu handler path.
        crate::app::reduce(Msg::Input(/* F10 press */ f10()), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_some(), "F10 opens the menu");
        // drive a selection of "copy" through the menu→dispatch path (helper exercising drain_events→dispatch_overlay_command)
        crate::app::menu_select_for_test(&mut e, &reg, &ex, &clk, &tx, crate::registry::CommandId("copy"));
        assert!(e.menu.is_none(), "selection closes the menu");
        assert_eq!(e.register.get(), Some("abc"));
    }
```
(`menu_select_for_test` is a thin `#[cfg(test)]` shim that calls `dispatch_overlay_command(.., copy)` — the real path `drain_events()` feeds; it lets us test routing without simulating tui-menu's internal nav. Define `f10()` building the F10 KeyEvent.)

- [ ] **Step 3: Run to verify failure.** `cargo test -p wordcartel --lib menu:: f10_opens_menu` → FAIL.

- [ ] **Step 4: Implement `menu.rs`** — build the grouped tree (testable intermediate → `MenuItem`s), with chords baked into leaf labels and `CommandId` payloads, "Command Palette…" under View. Structure so the grouping (`Vec<(MenuCategory, Vec<(String /*label+chord*/, CommandId)>)>`) is built + unit-testable BEFORE conversion to `tui_menu::MenuItem`s.

- [ ] **Step 5: Editor field + command + reduce + render wiring** (`editor.rs`/`app.rs`/`render.rs`):
  - `editor.rs`: `pub menu: Option<crate::menu::MenuView>,` init `None`; clear on XOR (prompt/minibuffer/palette open).
  - `builtins()`: `r.register("menu", "Menu Bar", None, |c| { c.editor.menu = Some(crate::menu::MenuView::empty()); CommandResult::Handled });` (palette-only meta — it's a toggle, not a menu entry). Toggle semantics: if already open, the command closes it (`if c.editor.menu.is_some() { c.editor.menu = None } else { Some(empty) }`).
  - `hydrate_overlays`: if `editor.menu` is Some + unbuilt → `*editor.menu = Some(menu::build(reg, keymap))` (build the real tree; `MenuView::empty()` marks "needs build").
  - **Menu reduce block** (ABOVE prompt, alongside palette): `if editor.menu.is_some() { if let Msg::Input(Event::Key(k)) = &msg { Press: Esc → editor.menu=None; arrows/Enter → drive the tui-menu nav methods on editor.menu.state; then drain: for ev in state.drain_events() { if let MenuEvent::Selected(id) = ev { dispatch_overlay_command(.., id); break } } } for r in ex.drain(){apply_result(r,editor);} return !editor.quit; } /* non-key falls through */ }`.
  - Keymap preset: add `("f10","menu")` to CUA.
  - `render.rs`: when `editor.menu.is_some()`, render the menu bar on the top row + the `tui_menu::Menu` (built from `editor.menu.items`) with `editor.menu.state` beneath. Paint from `editor.menu` only.

- [ ] **Step 6: Run tests + suite.** `cargo test --workspace` → all pass; `cargo build --workspace` zero warnings.

- [ ] **Step 7: Commit.**
```bash
git add wordcartel/Cargo.toml wordcartel/src/lib.rs wordcartel/src/menu.rs wordcartel/src/editor.rs wordcartel/src/app.rs wordcartel/src/registry.rs wordcartel/src/render.rs
git commit -m "feat(menu): F10 hideable menu bar (tui-menu view over registry) — categories, chords-in-labels, palette cross-link"
```

---

## Self-Review (5b)

**Spec coverage:** §2 deps/modules (T1/T3/T4); §3 CommandMeta + ordered registry + iteration (T1); §3.1 transform commands (T1); §4 palette overlay + nucleo + dispatch (T3); §5 chord_for (T2); §6 menu via tui-menu, categories, chords-in-labels, palette cross-link, F10 soft (T4); §7.1 render-from-precomputed-state (T3/T4 hydrate-on-open + render paints from Editor state); §7.2 XOR (T3/T4 fields + clear-on-open); §7.3 key-only interception + non-key fallthrough (T3/T4 blocks); §7.4 dispatch_overlay_command (T3, reused T4); §7.5 Esc precedence (T3/T4 blocks above prompt); §8/§10 error handling + tests (each task). ✅

**Codex spec-review fixes reflected:** ordered registry (T1 Vec+index); render reads precomputed state via hydrate-on-open (T3/T4 — render unchanged, sidesteps 5a mem::take); XOR + clear pending on open (T3/T4); key-only interception, non-key fallthrough (T3/T4 + the `palette_esc_closes_and_nonkey_falls_through` test); shared `dispatch_overlay_command` (T3); chord_for tie-break by rendered string + reuse `chords_display` (T2); verified tui-menu 0.3.0 API re-confirmed at impl (T4 Step 1).

**Type consistency:** `CommandMeta`/`MenuCategory`/`MENU_ORDER`/`commands()`/`meta()` (T1) → `chord_for` (T2) → `Palette`/`PaletteRow`/`rebuild_rows`/`Editor.palette`/`dispatch_overlay_command`/`hydrate_overlays` (T3) → `MenuView`/`build`/`Editor.menu` (T4, reusing `dispatch_overlay_command`+`hydrate_overlays`). `reduce` already takes `&KeyTrie` (5a); palette/menu blocks added above the prompt block.

**Implementer-verify markers:** the exact `nucleo-matcher` 0.3 scoring call (`Pattern::score`/`match_list`); the exact `tui-menu` 0.3.0 item/state/event API (T4 Step 1 re-confirm); the current full `builtins()` command list to transcribe with labels/categories (T1); `Ctrl+P`/`F10` free in the keymap; `CommandResult::Noop` for the unknown-id dispatch path; the existing filter/save/quit/export handlers to move into `register(...)` calls.

---

## Execution Handoff

Plan complete. Recommended: **subagent-driven execution** (fresh subagent per task + per-task review), then an opus whole-branch review and a Codex pre-merge gate before merge — the flow that shipped 4b/4c/5a.
