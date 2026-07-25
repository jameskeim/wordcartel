# B18 — landmark-visibility toggle: design spec

**Date:** 2026-07-24. **Status:** draft for Codex spec gate.
**Item:** B18 (landmark visibility toggle — hide marks/block markers in-text while editing;
`backlog.toml` id `B18`, filed 2026-07-24). The visibility toggle Effort ④ deliberately
deferred ("add later only if the speckle proves real" — it has).
**Branch:** `effort-b18-landmark-toggle` off main @ `4339f67` (the ④ merge). No source edits
before plan execution.
**Grounding inputs:** `scratchpad/b18/` (`fable-grounding-brief.md`, `controller-prescan.md`,
`toggle-template.md`, `b18-forks.md` — all forks human-resolved 2026-07-24); every claim below
re-verified against the working tree at `4339f67`.

All anchors are SYMBOL names (file + symbol), never line numbers, except where a specific
expression is quoted.

**Framing intent (human):** a writer composing prose may not want the reversed landmark cells
peppering the text. The landmarks stay LIVE — jumps work, status keeps narrating — only the
in-text paint is suppressed, and only when the writer asks.

---

## 1. Summary and locked decisions

Human-locked decisions (2026-07-24, from `scratchpad/b18/b18-forks.md`):

1. **Scope.** ONE boolean view option, `view_opts.landmarks_visible` (global, not
   per-buffer), default `true` — ④'s current always-on look is the default. When `false`,
   ALL in-text landmark paint disappears: block boundary cells, interior tint, the pending
   ^KB anchor cell, LandmarkGlyph mark/bookmark cells, and the §3.3-④ trailing EOL markers.
2. **Fork 1 — in-text only.** The status segments (`· BLK`, `· BLK·hidden`, `BLK↑`/`BLK↓`,
   `· BLK…`, `· MK <ids>`) are UNTOUCHED and keep rendering when the toggle is off — that is
   the "landmarks live, jump still works" story. This falls out of the architecture for free
   (§2.2): the gate is a single early-return in `block_paint::gather`; `render_status.rs`
   reads `&Editor` directly and never sees `BlockPaint`.
3. **Fork 3 — inline flip, toggle-only.** The command flips the field inline in its handler
   (no shared setter, no set-per-state primitives) — matching all five existing single-path
   bool view toggles (§2.4). Contract-compliant: Law 6 is not test-enforced and its text
   scopes the setter requirement to multi-caller mutation (§5).
4. **Fork 2 — default ON; persists.** The option rides the full config + settings-overrides
   machinery; a deliberate OFF persists across sessions like every other view toggle.
5. **Fork 4 — naming.** Field `landmarks_visible`; config key `[view] landmarks_visible`;
   snapshot field `view_landmarks_visible`; command `toggle_landmarks`, label
   "Toggle Landmarks", `MenuCategory::View`, `register_stateful` with `MenuMark::OnOff`.
   NO default keybinding (none of the five view toggles has one; palette + View menu reach).
6. **Fork 5 — pending ^KB hidden when OFF: ACCEPTED.** A writer mid-mark with landmarks off
   sees no in-text anchor cell; the `· BLK…` status segment (which reads
   `pending_block_begin` directly) carries the mid-mark feedback. Documented in §4.
7. **Fork 6 — composes with per-block `block_toggle_hidden` with ZERO changes.** The new
   gate is the OUTERMOST early-return in `gather`; the existing
   `marked_block.filter(|mb| !mb.hidden)` sits below it. Global OFF hides everything
   (per-block flag moot); global ON preserves today's per-block behavior byte-for-byte.

Command-surface contract: **ENGAGES** (a new user-settable option + command) — conformance
in §5. Anti-regrowth: no budgeted hub is touched (§6). Template: `toggle_wrap_guide`,
mirrored across every layer (§3.2).

---

## 2. Current behavior (grounded, symbol-anchored)

### 2.1 The one seam for in-text landmark paint

