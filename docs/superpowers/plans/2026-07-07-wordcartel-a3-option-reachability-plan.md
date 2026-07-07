# A3 — Option Reachability + Preset-Aware Hints — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Every user-settable chrome/view option is reachable as an individual command (palette + plugin), profile and command share one setter per option, and the keybinding-hint plumbing is corrected + locked with tests.

**Architecture:** Shell-only (`wordcartel/src`). Add `Editor` setter methods that both the new commands and `density::apply_bundle` call (law 6); add set-per-state + representative commands for scrollbar/status_line/menu_bar (shape rule 8); give `KeyTrie` binding provenance so `chord_for` prefers a user's config binding (law 7); land the three law tests.

**Source spec:** `docs/superpowers/specs/2026-07-07-wordcartel-a3-option-reachability-design.md` (Codex spec gate READY, round 3).

## Command-surface-contract conformance

This effort **implements** `docs/design/command-surface-contract.md`: fixes law-2 (adds commands for `scrollbar`/`status_line`), law-6 (shared setters), law-7 (chord_for explicit-binding preference), law-3 (palette-completeness test); shape rule 8 (set-per-state + stateful menu representative); rule 10 (nullary, no arg model). Out of scope: A3b (placement sweep), parameterized commands (Effort P).

## Global Constraints

- **Existing option commands + the profile keep today's behavior, with ONE intentional fix.** `menu_bar_pin` and `toggle_chrome` are behavior-identical (now routed through the shared setters). `apply_bundle` is behavior-preserving for its 6 owned fields + the 9 dwell-clears AND now additionally keeps `menu_bar_unpinned_mode` consistent — an **intentional** change (spec-accepted; previously `apply_bundle` set `menu_bar_mode` directly and left `menu_bar_unpinned_mode` stale). The density tests stay green (they assert the owned fields + `menu_bar_revealed`, not `unpinned_mode`).
- **Status has no true Off** — `set_status_line_mode(Off)` coerces to `Auto`.
- **Nullary commands** (rule 10); no argument model.
- **House style / GATEs:** `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean; build/`--no-run` warning-free; no `cargo fmt`; em-dash prose comments; exhaustive matches on `TransientMode`/`MenuBarMode`; smoke mandatory-run. Doc-comment new public items.

**Task order:** 1 (setters + refactor) → 2 (commands) → 3 (chord_for provenance) → 4 (law tests).

---

## Task 1: Shared setters (`Editor` methods) + refactor `apply_bundle` & `menu_bar_pin`

**Files:** Modify `wordcartel/src/editor.rs` (add methods), `wordcartel/src/density.rs` (`apply_bundle` :61-79), `wordcartel/src/registry.rs` (`menu_bar_pin` :459-478). Test: inline in `editor.rs`.

**Interfaces produced:** `Editor::set_scrollbar_mode(TransientMode)`, `set_status_line_mode(TransientMode)`, `set_menu_bar_mode(MenuBarMode)` — each sets the field + clears its dwell state; `set_menu_bar_mode` also keeps `menu_bar_unpinned_mode` consistent and clears menu dwell.

- [ ] **Step 1: Failing tests.**
```rust
#[test]
fn setters_set_field_and_clear_dwell() {
    use crate::config::{TransientMode, MenuBarMode};
    let mut e = Editor::new_from_text("x\n", None, (40, 8));
    e.mouse.scrollbar_revealed = true; e.mouse.scrollbar_reveal_due = Some(9);
    e.set_scrollbar_mode(TransientMode::On);
    assert_eq!(e.scrollbar_mode, TransientMode::On);
    assert!(!e.mouse.scrollbar_revealed && e.mouse.scrollbar_reveal_due.is_none());
    // status: Off coerces to Auto (no true Off) + status dwell cleared
    e.mouse.status_revealed = true; e.mouse.status_hide_due = Some(7);
    e.set_status_line_mode(TransientMode::Off);
    assert_eq!(e.status_line_mode, TransientMode::Auto);
    assert!(!e.mouse.status_revealed && e.mouse.status_hide_due.is_none());
    // menu: dwell cleared
    e.mouse.menu_bar_revealed = true; e.mouse.menu_reveal_due = Some(3);
    e.set_menu_bar_mode(MenuBarMode::Auto);
    assert!(!e.mouse.menu_bar_revealed && e.mouse.menu_reveal_due.is_none());
}

