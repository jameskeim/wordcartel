//! Startup splash / welcome overlay (spec 2026-07-09-startup-splash-design.md).
//! Branded first frame — wordmark + version + tagline + active-keymap hints —
//! dismissed (and the event consumed) by the first key press or mouse click.
//! Idle-is-free: no timers, no background work, no auto-timeout.

use crate::keymap::KeyTrie;
use crate::registry::CommandId;

/// The splash wordmark — the app's styled-text identity (no ASCII art).
#[allow(dead_code)] // wired in Task 4 (painter)
const WORDMARK: &str = "wordcartel";
/// The tagline painted dim under the version line.
#[allow(dead_code)] // wired in Task 4 (painter)
const TAGLINE: &str = "Everyone needs a cover story";
/// The dismiss-hint footer, painted dim.
#[allow(dead_code)] // wired in Task 4 (painter)
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
