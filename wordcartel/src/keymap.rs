//! Keymap engine: chord parsing, trie resolution, preset bindings, config patch merge.
//!
//! `KeyChord` normalises modifier+key pairs so that config strings and live
//! crossterm events agree:  `ctrl-shift-z` (config) == `Char('Z')+CONTROL` (event).
//!
//! The trie is a flat `HashMap<Vec<KeyChord>, CommandId>` with a prefix scan
//! for `Pending` — efficient enough for v1's small keymap (< 100 entries).

use std::collections::{HashMap, HashSet};
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
    /// Sequences bound by a config patch (user-explicit), as opposed to the preset base.
    /// `chord_for` prefers these so a user's binding wins over an inherited default (contract law 7).
    user_bound: HashSet<Vec<KeyChord>>,
}

impl KeyTrie {
    /// Bind `seq` to `id`, overwriting any existing binding.
    pub fn bind(&mut self, seq: Vec<KeyChord>, id: CommandId) {
        self.map.insert(seq, id);
    }

    /// Bind `seq` to `id` AND mark it user-explicit (a config patch binding).
    ///
    /// Used by `apply_patch_tables` so that `chord_for` can prefer a user's
    /// explicit binding over an inherited preset default (contract law 7).
    pub fn bind_user(&mut self, seq: Vec<KeyChord>, id: CommandId) {
        self.user_bound.insert(seq.clone());
        self.map.insert(seq, id);
    }

    /// Remove any binding for `seq` (no-op if absent).
    pub fn unbind(&mut self, seq: &[KeyChord]) {
        self.map.remove(seq);
        self.user_bound.remove(seq);
    }

