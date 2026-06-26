// wordcartel/src/input.rs — CUA keymap: KeyEvent → Option<Command>.
//
// Only handles KeyEventKind::Press (ignores Release/Repeat) to avoid
// double-input on terminals that emit both.

#[cfg(test)]
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[cfg(test)]
use crate::commands::{Command, Dir};
#[cfg(test)]
use crate::registry::CommandId;

/// Translate a crossterm `KeyEvent` to a `Command`, or `None` for unmapped keys.
#[cfg(test)]
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

/// What a key resolves to: a named command, or a literal character insert
/// (the §10.4 printable fallthrough — not a registered command).
///
/// Retained for tests; production dispatch now goes through the keymap trie
/// in `reduce` (Task 4).
#[cfg(test)]
#[derive(Debug)]
pub enum KeyAction {
    Id(CommandId),
    Insert(char),
}

/// Registry-facing keymap: key → CommandId (or literal insert).
///
/// Retired from production use in Task 4; kept here `#[cfg(test)]` so the
/// existing mapping tests continue to exercise the translation table.
#[cfg(test)]
pub fn key_to_command_id(key: KeyEvent) -> Option<KeyAction> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let id = |s| Some(KeyAction::Id(CommandId(s)));
    match key.code {
        KeyCode::Char('z') if ctrl && !shift => id("undo"),
        KeyCode::Char('y') if ctrl           => id("redo"),
        KeyCode::Char('Z') if ctrl && shift  => id("redo"),
        KeyCode::Char('c') if ctrl           => id("copy"),
        KeyCode::Char('x') if ctrl           => id("cut"),
        KeyCode::Char('v') if ctrl           => id("paste"),
        KeyCode::Char('s') if ctrl           => id("save"),
        KeyCode::Char('q') if ctrl           => id("quit"),
        KeyCode::Char('e') if ctrl           => id("filter"),
        KeyCode::Char('t') if ctrl           => id("transform"),
        KeyCode::Char('\\') if ctrl          => id("cycle_render_mode"),
        KeyCode::Char('f') if ctrl           => id("find"),
        KeyCode::Char('r') if ctrl           => id("replace"),
        KeyCode::F(3) if shift               => id("find_prev"),
        KeyCode::F(3)                        => id("find_next"),
        KeyCode::Char('.') if ctrl           => id("quick_fix"),
        KeyCode::F(8) if shift               => id("diag_prev"),
        KeyCode::F(8)                        => id("diag_next"),

        KeyCode::Left  => id(if shift { "select_left" } else { "move_left" }),
        KeyCode::Right => id(if shift { "select_right" } else { "move_right" }),
        KeyCode::Up    => id(if shift { "select_up" } else { "move_up" }),
        KeyCode::Down  => id(if shift { "select_down" } else { "move_down" }),
        KeyCode::Home  => id(if shift { "select_line_start" } else { "move_line_start" }),
        KeyCode::End   => id(if shift { "select_line_end" } else { "move_line_end" }),

        KeyCode::Enter     => id("insert_newline"),
        KeyCode::Backspace => id("backspace"),
        KeyCode::Delete    => id("delete_forward"),
        KeyCode::F(1)      => id("cycle_render_mode"),

        KeyCode::Char(c) if !ctrl && !alt => Some(KeyAction::Insert(c)),
        _ => None,
    }
}
