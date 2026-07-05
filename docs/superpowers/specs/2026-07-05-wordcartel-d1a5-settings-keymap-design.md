# D1+A5 — settings write-back + runtime keymap switching (design)

Status: CLEAN — Codex ×3 ("ready for planning") + Fable ×3 (READY; probe-verified TOML
schema, wiring, reachability; the diff law adversarially walked incl. cross-project and
--config-mask scenarios), 2026-07-05. Three user ratifications: contradiction-only-removal
diff law (I-2 A), rule-3 mask-guard (N-1 A), plus the seven design forks resolved one at a
time during brainstorm.
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

## Grounded facts (grounded at 2551463; fold-verified by Codex r1+r2 — line anchors may drift with fold commits)

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

- `KeymapPatch` gains two EXPLICIT optional named fields — `cua: Option<ScopedPatch>`,
  `wordstar: Option<ScopedPatch>` (`ScopedPatch` = the same `bind`/`unbind` pair) — NOT a
  flattened map, so future scalar keys in `[keymap]` can't be silently absorbed (Codex
  m-4; RawConfig has no deny_unknown_fields, so unknown sub-tables were already tolerated
  — the named-field shape keeps parsing strict where it matters). RawKeymap mirrors the
  same two optional sub-tables. Each layer still contributes exactly one `KeymapPatch`.
  Unknown sub-tables under `[keymap]` are invisible to serde (tolerated-unknown, exactly
  as today) — there is no unknown-preset warning because, with named fields, the case is
  structurally impossible.
- **Merge law — "later file wins; within a file, specific wins":** `build_keymap` applies,
  for each layer lowest→highest: the layer's GLOBAL bind/unbind, then the layer's
  scoped table for the ACTIVE preset only. (Consequence, documented: a scoped bind in an
  earlier layer can be beaten by a global bind in a later layer — layer precedence stays
  the single outer law.)
- Back-compat: configs with only global keys behave byte-identically (empty scoped maps).
- Hand-edit footgun, documented (Fable M-7, probe P1c): once a `[keymap.cua]` header
  appears, any `[keymap]`-intended key typed BELOW it silently belongs to the scoped
  table (TOML section semantics) and is tolerated-unknown-ignored — e.g. a trailing
  `preset = "wordstar"` vanishes. One sentence in the config docs; scoped tables make
  `[keymap]` multi-part for the first time.

`build_keymap`'s signature is unchanged — it already receives `&KeymapConfig`. Scoped
application keys off `resolve_preset(km.preset)` — the RESOLVED base, not the raw string
(Fable M-5: with `preset = "dvorak"` the base falls back to cua, so `[keymap.cua]` must
apply) — the same helper D2 mandates, so base and scoped application cannot disagree.

## D2. Runtime keymap switch (A5)

- Two commands (registry, `MenuCategory::Settings` — see D5): `keymap_cua` ("Keymap: CUA"),
  `keymap_wordstar` ("Keymap: WordStar"). Idempotent: switching to the already-active
  preset is a no-op with a status message ("keymap: cua (already active)" or similar —
  plan pins copy).
- `Editor` gains:
  - `active_keymap_preset: String` — seeded in `run()` from the RESOLVED preset via a
    new `pub fn resolve_preset(&str) -> &'static str` in keymap.rs (returns "cua" or
    "wordstar", falls back to "cua"), used by BOTH `build_keymap` internally and run()'s
    seeding so they cannot disagree (Codex m-3: build_keymap's return type doesn't expose
    the fallback today — the shared helper is REQUIRED, not optional). Read by: the
    switch commands (idempotence + diff), `save_settings` (D3), E2 later.
  - `keymap_rebuild: bool` — the rebuild request flag.
- Dispatch sets `active_keymap_preset` + `keymap_rebuild = true` + a status message
  ("keymap: wordstar"). The RUN LOOP, after `reduce` returns and before the next input
  wait, checks the flag: rebuilds via `build_keymap(&KeymapConfig { preset:
  editor.active_keymap_preset.clone(), patches: <the startup layer patches> }, &reg)`,
  reassigns the loop-local trie, clears the flag, CLEARS `editor.pending_keys` and its
  "… " pending status (Fable I-3: a half-typed prefix like `ctrl-k` can survive into the
  rebuild via the mouse-menu path — mouse.rs:242 opens the menu without clearing the
  buffer — and would complete against the NEW preset, misfiring e.g. `[ctrl-k, q]` →
  quit under WordStar), and surfaces any (unlikely) new warnings to the status line. The startup `cfg.keymap.patches` remain in scope in `run()` for exactly
  this (as `cfg` already stays alive for `max_entries`). Compile surface (Codex m-2): the
  loop-local becomes `let mut keymap = std::mem::take(&mut editor.keymap);`
  (app.rs:1334) — the single production `reduce` call site (app.rs:1473) is unchanged;
  nothing holds `&keymap` across iterations (Codex-verified).
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
  but save_settings never writes one; only hand edits could put one there, the header
  discourages hand edits, and a hand-added non-inventory key survives only until the
  next save: the wholesale inventory-only rewrite drops it — Fable N-2).
