// wordcartel/src/app.rs — testable `step` + the real crossterm `run` loop.
//
// Design: terminal IO lives ONLY in `run`; `step` is pure and unit-testable.
// The real loop calls `step` then draws — `step` never touches the terminal.

use crossterm::event::{Event, KeyEvent};
use std::path::PathBuf;

use crate::{commands, derive, editor::Editor, file, input, render, term};
use wordcartel_core::history::Clock;

// ---------------------------------------------------------------------------
// step — pure, testable; no terminal IO
// ---------------------------------------------------------------------------

/// Translate one key event, run the resulting command (if any), then return
/// `true` while the app should keep running (`false` → caller should exit).
///
/// All editor mutation goes through `commands::run`; this function adds no
/// logic of its own beyond the translation.
pub fn step(editor: &mut Editor, key: KeyEvent, clock: &dyn Clock) -> bool {
    if let Some(cmd) = input::key_to_command(key) {
        commands::run(cmd, editor, clock);
    }
    !editor.quit
}

// ---------------------------------------------------------------------------
// SystemClock — used only by the real `run` loop, never by unit tests
// ---------------------------------------------------------------------------

struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// run — the real terminal loop; terminal IO lives entirely here
// ---------------------------------------------------------------------------

/// Open `path` (or scratch buffer), install the terminal guard, then loop:
/// draw → read event → step → repeat until `editor.quit`.
pub fn run(path: Option<PathBuf>) -> std::io::Result<()> {
    // Install the panic hook (once) so the terminal is restored on panic.
    term::install_panic_hook();

    // Determine the initial terminal size.
    let (cols, rows) = crossterm::terminal::size()?;
    let area = (cols, rows);

    // Open the file and branch on errors per §C5.
    let editor = match path.as_deref() {
        None => {
            // No path given: scratch buffer.
            Editor::new_from_text("\n", None, area)
        }
        Some(p) => match file::open(p) {
            Ok(text) => {
                Editor::new_from_text(&text, Some(p.to_path_buf()), area)
            }
            Err(file::OpenError::NotFound(_)) => {
                // New file: empty buffer NAMED with the path; first save creates it.
                let mut e = Editor::new_from_text("\n", Some(p.to_path_buf()), area);
                e.status = "new file".to_string();
                e
            }
            Err(e @ file::OpenError::Binary(_))
            | Err(e @ file::OpenError::Permission(_))
            | Err(e @ file::OpenError::IsDir(_)) => {
                // Rejected target: UNNAMED buffer so a save can't clobber it.
                let mut ed = Editor::new_from_text("\n", None, area);
                ed.status = e.to_string();
                ed
            }
            Err(e @ file::OpenError::Io(_)) => {
                // Generic IO error: also unnamed.
                let mut ed = Editor::new_from_text("\n", None, area);
                ed.status = e.to_string();
                ed
            }
        },
    };

    let mut editor = editor;

    // Install the terminal guard: enable raw mode + enter alternate screen.
    let mut guard = term::TerminalGuard::new()?;

    // Initial derive so the first draw has up-to-date layouts.
    derive::rebuild(&mut editor);

    let clock = SystemClock;

    loop {
        // Draw
        guard.terminal().draw(|f| render::render(f, &editor))?;

        // Blocking read — synchronous 4a; no worker thread.
        let ev = crossterm::event::read()?;

        match ev {
            Event::Key(key) => {
                let keep_running = step(&mut editor, key, &clock);
                if !keep_running {
                    break;
                }
                // NOTE: do NOT rebuild here. `commands::run` (called by `step`)
                // already runs `derive::rebuild` for every layout-affecting
                // command — that rebuild uses the O(1) pre-edit snapshot + Edit
                // for an O(region) incremental reparse (§3.9). A second rebuild
                // here would fire with `pre_edit_rope`/`last_edit` already taken,
                // forcing a full O(document) reparse on every keystroke.
            }
            Event::Resize(w, h) => {
                editor.view.area = (w, h);
                derive::rebuild(&mut editor);
            }
            _ => {}
        }
    }

    // Terminal restored by TerminalGuard::drop.
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — written FIRST (RED phase) before any implementation
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use crate::editor::Editor;
    use wordcartel_core::history::Clock;

    struct TestClock(u64);
    impl Clock for TestClock {
        fn now_ms(&self) -> u64 { self.0 }
    }

    /// Build a KeyEvent for a printable character (no modifiers, Press).
    fn key_char(c: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// Build a KeyEvent for Ctrl+<char> (Press).
    fn key_ctrl(c: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    // -------------------------------------------------------------------------
    // Brief's required failing test (Task 12 step 1)
    // -------------------------------------------------------------------------

    /// Feed "hi" then double Ctrl+Q; confirm the buffer holds "hi\n" and quit.
    #[test]
    fn step_processes_typing_and_quit() {
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let clk = TestClock(0);
        for c in "hi".chars() {
            crate::app::step(&mut e, key_char(c), &clk);
        }
        // First Ctrl+Q: dirty → pending confirm, NOT quit yet
        crate::app::step(&mut e, key_ctrl('q'), &clk);
        assert!(!e.quit);
        // Second Ctrl+Q: force quit
        crate::app::step(&mut e, key_ctrl('q'), &clk);
        assert!(e.quit);
        assert_eq!(e.document.buffer.to_string(), "hi\n");
    }

    // -------------------------------------------------------------------------
    // key_to_command mapping tests
    // -------------------------------------------------------------------------

    /// Ctrl+S maps to Command::Save.
    #[test]
    fn key_to_command_ctrl_s_is_save() {
        use crate::commands::Command;
        let k = KeyEvent {
            code: KeyCode::Char('s'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(crate::input::key_to_command(k), Some(Command::Save)));
    }

    /// Shift+Right maps to Move { dir: Right, extend: true }.
    #[test]
    fn key_to_command_shift_right_extends() {
        use crate::commands::{Command, Dir};
        let k = KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(
            crate::input::key_to_command(k),
            Some(Command::Move { dir: Dir::Right, extend: true })
        ));
    }

    /// A printable char maps to InsertChar.
    #[test]
    fn key_to_command_printable_is_insert_char() {
        use crate::commands::Command;
        let k = key_char('A');
        assert!(matches!(crate::input::key_to_command(k), Some(Command::InsertChar('A'))));
    }

    /// An unmapped key (F5) returns None.
    #[test]
    fn key_to_command_unmapped_is_none() {
        let k = KeyEvent {
            code: KeyCode::F(5),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(crate::input::key_to_command(k).is_none());
    }

    /// Release events return None (double-input guard).
    #[test]
    fn key_to_command_release_is_none() {
        let k = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        assert!(crate::input::key_to_command(k).is_none());
    }
}
