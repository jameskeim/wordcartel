# Wordcartel 5a — Config + Data-Driven Keymap + Session State Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Wordcartel configurable (layered TOML), rebindable (a data-driven multi-key keymap resolving through the 4b registry, with CUA default + bundled opt-in WordStar preset), and resumable (path-keyed session state restoring cursor/scroll, plus the marks store).

**Architecture:** Three new shell-crate modules — `config.rs` (typed `Config` + layered TOML load), `keymap.rs` (`KeyTrie` with multi-key sequence resolution + presets + patch-merge), `state.rs` (path-keyed session store with mtime+size staleness guard) — wired into `run()`/`reduce`. The registry gains name-based resolution so config strings map to `CommandId`s. `wordcartel-core` is untouched.

**Tech Stack:** Rust; `toml` + `serde`/`serde_derive` (config); `dirs` (already a dep, for `config_dir`); reuses `swap::state_dir`, `file::save_atomic_bytes`, the `Msg`/`reduce` loop, and the 4b `Registry`.

**Spec:** `docs/superpowers/specs/2026-06-24-wordcartel-05a-config-keymap-design.md` (Codex-reviewed: 2 crit + 6 imp + 3 min applied).

## Global Constraints

- `#![forbid(unsafe_code)]` in the shell crate; `wordcartel-core` stays IO/thread-free (config/keymap/state are shell concerns). New deps (`toml`, `serde`, `serde_derive`) added to `wordcartel/Cargo.toml` ONLY.
- **Degrade, never abort:** a missing/bad config or state file warns (status line) and falls back to defaults; the editor always starts and always edits. Never `unwrap` on config/state IO.
- **Resolve through the registry:** keymap chords map to `CommandId`s resolved/validated via `Registry::resolve_name`; the in-memory keymap never holds an id the registry doesn't know.
- **Esc precedence (exact):** prompt-dismiss > minibuffer-dismiss > pending-sequence-cancel > filter-cancel > normal keymap dispatch. Opening a prompt/minibuffer clears `pending_keys`. A pending sequence can only exist in normal mode.
- **Config keymap shape (serde-native):** per-layer raw `RawKeymap { preset: Option<String>, bind: BTreeMap<String,String>, unbind: Vec<String> }`; folded into `KeymapConfig { preset: String, patches: Vec<KeymapPatch> }` where `patches` is one ordered entry per layer. `preset` resolved across all layers first (last set wins) → base; then each layer's `bind`/`unbind` apply **in precedence order** (so a high layer's bind beats a low layer's unbind). State + preset merge **per-field** (an omitted field inherits the lower layer, never resets to default). Unknown keys silently ignored (forward-compat, no `deny_unknown_fields`).
- **Config precedence:** built-in `<` XDG `~/.config/wordcartel/config.toml` `<` project-local `.wordcartel.toml` (anchor = initial CLI file's parent dir, else CWD) `<` `--config <path>`. `--no-config` → built-ins only.
- **Session state:** path-keyed (canonical abs path) `{cursor,scroll,marks,mtime,size}` in the XDG state dir; restore only if mtime+size match (else discard); debounced write on save/quit (not per keystroke); LRU prune at `max_entries` (default 200); scratch buffers not persisted; atomic write via `save_atomic_bytes`.
- `cargo build --workspace` zero warnings; not-yet-wired items carry scoped `#[allow(dead_code)]` with a `// wired in Task N` note. No prior test weakened.

---

## File Structure

- **Create:** `wordcartel/src/config.rs`, `wordcartel/src/keymap.rs`, `wordcartel/src/state.rs`; declare each `pub mod` in `lib.rs`.
- **Modify:** `wordcartel/Cargo.toml` (deps), `wordcartel/src/registry.rs` (`Borrow<str>` + `resolve_name`), `wordcartel/src/input.rs` (`KeyChord` + retire/redirect legacy path), `wordcartel/src/app.rs` (`reduce` gains `&KeyTrie`; `pending_keys`; Esc precedence; pending status; `run()` loads config/state, builds keymap, resumes, persists), `wordcartel/src/editor.rs` (`pending_keys` field; resume helpers), `wordcartel/src/main.rs` (CLI parse).

---

### Task 1: Registry name resolution (`Borrow<str>` + `resolve_name`)

**Files:**
- Modify: `wordcartel/src/registry.rs`
- Test: `wordcartel/src/registry.rs`

**Interfaces:**
- Produces: `impl std::borrow::Borrow<str> for CommandId`; `Registry::resolve_name(&self, name: &str) -> Option<CommandId>`.

- [ ] **Step 1: Write the failing test** in `registry.rs`:
```rust
    #[test]
    fn resolve_name_recovers_static_command_id() {
        let reg = Registry::builtins();
        assert_eq!(reg.resolve_name("cut"), Some(CommandId("cut")));
        assert_eq!(reg.resolve_name("save"), Some(CommandId("save")));
        assert_eq!(reg.resolve_name("definitely-not-a-command"), None);
    }
```
(Confirm `"cut"`/`"save"` are real builtin ids; substitute two ids that exist in `builtins()` if the names differ.)

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib registry::` → FAIL (`resolve_name` missing).

- [ ] **Step 3: Implement** in `registry.rs`:
```rust
impl std::borrow::Borrow<str> for CommandId {
    fn borrow(&self) -> &str { self.0 }
}

impl Registry {
    /// Resolve a runtime command-id string to the registry's stored `CommandId`
    /// (which wraps a `&'static str`) — without allocating or leaking. Returns
    /// None if no command with that name is registered.
    pub fn resolve_name(&self, name: &str) -> Option<CommandId> {
        self.map.get_key_value(name).map(|(id, _)| *id)
    }
}
```
(`HashMap<CommandId, Handler>::get_key_value::<str>(name)` works because `CommandId: Borrow<str>` and `CommandId`'s derived `Hash`/`Eq` hash/compare the inner string — consistent with `str`'s, satisfying the `Borrow` contract.)

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib registry::` → pass; `cargo test --workspace` green; `cargo build --workspace` zero warnings. (`resolve_name` is unused until Task 3 → `#[allow(dead_code)] // wired in Task 3`.)

- [ ] **Step 5: Commit.**
```bash
git add wordcartel/src/registry.rs
git commit -m "feat(registry): Borrow<str> for CommandId + resolve_name(&str) for config-driven dispatch"
```

---

### Task 2: Config substrate + CLI parser

**Files:**
- Modify: `wordcartel/Cargo.toml`, `wordcartel/src/lib.rs`
- Create: `wordcartel/src/config.rs`
- Test: `wordcartel/src/config.rs`

**Interfaces:**
- Produces:
  - `pub struct Cli { pub path: Option<PathBuf>, pub config_path: Option<PathBuf>, pub no_config: bool }` + `pub fn parse_cli<I: IntoIterator<Item = String>>(args: I) -> Cli` (skips argv[0]).
  - `pub struct Config { pub keymap: KeymapConfig, pub state: StateConfig }` (resolved/folded result).
  - `pub struct KeymapConfig { pub preset: String, pub patches: Vec<KeymapPatch> }` — `preset` is the final-merged base name (default `"cua"`); `patches` is the **ordered** (lowest→highest precedence) per-layer patch list, applied in order by `build_keymap` (Task 3). **This preserves cross-layer precedence** — a low layer's `unbind` can't clobber a high layer's later `bind`.
  - `pub struct KeymapPatch { pub bind: BTreeMap<String,String>, pub unbind: Vec<String> }` (one layer's chord changes).
  - `pub struct StateConfig { pub resume: bool, pub max_entries: usize }` (defaults: `resume=true`, `max_entries=200`).
  - Internal raw types (per-layer deserialize, all fields optional so an omitted key inherits rather than resets): `RawConfig { keymap: RawKeymap, state: RawState }`, `RawKeymap { preset: Option<String>, bind: BTreeMap<String,String>, unbind: Vec<String> }`, `RawState { resume: Option<bool>, max_entries: Option<usize> }`.
  - `pub fn config_layer_paths(cli: &Cli, xdg_config_dir: Option<&Path>, anchor_dir: &Path) -> Vec<PathBuf>` — the ordered EXISTING config files (XDG, project-local `.wordcartel.toml` nearest walking up from `anchor_dir`, `--config`), or empty when `cli.no_config`.
  - `pub fn load(paths: &[PathBuf]) -> (Config, Vec<String>)` — parse each layer (lowest→highest), **per-field** fold (a field is overridden only when the layer sets it), returning warnings.

- [ ] **Step 1: Add deps + module.** In `wordcartel/Cargo.toml` `[dependencies]`:
```toml
serde = { version = "1", features = ["derive"] }
toml = "0.8"
```
In `lib.rs` add `pub mod config;`. Run `cargo build -p wordcartel` to confirm they resolve. (Confirm `toml` 0.8.x is current; adjust if a newer line is standard.)

- [ ] **Step 2: Write failing tests** in `config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn parse_cli_separates_path_config_and_noconfig() {
        let c = parse_cli(["wcartel", "notes.md"].map(String::from));
        assert_eq!(c.path.as_deref(), Some(std::path::Path::new("notes.md")));
        assert!(c.config_path.is_none() && !c.no_config);

        let c = parse_cli(["wcartel", "--config", "my.toml", "notes.md"].map(String::from));
        assert_eq!(c.config_path.as_deref(), Some(std::path::Path::new("my.toml")));
        assert_eq!(c.path.as_deref(), Some(std::path::Path::new("notes.md")));

        let c = parse_cli(["wcartel", "--no-config"].map(String::from));
        assert!(c.no_config && c.path.is_none());
    }

    #[test]
    fn later_layers_override_per_field_and_keep_ordered_patches() {
        let d = tempdir();
        // lo sets BOTH state fields + a bind; hi sets ONLY max_entries (omits resume) + preset + a bind.
        let lo = write(&d, "lo.toml", "[state]\nresume=false\nmax_entries=50\n[keymap]\npreset='cua'\nbind={ \"ctrl-a\"='move_line_start' }\n");
        let hi = write(&d, "hi.toml", "[state]\nmax_entries=99\n[keymap]\npreset='wordstar'\nbind={ \"ctrl-b\"='move_left' }\n");
        let (cfg, warns) = load(&[lo, hi]);
        assert!(warns.is_empty());
        assert_eq!(cfg.state.max_entries, 99, "hi set it → wins");
        assert_eq!(cfg.state.resume, false, "hi OMITTED resume → lo's false is preserved (NOT reset to default true)");
        assert_eq!(cfg.keymap.preset, "wordstar", "final-merged preset");
        assert_eq!(cfg.keymap.patches.len(), 2, "one ordered patch per layer");
        assert!(cfg.keymap.patches[0].bind.contains_key("ctrl-a"));
        assert!(cfg.keymap.patches[1].bind.contains_key("ctrl-b"));
    }

    #[test]
    fn defaults_when_no_layers() {
        let (cfg, warns) = load(&[]);
        assert!(warns.is_empty());
        assert!(cfg.state.resume);
        assert_eq!(cfg.state.max_entries, 200);
        assert_eq!(cfg.keymap.preset, "cua");
        assert!(cfg.keymap.patches.is_empty());
    }

    #[test]
    fn malformed_toml_warns_and_skips_layer() {
        let d = tempdir();
        let bad = write(&d, "bad.toml", "[state]\nmax_entries = = =\n");
        let (cfg, warns) = load(&[bad]);
        assert_eq!(warns.len(), 1, "one warning for the bad layer");
        assert_eq!(cfg.state.max_entries, 200, "fell back to default");
    }

    // tiny temp-dir helper (unique; avoids real $HOME)
    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!("wc-cfg-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
```

- [ ] **Step 3: Run to verify failure.** `cargo test -p wordcartel --lib config::tests` → FAIL (items missing).

- [ ] **Step 4: Implement** `config.rs`:
```rust
//! Layered TOML config + CLI parsing. Built-in defaults < XDG < project-local < --config.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use serde::Deserialize;

#[derive(Debug, Default, Clone)]
pub struct Cli { pub path: Option<PathBuf>, pub config_path: Option<PathBuf>, pub no_config: bool }

/// Hand-rolled (no clap dep): `[--config <path>] [--no-config] [file]`.
pub fn parse_cli<I: IntoIterator<Item = String>>(args: I) -> Cli {
    let mut cli = Cli::default();
    let mut it = args.into_iter();
    let _ = it.next(); // argv[0]
    while let Some(a) = it.next() {
        match a.as_str() {
            "--no-config" => cli.no_config = true,
            "--config" => cli.config_path = it.next().map(PathBuf::from),
            _ => if cli.path.is_none() { cli.path = Some(PathBuf::from(a)); },
        }
    }
    cli
}

// --- Resolved (folded) config the rest of the app consumes ---
#[derive(Debug, Default, Clone)]
pub struct Config { pub keymap: KeymapConfig, pub state: StateConfig }

#[derive(Debug, Clone)]
pub struct KeymapConfig { pub preset: String, pub patches: Vec<KeymapPatch> }
impl Default for KeymapConfig { fn default() -> Self { KeymapConfig { preset: "cua".into(), patches: Vec::new() } } }

#[derive(Debug, Clone, Default)]
pub struct KeymapPatch { pub bind: BTreeMap<String, String>, pub unbind: Vec<String> }

#[derive(Debug, Clone)]
pub struct StateConfig { pub resume: bool, pub max_entries: usize }
impl Default for StateConfig { fn default() -> Self { StateConfig { resume: true, max_entries: 200 } } }

// --- Raw per-layer deserialize: every field optional so an OMITTED key inherits
//     the lower layer rather than resetting it to a default (Codex plan-review fix) ---
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig { keymap: RawKeymap, state: RawState }
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawKeymap { preset: Option<String>, bind: BTreeMap<String, String>, unbind: Vec<String> }
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawState { resume: Option<bool>, max_entries: Option<usize> }

/// Ordered existing config files, lowest→highest precedence. Empty when --no-config.
pub fn config_layer_paths(cli: &Cli, xdg_config_dir: Option<&Path>, anchor_dir: &Path) -> Vec<PathBuf> {
    if cli.no_config { return Vec::new(); }
    let mut v = Vec::new();
    if let Some(x) = xdg_config_dir {
        let p = x.join("wordcartel").join("config.toml");
        if p.is_file() { v.push(p); }
    }
    // project-local: nearest .wordcartel.toml walking up from anchor_dir
    let mut dir = Some(anchor_dir);
    while let Some(d) = dir {
        let p = d.join(".wordcartel.toml");
        if p.is_file() { v.push(p); break; }
        dir = d.parent();
    }
    if let Some(c) = &cli.config_path {
        if c.is_file() { v.push(c.clone()); }
        // (a missing --config path is surfaced as a warning by the caller in Task 5)
    }
    v
}

/// Parse + fold layers (lowest→highest precedence) into a resolved Config.
/// PER-FIELD merge: `preset` & each `state` field override only when the layer
/// SETS them (Option); `patches` keeps one ordered entry per layer so
/// build_keymap applies them in precedence order (Codex plan-review fix).
pub fn load(paths: &[PathBuf]) -> (Config, Vec<String>) {
    let mut cfg = Config::default();
    let mut warns = Vec::new();
    for p in paths {
        let text = match std::fs::read_to_string(p) {
            Ok(t) => t,
            Err(e) => { warns.push(format!("config: cannot read {}: {e}", p.display())); continue; }
        };
        let raw: RawConfig = match toml::from_str(&text) {
            Ok(r) => r,
            Err(e) => { warns.push(format!("config: parse error in {}: {e}", p.display())); continue; }
        };
        // keymap: preset overrides only if set; each layer contributes ONE ordered patch.
        if let Some(p) = raw.keymap.preset { cfg.keymap.preset = p; }
        cfg.keymap.patches.push(KeymapPatch { bind: raw.keymap.bind, unbind: raw.keymap.unbind });
        // state: per-field override (omitted field inherits the lower layer).
        if let Some(r) = raw.state.resume { cfg.state.resume = r; }
        if let Some(m) = raw.state.max_entries { cfg.state.max_entries = m; }
    }
    (cfg, warns)
}
```
**Implementer notes:** (a) Parsing into `RawConfig` (Option fields) is what makes per-field merge possible — `#[serde(default)]` alone can't distinguish "set to the default value" from "omitted". (b) An empty patch (a layer with no `bind`/`unbind`) is harmless in the `patches` vec; build_keymap skips empties. (c) `toml = "0.8"` + `serde` derive; confirm versions.

- [ ] **Step 5: Run tests + suite.** `cargo test -p wordcartel --lib config::tests` → pass; `cargo test --workspace` green; zero warnings. (`config_layer_paths`/`Cli`/`Config` unused in production until Task 5 → scoped `#[allow(dead_code)] // wired in Task 5`.)

- [ ] **Step 6: Commit.**
```bash
git add wordcartel/Cargo.toml wordcartel/src/lib.rs wordcartel/src/config.rs
git commit -m "feat(config): layered TOML config (precedence/merge) + CLI parser (--config/--no-config)"
```

---

### Task 3: Keymap engine (`keymap.rs`)

**Files:**
- Modify: `wordcartel/src/lib.rs`
- Create: `wordcartel/src/keymap.rs`
- Test: `wordcartel/src/keymap.rs`

**Interfaces:**
- Consumes: `config::KeymapConfig` (Task 2), `registry::{Registry, CommandId}` + `resolve_name` (Task 1).
- Produces:
  - `pub struct KeyChord { pub code: crossterm::event::KeyCode, pub mods: crossterm::event::KeyModifiers }` + `pub fn from_key_event(k: crossterm::event::KeyEvent) -> Option<KeyChord>` (None unless `kind == Press`) + `pub fn parse_chord(s: &str) -> Option<KeyChord>` + `pub fn parse_seq(s: &str) -> Option<Vec<KeyChord>>` (space-separated).
  - `pub enum Resolution { Command(CommandId), Pending, None }`.
  - `pub struct KeyTrie { /* normal-mode trie */ }` + `pub fn resolve(&self, pending: &[KeyChord]) -> Resolution`.
  - `pub fn build_keymap(km: &config::KeymapConfig, reg: &Registry) -> (KeyTrie, Vec<String>)` — preset base (`km.preset` or `"cua"`) → apply `bind` (parse seq + `resolve_name` → insert) → apply `unbind` (remove); warnings for unknown preset / bad chord / unknown command-id.
  - `pub fn preset_bindings(name: &str) -> Option<&'static [(&'static str, &'static str)]>` — the `cua` and `wordstar` tables (chord-seq, command-id).

- [ ] **Step 1: Write failing tests** in `keymap.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Registry, CommandId};
    use crossterm::event::{KeyCode, KeyModifiers};

    fn km(bind: &[(&str,&str)], unbind: &[&str], preset: Option<&str>) -> (KeyTrie, Vec<String>) {
        let cfg = crate::config::KeymapConfig {
            preset: preset.unwrap_or("cua").to_string(),
            patches: vec![crate::config::KeymapPatch {
                bind: bind.iter().map(|(k,v)| (k.to_string(), v.to_string())).collect(),
                unbind: unbind.iter().map(|s| s.to_string()).collect(),
            }],
        };
        build_keymap(&cfg, &Registry::builtins())
    }

    #[test]
    fn cross_layer_high_bind_beats_low_unbind() {
        // CRITICAL fix: low layer unbinds ctrl-c, high layer re-binds it → bound (high wins).
        let cfg = crate::config::KeymapConfig {
            preset: "cua".into(),
            patches: vec![
                crate::config::KeymapPatch { bind: Default::default(), unbind: vec!["ctrl-c".into()] }, // low
                crate::config::KeymapPatch { bind: [("ctrl-c".to_string(), "copy".to_string())].into_iter().collect(), unbind: vec![] }, // high
            ],
        };
        let (t, _) = build_keymap(&cfg, &Registry::builtins());
        let c = parse_chord("ctrl-c").unwrap();
        assert!(matches!(t.resolve(&[c]), Resolution::Command(CommandId("copy"))), "high-layer bind beats low-layer unbind");
    }

    #[test]
    fn shift_char_normalizes_identically_in_event_and_config() {
        use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState};
        // crossterm delivers a shifted letter as Char('Z') + SHIFT (+ CONTROL).
        let ev = KeyEvent { code: KeyCode::Char('Z'), modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            kind: KeyEventKind::Press, state: KeyEventState::NONE };
        let from_ev = from_key_event(ev).unwrap();
        let from_cfg = parse_chord("ctrl-shift-z").unwrap();
        assert_eq!(from_ev, from_cfg, "event + config chord must normalize the same way");
        assert_eq!(from_cfg, KeyChord { code: KeyCode::Char('Z'), mods: KeyModifiers::CONTROL });
    }

    #[test]
    fn parse_and_resolve_single_chord() {
        let (t, w) = km(&[], &[], Some("cua"));
        assert!(w.is_empty());
        let cut = parse_chord("ctrl-x").unwrap();
        assert!(matches!(t.resolve(&[cut]), Resolution::Command(CommandId("cut"))));
    }

    #[test]
    fn multi_key_sequence_is_pending_then_command() {
        let (t, _) = km(&[("ctrl-k ctrl-s", "save")], &[], Some("cua"));
        let k = parse_chord("ctrl-k").unwrap();
        let s = parse_chord("ctrl-s").unwrap();
        assert!(matches!(t.resolve(&[k]), Resolution::Pending));        // prefix
        assert!(matches!(t.resolve(&[k, s]), Resolution::Command(CommandId("save"))));
    }

    #[test]
    fn unknown_sequence_resolves_none() {
        let (t, _) = km(&[], &[], Some("cua"));
        let z = KeyChord { code: KeyCode::Char('z'), mods: KeyModifiers::ALT };
        assert!(matches!(t.resolve(&[z]), Resolution::None));
    }

    #[test]
    fn bind_overrides_and_unbind_removes() {
        let (t, w) = km(&[("ctrl-x", "copy")], &["ctrl-c"], Some("cua"));
        assert!(w.is_empty());
        let x = parse_chord("ctrl-x").unwrap();
        let c = parse_chord("ctrl-c").unwrap();
        assert!(matches!(t.resolve(&[x]), Resolution::Command(CommandId("copy"))), "rebound");
        assert!(matches!(t.resolve(&[c]), Resolution::None), "unbound");
    }

    #[test]
    fn unknown_command_id_is_dropped_with_warning() {
        let (t, w) = km(&[("ctrl-j", "no-such-command")], &[], Some("cua"));
        assert_eq!(w.len(), 1);
        let j = parse_chord("ctrl-j").unwrap();
        assert!(matches!(t.resolve(&[j]), Resolution::None));
    }

    #[test]
    fn from_key_event_ignores_non_press() {
        use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState};
        let rel = KeyEvent { code: KeyCode::Char('a'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release, state: KeyEventState::NONE };
        assert!(from_key_event(rel).is_none());
    }

    #[test]
    fn both_presets_resolve_against_builtins() {
        let reg = Registry::builtins();
        for preset in ["cua", "wordstar"] {
            for (chord, id) in preset_bindings(preset).unwrap() {
                assert!(parse_seq(chord).is_some(), "preset {preset} bad chord {chord}");
                assert!(reg.resolve_name(id).is_some(), "preset {preset} id {id} not in registry");
            }
        }
    }
}
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib keymap::tests` → FAIL.

- [ ] **Step 3: Implement** `keymap.rs` + `pub mod keymap;` in lib.rs. Sketch (fill in completely):
```rust
use std::collections::HashMap;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crate::registry::{Registry, CommandId};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyChord { pub code: KeyCode, pub mods: KeyModifiers }

pub fn from_key_event(k: KeyEvent) -> Option<KeyChord> {
    if k.kind != KeyEventKind::Press { return None; }
    Some(normalize(k.code, k.modifiers))
}

/// Normalize a (code, mods) so config chords and live events agree (Codex fix):
/// for a char, SHIFT is folded into the char itself (uppercase) and the SHIFT
/// bit is dropped; CONTROL/ALT are kept. crossterm delivers a shifted letter as
/// `Char('Z') + SHIFT`, so we uppercase + strip SHIFT on BOTH sides.
fn normalize(code: KeyCode, mods: KeyModifiers) -> KeyChord {
    match code {
        KeyCode::Char(c) => {
            let c = if mods.contains(KeyModifiers::SHIFT) { c.to_ascii_uppercase() } else { c };
            let mut m = mods; m.remove(KeyModifiers::SHIFT);
            KeyChord { code: KeyCode::Char(c), mods: m }
        }
        other => KeyChord { code: other, mods },
    }
}

pub fn parse_chord(s: &str) -> Option<KeyChord> {
    let mut mods = KeyModifiers::NONE;
    let parts: Vec<&str> = s.split('-').collect();
    let (mod_parts, key) = parts.split_at(parts.len().saturating_sub(1));
    for m in mod_parts {
        match *m { "ctrl" => mods |= KeyModifiers::CONTROL, "alt" => mods |= KeyModifiers::ALT,
                   "shift" => mods |= KeyModifiers::SHIFT, _ => return None }
    }
    let code = match *key.first()? {
        "enter" => KeyCode::Enter, "tab" => KeyCode::Tab, "esc" => KeyCode::Esc,
        "space" => KeyCode::Char(' '), "backspace" => KeyCode::Backspace,
        "left" => KeyCode::Left, "right" => KeyCode::Right, "up" => KeyCode::Up, "down" => KeyCode::Down,
        "\\" => KeyCode::Char('\\'),
        f if f.starts_with('f') && f[1..].parse::<u8>().is_ok() => KeyCode::F(f[1..].parse().unwrap()),
        c if c.chars().count() == 1 => KeyCode::Char(c.chars().next().unwrap()),
        _ => return None,
    };
    Some(normalize(code, mods)) // same normalization as from_key_event (uppercase+strip SHIFT for chars)
}

pub fn parse_seq(s: &str) -> Option<Vec<KeyChord>> {
    s.split_whitespace().map(parse_chord).collect()
}

pub enum Resolution { Command(CommandId), Pending, None }

#[derive(Debug, Clone, Default)]
pub struct KeyTrie { map: HashMap<Vec<KeyChord>, CommandId> } // exact-seq → cmd
// (Debug+Clone required because `Editor` derives them and stores a KeyTrie — Task 5.)

impl KeyTrie {
    fn insert(&mut self, seq: Vec<KeyChord>, id: CommandId) { self.map.insert(seq, id); }
    fn remove(&mut self, seq: &[KeyChord]) { self.map.remove(seq); }
    pub fn resolve(&self, pending: &[KeyChord]) -> Resolution {
        if let Some(id) = self.map.get(pending) { return Resolution::Command(*id); }
        // prefix? any binding strictly longer that starts with `pending`
        if self.map.keys().any(|k| k.len() > pending.len() && k.starts_with(pending)) {
            return Resolution::Pending;
        }
        Resolution::None
    }
}

pub fn preset_bindings(name: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match name {
        "cua" => Some(CUA), "wordstar" => Some(WORDSTAR), _ => None,
    }
}

// The current CUA defaults, expressed as data. (Mirror input::key_to_command_id exactly.)
static CUA: &[(&str, &str)] = &[
    ("ctrl-c","copy"), ("ctrl-x","cut"), ("ctrl-v","paste"), ("ctrl-s","save"),
    ("ctrl-z","undo"), ("ctrl-y","redo"), /* … the full current id-map … */
];
// WordStar two-key families mapped onto existing command ids (best-effort over v1 commands).
static WORDSTAR: &[(&str, &str)] = &[
    ("ctrl-k ctrl-s","save"), /* ("ctrl-q ctrl-f","find") when search lands in 5e … */
    /* the subset that maps to commands that exist today */
];

pub fn build_keymap(km: &crate::config::KeymapConfig, reg: &Registry) -> (KeyTrie, Vec<String>) {
    let mut warns = Vec::new();
    let mut trie = KeyTrie::default();
    // 1) Preset base (the final-merged preset name was resolved across all layers in config::load).
    let base = preset_bindings(&km.preset).unwrap_or_else(|| {
        warns.push(format!("config: unknown keymap.preset '{}', using 'cua'", km.preset));
        preset_bindings("cua").unwrap()
    });
    let apply_bind = |chord: &str, id_str: &str, warns: &mut Vec<String>, trie: &mut KeyTrie| {
        let Some(seq) = parse_seq(chord) else { warns.push(format!("config: bad chord '{chord}'")); return; };
        let Some(id) = reg.resolve_name(id_str) else { warns.push(format!("config: '{chord}' → unknown command '{id_str}'")); return; };
        trie.insert(seq, id);
    };
    for (chord, id) in base { apply_bind(chord, id, &mut warns, &mut trie); } // preset-integrity test guarantees no warns here
    // 2) Apply each layer's patch IN PRECEDENCE ORDER (lowest→highest) so a high
    //    layer's bind overrides a low layer's bind/unbind (Codex CRITICAL fix).
    for patch in &km.patches {
        for (chord, id) in &patch.bind { apply_bind(chord, id, &mut warns, &mut trie); }
        for chord in &patch.unbind {
            match parse_seq(chord) { Some(seq) => trie.remove(&seq), None => warns.push(format!("config: bad unbind chord '{chord}'")) }
        }
    }
    (trie, warns)
}
```
**Implementer notes:** (a) Fill `CUA` to mirror EVERY binding in today's `input::key_to_command_id` (so behavior is unchanged) — read it and transcribe; the `both_presets_resolve_against_builtins` test will catch any id typo. (b) `WORDSTAR` includes only sequences whose command-ids exist in v1 (it grows in later sub-efforts). (c) The trie is a flat `HashMap<Vec<KeyChord>, CommandId>` with a prefix scan for `Pending` — fine for the small keymap; a real prefix-tree is an optional optimization, not required. (d) `KeyTrie` carries no mode dimension yet in code, but `build_keymap`/`resolve` are the only consumers, so adding a `mode` later is non-breaking (spec §4.3).

- [ ] **Step 4: Run tests + suite.** `cargo test -p wordcartel --lib keymap::tests` → pass; `cargo test --workspace` green; zero warnings. (`build_keymap`/`KeyTrie` unused in production until Task 4/5 → scoped `#[allow(dead_code)]`.)

- [ ] **Step 5: Commit.**
```bash
git add wordcartel/src/lib.rs wordcartel/src/keymap.rs
git commit -m "feat(keymap): KeyTrie multi-key resolution + cua/wordstar presets + patch-merge over registry"
```

---

### Task 4: Keymap → `reduce` integration (pending sequences, Esc precedence, dispatch)

**Files:**
- Modify: `wordcartel/src/editor.rs` (`pending_keys`), `wordcartel/src/app.rs` (`reduce` gains `&KeyTrie`; resolution; Esc precedence; pending status), `wordcartel/src/input.rs` (retire/redirect legacy path)
- Test: `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `keymap::{KeyTrie, KeyChord, Resolution, from_key_event}`, `registry::dispatch`, `keymap::build_keymap` (for a test keymap).
- Produces:
  - `Editor.pending_keys: Vec<keymap::KeyChord>` (init empty).
  - `reduce` gains a trailing-ish `keymap: &keymap::KeyTrie` param (placed right after `reg`): `reduce(msg, editor, reg, keymap, ex, clock, msg_tx)`. ALL call sites (production + tests) updated.
  - Normal-mode key handling resolves through `keymap` (replacing `input::key_to_command_id` dispatch), with the Esc precedence + pending status.

- [ ] **Step 1: Write failing tests** in `app.rs` (use a real CUA keymap built from defaults):
```rust
    fn cua_keymap() -> crate::keymap::KeyTrie {
        let (t, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        t
    }
    fn press(code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) -> Msg {
        use crossterm::event::{Event, KeyEvent, KeyEventKind, KeyEventState};
        Msg::Input(Event::Key(KeyEvent { code, modifiers: mods, kind: KeyEventKind::Press, state: KeyEventState::NONE }))
    }

    #[test]
    fn single_chord_dispatches_via_keymap() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3); // select abc
        let km = cua_keymap(); let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.register.get(), Some("abc"), "Ctrl+C copied via the data-driven keymap");
    }

    #[test]
    fn pending_sequence_then_completes() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        // bind a 2-key save sequence
        let cfg = crate::config::KeymapConfig { preset: "cua".into(),
            patches: vec![crate::config::KeymapPatch {
                bind: [("ctrl-k ctrl-s".to_string(), "save".to_string())].into_iter().collect(), unbind: vec![] }] };
        let (km, _) = crate::keymap::build_keymap(&cfg, &Registry::builtins());
        let mut e = Editor::new_from_text("x\n", Some("/tmp/wc-kmtest.md".into()), (80, 24));
        let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('k'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.pending_keys.len(), 1, "first key is pending");
        assert!(e.status.contains("ctrl-k") || e.status.to_lowercase().contains("k"), "pending shown");
        crate::app::reduce(press(KeyCode::Char('s'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.pending_keys.is_empty(), "sequence resolved, pending cleared");
        // (save dispatched — the file path means dispatch_save runs; assert via status or saved flag per the real save)
    }

    #[test]
    fn esc_cancels_pending_without_other_effect() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        let cfg = crate::config::KeymapConfig { preset: "cua".into(),
            patches: vec![crate::config::KeymapPatch {
                bind: [("ctrl-k ctrl-s".to_string(), "save".to_string())].into_iter().collect(), unbind: vec![] }] };
        let (km, _) = crate::keymap::build_keymap(&cfg, &Registry::builtins());
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        let before = e.active().document.buffer.to_string();
        let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('k'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.pending_keys.len(), 1);
        crate::app::reduce(press(KeyCode::Esc, KeyModifiers::NONE), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.pending_keys.is_empty(), "Esc cleared the pending sequence");
        assert_eq!(e.active().document.buffer.to_string(), before, "no buffer change");
    }

    #[test]
    fn printable_falls_through_to_insert() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut e = Editor::new_from_text("", None, (80, 24));
        let km = cua_keymap(); let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('h'), KeyModifiers::NONE), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "h", "unbound printable inserts literally");
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib single_chord_dispatches pending_sequence esc_cancels_pending printable_falls_through` → FAIL (reduce signature/keymap path missing).

- [ ] **Step 3: Add `Editor.pending_keys`** (`editor.rs`): `pub pending_keys: Vec<crate::keymap::KeyChord>,` init `Vec::new()` in `new_from_text`.

- [ ] **Step 4: Thread `keymap: &KeyTrie` through `reduce`** and replace the normal-mode key path. In `app.rs`:
  - Change `reduce` signature to `pub fn reduce(msg, editor, reg, keymap: &crate::keymap::KeyTrie, ex, clock, msg_tx) -> bool`. To keep this task compiling standalone (before Task 5 adds config-driven startup), have `run()` build a **local default keymap** and pass `&keymap`: `let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg); /* loop: */ reduce(msg, &mut editor, &reg, &keymap, &executor, &clock, &msg_tx)`. (Task 5 replaces this local with the config-loaded keymap.) Update EVERY test call site (mechanical — add `&km`/`cua_keymap()` after `reg`).
  - In the **normal-mode** key arm (the one that currently calls `input::key_to_command_id` → `reg.dispatch`), replace with:
```rust
        Msg::Input(Event::Key(k)) if k.kind == crossterm::event::KeyEventKind::Press => {
            // Esc precedence (Codex CRITICAL): prompt/minibuffer Esc are handled in their
            // interception blocks ABOVE this point. Here in normal mode the order is
            // pending-cancel > filter-cancel. This arm SUBSUMES the old standalone
            // filter-cancel Esc check (remove that separate check — it must NOT run before
            // pending-cancel). Esc is reserved for cancel/dismiss in v1 (not routed to the keymap).
            if k.code == crossterm::event::KeyCode::Esc {
                if !editor.pending_keys.is_empty() {
                    editor.pending_keys.clear();
                    editor.status.clear(); // drop the pending indicator
                } else if editor.filter_in_flight.is_some() {
                    editor.filter_in_flight.take().unwrap().cancel(); // existing filter-cancel, now sequenced AFTER pending-cancel
                    editor.status = "cancelling…".into();
                }
            } else if let Some(chord) = crate::keymap::from_key_event(k) {
                editor.pending_keys.push(chord);
                match keymap.resolve(&editor.pending_keys) {
                    crate::keymap::Resolution::Command(id) => {
                        editor.pending_keys.clear(); editor.status.clear();
                        let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
                        reg.dispatch(id, &mut ctx);
                    }
                    crate::keymap::Resolution::Pending => {
                        editor.status = format!("{} …", chords_display(&editor.pending_keys));
                    }
                    crate::keymap::Resolution::None => {
                        let was_single = editor.pending_keys.len() == 1;
                        editor.pending_keys.clear(); editor.status.clear();
                        // printable fallthrough: single unmodified printable → literal insert
                        if was_single {
                            if let crossterm::event::KeyCode::Char(c) = k.code {
                                if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                                   && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) {
                                    // reuse the EXACT existing insert-char path (pending already cleared above)
                                    crate::commands::run(crate::commands::Command::InsertChar(c), editor, clock);
                                }
                            }
                        }
                    }
                }
            }
        }