    /// Reverse-lookup: a display chord bound to `id`, or None if unbound.
    ///
    /// Prefers user-explicit bindings (set via `bind_user`) over preset defaults.
    /// Among the winning pool, shortest sequence wins; ties broken alphabetically.
    pub fn chord_for(&self, id: CommandId) -> Option<String> {
        let all: Vec<&Vec<KeyChord>> = self.map.iter()
            .filter(|(_, v)| **v == id)
            .map(|(s, _)| s)
            .collect();
        let user: Vec<&Vec<KeyChord>> = all.iter()
            .copied()
            .filter(|s| self.user_bound.contains(*s))
            .collect();
        let pool = if user.is_empty() { all } else { user };
        pool.into_iter()
            .map(|seq| chords_display(seq))
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
/// The known preset names — the single list preset-wide pins iterate. Keep in
/// sync with preset_bindings' arms (the surface pin unwraps preset_bindings for
/// every entry, so a PRESETS name without a bindings arm fails loudly).
pub const PRESETS: &[&str] = &["cua", "wordstar"];

pub fn preset_bindings(name: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match name {
        "cua"      => Some(CUA),
        "wordstar" => Some(WORDSTAR),
        _          => None,
    }
}

/// Resolve a raw preset string to a known base ("cua" | "wordstar"); unknown → "cua".
/// Shared by build_keymap (scoped-table selection — base selection keeps its own
/// warning-emitting fallback arm) and run()'s seeding so the two can never disagree
/// about what an unknown preset fell back to.
pub fn resolve_preset(name: &str) -> &'static str {
    match name { "wordstar" => "wordstar", _ => "cua" }
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
    // Select All
    ("ctrl-a", "select_all"),
    // Clipboard  (input.rs lines 95–97)
    ("ctrl-c", "copy"),
    ("ctrl-x", "cut"),
    ("ctrl-v", "paste"),
    // File  (input.rs lines 98–99)
    ("ctrl-o", "open"),
    ("ctrl-n", "new"),
    ("ctrl-s", "save"),
    ("ctrl-shift-s", "save_as"),
    ("ctrl-q", "quit"),
    // Tools  (input.rs lines 100–101)
    ("ctrl-e", "filter"),
    ("ctrl-t", "transform"),
    ("ctrl-p", "palette"),
    ("f10",    "menu"),
    // Search
    ("ctrl-f", "find"),
    ("ctrl-r", "replace"),
    // Go to line (Effort 8)
    ("ctrl-g", "goto_line"),
    ("f3",     "find_next"),
    ("shift-f3", "find_prev"),
    // View  (input.rs lines 102, 114)
    ("ctrl-\\", "cycle_render_mode"),
    ("f1",      "cycle_render_mode"),
    ("alt-r",   "view_review"),
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
    // Paragraph / page / document navigation
    ("ctrl-up",   "move_paragraph_up"),
    ("ctrl-down", "move_paragraph_down"),
    ("pageup",    "move_page_up"),
    ("pagedown",  "move_page_down"),
    ("ctrl-home", "move_doc_start"),
    ("ctrl-end",  "move_doc_end"),
    // Word navigation
    ("ctrl-left",       "move_word_left"),
    ("ctrl-right",      "move_word_right"),
    ("ctrl-shift-left", "select_word_left"),
    ("ctrl-shift-right","select_word_right"),
    // Sentence motions (S5) — Emacs M-a / M-e.
    ("alt-a",       "sentence_left"),
    ("alt-e",       "sentence_right"),
    ("alt-shift-a", "select_sentence_left"),
    ("alt-shift-e", "select_sentence_right"),
    // Word delete
    ("ctrl-backspace", "delete_word_back"),
    ("ctrl-del",       "delete_word_forward"),
    // Text object expand/shrink (Task 7 / Effort 5c)
    ("ctrl-w",       "expand_selection"),
    ("ctrl-shift-w", "shrink_selection"),
    // Named marks (Task 8 / Effort 5c)
    ("ctrl-k m", "set_mark"),
    ("ctrl-k j", "jump_to_mark"),
    // Jump-back ring (Task 9 / Effort 5c)
    ("alt-left",  "jump_back"),
    ("alt-right", "jump_forward"),
    // Editing  (input.rs lines 111–113)
    ("enter",     "insert_newline"),
    ("backspace", "backspace"),
    ("del",       "delete_forward"),
    // Diagnostics (Task 6 / Effort 5f)
    ("ctrl-.", "quick_fix"),
    ("f8",      "diag_next"),
    ("shift-f8","diag_prev"),
    // Outline & folding (Effort 5g)
    ("alt-o",        "outline"),
    ("alt-up",       "heading_prev"),
    ("alt-down",     "heading_next"),
    ("alt-shift-up", "heading_parent"),
    ("alt-z",        "fold_toggle"),
    ("alt-shift-z",  "fold_all"),
    ("alt-shift-x",  "unfold_all"),
    // Block operations (Effort 9A)
    ("alt-b",        "mark_block_from_selection"),
    // Effort 6: send-to-scratch verbs (CUA bindings)
    ("alt-shift-c",  "copy_block_to_scratch"),
    ("alt-shift-v",  "move_block_to_scratch"),
    // Effort 6: buffer cycling (CUA bindings)
    ("alt-,", "prev_buffer"),
    ("alt-.", "next_buffer"),
    // Effort 6: buffer switcher (ctrl-shift-e is free; ctrl-e is "filter")
    ("ctrl-shift-e", "switch_buffer"),
    // Command-surface curation (A14/A12): ten atomic edits + toggle_scratch, alt- plane
    ("alt-u",       "upcase"),
    ("alt-l",       "downcase"),
    ("alt-c",       "capitalize"),
    ("alt-t",       "transpose_chars"),
    ("alt-shift-t", "transpose_words"),
    ("alt-shift-l", "transpose_lines"),
    ("alt-j",       "join_line"),
    ("alt-space",   "just_one_space"),
    ("alt-shift-j", "delete_blank_lines"),
    ("alt-\\",      "delete_horizontal_space"),
    ("alt-s",       "toggle_scratch"),
];

/// WordStar preset — full faithful classic keymap mapped to v1 command ids.
///
/// Diamond `^E/^X/^S/^D` (cursor), `^A/^F` (word), `^R/^C` (page),
/// `^W/^Z` (scroll), delete/undo cluster, `^Q` quick prefix (both
/// ctrl-held and plain second-key forms), `^K` block/file prefix (both
/// forms except `^KM`/`^KJ` which are plain-only — `^M`/`^J` are
/// terminal-reserved), and kept modern arrow/home/end/shift-select keys.
static WORDSTAR: &[(&str, &str)] = &[
    // Cursor diamond
    ("ctrl-e", "move_up"),
    ("ctrl-x", "move_down"),
    ("ctrl-s", "move_left"),
    ("ctrl-d", "move_right"),
    ("ctrl-a", "move_word_left"),   // Task 1 fix
    ("ctrl-f", "move_word_right"),  // Task 1 fix
    ("ctrl-r", "move_page_up"),
    ("ctrl-c", "move_page_down"),
    ("ctrl-w", "scroll_line_up"),
    ("ctrl-z", "scroll_line_down"),
    // Delete / undo / redo
    ("ctrl-g", "delete_forward"),
    ("ctrl-t", "delete_word_forward"),
    ("ctrl-y", "delete_line"),
    ("ctrl-u", "undo"),
    ("ctrl-shift-u", "redo"),
    // ^Q "quick" prefix (ctrl-held OR plain second key)
    ("ctrl-q ctrl-s", "move_line_start"), ("ctrl-q s", "move_line_start"),
    ("ctrl-q ctrl-d", "move_line_end"),   ("ctrl-q d", "move_line_end"),
    ("ctrl-q ctrl-r", "move_doc_start"),  ("ctrl-q r", "move_doc_start"),
    ("ctrl-q ctrl-c", "move_doc_end"),    ("ctrl-q c", "move_doc_end"),
    ("ctrl-q ctrl-e", "move_screen_top"), ("ctrl-q e", "move_screen_top"),
    ("ctrl-q ctrl-x", "move_screen_bottom"), ("ctrl-q x", "move_screen_bottom"),
    ("ctrl-q ctrl-f", "find"),    ("ctrl-q f", "find"),
    ("ctrl-q ctrl-a", "replace"), ("ctrl-q a", "replace"),
    ("ctrl-q ctrl-l", "find_next"), ("ctrl-q l", "find_next"),
    ("ctrl-q ctrl-p", "jump_back"), ("ctrl-q p", "jump_back"),
    ("ctrl-q ctrl-y", "delete_to_line_end"), ("ctrl-q y", "delete_to_line_end"),
    ("ctrl-q 0", "jump_bookmark_0"), ("ctrl-q 1", "jump_bookmark_1"),
    ("ctrl-q 2", "jump_bookmark_2"), ("ctrl-q 3", "jump_bookmark_3"),
    ("ctrl-q 4", "jump_bookmark_4"), ("ctrl-q 5", "jump_bookmark_5"),
    ("ctrl-q 6", "jump_bookmark_6"), ("ctrl-q 7", "jump_bookmark_7"),
    ("ctrl-q 8", "jump_bookmark_8"), ("ctrl-q 9", "jump_bookmark_9"),
    // ^K "block/file" prefix (ctrl-held OR plain, except ^KM/^KJ plain-only)
    ("ctrl-k ctrl-s", "save"), ("ctrl-k s", "save"),
    ("ctrl-k ctrl-d", "save"), ("ctrl-k d", "save"),
    ("ctrl-k ctrl-x", "save_and_quit"), ("ctrl-k x", "save_and_quit"),
    ("ctrl-k ctrl-q", "quit"), ("ctrl-k q", "quit"),
    // Block operations (Effort 9A — reclaims ^KC/^KV from interim copy/paste)
    ("ctrl-k ctrl-b", "block_begin"),          ("ctrl-k b", "block_begin"),
    ("ctrl-k ctrl-k", "block_end"),            ("ctrl-k k", "block_end"),
    ("ctrl-k ctrl-c", "block_copy"),           ("ctrl-k c", "block_copy"),
    ("ctrl-k ctrl-v", "block_move"),           ("ctrl-k v", "block_move"),
    ("ctrl-k ctrl-y", "block_delete"),         ("ctrl-k y", "block_delete"),
    ("ctrl-k ctrl-w", "block_write"),          ("ctrl-k w", "block_write"),
    ("ctrl-k ctrl-h", "block_toggle_hidden"),  ("ctrl-k h", "block_toggle_hidden"),
    // Effort 6: send-to-scratch verbs (WordStar ^K bindings)
    ("ctrl-k ctrl-g", "copy_block_to_scratch"), ("ctrl-k g", "copy_block_to_scratch"),
    ("ctrl-k ctrl-a", "move_block_to_scratch"), ("ctrl-k a", "move_block_to_scratch"),
    // Effort 6: buffer cycling (WordStar ^K bindings — plain-only, precedent ^KM/^KJ)
    ("ctrl-k ,", "prev_buffer"),
    ("ctrl-k .", "next_buffer"),
    // Effort 6: buffer switcher (^KL / ^K^L — free; ^QL is find_next, not ^KL)
    ("ctrl-k ctrl-l", "switch_buffer"), ("ctrl-k l", "switch_buffer"),
    ("ctrl-q ctrl-b", "block_jump_begin"),     ("ctrl-q b", "block_jump_begin"),
    ("ctrl-q ctrl-k", "block_jump_end"),       ("ctrl-q k", "block_jump_end"),
    ("ctrl-k m", "set_mark"),       // plain-only (^M reserved)
    ("ctrl-k j", "jump_to_mark"),   // plain-only (^J reserved)
    ("ctrl-k 0", "set_bookmark_0"), ("ctrl-k 1", "set_bookmark_1"),
    ("ctrl-k 2", "set_bookmark_2"), ("ctrl-k 3", "set_bookmark_3"),
    ("ctrl-k 4", "set_bookmark_4"), ("ctrl-k 5", "set_bookmark_5"),
    ("ctrl-k 6", "set_bookmark_6"), ("ctrl-k 7", "set_bookmark_7"),
    ("ctrl-k 8", "set_bookmark_8"), ("ctrl-k 9", "set_bookmark_9"),
    // Kept modern keys (arrows / Home / End / Shift-select / editing)
    ("backspace", "backspace"),
    ("del",       "delete_forward"),
    ("enter",     "insert_newline"),
    ("left",  "move_left"),
    ("right", "move_right"),
    ("up",    "move_up"),
    ("down",  "move_down"),
    ("home",  "move_line_start"),
    ("end",   "move_line_end"),
    ("shift-left",  "select_left"),
    ("shift-right", "select_right"),
    ("shift-up",    "select_up"),
    ("shift-down",  "select_down"),
    ("shift-home",  "select_line_start"),
    ("shift-end",   "select_line_end"),
    // Command-surface escape hatch (D1+A5 live-sanity finding): WordStar's control
    // plane never used F-keys, and without this the preset is a keyboard trap once
    // runtime switching exists — no palette, no menu, no way back without a mouse.
    ("f10", "menu"),
    // View (E7): WordStar had no cycle-render-mode or Review binding before this
    ("f1",    "cycle_render_mode"),
    ("alt-r", "view_review"),
    // Command-surface curation (A14/A12): ten atomic edits under ^Q, toggle_scratch under ^K
    ("ctrl-q ctrl-t", "transpose_chars"),          ("ctrl-q t", "transpose_chars"),
    ("ctrl-q ctrl-w", "transpose_words"),          ("ctrl-q w", "transpose_words"),
    ("ctrl-q ctrl-n", "transpose_lines"),          ("ctrl-q n", "transpose_lines"),
    ("ctrl-q ctrl-u", "upcase"),                   ("ctrl-q u", "upcase"),
    ("ctrl-q ctrl-o", "downcase"),                 ("ctrl-q o", "downcase"),
    ("ctrl-q ctrl-g", "capitalize"),               ("ctrl-q g", "capitalize"),
    ("ctrl-q j", "join_line"),                     // plain-only (^J reserved)
    ("ctrl-q ctrl-v", "just_one_space"),           ("ctrl-q v", "just_one_space"),
    ("ctrl-q ctrl-z", "delete_blank_lines"),       ("ctrl-q z", "delete_blank_lines"),
    ("ctrl-q h", "delete_horizontal_space"),       // plain-only (^H reserved)
    ("ctrl-k ctrl-t", "toggle_scratch"),           ("ctrl-k t", "toggle_scratch"),
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
    //    Within each patch: global tables first, then the active preset's scoped table —
    //    "later file wins; within a file, specific wins" (spec D1).
    for patch in &km.patches {
        // 2a) Global bind/unbind tables.
        apply_patch_tables(&mut trie, &mut warns, reg, &patch.bind, &patch.unbind);

        // 2b) The active preset's scoped table, after the layer's global tables —
        //     "later file wins; within a file, specific wins" (spec D1).
        let scoped = match resolve_preset(&km.preset) {
            "wordstar" => patch.wordstar.as_ref(),
            _ => patch.cua.as_ref(),
        };
        if let Some(s) = scoped {
            apply_patch_tables(&mut trie, &mut warns, reg, &s.bind, &s.unbind);
        }
    }

    (trie, warns)
}

/// Apply a bind map and unbind list to `trie`, pushing warnings for bad chords /
/// unknown commands.  Shared by the global and scoped application paths so the
/// warning strings stay in one place with no duplication.
fn apply_patch_tables(
    trie: &mut KeyTrie,
    warns: &mut Vec<String>,
    reg: &Registry,
    bind: &std::collections::BTreeMap<String, String>,
    unbind: &[String],
) {
    for (chord_str, id_str) in bind {
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
        trie.bind_user(seq, id);
    }
    for chord_str in unbind {
        match parse_seq(chord_str) {
            Some(seq) => trie.unbind(&seq),
            None => warns.push(format!("config: bad unbind chord '{chord_str}'")),
        }
    }
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
                ..Default::default()
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
                crate::config::KeymapPatch { bind: Default::default(), unbind: vec!["ctrl-c".into()], ..Default::default() }, // low
                crate::config::KeymapPatch { bind: [("ctrl-c".to_string(), "copy".to_string())].into_iter().collect(), unbind: vec![], ..Default::default() }, // high
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
        // alt-k has no CUA binding (Task 3.4 claimed alt-j for join_line); use it as
        // the "unbound" sentinel.
        let k = KeyChord { code: KeyCode::Char('k'), mods: KeyModifiers::ALT };
        assert!(matches!(t.resolve(&[k]), Resolution::None));
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
        for preset in PRESETS {
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

    #[test]
    fn fold_and_outline_binds_resolve_and_do_not_collide() {
        let (km, warns) = build_keymap(&crate::config::KeymapConfig::default(), &crate::registry::Registry::builtins());
        assert!(warns.is_empty(), "no warnings expected: {warns:?}");
        let seq = |s: &str| parse_seq(s).unwrap();
        assert!(matches!(km.resolve(&seq("alt-o")),         Resolution::Command(CommandId("outline"))));
        assert!(matches!(km.resolve(&seq("alt-up")),        Resolution::Command(CommandId("heading_prev"))));
        assert!(matches!(km.resolve(&seq("alt-down")),      Resolution::Command(CommandId("heading_next"))));
        assert!(matches!(km.resolve(&seq("alt-shift-up")),  Resolution::Command(CommandId("heading_parent"))));
        assert!(matches!(km.resolve(&seq("alt-z")),         Resolution::Command(CommandId("fold_toggle"))));
        assert!(matches!(km.resolve(&seq("alt-shift-z")),   Resolution::Command(CommandId("fold_all"))));
        assert!(matches!(km.resolve(&seq("alt-shift-x")),   Resolution::Command(CommandId("unfold_all"))));
    }

    #[test]
    fn wordstar_ctrl_a_f_are_word_motions() {
        let cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] };
        let (t, w) = build_keymap(&cfg, &Registry::builtins());
        assert!(w.is_empty(), "no warnings: {w:?}");
        let a = parse_chord("ctrl-a").unwrap();
        let f = parse_chord("ctrl-f").unwrap();
        assert!(matches!(t.resolve(&[a]), Resolution::Command(CommandId("move_word_left"))), "^A = word-left");
        assert!(matches!(t.resolve(&[f]), Resolution::Command(CommandId("move_word_right"))), "^F = word-right");
    }

