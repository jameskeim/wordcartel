# D1+A5 Settings Write-back + Keymap Switching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** runtime keymap switching (cua ⇄ wordstar) with preset-scoped patches, and an explicit Save Settings command persisting runtime settings to a machine-owned overrides file under the contradiction-only-removal diff law.

**Architecture:** scoped patch fields on `KeymapPatch`/`RawKeymap` + a shared `resolve_preset`; two request flags on `Editor` honored by a between-reduces block in `run()` (loop-local trie reassignment; save owns the three snapshots); a new `settings` module holds the mirror serde structs, the diff law, and the seam-parameterized writer; provenance-typed theme identity threaded through the picker's single preview funnel. Four tasks: T1 schema+merge, T2 runtime switch, T3 settings core+picker, T4 run() wiring+journey.

**Tech Stack:** Rust; shell crate only (fsx/Fs seam is `pub(crate)` in wordcartel — same crate, no visibility changes); serde + toml 0.8 (add the `Serialize` import in the new module only); no new dependencies; no core changes.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-05-wordcartel-d1a5-settings-keymap-design.md` (CLEAN — Codex ×3 + Fable ×3; ratified: contradiction-only-removal diff law, rule-3 mask-guard with the provenance-typed theme predicate, preset-scoped patches, explicit-save-only, close_buffer unbound by design). Grounding data (verbatim HEAD anchors): `.superpowers/sdd/d1a5-grounding.md`.
- **Gates after EVERY commit:** `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean (deny gate LIVE); `cargo build` warning-free. NO `cargo fmt`; `—` em-dash prose comments; no emoji; hand-match neighbors; no catch-all `_` arms on `MenuCategory`/`ThemeIdentity` matches.
- Exact status copy (spec-pinned): `"settings saved"`, `"settings: {io error Display}"`, `"settings: disabled by --no-config"`, `"settings: no config directory"`, switch: `"keymap: wordstar"` / `"keymap: cua"`, idempotent: `"keymap: cua (already active)"`.
- The five persisted view toggles are typewriter/focus/measure/wrap_guide/word_count ONLY. `[mouse]` key is `capture`. `[menu] bar` round-trips as the lowercase string (MenuBarMode has NO serde derive).
- Line anchors are HEAD (`20e40e7`) references from the grounding; locate by quoted code after earlier tasks shift lines.
- Exclude Cargo.lock drift from commits (`git checkout -- Cargo.lock` if the repar path dep bumps).
- Every commit ends with the trailers, verbatim (use `git commit -F -` with a quoted 'EOF' heredoc — `!` breaks zsh inside double-quoted `-m`):
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: preset-scoped patches + resolve_preset (config.rs + keymap.rs)

**Files:**
- Modify: `wordcartel/src/config.rs` (KeymapPatch/ScopedPatch, RawKeymap/RawScoped, the patch push, tests)
- Modify: `wordcartel/src/keymap.rs` (resolve_preset, build_keymap scoped application, tests)

**Interfaces:**
- Produces: `pub struct ScopedPatch { pub bind: BTreeMap<String,String>, pub unbind: Vec<String> }`; `KeymapPatch` gains `pub cua: Option<ScopedPatch>, pub wordstar: Option<ScopedPatch>`; `pub fn resolve_preset(name: &str) -> &'static str` (keymap.rs). T2 consumes `resolve_preset`; T3/T4 consume nothing from T1 beyond unchanged signatures.

- [ ] **Step 1: failing merge tests** (config.rs + keymap.rs test modules; the `write`/`tempdir` helpers are at config.rs:429-433/:508-518 — NOT :455-465):

```rust
    // config.rs tests — scoped tables parse into the named fields
    #[test]
    fn scoped_keymap_tables_parse_into_named_fields() {
        let d = tempdir();
        let p = write(&d, "s.toml",
            "[keymap]\npreset='cua'\nbind={ \"ctrl-g\"='goto_line' }\n[keymap.cua]\nbind={ \"ctrl-w\"='close_buffer' }\n[keymap.wordstar]\nunbind=[\"ctrl-q ctrl-q\"]\n");
        let (cfg, warns) = load(&[p]);
        assert!(warns.is_empty());
        let patch = &cfg.keymap.patches[0];
        assert!(patch.bind.contains_key("ctrl-g"), "global bind unchanged");
        assert_eq!(patch.cua.as_ref().unwrap().bind.get("ctrl-w").unwrap(), "close_buffer");
        assert_eq!(patch.wordstar.as_ref().unwrap().unbind[0], "ctrl-q ctrl-q");
    }

    #[test]
    fn global_only_configs_leave_scoped_fields_none() {
        let d = tempdir();
        let p = write(&d, "g.toml", "[keymap]\nbind={ \"ctrl-g\"='goto_line' }\n");
        let (cfg, _) = load(&[p]);
        assert!(cfg.keymap.patches[0].cua.is_none() && cfg.keymap.patches[0].wordstar.is_none());
    }
```