```
**Implementer notes:** (a) `chords_display(&[KeyChord]) -> String` renders the pending prefix (e.g. `"ctrl-k"`); add a small helper in keymap.rs (inverse of `parse_chord`, good enough for the indicator). (b) The printable fallthrough is **confirmed** (Codex): today an unbound printable → `KeyAction::Insert(c)` → `Command::InsertChar(c)` via `commands::run`, which does the rebuild + ensure-visible + `desired_col` reset; the sketch's `commands::run(Command::InsertChar(c), editor, clock)` reuses that exact path. (c) **Esc precedence (CRITICAL):** this normal-mode arm runs AFTER the `editor.prompt.is_some()` and `editor.minibuffer.is_some()` interception blocks (which already consume Esc + return). **Delete the pre-existing standalone filter-cancel Esc check** — the Esc handling is now entirely in this arm, ordered pending-cancel → filter-cancel. Also clear `editor.pending_keys` wherever a prompt/minibuffer is OPENED (defensive; pending only accrues in normal mode). (d) Borrow care: `reg.dispatch` needs `&mut Ctx` holding `&mut editor`; clear pending/status BEFORE building `Ctx` (as shown).

- [ ] **Step 5: Retire the legacy key path** (`input.rs` / tests). The production dispatch no longer calls `input::key_to_command_id`. Either delete `key_to_command_id` (if nothing else uses it) or `#[cfg(test)]`-gate it; rewrite the legacy `key_to_command`/`step` test helper to drive `reduce` with the keymap (so the duplicate CUA table doesn't rot). Keep `KeyAction` only if still referenced. (Confirm what `step`/`key_to_command` are used by and migrate them.)

