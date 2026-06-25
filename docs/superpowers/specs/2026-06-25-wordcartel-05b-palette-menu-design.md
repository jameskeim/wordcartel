# Wordcartel Effort 5b — Command Palette + Hideable Menu (design)

**Status:** design (brainstormed + crate-verified 2026-06-25)
**Parent:** Effort 5 (App) — sub-effort 5b (after 5a config+keymap, merged). Builds on the 4b name-keyed registry + 5a's keymap.
**Spec source:** main design §12.2 (command discovery: palette + hideable menu), §3.9 (instant feedback / no menu-lag), §10.4 (registry is the dispatch boundary), §18.4 (plugin substrate registers the same way).

---

## 1. Goal

Make every command **discoverable and reachable** without memorizing chords, via two complementary surfaces — both **view layers over the name-keyed command registry** (a widget renders + routes a selection back to a `CommandId`; it never owns the command list):
1. **Command palette** (primary power path): `Ctrl+P` opens a fuzzy-searchable overlay listing **every** command (each with its current chord); type to filter, Enter dispatches. The safety net that makes no command unreachable even if unbound.
2. **Hideable menu bar** (browsable common actions): `F10` toggles a `File / Edit / Format / View / Export` bar (hidden by default for distraction-free editing); each entry shows its chord; a "Command Palette…" entry cross-links to the palette.

Both render **instantly with feedback** (§3.9) — the surface opens immediately; any slow command they trigger shows status, never freezes.

## 2. Architecture

Shell-crate work (`wordcartel`); `wordcartel-core` untouched. The registry (4b) stays the single source of truth for commands; 5b adds **display metadata** to it and two **view** surfaces that enumerate it.

- **New files:** `wordcartel/src/palette.rs` (palette state + `nucleo-matcher` ranking + dispatch), `wordcartel/src/menu.rs` (build the `tui-menu` tree from the registry by category + route selections).
- **Modified:** `registry.rs` (per-command `CommandMeta` + iteration), `transform.rs` (3 thin transform commands), `keymap.rs` (`chord_for` reverse lookup), `app.rs` (palette/menu modes, `Ctrl+P`/`F10`, Esc precedence), `render.rs` (overlay stack + menu bar), the keymap presets (bind `palette`/`menu`).
- **New deps (shell crate only; verified ratatui-0.29-safe 2026-06-25):**
  - `nucleo-matcher = "0.3"` — synchronous fuzzy scorer (MPL-2.0; no ratatui dep; instant for a few-hundred-item static list). NOT the async `nucleo` / the `nucleo-picker` app.
  - `tui-menu = "=0.3.0"` — the **last ratatui-0.29 release** (0.3.1 moved to ratatui 0.30; pin exactly `=0.3.0`). MIT/Apache. A pure view: we build the tree, drive it with our crossterm keys, and read selections via `drain_events()`.
  - **No** `tui-popup`/`tui-overlay` — the palette overlay is hand-rolled (`Clear` + centered `Rect` + `List`). *(A ratatui 0.29→0.30 upgrade, which would free the newer widget versions, is a separate future effort.)*

## 3. Command metadata (the foundation)

The registry today keys `CommandId(&'static str) -> Handler` with no display info. 5b adds, per command, a `CommandMeta`:
```rust
#[derive(Clone, Copy)]
pub struct CommandMeta { pub label: &'static str, pub menu: Option<MenuCategory> }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuCategory { File, Edit, Format, View, Export }
```
- `label` is the human name shown in the palette + menu (e.g. `"Cut"`, `"Reflow"`, `"Export → HTML"`).
- `menu: Some(cat)` → the command also appears in the menu bar under that category; `menu: None` → **palette-only** (e.g. obscure motions). The palette lists **all** commands regardless of `menu`.
- Registered alongside the handler (one registration call supplies handler + meta; plugin-ready per §18.4). The registry exposes `pub fn commands(&self) -> impl Iterator<Item = (CommandId, &CommandMeta)>` for the palette/menu to enumerate, plus the 5a `resolve_name`.
- **The chord is NOT stored** here — it's reverse-looked-up from the keymap (§5) so it stays correct under user rebinds.

