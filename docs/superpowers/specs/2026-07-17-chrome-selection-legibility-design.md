# Chrome selection legibility — design note (spec)

**Date:** 2026-07-17
**Effort size:** S (small) — ~4 tasks.
**Anchors:** B7 (selected menu-dropdown row text too light on dark/phosphor themes) + A16
(drop the redundant `Transform…` umbrella row from the Format menu). Ride-along, folded in
per the coordinator.
**Severity:** cosmetic — no data-loss surface, no hot-path change.
**Grounding packet:** `scratchpad/chrome-sel/grounding.md` (live `snap -e` ANSI captures +
code-map census). This spec supersedes the packet's "N=1 menu-only" claim: the true count is
**two leaking paint sites**, both in the menu family (see §1).

---

## 1. Problem statement

### 1.1 Mechanism — a leaked `Modifier::DIM` on selected chrome rows

Two wordcartel chrome faces carry `dim: Some(true)`:

- **`ChromeMuted`** — the dropdown-normal item + scrollbar-track face. Derived in
  `Theme::derive_chrome` (`wordcartel-core/src/theme.rs`, the `chrome_muted` arm:
  `Face { fg: …, bg: …, dim: Some(true), .. }`); set explicitly in `terminal_ansi()`
  (`chrome_muted: Face { … dim: Some(true), .. }`) and in `mono_faces()`
  (`chrome_muted: Face { dim: Some(true), .. }`).
- **`Chrome`** — the bar/status base face. Carries `dim: Some(true)` from the E5 recede
  (pinned by `e5_chrome_bar_fg_recedes_and_dims` and `e5_non_rgb_chrome_carries_dim` in
  `theme.rs`).

`Face.dim` becomes `Modifier::DIM` in `compose::face_to_ratatui` (`wordcartel/src/compose.rs`,
the final `s = add(face.dim, Modifier::DIM, s)` line). DIM is an **add-modifier flag only** —
it never mutates the fg color; the terminal fades the composed fg toward the bg at paint time.

The two menu painters underlay a rect with a DIM-bearing style and then paint the
selected/active cell with `ChromeSelected` — a **clean fg+bg swap** (`chrome_selected` in
`derive_chrome`: `Face { fg: Some(base_bg), bg: Some(base_fg), .. }`; explicit Black-on-White
in `terminal_plain` / `terminal_ansi()` and the Ansi16 policy; reverse-only in `mono_faces()`).
In every colored source, `ChromeSelected` carries **no modifiers of its own**; the sole
exception is `mono_faces()` (Depth::None), where it carries a `REVERSED` modifier and no color
(see §1.4). Crucially, in **no** source does it carry a `DIM` modifier or a `sub_modifier` that
would clear one. In ratatui 0.30.2, `Cell::set_style`
(`ratatui-core-0.1.2/src/buffer/cell.rs`) applies a style by patch semantics:

```rust
self.modifier.insert(style.add_modifier);
self.modifier.remove(style.sub_modifier);
```

`compose(ChromeSelected)` has an empty `sub_modifier`, so the underlay's DIM is **not cleared**
— it rides on top of the swapped-in selected cell. The fg color swaps correctly; only DIM
leaks. (This corrects the original B7 triage guess of "a leftover dim fg color" — it is the
modifier, not the color.)

### 1.2 The two leaking sites (both menu-family)

Both live in `wordcartel/src/render_overlays.rs`:

1. **Menu dropdown selected row** — `paint_menu_dropdown`:
   `frame.buffer_mut().set_style(drop_rect, cs.menu_norm)` fills the whole dropdown rect with
   `menu_norm` = `compose(ChromeMuted)` (DIM), then the selected item is styled
   `.style(cs.menu_sel)` where `menu_sel` = `compose(ChromeSelected)`. DIM leaks onto the
   selected row. This is the live-captured, user-reported site.

2. **Menu bar open-category label** — `paint_menu_bar`:
   `frame.buffer_mut().set_style(bar_row, cs.menu_closed)` fills the bar row with
   `menu_closed` = `compose(Chrome)` (DIM), then the open category label is painted
   `Paragraph::new(text).style(cs.menu_open)` where `menu_open` = `compose(ChromeSelected)`.
   Same patch path, same missing `sub_modifier` → DIM leaks onto the open label. This site was
   **not** in the packet census (which covered only the seven list overlays) and was not
   live-captured; it is structurally certain from the same mechanism and is proven empirically
   by the first-failing buffer test in §4(b).