```rust
    // keymap.rs tests — the merge law + resolve_preset
    #[test]
    fn resolve_preset_falls_back_to_cua() {
        assert_eq!(resolve_preset("wordstar"), "wordstar");
        assert_eq!(resolve_preset("cua"), "cua");
        assert_eq!(resolve_preset("dvorak"), "cua");
    }

    #[test]
    fn scoped_patch_applies_only_under_its_preset() {
        // cua-scoped rebind of ctrl-w: applies under cua, not under wordstar.
        let scoped = crate::config::ScopedPatch {
            bind: [("ctrl-w".to_string(), "close_buffer".to_string())].into(), unbind: vec![] };
        let mk = |preset: &str| crate::config::KeymapConfig {
            preset: preset.into(),
            patches: vec![crate::config::KeymapPatch { cua: Some(scoped.clone()), ..Default::default() }],
        };
        let reg = Registry::builtins();
        let (cua_trie, w1) = build_keymap(&mk("cua"), &reg);
        let (ws_trie, w2) = build_keymap(&mk("wordstar"), &reg);
        assert!(w1.is_empty() && w2.is_empty());
        let cw = parse_seq("ctrl-w").unwrap();
        assert!(matches!(cua_trie.resolve(&cw), Resolution::Command(CommandId("close_buffer"))));
        assert!(matches!(ws_trie.resolve(&cw), Resolution::Command(CommandId("scroll_line_up"))),
            "wordstar keeps its own ctrl-w — the scoped patch must not leak");
    }

    #[test]
    fn specific_wins_within_a_layer_and_later_layer_wins_across() {
        // Layer 1: global ctrl-g -> goto_line, cua-scoped ctrl-g -> copy (specific wins in layer 1).
        // Layer 2: global ctrl-g -> paste (later layer's GLOBAL beats earlier layer's SCOPED).
        let l1 = crate::config::KeymapPatch {
            bind: [("ctrl-g".to_string(), "goto_line".to_string())].into(),
            cua: Some(crate::config::ScopedPatch {
                bind: [("ctrl-g".to_string(), "copy".to_string())].into(), unbind: vec![] }),
            ..Default::default() };
        let l2 = crate::config::KeymapPatch {
            bind: [("ctrl-g".to_string(), "paste".to_string())].into(), ..Default::default() };
        let reg = Registry::builtins();
        let (one, _) = build_keymap(&crate::config::KeymapConfig {
            preset: "cua".into(), patches: vec![l1.clone()] }, &reg);
        let g = parse_seq("ctrl-g").unwrap();
        assert!(matches!(one.resolve(&g), Resolution::Command(CommandId("copy"))), "specific wins within the layer");
        let (two, _) = build_keymap(&crate::config::KeymapConfig {
            preset: "cua".into(), patches: vec![l1, l2] }, &reg);
        assert!(matches!(two.resolve(&g), Resolution::Command(CommandId("paste"))), "later layer wins outright");
    }

    #[test]
    fn scoped_tables_key_off_the_resolved_preset() {
        // preset="dvorak" resolves to cua → [keymap.cua] applies (spec M-5).
        let cfgk = crate::config::KeymapConfig {
            preset: "dvorak".into(),
            patches: vec![crate::config::KeymapPatch {
                cua: Some(crate::config::ScopedPatch {
                    bind: [("ctrl-w".to_string(), "close_buffer".to_string())].into(), unbind: vec![] }),
                ..Default::default() }],
        };
        let (t, warns) = build_keymap(&cfgk, &Registry::builtins());
        assert!(warns.iter().any(|w| w.contains("unknown keymap.preset")), "fallback still warns");
        assert!(matches!(t.resolve(&parse_seq("ctrl-w").unwrap()), Resolution::Command(CommandId("close_buffer"))));
    }
```

