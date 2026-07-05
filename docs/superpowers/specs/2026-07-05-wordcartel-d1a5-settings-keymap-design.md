# D1+A5 — settings write-back + runtime keymap switching (design)

Status: DRAFT (user-approved design 2026-07-05; forks resolved one at a time)
Effort: D1 (save settings from the session) + A5 (switch keybind system from the menu),
one effort per the backlog working order. Also closes C4's deferred close-buffer binding.

## Goals

1. **A5:** switch the keymap preset (cua ⇄ wordstar) mid-session from the menu/palette;
   hand-written patches survive the switch; hints stay fresh.
2. **D1:** an explicit "Save Settings" command persists the session's runtime settings as
   a machine-owned overrides file layered into the existing config chain. Hand-written
   files are never touched.
3. **Preset-scoped patches:** `[keymap.cua]` / `[keymap.wordstar]` sub-tables so a rebind
   meant for one keymap cannot silently shadow another's core binding (user-identified
   hazard, sharpened by runtime switching itself).
4. **C4 closure:** `close_buffer` stays unbound by design; per-preset patches are the
   supported binding path.

## Non-goals

- No settings modal/panel — deferred to the E arc (E2 radio marks + E3 chrome coherence);
  the commands shipped here are the substrate such a panel would dispatch (user-ratified).
- No auto-persistence — settings hit disk only on explicit `save_settings` (user-ratified).
- No bind-editing UI; no separate keymap file family (an `include` mechanism is the
  recorded escape hatch if patch sets ever outgrow the config file).
- No E2 checkable/radio menu items (the `active_keymap_preset` field this effort adds is
  their hook).
- `session.toml` (state store) is untouched: settings = user intent → overrides file;
  state = machine bookkeeping (cursor/marks/folds/scratch) → session store, as today.

## Grounded facts (verified against HEAD 2551463)

- `Config` (config.rs) is consumed at startup: `run()` seeds parallel `Editor` fields
  (app.rs:1244-1261 — `view_opts`, `diag_cfg`, `export_cfg`, `menu_bar_mode`,
  `mouse_capture`; theme via `theme_resolve::resolve_theme` → `editor.theme`). Only
  `cfg.state.max_entries` is read later (persist_session, app.rs:1608). NO config-writing
  code exists anywhere; `state.rs:84-94` writes session.toml via `toml::to_string` +
  `file::save_atomic_bytes` (→ `fsx::atomic_replace`, the M3 `Fs` seam). Deps: `toml 0.8`
  (read+write), serde derive. No `toml_edit`.
- Layer chain (`config_layer_paths`, config.rs:254-286), lowest→highest: XDG
  `config_dir()/wordcartel/config.toml` → nearest `.wordcartel.toml` walking up →
  `--config` path. `--no-config` skips all.
- `KeymapConfig { preset: String ("cua"), patches: Vec<KeymapPatch> }`;
  `KeymapPatch { bind: BTreeMap<String,String>, unbind: Vec<String> }` (config.rs:133-147);
  each layer contributes ONE patch (config.rs:314-317); TOML shape:
  `[keymap] preset='cua' bind={ "ctrl-a"='move_line_start' } unbind=[...]`.
- `build_keymap(&KeymapConfig, &Registry) -> (KeyTrie, Vec<String>)` (keymap.rs:425-488):
  preset base (unknown → warn + cua), then patches in order (binds before unbinds per
  patch). `KeyTrie` = flat `HashMap<Vec<KeyChord>, CommandId>`; `chord_for(id)` reverse
  lookup (shortest wins). CUA `ctrl-w → expand_selection` (keymap.rs:284); WordStar
  `ctrl-w → scroll_line_up` (keymap.rs:337). `close_buffer` (registry.rs:282,
  MenuCategory::File) is unbound in both presets.
