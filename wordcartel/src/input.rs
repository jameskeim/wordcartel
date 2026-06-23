// wordcartel/src/input.rs — CUA keymap: KeyEvent → Option<Command>.
//
// Only handles KeyEventKind::Press (ignores Release/Repeat) to avoid
// double-input on terminals that emit both.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::commands::{Command, Dir};

/// Translate a crossterm `KeyEvent` to a `Command`, or `None` for unmapped keys.
pub fn key_to_command(key: KeyEvent) -> Option<Command> {
    // Guard: only act on key-press events.
    if key.kind != KeyEventKind::Press {
        return None;
    }

    let ctrl  = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    // Alt modifier — not used in 4a, but check so we don't accidentally map
    // Alt+char as InsertChar.
    let alt   = key.modifiers.contains(KeyModifiers::ALT);

    match key.code {
        // -----------------------------------------------------------------
        // Ctrl combos (checked first so Ctrl+S is not mistaken for 's')
        // -----------------------------------------------------------------
        KeyCode::Char('z') if ctrl && !shift => Some(Command::Undo),
        KeyCode::Char('y') if ctrl           => Some(Command::Redo),
        KeyCode::Char('Z') if ctrl && shift  => Some(Command::Redo),
        KeyCode::Char('c') if ctrl           => Some(Command::Copy),
        KeyCode::Char('x') if ctrl           => Some(Command::Cut),
        KeyCode::Char('v') if ctrl           => Some(Command::Paste),
        KeyCode::Char('s') if ctrl           => Some(Command::Save),
        KeyCode::Char('q') if ctrl           => Some(Command::Quit),
        KeyCode::Char('\\') if ctrl          => Some(Command::CycleRenderMode),

        // -----------------------------------------------------------------
        // Navigation (with optional Shift to extend the selection)
        // -----------------------------------------------------------------
        KeyCode::Left  => Some(Command::Move { dir: Dir::Left,      extend: shift }),
        KeyCode::Right => Some(Command::Move { dir: Dir::Right,     extend: shift }),
        KeyCode::Up    => Some(Command::Move { dir: Dir::Up,        extend: shift }),
        KeyCode::Down  => Some(Command::Move { dir: Dir::Down,      extend: shift }),
        KeyCode::Home  => Some(Command::Move { dir: Dir::LineStart, extend: shift }),
        KeyCode::End   => Some(Command::Move { dir: Dir::LineEnd,   extend: shift }),

        // -----------------------------------------------------------------
        // Editing
        // -----------------------------------------------------------------
        KeyCode::Enter     => Some(Command::InsertNewline),
        KeyCode::Backspace => Some(Command::Backspace),
        KeyCode::Delete    => Some(Command::DeleteForward),

        // -----------------------------------------------------------------
        // F-keys: F1 cycles render mode (terminal-safe; Ctrl+\ is the alt)
        // -----------------------------------------------------------------
        KeyCode::F(1) => Some(Command::CycleRenderMode),

        // -----------------------------------------------------------------
        // Printable characters: no Ctrl, no Alt.
        // Shift alone is fine (upper-case letters / shifted symbols).
        // -----------------------------------------------------------------
        KeyCode::Char(c) if !ctrl && !alt => Some(Command::InsertChar(c)),

        // Anything else (PageUp/Down, mouse, F2–F12, etc.) → unmapped.
        _ => None,
    }
}
