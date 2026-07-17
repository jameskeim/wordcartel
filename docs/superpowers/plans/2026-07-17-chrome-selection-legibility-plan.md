# Chrome selection legibility — implementation plan

**Spec:** `docs/superpowers/specs/2026-07-17-chrome-selection-legibility-design.md` (Codex-clean).
**Date:** 2026-07-17
**Effort size:** S — 4 tasks.

---

## Goal

Kill the leaked `Modifier::DIM` that washes out selected chrome rows (B7) by stripping DIM at the
one style-cache seam every overlay pulls selection from, and drop the redundant `Transform…`
umbrella row from the Format menu (A16). Cosmetic severity, no data-loss surface.

Concretely:
1. Append `.remove_modifier(Modifier::DIM)` to the three `ChromeSelected`-derived fields in
   `ChromeStyles::build` (`wordcartel/src/render.rs`): `menu_sel`, `menu_open`, `overlay_selected`.
2. Prove it with a first-failing buffer-level render test (both leak sites) + a `ChromeStyles::build`
   seam-invariant unit test.
3. Un-tag the `transform` command from `MenuCategory::Format` (keep it palette-reachable).

## Architecture

- **Functional core untouched.** No `wordcartel-core` change. `Face`/`compose` semantics are NOT
  modified — the deeper "teach compose modifier subtraction" fix is deferred to backlog H25. We
  work at the shell's ratatui-`Style` layer only, where `Style::remove_modifier(m)` records `m`
  into the style's `sub_modifier`, and ratatui 0.30.2 `Cell::set_style`
  (`ratatui-core-0.1.2/src/buffer/cell.rs`) honors it: `self.modifier.remove(style.sub_modifier)`.
- **One seam.** `ChromeStyles::build` (`wordcartel/src/render.rs`) is the single per-frame style
  cache; the three selection styles it computes (`menu_sel`, `menu_open`, `overlay_selected`, all
  `compose(theme, depth, &[SE::ChromeSelected])`) are the complete set of chrome selection styles.
  `menu_norm` (= `compose(ChromeMuted)`) is deliberately NOT touched — dropdown-normal rows keep
  their DIM recede.
- **No dispatch bulk** (house Module-structure rule): three method-chain edits in the existing
  builder initializer + a one-tag registry edit + tests. No new `match` arm, loop, or hub wiring;
  no function crosses the `too_many_lines` threshold; `render.rs`/`registry.rs` stay within
  `module_budgets`.

## Tech stack

Rust workspace: `wordcartel-core` (pure, `#![forbid(unsafe_code)]`) + `wordcartel` shell. ratatui
0.30.2, crossterm. Tests are in-crate `#[cfg(test)] mod tests { use super::*; }`. Buffer assertions
use `ratatui::backend::TestBackend` + `Terminal::draw`.

## Global constraints (binding — copied verbatim from the house rules)

- **GATE:** `cargo test` green across all suites (`wordcartel-core` lib + oracle, `wordcartel` lib).
- **GATE:** `cargo build` and `cargo test --no-run` warning-free for the crate(s) you touched.
- **GATE:** Workspace clippy clean — `cargo clippy --workspace --all-targets` MUST pass clean
  before merge. Deliberate exceptions need an item-local `#[allow(clippy::…)]` with a one-line
  rationale (never a blanket allow).
- **Each commit must be WORKSPACE-green, not just crate-green** (B17 cross-crate-greenness lesson):
  a fix and the test that proves it land in the SAME commit if a split would leave the workspace
  red at an intermediate commit.
- **Do NOT run `cargo fmt`.** This repo is hand-formatted in a deliberate dense style with no
  `rustfmt.toml`; match the neighbours by hand. Do not reflow code you did not otherwise change.
- **House dense style:** snake_case fns/vars, 4-space indent, ~100-char hand-wrapped lines, em-dash
  `—` in prose comments (never `--`), no emoji in code. Doc-comment new public items; unit tests
  Arrange-Act-Assert.