    #[test]
    fn wordstar_new_chords_resolve() {
        let cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] };
        let (t, w) = build_keymap(&cfg, &Registry::builtins());
        assert!(w.is_empty(), "no warnings: {w:?}");
        let seq = |s: &str| parse_seq(s).unwrap();
        let cmd = |s: &str| t.resolve(&seq(s));
        // diamond extensions
        assert!(matches!(cmd("ctrl-r"), Resolution::Command(CommandId("move_page_up"))));
        assert!(matches!(cmd("ctrl-c"), Resolution::Command(CommandId("move_page_down"))));
        assert!(matches!(cmd("ctrl-w"), Resolution::Command(CommandId("scroll_line_up"))));
        assert!(matches!(cmd("ctrl-z"), Resolution::Command(CommandId("scroll_line_down"))));
        assert!(matches!(cmd("ctrl-y"), Resolution::Command(CommandId("delete_line"))));
        assert!(matches!(cmd("ctrl-t"), Resolution::Command(CommandId("delete_word_forward"))));
        assert!(matches!(cmd("ctrl-g"), Resolution::Command(CommandId("delete_forward"))));
        assert!(matches!(cmd("ctrl-u"), Resolution::Command(CommandId("undo"))));
        assert!(matches!(cmd("ctrl-shift-u"), Resolution::Command(CommandId("redo"))));
        // ^Q quick, both forms
        assert!(matches!(cmd("ctrl-q ctrl-s"), Resolution::Command(CommandId("move_line_start"))));
        assert!(matches!(cmd("ctrl-q s"),      Resolution::Command(CommandId("move_line_start"))));
        assert!(matches!(cmd("ctrl-q e"),      Resolution::Command(CommandId("move_screen_top"))));
        assert!(matches!(cmd("ctrl-q x"),      Resolution::Command(CommandId("move_screen_bottom"))));
        assert!(matches!(cmd("ctrl-q f"),      Resolution::Command(CommandId("find"))));
        assert!(matches!(cmd("ctrl-q y"),      Resolution::Command(CommandId("delete_to_line_end"))));
        assert!(matches!(cmd("ctrl-q 0"),      Resolution::Command(CommandId("jump_bookmark_0"))));
        assert!(matches!(cmd("ctrl-q 9"),      Resolution::Command(CommandId("jump_bookmark_9"))));
        // ^K block/file, both forms + bookmarks
        assert!(matches!(cmd("ctrl-k ctrl-s"), Resolution::Command(CommandId("save"))));
        assert!(matches!(cmd("ctrl-k s"),      Resolution::Command(CommandId("save"))));
        assert!(matches!(cmd("ctrl-k x"),      Resolution::Command(CommandId("save_and_quit"))));
        assert!(matches!(cmd("ctrl-k 5"),      Resolution::Command(CommandId("set_bookmark_5"))));
        // ^KM / ^KJ plain-only; the ctrl-form must NOT be bound
        assert!(matches!(cmd("ctrl-k m"), Resolution::Command(CommandId("set_mark"))));
        assert!(matches!(cmd("ctrl-k j"), Resolution::Command(CommandId("jump_to_mark"))));
        assert!(matches!(cmd("ctrl-k ctrl-m"), Resolution::None), "^KM ctrl-form reserved, not bound");
        assert!(matches!(cmd("ctrl-k ctrl-j"), Resolution::None), "^KJ ctrl-form reserved, not bound");
    }

    #[test]
    fn view_review_binds_in_both_presets() {
        for preset in ["cua", "wordstar"] {
            let cfg = crate::config::KeymapConfig { preset: preset.into(), patches: vec![] };
            let (t, w) = build_keymap(&cfg, &Registry::builtins());
            assert!(w.is_empty(), "{preset}: no warnings: {w:?}");
            assert!(matches!(t.resolve(&parse_seq("alt-r").unwrap()),
                Resolution::Command(CommandId("view_review"))), "{preset}: alt-r → view_review");
        }
    }

    #[test]
    fn wordstar_f1_cycles_render_mode() {
        let cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] };
        let (t, _) = build_keymap(&cfg, &Registry::builtins());
        assert!(matches!(t.resolve(&parse_seq("f1").unwrap()),
            Resolution::Command(CommandId("cycle_render_mode"))));
    }

    #[test]
    fn wordstar_block_chords_resolve() {
        let cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] };
        let (t, w) = build_keymap(&cfg, &Registry::builtins());
        assert!(w.is_empty(), "{w:?}");
        let cmd = |s: &str| t.resolve(&parse_seq(s).unwrap());
        // both the plain AND ctrl-held second-key forms (9B prefix convention)
        assert!(matches!(cmd("ctrl-k b"), Resolution::Command(CommandId("block_begin"))));
        assert!(matches!(cmd("ctrl-k ctrl-b"), Resolution::Command(CommandId("block_begin"))));
        assert!(matches!(cmd("ctrl-k k"), Resolution::Command(CommandId("block_end"))));
        assert!(matches!(cmd("ctrl-k c"), Resolution::Command(CommandId("block_copy"))), "^KC reclaimed from copy");
        assert!(matches!(cmd("ctrl-k ctrl-c"), Resolution::Command(CommandId("block_copy"))), "^K^C reclaimed too");
        assert!(matches!(cmd("ctrl-k v"), Resolution::Command(CommandId("block_move"))), "^KV reclaimed from paste");
        assert!(matches!(cmd("ctrl-k ctrl-v"), Resolution::Command(CommandId("block_move"))), "^K^V reclaimed too");
        assert!(matches!(cmd("ctrl-k y"), Resolution::Command(CommandId("block_delete"))));
        assert!(matches!(cmd("ctrl-k w"), Resolution::Command(CommandId("block_write"))));
        assert!(matches!(cmd("ctrl-k h"), Resolution::Command(CommandId("block_toggle_hidden"))));
        assert!(matches!(cmd("ctrl-q b"), Resolution::Command(CommandId("block_jump_begin"))));
        assert!(matches!(cmd("ctrl-q k"), Resolution::Command(CommandId("block_jump_end"))));
        // remaining ctrl-held forms (lock the both-forms contract — Codex completeness)
        assert!(matches!(cmd("ctrl-k ctrl-k"), Resolution::Command(CommandId("block_end"))));
        assert!(matches!(cmd("ctrl-k ctrl-y"), Resolution::Command(CommandId("block_delete"))));
        assert!(matches!(cmd("ctrl-k ctrl-w"), Resolution::Command(CommandId("block_write"))));
        assert!(matches!(cmd("ctrl-k ctrl-h"), Resolution::Command(CommandId("block_toggle_hidden"))));
        assert!(matches!(cmd("ctrl-q ctrl-b"), Resolution::Command(CommandId("block_jump_begin"))));
        assert!(matches!(cmd("ctrl-q ctrl-k"), Resolution::Command(CommandId("block_jump_end"))));
    }

    #[test]
    fn switch_buffer_binds_resolve_and_do_not_collide() {
        // CUA: ctrl-shift-e (free — ctrl-e is already "filter", ctrl-shift-e is distinct)
        let (km, warns) = build_keymap(&crate::config::KeymapConfig::default(), &crate::registry::Registry::builtins());
        assert!(warns.is_empty(), "no CUA warnings expected: {warns:?}");
        let seq = |s: &str| parse_seq(s).unwrap();
        assert!(matches!(km.resolve(&seq("ctrl-shift-e")), Resolution::Command(CommandId("switch_buffer"))),
            "CUA ctrl-shift-e must resolve to switch_buffer");
        // WordStar: ctrl-k l (plain) and ctrl-k ctrl-l (ctrl-held), both free
        let ws_cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] };
        let (km_ws, warns_ws) = build_keymap(&ws_cfg, &crate::registry::Registry::builtins());
        assert!(warns_ws.is_empty(), "WordStar: no warnings expected: {warns_ws:?}");
        assert!(matches!(km_ws.resolve(&seq("ctrl-k l")), Resolution::Command(CommandId("switch_buffer"))),
            "WordStar ctrl-k l must resolve to switch_buffer");
        assert!(matches!(km_ws.resolve(&seq("ctrl-k ctrl-l")), Resolution::Command(CommandId("switch_buffer"))),
            "WordStar ctrl-k ctrl-l must resolve to switch_buffer");
        // Confirm ctrl-q l is still find_next (not shadowed)
        assert!(matches!(km_ws.resolve(&seq("ctrl-q l")), Resolution::Command(CommandId("find_next"))),
            "ctrl-q l (find_next) must not be shadowed by ctrl-k l");
    }

    #[test]
    fn cua_alt_b_promotes() {
        let (t, _) = build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        assert!(matches!(t.resolve(&parse_seq("alt-b").unwrap()), Resolution::Command(CommandId("mark_block_from_selection"))));
    }

    #[test]
    fn sentence_motions_bound_in_cua_unbound_in_wordstar() {
        let reg = Registry::builtins();
        let seq = |s: &str| parse_seq(s).unwrap();
        let (cua, warns) = build_keymap(
            &crate::config::KeymapConfig { preset: "cua".into(), patches: vec![] }, &reg);
        assert!(warns.is_empty(), "cua warns: {warns:?}");
        assert!(matches!(cua.resolve(&seq("alt-a")), Resolution::Command(CommandId("sentence_left"))));
        assert!(matches!(cua.resolve(&seq("alt-e")), Resolution::Command(CommandId("sentence_right"))));
        assert!(matches!(cua.resolve(&seq("alt-shift-a")), Resolution::Command(CommandId("select_sentence_left"))));
        assert!(matches!(cua.resolve(&seq("alt-shift-e")), Resolution::Command(CommandId("select_sentence_right"))));
        let (ws, warns) = build_keymap(
            &crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] }, &reg);
        assert!(warns.is_empty(), "wordstar warns: {warns:?}");
        // Deliberately unbound in WordStar (law 7: palette-only, no hint). ALL FOUR chords must
        // resolve to NONE of the four sentence commands (spec T-12).
        for chord in ["alt-a", "alt-e", "alt-shift-a", "alt-shift-e"] {
            assert!(!matches!(ws.resolve(&seq(chord)),
                Resolution::Command(CommandId(
                    "sentence_left" | "sentence_right"
                    | "select_sentence_left" | "select_sentence_right"))),
                "WordStar must not bind {chord} to any sentence motion (law 7: palette-only, no hint)");
        }
    }

    #[test]
    fn scratch_verbs_resolve_in_both_presets() {
        // CUA preset: single-chord bindings
        {
            let cfg = crate::config::KeymapConfig { preset: "cua".into(), patches: vec![] };
            let (t, w) = build_keymap(&cfg, &Registry::builtins());
            assert!(w.is_empty(), "cua: no warnings expected: {w:?}");
            let seq = |s: &str| parse_seq(s).unwrap();
            assert!(matches!(t.resolve(&seq("alt-shift-c")), Resolution::Command(CommandId("copy_block_to_scratch"))),
                "CUA: alt-shift-c → copy_block_to_scratch");
            assert!(matches!(t.resolve(&seq("alt-shift-v")), Resolution::Command(CommandId("move_block_to_scratch"))),
                "CUA: alt-shift-v → move_block_to_scratch");
        }
        // WordStar preset: plain second-key forms of ^K prefix bindings
        {
            let cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] };
            let (t, w) = build_keymap(&cfg, &Registry::builtins());
            assert!(w.is_empty(), "wordstar: no warnings expected: {w:?}");
            let seq = |s: &str| parse_seq(s).unwrap();
            assert!(matches!(t.resolve(&seq("ctrl-k g")), Resolution::Command(CommandId("copy_block_to_scratch"))),
                "WordStar: ctrl-k g → copy_block_to_scratch");
            assert!(matches!(t.resolve(&seq("ctrl-k a")), Resolution::Command(CommandId("move_block_to_scratch"))),
                "WordStar: ctrl-k a → move_block_to_scratch");
        }
    }

    #[test]
    fn atomic_edits_and_toggle_scratch_bound_in_both_presets() {
        // Task 3.4: the ten A14 atomic-edit commands + toggle_scratch, default-bound
        // in both presets (Codex-conflict-checked, see task-3.4 brief).
        const IDS: &[&str] = &[
            "upcase", "downcase", "capitalize", "transpose_chars", "transpose_words",
            "transpose_lines", "join_line", "just_one_space", "delete_blank_lines",
            "delete_horizontal_space", "toggle_scratch",
        ];
        let reg = Registry::builtins();

        let cua_cfg = crate::config::KeymapConfig { preset: "cua".into(), patches: vec![] };
        let (cua, warns) = build_keymap(&cua_cfg, &reg);
        assert!(warns.is_empty(), "cua: no warnings expected: {warns:?}");
        let ws_cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] };
        let (ws, warns) = build_keymap(&ws_cfg, &reg);
        assert!(warns.is_empty(), "wordstar: no warnings expected: {warns:?}");

        for id in IDS {
            assert!(cua.chord_for(CommandId(id)).is_some(), "cua: {id} has no default chord");
            assert!(ws.chord_for(CommandId(id)).is_some(), "wordstar: {id} has no default chord");
        }

        // Exact CUA chords (alt- plane).
        let seq = |s: &str| parse_seq(s).unwrap();
        assert!(matches!(cua.resolve(&seq("alt-u")), Resolution::Command(CommandId("upcase"))));
        assert!(matches!(cua.resolve(&seq("alt-l")), Resolution::Command(CommandId("downcase"))));
        assert!(matches!(cua.resolve(&seq("alt-c")), Resolution::Command(CommandId("capitalize"))));
        assert!(matches!(cua.resolve(&seq("alt-t")), Resolution::Command(CommandId("transpose_chars"))));
        assert!(matches!(cua.resolve(&seq("alt-shift-t")), Resolution::Command(CommandId("transpose_words"))));
        assert!(matches!(cua.resolve(&seq("alt-shift-l")), Resolution::Command(CommandId("transpose_lines"))));
        assert!(matches!(cua.resolve(&seq("alt-j")), Resolution::Command(CommandId("join_line"))));
        assert!(matches!(cua.resolve(&seq("alt-space")), Resolution::Command(CommandId("just_one_space"))));
        assert!(matches!(cua.resolve(&seq("alt-shift-j")), Resolution::Command(CommandId("delete_blank_lines"))));
        assert!(matches!(cua.resolve(&seq("alt-\\")), Resolution::Command(CommandId("delete_horizontal_space"))));
        assert!(matches!(cua.resolve(&seq("alt-s")), Resolution::Command(CommandId("toggle_scratch"))));

        // Exact WordStar chords — both ctrl-held and plain second-key forms under
        // the ^Q prefix, plain-only where the ctrl-form is terminal-reserved.
        assert!(matches!(ws.resolve(&seq("ctrl-q ctrl-t")), Resolution::Command(CommandId("transpose_chars"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q t")),      Resolution::Command(CommandId("transpose_chars"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q ctrl-w")), Resolution::Command(CommandId("transpose_words"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q w")),      Resolution::Command(CommandId("transpose_words"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q ctrl-n")), Resolution::Command(CommandId("transpose_lines"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q n")),      Resolution::Command(CommandId("transpose_lines"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q ctrl-u")), Resolution::Command(CommandId("upcase"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q u")),      Resolution::Command(CommandId("upcase"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q ctrl-o")), Resolution::Command(CommandId("downcase"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q o")),      Resolution::Command(CommandId("downcase"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q ctrl-g")), Resolution::Command(CommandId("capitalize"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q g")),      Resolution::Command(CommandId("capitalize"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q j")), Resolution::Command(CommandId("join_line"))),
            "^QJ plain-only (^J reserved)");
        assert!(matches!(ws.resolve(&seq("ctrl-q ctrl-v")), Resolution::Command(CommandId("just_one_space"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q v")),      Resolution::Command(CommandId("just_one_space"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q ctrl-z")), Resolution::Command(CommandId("delete_blank_lines"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q z")),      Resolution::Command(CommandId("delete_blank_lines"))));
        assert!(matches!(ws.resolve(&seq("ctrl-q h")), Resolution::Command(CommandId("delete_horizontal_space"))),
            "^QH plain-only (^H reserved)");
        assert!(matches!(ws.resolve(&seq("ctrl-k ctrl-t")), Resolution::Command(CommandId("toggle_scratch"))));
        assert!(matches!(ws.resolve(&seq("ctrl-k t")),      Resolution::Command(CommandId("toggle_scratch"))));

        // The ^Q/^K prefixes must resolve the new subtree WITHOUT shadowing existing
        // top-level binds: ^T (delete_word_forward) and ^Q/^K used as other prefixes.
        assert!(matches!(ws.resolve(&seq("ctrl-t")), Resolution::Command(CommandId("delete_word_forward"))),
            "top-level ^T must not be shadowed by ^Q^T/^QT");
        assert!(matches!(ws.resolve(&seq("ctrl-q l")), Resolution::Command(CommandId("find_next"))),
            "^QL (find_next) must not be shadowed");
        assert!(matches!(ws.resolve(&seq("ctrl-k g")), Resolution::Command(CommandId("copy_block_to_scratch"))),
            "^KG (copy_block_to_scratch) must not be shadowed by ^KT");
    }

    #[test]
    fn buffer_cycle_chords_parse_and_resolve() {
        // Codex I2: the real sequence parser is `parse_seq` (keymap.rs:109), NOT
        // `parse_chord_seq`. `parse_chord` (keymap.rs:59) parses a single chord.
        for s in ["ctrl-k ,", "ctrl-k .", "alt-,", "alt-."] {
            assert!(crate::keymap::parse_seq(s).is_some(), "parse {s}");
        }
        // Both presets build with no warnings (no collision / prefix-shadow).
        let (_t, w) = km(&[], &[], Some("wordstar"));
        assert!(w.is_empty(), "no wordstar warnings: {w:?}");
        let (_t, w) = km(&[], &[], Some("cua"));
        assert!(w.is_empty(), "no cua warnings: {w:?}");
    }

    // -----------------------------------------------------------------------
    // Task 1 (D1+A5): resolve_preset + preset-scoped keymap patches
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_preset_falls_back_to_cua() {
        assert_eq!(resolve_preset("wordstar"), "wordstar");
        assert_eq!(resolve_preset("cua"), "cua");
        assert_eq!(resolve_preset("dvorak"), "cua");
        // PRESETS is the single source preset-wide pins iterate — every entry must
        // have bindings and resolve to itself (a new preset missing either fails here).
        for p in PRESETS {
            assert!(preset_bindings(p).is_some(), "{p} must have a bindings table");
            assert_eq!(resolve_preset(p), *p, "{p} must resolve to itself");
        }
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

    #[test]
    fn every_preset_reaches_a_command_surface_by_keyboard() {
        // D1+A5 live-sanity finding: with runtime switching, a preset without any
        // keyboard route to the menu or palette strands keyboard-only users (no
        // switch-back). Every preset must bind at least one of "menu" / "palette".
        for preset in PRESETS {
            let surfaced = preset_bindings(preset).unwrap().iter()
                .any(|(_, id)| *id == "menu" || *id == "palette");
            assert!(surfaced, "{preset} must reach a command surface by keyboard");
        }
    }

    #[test]
    fn close_buffer_is_unbound_in_both_presets_by_design() {
        // C4 closure (spec D5): per-preset patches are the supported binding path.
        for preset in PRESETS {
            for (_, id) in preset_bindings(preset).unwrap() {
                assert_ne!(*id, "close_buffer", "{preset} must not bind close_buffer");
            }
        }
    }

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

    // -----------------------------------------------------------------------
    // LAW 7 (command-surface contract): hints re-resolve on preset switch
    // -----------------------------------------------------------------------

    #[test]
    fn hints_reresolve_on_preset_switch() {
        let reg = crate::registry::Registry::builtins();
        let cfg = |p: &str| crate::config::KeymapConfig { preset: p.into(), patches: vec![] };
        let (cua, _) = build_keymap(&cfg("cua"), &reg);
        let (ws, _)  = build_keymap(&cfg("wordstar"), &reg);
        // save: CUA = `ctrl-s` (shortest, 6 chars); WordStar binds save ONLY under ctrl-k combos
        // (`ctrl-k s` / `ctrl-k d` / ctrl-held forms — all 8+ chars). The two presets' save
        // hints genuinely differ.
        // (Do NOT use move_up: WordStar binds BOTH `ctrl-e` AND `up` to it, so chord_for returns
        //  "up" for both presets — a vacuous assert_ne.)
        assert_ne!(cua.chord_for(crate::registry::CommandId("save")),
                   ws.chord_for(crate::registry::CommandId("save")));
    }

    #[test]
    fn wordstar_has_no_chord_collisions_or_prefix_shadows() {
        let rows = preset_bindings("wordstar").unwrap();
        // (a) no duplicate chord maps to two ids
        let mut seen: std::collections::HashMap<Vec<KeyChord>, &str> = std::collections::HashMap::new();
        for (chord, id) in rows {
            let seq = parse_seq(chord).unwrap();
            if let Some(prev) = seen.insert(seq, id) {
                assert_eq!(prev, *id, "duplicate chord {chord} maps to {prev} AND {id}");
            }
        }
        // (b) no bound sequence is a strict prefix of another (would shadow it on exact-match)
        let seqs: Vec<Vec<KeyChord>> = rows.iter().map(|(c, _)| parse_seq(c).unwrap()).collect();
        for a in &seqs {
            for b in &seqs {
                if a.len() < b.len() && b.starts_with(a) {
                    panic!("chord {a:?} is a strict prefix of {b:?} — would shadow it");
                }
            }
        }
    }
}