- The trie is a LOOP-LOCAL: `run()` builds it, seeds `editor.keymap`, then
  `std::mem::take`s it out to avoid a `&mut editor`/`&editor.keymap` conflict
  (app.rs:1325-1334, comment: "doesn't change during the loop in v1"). `reduce` borrows it
  (`keymap: &KeyTrie`, app.rs:209-218). Rebuild-between-reduces is therefore conflict-free.
- Hints: palette recomputes `keymap.chord_for(id)` on every `rebuild_rows` (palette.rs:79);
  menu bakes chords into labels but rebuilds on every open (menu.rs:37-44,
  `hydrate_overlays`). No caching survives a rebuild.
- Runtime-mutable settings today (all mutate `Editor` fields; the startup `cfg` is never
  touched): theme (`apply_theme`, editor.rs:736; picker rows are builtin NAMES,
  theme_picker.rs:19-27; `Theme.name: String` — theme.rs:117 — so `editor.theme.name` is
  always the live name), five view toggles (registry.rs:385-389 →
  `editor.view_opts: ViewConfig`), `menu_bar_pin` (registry.rs:438 →
  `editor.menu_bar_mode` + `menu_bar_unpinned_mode`), `toggle_mouse_capture`
  (registry.rs:314 → `editor.mouse_capture`).
- Menu: `MENU_ORDER = [File, Edit, Format, View, Export]` (registry.rs:41-42); labels in
  `menu::category_label` (menu.rs:60-68); `CommandMeta` has no checked/radio state.
- e2e Harness builds a default-CUA trie (e2e.rs:36-37); tests use `cua_keymap()`
  (app.rs:1676-1679).

## D1. Preset-scoped patches (config.rs + keymap.rs)

Schema, per layer — today's `[keymap]` keys keep their exact meaning (GLOBAL: applies
under every base), plus two optional sub-tables:

```toml
[keymap]
preset = "wordstar"                       # unchanged
bind = { "ctrl-g" = 'goto_line' }         # unchanged — global, all presets
unbind = ["ctrl-q"]                       # unchanged — global
[keymap.cua]                              # NEW — applies only when base == cua
bind = { "ctrl-w" = 'close_buffer' }
unbind = []
[keymap.wordstar]                         # NEW — applies only when base == wordstar
bind = { "ctrl-k ctrl-o" = 'close_buffer' }
```