- **Anchor on symbol NAMES, not line numbers** — locate `ChromeStyles::build`,
  `a3b_placement_sweep_categories`, etc. by name; the `:NNN` references here are convenience only
  and drift as tasks edit files.
- **Commit trailers** (`Co-Authored-By` + `Claude-Session:`) are added by the controller at commit
  time — do NOT invent or construct a session URL in a task.
- **Verify with cargo, not editor hints:** for any compile/usage/signature question on code you are
  editing, trust `cargo build`/`cargo test`/`grep`, never a rust-analyzer "unused"/"undefined"
  snapshot.

## File structure (files touched)

| File | Change |
|---|---|
| `wordcartel/src/render.rs` | Task 1: `.remove_modifier(Modifier::DIM)` on 3 fields in `ChromeStyles::build`; new buffer test `menu_selection_and_open_label_have_no_leaked_dim`. Task 2: new unit test `chrome_selected_styles_strip_dim_via_sub_modifier`. |
| `wordcartel/src/registry.rs` | Task 3: `transform` menu tag `Some(MenuCategory::Format)` → `None`; flip assertion in `a3b_placement_sweep_categories`. |

No new files. No `wordcartel-core` change.

---

## Task 1 — buffer-level leak-proof test (first-failing) + the DIM strip

**Why one commit:** the strip lives in `render.rs` and the test that proves it lives in the same
crate's test module — but the TDD sequence (test red on current tree → strip → test green) must not
leave an intermediate red commit, so the failing test and the fix land together (Global Constraints:
workspace-green per commit).

### Step 1.1 — write the failing buffer test (RED)

The test opens a menu under a DERIVED RGB theme (tokyo-night) whose `ChromeMuted` AND `Chrome` both
carry `dim: Some(true)`, so BOTH leak sites are exercised. (The default `terminal-plain` theme would
NOT exercise the dropdown site — its `ChromeMuted` has no dim — so tokyo-night is required to make
the dropdown assertion first-failing; see spec §1.3.)

Add to `wordcartel/src/render.rs`, inside `#[cfg(test)] mod tests` (near the existing
`menu_open_suppresses_editor_caret` / the menu-render tests). Grounded in the real idioms of
`menu_open_suppresses_editor_caret` (menu build) and the menu-highlight geometry test that uses
`chrome_geom::menu_dropdown_rect` / `menu_bar_layout`:

