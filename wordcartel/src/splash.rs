//! Startup splash / welcome overlay (spec 2026-07-09-startup-splash-design.md).
//! Branded first frame — wordmark + version + tagline + active-keymap hints —
//! dismissed (and the event consumed) by the first key press or mouse click.
//! Idle-is-free: no timers, no background work, no auto-timeout.

use crate::keymap::KeyTrie;
use crate::registry::CommandId;
use crate::app::{Handled, Msg};
use crossterm::event::{Event, KeyEventKind, MouseEventKind};
use ratatui::{layout::Rect, style::Modifier, text::{Line, Span}, widgets::{Clear, Paragraph}, Frame};
use wordcartel_core::theme::SemanticElement as SE;

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

/// The `run()` startup gate: show the splash iff it is enabled in config, not
/// suppressed by `--no-splash`, and no prompt (the swap-recovery prompt is the only
/// pre-first-draw one) is pending at launch — never bury "recover your work?".
pub fn show_at_startup(cfg_splash: bool, no_splash: bool, prompt_pending: bool) -> bool {
    cfg_splash && !no_splash && !prompt_pending
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_at_startup_truth_table() {
        assert!(show_at_startup(true, false, false), "enabled, no flag, no prompt → show");
        assert!(!show_at_startup(false, false, false), "view.splash = false wins");
        assert!(!show_at_startup(true, true, false), "--no-splash wins for this launch");
        assert!(!show_at_startup(true, false, true), "recovery prompt wins — never bury it");
        assert!(!show_at_startup(false, true, true), "all suppressors together");
    }

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
}