#[test]
fn set_menu_bar_mode_keeps_unpinned_mode_consistent() {
    use crate::config::MenuBarMode;
    let mut e = Editor::new_from_text("x\n", None, (40, 8));
    e.set_menu_bar_mode(MenuBarMode::Auto);
    assert_eq!(e.menu_bar_mode, MenuBarMode::Auto);
    assert_eq!(e.menu_bar_unpinned_mode, MenuBarMode::Auto, "non-Pinned set → remembered");
    e.set_menu_bar_mode(MenuBarMode::Pinned);
    assert_eq!(e.menu_bar_unpinned_mode, MenuBarMode::Auto, "Pinned set → remembers prior non-Pinned");
    e.set_menu_bar_mode(MenuBarMode::Hidden);
    assert_eq!(e.menu_bar_unpinned_mode, MenuBarMode::Hidden);
}

#[test]
fn apply_bundle_keeps_menu_bar_unpinned_mode_consistent() {
    // INTENTIONAL change (spec-accepted): apply_bundle now routes menu_bar through
    // set_menu_bar_mode, so FULL (Pinned) remembers the prior non-Pinned mode as the unpin
    // target; previously apply_bundle left menu_bar_unpinned_mode stale.
    use crate::config::MenuBarMode;
    let mut e = Editor::new_from_text("x\n", None, (40, 8));
    e.set_menu_bar_mode(MenuBarMode::Hidden); // prior non-Pinned mode = Hidden
    crate::density::apply_bundle(&mut e, &crate::density::FULL); // FULL sets Pinned
    assert_eq!(e.menu_bar_mode, MenuBarMode::Pinned);
    assert_eq!(e.menu_bar_unpinned_mode, MenuBarMode::Hidden, "FULL remembers the prior mode as unpin target");
}
```

- [ ] **Step 2: Run — FAIL** (methods undefined).

- [ ] **Step 3: Implement.** In `editor.rs` (`impl Editor`):
```rust
    /// Set the scrollbar transient mode and clear its stale dwell state. The single
    /// setter both the `scrollbar_*` commands and `density::apply_bundle` call (contract law 6).
    pub fn set_scrollbar_mode(&mut self, mode: crate::config::TransientMode) {
        self.scrollbar_mode = mode;
        self.mouse.scrollbar_reveal_due = None;
        self.mouse.scrollbar_hide_due = None;
        self.mouse.scrollbar_revealed = false;
    }
    /// Set the status-line transient mode (Off coerces to Auto — status has no true Off,
    /// no-silent-UI) and clear its stale dwell state.
    pub fn set_status_line_mode(&mut self, mode: crate::config::TransientMode) {
        use crate::config::TransientMode;
        self.status_line_mode = if mode == TransientMode::Off { TransientMode::Auto } else { mode };
        self.mouse.status_reveal_due = None;
        self.mouse.status_hide_due = None;
        self.mouse.status_revealed = false;
    }
    /// Set the menu-bar mode, keeping `menu_bar_unpinned_mode` (the mode `menu_bar_pin`
    /// restores on unpin) consistent, and clear menu dwell state. Generalizes menu_bar_pin.
    pub fn set_menu_bar_mode(&mut self, mode: crate::config::MenuBarMode) {
        use crate::config::MenuBarMode;
        if mode == MenuBarMode::Pinned {
            if self.menu_bar_mode != MenuBarMode::Pinned { self.menu_bar_unpinned_mode = self.menu_bar_mode; }
        } else {
            self.menu_bar_unpinned_mode = mode;
        }
        self.menu_bar_mode = mode;
        self.mouse.menu_reveal_due = None;
        self.mouse.menu_hide_due = None;
        self.mouse.menu_bar_revealed = false;
    }
```
Refactor `apply_bundle` (density.rs :61-79) — route the three modes through the setters (keeps all 9 dwell-clears + adds unpinned-mode consistency):
```rust
pub fn apply_bundle(editor: &mut Editor, bundle: &ChromeBundle) {
    editor.chrome_disposition = bundle.chrome_disposition;
    editor.set_menu_bar_mode(bundle.menu_bar);
    editor.set_status_line_mode(bundle.status_line);
    editor.set_scrollbar_mode(bundle.scrollbar);
    editor.view_opts.measure = bundle.measure;
    editor.view_opts.word_count = bundle.word_count;
}
```
Refactor `menu_bar_pin` (registry.rs :459-478) handler body to call the setter (behavior-identical):
```rust
            |c| {
                use crate::config::MenuBarMode;
                let target = if c.editor.menu_bar_mode == MenuBarMode::Pinned {
                    c.editor.menu_bar_unpinned_mode
                } else { MenuBarMode::Pinned };
                c.editor.set_menu_bar_mode(target);
                CommandResult::Handled
            });
