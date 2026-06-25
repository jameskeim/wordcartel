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
- Registered alongside the handler (one registration call supplies handler + meta; plugin-ready per §18.4).
- **Ordered registry (Codex spec-review fix — `HashMap` iteration is nondeterministic):** the registry must store commands in a **stable insertion order** so the palette and menu render the same order every run. Change the representation to an insertion-ordered `Vec<CommandEntry { id, handler, meta }>` plus a `HashMap<CommandId, usize>` index for O(1) `dispatch`/`resolve_name`. `pub fn commands(&self) -> impl Iterator<Item = (CommandId, &CommandMeta)>` yields in registration order. (The menu groups by category but preserves this order within each category; the palette's order is whatever `nucleo` scoring produces, with registration order as the stable tiebreak for equal scores / empty query.)
- **The chord is NOT stored** here — it's reverse-looked-up from the keymap (§5) so it stays correct under user rebinds.

### 3.1 Transform commands (so the palette reaches them by name)
4c-2's Reflow/Unwrap/Ventilate are `PromptAction::Transform(kind)` reached only via the `Ctrl+T` chooser. 5b registers **thin discrete commands** `reflow` / `unwrap` / `ventilate` that route through 4c-2's existing dispatch:
```rust
map.insert(CommandId("reflow"), |c| {
    crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Reflow, c.clock, &c.msg_tx);
    CommandResult::Handled
}); // + unwrap, ventilate; each with CommandMeta { label, menu: Some(Format) }
```
Now the palette reaches them by name, they're individually keymap-bindable, and the `Ctrl+T` chooser + palette share one dispatch. (Export `export_html`/`docx`/`pdf` are already registry commands from 4c-1 — they just gain `CommandMeta`.) **Note (Codex):** a future `TransformKind` variant must be added in BOTH the `Ctrl+T` chooser (`prompt::transform_chooser`) AND here as a registry command — keep them in sync (ideally drive both from one list of `TransformKind`s).

## 4. Command palette (hand-rolled overlay)

- **State** (`palette.rs`): `Palette { query: String, cursor: usize, rows: Vec<PaletteRow>, selected: usize }` where `PaletteRow { id: CommandId, label: String, chord: String }` is **precomputed** (label + reverse-looked-up chord) so render paints directly (§7.1). Lives as `Editor.palette: Option<Palette>` (init `None`).
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

`keymap.rs`: `pub fn chord_for(&self, id: CommandId) -> Option<String>` on `KeyTrie` — scan `self.map` for sequences whose value == `id`; pick the shortest sequence, **tie-broken by the rendered display string** (`KeyChord` is not `Ord`, so compare the rendered `String`s for determinism — Codex note). Render via the existing `chords_display` helper (keymap.rs already has it — reuse, don't add a second renderer). `None` if unbound (palette/menu show blank). Both surfaces call this so shortcuts display in place and track rebinds.

## 6. Hideable menu bar (`tui-menu`, view over the registry)

**Verified `tui-menu` 0.3.0 API (web-confirmed 2026-06-25 — re-confirm at impl time):** `Cargo.toml` declares `ratatui = "0.29"` (matches our pin; 0.3.1 jumped to 0.30 — pin **`=0.3.0`**). Item model: `MenuItem::item(label, data)` and `MenuItem::group(label, children)` — a label String + an arbitrary `Clone` **payload** (our `CommandId`); **no trailing-text field**, so the chord is formatted into the label. `Menu::new(items)` builds the widget; `MenuState<T>` holds the open/highlight nav state (`Editor.menu: Option<MenuState<CommandId>>`). **No built-in event loop** — we call its navigation methods from our crossterm key handling, then `state.drain_events()` yields `MenuEvent::Selected(payload)`. MIT/Apache.

- **State:** `Editor.menu: Option<MenuState<CommandId>>` (the `tui-menu` navigation state; `None` = hidden). The `menu` registry command (bound `F10`) toggles it. **F10 is a soft binding** — some terminals remap it; it's rebindable via config (5a), and the menu is also reachable conceptually only as a convenience (the palette reaches everything).
- **Build (`menu.rs`, each frame):** group the registry's commands by `CommandMeta.menu` into the bar's categories, in a fixed category order (File, Edit, Format, View, Export); within each, a fixed/registration order. Each `tui-menu` item's label embeds its chord (`"Cut    Ctrl+X"`) — `tui-menu` has no trailing-text field, so we format the label ourselves; the item's payload is the `CommandId`. Add a **"Command Palette…"** item under View whose payload dispatches the `palette` command. **Empty categories are omitted** (e.g. `Insert` — nothing maps to it in v1).
- **v1 bar (given today's commands; grows as later sub-efforts add commands):** **File** (Save, Quit) · **Edit** (Undo, Redo, Cut, Copy, Paste, Filter…) · **Format** (Reflow, Unwrap, Ventilate) · **View** (Command Palette…, Cycle Render Mode) · **Export** (HTML, DOCX, PDF).
- **Keys:** we feed `tui-menu` our crossterm key events (arrow nav into/out of dropdowns); after handling, `menu.drain_events()` → `MenuEvent::Selected(command_id)` → `reg.dispatch` (and close the menu). Esc closes the menu. Mouse interaction is **5c** (out of scope here).

## 7. State, render, dispatch & Esc precedence

### 7.1 Render reads from precomputed overlay state (not the registry/keymap)
**Codex spec-review fix:** `render::render(&Editor)` cannot reach the `Registry` (labels) or the `KeyTrie` (chords) — and 5a `mem::take`s the keymap into a `run()` loop-local, so it isn't even on `Editor`. So the palette/menu **precompute their display data in the reduce path** (which holds `&reg` and `&keymap`) and store it in their `Editor` state; render just paints from that state, keeping `render(&Editor)` unchanged.
- `Palette` stores `rows: Vec<PaletteRow { id: CommandId, label: String, chord: String }>` — rebuilt on each query edit in reduce (rank + `chord_for`), so render paints rows directly.
- The menu's `MenuState<CommandId>` + its item labels (with chords baked in) are built when the menu opens (in reduce, via `commands()` + `chord_for`); render hands the prebuilt `Menu` to `tui-menu`.
This means no change to `render`'s signature and no need to thread `&reg`/`&keymap` into render.

### 7.2 Overlay state + the XOR invariant
`Editor` gains `palette: Option<Palette>` and `menu: Option<MenuState<CommandId>>` (both init `None`). **At most ONE of {prompt, minibuffer, palette, menu} is active at a time** (extends 5a's prompt-XOR-minibuffer). The `palette`/`menu` commands **only open in normal mode** — if a prompt or minibuffer is open, their keys are already intercepted upstream so the command never fires (consistent with how `Ctrl+T`/`Ctrl+P` can't reach dispatch under a modal). Palette and menu are mutually exclusive with each other too: the menu's "Command Palette…" entry closes the menu, then opens the palette.

### 7.3 Key interception (async results still flow)
While a palette or menu is open, its reduce block intercepts **only key events** (`Msg::Input(Event::Key(_))`) and returns early — exactly like the minibuffer block. **Non-key messages** (`JobDone`/`FilterDone`/`ExportDone`/`TransformDone`/`Tick`/`Resize`) **fall through** to their normal handlers, so a background save/filter/transform/export completing while the palette is open still applies. Opening a palette/menu clears `pending_keys` (mirroring 5a's minibuffer rule).

### 7.4 Shared overlay-dispatch helper
**Codex spec-review fix (avoid duplication):** palette-Enter and menu-`Selected` both need the same sequence as the keymap path — close the overlay, build `Ctx`, `reg.dispatch(id, &mut ctx)`, drain results. Factor `fn dispatch_overlay_command(editor, reg, id, ex, clock, msg_tx)` that closes the active overlay FIRST, then runs the normal dispatch+drain. A command that itself opens a prompt/minibuffer (e.g. `filter`, `transform`) thus runs after the overlay is closed → the new modal opens cleanly (no two overlays at once).

### 7.5 Esc precedence (extends 5a's stack)
**active overlay (palette/menu) Esc closes it > prompt-dismiss > minibuffer-dismiss > pending-sequence-cancel > filter-cancel > keymap dispatch.** The palette/menu key block sits above the prompt/minibuffer blocks (an overlay can only be open in normal mode, so this ordering is unambiguous).

### 7.6 Instant feedback (§3.9)
Opening either surface is immediate. Dispatch inherits 4c-2/4c-1 behavior: a **small** transform/filter region runs synchronously (sub-frame — fine), a **large** one or an export uses the existing async status path. So "never freezes" means the overlay open + the loop are never blocked; a genuinely large operation shows the existing status/spinner, not a freeze.

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
- **XOR + async-fallthrough:** the `palette`/`menu` commands don't open while a prompt/minibuffer is active (and opening clears `pending_keys`); a non-key `Msg` (e.g. a synthesized `TransformDone`/`FilterDone`) delivered while the palette is open still applies to the buffer (falls through), not swallowed.
- **Ordered enumeration:** `commands()` yields a deterministic (registration) order across runs; the menu groups by category preserving it.
- **Overlay dispatch helper:** `dispatch_overlay_command` closes the active overlay before dispatching, so a command that opens a modal (filter/transform) leaves exactly one overlay/modal active.
- `tui-menu`/ratatui widget *rendering* is exercised via their APIs (build + drain), not pixel snapshots. No prior test weakened; `cargo build --workspace` zero warnings; `wordcartel-core` untouched.

## 11. Non-goals (explicit)

- **Mouse interaction** with the menu/palette → 5c (mouse work).
- **`Insert` menu category content** → nothing maps to it in v1; the category is omitted until a command needs it.
- **Non-command "filter presets" as discrete palette entries** → `filter` stays a single command that opens its input; individual saved filter presets are a later/config concern.
- **A scrollable palette beyond the visible window** → v1 shows the top-N ranked matches in the overlay; full scroll is a later refinement if needed.
- **ratatui 0.30 upgrade** → separate future effort (would free `tui-menu` ≥0.3.1 / `tui-popup` ≥0.7).
