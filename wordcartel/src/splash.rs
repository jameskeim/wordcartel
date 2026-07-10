//! Startup splash / welcome overlay (spec 2026-07-09-startup-splash-design.md).
//! Branded first frame — wordmark + version + tagline + active-keymap hints —
//! dismissed (and the event consumed) by the first key press or mouse click.
//! Idle-is-free: no timers, no background work, no auto-timeout.

use crate::keymap::KeyTrie;
use crate::registry::CommandId;
use crate::app::{Handled, Msg};
use crossterm::event::{Event, KeyEventKind, MouseEventKind};

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
}