```
(The `menu_bar_pin` `register_stateful` state-fn stays; only the handler body changes.)

- [ ] **Step 4: Run — PASS.** `cargo test -p wordcartel --lib` green; the density tests (`apply_bundle_sets_every_owned_field_and_clears_menu_dwell`) + `menu_bar_pin` tests still pass.

- [ ] **Step 5: Commit** — `feat(editor): shared option setters; route apply_bundle + menu_bar_pin through them`.

---

## Task 2: Option-reachability commands (set-per-state + representative)

**Files:** Modify `wordcartel/src/registry.rs` (register the new commands near the other View/Settings commands). Test: inline in `registry.rs`.

**Interfaces:** new command ids: `scrollbar_off`/`scrollbar_auto`/`scrollbar_on`/`cycle_scrollbar`; `status_line_auto`/`status_line_on`/`toggle_status_line`; `menu_bar_hidden`/`menu_bar_auto`/`menu_bar_pinned`. All call the Task-1 setters.

- [ ] **Step 1: Failing tests.**
```rust
#[test]
fn scrollbar_commands_set_and_cycle() {
    use crate::config::TransientMode;
    let reg = Registry::builtins();
    let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
    dispatch_id(&mut ed, "scrollbar_off"); assert_eq!(ed.scrollbar_mode, TransientMode::Off);
    dispatch_id(&mut ed, "cycle_scrollbar"); assert_eq!(ed.scrollbar_mode, TransientMode::Auto); // Off→Auto
    dispatch_id(&mut ed, "cycle_scrollbar"); assert_eq!(ed.scrollbar_mode, TransientMode::On);   // Auto→On
    // palette-only: the set commands are not in the menu
    assert_eq!(reg.meta(CommandId("scrollbar_off")).unwrap().menu, None);
    // the representative is a View menu command with state-in-label
    assert_eq!(reg.meta(CommandId("cycle_scrollbar")).unwrap().menu, Some(MenuCategory::View));
}

#[test]
fn status_line_toggle_and_menu_bar_sets() {
    use crate::config::{TransientMode, MenuBarMode};
    let reg = Registry::builtins();
    let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
    ed.set_status_line_mode(TransientMode::Auto);
    dispatch_id(&mut ed, "toggle_status_line"); assert_eq!(ed.status_line_mode, TransientMode::On);
    dispatch_id(&mut ed, "toggle_status_line"); assert_eq!(ed.status_line_mode, TransientMode::Auto);
    dispatch_id(&mut ed, "menu_bar_hidden"); assert_eq!(ed.menu_bar_mode, MenuBarMode::Hidden);
    assert_eq!(reg.meta(CommandId("menu_bar_hidden")).unwrap().menu, None);
}
```
(`dispatch_id` is the existing registry-test helper — verify its name/shape, used by `toggle_chrome_flips_and_requests_rederive`.)

- [ ] **Step 2: Run — FAIL** (commands unregistered).

- [ ] **Step 3: Implement.** Register near the View/Settings commands (registry.rs ~400-520):
```rust
        // Scrollbar — set-per-state (palette-only) + a 3-state cycle representative (View, state-in-label).
        use crate::config::TransientMode;
        r.register("scrollbar_off",  "Scrollbar: Off",  None, |c| { c.editor.set_scrollbar_mode(TransientMode::Off);  CommandResult::Handled });
        r.register("scrollbar_auto", "Scrollbar: Auto", None, |c| { c.editor.set_scrollbar_mode(TransientMode::Auto); CommandResult::Handled });
        r.register("scrollbar_on",   "Scrollbar: On",   None, |c| { c.editor.set_scrollbar_mode(TransientMode::On);   CommandResult::Handled });
        r.register_stateful("cycle_scrollbar", "Scrollbar", Some(MenuCategory::View),
            |e| MenuMark::Value(match e.scrollbar_mode {
                TransientMode::Off => "Off", TransientMode::Auto => "Auto", TransientMode::On => "On" }),
            |c| { let next = match c.editor.scrollbar_mode {
                    TransientMode::Off => TransientMode::Auto, TransientMode::Auto => TransientMode::On,
                    TransientMode::On => TransientMode::Off };
                  c.editor.set_scrollbar_mode(next); CommandResult::Handled });

        // Status line — set-per-state (palette-only) + a 2-state toggle representative.
        r.register("status_line_auto", "Status Line: Auto", None, |c| { c.editor.set_status_line_mode(TransientMode::Auto); CommandResult::Handled });
        r.register("status_line_on",   "Status Line: On",   None, |c| { c.editor.set_status_line_mode(TransientMode::On);   CommandResult::Handled });
        r.register_stateful("toggle_status_line", "Status Line", Some(MenuCategory::View),
            |e| MenuMark::Value(match e.status_line_mode { TransientMode::On => "On", _ => "Auto" }),
            |c| { let next = if c.editor.status_line_mode == TransientMode::On { TransientMode::Auto } else { TransientMode::On };
                  c.editor.set_status_line_mode(next); CommandResult::Handled });

        // Menu bar — deterministic set-per-state (palette-only). menu_bar_pin stays the menu representative.
        use crate::config::MenuBarMode;
        r.register("menu_bar_hidden", "Menu Bar: Hidden", None, |c| { c.editor.set_menu_bar_mode(MenuBarMode::Hidden); CommandResult::Handled });
        r.register("menu_bar_auto",   "Menu Bar: Auto",   None, |c| { c.editor.set_menu_bar_mode(MenuBarMode::Auto);   CommandResult::Handled });
        r.register("menu_bar_pinned", "Menu Bar: Pinned", None, |c| { c.editor.set_menu_bar_mode(MenuBarMode::Pinned); CommandResult::Handled });