- [ ] **Step 6: Run tests + suite.** `cargo test --workspace` → all pass (the 4 new + all prior, with the mechanical `&km` added to every `reduce` call); `cargo build --workspace` zero warnings.

- [ ] **Step 7: Commit.**
```bash
git add wordcartel/src/editor.rs wordcartel/src/app.rs wordcartel/src/input.rs
git commit -m "feat(keymap): resolve keys through the trie in reduce (pending sequences, Esc-cancel, dispatch); retire hardcoded path"
```

---

### Task 5: Startup wiring (CLI → load config → build keymap → run)

**Files:**
- Modify: `wordcartel/src/main.rs`, `wordcartel/src/app.rs` (`run` signature + startup), `wordcartel/src/editor.rs` (hold the keymap)
- Test: `wordcartel/src/app.rs` (a startup-builds-keymap test) + `wordcartel/src/main.rs` (CLI smoke via `parse_cli`, already tested in Task 2)

**Interfaces:**
- Consumes: `config::{parse_cli, Cli, config_layer_paths, load}`, `keymap::build_keymap`, `dirs::config_dir`.
- Produces: `app::run(cli: config::Cli) -> std::io::Result<()>` (replaces `run(path)`); the loaded keymap stored on `Editor` (`editor.keymap`) and passed to `reduce`.