- **Baseline:** `load()` additionally returns (or exposes) the merged config WITHOUT the
  overrides layer — the baseline. Implementation shape: build the merge in two stages
  (hand chain + `--config` staged around the overrides layer — plan pins the exact
  two-pass or snapshot mechanics against load()'s accumulator loop). `run()` keeps the
  baseline's settings-relevant values (a small `SettingsSnapshot` struct) alive alongside
  `cfg`.
- **`save_settings`** (command "Save Settings", `MenuCategory::Settings`) — dispatch
  shape (Codex I-1: registry handlers are plain fns over `Ctx{editor, clock, executor,
  msg_tx}` — registry.rs:23/:58/:466 — and CANNOT reach run()-local baseline/paths): the
  handler sets `editor.settings_save_requested = true` + returns; the RUN LOOP, which
  owns the baseline snapshot and the overrides path, performs the save after `reduce`
  returns — the SAME between-reduces pattern as `keymap_rebuild` (D2). The loop collects
  the CURRENT runtime values (D4 inventory) from the Editor and computes the new file
  content by the **contradiction-only-removal diff law (user-ratified A, 2026-07-05,
  Fable I-2 — the baseline includes the anchor-dependent project layer, so a global file
  diffed naively against it lets saves from different directories erase each other's
  keys):** for each inventory key K —
  (1) runtime ≠ baseline → WRITE the runtime value (a new or updated override);
  (2) runtime == baseline AND the existing overrides layer has K with the SAME value →
  KEEP it (saved intent persists even where a project baseline coincides with it);
  (3) runtime == baseline AND the existing overrides layer has K with a DIFFERENT value →
  REMOVE it (the user actively changed the value back — the only un-save path) — UNLESS
  the `--config` layer itself sets K: then KEEP the existing override verbatim
  (**mask-guard, user-ratified A, 2026-07-05, Fable N-1**: `--config` is the one layer
  above the overrides, so a mask manufactures the coincidence+contradiction with no user
  action — while K is masked, rule 3 never fires; rule 1 still applies, so a GENUINE
  mid-masked-session change writes). Requires a third per-key snapshot of the `--config`
  layer's inventory values (`MaskSnapshot`, sibling of the other two). For the THEME key
  the mask predicate is PROVENANCE-based (Fable N-4): `--config` setting EITHER
  `[theme] name` OR `[theme] file` masks the `[theme] name` override (a later layer's
  `file` clears `name` in the merge, so a file-mask erases the pick by the same
  mechanism);
  (4) otherwise → absent.
  The overrides layer is already parsed in the chain; the loop keeps its raw inventory
  values (an `OverridesSnapshot`, sibling of the baseline snapshot). The result
  serializes via `toml::to_string` of a serde struct mirroring the config sections and
  writes atomically through the `Fs` seam (below). Synchronous fsync'd IO on the run
  loop matches the `persist_session` precedent (app.rs:1483) — tiny file, explicit
  command.
  Status line: "settings saved" / "settings: <io error>" on failure (`write_overrides`
  returns `std::io::Result<()>` from atomic_replace — no SaveError wrapper; the io::Error
  Display is the message body). No silent UI.
- **Write seam (Codex I-5):** `file::save_atomic_bytes` hardcodes `RealFs` (file.rs:176)
  and `FaultFs` is private to fsx tests (fsx.rs:369). The settings writer is therefore a
  function parameterized over the seam — `write_overrides(fs: &dyn crate::fsx::Fs, path,
  bytes)` calling `fsx::atomic_replace(fs, …)` directly (mode `Fixed(0o600)`, dir_fsync true —
  the exact opts `save_atomic_bytes` uses today, file.rs:176/fsx.rs:181: a machine-owned
  file like session.toml, not a user document); production passes `&RealFs`; the failure test passes
  a small test-local failing `Fs` impl (no need to expose FaultFs).
- **Refusals (Fable I-4/M-1):** under `--no-config` the baseline is a fiction (empty
  chain) and the session promised no config — `save_settings` is REFUSED with status
  "settings: disabled by --no-config". When `dirs::config_dir()` is None (odd/headless
  env; the load side already tolerates it — app.rs:1188) the save is refused with
  "settings: no config directory". Both pinned by test.
- **Directory creation (Codex I-2, shape per Fable M-6):** nothing creates `<config_dir>/wordcartel` today
  (config_layer_paths only reads; state.rs works because swap::state_dir creates its own
  dir — swap.rs:31). The save path runs `std::fs::create_dir_all` on the parent
  (0700 on Unix, mirroring the swap::state_dir precedent — swap.rs:31-40) BEFORE the
  atomic write, OUTSIDE the `Fs` seam (the trait stays create/write/rename/sync-only —
  Fable M-6); creation failure surfaces as the same status-line error. The
  write-failure fault test injects through the seam; the dir-creation test uses a real
  tempdir.
- **Reachability (Codex I-6, intentional):** modal guards live in reduce()'s input
  routing (app.rs:688) and overlay dispatch clears overlays before dispatching
  (app.rs:153) — so via keys the command is unreachable while a modal is open, and
  menu/palette close themselves. A programmatic `reg.dispatch` during a modal would only
  set the request flag (the loop does the IO) — harmless by construction. Stated as
  intentionally unrestricted; no special casing.
- Consequences (all pinned): idempotent within AND across sessions/projects (save twice
  → identical file; re-save from a project whose baseline coincides with a saved key →
  key preserved, rule 2); self-healing (delete the file → next session = hand config
  exactly); minimal in the common case (values never saved and never diverged don't
  appear, so hand-config edits shine through); un-saving is explicit (rule 3 — change the
  value back and save); `--config` can MASK a saved key (overrides sit below it): the
  save still reports "settings saved", the key has no effect while that `--config` is
  passed, and a masked save never removes it (rule 3's mask-guard) — recorded
  consequence, no warning in v1 (Fable M-4/N-1).
- Empty diff → the file is written with the header ONLY (never deleted — one write path,
  deterministic; an all-comments TOML parses to an empty layer, a no-op). Pinned by test.

## D4. The persisted-settings inventory

Exactly the runtime-mutable set, keyed to their config sections:

| Runtime value (Editor field) | Overrides key |
|---|---|
| `active_keymap_preset` | `[keymap] preset` |
| `theme_identity` (provenance — `Builtin(name)` written; `File` never) | `[theme] name` |
| `view_opts.typewriter/focus/measure/wrap_guide/word_count` | `[view] *` (the five) |
| `menu_bar_mode` | `[menu] bar` |
| `mouse_capture` | `[mouse] capture` (RawMouse field is `capture` — config.rs:226; Codex I-3) |

- Theme identity is PROVENANCE-based, not name-string-based (Codex I-4: a file theme's
  runtime name derives from its Base16 scheme name — theme_resolve.rs:100 — and can
  COLLIDE with a builtin name, so raw name-diffing can silently drop a picker pick).
  Definition: an identity is `File` (baseline resolved from `theme.file`) or
  `Builtin(name)`. The baseline's identity comes from the baseline config (`file` set →
  `File`; else `name`/default → `Builtin`). The Editor tracks the RUNTIME identity in a
  new `theme_identity` field: seeded at startup from the MERGED config's provenance
  (an overrides/hand `name` outranking `file` → `Builtin`); the theme picker sets it to
  `Builtin(previewed name)` on Enter ONLY IF a preview was actually applied during that
  open (Fable I-1: the real picker has NO commit-apply — Enter keeps the last preview,
  app.rs:469; an untouched open→Enter applies nothing and must leave the identity
  unchanged, else it would persist `rows[0]` the user never saw, or a file theme's
  scheme name as a fake Builtin). Mechanically: `ThemePicker` gains
  `previewed: Option<String>`, set by every `preview_selected_theme` application,
  consumed by Enter; Esc-restore clears it and restores `original`. The diff writes
  `[theme] name` iff runtime identity is `Builtin(n)` AND baseline identity !=
  `Builtin(n)` — so a picker pick over a file theme persists even when the names collide,
  and a never-touched file theme never writes. Rules 2/3 compare the provenance-typed key
  the same way (Fable N-3): rule 2 KEEPS `[theme] name = n` iff the runtime identity is
  `Builtin(n)`; an override name contradicting the runtime identity is rule-3-removed
  (subject to the mask-guard). Idempotent across sessions by
  construction (next session: runtime identity `Builtin(n)` from the overrides layer,
  baseline still `File` → key persists on re-save).
- NEVER persisted: `cycle_render_mode`'s per-buffer `RenderMode` (registry.rs:169 →
  `active().view.mode`, editor.rs:114 — runtime-mutable but PER-BUFFER state with no
  config key; recorded exclusion, Fable M-3), keymap patches, `theme.styles`/`file`/`depth` structures, diagnostics,
  export (no runtime mutator exists for them — nothing to diff), state (session.toml's
  domain), `view_opts.typewriter_anchor`/`focus_granularity`/`wrap_column` (no runtime
  mutator; the five TOGGLES only).