```
(`MenuMark`/`register_stateful` per registry.rs:44-83; `menu_leaf_label` composes "Scrollbar: Auto" etc. from the base + Value — the "Toggle "-strip/`:`-split rule handles these labels. Verify the `menu_leaf_label` output for label "Scrollbar" + Value("Auto") = "Scrollbar: Auto".)

- [ ] **Step 4: Run — PASS.** Full `cargo test -p wordcartel --lib` green. Update the Settings/View menu-list assertion at registry.rs:801 area if it enumerates the View menu commands (the new `cycle_scrollbar`/`toggle_status_line` join the View menu; the `*_off/auto/on/hidden/pinned` sets are `menu: None`).

- [ ] **Step 5: Commit** — `feat(registry): scrollbar/status_line/menu_bar option-reachability commands`.

---

## Task 3: `chord_for` provenance + prefer-explicit (law 7)

**Files:** Modify `wordcartel/src/keymap.rs` (`KeyTrie` :162, `bind`/`unbind` :168-175, `chord_for` :177+, `apply_patch_tables` :520). Test: inline in `keymap.rs`.

**Interfaces:** `KeyTrie` gains `user_bound: HashSet<Vec<KeyChord>>`; `bind_user(seq, id)`; `chord_for` prefers a user-bound chord.

- [ ] **Step 1: Failing test.**
```rust
#[test]
fn chord_for_prefers_user_bound_over_shortest_default() {
    let reg = crate::registry::Registry::builtins();
    // cut has the default CUA binding ctrl-x; add a LONGER custom binding via a patch.
    let patch = crate::config::KeymapPatch {
        bind: [("ctrl-alt-c".to_string(), "cut".to_string())].into_iter().collect(),
        unbind: vec![], cua: None, wordstar: None };
    let cfg = crate::config::KeymapConfig { preset: "cua".into(), patches: vec![patch] };
    let (km, warns) = build_keymap(&cfg, &reg);
    assert!(warns.is_empty(), "{warns:?}");
    // Without the fix chord_for returns the shorter ctrl-x; with it, the user's ctrl-alt-c.
    assert_eq!(km.chord_for(crate::registry::CommandId("cut")).as_deref(), Some("ctrl-alt-c"));
    // No custom binding → unchanged (shortest default).
    let (base, _) = build_keymap(&crate::config::KeymapConfig::default(), &reg);
    assert_eq!(base.chord_for(crate::registry::CommandId("cut")).as_deref(), Some("ctrl-x"));
}
```

- [ ] **Step 2: Run — FAIL** (chord_for returns ctrl-x, the shorter default).

- [ ] **Step 3: Implement.** `KeyTrie` (keymap.rs:162):
```rust
#[derive(Debug, Clone, Default)]
pub struct KeyTrie {
    map: HashMap<Vec<KeyChord>, CommandId>,
    /// Sequences bound by a config patch (user-explicit), as opposed to the preset base.
    /// `chord_for` prefers these so a user's binding wins over an inherited default (contract law 7).
    user_bound: std::collections::HashSet<Vec<KeyChord>>,
}
```
Add `bind_user`, and make `unbind` clear both:
```rust
    /// Bind `seq` to `id` AND mark it user-explicit (a config patch binding).
    pub fn bind_user(&mut self, seq: Vec<KeyChord>, id: CommandId) {
        self.user_bound.insert(seq.clone());
        self.map.insert(seq, id);
    }
    pub fn unbind(&mut self, seq: &[KeyChord]) {
        self.map.remove(seq);
        self.user_bound.remove(seq);
    }