- [ ] **Step 1: Decide where the keymap lives at runtime.** Store the built `KeyTrie` on `Editor` (`pub keymap: crate::keymap::KeyTrie`, default = CUA built from `Registry::builtins()` so `new_from_text` works in tests without config). Then `run()` rebuilds it from the loaded config and `reduce` passes `&editor.keymap`. (Alternative: thread the keymap as a separate local in `run`; storing on Editor keeps `reduce`'s new `&KeyTrie` param fed from one place and lets tests set `e.keymap`.) **Add `Editor.keymap` with a CUA default in `new_from_text`** (`let (keymap,_) = keymap::build_keymap(&KeymapConfig::default(), &Registry::builtins());`). Update the Task-4 `reduce` call in `run()` to pass `&editor.keymap`.

- [ ] **Step 2: Write the failing test** in `app.rs`:
```rust
    #[test]
    fn run_startup_builds_keymap_from_config_with_user_bind() {
        // We can't run the TUI loop in a test, so test the startup builder in isolation:
        // a helper that turns (Cli-derived paths) into the effective keymap.
        let cfg = crate::config::KeymapConfig {
            preset: "cua".into(),
            patches: vec![crate::config::KeymapPatch {
                bind: [("ctrl-g".to_string(), "move_line_start".to_string())].into_iter().collect(),
                unbind: vec![],
            }],
        };
        let (km, warns) = crate::keymap::build_keymap(&cfg, &crate::registry::Registry::builtins());
        assert!(warns.is_empty());
        let g = crate::keymap::parse_chord("ctrl-g").unwrap();
        assert!(matches!(km.resolve(&[g]), crate::keymap::Resolution::Command(crate::registry::CommandId("move_line_start"))));
    }
```
(The end-to-end `run()` path itself is not unit-testable without a TTY; this pins the builder. The config layering is already tested in Task 2 and the keymap build in Task 3 — Task 5 is the thin glue.)

- [ ] **Step 2b: Run to verify it compiles/fails appropriately** — `cargo test -p wordcartel --lib run_startup_builds_keymap` (passes once Tasks 2-3 are in; serves as a guard that the glue types line up).

- [ ] **Step 3: Implement `run(cli)`** in `app.rs`. Replace `pub fn run(path: Option<PathBuf>)` with `pub fn run(cli: crate::config::Cli)`:
```rust
pub fn run(cli: crate::config::Cli) -> std::io::Result<()> {
    // Resolve config layers.
    let anchor = cli.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    let xdg = dirs::config_dir();
    let paths = crate::config::config_layer_paths(&cli, xdg.as_deref(), &anchor);
    let (cfg, mut warns) = crate::config::load(&paths);
    if let Some(c) = &cli.config_path { if !c.is_file() { warns.push(format!("config: --config path not found: {}", c.display())); } }

    // ... existing editor construction from cli.path ...
    let reg = Registry::builtins();
    let (keymap, mut kw) = crate::keymap::build_keymap(&cfg.keymap, &reg);
    warns.append(&mut kw);
    editor.keymap = keymap;
    if let Some(w) = warns.first() { editor.status = w.clone(); } // surface the first warning on the status line

    // ... existing loop, but: reduce(msg, &mut editor, &reg, &editor.keymap, ...) ...
}
```
**Implementer note (borrow):** `reduce(msg, &mut editor, &reg, &editor.keymap, …)` borrows `editor` both mutably and (for `&editor.keymap`) immutably — not allowed. Resolve by **taking the keymap out of `editor` into a local for the loop**: `let keymap = std::mem::take(&mut editor.keymap);` before the loop, pass `&keymap` to `reduce`, and (since the keymap never changes during the loop in v1) leave it in the local. (Or store the keymap as a `run()` local from the start rather than on `Editor`, and give `Editor` no `keymap` field — but the Task-4 tests set `e.keymap`, so keep the field for tests and `mem::take` it for the loop.)

- [ ] **Step 4: Update `main.rs`:**
```rust
fn main() {
    let cli = wordcartel::config::parse_cli(std::env::args());
    if let Err(e) = wordcartel::app::run(cli) {
        eprintln!("wcartel: {e}");
        std::process::exit(1);
    }
}
```

- [ ] **Step 5: Run tests + suite + manual smoke.** `cargo test --workspace` → all pass; `cargo build --workspace` zero warnings. Manual (optional): `wcartel --no-config file.md` opens; a `~/.config/wordcartel/config.toml` with a `[keymap] bind` rebinds; `--config bad.toml` (missing) shows a status warning, still opens.

- [ ] **Step 6: Commit.**
```bash
git add wordcartel/src/main.rs wordcartel/src/app.rs wordcartel/src/editor.rs
git commit -m "feat(config): wire CLI → layered config → built keymap into run() startup"
```

---

### Task 6: Session state (resume-at-position + marks store)

**Files:**
- Modify: `wordcartel/src/lib.rs`, `wordcartel/src/app.rs` (resume on open, persist on save/quit)
- Create: `wordcartel/src/state.rs`
- Test: `wordcartel/src/state.rs`, `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `swap::state_dir`, `file::save_atomic_bytes`, `toml`/`serde`.
- Produces:
  - `pub struct StateEntry { pub cursor: usize, pub scroll: usize, pub marks: BTreeMap<String,usize>, pub mtime: i64, pub size: u64, pub seq: u64 }`. **`marks` keys are single-char Strings, NOT `char`** (Codex CRITICAL: `toml` rejects non-string map keys). The mark/jump commands (5c) convert `char`↔single-char `String` at the app boundary; 5a only stores/loads the map.
  - `pub struct SessionState { pub entries: BTreeMap<String, StateEntry> }` (key = canonical abs path string).
  - `pub fn load() -> SessionState` (tolerate corrupt → empty); `SessionState::record(&mut self, path, entry, max_entries)` (insert + LRU-prune by `seq`); `pub fn save(&self) -> std::io::Result<()>` (TOML → `save_atomic_bytes` into `state_dir()/session.toml`).
  - `pub fn file_identity(path: &Path) -> Option<(i64, u64)>` (mtime secs + size) for the staleness guard.

- [ ] **Step 1: Write failing tests** in `state.rs` (inject a temp dir so tests don't touch the real state dir — pass the dir into load/save variants, mirroring swap.rs's `*_in(dir)` pattern):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_and_prune_lru() {
        let dir = tmp();
        let mut s = SessionState::default();
        for i in 0..5u64 {
            s.record(format!("/f{i}"), StateEntry { cursor: i as usize, scroll: 0,
                marks: Default::default(), mtime: 1, size: 1, seq: i }, 3); // cap 3
        }
        assert_eq!(s.entries.len(), 3, "LRU-pruned to cap");
        assert!(s.entries.contains_key("/f4") && !s.entries.contains_key("/f0"));
        s.save_in(&dir).unwrap();
        let back = load_in(&dir);
        assert_eq!(back.entries.len(), 3);
        assert_eq!(back.entries["/f4"].cursor, 4);
    }
    #[test]
    fn corrupt_state_file_loads_empty() {
        let dir = tmp();
        std::fs::write(dir.join("session.toml"), b"\xff not toml").unwrap();
        assert!(load_in(&dir).entries.is_empty());
    }
    fn tmp() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering}; static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!("wc-state-{}-{}", std::process::id(), N.fetch_add(1,Ordering::Relaxed)));
        std::fs::create_dir_all(&p).unwrap(); p
    }
}
```
And in `app.rs`, a resume test:
```rust
    #[test]
    fn resume_restores_when_identity_matches_and_clamps_when_not() {
        // unit-test the resume decision helper directly (no TTY):
        // apply_resume(entry, current_identity, doc_len) -> Option<(cursor,scroll)>
        use crate::state::StateEntry;
        let e = StateEntry { cursor: 4, scroll: 2, marks: Default::default(), mtime: 10, size: 20, seq: 0 };
        // identity match → restore (clamped to doc_len)
        assert_eq!(crate::app::apply_resume(&e, (10,20), 100), Some((4,2)));
        assert_eq!(crate::app::apply_resume(&e, (10,20), 3), Some((3,2)), "cursor clamped to doc_len");
        // identity mismatch → discard
        assert_eq!(crate::app::apply_resume(&e, (11,20), 100), None);
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib state:: resume_restores` → FAIL.

- [ ] **Step 3: Implement `state.rs`** (with `*_in(dir)` testable variants + thin real wrappers calling `swap::state_dir()`):
```rust
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateEntry { pub cursor: usize, pub scroll: usize,
    pub marks: BTreeMap<String, usize>, pub mtime: i64, pub size: u64, pub seq: u64 } // String keys (toml has no char keys)

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState { pub entries: BTreeMap<String, StateEntry> }

impl SessionState {
    pub fn record(&mut self, path: String, entry: StateEntry, max_entries: usize) {
        self.entries.insert(path, entry);
        while self.entries.len() > max_entries {
            // evict lowest seq (LRU)
            if let Some(k) = self.entries.iter().min_by_key(|(_, e)| e.seq).map(|(k,_)| k.clone()) {
                self.entries.remove(&k);
            } else { break; }
        }
    }
    pub fn save_in(&self, dir: &Path) -> std::io::Result<()> {
        // Propagate serialization errors (Codex fix: do NOT silently drop state via unwrap_or_default).
        let text = toml::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("session serialize: {e}")))?;
        crate::file::save_atomic_bytes(&dir.join("session.toml"), text.as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }
    pub fn save(&self) -> std::io::Result<()> { self.save_in(&crate::swap::state_dir()?) }
}

pub fn load_in(dir: &Path) -> SessionState {
    match std::fs::read_to_string(dir.join("session.toml")) {
        Ok(t) => toml::from_str(&t).unwrap_or_default(),
        Err(_) => SessionState::default(),
    }
}
pub fn load() -> SessionState {
    match crate::swap::state_dir() { Ok(d) => load_in(&d), Err(_) => SessionState::default() }
}

pub fn file_identity(path: &Path) -> Option<(i64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m.modified().ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64).unwrap_or(0);
    Some((mtime, m.len()))
}
```
Declare `pub mod state;` in lib.rs.

- [ ] **Step 4: Add `apply_resume` + wire into `run()`** (`app.rs`):
```rust
/// Decide the resume position: restore (clamped) only if the stored identity matches.
pub fn apply_resume(e: &crate::state::StateEntry, current: (i64,u64), doc_len: usize) -> Option<(usize,usize)> {
    if (e.mtime, e.size) != current { return None; }
    Some((e.cursor.min(doc_len), e.scroll))
}
```
In `run()`: load the `SessionState` once at startup. After loading the document, if `cfg.state.resume` and the file has a canonical path, look up `session.entries[path]`, compute `file_identity`, call `apply_resume`, and if `Some((cur,scroll))` set the editor cursor + scroll.

**Persistence happens in the `run()` LOOP and at quit — NOT inside `reduce` (Codex IMPORTANT).** Save-success is observed via the document's `saved_version` (4b's saved/dirty model), which `reduce`→`apply_result` advances internally; the loop can detect it without threading session state through `reduce`:
```rust
    let mut last_persisted_saved = editor.active().saved_version;
    loop {
        // … recv msg …
        let keep = reduce(msg, &mut editor, &reg, &keymap, &executor, &clock, &msg_tx);
        // … draw, clipboard drain, etc. …
        // Persist session state when a save just completed (saved_version advanced).
        let sv = editor.active().saved_version;
        if sv != last_persisted_saved {
            persist_session(&mut session, &editor, &cfg);   // record active entry + session.save()
            last_persisted_saved = sv;
        }
        if !keep { break; }
    }
    // On clean quit: persist once more (cursor moved since the last save).
    persist_session(&mut session, &editor, &cfg);
```
where `persist_session` records the active file's entry — `{cursor, scroll, marks (empty in 5a), file_identity(path), seq: next_seq()}` via `session.record(canonical_path, entry, cfg.state.max_entries)` then best-effort `session.save()` (a write error → status warning, never blocks quit).