- `MenuBarMode` persists the CURRENT mode: if the user pinned the bar (`menu_bar_pin`),
  the pinned mode is what saves (by-design: "save what I'm looking at"). Accepted loss
  (Fable M-2): `menu_bar_unpinned_mode` is NOT persisted — after a save-while-pinned,
  the next session's unpin target is Auto (app.rs:1251-1255) even if the pre-pin mode
  was Hidden. Recorded; persisting the unpin target is E2-adjacent polish.

## D5. Settings menu category + C4 closure

- `MenuCategory::Settings` added — the complete compile surface (Codex m-5): the enum
  (registry.rs:38), `MENU_ORDER` between View and Export (registry.rs:41-42), the
  `category_label` arm "Settings" (menu.rs:60-68), plus any exhaustive matches over
  `MenuCategory` the compiler surfaces (house rule: no catch-all arms). Holds exactly three commands: `keymap_cua`,
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
- `save_settings` write failure: the `io::Error` → status line ("settings: <error>"),
  settings remain live in session (nothing lost), no partial file (atomic replace).
- Unknown scoped sub-tables under `[keymap]`: invisible to serde, tolerated as today
  (named-field schema — D1); nothing to warn.
- `save_settings` while a modal/prompt is open: via keys it is unreachable (modal guards
  live in reduce's input routing — app.rs:688; menu/palette close before dispatching —
  app.rs:153). `reg.dispatch` itself has NO modal guards (registry.rs:466) — and needs
  none here: the handler only sets the request flag; the loop does the IO. Intentionally
  unrestricted (single statement of D3's reachability rule; no contradiction).

## Testing

- **Unit (config.rs/keymap.rs):** scoped-patch merge matrix — global-only layer
  (back-compat byte-identical), scoped beats global within a layer, later layer's global
  beats earlier layer's scoped, scoped table keys off active preset only; `resolve_preset`
  fallback pin; round-trip of the overrides serialization (incl. `[mouse] capture` — the
  RawMouse field name); baseline excludes the overrides layer (load a chain WITH an
  overrides file, assert baseline ≠ merged); theme-identity diff matrix (picker pick over
  a file theme with a COLLIDING name persists; untouched file theme writes nothing;
  un-diverged never-saved builtin writes nothing); the diff-law matrix (rules 1-4:
  write-on-divergence, keep-on-coincidence — the cross-project walkthrough with two
  baselines, remove-on-contradiction, absent-otherwise; the mask-guard: a --config layer
  setting K → masked save keeps the override verbatim; the theme FILE-mask arm — a
  --config `[theme] file` also guards the `name` override; and a genuine masked-session
  divergence still writes).
- **Behavior (app.rs):** switch command rebuilds the trie — the same chord (`ctrl-w`)
  resolves to `expand_selection` under cua and `scroll_line_up` after `keymap_wordstar`
  dispatch (through the real run-loop flag path or its testable seam); patches survive the
  switch (a global patch bind present under both bases; a scoped patch present only under
  its base); switch idempotence (re-dispatch active preset → status, no rebuild); the rebuild
  clears a pending chord prefix (seed pending_keys, request rebuild, assert cleared);
  untouched picker open→Enter leaves theme_identity unchanged; save refused under
  --no-config and under config_dir()==None (status pins); dirty
  settings save→reload round-trip (save with the Fs-seam mock or a tempdir, re-load the
  chain, assert the runtime values restore); save failure surfaces on the status line
  (a test-local failing `Fs` impl through the `write_overrides` seam); first save creates
  `<config_dir>/wordcartel` when absent; diff minimality (toggle one setting, save → file
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
