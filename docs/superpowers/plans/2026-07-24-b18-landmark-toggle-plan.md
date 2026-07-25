# B18 — Landmark-Visibility Toggle Implementation Plan

**Spec:** `docs/superpowers/specs/2026-07-24-b18-landmark-toggle-design.md` (Codex-gated
READY; two Minor wording folds applied). **Branch:** `effort-b18-landmark-toggle` off main
@ `4339f67` (already created).
**Anchors are SYMBOL names** — re-locate by name (`workspaceSymbol`/grep), never trust a
line number. For compile/usage questions on code you are editing, trust `cargo` + `grep`,
NOT editor/LSP "unused"/"undefined" hints (they lag edits; the controller's diagnostics
about your files are the most stale).

## Global Constraints

- **Commit only when a task says commit; push never.** Every commit message ends with the
  two project trailers, verbatim:

  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01FAx3iA5vRBiLXEfCnudR6j
  ```

- **Do NOT run `cargo fmt`** (no rustfmt.toml; the tree is hand-formatted — match
  neighbors by hand; never reflow untouched code).
- **Merge GATEs** (every task ends green; Task 5 re-verifies all): `cargo test --workspace`
  green; `cargo build` and `cargo test --no-run` warning-free for touched crates;
  `cargo clippy --workspace --all-targets` clean; `module_budgets` green — B18 touches NO
  budgeted hub in production (`app.rs`/`render.rs`/`timers.rs`/`plugin/host.rs`/
  `plugin/pump.rs`), so the budgets must pass untouched.
- **LOCKED decisions (do not re-open; a conflict is a HUMAN decision):** default
  `landmarks_visible = true` (④'s always-on look is the out-of-box state); the gate is
  IN-TEXT ONLY — one early-return in `block_paint::gather`, the status segments
  (`· BLK`, `· BLK·hidden`, `BLK↑`/`BLK↓`, `· BLK…`, `· MK <ids>`) always survive and
  `render_status.rs` gets ZERO production edits; INLINE flip in the command handler — no
  shared setter, no set-per-state primitives (the wrap_guide precedent class; Law 6 is
  not test-enforced and scopes setters to multi-caller mutation); names are
  `landmarks_visible` / `toggle_landmarks` / "Toggle Landmarks" /
  `Some(MenuCategory::View)` + `MenuMark::OnOff`; NO default keybinding (zero `keymap.rs`
  edits); pending-^KB cell hidden when OFF is ACCEPTED (the `· BLK…` status carries
  mid-mark feedback); composes with per-block `block_toggle_hidden` with zero changes
  (the global gate is OUTERMOST; the existing `.filter(|mb| !mb.hidden)` sits below it).
- **No new `SemanticElement`**, no theme edits, no `derive::rebuild()` (paint-only —
  layout, ColMap, and wrap are untouched; the wrap_guide template, not the measure one).
- **Perf note (spec §3.3):** the OFF path is cheaper than ON — an empty `BlockPaint`
  makes `wants_placed()` false, so a buffer whose only placed-path trigger was landmarks
  drops back to the segs render path. Never add per-keystroke O(document) work.
- **`render.rs` and `render_status.rs`: zero PRODUCTION edits.** The only test file
  additions outside the plumbing files are `block_paint.rs` unit tests and one `e2e.rs`
  journey (spec §7's placement note).

## Command-surface contract (how this plan honors it)

`toggle_landmarks` makes the new persisted option a command (Law 2 — enforced by the
Task 3 `every_persisted_setting_has_a_command` assertion, whose no-`..` destructure makes
omission a compile error); the palette lists it automatically (Law 3 — the palette
enumerates the registry; the existing completeness invariant covers it); the View menu
carries it as the stateful On/Off representative (Law 4/Rule 8 —
`toggle_chrome`/`toggle_canvas` precedent); the palette is the keyboard path (Law 5); the
inline flip is the single mutation path (Law 6 — compliant per the spec §5 grounding: no
enforcing test exists, no profile touches the option, five-strong inline precedent);
hints track the active keymap automatically, none rendered while unbound (Law 7); the
command is nullary and registry-dispatched, hence plugin-callable (Rule 10). Config +
settings persistence rides the full wrap_guide chain (Task 1 + Task 3).

## File Map

| File | Change |
|---|---|
| `wordcartel/src/config.rs` | `ViewConfig.landmarks_visible` + `Default` true + `RawView` mirror + merge line; config fold test; `save_reload_roundtrip_restores_settings` literal (T3) |
| `wordcartel/src/registry.rs` | 1 `register_stateful` row + 1 test |
| `wordcartel/src/settings.rs` | `SettingsSnapshot.view_landmarks_visible` + both builders + `OView` + `diff_key` + `any_view` + `field_guard` + law-2 assert + `snap()` literal + round-trip test |
| `wordcartel/src/e2e.rs` | settings-save `baseline` literal (T3); 1 journey (T4) |
| `wordcartel/src/block_paint.rs` | the 3-line gate in `gather` + unit tests |

## Task Order & Rationale

1. **T1 config option** — the field exists end-to-end in config (defaults, serde, merge);
   nothing reads it yet, tree green.
2. **T2 command** — `toggle_landmarks` flips the real field; registered BEFORE Task 3 so
   the law-2 assertion lands green.
3. **T3 settings persistence** — the snapshot field + the whole diff chain + the three
   compiler-forced test literals + the law-2 assert, one commit so every literal compiles
   together.
4. **T4 the gate + behavior tests** — the only behavior change, red-first at both the
   unit and the journey level.
5. **T5 final verification** — full gates + PTY smoke (advisory), no merge.

Each intermediate state compiles and is fully green at its commit.

---

### Task 1: The config option (`ViewConfig.landmarks_visible`)

**RED.** In `wordcartel/src/config.rs`'s `mod tests`, directly after
`view_splash_defaults_on_and_folds_from_a_layer` (its exact template):

```rust
    #[test]
    fn view_landmarks_visible_defaults_on_and_folds_from_a_layer() {
        let (cfg, warns) = load(&[]);
        assert!(warns.is_empty());
        assert!(cfg.view.landmarks_visible, "built-in default is on (④ always-on)");
        let d = tempdir();
        let p = write(&d, "landmarks.toml", "[view]\nlandmarks_visible = false\n");
        let (cfg, warns) = load(&[p]);
        assert!(warns.is_empty());
        assert!(!cfg.view.landmarks_visible, "a layer that SETS the field overrides the default");
    }