```rust
/// B7 leak-proof: with a menu open under a DIM-bearing derived theme (tokyo-night —
/// both `ChromeMuted` and `Chrome` carry dim), neither the SELECTED dropdown row nor the
/// OPEN-category bar label may carry a leaked `Modifier::DIM`. This is the crux regression
/// for the chrome-selection-legibility fix and covers BOTH leak sites (spec §1.2):
///   - dropdown selected row  ← `menu_norm` (ChromeMuted/DIM) underlay + `menu_sel` swap
///   - open-category bar label ← `menu_closed` (Chrome/DIM) bar fill + `menu_open` swap
/// First-failing on the unpatched tree (DIM leaks through ratatui's OR-merge); passes once
/// `ChromeStyles::build` strips DIM via `sub_modifier`.
#[test]
fn menu_selection_and_open_label_have_no_leaked_dim() {
    use wordcartel_core::theme::{ChromeDisposition, Depth, Theme};
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    // Arrange: derived RGB theme with a DIM-bearing dropdown + bar, at truecolor.
    let reg = crate::registry::Registry::builtins();
    let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
    let mut ed = Editor::new_from_text("hello world\n", None, (80, 12));
    let mut theme = Theme::builtin("tokyo-night").unwrap();
    theme.derive_chrome(ChromeDisposition::Full);
    ed.theme = theme;
    ed.depth = Depth::Truecolor;
    derive::rebuild(&mut ed);
    // Open the first category's menu (build → open:0, highlighted:0, scroll_top:0). No
    // menu_bar_mode setup is needed: `menu_bar_rows()` returns `u16::from(bar || menu.is_some())`
    // (editor.rs), so an open menu forces the bar to 1 row regardless of the default Auto mode —
    // this is what makes the open-label (bar) assertion exercisable/first-failing.
    ed.menu = Some(crate::menu::build(&reg, &km, &ed));

    // Geometry, via the same helpers the painter uses (menu_area excludes the status row).
    let menu_area = crate::chrome_geom::menu_area(Rect::new(0, 0, 80, 12));
    let groups = ed.menu.as_ref().unwrap().groups.clone();
    let open = ed.menu.as_ref().unwrap().open;
    let bar = crate::chrome_geom::menu_bar_layout(menu_area, &groups);
    let (_, label_rect) = bar[open];
    let drop_rect = crate::chrome_geom::menu_dropdown_rect(menu_area, &groups, open)
        .expect("open category must produce a dropdown rect");

    // Act.
    let buf = render_to_buffer(&mut ed, 80, 12);

    // Assert — selected dropdown row (highlighted 0, scroll_top 0 → the top item row).
    let sel_y = drop_rect.y;
    let sel_x = drop_rect.x + 1; // first text cell inside the item (mirrors the highlight test)
    assert!(
        !buf[(sel_x, sel_y)].style().add_modifier.contains(Modifier::DIM),
        "selected dropdown row must not carry leaked DIM at ({sel_x},{sel_y}); \
         style={:?}", buf[(sel_x, sel_y)].style(),
    );

    // Assert — open-category bar label: no cell across its own columns may carry DIM.
    let label_has_dim = (label_rect.x..label_rect.x + label_rect.width)
        .any(|x| buf[(x, label_rect.y)].style().add_modifier.contains(Modifier::DIM));
    assert!(
        !label_has_dim,
        "open-category bar label (row {}) must not carry leaked DIM", label_rect.y,
    );
}
```

Run it and CONFIRM it FAILS on the current tree:

```sh
cargo test -p wordcartel --lib menu_selection_and_open_label_have_no_leaked_dim
```

Expected: red — the dropdown selected cell (and the open label) carry DIM because `menu_sel` /
`menu_open` have an empty `sub_modifier`, so ratatui's `Cell::set_style` never clears the underlay's
DIM. (If the dropdown assertion unexpectedly passes, STOP: the theme/derive setup is wrong — verify
`compose(theme, Truecolor, &[SE::ChromeMuted]).add_modifier.contains(Modifier::DIM)` is true.)

### Step 1.2 — apply the DIM strip (GREEN)

In `ChromeStyles::build` (`wordcartel/src/render.rs`), append `.remove_modifier(Modifier::DIM)` to
the three `ChromeSelected`-derived fields. `Modifier` is already imported at the top of `render.rs`
(`use ratatui::style::{Color, Modifier, Style as RStyle};`) — no new import.

Current:

```rust
        ChromeStyles {
            overlay_selected: compose::compose(theme, depth, &[SE::ChromeSelected]),
            ov_query,
            menu_open:        compose::compose(theme, depth, &[SE::ChromeSelected]),
            menu_closed:      compose::compose(theme, depth, &[SE::Chrome]),
            menu_sel:         compose::compose(theme, depth, &[SE::ChromeSelected]),
            menu_norm:        compose::compose(theme, depth, &[SE::ChromeMuted]),
```

Changed (only the three `ChromeSelected` lines gain the strip; `menu_closed`/`menu_norm` keep DIM):

