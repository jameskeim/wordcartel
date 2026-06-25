//! Keymap engine: chord parsing, trie resolution, preset bindings, config patch merge.
//!
//! `KeyChord` normalises modifier+key pairs so that config strings and live
//! crossterm events agree:  `ctrl-shift-z` (config) == `Char('Z')+CONTROL` (event).
//!
//! The trie is a flat `HashMap<Vec<KeyChord>, CommandId>` with a prefix scan
//! for `Pending` — efficient enough for v1's small keymap (< 100 entries).

use std::collections::HashMap;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crate::registry::{Registry, CommandId};

// ---------------------------------------------------------------------------
// KeyChord
// ---------------------------------------------------------------------------

/// A single key-press with its modifier set, in normalised form.
///
/// Normalisation: for `Char` keys, if SHIFT is set, the char is uppercased
/// and the SHIFT bit is removed.  Non-char keys keep SHIFT (e.g. `Shift+Left`
/// is a valid distinct binding).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyChord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

/// Shared normalisation used by both `from_key_event` and `parse_chord`.
fn normalize(code: KeyCode, mods: KeyModifiers) -> KeyChord {
    match code {
        KeyCode::Char(c) => {
            let c = if mods.contains(KeyModifiers::SHIFT) {
                c.to_ascii_uppercase()
            } else {
                c
            };
            let mut m = mods;
            m.remove(KeyModifiers::SHIFT);
            KeyChord { code: KeyCode::Char(c), mods: m }
        }
        other => KeyChord { code: other, mods },
    }
}

/// Convert a live crossterm event to a `KeyChord`.  Returns `None` for
/// non-Press events (Release / Repeat) to avoid double-dispatch.
pub fn from_key_event(k: KeyEvent) -> Option<KeyChord> {
    if k.kind != KeyEventKind::Press {
        return None;
    }
    Some(normalize(k.code, k.modifiers))
}

/// Parse a chord string such as `"ctrl-z"`, `"shift-left"`, `"alt-f4"`,
/// `"enter"`, `"f1"`.  Modifier tokens are `ctrl`, `alt`, `shift` (lowercased
/// internally), separated from the key and from each other by `-`.
///
/// Returns `None` on any unrecognised token or ambiguous parse.
pub fn parse_chord(s: &str) -> Option<KeyChord> {
    let lower = s.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('-').collect();
    if parts.is_empty() {
        return None;
    }

    // Split: everything except the last token is a modifier.
    let (mod_parts, key_parts) = parts.split_at(parts.len().saturating_sub(1));

    let mut mods = KeyModifiers::NONE;
    for m in mod_parts {
        match *m {
            "ctrl"  => mods |= KeyModifiers::CONTROL,
            "alt"   => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            _       => return None,
        }
    }

    let key = key_parts.first()?;
    let code = match *key {
        "enter"     => KeyCode::Enter,
        "tab"       => KeyCode::Tab,
        "esc"       => KeyCode::Esc,
        "space"     => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        "del"       => KeyCode::Delete,
        "left"      => KeyCode::Left,
        "right"     => KeyCode::Right,
        "up"        => KeyCode::Up,
        "down"      => KeyCode::Down,
        "pageup"    => KeyCode::PageUp,
        "pagedown"  => KeyCode::PageDown,
        "home"      => KeyCode::Home,
        "end"       => KeyCode::End,
        "\\"        => KeyCode::Char('\\'),
        f if f.starts_with('f') && f.len() > 1 && f[1..].parse::<u8>().is_ok() => {
            KeyCode::F(f[1..].parse().unwrap())
        }
        c if c.chars().count() == 1 => KeyCode::Char(c.chars().next().unwrap()),
        _           => return None,
    };

    // Apply the same normalisation as from_key_event (SHIFT+char → uppercase, strip SHIFT).
    Some(normalize(code, mods))
}

/// Parse a space-separated sequence of chord strings into `Vec<KeyChord>`.
/// Returns `None` if any chord fails to parse.
pub fn parse_seq(s: &str) -> Option<Vec<KeyChord>> {
    s.split_whitespace().map(parse_chord).collect()
}