```

Baseline state: `ViewConfig` has no `landmarks_visible` field. Red = **compile error
E0609** (`no field landmarks_visible`) — `cargo test -p wordcartel config` fails to
build. (Named-baseline red: a new-field test's red is the compile failure; no runtime-red
is possible before the field exists.)

**GREEN.** Four edits in `wordcartel/src/config.rs`:

1. `ViewConfig` — after `pub wrap_guide: bool,`:

```rust
    /// In-text landmark paint (block boundaries/tint, pending ^KB cell, mark cells —
    /// `[view] landmarks_visible`). Default true (④'s always-on). OFF suppresses the
    /// paint at the `block_paint::gather` seam; the status segments always survive (B18).
    pub landmarks_visible: bool,
```

2. `impl Default for ViewConfig` — the dense literal line
   `wrap_column: 72, wrap_guide: false, word_count: false,` becomes:

```rust
            wrap_column: 72, wrap_guide: false, landmarks_visible: true, word_count: false,
```

3. `RawView` — after `wrap_guide: Option<bool>,`:

```rust
    landmarks_visible: Option<bool>,
```

4. The view merge block in `load` — after the `wrap_guide` line:

```rust
        if let Some(v) = raw.view.landmarks_visible { cfg.view.landmarks_visible = v; }
```

**Verify:** `cargo test -p wordcartel config` green (the new test + all config tests);
`cargo clippy -p wordcartel --all-targets` clean.

**Commit:** `b18 T1: [view] landmarks_visible config option (default on)` + trailers.

---

### Task 2: The `toggle_landmarks` command (inline flip, View menu)

**RED.** In `wordcartel/src/registry.rs`'s `mod tests`, directly after
`toggle_ventilate_is_stateful_onoff_and_flips_the_flag` (its exact template; the test
fixtures `Z`, `InlineExecutor`, `test_support::test_fs` are already in scope there):

```rust
    /// B18: `toggle_landmarks` is the stateful View-menu representative for the global
    /// `view_opts.landmarks_visible` paint toggle (command-surface Laws 2/6/8) — an
    /// INLINE flip (single mutation path; no setter, matching the wrap_guide class).
    #[test]
    fn toggle_landmarks_is_stateful_onoff_and_flips_the_field() {
        let reg = Registry::builtins();
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
        let m = reg.meta(CommandId("toggle_landmarks")).expect("toggle_landmarks registered");
        assert_eq!(m.menu, Some(MenuCategory::View), "toggle_landmarks is a View row");
        let f = m.state.expect("toggle_landmarks is stateful");
        assert!(matches!(f(&ed), MenuMark::OnOff(true)), "defaults ON (④ always-on)");
        let ex = InlineExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut ed, clock: &Z, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        assert_eq!(reg.dispatch(CommandId("toggle_landmarks"), &mut ctx), CommandResult::Handled);
        assert!(!ed.view_opts.landmarks_visible, "dispatch turned paint off");
        assert!(matches!(f(&ed), MenuMark::OnOff(false)));
    }