```rust
        ChromeStyles {
            // B7: the selected/active chrome styles are painted OVER a DIM-bearing underlay
            // (`menu_norm`=ChromeMuted for the dropdown, `menu_closed`=Chrome for the bar).
            // ratatui's `Cell::set_style` OR-merges add_modifiers, so a bare ChromeSelected swap
            // leaves the underlay's DIM riding on the selected cell → washout on dark/phosphor.
            // Record DIM in each selection style's `sub_modifier` so set_style CLEARS it. The
            // deeper "teach compose modifier subtraction" fix is deferred to backlog H25.
            overlay_selected: compose::compose(theme, depth, &[SE::ChromeSelected]).remove_modifier(Modifier::DIM),
            ov_query,
            menu_open:        compose::compose(theme, depth, &[SE::ChromeSelected]).remove_modifier(Modifier::DIM),
            menu_closed:      compose::compose(theme, depth, &[SE::Chrome]),
            menu_sel:         compose::compose(theme, depth, &[SE::ChromeSelected]).remove_modifier(Modifier::DIM),
            menu_norm:        compose::compose(theme, depth, &[SE::ChromeMuted]),
```

(`Style::remove_modifier` takes `self` and returns `Self` — `pub const fn remove_modifier(mut self, modifier: Modifier) -> Self`, `ratatui-core-0.1.2/src/style.rs` — so it chains directly onto the `compose(...)` `RStyle`.)

Run it and CONFIRM it PASSES:

```sh
cargo test -p wordcartel --lib menu_selection_and_open_label_have_no_leaked_dim
```

### Step 1.3 — commit (workspace-green)

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

Both must be clean, then commit the strip + test together (one commit). Suggested message:
`fix(chrome): strip leaked DIM from selected chrome styles (B7)`.

---

## Task 2 — `ChromeStyles::build` seam-invariant unit test

A cheap, theme-swept guard that pins the strip at the seam directly (independent of render
geometry): each of the three selection styles must carry DIM in `sub_modifier` and NOT in
`add_modifier`, while `menu_norm` must STILL carry DIM (guard against over-stripping).

This is a **pinning/guard test**, added after Task 1's strip, so it is GREEN when written. To prove
it is meaningful (i.e. would catch a regression), the implementer confirms it by TEMPORARILY
deleting one `.remove_modifier(Modifier::DIM)` and observing the matching `sub_modifier` assertion
fail, then restoring — this is the RED→GREEN confirmation for a guard that cannot precede the fix
it guards.

### Step 2.1 — write the test

Add to `wordcartel/src/render.rs` tests (near the existing `ChromeStyles` tests, e.g.
`transparent_suppresses_overlay_interior`). `Style::add_modifier` and `Style::sub_modifier` are both
`pub` fields (`ratatui-core-0.1.2/src/style.rs`), read directly:

```rust
/// B7 seam invariant: `ChromeStyles::build` records DIM in the `sub_modifier` of every
/// ChromeSelected-derived selection style (so `Cell::set_style` clears an underlay's DIM) and
/// never leaves DIM in their `add_modifier`. `menu_norm` (dropdown-normal) MUST retain DIM in
/// `add_modifier` — the strip is scoped to selection, not the recede. Swept across a derived RGB
/// theme, terminal-ansi, and the no-color/mono theme (Depth::None).
#[test]
fn chrome_selected_styles_strip_dim_via_sub_modifier() {
    use wordcartel_core::theme::{ChromeDisposition, CanvasMode, Depth, Theme};
    use ratatui::style::Modifier;

    // (theme, depth) sweep: derived RGB (tokyo-night), explicit terminal-ansi, mono no-color.
    let mut tokyo = Theme::builtin("tokyo-night").unwrap();
    tokyo.derive_chrome(ChromeDisposition::Full);
    let cases = [
        (tokyo,                              Depth::Truecolor, "tokyo-night"),
        (Theme::builtin("terminal-ansi").unwrap(), Depth::Ansi16, "terminal-ansi"),
        (Theme::builtin("no-color").unwrap(),      Depth::None,   "no-color"),
    ];
    for (theme, depth, name) in cases {
        let cs = ChromeStyles::build(&theme, depth, CanvasMode::Opaque);
        for (label, style) in [
            ("overlay_selected", cs.overlay_selected),
            ("menu_open",        cs.menu_open),
            ("menu_sel",         cs.menu_sel),
        ] {
            assert!(style.sub_modifier.contains(Modifier::DIM),
                "{name}/{label}: selection style must record DIM in sub_modifier (strip applied)");
            assert!(!style.add_modifier.contains(Modifier::DIM),
                "{name}/{label}: selection style must not carry DIM in add_modifier");
        }
        // Guard: the strip must NOT touch the dropdown-normal recede.
        assert!(cs.menu_norm.add_modifier.contains(Modifier::DIM),
            "{name}: menu_norm (ChromeMuted) must keep its DIM recede — strip is selection-only");
    }
}
```