- `KeymapPatch` gains `scoped: BTreeMap<String, ScopedPatch>` (or equivalent — plan pins
  the exact shape against RawConfig's serde derive), where `ScopedPatch` is the same
  `bind`/`unbind` pair. Each layer still contributes exactly one `KeymapPatch`.
- **Merge law — "later file wins; within a file, specific wins":** `build_keymap` applies,
  for each layer lowest→highest: the layer's GLOBAL bind/unbind, then the layer's
  scoped table for the ACTIVE preset only. (Consequence, documented: a scoped bind in an
  earlier layer can be beaten by a global bind in a later layer — layer precedence stays
  the single outer law.)
- A scoped table naming an unknown preset (not `cua`/`wordstar`) → one warning through the
  existing warnings vec, table skipped. Same treatment as unknown chords/commands today.
- Back-compat: configs with only global keys behave byte-identically (empty scoped maps).

`build_keymap`'s signature grows the active-preset input only if needed — it already
receives `&KeymapConfig` whose `preset` field IS the active base; scoped application keys
off that same field. Signature unchanged.

## D2. Runtime keymap switch (A5)

- Two commands (registry, `MenuCategory::Settings` — see D5): `keymap_cua` ("Keymap: CUA"),
  `keymap_wordstar` ("Keymap: WordStar"). Idempotent: switching to the already-active
  preset is a no-op with a status message ("keymap: cua (already active)" or similar —
  plan pins copy).
- `Editor` gains:
  - `active_keymap_preset: String` — seeded in `run()` from the RESOLVED preset (after
    unknown-preset fallback, i.e. what `build_keymap` actually used; the fallback
    resolution becomes a small shared helper so run() and build_keymap can't disagree).
    Read by: the switch commands (idempotence + diff), `save_settings` (D3), E2 later.
  - `keymap_rebuild: bool` — the rebuild request flag.
- Dispatch sets `active_keymap_preset` + `keymap_rebuild = true` + a status message
  ("keymap: wordstar"). The RUN LOOP, after `reduce` returns and before the next input
  wait, checks the flag: rebuilds via `build_keymap(&KeymapConfig { preset:
  editor.active_keymap_preset.clone(), patches: <the startup layer patches> }, &reg)`,
  reassigns the loop-local trie, clears the flag, surfaces any (unlikely) new warnings to
  the status line. The startup `cfg.keymap.patches` remain in scope in `run()` for exactly
  this (as `cfg` already stays alive for `max_entries`).
- Patches survive the switch by construction (same patch chain, new base; scoped tables
  re-key off the new preset). Pinned by test.
- Hints: no new machinery — palette `rebuild_rows` and menu-open rebuilds pick up the new
  trie (grounded above). Dispatch closes the issuing overlay, so no stale-open-overlay
  case exists; the plan verifies the menu path (menu dispatch → menu closes → next open
  rebuilds).
- The e2e Harness gains the same flag-check in its `step`/`advance` path so the journey
  can pin the rebuild (plan grounds the exact seam; the harness owns its trie the same way
  run() does).

## D3. The overrides file (D1)

- Path: `<dirs::config_dir()>/wordcartel/settings-overrides.toml`. Machine-owned: header
  comment ("managed by wcartel — edits may be overwritten by Save Settings"), rewritten
  WHOLESALE on every save.
- **Layering:** inserted into `config_layer_paths` ABOVE the hand-written chain (XDG
  config, `.wordcartel.toml`) and BELOW `--config`. `--no-config` skips it like everything
  else. It contributes a layer like any other (including, in principle, a keymap patch —
  but save_settings never writes one; only hand edits could put one there, and the file
  header discourages hand edits).
- **Baseline:** `load()` additionally returns (or exposes) the merged config WITHOUT the
  overrides layer — the baseline. Implementation shape: build the merge in two stages
  (hand chain + `--config` staged around the overrides layer — plan pins the exact
  two-pass or snapshot mechanics against load()'s accumulator loop). `run()` keeps the
  baseline's settings-relevant values (a small `SettingsSnapshot` struct) alive alongside
  `cfg`.
- **`save_settings`** (command "Save Settings", `MenuCategory::Settings`): collects the
  CURRENT runtime values (D4 inventory) from the Editor, diffs against the baseline
  snapshot, serializes ONLY the differing values via `toml::to_string` of a serde struct
  mirroring the config sections, writes via `file::save_atomic_bytes` (the M3 seam —
  mode-preserving atomic replace, dir fsync). Status line: "settings saved" /
  "settings: <SaveError>" on failure — no silent UI, like every save.
- Consequences (all pinned): idempotent (save twice → identical file); self-healing
  (delete the file → next session = hand config exactly); minimal (un-diverged values
  never appear, so later hand-config edits to those values shine through); un-divergence
  removes the key (change a toggle back to the baseline value, save → key absent).
- Empty diff → the file is written with the header ONLY (never deleted — one write path,
  deterministic; an all-comments TOML parses to an empty layer, a no-op). Pinned by test.

## D4. The persisted-settings inventory

Exactly the runtime-mutable set, keyed to their config sections:

| Runtime value (Editor field) | Overrides key |
|---|---|
| `active_keymap_preset` | `[keymap] preset` |
| `theme.name` (builtin picks only — the picker lists builtins only) | `[theme] name` |
| `view_opts.typewriter/focus/measure/wrap_guide/word_count` | `[view] *` (the five) |
| `menu_bar_mode` | `[menu] bar` |
| `mouse_capture` | `[mouse] mouse_capture` |

- Theme nuance: if the session theme came from `theme.file` and was never changed, the
  runtime name equals the baseline's resolved name → no diff → nothing written. A picker
  pick always yields a builtin name → diffs → `[theme] name` written (which then outranks
  the hand `file=` at load, correctly — the user's last explicit choice wins).
- NEVER persisted: keymap patches, `theme.styles`/`file`/`depth` structures, diagnostics,
  export (no runtime mutator exists for them — nothing to diff), state (session.toml's
  domain), `view_opts.typewriter_anchor`/`focus_granularity`/`wrap_column` (no runtime
  mutator; the five TOGGLES only).
- `MenuBarMode` persists the CURRENT mode: if the user pinned the bar (`menu_bar_pin`),
  the pinned mode is what saves (by-design: "save what I'm looking at").

## D5. Settings menu category + C4 closure

- `MenuCategory::Settings` added (registry.rs enum + `MENU_ORDER` between View and
  Export + `category_label` arm "Settings"). Holds exactly three commands: `keymap_cua`,
  `keymap_wordstar`, `save_settings`. No existing items move (A3 curation's job).
- C4 closure (recorded decision): `close_buffer` remains unbound in both presets BY
  DESIGN. Rationale: `ctrl-w` is load-bearing in both presets; modified-F-keys are
  terminal-fragile; per-preset patches (D1 of this spec) are the supported user path
  (`[keymap.cua] bind={ "ctrl-w"='close_buffer' }` overrides expand_selection for a user
  who wants browser muscle-memory — their explicit, scoped choice).

## Error handling

- Overrides file unreadable/corrupt at load: same policy as any config layer today (parse
  error → warning, layer skipped) — verify and pin; a corrupt machine file must not brick
  startup.
- `save_settings` write failure: `SaveError` → status line, settings remain live in
  session (nothing lost), no partial file (atomic replace).
- Unknown preset in a scoped table: warn + skip (D1).
- `save_settings` while a modal/prompt is open: unreachable via menu/palette (they close);
  direct dispatch honors the same guards as other commands (plan verifies against the
  registry Ctx path — no special casing expected).

## Testing

- **Unit (config.rs/keymap.rs):** scoped-patch merge matrix — global-only layer
  (back-compat byte-identical), scoped beats global within a layer, later layer's global
  beats earlier layer's scoped, scoped table keys off active preset only, unknown-preset
  scoped table warns + skips; round-trip of the overrides serialization; baseline excludes
  the overrides layer (load a chain WITH an overrides file, assert baseline ≠ merged).
- **Behavior (app.rs):** switch command rebuilds the trie — the same chord (`ctrl-w`)
  resolves to `expand_selection` under cua and `scroll_line_up` after `keymap_wordstar`
  dispatch (through the real run-loop flag path or its testable seam); patches survive the
  switch (a global patch bind present under both bases; a scoped patch present only under
  its base); switch idempotence (re-dispatch active preset → status, no rebuild); dirty
  settings save→reload round-trip (save with the Fs-seam mock or a tempdir, re-load the
  chain, assert the runtime values restore); save failure surfaces on the status line
  (fault-injected via the M3 FaultFs); diff minimality (toggle one setting, save → file
  contains exactly that key; toggle it back, save → key gone); `close_buffer` unbound in
  both presets (a pin that fails if someone binds it).
- **e2e (e2e.rs):** one journey — switch preset via menu/palette, assert a
  preset-differentiated binding changes behavior; dispatch Save Settings, assert the
  overrides file exists with exactly the expected keys (Harness pointed at a temp
  config_dir via the layer seam — plan grounds how config_dir is injectable; if it is not,
  the journey pins the command dispatch + status and the file content is covered by the
  behavior tests through the seam).
- **Smoke:** advisory run + verbatim quote as always; no new smoke checks required by this
  effort.

## Deferred (recorded)

- Settings panel/modal overlay — E arc (E2 radio marks + E3 chrome), dispatching these
  same commands.
- Preset-scoped patches for MORE presets / a bind-editing UI — on demand.
- Config `include` mechanism if keymap patch sets outgrow the config file.
- Dirty-settings indicator ("unsaved settings" hint) — with E2.