```
`apply_patch_tables` (keymap.rs:520): change `trie.bind(seq, id)` → `trie.bind_user(seq, id)`. (The preset base load in `build_keymap` step 1 keeps using `trie.bind` — those are defaults, not user-explicit.)
`chord_for` (keymap.rs:177): prefer user-bound:
```rust
    pub fn chord_for(&self, id: CommandId) -> Option<String> {
        let all: Vec<&Vec<KeyChord>> = self.map.iter().filter(|(_, v)| **v == id).map(|(s, _)| s).collect();
        let user: Vec<&Vec<KeyChord>> = all.iter().copied().filter(|s| self.user_bound.contains(*s)).collect();
        let pool = if user.is_empty() { all } else { user };
        pool.into_iter()
            .map(|seq| chords_display(seq))
            .min_by(|a, b| a.chars().count().cmp(&b.chars().count()).then_with(|| a.cmp(b)))
    }
```

- [ ] **Step 4: Run — PASS.** Full `cargo test -p wordcartel --lib` green; the existing `chord_for_returns_shortest_and_blank_when_unbound` test still passes (no user binds → unchanged).

- [ ] **Step 5: Commit** — `feat(keymap): chord_for prefers a user's config binding over the shortest default`.

---

## Task 4: The three law tests (the contract as regression nets)

**Files:** Add tests in `wordcartel/src/settings.rs` (recurrence guard + SettingsSnapshot doc-comment), `wordcartel/src/palette.rs` (palette-completeness), `wordcartel/src/keymap.rs` or `menu.rs` (hints re-resolution + custom-bind surfaces).

- [ ] **Step 1 + 3 (these are pure tests — write them, they pass on the code from Tasks 1-3).**