```

Baseline state: Task 1 merged — the field exists, the command does not. Red = **runtime
panic** at `.expect("toggle_landmarks registered")` (`Registry::resolve_name` /`meta`
returns `None` for an unregistered id) — a genuine failing test, not a compile error.

**GREEN.** In `registry.rs::register_builtins`, directly after the `toggle_wrap_guide`
row (the "View menu — writing-experience toggles" block):

```rust
        r.register_stateful("toggle_landmarks", "Toggle Landmarks", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.landmarks_visible),
            |c| { c.editor.view_opts.landmarks_visible = !c.editor.view_opts.landmarks_visible; CommandResult::Handled });
```

No `derive::rebuild()` (paint-only). No keymap edit. Nothing else.

**Verify:** `cargo test -p wordcartel registry` green (the new test + the palette
completeness invariant, which now enumerates the new row automatically);
`cargo clippy -p wordcartel --all-targets` clean.

**Commit:** `b18 T2: toggle_landmarks command — stateful View OnOff, inline flip` + trailers.

---

### Task 3: Settings persistence (snapshot → diff law → overrides file)

**RED.** In `wordcartel/src/settings.rs`'s `mod tests`, directly after
`splash_round_trips_through_snapshot_diff_and_parse` (its exact template):

```rust
    #[test]
    fn landmarks_visible_round_trips_through_snapshot_diff_and_parse() {
        // snapshot_of reads the config default (on); runtime diverges to off.
        let baseline = snapshot_of(&crate::config::Config::default(), "tokyo-night");
        assert!(baseline.view_landmarks_visible, "config default is on");
        let mut runtime = baseline.clone();
        runtime.view_landmarks_visible = false;
        let of = compute_overrides(&runtime, &baseline,
            &OverridesFile::default(), &OverridesFile::default());
        assert_eq!(of.view.as_ref().and_then(|v| v.landmarks_visible), Some(false),
            "divergence writes the key");
        // …and the written key deserializes back through parse_overrides.
        let text = toml::to_string(&of).expect("serialize overrides");
        let re = parse_overrides(&text);
        assert_eq!(re.view.and_then(|v| v.landmarks_visible), Some(false));
        // No divergence → the key (and the empty section) stays absent (rule 4).
        let of2 = compute_overrides(&baseline, &baseline,
            &OverridesFile::default(), &OverridesFile::default());
        assert!(of2.view.is_none(), "unchanged toggle writes no view key");
        // runtime_snapshot reads the live editor field (the inline-flip path).
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        e.view_opts.landmarks_visible = false;
        assert!(!runtime_snapshot(&e).view_landmarks_visible);
    }
```

Baseline state: Tasks 1-2 merged — `SettingsSnapshot` has no `view_landmarks_visible`
field, `OView` has no `landmarks_visible`. Red = **compile error E0609/E0560** in the new
test. (Named-baseline red: compile failure; the field does not exist yet.)

**GREEN.** Edits in `wordcartel/src/settings.rs` (each beside its `wrap_guide` sibling):

1. `SettingsSnapshot` — after `pub view_wrap_guide: bool,`:

```rust
    /// In-text landmark paint (`view.landmarks_visible`). Flipped inline by
    /// `toggle_landmarks` (B18); OFF hides paint only — status segments survive.
    pub view_landmarks_visible: bool,
```

2. `snapshot_of` — after the `view_wrap_guide:` line:

```rust
        view_landmarks_visible: cfg.view.landmarks_visible,
```

3. `runtime_snapshot` — after the `view_wrap_guide:` line:

```rust
        view_landmarks_visible: editor.view_opts.landmarks_visible,
```

4. `OView` — after the `wrap_guide` field:

```rust
    #[serde(skip_serializing_if = "Option::is_none")] pub landmarks_visible: Option<bool>,
```

5. `compute_overrides` — after the `wrap_guide` `diff_key` block:

```rust
    let landmarks_visible = diff_key(
        &runtime.view_landmarks_visible, &baseline.view_landmarks_visible,
        ex_view.and_then(|v| v.landmarks_visible.as_ref()),
        mk_view.and_then(|v| v.landmarks_visible).is_some(),
    );
