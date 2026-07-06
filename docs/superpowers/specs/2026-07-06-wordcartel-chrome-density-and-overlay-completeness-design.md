# Chrome Density Presets + Overlay/Mouse Completeness — Design

**Status:** design; Codex spec gate round 1 folded (3 fixups + 1 wording), re-review pending.
**Effort:** `effort-chrome-density-overlays` (branch off `main`).
**Supersedes backlog items:** E1 (chrome/density presets), E2 (visual polish), plus the
overlay/mouse-completeness and menu-windowing gaps surfaced 2026-07-06.

**Goal.** Turn the shipped `[theme] chrome = zen|full` color axis into a full **density
preset** that also drives chrome *visibility*, make every overlay/modal mouse-complete to the
command palette's standard, and give the menu dropdown the windowing the other overlays already
have — so tall menus no longer truncate on short screens.

**Architecture.** A density preset is **data** — a bundle of `element → value` applied by one
general routine — not hardcoded `if zen {…}` branching. Two built-in bundles (`zen`, `full`)
this effort; the shape is deliberately extensible so config-defined bundles (L2) and named
profiles that also set the theme (L3) are *additive future efforts*, not rewrites.

**Tech stack.** `wordcartel-core` (pure model: theme/faces) + `wordcartel` shell (ratatui 0.30,
crossterm). No new dependencies.

---

## Global Constraints

- **No-silent-UI is inviolable.** Any status message — errors above all — must become visible
  regardless of density mode. A "hide the status line outright" option is rejected on principle.
- **No hot-path jank.** The reveal/hide of transient chrome must not reflow the document under
  the writer's hands (see §1.3, the reserved-status-row decision). Per-keystroke work stays
  `O(visible)`.
- **House style / GATEs:** `cargo test` green (core + shell), `cargo clippy --workspace
  --all-targets` clean (deny), no `cargo fmt`, em-dash prose comments, no decorative/emoji
  unicode in code, exhaustive matches on `SemanticElement`/`ChromeDisposition`/`CanvasMode`
  (no catch-all `_` that silently absorbs a new variant). Smoke suite mandatory-run/advisory.
- **Individual overrides win.** An explicit user config key (e.g. `[menu] bar = pinned`) always
  overrides a preset's default for that element and persists across restarts.

---

## Real-code anchors (verified against the branch, for reviewer cross-check)

- Color axis: `ChromeDisposition { Full, Zen }` (`wordcartel-core/src/theme.rs:45`), the *only*
  consumer `Theme::derive_chrome(disp)` (`theme.rs:233`) which selects the WCAG contrast target
  (`Full → FULL_STEP_CR`, `Zen → SEP_FLOOR_CR`, `theme.rs:251`) + `ZEN_ACCENT_EXTRA`
  (`theme.rs:331`). Parse `parse_chrome` (`theme_resolve.rs:62`); command `toggle_chrome`
  (`registry.rs:482`, fn `:544`); runtime `Editor.chrome_disposition` (`editor.rs:423`) +
  `theme_rederive` flag (`editor.rs:429`) consumed by `rederive_theme_if_requested`
  (`app.rs:225`); persists via `SettingsSnapshot.chrome_disposition` (`settings.rs:46`).
  **This axis is theming-only today — it changes chrome COLORS, hides/shows nothing.**
- Visibility toggles: `MenuBarMode { Hidden, Auto, Pinned }` (`config.rs:79`), `MenuConfig.bar`
  (`config.rs:83`), `Editor.menu_bar_mode` (`editor.rs:393`), the geometry authority
  `Editor::menu_bar_rows()` (`editor.rs:550`), command `menu_bar_pin` (`registry.rs:438`);
  `ViewConfig { measure, wrap_column, wrap_guide, word_count }` (`config.rs:95`), commands
  `toggle_measure`/`toggle_wrap_guide`/`toggle_word_count` (`registry.rs:387`); scrollbar state
  in `MouseState` (`editor.rs:330`) recomputed by `recompute_scrollbar_visible`
  (`app.rs:1665`); status row rendered unconditionally (`render.rs:745`), `edit_height =
  h - 1(status) - menu_rows` (`render.rs:362`).