Recurrence guard (settings.rs) + doc-comment on `SettingsSnapshot`:
```rust
/// LAW 2 (command-surface contract): every persisted field here MUST be changeable via a
/// command / command-surface. Adding a field REQUIRES adding a line to
/// `every_persisted_setting_has_a_command`.
```
```rust
#[test]
fn every_persisted_setting_has_a_command() {
    let reg = crate::registry::Registry::builtins();
    let has = |id: &str| reg.meta(crate::registry::CommandId(id)).is_some();
    // SettingsSnapshot field → a command / command-surface that changes it:
    assert!(has("keymap_next"), "keymap_preset");
    assert!(has("theme"), "theme_identity (picker surface)");
    assert!(has("toggle_typewriter"), "view_typewriter");
    assert!(has("toggle_focus"), "view_focus");
    assert!(has("toggle_measure"), "view_measure");
    assert!(has("toggle_wrap_guide"), "view_wrap_guide");
    assert!(has("toggle_word_count"), "view_word_count");
    assert!(has("set_wrap_column"), "view_wrap_column");
    assert!(has("cycle_scrollbar") && has("scrollbar_auto"), "view_scrollbar");
    assert!(has("toggle_status_line") && has("status_line_auto"), "view_status_line");
    assert!(has("menu_bar_pin") && has("menu_bar_auto"), "menu_bar");
    assert!(has("toggle_mouse_capture"), "mouse_capture");
    assert!(has("toggle_chrome"), "chrome_disposition");
    assert!(has("toggle_canvas"), "canvas");
}
```
Palette-completeness (palette.rs — formalize the near-miss at :138):
```rust
#[test]
fn palette_is_exhaustive_over_the_registry() {
    let reg = crate::registry::Registry::builtins();
    let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
    let mut p = Palette::default();
    rebuild_rows(&mut p, &reg, &km);
    let ids: std::collections::HashSet<_> = p.rows.iter().map(|r| r.id).collect();
    for (id, _) in reg.commands() {
        assert!(ids.contains(&id), "palette missing registered command {}", id.0);
    }
    assert_eq!(p.rows.len(), reg.commands().count(), "row count == registry command count");
}
```
Hints re-resolution + custom-bind surfaces (keymap.rs for re-resolution; menu.rs/palette.rs for surfacing):
```rust
#[test]
fn hints_reresolve_on_preset_switch() {
    let reg = crate::registry::Registry::builtins();
    let cfg = |p: &str| crate::config::KeymapConfig { preset: p.into(), patches: vec![] };
    let (cua, _) = build_keymap(&cfg("cua"), &reg);
    let (ws, _)  = build_keymap(&cfg("wordstar"), &reg);
    // save: CUA = `ctrl-s` (keymap.rs:249, shortest); WordStar binds save ONLY under ctrl-k combos
    // (`ctrl-k s` / `ctrl-k ctrl-s` / …, keymap.rs:376 — all longer), and `ctrl-s` is move_left in
    // WordStar. So the two presets' save hints genuinely differ.
    // (Do NOT use move_up: WordStar binds BOTH `ctrl-e` AND `up` to it, so chord_for returns "up"
    //  for both presets — a vacuous assert_ne.)
    assert_ne!(cua.chord_for(crate::registry::CommandId("save")),
               ws.chord_for(crate::registry::CommandId("save")));
}

#[test]
fn custom_bind_surfaces_in_menu_and_palette() {
    let reg = crate::registry::Registry::builtins();
    let patch = crate::config::KeymapPatch {
        bind: [("ctrl-alt-c".to_string(), "cut".to_string())].into_iter().collect(),
        unbind: vec![], cua: None, wordstar: None };
    let (km, _) = crate::keymap::build_keymap(
        &crate::config::KeymapConfig { preset: "cua".into(), patches: vec![patch] }, &reg);
    let ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
    let groups = crate::menu::build(&reg, &km, &ed).groups; // menu bakes chord_for into the leaf label
    assert!(groups.iter().any(|(_, ls)| ls.iter().any(|(label, id)|
        *id == crate::registry::CommandId("cut") && label.contains("ctrl-alt-c"))), "menu hint");
    let mut p = Palette::default();
    crate::palette::rebuild_rows(&mut p, &reg, &km);
    assert!(p.rows.iter().any(|r| r.id == crate::registry::CommandId("cut") && r.chord == "ctrl-alt-c"),
        "palette hint");
}
```
(The re-resolution test uses `save` — verified preset-differing: CUA `ctrl-s` (keymap.rs:249) vs WordStar's `ctrl-k` combos (keymap.rs:376). Do NOT use `move_up`: WordStar binds both `ctrl-e` and `up` to it, so `chord_for` returns `up` for both presets — vacuous.)

- [ ] **Step 4: Run — PASS.** Full `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean; build/`--no-run` warning-free; run `scripts/smoke/run.sh` and quote the summary.

- [ ] **Step 5: Commit** — `test(a3): law tests — recurrence guard, palette-completeness, preset-aware hints`.

---

## Testing & gates (whole-effort)

- Per-task TDD tests + the three law tests. Cross-cutting: the existing density/`toggle_chrome`/`menu_bar_pin` tests stay green (their asserted behavior is unchanged), and the ONE intentional `apply_bundle` change — routing menu_bar through `set_menu_bar_mode` now keeps `menu_bar_unpinned_mode` consistent — is covered by the new `apply_bundle_keeps_menu_bar_unpinned_mode_consistent` test.
- GATEs: `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean; build/`--no-run` warning-free; smoke mandatory-run.
- Pipeline gates: Codex plan review (loop clean) → subagent execution → Codex pre-merge + Fable whole-branch → merge.

## Notes for the executor

- Verify `dispatch_id` (registry test helper), `menu_leaf_label` composition ("Scrollbar" + Value("Auto") → "Scrollbar: Auto"), and the exact `KeymapConfig`/`KeymapPatch` field names (`bind: BTreeMap<String,String>`, `unbind: Vec<String>`, `cua`/`wordstar: Option<ScopedPatch>`) against the real source before writing the tests.
- If any View menu-list assertion (registry.rs ~801) enumerates View commands, add `cycle_scrollbar`/`toggle_status_line`.
- `hints_reresolve_on_preset_switch` uses `save` (verified preset-differing: CUA `ctrl-s` at keymap.rs:249 vs WordStar's `ctrl-k` combos at keymap.rs:376; `move_up` is NOT usable — WordStar binds both `ctrl-e` and `up` to it).