The three `ChromeStyles` fields derived from `ChromeSelected` — `menu_sel`, `menu_open`,
`overlay_selected` (all `compose(theme, depth, &[SE::ChromeSelected])` in `ChromeStyles::build`,
`wordcartel/src/render.rs`) — are the complete set of selection styles. The **six windowed
overlays** (palette, file browser, diagnostics, theme picker, outline, cursor-style picker) are
**clean**: each underlays with `cs.ov_fill` = `compose(ChromeOverlay)` (no dim) and highlights
with `overlay_selected`, so there is no DIM to leak. This was confirmed by reading all seven
painters in `render_overlays.rs` and by the clean live palette capture (packet Part A). No
production consumer of `menu_sel` / `menu_open` / `overlay_selected` exists outside
`render_overlays.rs`.

### 1.3 Theme-dependent visibility (why dark/phosphor break, light survives)

Each leak is present only where its underlay face actually carries DIM, and is *visibly harmful*
only where the selected face is dark-fg-on-light-bg:

- **Dropdown selected-row leak** — present where `ChromeMuted` carries `dim: Some(true)`: the
  derived RGB themes, the Ansi16 policy, `terminal-ansi`, and mono. It is **absent for
  `terminal-plain`**, whose `ChromeMuted` (`Face { fg: White, bg: DarkGray, .. }`) has no dim.
- **Menu-bar open-label leak** — present where `Chrome` carries `dim: Some(true)`: all of the
  above **and `terminal-plain`** (whose `Chrome` is `Face { fg: White, bg: Black, dim: Some(true), .. }`
  — the E5 recede applies even to the frameless baseline theme).

Where an underlay face carries no DIM, there is nothing to leak and the `.remove_modifier` strip
(§2) is a harmless no-op. From the live `snap -e` captures (packet Part A), for the sites that
do leak:

| theme | selected `Save` row | fg (RGB) | bg (RGB) | DIM leaked? | legible? |
|---|---|---|---|---|---|
| tokyo-night (dark) | swap → dark-on-light | 26,27,38 (near-black) | 192,202,245 (light) | yes | **NO** — DIM lightens the near-black fg toward the light bg → contrast collapse |
| phosphor-green | swap → dark-on-bright | 0,41,0 (dark green) | 0,255,0 (bright) | yes | **NO** — same collapse |
| solarized-light | swap → light-on-mid | 253,246,227 (cream) | 88,110,117 (mid) | yes | yes — DIM lightens an already-light fg, still above the darker bg → survives |

Reference: the tokyo-night Command Palette renders the *same* dark-on-light selected face on
the *same* theme but is legible, because its selection style (`overlay_selected`) is painted
over a non-DIM `ov_fill` — no DIM to leak. The palette is the correct-pattern reference.

### 1.4 Depth::None (mono) — fixed for free

At `Depth::None`, `face_to_ratatui` drops all color and keeps only modifiers
(`depth_none_suppresses_color_keeps_modifiers` in `compose.rs`). In `mono_faces()`,
`chrome_selected` is reverse-only while `chrome` / `chrome_muted` carry `dim: Some(true)`, so
today the mono selected cell is **REVERSED + leaked DIM** — a dim reverse-video row. Not
user-reported, but the same bug class; the §2 fix repairs it at the same seam (the strip
removes only DIM; REVERSED is untouched).

---

## 2. The fix — strip DIM at the `ChromeStyles` cache seam

In `ChromeStyles::build` (`wordcartel/src/render.rs`), append
`.remove_modifier(Modifier::DIM)` to the three `ChromeSelected`-derived fields as they are
composed:

- `overlay_selected: compose::compose(theme, depth, &[SE::ChromeSelected])` → `… .remove_modifier(Modifier::DIM)`
- `menu_open:        compose::compose(theme, depth, &[SE::ChromeSelected])` → `… .remove_modifier(Modifier::DIM)`
- `menu_sel:         compose::compose(theme, depth, &[SE::ChromeSelected])` → `… .remove_modifier(Modifier::DIM)`

(`Modifier` is already imported in `render.rs` — `use ratatui::style::{…, Modifier, …}` at the
top of the file — so no new import is needed.)

### 2.1 Why this is the right seam

- **Single source of truth.** `ChromeStyles` is the one style cache every overlay and menu
  painter pulls its selection style from; the three fields above are the complete selection set.
  Stripping DIM here fixes both current leak sites *and* immunizes any future overlay that
  underlays with a DIM face — one axis of change, in the module that already owns "turn a face
  into a paint-ready style for chrome."
- **Uniform across all theme sources.** `ChromeStyles::build` runs downstream of every way a
  theme is produced — RGB-derived (`derive_chrome`), explicit constructors
  (`terminal_ansi`, `terminal_plain`), the Ansi16 policy (`apply_ansi16_chrome_policy`), user
  `[theme.styles]` TOML overrides (`override_face`), and mono (`mono_faces`). One strip covers
  them all; we do not touch six face-constructor sites.