`wordcartel/src/block_paint.rs` (Effort ④'s module) owns ALL in-text landmark paint:

- `pub(crate) fn gather(editor: &Editor) -> BlockPaint` — reads the active buffer:
  `block: b.marked_block.filter(|mb| !mb.hidden)`, `pending: b.pending_block_begin`,
  `landmarks:` the sorted+deduped values of `b.marks`. **Exactly one production caller:**
  `render.rs::gather_row_ctx` (`let block_paint = crate::block_paint::gather(editor);`).
  It takes `&Editor`, so it can read `editor.view_opts` directly.
- `BlockPaint`'s three private fields feed exactly three consumers, all in the render row
  path: `wants_placed()` (one term of `gather_row_ctx`'s `use_placed` disjunction — nothing
  else forces the placed path on behalf of landmarks), `patch_glyph` (the per-glyph face
  patch in `row_spans_placed`), and `trailing_marker` (the EOL/empty-line marker cell,
  called on an entry's last visual row).
- With an EMPTY `BlockPaint` (`block: None, pending: None, landmarks: vec![]`), each
  consumer degrades to a no-op by construction: `wants_placed()` → `false` (it is
  `self.block.is_some() || self.pending.is_some() || !self.landmarks.is_empty()`);
  `classify` → `None` → `patch_glyph` returns `style` unchanged; `trailing_marker` falls
  through every arm → `None`.
- The three landmark `SemanticElement`s (`MarkedBlock`, `MarkedBlockBoundary`,
  `LandmarkGlyph`) have NO production paint site outside `block_paint.rs` (verified by grep
  across both crates; all other hits are tests and the theme constructors). There is no
  second paint path — no scrollbar/gutter landmark marks.

Therefore: an early return of the empty value in `gather` suppresses ALL in-text landmark
paint, with no other production code change. That is the entire Fork-1 mechanism.

### 2.2 The status segments read `&Editor`, not `BlockPaint`

`render_status.rs::status_left_text` builds the landmark status directly from editor state:
the `· BLK` / `· BLK·hidden` match on `editor.active().marked_block`; the direction arrow
via `block_paint::blk_direction(editor, b)` (a free function on `&Editor`); the `· BLK…`
append on `editor.active().pending_block_begin.is_some()`; the `· MK <ids>` segment via
`block_paint::marks_on_caret_line(editor)` (also a free function on `&Editor`). None of
these touch `BlockPaint`, so gating `gather` cannot affect them. **B18 does not edit
`render_status.rs` at all.**

### 2.3 The `toggle_wrap_guide` template (the option B18 mirrors layer-for-layer)

- **Field:** `config.rs::ViewConfig.wrap_guide: bool`, seeded `false` in `impl Default for
  ViewConfig`. The Editor carries the whole struct: `editor.rs::Editor.view_opts:
  crate::config::ViewConfig` (constructed `ViewConfig::default()`), overwritten at startup
  by `startup.rs`: `editor.view_opts = cfg.view.clone();` — a new `ViewConfig` field flows
  from config to the live editor with zero extra wiring.
- **Config serde + merge:** `config.rs::RawView.wrap_guide: Option<bool>`
  (`#[serde(default)]` struct); in `load`'s view merge block:
  `if let Some(v) = raw.view.wrap_guide { cfg.view.wrap_guide = v; }`. Plain bool — no
  validation arm.
- **Command:** `registry.rs::register_builtins`:
  ```rust
  r.register_stateful("toggle_wrap_guide", "Toggle Wrap Guide", Some(MenuCategory::View),
      |e| MenuMark::OnOff(e.view_opts.wrap_guide),
      |c| { c.editor.view_opts.wrap_guide = !c.editor.view_opts.wrap_guide; CommandResult::Handled });
  ```
  No `derive::rebuild()` — wrap_guide is paint-only (contrast `toggle_measure`, which
  rebuilds because layout changes).
- **Settings persistence:** `settings.rs` — `SettingsSnapshot.view_wrap_guide: bool`;
  `snapshot_of` (`cfg.view.wrap_guide`); `runtime_snapshot` (`editor.view_opts.wrap_guide`);
  `OView.wrap_guide: Option<bool>` (`#[serde(skip_serializing_if = "Option::is_none")]`);
  in `compute_overrides`, a `diff_key` call
  (`fn diff_key<T: PartialEq + Clone>(rt: &T, base: &T, existing: Option<&T>, masked: bool)
  -> Option<T>`) plus membership in the `any_view` disjunction and the `OView` struct
  literal passed to `some_if`.
- **Law-2 guard:** `settings.rs::every_persisted_setting_has_a_command` destructures
  `SettingsSnapshot` with NO `..` (the `field_guard` fn), so adding a snapshot field is a
  COMPILE error until a `has("…")` assertion line is added. The assertion checks only that
  the command name resolves: `let has = |id: &str| reg.resolve_name(id).is_some();`.
- **Struct-literal sites the compiler forces** (because the snapshot has no `Default` and
  the guard has no `..`): `settings.rs` tests' `snap()` helper, `config.rs` test
  `save_reload_roundtrip_restores_settings`'s `runtime` literal, and `e2e.rs`'s
  settings-save `baseline` literal. **All three named here because an earlier sweep missed
  two of them** — the compile errors are the enforcement, but the plan must budget for them.
- **Palette / menu / hints:** automatic — the palette is exhaustive over the registry; the
  View menu row and On/Off mark come from `Some(MenuCategory::View)` + the state closure;
  keybinding hints resolve from the active `KeyTrie` (no binding → no hint).

### 2.4 The inline-flip precedent class (Fork 3's grounding)

All five existing global bool view toggles with a single mutation path flip inline in their
registered handler: `toggle_typewriter`, `toggle_focus`, `toggle_measure`,
`toggle_wrap_guide`, `toggle_word_count` (`registry.rs::register_builtins`, the
"View menu — writing-experience toggles" block). Named shared setters exist exactly where a
SECOND caller exists: `ventilate::set_ventilate` (called by `toggle_ventilate` AND
`lenses.rs`), `lenses::set_prose_lens` (six commands), `Editor::set_splash` /
`set_caret_blink` / `set_caret_shape` / `set_show_clutter` / `set_messages_min_kind` (each
shared by a toggle + on/off/set primitives), `workspace::switch_to` (commands + the dynamic
Documents menu section). No profile or preset touches landmark visibility (the chrome
ZEN/FULL preset sets `status_line`/`scrollbar` modes only).

### 2.5 What per-block `hidden` does today

`blocks_marked::block_toggle_hidden` flips `MarkedBlock.hidden` on the active buffer;
`gather` filters it (`.filter(|mb| !mb.hidden)`); `status_left_text` renders `· BLK·hidden`
for a hidden block (no arrow). Per-block `hidden` is runtime per-buffer state, NOT
persisted. `landmarks_visible` will be the first *persisted* landmark-related setting.

---

## 3. Design

### 3.1 The gate — one early-return in `block_paint::gather`

```rust
/// Gather the active buffer's landmark state. O(#marks log #marks), once per frame.
/// B18: when the user has toggled landmark visibility OFF, return the empty snapshot —
/// every consumer (wants_placed / patch_glyph / trailing_marker) no-ops on it, so ALL
/// in-text landmark paint is suppressed at this single seam. The status segments
/// (render_status.rs) read the Editor directly and deliberately survive (Fork 1).
pub(crate) fn gather(editor: &Editor) -> BlockPaint {
    if !editor.view_opts.landmarks_visible {
        return BlockPaint { block: None, pending: None, landmarks: Vec::new() };
    }
    let b = editor.active();
    let mut landmarks: Vec<usize> = b.marks.values().copied().collect();
    landmarks.sort_unstable();
    landmarks.dedup();
    BlockPaint { block: b.marked_block.filter(|mb| !mb.hidden), pending: b.pending_block_begin, landmarks }
}
```

The existing body is untouched below the new guard. `Vec::new()` allocates nothing (empty
vec) — the OFF path is cheaper than the ON path. Composition with per-block `hidden`
(locked decision 7) is by position: the global gate is OUTERMOST; when it passes, the
per-block filter applies exactly as today.

**This is the only production edit outside the option-plumbing template.** `render.rs`,
`render_status.rs`, `blocks_marked.rs`, and `marks.rs` are not touched.

### 3.2 The option plumbing (the wrap_guide template, applied)

Each row mirrors the §2.3 anchor; place every new field/arm beside its `wrap_guide`
counterpart for grep coherence.

1. **`config.rs`** — `ViewConfig.landmarks_visible: bool`, doc-commented:
   ```rust
   /// In-text landmark paint (block boundaries/tint, pending ^KB cell, mark cells —
   /// `[view] landmarks_visible`). Default true (④'s always-on). OFF suppresses the
   /// paint at the block_paint::gather seam; the status segments always survive (B18).
   pub landmarks_visible: bool,
   ```
   `impl Default for ViewConfig` seeds `landmarks_visible: true`.
   `RawView.landmarks_visible: Option<bool>`. Merge line in `load`'s view block:
   `if let Some(v) = raw.view.landmarks_visible { cfg.view.landmarks_visible = v; }`.
2. **`registry.rs`** — beside `toggle_wrap_guide`:
   ```rust
   r.register_stateful("toggle_landmarks", "Toggle Landmarks", Some(MenuCategory::View),
       |e| MenuMark::OnOff(e.view_opts.landmarks_visible),
       |c| { c.editor.view_opts.landmarks_visible = !c.editor.view_opts.landmarks_visible; CommandResult::Handled });
   ```
   `register_stateful` (not `register_mut`): a view option, not a document edit — identical
   to every neighbor. NO `derive::rebuild()` — paint-only; layout, ColMap, and wrap are
   untouched by landmark paint (④'s B-lite invariant: styles existing cells only).
3. **`settings.rs`** —
   - `SettingsSnapshot.view_landmarks_visible: bool` (doc-comment: "In-text landmark paint
     (`view.landmarks_visible`). Flipped inline by `toggle_landmarks` (B18).").
   - `snapshot_of`: `view_landmarks_visible: cfg.view.landmarks_visible,`.
   - `runtime_snapshot`: `view_landmarks_visible: editor.view_opts.landmarks_visible,`.
   - `OView.landmarks_visible: Option<bool>` with
     `#[serde(skip_serializing_if = "Option::is_none")]`.
   - `compute_overrides`: a `diff_key` call mirroring wrap_guide's —
     ```rust
     let landmarks_visible = diff_key(
         &runtime.view_landmarks_visible, &baseline.view_landmarks_visible,
         ex_view.and_then(|v| v.landmarks_visible.as_ref()),
         mk_view.and_then(|v| v.landmarks_visible).is_some(),
     );
     ```
     plus `|| landmarks_visible.is_some()` in the `any_view` disjunction and
     `landmarks_visible` in the `OView` struct literal passed to `some_if`.
   - `every_persisted_setting_has_a_command`: the `field_guard` destructure gains
     `view_landmarks_visible: _,` and the assertion list gains
     `assert!(has("toggle_landmarks"), "view_landmarks_visible");`.
4. **The three compiler-forced `SettingsSnapshot` test literals** (§2.3): `settings.rs`
   `snap()` gains `view_landmarks_visible: true,`; `config.rs`
   `save_reload_roundtrip_restores_settings`'s `runtime` literal gains
   `view_landmarks_visible: true,` (unchanged from default — the test's divergence set is
   deliberately untouched); `e2e.rs`'s settings-save `baseline` literal gains
   `view_landmarks_visible: true,`.
5. **Palette / menu / hints:** zero code — automatic per §2.3. The View menu shows
   "Toggle Landmarks · On/Off" from the state closure; the palette lists it; no default
   keybinding is added anywhere in `keymap.rs` (locked decision 5), so no hint renders
   until a user patch-binds it (law 7 machinery then surfaces it automatically).

### 3.3 What OFF looks like (the behavior contract)

With `landmarks_visible == false`:
- No block boundary cells, no interior tint, no pending anchor cell, no mark/bookmark
  cells, no trailing `[` / `]` / `·` EOL markers — the canvas is indistinguishable from a
  buffer with no landmarks at all.
- `wants_placed()` contributes `false`, so a buffer whose ONLY placed-path trigger was
  landmarks drops back to the cheap segs path — the OFF state is also a (marginal)
  performance win, and `use_placed`'s other terms (search, diagnostics, selection, prose
  lens) are unaffected.
- The status line still shows: `· BLK` (+ `↑`/`↓` direction when scrolled off), or
  `· BLK·hidden` for a per-block-hidden block; `· BLK…` while a ^KB anchor is pending
  (locked decision 6 — the accepted mid-mark feedback); `· MK <ids>` on the caret's line.
  The two suppression states remain distinguishable in status: per-block hidden reads
  `· BLK·hidden`; globally-unpainted (toggle off) reads plain `· BLK`(+arrow).
- Every landmark COMMAND behaves identically: set/jump/clear marks and bookmarks, ^KB/^KK,
  block ops, `block_toggle_hidden`, jumps (which still unfold and ensure-visible). The
  model layer never consults the toggle.

With `landmarks_visible == true` (default): behavior is byte-for-byte ④'s shipped
rendering — the gate adds one branch-not-taken to `gather`.

---

## 4. Visible behavior changes (deliberate, user-facing)

- **New command** "Toggle Landmarks" in the View menu (with On/Off state) and the palette.
- **OFF hides the pending ^KB anchor cell** (locked decision 6, Fork 5): a writer mid-mark
  with landmarks off must rely on the `· BLK…` status segment for the "^KK pending" state.
  Accepted — the writer chose to suppress in-text paint, and the status narrates.
- **The OFF choice persists across sessions** (config + settings-overrides). A user who
  toggles landmarks off and later wonders where the paint went finds the answer in the View
  menu's Off mark — the same discoverability story as every persisted view toggle.
- No default-state change: fresh configs render exactly as ④ shipped.

---

## 5. Command-surface contract — conformance (ENGAGES)

Per `docs/design/command-surface-contract.md`:

- **Law 1 (registry = single source of truth):** the ONLY runtime mutation path for
  `landmarks_visible` is the registered `toggle_landmarks` handler. Startup seeding
  (`startup.rs`'s `editor.view_opts = cfg.view.clone()`) is config application, identical
  to every existing view option.
- **Law 2 (every option is a command):** `view_landmarks_visible` enters
  `SettingsSnapshot`; `toggle_landmarks` is its command; the
  `every_persisted_setting_has_a_command` assertion is added, and the no-`..` destructure
  makes omission a compile error. **Enforcing GATE test.**
- **Law 3 (palette exhaustive):** automatic on registration; the palette-completeness
  invariant covers it with zero new test code. **GATE (existing suite).**
- **Law 4 (menu ⊆ palette):** the View row names a registered command — holds by
  construction.
- **Law 5 (keyboard path):** the palette.
- **Law 6 (one setter; profiles use it too):** the inline flip is compliant (locked
  decision 3, grounded): the contract's Enforcement section lists enforcing tests for laws
  2/3/7 only — Law 6 has NO enforcing test — and the law's own text plus decision-procedure
  step 3 ("Does a profile set it? → the profile calls the same setter") scope the shared
  setter to multi-caller mutation. `landmarks_visible` has exactly one mutation path (the
  handler), no profile touches it, and the five-strong inline precedent class (§2.4) is the
  established house pattern. If a future effort adds a second caller (set-per-state
  primitives, a profile, a dynamic menu row), THAT effort introduces
  `set_landmarks_visible` and reroutes the toggle through it — the same evolution
  `splash`/`caret_blink` already followed.
- **Law 7 (hints track the active keymap):** automatic; unbound ⇒ no hint; a user patch
  binding surfaces in both palette and menu via the existing re-resolution machinery.
  **GATE (existing suite).**
- **Rule 8 (multi-state shape):** a 2-state bool; the toggle IS the stateful menu
  representative with state-in-label (`MenuMark::OnOff`) — the
  `toggle_chrome`/`toggle_canvas` precedent. Set-per-state primitives deliberately NOT
  added (locked decision 3); Effort P's parameterized set-commands can absorb this option
  without breaking the contract (rule 10's forward path).
- **Rules 9/10:** no preset involved; `toggle_landmarks` is nullary and
  registry-dispatched — plugin-callable like every command.

---

## 6. Anti-regrowth: GATE conformance accounting

- **`module_budgets`:** the budgeted hubs are `app.rs` (1000), `render.rs` (900),
  `timers.rs` (400), `plugin/host.rs` (400), and `plugin/pump.rs` (350)
  (`wordcartel/tests/module_budgets.rs`) — **B18 touches none of them in production.**
  All production edits land in `block_paint.rs` (+3 lines), `config.rs`, `registry.rs`
  (1 row), `settings.rs` — none budgeted. `render.rs` and `render_status.rs` have ZERO
  PRODUCTION edits; a TEST may be added (§7 — `production_lines` counts only lines before
  `mod tests`, so a test addition cannot move a budget).
- **`clippy::too_many_lines` (100):** `gather` grows to ~10 lines; every other edit is a
  field/arm/row. No `#[allow]` anticipated.
- **Registration seam doctrine:** the new behavior enters via a registry row + a gate
  inside the existing feature module — no dispatcher grows.
- Standard GATEs: `cargo test` all suites; `cargo build`/`cargo test --no-run`
  warning-free; `cargo clippy --workspace --all-targets` clean. PTY smoke suite run +
  verbatim one-line summary at pre-merge (advisory). `cargo fmt` never run (house style);
  match neighbors by hand.

---

## 7. Test plan (contracts; TDD detail belongs to the plan)

**`block_paint.rs` unit tests**
- `gather` with the toggle OFF returns the empty snapshot: `wants_placed()` false even
  with a visible block + pending + marks all set on the buffer (RED first — the field
  doesn't exist yet).
- `gather` with the toggle ON (default) is unchanged: the existing
  `gather_filters_hidden_and_sorts_marks` stays green unmodified.
- Composition (locked decision 7): toggle ON + per-block hidden → block filtered (today's
  behavior); toggle OFF + per-block NOT hidden → still empty.

**render / TestBackend tests (the "render the screen, don't assert the struct"
discipline — S8/C5 lesson)**

Where the tests live (to keep §6's claim precise): the GATE-level tests are
`block_paint.rs` unit tests; the end-to-end OFF-hides-cells-AND-status-survives assertion
is a TestBackend test (the e2e `Harness` or `render.rs`'s `mod tests`) — a TEST addition
to those files, never a production edit.
- The paint test, both polarities on one buffer with a completed block + a mark: toggle ON
  → the boundary cell carries REVERSED (and the mark cell ITALIC, per the ④ pins); flip
  `view_opts.landmarks_visible = false` → re-render → those same cells carry NONE of the
  landmark modifiers/faces (assert the actual cell styles, not `BlockPaint` internals).
- **Status survival (the Fork-1 contract, pinned):** in the OFF render, the status line
  still contains `· BLK` (and `· MK <id>` with the caret on the mark's line); with a
  pending ^KB, `BLK…` still shows while the anchor cell does not paint (locked
  decision 6's edge, pinned as a test).
- The existing ④ suite (`marked_block_paints_and_status_shows_blk`, the trailing-marker
  probes, `journey_landmarks_visible_and_clearable`) stays green unmodified — default ON.

**Command + persistence**
- Dispatch `toggle_landmarks` twice through the registry: field flips false then true;
  the menu state closure reports `MenuMark::OnOff` matching the field (mirror
  `toggle_ventilate_is_stateful_onoff_and_flips_the_flag`'s shape).
- Config fold: default true; a layer with `[view]\nlandmarks_visible = false\n` folds to
  false (mirror `view_splash_defaults_on_and_folds_from_a_layer`).
- Settings round-trip (mirror `splash_round_trips_through_snapshot_diff_and_parse`):
  runtime false vs baseline true → `compute_overrides` writes
  `[view] landmarks_visible = false`; serialized TOML re-parses via `parse_overrides`;
  no divergence → no key (diff-law rule 4); `runtime_snapshot` reads the live field.
- `every_persisted_setting_has_a_command` extended (§3.2 item 3) — the Law-2 GATE.

---

## 8. Out of scope (unchanged decisions, follow-ons)

- **Hiding the status segments** — rejected at fork time (Fork 1B): the editor would give
  no visual evidence that live landmarks affect ^K commands.
- **Always-painting the pending anchor when OFF** — rejected (Fork 5B): would split the
  one-gate invariant; status carries the feedback.
- **Set-per-state primitives + shared setter** (`landmarks_on`/`landmarks_off`/
  `set_landmarks_visible`) — deferred to whenever a second caller exists (Effort P
  parameterized sets, a profile, or a dynamic row); §5 Law 6 records the evolution path.
- **Default keybinding** — none; user patch-binding is the supported path.
- **Per-buffer visibility** — the option is global (`view_opts`), like every view toggle;
  per-buffer suppression already exists for the block via `block_toggle_hidden`.

## 9. Backlog bookkeeping (at merge)

`backlog.toml`: B18 → shipped (this effort's merge); its prose section moves from
`docs/ux-backlog.md` to `docs/backlog-archive.md` with `doc =` repointed;
`scripts/backlog bless`.

## 10. Residual claims — verify at execute

None load-bearing. Two honesty notes: (1) the OFF-state screen assertions are fully
provable in `TestBackend` (paint suppression is cell-style absence — no live-terminal
capability like A21's mouse-motion is involved), so no tui-interact pass is REQUIRED;
a brief live eyeball of the View-menu On/Off row is a courtesy, not a gate. (2) The e2e
journey `journey_landmarks_visible_and_clearable` predates B18 and shares the phrase
"landmarks_visible" — a name collision in grep results only, no symbol conflict; do not
rename it.

## History

- 2026-07-24 — drafted for the Codex spec gate (Fable warm thread, effort B18), from the
  human-resolved forks in `scratchpad/b18/b18-forks.md`.
- 2026-07-24 — Codex spec gate: READY (no Critical/Important). Two Minor wording folds:
  §6 hub list completed (`plugin/host.rs` + `plugin/pump.rs` added; the zero-hub claim
  holds) and §6/§7 now distinguish zero PRODUCTION edits to `render.rs`/`render_status.rs`
  from test additions (test placement stated in §7).