- Dwell machinery (to generalize): `menu_reveal_due`/`menu_hide_due`/`menu_bar_revealed` in
  `MouseState` (`editor.rs:324`), armed `mouse.rs:98`, fired `recompute_menu_bar`
  (`app.rs:1671`), constants `MENU_DWELL_MS`/`MENU_LEAVE_GRACE_MS` (`mouse.rs:7`).
- Menu model: `CommandMeta { label, menu }` (`registry.rs:44`, **no state field**),
  `grouped_commands` (`menu.rs:30`), `MenuView { groups, open, highlighted, built }`
  (`menu.rs:4`, **no scroll_top**), keymap radio hook `active_keymap_preset` (`editor.rs:384`),
  `switch_keymap_preset` (`registry.rs:528`).
- Styling: `ChromeStyles` (`render.rs:267`, built `:298`), overlay painters
  (`render_overlays.rs:35`), fg-only `overlay_border` (`render.rs:306`).
- Windowing: `list_window::list_h_for` (`list_window.rs:11`) + `keep_visible` (`:19`),
  `keep_overlay_visible` (`app.rs:121`), `windowed_indicator` (`render.rs:171`),
  `palette_overlay_rect` (`render.rs:184`, used by all list overlays), the outlier
  `menu_dropdown_rect` (`render.rs:151`, raw leaf count, no window).
- Overlay structs + mouse: `mouse.rs::handle` (`mouse.rs:72`) has branches for palette
  (scroll+click+click-away), menu (**consumes every event via `return` at `mouse.rs:174-200`;
  only left-click currently acts — §4 adds a scroll arm, this is NOT a leak fix**), theme_picker
  (scroll only), file_browser (scroll only), each `return`-consuming; **`prompt`/`minibuffer`/
  `outline`/`diag`/`search` have NO branch → mouse falls through to the editor match
  (`mouse.rs:231`).** Ordering caveat: the menu-bar dwell-arming block (`mouse.rs:94-119`) runs
  BEFORE these branches and its exclusion gate (`mouse.rs:107-110`) lists only
  menu/palette/theme_picker/file_browser — see Part 3's ordering requirement. Structs with a
  `scroll_top`: `Palette` (`palette.rs:18`), `ThemePicker` (`theme_picker.rs:6`), `FileBrowser`
  (`file_browser.rs:13`), `OutlineOverlay` (`outline_overlay.rs`). Without: `MenuView`,
  `Prompt` (`prompt.rs:43`, key-driven status-row choices), `Minibuffer` (`minibuffer.rs:19`),
  `DiagOverlay` (`diag_overlay.rs`, windows via inline `.min(15)` at `render_overlays.rs:364-370`
  → gains a real `scroll_top`, Part 3).
- Persistence pattern (6 steps): config field + default → `Raw*` fold w/ validation → runtime
  field → command → `SettingsSnapshot`/`O*` mirror + `diff_key` block (`settings.rs:252`) →
  startup seed in `app.rs::run`.

---

## Part 1 — E1: density presets

### 1.1 Unify onto `chrome = zen|full` (Fork 1 = A)

`[theme] chrome = zen|full` continues to drive `derive_chrome`'s color derivation **and now
also** drives the visibility bundle. One key, one `toggle_chrome` command. No new top-level
axis (avoids a third `zen|full`-flavored key colliding with the existing `chrome` and `canvas`
axes). Tradeoff accepted: color follows the mode; independent visibility is reached via
per-element overrides (§1.5).

### 1.2 The bundle model (Level 1, built for L2/L3)

Introduce a `ChromeBundle` record (in `wordcartel` shell, near the density logic) holding the
resolved target for each preset-owned element:

- `chrome_disposition: ChromeDisposition` (color; already exists)
- `menu_bar: MenuBarMode`
- `status_line: TransientMode` (new enum — see §1.3)
- `scrollbar: TransientMode` (new — scrollbar has no config today)
- `measure: bool`
- `word_count: bool`
- right-edge content: **deferred** (§1.4) — not a bundle field this effort.