- **Minimal, no dispatch bulk.** Three method-chain edits in an existing builder (see §8).

### 2.2 The ratatui 0.30.2 fact, stated precisely

ratatui 0.30 **does** provide a subtract path: `Style::sub_modifier` exists, and
`Style::remove_modifier(m)` (`ratatui-core-0.1.2/src/style.rs`) adds `m` to `sub_modifier`
(`self.add_modifier = self.add_modifier.difference(modifier); self.sub_modifier = self.sub_modifier.union(modifier)`).
`Cell::set_style` honors `sub_modifier` via `self.modifier.remove(style.sub_modifier)` (§1.1).
So the fix needs **no new seam** — one call on an already-composed `RStyle`.

What lacks a subtract path is wordcartel's own `compose::face_to_ratatui`, which is **add-only**:
it emits `Modifier::DIM` only for `dim == Some(true)` and has **no path to emit a ratatui
`sub_modifier`** at all. So a face's `dim` field can never *subtract* a DIM that an underlay's
`set_style` already wrote into the cell — regardless of the face's value. (Note this gap is in
`compose`, not `override_face`: `Theme::override_face` (`theme.rs`) *does* honor `Some(false)` —
`if patch.dim.is_some() { f.dim = patch.dim; }` replaces the stored field — so a user
`[theme.styles]` `dim = false` clears the *face* field; it simply then composes to "no DIM
added," never to "DIM removed from an inherited cell.") Teaching `compose` to express modifier
**subtraction** — so a face can clear an inherited/underlaid modifier rather than only add its
own — is the deeper, more honest fix but touches core compose semantics; it is **deferred as
backlog item H25** and is explicitly NOT part of this effort.

---

## 3. Intended side effect (not a regression)

Themes where the leak currently *survives* legibly — solarized-light and any other
light-fg-on-darker-bg selected face — will also lose DIM on their selected chrome rows and open
menu label. Those rows become marginally more saturated/brighter. This is an intended, uniform
consequence of the strip, not a regression: the selected face was always meant to be a clean
swap, and DIM on it was never intended. The spec calls it out so a reviewer diffing light-theme
captures does not flag it.

---

## 4. Tests / durable guarantee

No contrast-floor guard (see §4.3). Two tests pin the fix:

**(a) `ChromeStyles::build` unit test — the seam invariant.** Across a theme sweep that includes
at least one RGB dark theme (e.g. `tokyo-night`), one phosphor theme, one light theme
(e.g. `solarized-light`), `terminal-ansi`, and the `no-color`/mono theme, assert for each of
`overlay_selected`, `menu_open`, `menu_sel`:
- `add_modifier` does **not** contain `Modifier::DIM`, and
- `sub_modifier` **does** contain `Modifier::DIM` (proving the strip is applied, not merely
  absent because the source face happened to lack DIM).

This lives alongside the existing `ChromeStyles` tests in `render.rs` (near
`transparent_suppresses_overlay_interior`).

**(b) Buffer-level render test — proves both leak sites, first-failing.** With a menu open and
the selection on a dropdown row, render to a `TestBackend` buffer and assert that the cells of
**both** the selected dropdown row **and** the open-category bar label carry no `Modifier::DIM`
in their resolved cell modifier set. This is the test that empirically demonstrates the bar-label
leak (§1.2 site 2); it MUST be written first and observed to FAIL on the unpatched tree, then
pass after the strip. It slots next to the existing menu-render tests in `render.rs` (the block
around the `menu_sel` / `menu_norm` fg assertions).

### 4.3 No contrast floor

`ChromeSelected` is the `base_fg`/`base_bg` swap — maximal theme-native contrast by
construction. The E5 `FG_FLOOR` problem (blended/derived fgs sinking below a legibility floor)
does not arise for a pure swap. A floor guard here would be scope creep; the durable guarantee
is the two tests above.

---

## 5. Fallbacks — no regression

- **Depth::None (mono):** colors are already dropped in `face_to_ratatui`
  (`depth_none_suppresses_color_keeps_modifiers`, `compose.rs`); the selected cell stays
  REVERSED (from `mono_faces().chrome_selected`), now minus the leaked DIM — strict improvement.
  The mono theme's modifier cues are pinned by `no_color_is_monochrome_with_modifier_cues` and
  `marked_block_mono_modifier_is_distinct` (`theme.rs`), unaffected (the strip touches only
  DIM on the three selection styles, not any content face).
