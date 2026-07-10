# Startup Splash / Welcome Screen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Paint a branded, dismissible startup splash (wordmark + version + tagline + three active-keymap-resolved hints) over the first frame, gated by a persisted `view.splash` option, a `--no-splash` CLI flag, and recovery-prompt suppression.

**Architecture:** A new `wordcartel/src/splash.rs` domain module owns everything splash: the `Splash` content model (hints resolved ONCE at construction against the loop-local keymap — paint has no keymap), the pure startup gate, an `intercept` stage inserted at the top of `reduce`'s chain (first key press / mouse-down clears + consumes; everything else passes), and a full-frame painter delegated from `render_overlays::paint`. The option follows the command-surface contract's set-per-state shape (`splash_on`/`splash_off` palette-only + stateful `toggle_splash` View representative, all through one shared `Editor::set_splash`), and persists through the `settings.rs` snapshot/diff-law machinery.

**Tech Stack:** Rust (edition per workspace), ratatui 0.30 (`TestBackend` for render tests), crossterm events, serde+toml (config/overrides), no new dependencies.

**Spec (source of truth):** `docs/superpowers/specs/2026-07-09-startup-splash-design.md` (committed at 00c458c, Codex GO). All line anchors below were re-verified against branch `effort-splash` at 00c458c — none had drifted.

**Codex plan review: GO** (round 2, 2026-07-10). Round 1 found one repeated Important (multi-filter `cargo test` run-steps — cargo rejects them; moved filters after `--`, folded at `1bc717e`); no Critical/Minor, and all code snippets/signatures/settings-migration sites were confirmed real against the branch. Round 2 confirmed the fix and re-verified the source shapes — clean GO. Ready for task-by-task subagent execution.

## Global Constraints

- Do NOT run `cargo fmt` (repo is hand-formatted, dense; no rustfmt.toml). Match neighbors by hand.
- GATES (before merge): `cargo test` green all suites; `cargo build` + `cargo test --no-run` warning-free for touched crates; `cargo clippy --workspace --all-targets` clean (workspace denies clippy::all + too_many_lines threshold 100 — a long fn needs an item-local `#[allow(clippy::too_many_lines)]` + one-line reason); module_budgets test (hub production budgets) must pass.
- House style: snake_case/PascalCase/SCREAMING_SNAKE; 4-space indent; ~100-col hand-wrapped; em-dash `—` in prose comments never `--`; NO emoji in code; private struct fields + accessors/validated constructors; typed error enums to the STATUS LINE never console (the app owns the terminal; print_stdout/print_stderr are DENIED — only main()'s allow'd exceptions); no `.unwrap()` on fallible/external paths (prefer `.expect("invariant")`); doc-comment public items.
- Command-surface contract is App law (docs/design/command-surface-contract.md): the plan MUST state how it conforms (set-per-state splash_on/splash_off menu:None + stateful toggle_splash representative; one shared setter Editor::set_splash; hints track active keymap; the invariant tests palette-completeness/every-option-has-a-command/hint-re-resolution stay green as merge gates).
- Commit trailer (every commit ends with, verbatim): `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` (omit the Claude-Session line — session URL unavailable).
- Idle-is-free / instant-typing priority: the splash must not add background work or block input; Tick/background/Resize pass through the intercept.

### Command-surface contract conformance (stated per the contract)

This effort adds a user-settable option (`view.splash`) and shows keybinding hints, so the contract governs it:

- **Every option is a command, set-per-state:** `splash_on` / `splash_off` are deterministic palette-only primitives (`menu: None`), `toggle_splash` is the stateful View-menu representative (`MenuMark::OnOff`) — the `status_line_on`/`status_line_auto`/`toggle_status_line` pattern at `registry.rs:489-495`.
- **Registry = single source of truth; palette exhaustive; menu ⊆ palette:** all three commands are registered via `Registry::builtins()`, so they appear in the palette automatically and only `toggle_splash` carries a menu tag.
- **One shared setter:** `Editor::set_splash(bool)` is the single write path all three commands call (Task 2/6); no direct `view_opts.splash` writes outside it (the config seed at `app.rs:484` is the established `view_opts.clone()` path, left as-is per spec).
- **Hints track the active keymap:** splash hint chords come from `KeyTrie::chord_for` against the keymap active at construction (startup). The splash never outlives a preset change (dismissed by the first input), so one-shot resolution IS active-keymap resolution; unbound hints are omitted.
- **Merge gates stay green:** palette-completeness holds by registration; every-option-has-a-command is extended with the `view_splash` line in `every_persisted_setting_has_a_command` (Task 7); hint re-resolution is satisfied structurally (construction-time resolution, asserted under CUA and WordStar in Task 2).

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `wordcartel/src/splash.rs` | **Create** | The whole splash domain: `Splash` model + `Splash::new` (hint resolution), `show_at_startup` gate, `intercept` reduce stage, `paint` full-frame painter. Hubs only delegate here (anti-regrowth). |
| `wordcartel/src/lib.rs` | Modify | Register `pub mod splash;`. |
| `wordcartel/src/config.rs` | Modify | `ViewConfig.splash` (default true), `RawView.splash` + fold arm, `Cli.no_splash` + `"--no-splash"` parse arm, usage doc-comment. |
| `wordcartel/src/editor.rs` | Modify | `Editor.splash: Option<Splash>` field + constructor init + `Editor::set_splash` shared setter. |
| `wordcartel/src/app.rs` | Modify | One thin intercept stage at the top of `reduce` (before `marks::intercept`, line 233); startup wiring in `run()` (before `first_frame_settle`, line 698). |
| `wordcartel/src/render_overlays.rs` | Modify | A delegating splash branch at the top of `paint` (line 37+). |
| `wordcartel/src/registry.rs` | Modify | `splash_on`/`splash_off`/`toggle_splash` registrations (after the status-line block, line 495). |
| `wordcartel/src/settings.rs` | Modify | Thread `view.splash` through `SettingsSnapshot`/`OView`/`snapshot_of`/`runtime_snapshot`/`compute_overrides`/`any_view`/command-guard test + `snap` helper. |
| `wordcartel/src/main.rs` | Modify | Usage doc-comment only (add `--no-splash`). |
| `wordcartel/src/e2e.rs` | Modify | Four splash journeys against the `TestBackend` harness. |

Tests are co-located `#[cfg(test)] mod tests` (`use super::*`) in each touched module; e2e journeys in `e2e.rs`.

---

### Task 1: Config + CLI plumbing (`view.splash`, `--no-splash`)

**Files:**
- Modify: `wordcartel/src/config.rs:8-13` (Cli struct), `:16` (usage doc), `:22-25` (parse_cli match), `:117-129` (ViewConfig), `:130-139` (Default), `:306-319` (RawView), `:400-405` (fold site)
- Modify: `wordcartel/src/main.rs:4` (usage doc-comment)
- Test: `wordcartel/src/config.rs` (existing `mod tests` at `:546`)

**Interfaces:**
- Consumes: existing `Cli`, `ViewConfig`, `RawView`, `load(paths) -> (Config, Vec<String>)`, `parse_cli<I>(args) -> Cli`; test helpers `write(dir, name, body)` (config.rs:549) and `tempdir()` (config.rs:634).
- Produces: `Cli.no_splash: bool` (default false), `ViewConfig.splash: bool` (default true), `RawView.splash: Option<bool>` folded per-field. Tasks 5 and 7 read `cfg.view.splash` and `cli.no_splash`.