Two built-in constants `ZEN` and `FULL` per the table (§1.3). One routine
`apply_bundle(&mut Editor, &ChromeBundle)` sets each element via its existing runtime field
(`editor.menu_bar_mode`, `editor.view_opts.measure`, …) — never hardcoded branching. The set of
selectable bundles is enumerable so the command/palette can list them generically (today
`[zen, full]`). **L2/L3 readiness (design constraint, reviewer-checkable):** the apply routine
takes a `&ChromeBundle` regardless of origin (built-in constant vs future config-deserialized
vs future named-registry entry), and bundle fields are additive (L3 adds `theme`, `keymap`,
… without touching `apply_bundle`'s mechanism).

### 1.3 The visibility table + the transient "Auto" mode (Fork 2, Fork 3 = A)

| Element | Zen | Full |
|---|---|---|
| Chrome colors | `Zen` (muted) | `Full` (elevated) |
| Menu bar | `Auto` (dwell) | `Pinned` |
| Status line | `Auto` (dwell + message) | `On` |
| Scrollbar | `Auto` (dwell + activity) | `On` |
| Centered measure | `On` | `Off` |
| Word count | `Off` | `On` |
| Wrap guide · Focus-dim · Typewriter · Heading glyphs | untouched | untouched |

**Transient "Auto" mode.** A new `TransientMode { Off, Auto, On }` unifies the three
reveal-on-dwell elements. `Auto` = reveal on pointer dwell near the element's region **plus** a
context trigger, hide after leave-grace. The menu bar already implements exactly this
(`menu_reveal_due`/`menu_hide_due`); the menu bar keeps its existing `MenuBarMode`
(`Hidden`/`Auto`/`Pinned` map to `Off`/`Auto`/`On`). Status and scrollbar gain the analogous
dwell timers (mirroring `mouse.rs:98` + `recompute_menu_bar`), with element-specific triggers:

- Menu bar — dwell at top row, or F10/menu-open. (Existing.)
- Scrollbar — dwell at the right-edge column, or scroll activity (`scrollbar_until_ms` already
  exists). Adds a right-edge dwell timer.
- Status line — dwell at the bottom row, or **any status message/prompt** (forced; the
  no-silent-UI guarantee). A prompt/minibuffer/search keeps it shown while active. **The status
  line has no true `Off`:** a message force-reveals the reserved row even if a bundle were to
  set `TransientMode::Off`, so the only meaningful status modes are `Auto` and `On` (the table
  never assigns it `Off`). The shared enum still carries `Off` for the scrollbar, which *can* be
  fully suppressed.

**Status line = RESERVED-SPACE model (Fork 3 = A, the anti-jank decision).** The bottom row is
**always reserved** — `edit_height` keeps its `-1` for status unconditionally (no geometry
change to `render.rs:362`). "Hidden" means the row renders as calm canvas (blank/base bg) while
idle; a message/prompt/dwell paints *into that reserved row* — revealing it visually with
**zero layout shift**. This preserves no-silent-UI (messages always paint) and no-jank (text
never moves). Consequence: zen's status payoff is visual calm, not a reclaimed line. (The menu
bar retains its existing reflow-on-reveal via `menu_bar_rows()`; only the status row is
reserve-not-reclaim, because its trigger — messages — is involuntary and frequent.)

Keyboard-only users are covered: menu → F10; status → messages (always); scrollbar → scroll
keys. `Auto` degrades to "reveal on the element's non-dwell trigger" without a mouse.

### 1.4 Right-edge content (Fork 5 = C)

Deferred. Full mode's top bar is the elevated pinned bar with a themed-but-empty right region
(current fill behavior). No content widget this effort; recorded as a future refinement.

### 1.5 Overrides + persistence