- **terminal-plain / terminal-ansi explicit Black-on-White:** `chrome_selected` there is an
  explicit fg/bg pair with **no dim** to remove; `remove_modifier(DIM)` is a no-op on color and
  on a modifier that is absent. Pinned by `terminal_ansi_is_fully_colored_and_chrome_coherent`
  and `terminal_plain_name_and_faces` (`theme.rs`), `terminal_plain_status_carries_chrome_face`
  and `terminal_plain_prompt_status_reverse_bold` (`render.rs`).
- **Ansi16 policy:** `apply_ansi16_chrome_policy` sets `chrome_selected` to Black-on-White; no
  dim. Pinned by `ansi16_policy_replaces_derived_chrome` and `user_styles_override_ansi16_policy`
  (`theme_resolve.rs`).
- **Transparent canvas:** `overlay_selected` keeps its bg in transparent mode, pinned by
  `transparent_suppresses_overlay_interior` (`render.rs`, asserts
  `transp.overlay_selected.bg.is_some()`). The strip removes only a modifier, never a bg, so
  this invariant holds; the plan re-runs this test.

The plan re-runs all named tests after the strip.

---

## 6. A16 — drop the `Transform…` umbrella row from the Format menu

In `wordcartel/src/registry.rs`, the `transform` command is registered
`r.register("transform", "Transform…", Some(MenuCategory::Format), …)`. Its discrete variants
(`reflow`/`unwrap`/`ventilate` and the `_buffer` forms) are already registered under
`MenuCategory::Format`, so the `Transform…` umbrella row duplicates a door already present.

**Change:** set the menu tag to `None` — `r.register("transform", "Transform…", None, …)`. The
command itself stays registered and palette-reachable; only its menu placement is removed.

**Test to flip:** `a3b_placement_sweep_categories` (`registry.rs`) currently asserts
`meta("transform").menu == Some(MenuCategory::Format)` — change it to expect `None` with an A16
rationale in the assertion message (palette-only; the discrete variants carry the Format door).

**Tests NOT affected:**
- `transforms_are_registered_commands_in_format_category` covers only the six discrete
  variants (`reflow`, `unwrap`, `ventilate`, `reflow_buffer`, `unwrap_buffer`,
  `ventilate_buffer`) — `transform` is not in its list.
- `keymap_ctrl_t_is_transform` asserts Ctrl-T still resolves to `CommandId("transform")` and
  opens the chooser — behaviour and keybinding are unchanged by a menu-tag removal.

No other reference to the `Transform…` label exists outside `registry.rs` (the four `app.rs`
hits are `TransformDone`, an unrelated message type). The menu builds its groups from registry
metas with no hardcoded row counts, so removing the tag simply omits the row.

---

## 7. Command-surface contract conformance

Per `docs/design/command-surface-contract.md`:

- **B7 (the DIM strip): N/A — does not touch the command surface.** It changes only how an
  already-registered selection *style* is composed for painting. No command, option, palette
  row, menu entry, or keybinding hint is added, removed, or re-registered.
- **A16 (menu curation): conforms under law 4 (menu ⊆ palette).** The `transform` command stays
  registered and therefore palette-reachable; only its `menu` placement is dropped, so the menu
  **shrinks** while the palette is unchanged — `menu ⊆ palette` is preserved trivially. The
  contract's invariant gates are untouched: palette-completeness (the command is still in the
  registry, still enumerated by the palette), every-option-has-a-command (no option involved),
  and hint re-resolution (the Ctrl-T binding and its hint still resolve via the registry; only
  the Format menu no longer *displays* the row — the palette row still shows the chord).

---

## 8. Anti-regrowth / module budgets

This effort adds **no dispatch bulk**: three method-chain edits inside the existing
`ChromeStyles::build` builder (a data-table-style initializer, not a dispatcher) plus a
one-tag removal and two small test additions. No new `match` arm, no new loop iteration, no new
hub wiring. It does not push `render.rs` or `app.rs` toward the `module_budgets` production
budgets and introduces no function over the `too_many_lines` threshold. Fully within the house
*Module structure* rule.

---

## 9. Scope / rigor

Single **small (S)** effort, ~4 tasks:
1. Buffer-level first-failing test (§4b) proving both leak sites red on the unpatched tree.
2. The `ChromeStyles::build` DIM strip (§2) + the seam unit test (§4a); fallback tests re-run.
3. A16 tag removal + `a3b_placement_sweep_categories` flip (§6).
4. Green-up + workspace clippy + the named fallback tests.

Standard Codex spec/plan gates + a normal whole-branch pre-merge pass. No special Fable-probe
complexity: the mechanism is fully settled by reading (ratatui patch semantics + the two
painters + the face `dim` flags) plus the live captures; nothing rides on a claim that needs a
compiled math probe. Cosmetic severity, no data-loss surface — rigor calibrated accordingly.
