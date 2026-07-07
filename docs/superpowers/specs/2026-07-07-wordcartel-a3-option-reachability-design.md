# A3 — Option Reachability + Preset-Aware Hints — Design

**Status:** design; Codex spec gate round 1 folded (3 Important + 2 Minor; incl. a contract shape-rule-8 refinement), re-review pending.
**Effort:** `effort-a3-option-reachability` (branch off `main`).
**Fixes backlog item:** A3.

## Command-surface-contract conformance (required opening — `docs/design/command-surface-contract.md`)

This effort **implements and enforces** the contract; conformance is its whole point:
- **Law 2 (every user-settable option is a command):** FIXES the two current violations — `status_line`
  and `scrollbar` modes are persisted settings with no command. Adds them, and a recurrence-guard test.
- **Law 6 (one shared setter per option; profiles call it):** refactors `density::apply_bundle` and the
  new commands to share one setter per option.
- **Law 7 (hints track the active keymap; prefer the user's explicit binding):** changes `chord_for` to
  prefer a config-patch-sourced binding over an inherited default; adds the hint-verification tests.
- **Law 3 (palette exhaustive):** formalizes the palette-completeness invariant test.
- **Shape rule 8 (set-value primitives + a stateful menu representative):** the new commands follow
  it — set-per-state primitives (palette-only) + a stateful menu representative (a toggle for the
  2-state `status_line`, a cycle for the 3-state `scrollbar`); `menu_bar` keeps its shipped
  `menu_bar_pin` toggle as its representative (§1.2), which rule 8 explicitly permits (a toggle or
  cycle; the menu representative need not expose every state — the palette does).

**Crate scope:** A3 is **entirely in `wordcartel/src`** (the shell) — `registry.rs`, `settings.rs`,
`density.rs`, `keymap.rs`, `menu.rs`, `palette.rs`. `wordcartel-core` is untouched. (The GATE runs
`cargo test -p wordcartel-core -p wordcartel` for the whole suite; core just doesn't change.)
- **Rule 10 (nullary now, parameterized deferred to P):** honored — no argument model introduced.
- **Out of scope (stated):** the menu-vs-palette *placement* judgment sweep (A3b); parameterized
  commands (Effort P).

**Goal.** Make every user-settable chrome/view option reachable as an individual command (so it lands
in the palette and is plugin-controllable), route profile and command through one shared setter per
option, and lock the keybinding-hint plumbing with a corrected display policy + tests.

---

## The gap (grounded)

`SettingsSnapshot` (`settings.rs:33-50`) enumerates every persisted setting. Cross-checked against the
registry, **all have a command except two:**

| Persisted setting | Command today |
|---|---|
| keymap_preset | `keymap_next` / `keymap_cua` / `keymap_wordstar` |
| theme_identity | `theme` (command id; opens the picker surface) |
| view_typewriter / focus / measure / wrap_guide / word_count | `toggle_*` |
| view_wrap_column | `set_wrap_column` |
| mouse_capture | `toggle_mouse_capture` |
| chrome_disposition | `toggle_chrome` |
| canvas | `toggle_canvas` |
| **view_scrollbar** | **NONE — reachable only via `toggle_chrome` (the ZEN/FULL profile) or config** |
| **view_status_line** | **NONE — same** |
| menu_bar | `menu_bar_pin` (a 2-way Pinned⇄remembered toggle — reachable, but not a deterministic 3-way) |

So `scrollbar`/`status_line` are the true law-2 violations; `menu_bar` is a consistency gap (has a
command, but no deterministic set-to-a-specific-state).

---

## Part 1 — Option-reachability commands (keymap-pattern) + shared setters

### 1.1 Shared setters (law 6)

Introduce one setter per multi-state option, each encapsulating the field write + any transition
hygiene, called by BOTH the new commands AND `density::apply_bundle` (which today sets the fields
directly, `density.rs:62-68`, and duplicates the menu-bar dwell-clear that `menu_bar_pin` also does):

- `set_scrollbar_mode(&mut Editor, TransientMode)` — sets `editor.scrollbar_mode`; clears the
  scrollbar dwell state (mirror the reset in `apply_bundle`).
- `set_status_line_mode(&mut Editor, TransientMode)` — sets `editor.status_line_mode`; clears status
  dwell state. (`Off` is coerced to `Auto` — status has no true Off, per E1.)
- `set_menu_bar_mode(&mut Editor, MenuBarMode)` — sets `editor.menu_bar_mode`; clears the menu dwell
  state (the hygiene currently inline in `menu_bar_pin` at `registry.rs:471-474` and in
  `apply_bundle`); **AND keeps `editor.menu_bar_unpinned_mode` consistent** (Codex spec gate — the
  "remembered mode" `menu_bar_pin` restores on unpin, `editor.rs:405`). Policy: setting to a
  non-Pinned mode makes THAT the remembered mode (`unpinned_mode = mode`); setting to `Pinned`
  remembers the current non-Pinned mode (only if not already Pinned) — i.e. `set_menu_bar_mode`
  generalizes `menu_bar_pin`'s current remember/restore logic so a deterministic `menu_bar_hidden/
  auto/pinned` (or `apply_bundle`) can't leave `menu_bar_pin` restoring a stale mode. (This also fixes
  a latent case today: `apply_bundle` sets `menu_bar_mode` directly without touching `unpinned_mode`.)

Placement: a small `settings_ops` module (or `editor.rs` methods) — the plan picks the least-churn
home. After this, `apply_bundle` sets the preset's owned modes THROUGH these setters (no bypass), so
profile and command can't drift. `menu_bar_pin` reduces to `set_menu_bar_mode(editor, if pinned {
unpinned_mode } else { Pinned })`.

### 1.2 The commands (shape rule 8: set-per-state primitives + a stateful menu representative)

Per the contract, each multi-state option gets deterministic **set-per-state** commands (`menu:
None` → palette-only) plus **one menu representative** (a cycle or the existing stateful toggle, with
state-in-label from E2). Nullary (rule 10).

- **Scrollbar** (3-state Off/Auto/On): `scrollbar_off`/`scrollbar_auto`/`scrollbar_on` (palette-only) +
  a **cycle** `cycle_scrollbar` ("Scrollbar: Auto", View menu, state-in-label; rotates Off→Auto→On→Off)
  — the 3-state representative.
- **Status line** (2-state Auto/On): `status_line_auto`/`status_line_on` (palette-only) + a **toggle**
  `toggle_status_line` ("Status Line: On", View menu, state-in-label; toggles Auto⇄On) — a 2-state
  toggle, consistent with `toggle_chrome`/`toggle_canvas`. No `status_line_off` (no true Off — E1).
- **Menu bar** (3-state Hidden/Auto/Pinned): `menu_bar_hidden`/`menu_bar_auto`/`menu_bar_pinned`
  (palette-only, the deterministic 3-way). **`menu_bar_pin` remains the menu representative** — a
  shipped, familiar, stateful pin toggle; NO second menu row and NO `cycle_menu_bar`. This is
  rule-8-compliant (the menu representative may be a toggle, and need not expose every state — the
  three states are all directly reachable in the palette via the explicit sets). *(Decided — not a
  plan fork.)*

All new palette-only commands appear in the palette automatically (law 3) with blank hints unless
bound (like `ventilate` today) — correct; they're keystroke-optional.

---

## Part 2 — Hint display policy + provenance (law 7)

Today `chord_for` (`keymap.rs:180-185`) returns the **shortest-then-alphabetical** chord among all
bindings for a command. When a command has a user-added binding *beside* its preset default, the
displayed hint may be the default (or an arbitrary one), not the user's.

**Fix:** `chord_for` prefers a **config-patch-sourced (user-explicit) binding**, falling back to
shortest-then-alphabetical among all when the command has none.

`KeyTrie` (`keymap.rs:162-164`) currently has only `map: HashMap<Vec<KeyChord>, CommandId>` — no
provenance. Add provenance so `chord_for` can tell user-explicit from preset-default:
- Add `user_bound: HashSet<Vec<KeyChord>>` to `KeyTrie` (or a per-entry source). Populate it in
  `apply_patch_tables` (`keymap.rs:487`) — every patch `bind` marks its seq user-bound; every patch
  `unbind` removes it. The preset base load (`build_keymap` step 1) does NOT mark user-bound.
- `chord_for(id)`: gather all seqs mapping to `id`; if any are in `user_bound`, choose among THOSE
  (shortest-then-alphabetical), else among all (current behavior).

This is preset-aware for free: scoped `[keymap.cua]`/`[keymap.wordstar]` patches only apply for the
active preset, so switching preset changes which user-explicit binds exist.

---

## Part 3 — The three enforcing tests (the laws as regression nets)

1. **Recurrence guard (law 2):** a test enumerating every `SettingsSnapshot` field and the command /
   command-surface that changes it, asserting each exists in `Registry::builtins()`. A hand-maintained
   mapping (Rust has no field reflection), with a doc-comment on `SettingsSnapshot` pointing to it
   ("every field here must have a command — see the reachability test") so adding a persisted setting
   forces updating the guard.
2. **Palette-completeness (law 3):** formalize `palette.rs:138` into a named invariant test — empty
   query → the palette rows contain EVERY registry command (`reg.commands()`), including `palette`
   itself (the palette does not special-case the sentinel today; only the menu does, `menu.rs:38`).
   There is no "internal command" concept currently; A3 does NOT introduce one, so the invariant is
   simply "all `reg.commands()` appear in the palette."
3. **Hints (law 7):** (a) build a keymap for CUA, then WordStar; assert `menu`/`palette` chords for a
   command differ per preset (re-resolution). (b) apply a config patch binding a command to a custom
   chord; assert `chord_for` returns the custom chord (user-explicit preferred) and that it surfaces
   in both `grouped_commands` (menu) and `rebuild_rows` (palette).

---

## Out of scope (explicit)

- **A3b** — the item-by-item menu-vs-palette placement sweep across all commands. This effort adds the
  *missing* commands and fixes the hint policy; it does not re-curate the whole menu.
- **Parameterized commands** (`set_scrollbar("off")` as one command with an arg) — Effort P.
- **A `cycle_menu_bar`** distinct from `menu_bar_pin` — decided against; `menu_bar_pin` is menu_bar's
  menu representative (§1.2), the explicit sets give the deterministic 3-way in the palette.

---

## Real-code anchors (for reviewer cross-check)

- Persisted settings: `SettingsSnapshot` `settings.rs:33-50`.
- Existing option commands: `toggle_*` `registry.rs:400-410`, `menu_bar_pin` `registry.rs:459-478`,
  `toggle_mouse_capture` `registry.rs:329`, `toggle_chrome`/`toggle_canvas` `registry.rs:514-525`,
  `set_wrap_column` `registry.rs:507`, keymap commands `registry.rs:492-506`.
- Command registration: `register`/`register_stateful` `registry.rs:64-83`; `MenuMark`/state-in-label
  `registry.rs:44-55`.
- Profile setter (to refactor): `density::apply_bundle` `density.rs:61-79`.
- Runtime fields: `editor.scrollbar_mode`/`status_line_mode`/`menu_bar_mode` (editor.rs); dwell state
  in `MouseState`.
- Keymap: `KeyTrie` `keymap.rs:162-164`, `bind` `:168`, `chord_for` `:180-185`, `build_keymap` `:441`,
  `apply_patch_tables` `:487`. Hint consumers: `menu::grouped_commands` (`chord_for` → `leaf_label`),
  `palette::rebuild_rows` (`chord_for` → `row.chord`).
- Palette-completeness near-miss: `palette.rs:138`.

---

## Testing & gates

- Unit tests per new setter/command (set-per-state sets the field + hygiene; cycle rotates; the
  shared setter is called by both command and `apply_bundle`). The three enforcing tests above.
- **No regression:** `toggle_chrome`/`apply_bundle` still apply the whole bundle (now via the shared
  setters); the density tests stay green.
- GATEs: `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets`
  clean; build/`--no-run` warning-free; smoke mandatory-run.
- Pipeline gates: Codex spec review (loop clean) → plan → Codex plan review (loop clean) → subagent
  execution → Codex pre-merge + Fable whole-branch → merge.

## Open questions the PLAN must resolve (implementation detail)

1. Home for the shared setters (`settings_ops` module vs `Editor` methods) and their exact hygiene.
2. The `KeyTrie` provenance representation (a `HashSet<seq>` vs a per-entry `Source` enum) + keeping it
   consistent across patch `unbind`.
3. The recurrence-guard's exact form (the field→command mapping table) and the `SettingsSnapshot`
   doc-comment.
4. Whether the scrollbar/status setters should also touch `scrollbar_visible`/`scrollbar_until_ms`
   (Codex noted current code relies on `recompute_scrollbar_visible` — likely leave to recompute; the
   plan confirms). (The `cycle_menu_bar`-vs-`menu_bar_pin` and `cycle_scrollbar` order questions are
   DECIDED in §1.2 — menu_bar_pin stays; scrollbar cycles Off→Auto→On.)