/// Render a pending key sequence as a human-readable string for the status bar.
/// E.g. `[ctrl-k, ctrl-s]` → `"ctrl-k ctrl-s"`.
pub fn chords_display(chords: &[KeyChord]) -> String {
    chords.iter().map(|ch| {
        let mut parts: Vec<&str> = Vec::new();
        if ch.mods.contains(KeyModifiers::CONTROL) { parts.push("ctrl"); }
        if ch.mods.contains(KeyModifiers::ALT)     { parts.push("alt"); }
        if ch.mods.contains(KeyModifiers::SHIFT)   { parts.push("shift"); }
        let key_str: String = match ch.code {
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter     => "enter".into(),
            KeyCode::Tab       => "tab".into(),
            KeyCode::Esc       => "esc".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Delete    => "del".into(),
            KeyCode::Left      => "left".into(),
            KeyCode::Right     => "right".into(),
            KeyCode::Up        => "up".into(),
            KeyCode::Down      => "down".into(),
            KeyCode::Home      => "home".into(),
            KeyCode::End       => "end".into(),
            KeyCode::PageUp    => "pageup".into(),
            KeyCode::PageDown  => "pagedown".into(),
            KeyCode::F(n)      => format!("f{n}"),
            other              => format!("{other:?}").to_ascii_lowercase(),
        };
        if parts.is_empty() { key_str } else { format!("{}-{key_str}", parts.join("-")) }
    }).collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// KeyTrie + Resolution
// ---------------------------------------------------------------------------

/// Result of querying the trie with a sequence of pressed keys.
pub enum Resolution {
    /// The sequence matches a bound command exactly.
    Command(CommandId),
    /// The sequence is a valid prefix — more keys are expected.
    Pending,
    /// No binding matches and no binding starts with this prefix.
    None,
}

/// A trie over `Vec<KeyChord>` sequences.
///
/// Internally a flat `HashMap` — fast enough for v1's small keymap.
/// `Debug + Clone + Default` are required by the `Editor` struct (Task 5).
#[derive(Debug, Clone, Default)]
pub struct KeyTrie {
    map: HashMap<Vec<KeyChord>, CommandId>,
}

impl KeyTrie {
    /// Bind `seq` to `id`, overwriting any existing binding.
    pub fn bind(&mut self, seq: Vec<KeyChord>, id: CommandId) {
        self.map.insert(seq, id);
    }

    /// Remove any binding for `seq` (no-op if absent).
    pub fn unbind(&mut self, seq: &[KeyChord]) {
        self.map.remove(seq);
    }

    /// Reverse-lookup: a display chord bound to `id`, or None if unbound.
    /// Shortest sequence wins; ties broken by the rendered string (KeyChord isn't Ord).
    #[allow(dead_code)] // wired in Task 3
    pub fn chord_for(&self, id: CommandId) -> Option<String> {
        self.map.iter()
            .filter(|(_, v)| **v == id)
            .map(|(seq, _)| chords_display(seq))
            .min_by(|a, b| a.chars().count().cmp(&b.chars().count()).then_with(|| a.cmp(b)))
    }

    /// Resolve `pending` against the trie.
    pub fn resolve(&self, pending: &[KeyChord]) -> Resolution {
        if let Some(id) = self.map.get(pending) {
            return Resolution::Command(*id);
        }
        // Is `pending` a strict prefix of any binding?
        if self.map.keys().any(|k| k.len() > pending.len() && k.starts_with(pending)) {
            return Resolution::Pending;
        }
        Resolution::None
    }
}

// ---------------------------------------------------------------------------
// Preset tables
// ---------------------------------------------------------------------------

/// Return the static binding table for a named preset, or `None` for unknown names.
pub fn preset_bindings(name: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match name {
        "cua"      => Some(CUA),
        "wordstar" => Some(WORDSTAR),
        _          => None,
    }
}

/// CUA preset — mirrors every named-command binding in `input::key_to_command_id`.
///
/// Excluded (no CommandId): printable `Char(c)` without ctrl/alt — those are
/// handled as `KeyAction::Insert(c)` fallthrough, not registry commands.
///
/// Note: `ctrl-shift-z` → redo covers the crossterm variant where a shifted
/// letter arrives as `Char('Z') + SHIFT`; after normalisation both become
/// `KeyChord { code: Char('Z'), mods: CONTROL }` so only one trie entry is needed.
/// The `ctrl-y` entry covers the other redo path.
static CUA: &[(&str, &str)] = &[
    // Undo / redo  (input.rs lines 92–94)
    ("ctrl-z",       "undo"),
    ("ctrl-y",       "redo"),
    ("ctrl-shift-z", "redo"),      // normalises to Char('Z')+CONTROL (same as ctrl-y variant)
    // Clipboard  (input.rs lines 95–97)
    ("ctrl-c", "copy"),
    ("ctrl-x", "cut"),
    ("ctrl-v", "paste"),
    // File  (input.rs lines 98–99)
    ("ctrl-s", "save"),
    ("ctrl-q", "quit"),
    // Tools  (input.rs lines 100–101)
    ("ctrl-e", "filter"),
    ("ctrl-t", "transform"),
    ("ctrl-p", "palette"),
    ("f10",    "menu"),
    // View  (input.rs lines 102, 114)
    ("ctrl-\\", "cycle_render_mode"),
    ("f1",      "cycle_render_mode"),
    // Navigation — no shift  (input.rs lines 104–109)
    ("left",  "move_left"),
    ("right", "move_right"),
    ("up",    "move_up"),
    ("down",  "move_down"),
    ("home",  "move_line_start"),
    ("end",   "move_line_end"),
    // Selecting motions — shift  (input.rs lines 104–109)
    ("shift-left",  "select_left"),
    ("shift-right", "select_right"),
    ("shift-up",    "select_up"),
    ("shift-down",  "select_down"),
    ("shift-home",  "select_line_start"),
    ("shift-end",   "select_line_end"),
    // Word navigation
    ("ctrl-left",       "move_word_left"),
    ("ctrl-right",      "move_word_right"),
    ("ctrl-shift-left", "select_word_left"),
    ("ctrl-shift-right","select_word_right"),
    // Word delete
    ("ctrl-backspace", "delete_word_back"),
    ("ctrl-del",       "delete_word_forward"),
    // Editing  (input.rs lines 111–113)
    ("enter",     "insert_newline"),
    ("backspace", "backspace"),
    ("del",       "delete_forward"),
];

/// WordStar preset — classic two-key diamond + file commands mapped to v1 command ids.
///
/// Only sequences whose target exists in `Registry::builtins()` are included;
/// the set grows as later sub-efforts add commands (find/replace, etc.).
static WORDSTAR: &[(&str, &str)] = &[
    // Diamond navigation (^E ^S ^D ^X)
    ("ctrl-s", "move_left"),
    ("ctrl-d", "move_right"),
    ("ctrl-e", "move_up"),
    ("ctrl-x", "move_down"),
    // Word navigation (^A left-word, ^F right-word — best-effort to char motions)
    ("ctrl-a", "move_left"),
    ("ctrl-f", "move_right"),
    // ^K block / file commands
    ("ctrl-k ctrl-s", "save"),
    ("ctrl-k ctrl-q", "quit"),
    ("ctrl-k ctrl-c", "copy"),
    ("ctrl-k ctrl-v", "paste"),
    // Undo / redo
    ("ctrl-z", "undo"),
    ("ctrl-y", "redo"),
    // Editing
    ("backspace", "backspace"),
    ("del",       "delete_forward"),
    ("enter",     "insert_newline"),
    // Arrow keys / home / end
    ("left",  "move_left"),
    ("right", "move_right"),
    ("up",    "move_up"),
    ("down",  "move_down"),
    ("home",  "move_line_start"),
    ("end",   "move_line_end"),
    // Shift+arrow selecting motions
    ("shift-left",  "select_left"),
    ("shift-right", "select_right"),
    ("shift-up",    "select_up"),
    ("shift-down",  "select_down"),
    ("shift-home",  "select_line_start"),
    ("shift-end",   "select_line_end"),
];

// ---------------------------------------------------------------------------
// build_keymap
// ---------------------------------------------------------------------------

/// Build a `KeyTrie` from a `KeymapConfig` and a registry.
///
/// Order of application (lowest → highest precedence):
/// 1. Preset base (from `km.preset`).
/// 2. Each patch layer in `km.patches` order, applying `bind` then `unbind`.
///    A later patch's bind overrides an earlier patch's unbind.
///
/// Unknown preset name → warning + fall back to `"cua"`.
/// Bad chord string or unknown command-id in a patch → warning + skip.
/// Preset base entries never warn (integrity guaranteed by `both_presets_resolve_against_builtins`).
#[allow(dead_code)] // wired in Task 4/5
pub fn build_keymap(km: &crate::config::KeymapConfig, reg: &Registry) -> (KeyTrie, Vec<String>) {
    let mut warns: Vec<String> = Vec::new();
    let mut trie = KeyTrie::default();

    // 1) Apply preset base.
    let base = match preset_bindings(&km.preset) {
        Some(b) => b,
        None => {
            warns.push(format!(
                "config: unknown keymap.preset '{}', falling back to 'cua'",
                km.preset
            ));
            preset_bindings("cua").unwrap()
        }
    };
    for (chord_str, id_str) in base {
        // Preset integrity is guaranteed by test; parse/resolve errors here are bugs.
        if let (Some(seq), Some(id)) = (parse_seq(chord_str), reg.resolve_name(id_str)) {
            trie.bind(seq, id);
        }
    }

    // 2) Apply patch layers in order (lowest → highest precedence).
    //    Within each patch, binds come before unbinds so a same-patch unbind
    //    can remove a just-added bind; across patches a later patch's bind
    //    overrides any earlier patch's unbind.
    for patch in &km.patches {
        for (chord_str, id_str) in &patch.bind {
            let seq = match parse_seq(chord_str) {
                Some(s) => s,
                None => {
                    warns.push(format!("config: bad chord '{chord_str}'"));
                    continue;
                }
            };
            // Esc is reserved for cancel/dismiss in v1: the reducer special-cases
            // KeyCode::Esc before routing to the keymap, so any config binding whose
            // first chord is Esc (any modifiers) would be a silent dead binding.
            if seq.first().map(|c| c.code == KeyCode::Esc).unwrap_or(false) {
                warns.push(format!(
                    "config: '{chord_str}' cannot be bound — Esc is reserved for cancel/dismiss"
                ));
                continue;
            }
            let id = match reg.resolve_name(id_str) {
                Some(i) => i,
                None => {
                    warns.push(format!(
                        "config: '{chord_str}' → unknown command '{id_str}'"
                    ));
                    continue;
                }
            };
            trie.bind(seq, id);
        }
        for chord_str in &patch.unbind {
            match parse_seq(chord_str) {
                Some(seq) => trie.unbind(&seq),
                None => warns.push(format!("config: bad unbind chord '{chord_str}'")),
            }
        }
    }

    (trie, warns)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
    fn esc_binding_in_config_is_warned_and_skipped() {
        // Esc is reserved for cancel/dismiss; a config patch binding it must be
        // warned and NOT inserted into the trie (would be a silent dead binding).
        let (t, w) = km(&[("esc", "quit")], &[], Some("cua"));
        assert_eq!(w.len(), 1, "expected exactly one warning for esc binding");
        assert!(
            w[0].contains("esc") || w[0].contains("Esc"),
            "warning must mention esc: {}", w[0]
        );
        assert!(
            w[0].contains("reserved"),
            "warning must mention 'reserved': {}", w[0]
        );
        // The trie must NOT resolve a bare Esc to any command.
        let esc = KeyChord { code: KeyCode::Esc, mods: KeyModifiers::NONE };
        assert!(
            matches!(t.resolve(&[esc]), Resolution::None),
            "Esc must not be bound in the trie"
        );
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

    #[test]
    fn chord_for_returns_shortest_and_blank_when_unbound() {
        let (t, _) = build_keymap(&crate::config::KeymapConfig::default(), &crate::registry::Registry::builtins());
        assert_eq!(t.chord_for(crate::registry::CommandId("cut")).as_deref(), Some("ctrl-x"));
        // a command with no default binding → None
        assert_eq!(t.chord_for(crate::registry::CommandId("ventilate")), None);
    }
}