```

   The `any_view` disjunction gains `|| landmarks_visible.is_some()` (add it to the line
   carrying `wrap_guide.is_some()`), and the `OView` struct literal passed to `some_if`
   gains `landmarks_visible` (field-init shorthand, after `wrap_guide`).

6. `every_persisted_setting_has_a_command` — the `field_guard` destructure gains
   `view_landmarks_visible: _,` (beside `view_wrap_guide: _`), and the assertion list
   gains, after the `toggle_wrap_guide` line:

```rust
        assert!(has("toggle_landmarks"), "view_landmarks_visible");
```

   **Green-on-arrival pin (honest label):** this assertion passes immediately because
   Task 2 registered the command — it is the LAW-2 RECURRENCE GUARD, not a red-first
   test. Its red form was demonstrated structurally: without this task's snapshot field
   the destructure would not compile, and without Task 2 the assert would fail.

7. **The three compiler-forced `SettingsSnapshot` literals** (the compiler finds them as
   E0063 after edit 1; fix all three in this task so the commit compiles):
   - `settings.rs` tests' `snap()` helper: add `view_landmarks_visible: true,` (beside
     `view_wrap_guide: false`).
   - `config.rs` test `save_reload_roundtrip_restores_settings`, the `runtime` literal:
     add `view_landmarks_visible: true,` (default — deliberately NOT part of that test's
     divergence set).
   - `e2e.rs`, the settings-save `baseline` literal (in the journey that dispatches
     `save_settings`): add `view_landmarks_visible: true,` (aligned with its neighbors'
     column style).

**Verify:** `cargo test -p wordcartel settings` green; `cargo test -p wordcartel config`
green; `cargo test -p wordcartel e2e` green (the baseline literal compiles);
`cargo clippy -p wordcartel --all-targets` clean.

**Commit:** `b18 T3: view_landmarks_visible settings persistence + law-2 guard` + trailers.

---

### Task 4: The gate — `block_paint::gather` early-return + behavior tests

**RED (three tests, two files).**

1. In `wordcartel/src/block_paint.rs`'s `mod tests`, after
   `gather_filters_hidden_and_sorts_marks`:

```rust
    // --- B18: the global visibility gate ---
    #[test]
    fn gather_gated_off_returns_the_empty_snapshot() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 5, hidden: false });
        e.active_mut().pending_block_begin = Some(7);
        e.active_mut().marks.insert('a', 2);
        e.view_opts.landmarks_visible = false;
        let p = gather(&e);
        assert!(p.block.is_none() && p.pending.is_none() && p.landmarks.is_empty(),
            "OFF: gather returns the empty snapshot");
        assert!(!p.wants_placed(), "OFF: landmarks never force the placed path");
    }
    #[test]
    fn gather_global_gate_composes_with_per_block_hidden() {
        // ON + per-block hidden → filtered (④ behavior — green-on-arrival pin).
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 5, hidden: true });
        assert!(gather(&e).block.is_none(), "per-block hidden filtered when globally ON");
        // OFF + NOT hidden → still empty (the global gate is outermost — the RED half).
        e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 5, hidden: false });
        e.view_opts.landmarks_visible = false;
        assert!(!gather(&e).wants_placed(), "global OFF hides a visible block");
    }