Run: `cargo test -p wordcartel -- scoped resolve_preset specific_wins` — FAIL to compile (fields/fn don't exist): the RED.

- [ ] **Step 2: config.rs schema.** Per grounding §B.1 exactly: add `ScopedPatch` (pub, `#[derive(Debug, Clone, Default)]`) below `KeymapPatch` (config.rs:144-148); add the two pub fields to `KeymapPatch`; add `RawScoped` (`#[derive(Debug, Default, Deserialize)] #[serde(default)]`, private, `bind`/`unbind`) below `RawKeymap` (config.rs:211-217); add `cua: Option<RawScoped>, wordstar: Option<RawScoped>` to `RawKeymap` — with the footgun doc-comment on the fields (the spec's config-docs sentence has no other home, Fable plan M1: `/// NOTE: once a [keymap.cua] header appears, [keymap]-intended keys typed below it silently belong to the scoped table (TOML section semantics).`); the patch push (config.rs:314-317) maps both via `.map(|s| ScopedPatch { bind: s.bind, unbind: s.unbind })`.

- [ ] **Step 3: keymap.rs.** Add beside `preset_bindings` (keymap.rs:205):

```rust
/// Resolve a raw preset string to a known base ("cua" | "wordstar"); unknown → "cua".
/// Shared by build_keymap (base + scoped selection) and run()'s seeding so the two
/// can never disagree about what an unknown preset fell back to.
pub fn resolve_preset(name: &str) -> &'static str {
    match name { "wordstar" => "wordstar", _ => "cua" }
}
```

In `build_keymap` (keymap.rs:424-489): keep the existing unknown-preset warning; base selection may stay as-is (the `None` arm already falls back). After each patch's global bind/unbind loops, apply the active scope with the SAME bind/unbind code shape (extract a small private `apply_patch_tables(trie, warns, reg, bind, unbind)` helper so the global and scoped applications share one implementation — no duplicated warn strings):

```rust
    // 2b) The active preset's scoped table, after the layer's global tables —
    // "later file wins; within a file, specific wins" (spec D1).
    let scoped = match resolve_preset(&km.preset) {
        "wordstar" => patch.wordstar.as_ref(),
        _ => patch.cua.as_ref(),
    };
    if let Some(s) = scoped {
        apply_patch_tables(&mut trie, &mut warns, reg, &s.bind, &s.unbind);
    }
```

Drop the stale `#[allow(dead_code)] // wired in Task 4/5` on `build_keymap` while editing it. **Migrate EVERY existing `KeymapPatch` struct literal (Codex plan C1 — the new fields break them all):** config.rs:314 (the push — gains the two mapped fields), keymap.rs:504 and :518-519 (the `km` helper + the cross-layer test), app.rs:3128/:3147/:3167 (three test literals — add `..Default::default()`). Grep `KeymapPatch {` across wordcartel/src to confirm no other site before committing.

- [ ] **Step 4: GREEN + gates.** Full two-crate suite; back-compat is proven by the untouched existing keymap/config tests passing verbatim.

- [ ] **Step 5: commit** — `feat(d1a5): preset-scoped keymap patches + resolve_preset — specific wins within a layer, later layer wins across`.

---

### Task 2: runtime keymap switch (A5)

**Files:**
- Modify: `wordcartel/src/editor.rs` (2 fields), `wordcartel/src/registry.rs` (MenuCategory::Settings + 2 commands + test), `wordcartel/src/menu.rs` (label arm), `wordcartel/src/app.rs` (seeding, `let mut keymap`, rebuild block, tests), `wordcartel/src/e2e.rs` (step flag check)

**Interfaces:**
- Consumes: T1's `resolve_preset`.
- Produces: `Editor.active_keymap_preset: String`, `Editor.keymap_rebuild: bool`; commands `keymap_cua`/`keymap_wordstar` (labels "Keymap: CUA"/"Keymap: WordStar", `MenuCategory::Settings`); `MenuCategory::Settings` in `MENU_ORDER` between View and Export. T3 registers `save_settings` into the same category; T4 wires the sibling save flag.

- [ ] **Step 1: failing tests** (app.rs test module, §C idioms — reduce-level with `cua_keymap()`, plus the manual rebuild seam since unit tests have no run loop):

```rust
    #[test]
    fn keymap_switch_command_sets_preset_and_rebuild_flag() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::{Registry, CommandId};
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
        reg.dispatch(CommandId("keymap_wordstar"), &mut ctx);
        assert_eq!(e.active_keymap_preset, "wordstar");
        assert!(e.keymap_rebuild, "switch requests a rebuild");
        assert_eq!(e.status, "keymap: wordstar");
    }

    #[test]
    fn keymap_switch_is_idempotent_with_status() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::{Registry, CommandId};
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
        reg.dispatch(CommandId("keymap_cua"), &mut ctx); // cua is already active
        assert!(!e.keymap_rebuild, "idempotent switch must not request a rebuild");
        assert_eq!(e.status, "keymap: cua (already active)");
    }

    #[test]
    fn switch_status_survives_the_rebuild() {
        // Fable plan C1: the rebuild must NOT wipe the switch status set in the same
        // reduce (no pending prefix in play → status untouched by the helper).
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = crate::registry::Registry::builtins();
        e.active_keymap_preset = "wordstar".into();
        e.keymap_rebuild = true;
        e.status = "keymap: wordstar".into();
        let t = crate::app::rebuild_keymap_if_requested(&mut e, &[], &reg);
        assert!(t.is_some());
        assert_eq!(e.status, "keymap: wordstar", "the pinned switch copy reaches the draw");
    }

    #[test]
    fn patches_survive_the_switch() {
        // Fable plan I3(c): a GLOBAL patch bind holds under both bases through the
        // real helper (the same patch slice run() passes from cfg.keymap.patches).
        use crate::keymap::{parse_seq, Resolution};
        let patches = vec![crate::config::KeymapPatch {
            bind: [("ctrl-g".to_string(), "copy".to_string())].into(),
            ..Default::default() }];
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = crate::registry::Registry::builtins();
        e.active_keymap_preset = "wordstar".into();
        e.keymap_rebuild = true;
        let t = crate::app::rebuild_keymap_if_requested(&mut e, &patches, &reg).unwrap();
        let g = parse_seq("ctrl-g").unwrap();
        assert!(matches!(t.resolve(&g), Resolution::Command(crate::registry::CommandId("copy"))),
            "the global patch rides onto the new base");
    }

    #[test]
    fn rebuild_seam_swaps_the_trie_and_clears_pending() {
        // Manual seam: seed pending_keys with ctrl-k (Pending under BOTH presets), set the
        // flag via dispatch, then run the same rebuild the loop runs; assert ctrl-w resolves
        // to scroll_line_up afterward and pending_keys is EMPTY (spec I-3).
        use crate::keymap::{build_keymap, parse_seq, Resolution};
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = crate::registry::Registry::builtins();
        e.pending_keys = parse_seq("ctrl-k").unwrap();
        e.status = "ctrl-k …".into();
        e.active_keymap_preset = "wordstar".into();
        e.keymap_rebuild = true;
        // The REAL production helper — the test proves run()'s code, not a copy (Fable I4).
        let mut keymap = cua_keymap();
        if let Some(t) = crate::app::rebuild_keymap_if_requested(&mut e, &[], &reg) {
            keymap = t;
        }
        assert!(e.pending_keys.is_empty(), "pending prefix must not survive the rebuild");
        assert!(e.status.is_empty(), "the pending '…' prompt is cleared");
        let cw = parse_seq("ctrl-w").unwrap();
        assert!(matches!(keymap.resolve(&cw), Resolution::Command(crate::registry::CommandId("scroll_line_up"))));
    }
```

Registry test: extend the existing meta-shape test (registry.rs:572-585 idiom) with a new `settings_commands_registered_in_settings_category` pin asserting `keymap_cua`/`keymap_wordstar` have `menu == Some(MenuCategory::Settings)` and labels "Keymap: CUA"/"Keymap: WordStar". Plus the C4-closure pin in keymap.rs:

```rust
    #[test]
    fn close_buffer_is_unbound_in_both_presets_by_design() {
        // C4 closure (spec D5): per-preset patches are the supported binding path.
        for preset in ["cua", "wordstar"] {
            for (_, id) in preset_bindings(preset).unwrap() {
                assert_ne!(*id, "close_buffer", "{preset} must not bind close_buffer");
            }
        }
    }
```

RED: fields/commands don't exist (the C4 pin is green from birth — it guards regression).

- [ ] **Step 2: Editor fields.** In the struct (near `pending_keys`/`keymap`, editor.rs:380-381): `pub active_keymap_preset: String,` + `pub keymap_rebuild: bool,`. Init in `new_from_text` (the SOLE constructor, editor.rs:438-481): `active_keymap_preset: "cua".into(), keymap_rebuild: false,`.

- [ ] **Step 3: registry + menu.** `MenuCategory` gains `Settings` (registry.rs:39); `MENU_ORDER` becomes `[MenuCategory; 6]` with Settings between View and Export (registry.rs:41-42); `category_label` gains `MenuCategory::Settings => "Settings",` (menu.rs:60-68 — the match is exhaustive, the compiler forces every site). Register (beside menu_bar_pin's neighborhood, before registry.rs:462):

```rust
        // Settings menu — runtime keymap preset switching (D1+A5).
        r.register("keymap_cua", "Keymap: CUA", Some(MenuCategory::Settings), |c| {
            switch_keymap_preset(c.editor, "cua");
            CommandResult::Handled
        });
        r.register("keymap_wordstar", "Keymap: WordStar", Some(MenuCategory::Settings), |c| {
            switch_keymap_preset(c.editor, "wordstar");
            CommandResult::Handled
        });
```

with one shared private fn in registry.rs (or editor.rs — implementer's judgment, match neighbors):

```rust
/// Request a keymap preset switch: no-op with a status when already active; else set the
/// preset and the rebuild flag — the run loop swaps the trie between reduces (spec D2).
fn switch_keymap_preset(editor: &mut crate::editor::Editor, preset: &str) {
    if editor.active_keymap_preset == preset {
        editor.status = format!("keymap: {preset} (already active)");
        return;
    }
    editor.active_keymap_preset = preset.to_string();
    editor.keymap_rebuild = true;
    editor.status = format!("keymap: {preset}");
}
```

- [ ] **Step 4: run() wiring.** Seeding block (app.rs:1244-1262) gains `editor.active_keymap_preset = keymap::resolve_preset(&cfg.keymap.preset).to_string();`. The loop-local becomes `let mut keymap = std::mem::take(&mut editor.keymap);` (app.rs:1334) and its stale comment drops the "doesn't change during the loop in v1" clause. Add the extracted helper (user-ratified A, Fable plan C1+I4 — ONE source of truth consumed by run(), the Harness, and the seam test; the unconditional status.clear() of the earlier draft wiped the switch status before the draw) as a pub(crate) fn in app.rs near `preview_selected_theme`:

```rust
/// Honor a requested keymap rebuild (spec D2). Returns the new trie for the caller to
/// swap into its loop-local. A half-typed prefix must not complete against the new base
/// (spec I-3): the buffer drops, and the status clears ONLY when it is the pending "…"
/// prompt — a switch status set in the same reduce must survive to the draw.
pub(crate) fn rebuild_keymap_if_requested(
    editor: &mut crate::editor::Editor,
    patches: &[crate::config::KeymapPatch],
    reg: &crate::registry::Registry,
) -> Option<crate::keymap::KeyTrie> {
    if !editor.keymap_rebuild { return None; }
    editor.keymap_rebuild = false;
    let (trie, kw) = crate::keymap::build_keymap(&crate::config::KeymapConfig {
        preset: editor.active_keymap_preset.clone(),
        patches: patches.to_vec(),
    }, reg);
    if !editor.pending_keys.is_empty() {
        editor.pending_keys.clear();
        if editor.status.ends_with('…') { editor.status.clear(); }
    }
    if let Some(w) = kw.first() { editor.status = w.clone(); }
    Some(trie)
}
```

Immediately after `let keep = reduce(...);` (app.rs:1473):

```rust
        if let Some(t) = rebuild_keymap_if_requested(&mut editor, &cfg.keymap.patches, &reg) {
            keymap = t;
        }
```

- [ ] **Step 5: e2e Harness.** In `step` (e2e.rs:50-58), after `reduce` returns and before `advance`:

```rust
        if let Some(t) = app::rebuild_keymap_if_requested(&mut self.editor, &[], &self.reg) {
            self.keymap = t;
        }
```

- [ ] **Step 6: GREEN + gates.** Also verify by hand-inspection that `hydrate_overlays`/menu/palette take the keymap parameter (grounding-verified — no `editor.keymap` reads in the loop) and note it in the report.

- [ ] **Step 7: commit** — `feat(d1a5): runtime keymap switching — Settings menu, rebuild between reduces, pending-prefix hygiene`.

---

### Task 3: settings core — mirror structs, diff law, writer, picker identity

**Files:**
- Create: `wordcartel/src/settings.rs`; register `pub mod settings;` in `wordcartel/src/lib.rs` (the pub-mod list at lib.rs:3-27)
- Modify: `wordcartel/src/editor.rs` (2 fields), `wordcartel/src/theme_picker.rs` (previewed field), `wordcartel/src/app.rs` (preview funnel + Enter/Esc arms), `wordcartel/src/registry.rs` (save_settings command + test extension)

**Interfaces:**
- Consumes: T2's `MenuCategory::Settings`; T1's `resolve_preset` (via callers).
- Produces (all in `settings.rs`, consumed by T4): `pub enum ThemeIdentity { File, Builtin(String) }` (derive `Debug, Clone, PartialEq, Eq`); `pub struct SettingsSnapshot { pub keymap_preset: String, pub theme_identity: ThemeIdentity, pub view_typewriter: bool, pub view_focus: bool, pub view_measure: bool, pub view_wrap_guide: bool, pub view_word_count: bool, pub menu_bar: crate::config::MenuBarMode, pub mouse_capture: bool }`; `pub struct OverridesFile` (serde mirror); `pub fn snapshot_of(cfg: &crate::config::Config, resolved_theme_name: &str) -> SettingsSnapshot`; `pub fn runtime_snapshot(editor: &crate::editor::Editor) -> SettingsSnapshot`; `pub fn theme_identity_of(theme_cfg: &crate::config::ThemeConfig, resolved_name: &str) -> ThemeIdentity`; `pub fn parse_overrides(bytes: &str) -> OverridesFile`; `pub fn compute_overrides(runtime: &SettingsSnapshot, baseline: &SettingsSnapshot, existing: &OverridesFile, mask: &OverridesFile) -> OverridesFile`; `pub(crate) fn save_overrides(fs: &dyn crate::fsx::Fs, path: &std::path::Path, of: &OverridesFile) -> std::io::Result<()>` — **pub(crate), NOT pub (Codex plan C2): `Fs`/`WriteSync`/`RealFs`/`atomic_replace` are all pub(crate) in fsx.rs (:15/:24/:39/:181), and a `pub` fn cannot expose a pub(crate) trait; `write_overrides` and the run()-facing save helper are pub(crate) for the same reason (the module itself stays `pub mod settings`)**; `Editor.settings_save_requested: bool` + `Editor.theme_identity: ThemeIdentity`.

- [ ] **Step 1: the module skeleton + failing unit tests.** `settings.rs` opens with `use serde::{Serialize, Deserialize};`. The mirror (grounding §B.5, sections wrapped `Option` so an all-empty file serializes to NOTHING — the header-only file):

```rust
/// The machine-owned overrides file (settings-overrides.toml): every field optional,
/// presence-sensitive — the diff law's rules 2/3 need "the layer HAS key K" exactly,
/// which config::load cannot answer (it folds defaults). Parsed and written ONLY here.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverridesFile {
    #[serde(skip_serializing_if = "Option::is_none")] pub keymap: Option<OKeymap>,
    #[serde(skip_serializing_if = "Option::is_none")] pub theme: Option<OTheme>,
    #[serde(skip_serializing_if = "Option::is_none")] pub view: Option<OView>,
    #[serde(skip_serializing_if = "Option::is_none")] pub menu: Option<OMenu>,
    #[serde(skip_serializing_if = "Option::is_none")] pub mouse: Option<OMouse>,
}
```

with `OKeymap { preset: Option<String> }`, `OTheme { name: Option<String> }`, `OView` (the five `Option<bool>`s), `OMenu { bar: Option<String> }`, `OMouse { capture: Option<bool> }` — each `#[derive(Debug, Default, PartialEq, Serialize, Deserialize)] #[serde(default)]` with per-field `skip_serializing_if`. Plus:

```rust
pub const OVERRIDES_HEADER: &str =
    "# managed by wcartel — edits may be overwritten by Save Settings\n";

/// "hidden"/"auto"/"pinned" — MenuBarMode has no serde derive; this mapping mirrors
/// load()'s string match (config.rs) and MUST stay in sync with it.
pub fn menu_bar_str(mode: crate::config::MenuBarMode) -> &'static str {
    match mode {
        crate::config::MenuBarMode::Hidden => "hidden",
        crate::config::MenuBarMode::Auto => "auto",
        crate::config::MenuBarMode::Pinned => "pinned",
    }
}
```

Failing tests (settings.rs `#[cfg(test)]`; corpus-driven, no run loop needed):

```rust
    fn snap(preset: &str, theme: ThemeIdentity, tw: bool) -> SettingsSnapshot {
        SettingsSnapshot { keymap_preset: preset.into(), theme_identity: theme,
            view_typewriter: tw, view_focus: false, view_measure: false,
            view_wrap_guide: false, view_word_count: false,
            menu_bar: crate::config::MenuBarMode::Auto, mouse_capture: true }
    }

    #[test]
    fn rule1_divergence_writes_and_rule4_absent_otherwise() {
        let rt = snap("wordstar", ThemeIdentity::Builtin("default".into()), true);
        let base = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        let of = compute_overrides(&rt, &base, &OverridesFile::default(), &OverridesFile::default());
        assert_eq!(of.keymap.as_ref().unwrap().preset.as_deref(), Some("wordstar"));
        assert_eq!(of.view.as_ref().unwrap().typewriter, Some(true));
        assert!(of.theme.is_none(), "un-diverged never-saved key stays absent");
        assert!(of.mouse.is_none() && of.menu.is_none());
    }

    #[test]
    fn rule2_keeps_coinciding_saved_key_across_baselines() {
        // The cross-project walkthrough: override typewriter=false; project-B baseline
        // ALSO false; runtime false → KEEP, not remove.
        let rt = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        let base_b = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        let existing = parse_overrides("[view]\ntypewriter=false\n");
        let of = compute_overrides(&rt, &base_b, &existing, &OverridesFile::default());
        assert_eq!(of.view.as_ref().unwrap().typewriter, Some(false), "saved intent survives coincidence");
    }

    #[test]
    fn rule3_removes_on_contradiction_unless_masked() {
        // User toggled back to the baseline value → the override contradicts → REMOVE...
        let rt = snap("cua", ThemeIdentity::Builtin("default".into()), true);
        let base = snap("cua", ThemeIdentity::Builtin("default".into()), true);
        let existing = parse_overrides("[view]\ntypewriter=false\n");
        let of = compute_overrides(&rt, &base, &existing, &OverridesFile::default());
        assert!(of.view.is_none(), "explicit un-save removes the key");
        // ...UNLESS the --config layer sets the key (mask-guard): KEEP verbatim.
        let mask = parse_overrides("[view]\ntypewriter=true\n");
        let of2 = compute_overrides(&rt, &base, &existing, &mask);
        assert_eq!(of2.view.as_ref().unwrap().typewriter, Some(false), "masked key never removed");
    }

    #[test]
    fn theme_mask_guard_is_provenance_typed() {
        // --config sets [theme] FILE (not name): runtime File == baseline File; the saved
        // name contradicts → rule-3 candidate — the FILE mask must still guard it (N-4).
        let rt = snap("cua", ThemeIdentity::File, false);
        let base = snap("cua", ThemeIdentity::File, false);
        let existing = parse_overrides("[theme]\nname='gruvbox'\n");
        let mask = parse_overrides("[theme]\nname='x'\n"); // name-mask arm
        let of = compute_overrides(&rt, &base, &existing, &mask);
        assert_eq!(of.theme.as_ref().unwrap().name.as_deref(), Some("gruvbox"));
        // The file-mask arm needs mask presence for [theme] file — OTheme carries name only,
        // so the mask snapshot for theme is provenance-collapsed at PARSE time: T4 parses the
        // --config layer with parse_mask (below), which sets theme presence when EITHER
        // name OR file is present. Here simulate it directly:
        let mask_file = parse_mask("[theme]\nfile='/tmp/x.yaml'\n");
        let of2 = compute_overrides(&rt, &base, &existing, &mask_file);
        assert_eq!(of2.theme.as_ref().unwrap().name.as_deref(), Some("gruvbox"), "file-mask guards the name override");
    }

    #[test]
    fn theme_rules_2_and_3_compare_by_provenance() {
        // rule 2: runtime Builtin(n) matching the saved name → keep.
        let rt = snap("cua", ThemeIdentity::Builtin("gruvbox".into()), false);
        let base = snap("cua", ThemeIdentity::Builtin("gruvbox".into()), false);
        let existing = parse_overrides("[theme]\nname='gruvbox'\n");
        let of = compute_overrides(&rt, &base, &existing, &OverridesFile::default());
        assert_eq!(of.theme.as_ref().unwrap().name.as_deref(), Some("gruvbox"));
        // rule 3: runtime File, saved name contradicts, no mask → removed.
        let rt2 = snap("cua", ThemeIdentity::File, false);
        let base2 = snap("cua", ThemeIdentity::File, false);
        let of2 = compute_overrides(&rt2, &base2, &existing, &OverridesFile::default());
        assert!(of2.theme.is_none());
    }

    #[test]
    fn theme_collision_pick_over_file_theme_writes_name() {
        // Fable plan I3(a): runtime Builtin(n) vs baseline File → rule 1 WRITES the name
        // even when n collides with the file theme's scheme name (the I-4 bug class).
        let rt = snap("cua", ThemeIdentity::Builtin("gruvbox".into()), false);
        let base = snap("cua", ThemeIdentity::File, false);
        let of = compute_overrides(&rt, &base, &OverridesFile::default(), &OverridesFile::default());
        assert_eq!(of.theme.as_ref().unwrap().name.as_deref(), Some("gruvbox"));
    }

    #[test]
    fn genuine_masked_session_divergence_still_writes() {
        // Fable plan I3(b): the mask-guard protects rule 3 only — rule 1 is untouched.
        let rt = snap("cua", ThemeIdentity::Builtin("default".into()), true);
        let base = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        let mask = parse_overrides("[view]\ntypewriter=false\n");
        let of = compute_overrides(&rt, &base, &OverridesFile::default(), &mask);
        assert_eq!(of.view.as_ref().unwrap().typewriter, Some(true), "explicit change wins through the mask");
    }

    #[test]
    fn corrupt_overrides_parse_to_an_empty_layer() {
        // Fable plan I3(d) / spec Error handling: a corrupt machine file must not brick.
        assert_eq!(parse_overrides("not [valid toml"), OverridesFile::default());
        assert_eq!(parse_mask("also ["), OverridesFile::default());
    }

    #[test]
    fn save_overrides_roundtrips_and_headers() {
        let d = tempdir(); // reuse the config.rs idiom: a small local tempdir helper
        let path = d.join("settings-overrides.toml");
        let mut of = OverridesFile::default();
        of.menu = Some(OMenu { bar: Some("pinned".into()) });
        of.mouse = Some(OMouse { capture: Some(false) });
        save_overrides(&crate::fsx::RealFs, &path, &of).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(OVERRIDES_HEADER));
        assert!(text.contains("bar = \"pinned\"") || text.contains("bar = 'pinned'"));
        assert_eq!(parse_overrides(&text), of, "round-trip identity");
        // all-empty → header only
        save_overrides(&crate::fsx::RealFs, &path, &OverridesFile::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), OVERRIDES_HEADER);
        // idempotence: same input twice → identical bytes
    }

    #[test]
    fn save_overrides_surfaces_io_failure() {
        struct FailFs;
        impl crate::fsx::Fs for FailFs {
            fn create_excl(&self, _: &std::path::Path, _: u32) -> std::io::Result<Box<dyn crate::fsx::WriteSync>> {
                Err(std::io::Error::other("boom")) // io_other_error is deny — the repo idiom (fsx.rs:394+)
            }
            fn existing_mode(&self, _: &std::path::Path) -> Option<u32> { None }
            fn rename(&self, _: &std::path::Path, _: &std::path::Path) -> std::io::Result<()> { unreachable!() }
            fn sync_dir(&self, _: &std::path::Path) -> std::io::Result<()> { unreachable!() }
            fn remove_file(&self, _: &std::path::Path) -> std::io::Result<()> { Ok(()) }
        }
        let d = tempdir();
        let err = save_overrides(&FailFs, &d.join("o.toml"), &OverridesFile::default()).unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn save_overrides_creates_the_parent_dir() {
        let d = tempdir();
        let path = d.join("nested").join("settings-overrides.toml");
        save_overrides(&crate::fsx::RealFs, &path, &OverridesFile::default()).unwrap();
        assert!(path.is_file());
    }
```

(`parse_mask` is a public sibling of `parse_overrides` that additionally collapses theme provenance: it deserializes a tiny private `MaskTheme { name: Option<String>, file: Option<String> }` view and sets the returned `OverridesFile.theme` to `Some(OTheme { name: Some(String::new()) })` when EITHER is present — presence is all the mask check reads. Document that with one comment.) RED: module doesn't exist.

- [ ] **Step 2: implement the module.** `compute_overrides` — complete shape:

```rust
/// The contradiction-only-removal diff law (spec D3, user-ratified; rules 1-4 + the
/// rule-3 mask-guard). Generic per-key helper: write on divergence; keep an existing
/// override that matches runtime; remove a contradicted override only when unmasked.
fn diff_key<T: PartialEq + Clone>(rt: &T, base: &T, existing: Option<&T>, masked: bool) -> Option<T> {
    if rt != base { return Some(rt.clone()); }
    match existing {
        Some(e) if e == rt => Some(e.clone()),
        Some(e) if masked => Some(e.clone()),
        Some(_) => None,
        None => None,
    }
}
```

Callers map each inventory key into its section (strings for preset/menu-bar via `menu_bar_str`, bools for the five toggles + capture); the THEME key uses the bespoke provenance logic (spec N-3/N-4):

```rust
    let theme_masked = mask.theme.is_some(); // provenance-collapsed at parse (name OR file)
    let theme_name: Option<String> = match (&runtime.theme_identity, &baseline.theme_identity) {
        (ThemeIdentity::Builtin(n), b) if *b != ThemeIdentity::Builtin(n.clone()) => Some(n.clone()),
        (rt, _) => match existing.theme.as_ref().and_then(|t| t.name.as_ref()) {
            Some(e) if *rt == ThemeIdentity::Builtin(e.clone()) => Some(e.clone()),
            Some(e) if theme_masked => Some(e.clone()),
            Some(_) | None => None,
        },
    };
```

Sections become `Some(...)` only when they hold at least one `Some` key (a tiny `fn some_if<T>(t: T, any: bool) -> Option<T>` or per-section construction — implementer's judgment). `save_overrides`: `std::fs::create_dir_all(parent)`, with Unix 0700 permissions applied ONLY when the dir did not already exist (`let existed = parent.exists();` first — Fable plan M2: `<config_dir>/wordcartel` also holds the hand-owned config.toml; a save must not silently tighten a user's 0755 dir, unlike swap's machine-only state dir), `toml::to_string(of)` (map the toml error into `io::Error::new(InvalidData, e)`), prepend `OVERRIDES_HEADER`, then `write_overrides(fs, path, bytes)` = `fsx::atomic_replace(fs, path, bytes, WriteOpts { mode: ModePolicy::Fixed(0o600), dir_fsync: true })`. `parse_overrides`: `toml::from_str::<OverridesFile>(bytes).unwrap_or_default()` (corrupt → empty layer, matching load()'s tolerance). `theme_identity_of(theme_cfg, resolved_name)`: `if theme_cfg.file.is_some() && theme_cfg.name.is_none() { File } else { Builtin(resolved_name.to_string()) }`. `snapshot_of(cfg)` uses `resolve_preset(&cfg.keymap.preset)` and needs the resolved theme name — give it the signature `snapshot_of(cfg: &Config, resolved_theme_name: &str)` so T4 passes the baseline's own `resolve_theme` result (do NOT resolve inside — resolve_theme takes an EnvSnapshot). `runtime_snapshot(editor)` reads the Editor fields directly (`editor.theme_identity.clone()` for the theme).

- [ ] **Step 3: Editor + picker threading.** Editor fields (beside T2's): `pub settings_save_requested: bool` (init false) + `pub theme_identity: crate::settings::ThemeIdentity` (init `Builtin("default".into())` — matches `theme: default()`). `ThemePicker` gains `pub previewed: Option<String>` (theme_picker.rs:6-15); init `None` at BOTH construction sites (editor.rs:676-679 `open_theme_picker` and the test literal theme_picker.rs:36-37). `preview_selected_theme` (app.rs:191-196) sets `tp.previewed = Some(name.clone())` when it applies a builtin (restructure to set the field via `editor.theme_picker.as_mut()` before/after `apply_theme` — mind the borrow: read the name, apply, then set the field). The Enter arm (app.rs:469) becomes:

```rust
                KeyCode::Enter => {
                    if let Some(tp) = editor.theme_picker.take() {
                        if let Some(n) = tp.previewed {
                            editor.theme_identity = crate::settings::ThemeIdentity::Builtin(n);
                        } // untouched open→Enter: no preview applied, identity unchanged (spec I-1)
                    }
                }
```

Esc arm (app.rs:465-468) already `take()`s and restores `original` — the taken `previewed` drops with it (nothing to add; verify and say so in the report). Register the third Settings command:

```rust
        r.register("save_settings", "Save Settings", Some(MenuCategory::Settings), |c| {
            c.editor.settings_save_requested = true;
            CommandResult::Handled
        });
```

Extend the T2 registry pin to all three Settings commands. Picker pins (app.rs tests, §C reduce idioms): `untouched_picker_enter_leaves_theme_identity_unchanged` (open via the `theme` command dispatch, Enter immediately, assert identity still the initial value) and `previewed_picker_enter_sets_builtin_identity` (open, send Down through reduce — the preview funnel fires — then Enter, assert `ThemeIdentity::Builtin(second row's name)`).

- [ ] **Step 4: GREEN + gates.**

- [ ] **Step 5: commit** — `feat(d1a5): settings module — diff law with mask-guard, atomic overrides writer, provenance theme identity`.

---

### Task 4: run() wiring — two-stage baseline, save block, refusals + e2e journey

**Files:**
- Modify: `wordcartel/src/app.rs` (two-stage load + snapshots + save block + seeding), `wordcartel/src/e2e.rs` (journey)

**Interfaces:**
- Consumes: everything above. Produces the shipped behavior.

- [ ] **Step 1: two-stage load + snapshots** (replacing the single load at app.rs:1189-1190; `cli`, `xdg`, `anchor` all in scope there):

```rust
    let hand_paths = config::config_layer_paths(&cli, xdg.as_deref(), &anchor);
    // The overrides layer: ABOVE the hand chain, BELOW --config (spec D3). --no-config
    // empties hand_paths and skips the overrides too (config_layer_paths returned early).
    let overrides_path = xdg.as_ref()
        .map(|x| x.join("wordcartel").join("settings-overrides.toml"));
    let mut all_paths = hand_paths.clone();
    if !cli.no_config {
        if let Some(op) = overrides_path.as_ref().filter(|p| p.is_file()) {
            // Race-free: derive from what config_layer_paths ACTUALLY pushed, not a re-stat
            // (Fable plan M3). The is_some() guard kills the None == None arm — an EMPTY
            // hand chain (no XDG config, no project file, no --config: the headline
            // save-once-then-relaunch flow) must append, not underflow (Fable r2 Critical).
            let has_cli_cfg = cli.config_path.is_some() && hand_paths.last() == cli.config_path.as_ref();
            let idx = if has_cli_cfg { all_paths.len() - 1 } else { all_paths.len() };
            all_paths.insert(idx, op.clone());
        }
    }
    let (baseline_cfg, _baseline_warns) = config::load(&hand_paths); // WITHOUT overrides
    let (cfg, mut warns) = config::load(&all_paths);                  // production config
```

Then, after the theme resolve in the seeding block: resolve the BASELINE theme too (`resolve_theme(&baseline_cfg.theme, &env)`) and build the three snapshots — `let baseline_snapshot = settings::snapshot_of(&baseline_cfg, &baseline_resolved.theme.name);` (the resolved `Theme.name`), `let mut overrides_snapshot = overrides_path.as_ref().filter(|p| p.is_file()).map(|p| std::fs::read_to_string(p).map(|s| settings::parse_overrides(&s)).unwrap_or_default()).unwrap_or_default();`, `let mask_snapshot = cli.config_path.as_ref().filter(|c| c.is_file()).map(|c| std::fs::read_to_string(c).map(|s| settings::parse_mask(&s)).unwrap_or_default()).unwrap_or_default();`. Seed `editor.theme_identity = settings::theme_identity_of(&cfg.theme, &resolved.theme.name);` (the MERGED config's provenance — an overrides/hand `name` wins over `file` per `theme_identity_of`'s rule).

- [ ] **Step 2: the save helper + block.** To make the refusals and statuses honestly unit-testable (Codex plan I2/I3 — run() calls `dirs::config_dir()` directly with no seam), the block's BODY is a pub(crate) fn in settings.rs:

```rust
/// Perform a requested settings save: refusals first, then diff + atomic write.
/// Returns the new overrides snapshot on success (the caller replaces its copy so
/// rules 2/3 stay correct for a second save in the same session). Sets editor.status
/// in every arm — no silent UI.
pub(crate) fn perform_settings_save(
    editor: &mut crate::editor::Editor,
    no_config: bool,
    overrides_path: Option<&std::path::Path>,
    baseline: &SettingsSnapshot,
    existing: &OverridesFile,
    mask: &OverridesFile,
    fs: &dyn crate::fsx::Fs,
) -> Option<OverridesFile> {
    if no_config {
        editor.status = "settings: disabled by --no-config".into();
        return None;
    }
    let Some(path) = overrides_path else {
        editor.status = "settings: no config directory".into();
        return None;
    };
    let runtime = runtime_snapshot(editor);
    let of = compute_overrides(&runtime, baseline, existing, mask);
    match save_overrides(fs, path, &of) {
        Ok(()) => { editor.status = "settings saved".into(); Some(of) }
        Err(e) => { editor.status = format!("settings: {e}"); None }
    }
}
```

run()'s block, beside T2's rebuild arm after `reduce`:

```rust
        if editor.settings_save_requested {
            editor.settings_save_requested = false;
            if let Some(of) = settings::perform_settings_save(
                &mut editor, cli.no_config, overrides_path.as_deref(),
                &baseline_snapshot, &overrides_snapshot, &mask_snapshot, &crate::fsx::RealFs)
            {
                overrides_snapshot = of;
            }
        }
```

- [ ] **Step 3: behavior pins.** In settings.rs (driving `perform_settings_save` directly — the honest homes Codex plan I2 demanded): `save_refused_under_no_config` (no_config=true → status "settings: disabled by --no-config", returns None, no file); `save_refused_without_config_dir` (overrides_path=None → "settings: no config directory"); `save_failure_surfaces_io_error` (the FailFs → status starts "settings: " and contains "boom"); `save_success_sets_status_and_returns_snapshot` (tempdir → "settings saved", returned OverridesFile == computed). In app.rs: `save_settings_command_sets_the_request_flag` (dispatch through the registry, assert the flag); `baseline_excludes_overrides_layer` (config.rs test home: load(&[hand]) vs load(&[hand, overrides_file]) differ on an overridden key — the spec's baseline pin); `save_reload_roundtrip_restores_settings` — a UNIT-level pin of the full pipeline without run(): build a `SettingsSnapshot` runtime with three divergences, `compute_overrides` + `save_overrides` to a tempdir file, then `config::load(&[hand_layer, overrides_file])` and assert the merged Config reflects the saved values (preset via patches? no — assert `cfg.keymap.preset == "wordstar"`, `cfg.view.typewriter`, `cfg.menu.bar == Pinned` — proving the written strings parse back through the REAL loader).

- [ ] **Step 4: e2e journey** (`journey_keymap_switch_scopes`, §C idioms). Codex plan I1: `scroll_line_up` only decrements when `view.scroll > 0` (nav.rs:623 — source tests seed scroll 4-5 first), so the corpus must make BOTH observables non-vacuous: build a 40-line doc (`"line\n".repeat(40)`), move the caret deep (`Selection::single` into line ~30) **then call `crate::nav::ensure_visible(&mut h.editor);`** (Fable plan I1: a direct selection write does NOT scroll — nothing in the harness path calls ensure_visible for it; nav.rs owns the scroll adjustment) and precondition-assert `h.editor.active().view.scroll > 0` (View.scroll is pub — editor.rs:110); under CUA assert ctrl-w EXPANDS the selection (`!h.editor.active().document.selection.primary().is_empty()` after ctrl-w, from an empty caret — precondition-assert empty before); collapse the selection; dispatch `keymap_wordstar` via the palette (`h.ctrl('p')`, `h.type_str("keymap: wordstar")`, precondition-assert the top row's label == "Keymap: WordStar", Enter; assert `h.status()` contains "keymap: wordstar"); then ctrl-w again: assert the selection STAYS empty AND `view.scroll` decremented by 1 (both directions of the flip pinned, neither vacuous).

- [ ] **Step 5: full gates + smoke.** Run `scripts/smoke/run.sh` once; quote the one-line summary VERBATIM in the report (advisory).

- [ ] **Step 6: commit** — `feat(d1a5): save-settings wiring — two-stage baseline, mask snapshot, refusals + keymap-switch journey`.

---

## Verification appendix (final gates charge)

- The three ratified laws hold end-to-end: contradiction-only removal (+ mask-guard, + provenance-typed theme predicate); explicit save only; scoped patches with "later file wins; within a file, specific wins".
- Hand files never written; the overrides file is 0600, atomic, header-first, wholesale.
- Refusals: --no-config, config_dir None. Statuses byte-exact per Global Constraints.
- The rebuild clears pending prefixes; hints verified fresh (palette rebuild_rows + menu open); `close_buffer` remains unbound in both presets (add a pin if T2 didn't).
- No core changes; no new deps; `resolve_preset` is the single fallback authority.
- Pre-merge: smoke verbatim + a live tmux sanity (switch preset via menu → a differentiated key changes behavior; Save Settings → status + file content inspected in the tmux shell).
- Ship-time bookkeeping: backlog D1/A5 → SHIPPED (+ C4 closure recorded); working order advances to E3(+E4 research).