Note the `menu_norm` guard: `ChromeMuted` carries `dim: Some(true)` for all three swept themes
(tokyo derived, terminal-ansi explicit, no-color mono), so this assertion is valid for the whole
sweep. (The meaningfulness confirmation — temporarily deleting one `.remove_modifier` — is described
above.)

### Step 2.2 — run + commit

```sh
cargo test -p wordcartel --lib chrome_selected_styles_strip_dim_via_sub_modifier
cargo test --workspace
cargo clippy --workspace --all-targets
```

Commit. Suggested message: `test(chrome): pin the ChromeStyles DIM-strip seam invariant (B7)`.

---

## Task 3 — A16: drop the `Transform…` Format-menu row

Menu curation only: remove the `transform` command's menu tag; the command stays registered and
palette-reachable. Command-surface law 4 (menu ⊆ palette) preserved — menu shrinks, palette
unchanged. Ctrl-T is independent (`input.rs` `KeyCode::Char('t') if ctrl => id("transform")`;
`keymap.rs` `("ctrl-t", "transform")`) and is NOT touched.

### Step 3.1 — flip the assertion first (RED)

In `wordcartel/src/registry.rs`, the ONLY test that pins `transform`'s menu placement is
`a3b_placement_sweep_categories` (verified: the sole `meta("transform").menu` assertion in the
crate; `transforms_are_registered_commands_in_format_category` covers only the six discrete
variants — `reflow`/`unwrap`/`ventilate` + `_buffer` forms — and is NOT touched).

Current:

```rust
        assert_eq!(meta("transform").menu, Some(MenuCategory::Format),
            "transform's discrete variants are all Format; View was a historical accident");
```

Change to:

```rust
        assert_eq!(meta("transform").menu, None,
            "A16: the Transform… umbrella row is dropped from the Format menu — its discrete \
             variants (reflow/unwrap/ventilate) already carry the Format door; the command stays \
             registered and palette-reachable");
```

Run and CONFIRM it FAILS (the registration still tags Format):

```sh
cargo test -p wordcartel --lib a3b_placement_sweep_categories
```

### Step 3.2 — drop the menu tag (GREEN)

In `Registry::builtins` (`wordcartel/src/registry.rs`), the `transform` registration:

Current:

```rust
        // transform: Format menu (A3b) — its discrete variants are all Format; View was
        // a historical accident.
        r.register("transform", "Transform…", Some(MenuCategory::Format), |c| {
            c.editor.open_prompt(crate::prompt::Prompt::transform_chooser());
            CommandResult::Handled
        });
```

Change to (third arg `None`; command body unchanged):

```rust
        // transform: palette-only (A16) — the discrete variants (reflow/unwrap/ventilate) already
        // carry the Format door, so the Transform… umbrella row is redundant. Command stays
        // registered + palette-reachable; Ctrl-T (input.rs/keymap.rs) is unaffected.
        r.register("transform", "Transform…", None, |c| {
            c.editor.open_prompt(crate::prompt::Prompt::transform_chooser());
            CommandResult::Handled
        });
```

Run and CONFIRM the flipped test PASSES, and the untouched sibling still passes:

```sh
cargo test -p wordcartel --lib a3b_placement_sweep_categories
cargo test -p wordcartel --lib transforms_are_registered_commands_in_format_category
cargo test -p wordcartel --lib keymap_ctrl_t_is_transform
```

### Step 3.3 — commit

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

Commit. Suggested message: `chore(menu): drop redundant Transform… umbrella row from Format (A16)`.

---

## Task 4 — no-regression re-run + full sweep (final gate)

No code change. Re-run the exact existing fallback/pin tests the spec names, plus the full workspace
gates and the smoke suite. If any is red, STOP and surface it (do not paper over).

### Step 4.1 — named fallback/pin tests green

```sh
cargo test -p wordcartel-core --lib \
  depth_none_suppresses_color_keeps_modifiers \
  terminal_ansi_is_fully_colored_and_chrome_coherent \
  terminal_plain_name_and_faces \
  no_color_is_monochrome_with_modifier_cues

cargo test -p wordcartel --lib \
  depth_none_suppresses_color_keeps_modifiers \
  terminal_plain_status_carries_chrome_face \
  terminal_plain_prompt_status_reverse_bold \
  ansi16_policy_replaces_derived_chrome \
  user_styles_override_ansi16_policy \
  transparent_suppresses_overlay_interior
```

(`depth_none_suppresses_color_keeps_modifiers` lives in `compose.rs`, compiled into the `wordcartel`
lib; the `terminal_ansi_*` / `no_color_*` / `terminal_plain_name_and_faces` pins live in
`wordcartel-core/src/theme.rs`; the rest in `wordcartel/src/{render.rs,theme_resolve.rs}`. All were
verified present by name.)

### Step 4.2 — full workspace gates (merge GATEs)

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build
cargo test --no-run
```

All must be clean/warning-free for the touched crates.

### Step 4.3 — smoke suite (mandatory-run, advisory-pass)

```sh
scripts/smoke/run.sh
```

Quote its one-line summary verbatim in the pre-merge report (e.g. `smoke: 8/8 PASS`, or
`smoke: SKIP — no tmux`, or a red result as `smoke: FAIL sN — advisory`). A red smoke result never
blocks merge but MUST be surfaced explicitly to the human.

No commit for Task 4 (verification only), unless a test needed a green-up fix — in which case that
fix is its own workspace-green commit.

---

## Spec-coverage checklist

| Spec section | Covered by |
|---|---|
| §2 the fix (3 `.remove_modifier` appends) | Task 1 Step 1.2 |
| §1.2 both leak sites (dropdown row + bar label) | Task 1 buffer test (both assertions) |
| §4(a) seam invariant unit test | Task 2 |
| §4(b) first-failing buffer test | Task 1 Step 1.1 (RED confirmed before strip) |
| §4.3 no contrast floor | Not added (by design) |
| §3 intended light-theme side effect | No test — documented behavior only |
| §5 fallbacks (Depth::None, terminal-plain/ansi, transparent) | Task 4 Step 4.1 (named tests) |
| §6 A16 tag removal + `a3b_placement_sweep_categories` flip | Task 3 |
| §6 sibling test NOT touched | Task 3 (explicit; re-run confirms) |
| §7 command-surface conformance | Task 3 (menu ⊆ palette preserved; command stays registered) |
| §8 anti-regrowth / budgets | No dispatch bulk (3 chain edits + 1 tag); no new function |

## Command-surface contract conformance

- **B7 (Tasks 1–2): N/A** — changes only how an already-registered selection *style* composes for
  painting; no command, option, palette row, menu entry, or hint added/removed/re-registered.
- **A16 (Task 3): conforms under law 4** — `transform` stays registered (palette-reachable); only
  its `menu` placement drops, so the menu shrinks while the palette is unchanged (menu ⊆ palette
  preserved). Palette-completeness, every-option-has-a-command, and hint-re-resolution gates are
  untouched; the Ctrl-T binding/hint still resolves via the registry.