### 3.1 Transform commands (so the palette reaches them by name)
4c-2's Reflow/Unwrap/Ventilate are `PromptAction::Transform(kind)` reached only via the `Ctrl+T` chooser. 5b registers **thin discrete commands** `reflow` / `unwrap` / `ventilate` that route through 4c-2's existing dispatch:
```rust
map.insert(CommandId("reflow"), |c| {
    crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Reflow, c.clock, &c.msg_tx);
    CommandResult::Handled
}); // + unwrap, ventilate; each with CommandMeta { label, menu: Some(Format) }
```
Now the palette reaches them by name, they're individually keymap-bindable, and the `Ctrl+T` chooser + palette share one dispatch. (Export `export_html`/`docx`/`pdf` are already registry commands from 4c-1 — they just gain `CommandMeta`.)

## 4. Command palette (hand-rolled overlay)

- **State** (`palette.rs`): `Palette { query: String, cursor: usize, matches: Vec<CommandId>, selected: usize }`. Lives as `Editor.palette: Option<Palette>` (init `None`).
- **Open:** the `palette` registry command (bound `Ctrl+P`) sets `editor.palette = Some(Palette::new())`. (Cross-link: the menu's "Command Palette…" entry dispatches this same command.)
- **Filter:** on each query edit, rank **all** commands' labels with `nucleo-matcher`:
  ```rust
  let mut matcher = Matcher::new(Config::DEFAULT);
  let pat = Pattern::parse(&query, CaseMatching::Ignore, Normalization::Smart);
  // score each (CommandId, label); keep matches sorted by score desc; empty query → all, label order
  ```
  Reuse one `Matcher` across keystrokes. An empty query lists all commands; no match → empty list + a "(no match)" hint.
- **Rows:** each visible row shows `label` left + its chord (§5) right-aligned (blank if unbound).
- **Keys (palette is an input mode):** printable → edit `query`; Backspace/Left/Right edit; ↑/↓ move `selected`; Enter → dispatch the selected `CommandId` via `reg.dispatch` (then close); Esc → close. While open, the palette intercepts key input (like the minibuffer) and returns early.

## 5. Chord reverse-lookup

`keymap.rs`: `pub fn chord_for(&self, id: CommandId) -> Option<String>` on `KeyTrie` — scan `self.map` for sequences whose value == `id`, pick the shortest (ties: lexicographically first for determinism), render it back to a display string (e.g. `"ctrl-x"`, `"ctrl-k ctrl-s"`) via the inverse of `parse_chord`. `None` if unbound (palette/menu show blank). Both surfaces call this so shortcuts display in place and track rebinds.

## 6. Hideable menu bar (`tui-menu`, view over the registry)

- **State:** `Editor.menu: Option<MenuState>` (the `tui-menu` navigation state; `None` = hidden). The `menu` registry command (bound `F10`) toggles it.
- **Build (`menu.rs`, each frame):** group the registry's commands by `CommandMeta.menu` into the bar's categories, in a fixed category order (File, Edit, Format, View, Export); within each, a fixed/registration order. Each `tui-menu` item's label embeds its chord (`"Cut    Ctrl+X"`) — `tui-menu` has no trailing-text field, so we format the label ourselves; the item's payload is the `CommandId`. Add a **"Command Palette…"** item under View whose payload dispatches the `palette` command. **Empty categories are omitted** (e.g. `Insert` — nothing maps to it in v1).
- **v1 bar (given today's commands; grows as later sub-efforts add commands):** **File** (Save, Quit) · **Edit** (Undo, Redo, Cut, Copy, Paste, Filter…) · **Format** (Reflow, Unwrap, Ventilate) · **View** (Command Palette…, Cycle Render Mode) · **Export** (HTML, DOCX, PDF).
- **Keys:** we feed `tui-menu` our crossterm key events (arrow nav into/out of dropdowns); after handling, `menu.drain_events()` → `MenuEvent::Selected(command_id)` → `reg.dispatch` (and close the menu). Esc closes the menu. Mouse interaction is **5c** (out of scope here).

## 7. Render + Esc precedence

- **Render (`render.rs`):** the palette is a centered overlay — `Clear` over a centered `Rect`, then a `Paragraph` (query line) + a `List` (ranked rows with right-aligned chords). The menu bar renders on the top row when open, with `tui-menu`'s dropdown beneath. Maintain a simple z-order: the **active overlay** (palette OR menu) draws last (on top).
- **Mutual exclusion:** palette and menu are mutually-exclusive top-level overlay modes — opening one closes the other (the menu's "Command Palette…" closes the menu, then opens the palette).
- **Esc precedence (extends 5a's stack):** **active overlay (palette/menu) Esc closes it > prompt-dismiss > minibuffer-dismiss > pending-sequence-cancel > filter-cancel > keymap dispatch.** While a palette/menu is open, its block intercepts key input before the keymap resolver (so a pending key sequence can't be mid-flight under an overlay; opening an overlay clears `pending_keys`, mirroring the minibuffer rule).
- **Instant feedback (§3.9):** opening either surface is immediate; a command it dispatches that kicks off slow async work (filter/transform/export) shows the existing status/spinner — the overlay never blocks the loop.

## 8. Error handling & edge cases

- Dispatching a command from the palette/menu behaves **exactly** as the keymap path (same `reg.dispatch`); a command that opens a prompt/minibuffer (e.g. `filter`, `transform`) closes the palette/menu first, then opens its own modal.
- Empty palette query → all commands; no fuzzy match → empty list + "(no match)" (never an error).
- A command with no bound chord → still listed/dispatchable; chord column blank.
- Terminal too small for the centered palette → clamp the overlay to the available area (don't panic / mis-render).
- The menu omits empty categories; if (hypothetically) the registry had zero menu-categorized commands, the bar shows only "Command Palette…".

## 9. Components / boundaries

| Unit | Responsibility | Depends on |
|------|----------------|------------|
| `registry.rs` `CommandMeta` + `commands()` iter | per-command display metadata + enumeration | — |
| `transform.rs` reflow/unwrap/ventilate commands | thin registry commands → 4c-2 dispatch | `dispatch_transform` |
| `keymap.rs::chord_for` | reverse CommandId→chord for display | `KeyTrie` |
| `palette.rs` (`Palette` + rank + dispatch) | fuzzy palette state/logic | `nucleo-matcher`, registry, keymap |
| `menu.rs` (build tree + route) | menu-bar view over registry by category | `tui-menu`, registry, keymap |
| `app.rs` modes + Esc | palette/menu open/close, key interception, dispatch | reduce, registry |
| `render.rs` overlay stack + menu bar | paint palette overlay + menu | ratatui `Clear`/`List`, `tui-menu` |

## 10. Testing (no TTY)

- **Metadata + iteration:** every registry command has a `CommandMeta`; `commands()` yields them; the 3 transform commands + the export commands have the right `MenuCategory`.
- **Transform commands:** dispatching `reflow`/`unwrap`/`ventilate` routes through 4c-2 (apply the transform), equivalent to the chooser path.
- **Chord reverse-lookup:** `chord_for` returns the bound chord (shortest on ties), blank when unbound, and reflects a rebind.
- **Palette logic:** `nucleo` ranks matching labels for a query (and orders by score); empty query → all; no match → empty. Open→type→Enter dispatches the selected `CommandId` (assert via a fake/echo command or the resulting editor effect); Esc closes; opening the palette clears `pending_keys`.
- **Menu:** the tree built from the registry has the expected categories/items with chords embedded in labels; a synthesized `MenuEvent::Selected(id)` routes to `reg.dispatch`; "Command Palette…" opens the palette; empty categories omitted.
- **Esc precedence:** with a palette/menu open, Esc closes it and does NOT fall through to pending-cancel/filter-cancel; prompt/minibuffer still dismiss correctly.
- `tui-menu`/ratatui widget *rendering* is exercised via their APIs (build + drain), not pixel snapshots. No prior test weakened; `cargo build --workspace` zero warnings; `wordcartel-core` untouched.

## 11. Non-goals (explicit)

- **Mouse interaction** with the menu/palette → 5c (mouse work).
- **`Insert` menu category content** → nothing maps to it in v1; the category is omitted until a command needs it.
- **Non-command "filter presets" as discrete palette entries** → `filter` stays a single command that opens its input; individual saved filter presets are a later/config concern.
- **A scrollable palette beyond the visible window** → v1 shows the top-N ranked matches in the overlay; full scroll is a later refinement if needed.
- **ratatui 0.30 upgrade** → separate future effort (would free `tui-menu` ≥0.3.1 / `tui-popup` ≥0.7).