`apply_bundle` sets the preset's defaults; an explicit user config key overrides that element
and persists (the existing diff-law, `settings.rs:252`). Re-selecting a preset re-applies its
bundle over unsaved runtime state. New persisted fields: `scrollbar` mode and `status_line`
mode each follow the 6-step new-key pattern (config field + `Raw*` fold at `config.rs:251-277` +
runtime field + command + `SettingsSnapshot`/`O*` mirror at `settings.rs:142-175` + `diff_key`
block at `settings.rs:252-259` + startup seed at `app.rs:1346-1358`). **TOML schema (Codex spec
gate — pinned):** both new keys live under `[view]`, matching the existing visibility toggles
(`measure`/`wrap_guide`/`word_count`): `[view] scrollbar = "auto" | "on" | "off"` and
`[view] status_line = "auto" | "on"`. `status_line` accepts only `auto`/`on` — a config `off` is
rejected by the parser (coerced to `auto` with a status warning) to honor no-silent-UI; the
shared `TransientMode::Off` exists solely for the scrollbar, which can be fully suppressed. The
menu-bar/measure/word-count keys already persist (`[menu] bar`, `[view]`). `chrome` disposition
already round-trips under `[theme]`.

---

## Part 2 — E2: visual polish

### 2.1 State-in-label menu items (Fork 4 = A)

Stateful menu rows show state **in the label text**, no glyphs: on/off toggles render
`Wrap Guide: On` / `Word Count: Off` / `Chrome: Zen`; radio groups collapse to a single
`Setting: Value` row (`Keymap: CUA`). Mechanism: add an optional
`state: Option<fn(&Editor) -> MenuMark>` to `CommandMeta` (or a parallel table keyed by
`CommandId`), where `MenuMark` carries the value to interpolate into the label. `grouped_commands`
gains `&Editor` so it can evaluate each command's live state at build time (state is owned by
`Editor`, not the registry). Rows with no state fn render their static label unchanged.
Exhaustive handling — no catch-all that silently drops a new stateful command.

### 2.2 Two-archetype styling language (Fork 8 = A)

No new styling primitives (E3 shipped the six-face family, `derive_chrome`, `ChromeStyles`,
fg-only borders, full-width fills). E2 = apply the shipped language consistently across two
overlay archetypes:

- **Floating overlays** (palette, theme picker, file browser, outline, diag): centered modals,
  **bordered boxes** (existing `palette_overlay_rect` + `overlay_border` + titled block).
- **Attached menu dropdown**: a **filled elevated panel** (its Chrome/Muted bg, no box border),
  reading as extending down from the bar; the `n/total` overflow indicator (from §4) sits on its
  bottom row.

Both reuse the identical chrome faces (`ChromeSelected` highlight, elevation, `windowed_indicator`).

---

## Part 3 — overlay/mouse completeness (Fork 7 = A)

**Universal no-leak guard.** While any overlay/modal is open, mouse events are consumed by the
overlay layer — never fall through to the editor match (`mouse.rs:231`). Add consuming branches
for `prompt`, `minibuffer`, `outline`, `diag`, `search` (today absent).

**Ordering requirement (Codex spec gate).** The overlay guard / dwell-suppression must run
*before* the menu-bar dwell-arming block (`mouse.rs:94-119`), not after. Today the dwell gate at
`mouse.rs:107-110` excludes only `menu`/`palette`/`theme_picker`/`file_browser`; a row-0 pointer
move over `prompt`/`minibuffer`/`search`/`diag`/`outline` would otherwise still arm/reveal the
menu bar before any late-added consuming branch fires. The plan MUST place a single "any overlay
open → suppress dwell + consume" guard ahead of the dwell-arming block (preferred over an
enumerated exclusion gate that a future overlay can forget to join).

**List overlays → palette standard.** Theme picker and file browser gain click-to-commit +
click-away (they already scroll). `outline` and `diag` gain the full set (scroll + click +
click-away; they have no mouse today). All reuse the palette's row-hit-test + click-away pattern
(`palette_row_at` analog per overlay).

**`DiagOverlay` gains real `scroll_top` (Codex spec gate).** Diag today windows via an inline
`.min(15)` cap (`render_overlays.rs:364-370`) with no scroll state, so wheel/click would run past
the rendered rows and the hit-test could not match the visible window. Diag therefore gets a real
`scroll_top: usize` (like `Palette`/`ThemePicker`/`FileBrowser`) plus `list_window::{list_h_for,
keep_visible}` windowing — the same treatment Part 4 gives the menu — replacing the inline cap.
This closes diag's latent tall-list truncation too, consistent with the effort's completeness goal.