- [ ] **Step 1: Write the failing tests** — append to `mod tests` in `wordcartel/src/config.rs` (after `malformed_toml_warns_and_skips_layer`, ~line 605):

```rust
    #[test]
    fn view_splash_defaults_on_and_folds_from_a_layer() {
        let (cfg, warns) = load(&[]);
        assert!(warns.is_empty());
        assert!(cfg.view.splash, "built-in default is on");
        let d = tempdir();
        let p = write(&d, "splash.toml", "[view]\nsplash = false\n");
        let (cfg, warns) = load(&[p]);
        assert!(warns.is_empty());
        assert!(!cfg.view.splash, "a layer that SETS the field overrides the default");
    }

    #[test]
    fn parse_cli_no_splash_flag() {
        let c = parse_cli(["wcartel", "--no-splash", "notes.md"].map(String::from));
        assert!(c.no_splash);
        assert_eq!(c.path.as_deref(), Some(std::path::Path::new("notes.md")));
        let c = parse_cli(["wcartel"].map(String::from));
        assert!(!c.no_splash, "defaults off");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel -- view_splash_defaults_on_and_folds_from_a_layer parse_cli_no_splash_flag`
Expected: COMPILE ERROR — `no field 'splash' on type 'ViewConfig'` / `no field 'no_splash' on type 'Cli'` (a compile failure is the correct red state for a new-field TDD step).

- [ ] **Step 3: Implement.** Five edits in `wordcartel/src/config.rs`:

(a) Add the CLI field — in `struct Cli` (line 8-13), after `pub no_config: bool,`:

```rust
    pub no_config: bool,
    /// `--no-splash` was passed: suppress the startup splash for THIS launch only
    /// (the persistent opt-out is `view.splash`; the flag never writes config).
    pub no_splash: bool,
```

(b) Update the parser doc-comment (line 16) and add the arm (after `"--no-config"`, line 24):

```rust
/// Hand-rolled (no clap dep): `[--version|-V] [--config <path>] [--no-config] [--no-splash] [file]`.
```

```rust
            "--no-config" => cli.no_config = true,
            "--no-splash" => cli.no_splash = true,
```

(c) Add the view field — in `struct ViewConfig` (line 117-129), after `pub status_line: TransientMode,`:

```rust
    pub status_line: TransientMode,
    /// Startup splash / welcome screen (`[view] splash`). On by default; the splash is
    /// painted over the first frame and dismissed by the first key press or mouse click.
    pub splash: bool,
```

(d) Extend the `Default` impl (line 130-139) — the literal gains the field:

```rust
            scrollbar: TransientMode::Auto, status_line: TransientMode::On, splash: true }
```

(e) Add the raw field + fold arm. In `struct RawView` (line 306-319), after `status_line: Option<String>,`:

```rust
    status_line: Option<String>,
    splash: Option<bool>,
```

In `load`'s view fold (line 400-405), after the `word_count` arm:

```rust
        if let Some(v) = raw.view.word_count { cfg.view.word_count = v; }
        if let Some(v) = raw.view.splash { cfg.view.splash = v; }
```

Then update the usage doc-comment in `wordcartel/src/main.rs:4` (both the module doc line and nothing else):

```rust
//! Usage: `wcartel [--version|-V] [--no-config] [--no-splash] [--config <path>] [file.md]`
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p wordcartel -- view_splash_defaults_on_and_folds_from_a_layer parse_cli_no_splash_flag`
Expected: PASS (2 tests). Then `cargo test -p wordcartel config::` — all config tests PASS (no regressions).

- [ ] **Step 5: Commit**