**Implementer notes:** (a) confirm the `saved_version` field name on the document (4b); if the save model exposes a different "saved" signal, watch that instead — the principle is "persist when a save completes + at quit," never per keystroke. (b) canonicalize via `std::fs::canonicalize` (fall back / skip if it fails — e.g. a not-yet-saved new file → don't persist). (c) Scratch (no path) → never recorded. (d) `seq` is a monotonic run-loop counter (only needs to be monotonic within a run for LRU). (e) clamp scroll too if the view requires it.

- [ ] **Step 5: Run tests + suite.** `cargo test --workspace` → all pass; `cargo build --workspace` zero warnings.

- [ ] **Step 6: Commit.**
```bash
git add wordcartel/src/lib.rs wordcartel/src/state.rs wordcartel/src/app.rs
git commit -m "feat(state): path-keyed session store — resume-at-position (mtime+size guard) + marks store + LRU prune"
```

---

## Self-Review (5a)

**Spec coverage:** §2 modules/deps (all tasks; toml/serde T2); §3 config load + precedence + `--config`/`--no-config` (T2) + project-local anchor (T2/T5); §4 keymap trie + multi-key + presets + patch-merge + pending + Esc precedence (T3 engine, T4 integration); §5.1 CommandId Borrow/resolve_name (T1); §5.2 replace key_to_command_id + Press guard + printable fallthrough + legacy retirement (T3/T4); §6 session state + mtime/size staleness + resume + prune + atomic + scratch-skip (T6); §7 degrade-don't-abort (warnings throughout); §9 tests (each task). ✅

**Codex plan-review fixes applied (4 critical + 2 important):** (1) per-field config merge via `RawConfig` Option fields — an omitted `[state]` field inherits the lower layer instead of resetting to default (T2); (2) ordered `Vec<KeymapPatch>` (one per layer) applied in precedence order in `build_keymap` — a high layer's bind beats a low layer's unbind (T2/T3, `cross_layer_high_bind_beats_low_unbind` test); (3) Esc precedence folded into the normal-mode arm (pending-cancel → filter-cancel; the standalone filter-cancel Esc check is removed) (T4); (4) `marks: BTreeMap<String,usize>` (toml has no char keys) + `save_in` propagates serialize errors (T6); SHIFT normalization shared by `from_key_event`/`parse_chord` (uppercase + strip SHIFT for chars) with a matching test (T3); `KeyTrie` derives `Debug,Clone,Default` so it can live on `Editor` (T3/T5); session persistence driven by a `saved_version` watch in the `run()` loop + at quit, not inside `reduce` (T6). Codex confirmed T1's `Borrow<str>` lookup and T4's `InsertChar` fallthrough are correct as written.

**Codex spec-review fixes reflected:** Esc precedence pinned (T4 Step 4 — normal-mode Esc cancels pending only after prompt/minibuffer blocks); serde-native `preset`/`bind`/`unbind` shape (T2); `Borrow<str>`+`resolve_name` (T1); Press guard in `from_key_event` (T3); project-local anchor = CLI-file parent/CWD (T5); preset resolved-before-patch (T3 `build_keymap`); mtime+size staleness guard (T6 `apply_resume`); real CLI parser (T2 `parse_cli`); legacy test-keymap retired (T4 Step 5); bundled-preset id-validation test (T3 `both_presets_resolve_against_builtins`).

**Type consistency:** `CommandId`/`resolve_name` (T1) → `KeymapConfig`/`Config`/`Cli`/`load`/`config_layer_paths` (T2) → `KeyChord`/`KeyTrie`/`Resolution`/`from_key_event`/`parse_chord`/`parse_seq`/`build_keymap`/`preset_bindings` (T3) → `Editor.pending_keys` + `reduce(.., keymap: &KeyTrie, ..)` (T4) → `run(cli)` + `Editor.keymap` + `apply_resume` (T5) → `SessionState`/`StateEntry`/`record`/`save`/`load`/`file_identity` (T6). The `reduce` signature gains `&KeyTrie` in T4 (every call site updated, mechanical, like the 4c-1 `msg_tx` add); `run(path)`→`run(cli)` in T5.

**Implementer-verify markers (real-code confirmations):** the exact current insert path for the printable fallthrough (T4 — reuse it verbatim); the exact list of bindings in `input::key_to_command_id` to transcribe into the `CUA` preset (T3); what consumes the legacy `key_to_command`/`step` (T4 retirement); `toml`/`serde` versions; `dirs::config_dir()` presence (T5); the editor cursor/scroll field names used by resume (T6). Each names what to check.

---

## Execution Handoff

Plan complete. Recommended: **subagent-driven execution** (fresh subagent per task + per-task review), then an opus whole-branch review and a Codex pre-merge gate before merge — the flow that shipped 4b/4c.