**Click = commit, uniformly.** One click on a row applies it: theme picker → apply theme, file
browser → open file / enter directory, outline → jump, diag → apply/goto. Matches the shipped
palette (click = dispatch). Preview stays available via scroll/arrow before the click.

**Prompt choices clickable.** The status-row prompt (dirty-quit `[S]ave [D]iscard [C]ancel`,
save-as) becomes mouse-operable — clicking a choice region activates its `PromptAction`.

**Text-input modals** (`minibuffer`, `search`): consume mouse (no leak); no row-clicking (you
type). Click-to-position-cursor is out of scope.

---

## Part 4 — menu windowing (Fork 6 = A)

Give the menu the windowing every other list overlay already has:

- `MenuView` gains `scroll_top: usize`.
- `menu_dropdown_rect` (`render.rs:151`) windows its height via `list_window::list_h_for`
  against the space below the open label (instead of raw `leaves.len()`), so a tall category
  scrolls rather than truncating.
- Keyboard ↑/↓ in the dropdown calls `list_window::keep_visible(highlighted, len, list_h,
  &mut scroll_top)` — the highlight is always dragged into view.
- `mouse.rs` menu branch gains a `ScrollUp/Down` arm (today it consumes all events but only
  left-click acts — the scroll arm adds behavior, it is not a leak fix).
- The `windowed_indicator` `" n/total "` renders on the dropdown's bottom row (the attached
  filled panel per §2.2) when the category overflows the window.

---

## Out of scope (explicit)

- **Heading-glyph runtime command + persistence gap** — `heading_level_glyph` has a config field
  but no runtime command and no `SettingsSnapshot` entry; the preset leaves it untouched, and this
  pre-existing gap is not closed here.
- **Focus-dim / typewriter** — writing modes, not visibility chrome; untouched.
- **L2 (config-defined bundle contents) / L3 (named profiles that also set theme/keymap)** —
  deliberate future efforts; this effort only guarantees the architecture leaves room for them.
- **Right-edge content widget** (§1.4) — deferred.

---

## Testing & gates

- Unit tests per new type/routine (Arrange-Act-Assert): `apply_bundle` sets each owned element
  and leaves un-owned ones untouched; `ZEN`/`FULL` bundle contents match the table;
  `TransientMode` reveal/hide transitions; the status reserved-row renders calm-when-idle and
  paints messages in place with no `edit_height` change; menu windowing (`scroll_top` keeps the
  highlight visible; tall category no longer truncates); each overlay's click-to-commit +
  click-away + no-leak; prompt-choice click dispatch; state-in-label rendering + the
  state-provider `fn` reads live `Editor` state.
- No-silent-UI regression test: a status message reveals the reserved row even in zen/`Auto`.
- Persistence round-trip tests for the new `scrollbar`/`status_line` keys (diff-law).
- GATEs: `cargo test` (core + shell) green, `cargo clippy --workspace --all-targets` clean,
  `cargo build`/`--no-run` warning-free for touched crates. Smoke suite mandatory-run.
- Pipeline gates: Codex spec review (loop clean) → plan → Codex plan review (loop clean) →
  subagent execution → Codex pre-merge + Fable whole-branch → merge.

---

## Open questions the PLAN must resolve (not design forks — implementation detail)

1. Exact `ChromeBundle` struct location and the precise `apply_bundle` override semantics
   (runtime-clobber vs config-sticky) expressed as code + tests.
2. `TransientMode` placement and whether the menu bar's `MenuBarMode` is mapped to it or kept
   parallel (the plan picks the least-churn wiring; `menu_bar_rows()` stays the geometry
   authority).
3. Scrollbar right-edge dwell timer + a `scrollbar_mode` runtime/config field (new — no scrollbar
   config exists today).
4. The `state: fn(&Editor) -> MenuMark)` signature + how `grouped_commands`/`MenuView::build`
   thread `&Editor`, and the exact label-composition for on/off vs radio-collapsed rows.
5. Per-overlay row-hit-test functions (analogous to `palette_row_at`) for theme picker / file
   browser / outline / diag, and the click-away rects.
6. The attached-filled dropdown render (no border) + `n/total` bottom row, and `menu_dropdown_rect`
   windowing math anchored under the label.