```bash
cd /home/jkeim/projects/groundwords
git add wordcartel/src/config.rs wordcartel/src/main.rs
git commit -m "feat(splash): view.splash config option (default on) + --no-splash CLI flag

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: `splash.rs` module — `Splash` model, hint resolution, editor field, shared setter

**Files:**
- Create: `wordcartel/src/splash.rs`
- Modify: `wordcartel/src/lib.rs:57` (add `pub mod splash;` after `pub mod file_browser;`)
- Modify: `wordcartel/src/editor.rs:444-445` (field), `:528` (constructor literal), `:853` (setter, after `set_status_line_mode`)
- Test: `wordcartel/src/splash.rs` (`mod tests`), `wordcartel/src/editor.rs` (existing `mod tests` at `:885`)

**Interfaces:**
- Consumes: `crate::keymap::KeyTrie::chord_for(CommandId) -> Option<String>` (keymap.rs:194), `crate::registry::CommandId(pub &'static str)` (registry.rs:16), `ViewConfig.splash` (Task 1), `crate::keymap::build_keymap(&KeymapConfig, &Registry) -> (KeyTrie, Vec<String>)` (keymap.rs:463).
- Produces (later tasks rely on these exact signatures):
  - `pub struct Splash` (private fields `version: String`, `hints: Vec<(String, &'static str)>`)
  - `impl Splash { pub fn new(keymap: &crate::keymap::KeyTrie, version: &str) -> Splash; pub fn version(&self) -> &str; pub fn hints(&self) -> &[(String, &'static str)] }`
  - consts `WORDMARK: &str = "wordcartel"`, `TAGLINE: &str = "Everyone needs a cover story"`, `FOOTER: &str = "press any key"` (module-private, used by Task 4's painter)
  - `Editor.splash: Option<crate::splash::Splash>` (pub field, `None` at construction)
  - `Editor::set_splash(&mut self, on: bool)` (writes `self.view_opts.splash`)

- [ ] **Step 1: Write the failing tests.** Create `wordcartel/src/splash.rs` with ONLY the test module first (plus the module doc so the file parses):

```rust
//! Startup splash / welcome overlay (spec 2026-07-09-startup-splash-design.md).
//! Branded first frame — wordmark + version + tagline + active-keymap hints —
//! dismissed (and the event consumed) by the first key press or mouse click.
//! Idle-is-free: no timers, no background work, no auto-timeout.

#[cfg(test)]
mod tests {
    use super::*;

    fn cua_keymap() -> crate::keymap::KeyTrie {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        km
    }

    #[test]
    fn new_resolves_all_three_hints_under_cua() {
        let s = Splash::new(&cua_keymap(), "0.1.0");
        assert_eq!(s.version(), "v0.1.0");
        let hints: Vec<(&str, &str)> = s.hints().iter().map(|(c, l)| (c.as_str(), *l)).collect();
        assert_eq!(hints, vec![
            ("ctrl-p", "Command palette"), ("ctrl-o", "Open file"), ("ctrl-q", "Quit")]);
    }

    #[test]
    fn new_omits_unbound_hints_under_wordstar() {
        // WordStar binds neither "palette" nor "open" (keymap.rs WORDSTAR table); quit is
        // bound as ctrl-k q / ctrl-k ctrl-q and chord_for picks the shortest display.
        let reg = crate::registry::Registry::builtins();
        let km_cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: Vec::new() };
        let (km, _) = crate::keymap::build_keymap(&km_cfg, &reg);
        let s = Splash::new(&km, "0.1.0");
        let hints: Vec<(&str, &str)> = s.hints().iter().map(|(c, l)| (c.as_str(), *l)).collect();
        assert_eq!(hints, vec![("ctrl-k q", "Quit")], "unbound hints are omitted, not blank");
    }
}
```

And append to `mod tests` in `wordcartel/src/editor.rs` (inside the existing test module at line 885+):

```rust
    #[test]
    fn splash_field_defaults_none_and_set_splash_is_the_single_write_path() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        assert!(e.splash.is_none(), "no splash until run() decides at startup");
        assert!(e.view_opts.splash, "ViewConfig default seeds on");
        e.set_splash(false);
        assert!(!e.view_opts.splash);
        e.set_splash(true);
        assert!(e.view_opts.splash);
    }
```

Register the module in `wordcartel/src/lib.rs` — after `pub mod file_browser;`:

```rust
pub mod file_browser;
pub mod splash;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel splash`
Expected: COMPILE ERROR — `cannot find type 'Splash'` / `no field 'splash' on type 'Editor'` / `no method named 'set_splash'`.

- [ ] **Step 3: Implement.** Add to `wordcartel/src/splash.rs` above the test module:

```rust
use crate::keymap::KeyTrie;
use crate::registry::CommandId;

/// The splash wordmark — the app's styled-text identity (no ASCII art).
const WORDMARK: &str = "wordcartel";
/// The tagline painted dim under the version line.
const TAGLINE: &str = "Everyone needs a cover story";
/// The dismiss-hint footer, painted dim.
const FOOTER: &str = "press any key";

/// The orientation hints in display order: command id → label. All three are real
/// registered commands ("help" was dropped in spec review — no such command exists).
const HINTS: [(&str, &str); 3] =
    [("palette", "Command palette"), ("open", "Open file"), ("quit", "Quit")];

/// Resolved splash content. Hints are resolved ONCE at construction: `run()` moves the
/// keymap out of the editor (`std::mem::take`, app.rs:613) before the first draw, so
/// paint-time resolution is impossible — and the splash is dismissed by the first input,
/// so it can never outlive a keymap change (one-shot resolution == active-keymap
/// resolution). Theme faces are read at paint time, not stored here.
#[derive(Debug, Clone)]
pub struct Splash {
    version: String,
    /// Surviving `(chord, label)` pairs — a hint whose command is unbound is omitted.
    hints: Vec<(String, &'static str)>,
}

impl Splash {
    /// Resolve the splash content against the active keymap.
    ///
    /// `version` is the bare `CARGO_PKG_VERSION` (e.g. `"0.1.0"`); the stored display
    /// line prepends `v`. A hint whose command has no chord in `keymap`
    /// (`chord_for` → `None`) is omitted — no dangling labels.
    ///
    /// # Examples
    /// ```
    /// let reg = wordcartel::registry::Registry::builtins();
    /// let (km, _) = wordcartel::keymap::build_keymap(
    ///     &wordcartel::config::KeymapConfig::default(), &reg);
    /// let s = wordcartel::splash::Splash::new(&km, "0.1.0");
    /// assert_eq!(s.version(), "v0.1.0");
    /// assert_eq!(s.hints().len(), 3); // palette/open/quit all bound under CUA
    /// ```
    pub fn new(keymap: &KeyTrie, version: &str) -> Splash {
        let hints = HINTS.iter()
            .filter_map(|&(id, label)| keymap.chord_for(CommandId(id)).map(|ch| (ch, label)))
            .collect();
        Splash { version: format!("v{version}"), hints }
    }

    /// The display version line, e.g. `"v0.1.0"`.
    pub fn version(&self) -> &str { &self.version }

    /// The resolved `(chord, label)` hint pairs (unbound hints already omitted).
    pub fn hints(&self) -> &[(String, &'static str)] { &self.hints }
}
```

In `wordcartel/src/editor.rs`, add the field after `file_browser` (line 444-445):

```rust
    /// File browser overlay state. XOR with all other overlays.
    pub file_browser: Option<crate::file_browser::FileBrowser>,
    /// Startup splash overlay. Set once in `run()` (gated on config, `--no-splash`, and
    /// no pending recovery prompt); cleared — consuming the event — by the first key
    /// press or mouse click (`splash::intercept`). XOR with the other overlays by
    /// construction: only ever set at launch before any input, and the first input that
    /// could open an overlay is consumed dismissing it (no `open_*` clearing needed).
    pub splash: Option<crate::splash::Splash>,
```

In the `new_from_text` constructor literal (line 528), after `file_browser: None,`:

```rust
            file_browser: None,
            splash: None,
```

Add the setter after `set_status_line_mode` (line 853):

```rust
    /// Set the startup-splash enablement (`view.splash`). The single write path the
    /// `splash_on`/`splash_off`/`toggle_splash` commands (and profiles) call (contract
    /// law 6). Takes effect on the NEXT launch — the splash itself paints only at startup.
    pub fn set_splash(&mut self, on: bool) {
        self.view_opts.splash = on;
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p wordcartel splash`
Expected: PASS (3 tests: 2 in splash.rs + 1 in editor.rs, plus the doctest). Then `cargo test -p wordcartel` — full crate green.

- [ ] **Step 5: Commit**

```bash
cd /home/jkeim/projects/groundwords
git add wordcartel/src/splash.rs wordcartel/src/lib.rs wordcartel/src/editor.rs
git commit -m "feat(splash): Splash model with construction-time hint resolution + editor field + set_splash

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: `splash::intercept` at the top of `reduce`'s chain

**Files:**
- Modify: `wordcartel/src/splash.rs` (add `intercept` + imports)
- Modify: `wordcartel/src/app.rs:233` (insert the stage BEFORE `marks::intercept`)
- Test: `wordcartel/src/splash.rs` (`mod tests`)

**Interfaces:**
- Consumes: `crate::app::Handled` (`pub(crate) enum Handled { Done(bool), Pass(Msg) }`, app.rs:123), `crate::app::Msg`, peer intercept shape `fn(Msg, &mut Editor, &dyn Executor, &dyn Clock, &Sender<Msg>) -> Handled` (marks.rs:13-15), `Editor.splash` (Task 2), test helpers `crate::test_support::{TestClock, press}` and `crate::jobs::InlineExecutor`.
- Produces: `pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor, _ex: &dyn crate::jobs::Executor, _clock: &dyn wordcartel_core::history::Clock, _msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled` — Task 5's e2e journeys exercise it through `reduce`.

- [ ] **Step 1: Write the failing tests** — append inside `mod tests` in `wordcartel/src/splash.rs`:

```rust
    use crate::app::{Handled, Msg};
    use crate::editor::Editor;
    use crate::jobs::InlineExecutor;
    use crate::test_support::{press, TestClock};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn splashed_editor() -> Editor {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        e.splash = Some(Splash::new(&cua_keymap(), "0.1.0"));
        e
    }

    fn run_intercept(msg: Msg, e: &mut Editor) -> Handled {
        let ex = InlineExecutor::default();
        let clk = TestClock::new(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        intercept(msg, e, &ex, &clk, &tx)
    }

    #[test]
    fn intercept_passes_everything_when_no_splash() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        assert!(e.splash.is_none());
        // Handled has no Debug derive — match exhaustively without formatting it.
        match run_intercept(press(KeyCode::Char('x'), KeyModifiers::NONE), &mut e) {
            Handled::Pass(Msg::Input(Event::Key(_))) => {}
            Handled::Pass(_) => panic!("the key must pass through as the SAME message"),
            Handled::Done(_) => panic!("no splash → the key must pass, not be consumed"),
        }
    }

    #[test]
    fn intercept_key_press_dismisses_and_consumes() {
        let mut e = splashed_editor();
        match run_intercept(press(KeyCode::Char('x'), KeyModifiers::NONE), &mut e) {
            Handled::Done(keep) => assert!(keep, "consumed, app keeps running"),
            Handled::Pass(_) => panic!("the dismissing key press must be consumed"),
        }
        assert!(e.splash.is_none(), "splash cleared");
        assert_eq!(e.active().document.buffer.to_string(), "hello\n", "nothing typed");
    }

    #[test]
    fn intercept_mouse_down_dismisses_and_consumes() {
        let mut e = splashed_editor();
        let msg = Msg::Input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10, row: 5, modifiers: KeyModifiers::NONE,
        }));
        match run_intercept(msg, &mut e) {
            Handled::Done(keep) => assert!(keep),
            Handled::Pass(_) => panic!("mouse-down must be consumed"),
        }
        assert!(e.splash.is_none());
    }

    #[test]
    fn intercept_passes_tick_resize_background_and_key_release() {
        let mut e = splashed_editor();
        // Tick, Resize, and a background result all pass through (idle-is-free: startup
        // warmup / timers / resize-reheal keep working while the splash is up).
        for msg in [
            Msg::Tick,
            Msg::Input(Event::Resize(100, 40)),
            Msg::ClipboardAvailability(true),
            Msg::Input(Event::Key(KeyEvent { code: KeyCode::Char('x'),
                modifiers: KeyModifiers::NONE, kind: KeyEventKind::Release,
                state: KeyEventState::NONE })),
        ] {
            match run_intercept(msg, &mut e) {
                Handled::Pass(_) => {}
                Handled::Done(_) => panic!("non-press, non-mouse-down messages must pass"),
            }
            assert!(e.splash.is_some(), "splash survives pass-through messages");
        }
    }

    #[test]
    fn intercept_done_reports_quit_flag() {
        let mut e = splashed_editor();
        e.quit = true; // hypothetical: the contract is Done(!editor.quit), verbatim
        match run_intercept(press(KeyCode::Char('x'), KeyModifiers::NONE), &mut e) {
            Handled::Done(keep) => assert!(!keep, "Done carries !editor.quit"),
            Handled::Pass(_) => panic!("must consume"),
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel splash::tests::intercept`
Expected: COMPILE ERROR — `cannot find function 'intercept' in this scope`.

- [ ] **Step 3: Implement.** In `wordcartel/src/splash.rs`, extend the imports and add `intercept` (below the `impl Splash` block, above `mod tests`):

```rust
use crate::keymap::KeyTrie;
use crate::registry::CommandId;
use crate::app::{Handled, Msg};
use crossterm::event::{Event, KeyEventKind, MouseEventKind};
```

```rust
/// Splash dismissal stage — the FIRST stage in `reduce`'s intercept chain.
///
/// Contract (spec §3): `splash.is_none()` → `Pass(msg)`; else the first key PRESS or
/// mouse-DOWN clears the splash and is CONSUMED (`Done(!editor.quit)`); every other
/// message — `Tick`, background job results, `Resize`, key release/repeat, mouse
/// move/scroll — passes through so startup warmup, the timer subsystems, and
/// resize-reheal keep working while the splash is up (idle-is-free).
pub(crate) fn intercept(msg: Msg, editor: &mut crate::editor::Editor,
    _ex: &dyn crate::jobs::Executor, _clock: &dyn wordcartel_core::history::Clock,
    _msg_tx: &std::sync::mpsc::Sender<Msg>) -> Handled {
    if editor.splash.is_none() { return Handled::Pass(msg); }
    let dismiss = match &msg {
        Msg::Input(Event::Key(k)) => k.kind == KeyEventKind::Press,
        Msg::Input(Event::Mouse(m)) => matches!(m.kind, MouseEventKind::Down(_)),
        _ => false,
    };
    if dismiss {
        editor.splash = None;
        Handled::Done(!editor.quit)
    } else {
        Handled::Pass(msg)
    }
}
```

Then wire the stage into `reduce` in `wordcartel/src/app.rs` — insert BEFORE the `marks::intercept` line (233), matching the surrounding two-line stage style:

```rust
    let msg = match crate::splash::intercept(msg, editor, ex, clock, msg_tx) {
        crate::app::Handled::Done(k) => return k, crate::app::Handled::Pass(m) => m };
    let msg = match crate::marks::intercept(msg, editor, ex, clock, msg_tx) {
        crate::app::Handled::Done(k) => return k, crate::app::Handled::Pass(m) => m };
```

(One added fixed stage — the chain stays bounded; app.rs production size 779+2 is well inside its 1000-line hub budget.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p wordcartel splash::tests::intercept`
Expected: PASS (5 tests). Then `cargo test -p wordcartel` — full crate green (the new stage is inert while `splash` is `None`, so no journey regresses).

- [ ] **Step 5: Commit**

```bash
cd /home/jkeim/projects/groundwords
git add wordcartel/src/splash.rs wordcartel/src/app.rs
git commit -m "feat(splash): dismiss-and-consume intercept as the first reduce stage

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Full-frame painter

**Files:**
- Modify: `wordcartel/src/splash.rs` (add `paint` + imports)
- Modify: `wordcartel/src/render_overlays.rs:37-40` (delegating branch at the top of `paint`)
- Test: `wordcartel/src/splash.rs` (`mod tests`)

**Interfaces:**
- Consumes: `crate::compose::{compose, base_canvas}` (`compose(theme, depth, stack) -> ratatui Style`, `base_canvas(theme, depth) -> ratatui Style`, compose.rs:39/43), `wordcartel_core::theme::{SemanticElement, CanvasMode}`, `render_overlays::paint(frame, editor, cs)` (render_overlays.rs:37, the last paint step called at render.rs:736), `render()`'s tiny-terminal guard `w < 4 || h < 2` (render.rs:222 — render returns before any overlay painter below it), `Splash::{version, hints}` + consts (Task 2).
- Produces: `pub(crate) fn paint(frame: &mut ratatui::Frame, editor: &crate::editor::Editor)` in `splash.rs` — a no-op when `editor.splash` is `None`.

- [ ] **Step 1: Write the failing tests** — append inside `mod tests` in `wordcartel/src/splash.rs`:

```rust
    /// Build a splashed editor sized to the terminal it will be drawn on.
    fn splashed_editor_sized(w: u16, h: u16) -> Editor {
        let mut e = Editor::new_from_text("hello\n", None, (w, h));
        e.splash = Some(Splash::new(&cua_keymap(), "0.1.0"));
        crate::derive::rebuild(&mut e);
        e
    }

    fn draw(e: &mut Editor, w: u16, h: u16) -> Vec<String> {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h))
            .expect("test terminal");
        term.draw(|f| crate::render::render(f, e)).expect("draw");
        let buf = term.backend().buffer().clone();
        (0..buf.area().height)
            .map(|y| (0..buf.area().width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn contains(rows: &[String], needle: &str) -> bool {
        rows.iter().any(|r| r.contains(needle))
    }

    #[test]
    fn paint_full_content_at_80x24() {
        let mut e = splashed_editor_sized(80, 24);
        let rows = draw(&mut e, 80, 24);
        assert!(contains(&rows, "wordcartel"), "wordmark:\n{rows:#?}");
        assert!(contains(&rows, "v0.1.0"), "version");
        assert!(contains(&rows, "Everyone needs a cover story"), "tagline");
        assert!(contains(&rows, "ctrl-p   Command palette"), "palette hint");
        assert!(contains(&rows, "ctrl-o   Open file"), "open hint");
        assert!(contains(&rows, "ctrl-q   Quit"), "quit hint");
        assert!(contains(&rows, "press any key"), "footer");
        assert!(!contains(&rows, "hello"), "the splash owns the screen — body text hidden");
    }

    #[test]
    fn paint_degrades_hints_then_tagline_keeping_the_wordmark() {
        // h=8 < the full block (9 rows with 3 hints): hints + footer drop, tagline stays.
        let mut e = splashed_editor_sized(80, 8);
        let rows = draw(&mut e, 80, 8);
        assert!(contains(&rows, "wordcartel") && contains(&rows, "v0.1.0"));
        assert!(contains(&rows, "Everyone needs a cover story"));
        assert!(!contains(&rows, "Command palette") && !contains(&rows, "press any key"));
        // h=2: tagline drops too; wordmark + version survive.
        let mut e = splashed_editor_sized(80, 2);
        let rows = draw(&mut e, 80, 2);
        assert!(contains(&rows, "wordcartel") && contains(&rows, "v0.1.0"));
        assert!(!contains(&rows, "Everyone needs a cover story"));
    }

    #[test]
    fn paint_never_panics_at_tiny_sizes() {
        // Sweep 1x1..=12x6 — includes the sub-guard sizes (w<4 or h<2) where render()
        // paints its clamped notice and never reaches the overlay painters.
        for w in 1..=12u16 {
            for h in 1..=6u16 {
                let mut e = splashed_editor_sized(w, h);
                let _ = draw(&mut e, w, h);
            }
        }
    }

    #[test]
    fn dismissed_splash_reveals_the_document() {
        let mut e = splashed_editor_sized(80, 24);
        let rows = draw(&mut e, 80, 24);
        assert!(!contains(&rows, "hello"));
        e.splash = None;
        let rows = draw(&mut e, 80, 24);
        assert!(contains(&rows, "hello"), "dismiss reveals the buffer");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel -- splash::tests::paint splash::tests::dismissed`
Expected: FAIL — compiles (nothing new referenced yet except behavior), but `paint_full_content_at_80x24` asserts fail: no wordmark painted (`render` has no splash branch yet). If the harness helpers compile-error first, that is the same red state.

- [ ] **Step 3: Implement.** In `wordcartel/src/splash.rs`, extend imports:

```rust
use ratatui::{layout::Rect, style::Modifier, text::{Line, Span}, widgets::{Clear, Paragraph}, Frame};
use wordcartel_core::theme::SemanticElement as SE;
```

Add the painter below `intercept`:

```rust
/// Paint the full-frame startup splash from the pre-resolved `Splash` content.
///
/// The splash owns the screen: every cell of the frame (including the status row) is
/// cleared, the base canvas is filled per `CanvasMode` (mirrors `render()`'s edit-band
/// fill), and the centered block is drawn over it. Degradation as height shrinks: the
/// hints + footer drop first, then the tagline, then the version — the wordmark always
/// stays. `render()` never calls the overlay painters below its `w < 4 || h < 2` guard,
/// and every rect here is clamped to the frame, so no terminal size can panic.
pub(crate) fn paint(frame: &mut Frame, editor: &crate::editor::Editor) {
    let Some(splash) = editor.splash.as_ref() else { return };
    let area = frame.area();
    let (w, h) = (area.width, area.height);
    // The splash owns the screen — reset every cell (hides the text + status behind it).
    frame.render_widget(Clear, area);
    // Opaque canvas: fill the WHOLE frame with base_bg so fg-only text sits on the page
    // (render.rs:251 pattern). Transparent mode and colorless themes skip the fill.
    if editor.canvas == wordcartel_core::theme::CanvasMode::Opaque {
        let mut cbg = crate::compose::base_canvas(&editor.theme, editor.depth);
        cbg.fg = None; // bg-only fill
        if cbg.bg.is_some() && cbg.bg != Some(ratatui::style::Color::Reset) {
            frame.buffer_mut().set_style(area, cbg);
        }
    }
    // Faces: wordmark = the theme's H1 accent + BOLD; body = plain text; DIM recedes.
    let accent = crate::compose::compose(&editor.theme, editor.depth, &[SE::Text, SE::Heading(1)])
        .add_modifier(Modifier::BOLD);
    let body = crate::compose::compose(&editor.theme, editor.depth, &[SE::Text]);
    let dim = body.add_modifier(Modifier::DIM);

    // Build the largest block that fits `h` (degrade: hints+footer → tagline → version;
    // the footer is orientation text and travels with the hints).
    let full_rows = 6 + splash.hints().len(); // wordmark, version, tagline, blank, hints…, blank, footer
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(WORDMARK, accent))];
    if h >= 2 { lines.push(Line::from(Span::styled(splash.version(), body))); }
    if h >= 3 { lines.push(Line::from(Span::styled(TAGLINE, dim))); }
    if (h as usize) >= full_rows && !splash.hints().is_empty() {
        lines.push(Line::default());
        let cw = splash.hints().iter().map(|(c, _)| c.chars().count()).max().unwrap_or(0);
        for (chord, label) in splash.hints() {
            lines.push(Line::from(Span::styled(format!("{chord:>cw$}   {label}"), body)));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(FOOTER, dim)));
    }
    // Vertically + horizontally centered; over-wide lines clip at the frame edge.
    let top = (h as usize).saturating_sub(lines.len()) / 2;
    for (i, line) in lines.into_iter().enumerate() {
        let y = top + i;
        if y >= h as usize { break; }
        let lw = line.width().min(w as usize) as u16;
        if lw == 0 { continue; } // blank spacer rows
        let x = (w - lw) / 2;
        frame.render_widget(Paragraph::new(line), Rect::new(area.x + x, area.y + y as u16, lw, 1));
    }
}
```

Then the delegating branch in `wordcartel/src/render_overlays.rs` — at the top of `paint`, right after `let h = area.height;` (line 39):

```rust
    let area = frame.area();
    let h = area.height;

    // Startup splash — owns the whole frame; nothing else can be open while it is up
    // (set only at launch with no prompt pending; dismissed by the first key/click).
    if editor.splash.is_some() {
        crate::splash::paint(frame, editor);
        return;
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p wordcartel -- splash::tests::paint splash::tests::dismissed`
Expected: PASS (4 tests). Then `cargo test -p wordcartel` — full crate green.

- [ ] **Step 5: Commit**

```bash
cd /home/jkeim/projects/groundwords
git add wordcartel/src/splash.rs wordcartel/src/render_overlays.rs
git commit -m "feat(splash): full-frame centered painter with height degradation, delegated from render_overlays

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Startup gate + `run()` wiring + e2e journeys

**Files:**
- Modify: `wordcartel/src/splash.rs` (add `show_at_startup`)
- Modify: `wordcartel/src/app.rs:697-698` (wire into `run()` before `first_frame_settle`)
- Test: `wordcartel/src/splash.rs` (`mod tests`), `wordcartel/src/e2e.rs`

**Interfaces:**
- Consumes: `cfg.view.splash` + `cli.no_splash` (Task 1; `cli.path` is moved at app.rs:439 but `no_splash: bool` is Copy — accessing it later is fine, same as `cli.config_path` at app.rs:531), `editor.prompt.is_some()` (recovery-on-open sets it via `open_prompt` at app.rs:568/575 → editor.rs:656; no other startup prompt precedes the first draw), the loop-local `keymap` (`std::mem::take` at app.rs:613), `Splash::new` + `intercept` + `paint` (Tasks 2-4), e2e `Harness` (e2e.rs:67 `Harness::new(text, path, size)`, `.step/.render/.type_str/.mouse_down/.doc_text/.screen_contains`), `crate::prompt::Prompt::swap_recovery()` (prompt.rs:104).
- Produces: `pub fn show_at_startup(cfg_splash: bool, no_splash: bool, prompt_pending: bool) -> bool` in `splash.rs` — the exact gate `run()` evaluates; e2e journeys mirror `run()` through it.

- [ ] **Step 1: Write the failing unit test** — append inside `mod tests` in `wordcartel/src/splash.rs`:

```rust
    #[test]
    fn show_at_startup_truth_table() {
        assert!(show_at_startup(true, false, false), "enabled, no flag, no prompt → show");
        assert!(!show_at_startup(false, false, false), "view.splash = false wins");
        assert!(!show_at_startup(true, true, false), "--no-splash wins for this launch");
        assert!(!show_at_startup(true, false, true), "recovery prompt wins — never bury it");
        assert!(!show_at_startup(false, true, true), "all suppressors together");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p wordcartel show_at_startup_truth_table`
Expected: COMPILE ERROR — `cannot find function 'show_at_startup'`.

- [ ] **Step 3: Implement the gate** — in `wordcartel/src/splash.rs`, below the `impl Splash` block:

```rust
/// The `run()` startup gate: show the splash iff it is enabled in config, not
/// suppressed by `--no-splash`, and no prompt (the swap-recovery prompt is the only
/// pre-first-draw one) is pending at launch — never bury "recover your work?".
pub fn show_at_startup(cfg_splash: bool, no_splash: bool, prompt_pending: bool) -> bool {
    cfg_splash && !no_splash && !prompt_pending
}
```

- [ ] **Step 4: Run to green**

Run: `cargo test -p wordcartel show_at_startup_truth_table`
Expected: PASS.

- [ ] **Step 5: Wire `run()`** — in `wordcartel/src/app.rs`, immediately BEFORE `first_frame_settle(&mut editor);` (line 698; the loop-local `keymap` from the `mem::take` at 613 and `cfg` are both in scope):

```rust
    // Startup splash (spec 2026-07-09): resolved against the loop-local keymap AFTER the
    // mem::take, gated on config + --no-splash + no pending recovery prompt, set before
    // the first draw. Dismissal is splash::intercept — the first stage of reduce.
    if crate::splash::show_at_startup(cfg.view.splash, cli.no_splash, editor.prompt.is_some()) {
        editor.splash = Some(crate::splash::Splash::new(&keymap, env!("CARGO_PKG_VERSION")));
    }
    first_frame_settle(&mut editor);
```

Run: `cargo build -p wordcartel` — Expected: compiles warning-free.

- [ ] **Step 6: Write the failing e2e journeys** — append to `wordcartel/src/e2e.rs` (after the existing journeys; `key_char`, `Event`, `Msg` are already imported at the top of the file):

```rust
#[test]
fn e2e_splash_first_frame_then_key_dismisses_and_is_consumed() {
    let mut h = Harness::new("hello behind\n", None, (80, 24));
    // Mirror run()'s startup wiring (app.rs: gate → resolve against the live keymap →
    // set before the first draw). view_opts carries the ViewConfig default (splash on).
    let show = crate::splash::show_at_startup(
        h.editor.view_opts.splash, false, h.editor.prompt.is_some());
    assert!(show, "default config + no flag + no prompt shows the splash");
    h.editor.splash = Some(crate::splash::Splash::new(&h.keymap, "0.1.0"));
    h.render();
    assert!(h.screen_contains("wordcartel"), "wordmark on the first frame");
    assert!(h.screen_contains("press any key"), "footer on the first frame");
    assert!(!h.screen_contains("hello behind"), "the splash owns the screen");
    // The first key press dismisses AND is consumed (not typed into the buffer).
    let keep = h.step(Msg::Input(Event::Key(key_char('x'))));
    assert!(keep);
    assert!(h.editor.splash.is_none(), "splash cleared by the first key");
    assert_eq!(h.doc_text(), "hello behind\n", "the dismissing key was consumed");
    assert!(h.screen_contains("hello behind"), "dismiss reveals the document");
    // The NEXT key edits normally.
    h.type_str("y");
    assert_eq!(h.doc_text(), "yhello behind\n");
}

#[test]
fn e2e_splash_mouse_click_dismisses_without_editing() {
    let mut h = Harness::new("hello\n", None, (80, 24));
    h.editor.splash = Some(crate::splash::Splash::new(&h.keymap, "0.1.0"));
    h.render();
    assert!(!h.screen_contains("hello"));
    h.mouse_down(10, 5);
    assert!(h.editor.splash.is_none(), "mouse-down dismisses");
    assert_eq!(h.doc_text(), "hello\n", "the click did not edit anything");
    assert!(h.screen_contains("hello"));
}

#[test]
fn e2e_no_splash_flag_suppresses_first_frame_splash() {
    let mut h = Harness::new("hello\n", None, (80, 24));
    let show = crate::splash::show_at_startup(
        h.editor.view_opts.splash, true, h.editor.prompt.is_some());
    assert!(!show, "--no-splash wins over the enabled config default");
    // run() therefore leaves editor.splash = None → the first frame is the plain editor.
    h.render();
    assert!(h.screen_contains("hello"));
    assert!(!h.screen_contains("press any key"));
}

#[test]
fn e2e_recovery_prompt_pending_suppresses_splash() {
    let mut h = Harness::new("hello\n", None, (80, 24));
    h.editor.open_prompt(crate::prompt::Prompt::swap_recovery());
    let show = crate::splash::show_at_startup(
        h.editor.view_opts.splash, false, h.editor.prompt.is_some());
    assert!(!show, "a pending recovery prompt suppresses the splash");
    h.render();
    assert!(h.screen_contains("Recovery file found"),
        "the recovery prompt is what the user sees:\n{:#?}", h.screen());
}
```

- [ ] **Step 7: Run the journeys**

Run: `cargo test -p wordcartel -- e2e_splash e2e_no_splash e2e_recovery_prompt_pending`
Expected: PASS (4 tests — the production paths landed in Tasks 3-4 and Step 5; these journeys pin the composed behavior. If any fails, the production code — not the test — is wrong; debug before proceeding).
Then: `cargo test -p wordcartel` — full crate green.

- [ ] **Step 8: Commit**

```bash
cd /home/jkeim/projects/groundwords
git add wordcartel/src/splash.rs wordcartel/src/app.rs wordcartel/src/e2e.rs
git commit -m "feat(splash): startup gate + run() wiring + e2e journeys (show/dismiss/flag/recovery)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: Commands — `splash_on` / `splash_off` / `toggle_splash`

**Files:**
- Modify: `wordcartel/src/registry.rs:495-497` (insert after the `toggle_status_line` block, before the `// Menu bar:` comment)
- Test: `wordcartel/src/registry.rs` (existing `mod tests` at `:749`)

**Interfaces:**
- Consumes: `Registry::{register, register_stateful}` (registry.rs:72/78), `MenuCategory::View`, `MenuMark::OnOff(bool)` (registry.rs:47), `CommandResult::Handled` (commands.rs:92), `Editor::set_splash(bool)` (Task 2), `Ctx` (registry.rs:26), test helpers `Editor::new_from_text`, `InlineExecutor`, `Z` clock (registry.rs:755).
- Produces: registered command ids `"splash_on"`, `"splash_off"` (labels `"Splash: On"`/`"Splash: Off"`, `menu: None`) and `"toggle_splash"` (label `"Startup Splash"`, `Some(MenuCategory::View)`, state `MenuMark::OnOff(view_opts.splash)`). Task 7's command-guard test asserts these ids exist.

- [ ] **Step 1: Write the failing tests** — append inside `mod tests` in `wordcartel/src/registry.rs`:

```rust
    #[test]
    fn splash_commands_registered_with_contract_shape() {
        let reg = Registry::builtins();
        let meta = |id: &str| reg.meta(reg.resolve_name(id).expect(id)).expect(id);
        assert_eq!(meta("splash_on").menu, None, "set-per-state primitives are palette-only");
        assert_eq!(meta("splash_off").menu, None, "set-per-state primitives are palette-only");
        let t = meta("toggle_splash");
        assert_eq!(t.menu, Some(MenuCategory::View), "toggle is the stateful View representative");
        let e = Editor::new_from_text("hi\n", None, (80, 24));
        assert_eq!((t.state.expect("stateful"))(&e), MenuMark::OnOff(true),
            "the OnOff mark mirrors the live option (default on)");
    }

    #[test]
    fn splash_commands_move_view_opts_through_set_splash() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
        assert!(ctx.editor.view_opts.splash, "default on");
        let r = reg.dispatch(CommandId("splash_off"), &mut ctx);
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert!(!ctx.editor.view_opts.splash);
        assert!(ctx.editor.status.contains("next launch"),
            "status notes the deferred effect: {}", ctx.editor.status);
        reg.dispatch(CommandId("toggle_splash"), &mut ctx);
        assert!(ctx.editor.view_opts.splash, "toggle flips back on");
        reg.dispatch(CommandId("splash_on"), &mut ctx);
        assert!(ctx.editor.view_opts.splash, "absolute set is idempotent");
        reg.dispatch(CommandId("toggle_splash"), &mut ctx);
        assert!(!ctx.editor.view_opts.splash, "toggle flips off");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel splash_commands`
Expected: FAIL — `meta("splash_on")` panics with `splash_on` (`resolve_name` → `None`; commands not registered).

- [ ] **Step 3: Implement.** In `Registry::builtins()` (`wordcartel/src/registry.rs`), after the `toggle_status_line` registration block (line 492-495) and before the `// Menu bar:` comment (line 497), insert:

```rust
        // Startup splash: set-per-state (palette-only) + 2-state toggle representative
        // (View, OnOff mark). All three route through Editor::set_splash (contract law 6);
        // the splash paints only at launch, so a change takes effect on the NEXT run.
        r.register("splash_on",  "Splash: On",  None, |c| { c.editor.set_splash(true);
            c.editor.status = "splash: on (takes effect next launch)".into(); CommandResult::Handled });
        r.register("splash_off", "Splash: Off", None, |c| { c.editor.set_splash(false);
            c.editor.status = "splash: off (takes effect next launch)".into(); CommandResult::Handled });
        r.register_stateful("toggle_splash", "Startup Splash", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.splash),
            |c| { let next = !c.editor.view_opts.splash; c.editor.set_splash(next);
                  c.editor.status = if next { "splash: on (takes effect next launch)".into() }
                                    else { "splash: off (takes effect next launch)".into() };
                  CommandResult::Handled });
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p wordcartel splash_commands`
Expected: PASS (2 tests). Then `cargo test -p wordcartel -- registry menu palette` — the registration-order, menu-window, and palette tests all stay green (the palette is exhaustive by construction; `toggle_splash` joins the View menu automatically).

- [ ] **Step 5: Commit**

```bash
cd /home/jkeim/projects/groundwords
git add wordcartel/src/registry.rs
git commit -m "feat(splash): splash_on/splash_off palette primitives + stateful toggle_splash View representative

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: Settings persistence — thread `view.splash` through the snapshot/diff-law machinery

**Files:**
- Modify: `wordcartel/src/settings.rs:37-57` (SettingsSnapshot), `:110-119` (OView), `:159-178` (snapshot_of), `:181-199` (runtime_snapshot), `:365-413` (compute_overrides view diff + `any_view` + final `OView` literal), `:552-562` (`snap` test helper), `:990-1023` (`every_persisted_setting_has_a_command` field guard + assertion)
- Test: `wordcartel/src/settings.rs` (existing `mod tests`)

**Interfaces:**
- Consumes: `SettingsSnapshot`, `OView`, `snapshot_of(cfg, resolved_theme_name)`, `runtime_snapshot(&Editor)`, `compute_overrides(runtime, baseline, existing, mask) -> OverridesFile`, `diff_key(rt, base, existing, masked)` (settings.rs:277), `parse_overrides(&str)` (settings.rs:207), `cfg.view.splash` / `editor.view_opts.splash` (Tasks 1-2), command ids `splash_on`/`splash_off`/`toggle_splash` (Task 6). `parse_overrides`/`parse_mask` need NO bespoke field — they deserialize `OView` directly (settings.rs:207/226-236), so adding the `OView` field covers them.
- Produces: `SettingsSnapshot.view_splash: bool`, `OView.splash: Option<bool>` — the complete Save-Settings round trip for `view.splash`.

- [ ] **Step 1: Write the failing test** — append inside `mod tests` in `wordcartel/src/settings.rs`:

```rust
    #[test]
    fn splash_round_trips_through_snapshot_diff_and_parse() {
        // snapshot_of reads the config default (on); runtime diverges to off.
        let baseline = snapshot_of(&crate::config::Config::default(), "tokyo-night");
        assert!(baseline.view_splash, "config default is on");
        let mut runtime = baseline.clone();
        runtime.view_splash = false;
        let of = compute_overrides(&runtime, &baseline,
            &OverridesFile::default(), &OverridesFile::default());
        assert_eq!(of.view.as_ref().and_then(|v| v.splash), Some(false),
            "divergence writes the key");
        // …and the written key deserializes back through parse_overrides.
        let text = toml::to_string(&of).expect("serialize overrides");
        let re = parse_overrides(&text);
        assert_eq!(re.view.and_then(|v| v.splash), Some(false));
        // No divergence → the key (and the empty section) stays absent (rule 4).
        let of2 = compute_overrides(&baseline, &baseline,
            &OverridesFile::default(), &OverridesFile::default());
        assert!(of2.view.is_none(), "unchanged splash writes no view key");
        // runtime_snapshot reads the live editor through view_opts (set_splash path).
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        e.set_splash(false);
        assert!(!runtime_snapshot(&e).view_splash);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p wordcartel splash_round_trips_through_snapshot_diff_and_parse`
Expected: COMPILE ERROR — `no field 'view_splash' on type 'SettingsSnapshot'` / `no field 'splash' on type 'OView'`.

- [ ] **Step 3: Implement — all seven sites** in `wordcartel/src/settings.rs`:

(a) `SettingsSnapshot` (line 37-57) — after `pub view_status_line: crate::config::TransientMode,`:

```rust
    pub view_status_line: crate::config::TransientMode,
    /// Startup splash enablement (`view.splash`). Persisted as a plain bool; a runtime
    /// change takes effect on the next launch.
    pub view_splash: bool,
```

(b) `OView` (line 110-119) — after the `status_line` field:

```rust
    #[serde(skip_serializing_if = "Option::is_none")] pub status_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub splash:      Option<bool>,
```

(c) `snapshot_of` (line 159-178) — after `view_status_line: cfg.view.status_line,`:

```rust
        view_status_line: cfg.view.status_line,
        view_splash:     cfg.view.splash,
```

(d) `runtime_snapshot` (line 181-199) — after `view_status_line: editor.status_line_mode,`:

```rust
        view_status_line: editor.status_line_mode,
        view_splash:     editor.view_opts.splash,
```

(e) `compute_overrides` view diff (after the `status_line` diff, line 404-409):

```rust
    let splash = diff_key(
        &runtime.view_splash, &baseline.view_splash,
        ex_view.and_then(|v| v.splash.as_ref()),
        mk_view.and_then(|v| v.splash).is_some(),
    );
```

(f) `any_view` + the final `OView` literal (lines 410-413) become:

```rust
    let any_view = typewriter.is_some() || focus.is_some() || measure.is_some()
        || wrap_guide.is_some() || word_count.is_some() || wrap_column.is_some()
        || scrollbar.is_some() || status_line.is_some() || splash.is_some();
    let view = some_if(OView { typewriter, focus, measure, wrap_guide, word_count, wrap_column, scrollbar, status_line, splash }, any_view);
```

(g) Test scaffolding + LAW 2 guard. The `snap` helper (line 552-562) gains the field — after `view_status_line: crate::config::TransientMode::On,`:

```rust
            view_status_line: crate::config::TransientMode::On,
            view_splash: true,
```

In `every_persisted_setting_has_a_command` (line 990-1023): the `field_guard` destructuring adds `view_splash: _,` (after `view_status_line: _,`), and the assertion list gains (after the `view_status_line` line):

```rust
        assert!(has("toggle_splash") && has("splash_on") && has("splash_off"), "view_splash");
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p wordcartel settings`
Expected: PASS — the new round-trip test, `every_persisted_setting_has_a_command` (now covering `view_splash`), and every pre-existing settings/diff-law test.
Then: `cargo test -p wordcartel` and `cargo test --workspace` — all suites green.

- [ ] **Step 5: Final whole-effort verification (merge-gate preflight)**

Run, in order, from `/home/jkeim/projects/groundwords`:
- `cargo test --workspace` — Expected: all green.
- `cargo build -p wordcartel && cargo test -p wordcartel --no-run` — Expected: zero warnings.
- `cargo clippy --workspace --all-targets` — Expected: clean (no new `too_many_lines`: `splash::paint` is ~50 lines, `intercept` ~15; if `Registry::builtins`/`config::load` trip anything it is pre-existing and already `#[allow]`ed).
- `cargo test -p wordcartel --test module_budgets` — Expected: PASS (app.rs gained ~8 production lines; budget 1000).
- `scripts/smoke/run.sh` — quote its one-line summary verbatim in the pre-merge report (e.g. `smoke: 8/8 PASS`); a red result is ADVISORY, surfaced to the human, never a block. NOTE: the splash changes the startup screen — if any smoke check asserts first-frame content, report the failure as advisory with that context.

- [ ] **Step 6: Commit**

```bash
cd /home/jkeim/projects/groundwords
git add wordcartel/src/settings.rs
git commit -m "feat(splash): persist view.splash through the settings snapshot/diff-law machinery

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage** (spec section → task):

| Spec section | Task(s) |
|---|---|
| §1 Behavior — shown iff config ∧ ¬flag ∧ ¬recovery-prompt; set after mem::take, before first draw | Task 5 (`show_at_startup` + run() wiring) |
| §1 Dismiss — first key press / mouse-down clears + consumes; Tick/background/Resize pass | Task 3 (`intercept` + reduce stage) |
| §1 Hybrid — identical with/without file; dismiss reveals buffer | Tasks 4-5 (painter is file-agnostic; e2e asserts reveal) |
| §2 Content — wordmark accent-bold, version, dim tagline/footer, 3 chord_for hints, omit unbound | Task 2 (model/resolution) + Task 4 (faces/layout) |
| §2 Layout — centered, degrade hints→tagline→version keeping wordmark, no panic, w<4/h<2 guard | Task 4 (painter + degradation/sweep tests) |
| §2 Canvas — full frame incl. status row, CanvasMode-respecting fill | Task 4 (`Clear` + base_canvas fill) |
| §3 State/module/lib.rs seam | Task 2 |
| §3 Interception at TOP of reduce, peer signature, Done(!editor.quit) | Task 3 |
| §3 Paint branch in render_overlays::paint | Task 4 |
| §3 Startup wiring in run() (loop-local keymap, env! version, prompt gate) | Task 5 |
| §3 Config (`ViewConfig.splash` default true, RawView + fold; seed via existing view_opts clone — no new code, stated) | Task 1 |
| §3 CLI (`no_splash` + parse arm) | Task 1 |
| §3 Commands (set-per-state palette-only + stateful View toggle, shared setter, next-launch status) | Task 6 (setter in Task 2) |
| §3 Settings persistence (all enumerated sites; parse_overrides/parse_mask need nothing) | Task 7 |
| §4 Contract conformance statement + invariant tests stay green | Global Constraints section + Tasks 6-7 tests |
| §5 Testing matrix (unit: gate/intercept/hints-omit CUA+WordStar/paint-degrade/commands/round-trip; e2e: 3 journeys; invariant test line; smoke advisory) | Tasks 1-7 test steps; smoke in Task 7 Step 5 |
| §6 Non-goals (no ASCII art / timeout / recent files / animation / first-run logic) | Respected — nothing in the plan adds them |
| Resolved decisions (3 hints, no help; unbound omitted) | Tasks 2, 4 |

Plan-level decisions within spec latitude (flag to reviewers, not deviations): the footer travels with the hints block in the degradation ladder (spec orders hints→tagline→version and leaves the footer unplaced); hint rows right-align chords to the widest chord with a 3-space gutter; the wordmark face is `compose([Text, Heading(1)]) + BOLD` (the theme's on-canvas accent) and dim is `Modifier::DIM` over the Text face.

**2. Placeholder scan:** No TBD/TODO/"similar to Task N"/"add error handling" anywhere; every code step shows complete, compilable code with real signatures verified against the branch (`Handled` has no `Debug` derive, so no test formats it; `Line::width()` confirmed present in the vendored ratatui-core 0.1.2; `toml::to_string` confirmed in use at settings.rs:479).

**3. Type consistency:** `Splash::new(&KeyTrie, &str) -> Splash`, `version() -> &str`, `hints() -> &[(String, &'static str)]` are defined in Task 2 and used identically in Tasks 3-5; `show_at_startup(bool, bool, bool) -> bool` defined Task 5 Step 3, used in run() and e2e with the same argument order (cfg, flag, prompt_pending); `Editor::set_splash(&mut self, on: bool)` defined Task 2, called in Task 6 handlers and Task 7's test; `intercept` matches the marks.rs peer shape with underscore-named unused params (keeps builds warning-free); `SettingsSnapshot.view_splash: bool` / `OView.splash: Option<bool>` consistent across all Task 7 sites.

**Anchor drift:** none — every spec anchor (app.rs:233/484/551-577/613/698-699; config.rs:7/16/22/117-139/306-319/400-405; editor.rs:411-445/656/838/847/885; keymap.rs:194; render_overlays.rs:37; render.rs:222/736; registry.rs:47/489-495/749; settings.rs:33/37/110/159/181/207/226/365/410-413/552/990) was re-verified on `effort-splash` @ 00c458c and matches.