```

2. In `wordcartel/src/e2e.rs`, after `journey_landmarks_visible_and_clearable` (reusing
   its exact dispatch-closure pattern):

```rust
/// B18 regression gate: toggling landmarks OFF suppresses ALL in-text landmark paint
/// (pending anchor cell, block boundaries, mark cells) while the status segments
/// survive (`BLK…` mid-mark — Fork 5; `· BLK` + `MK <ids>` — Fork 1); toggling back
/// ON repaints. Real commands throughout.
#[test]
fn journey_toggle_landmarks_hides_paint_keeps_status() {
    use crate::registry::{Ctx, CommandId};
    use ratatui::style::Modifier;
    let text = "alpha beta gamma\n";
    let mut h = Harness::new(text, None, (80, 24));
    let dispatch = |h: &mut Harness, id: &'static str| {
        let mut e = h.editor.borrow_mut();
        let clock = TestClock(h.now);
        let mut ctx = Ctx { editor: &mut e, clock: &clock, executor: &h.ex,
                            msg_tx: h.tx.clone(), fs: crate::test_support::test_fs() };
        h.reg.dispatch(CommandId(id), &mut ctx);
    };
    // Landmarks ON (default): a bookmark at 'b' of beta + a pending ^KB at 0 both paint.
    h.editor.borrow_mut().active_mut().document.selection =
        wordcartel_core::selection::Selection::single(6);
    dispatch(&mut h, "set_bookmark_1");
    h.editor.borrow_mut().active_mut().document.selection =
        wordcartel_core::selection::Selection::single(0);
    dispatch(&mut h, "block_begin");
    h.render();
    assert!(h.cell_modifiers(6, 0).contains(Modifier::ITALIC), "ON: bookmark cell italic");
    assert!(h.cell_modifiers(0, 0).contains(Modifier::BOLD), "ON: pending anchor bold");
    // OFF: the cells vanish; the status keeps narrating.
    dispatch(&mut h, "toggle_landmarks");
    h.render();
    assert!(!h.cell_modifiers(6, 0).contains(Modifier::ITALIC), "OFF: mark cell unpainted");
    assert!(!h.cell_modifiers(0, 0).contains(Modifier::BOLD), "OFF: pending cell unpainted");
    assert!(h.screen_contains("BLK…"), "OFF: pending status survives (Fork 5)");
    // Complete the block while OFF: the model is untouched; still nothing paints.
    h.editor.borrow_mut().active_mut().document.selection =
        wordcartel_core::selection::Selection::single(5);
    dispatch(&mut h, "block_end");
    h.render();
    assert!(!h.cell_modifiers(0, 0).contains(Modifier::REVERSED), "OFF: no begin boundary");
    assert!(!h.cell_modifiers(4, 0).contains(Modifier::REVERSED), "OFF: no end boundary");
    assert!(h.screen_contains("· BLK"), "OFF: block status survives (Fork 1)");
    assert!(h.screen_contains("MK 1"), "OFF: caret-line mark identity survives");
    // Back ON: the completed block and the mark repaint.
    dispatch(&mut h, "toggle_landmarks");
    h.render();
    assert!(h.cell_modifiers(0, 0).contains(Modifier::REVERSED), "ON again: begin boundary");
    assert!(h.cell_modifiers(6, 0).contains(Modifier::ITALIC), "ON again: mark repainted");
}
```

Baseline state: Tasks 1-3 merged — the field and command exist, `gather` ignores the
field. All three tests COMPILE and FAIL at runtime: `gather_gated_off…` fails its first
assert (gather still returns the block/pending/marks); the composition test fails its
second half; the journey fails at `"OFF: mark cell unpainted"` (the cell is still
italic). Genuine red — run `cargo test -p wordcartel block_paint e2e` and confirm the
three failures before implementing.

**GREEN.** In `wordcartel/src/block_paint.rs::gather`, the guard as the FIRST statement
(the existing body is untouched below it; update the fn doc-comment):

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

**Verify:** `cargo test -p wordcartel block_paint` green (the two new + all ④ tests
unmodified); `cargo test -p wordcartel e2e` green (the new journey + `journey_landmarks_
visible_and_clearable` untouched-green — default ON); `cargo test -p wordcartel render`
green (the ④ paint suite untouched); `cargo clippy -p wordcartel --all-targets` clean.

**Commit:** `b18 T4: gate block_paint::gather on landmarks_visible — in-text paint off, status survives` + trailers.

---

### Task 5: Final verification (no merge)

Run and record (no code changes expected; any failure → fix or escalate to the
controller, never merge red):

1. `cargo test --workspace` — all suites green (core lib + oracle, shell lib, e2e,
   backlog drift gate, module_budgets — all untouched hubs).
2. `cargo build -p wordcartel -p wordcartel-core` and `cargo test --no-run --workspace`
   — warning-free.
3. `cargo clippy --workspace --all-targets` — clean.
4. `scripts/smoke/run.sh` — quote the one-line summary VERBATIM in the pre-merge report
   (advisory, never blocking; a red result is surfaced to the human as
   `smoke: FAIL sN — advisory`).
5. Confirm zero production diffs in `render.rs`, `render_status.rs`, `keymap.rs`,
   `blocks_marked.rs`, `marks.rs` (`git diff main --stat`).

No commit (verification only). Backlog bookkeeping (B18 → shipped, prose to
`docs/backlog-archive.md`, `scripts/backlog bless`) happens AT MERGE per spec §9 — the
controller drives it, not this plan.

## History

- 2026-07-24 — drafted for the Codex plan gate (Fable warm thread, effort B18), after the
  spec's two Minor gate folds.
