// wordcartel/src/input.rs — normal-mode key handling: `handle_key` (keymap chord
// resolve → registry command dispatch → printable fallthrough), plus the CUA keymap
// translation table (KeyEvent → Option<Command>).
//
// Only handles KeyEventKind::Press (ignores Release/Repeat) to avoid
// double-input on terminals that emit both.

/// Normal-mode key dispatch: keymap chord resolve → registry command dispatch →
/// printable-character fallthrough. Extracted verbatim from `reduce`'s
/// `Msg::Input(Event::Key(k))` arm body (Effort H1 T9); the call site still runs inside
/// `reduce`'s normal `match`, so the version-hook epilogue (spec §8.1-A) still sees
/// key-driven edits — this is NOT given interception early-return semantics.
pub(crate) fn handle_key(
    k: crossterm::event::KeyEvent,
    editor: &mut crate::editor::Editor,
    reg: &crate::registry::Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    // Esc precedence (Codex CRITICAL): prompt/minibuffer Esc are handled in their
    // interception blocks ABOVE this point. Here in normal mode the order is
    // pending-cancel > filter-cancel > held-status dismiss (A17 T7, Q3). This arm
    // SUBSUMES the old standalone filter-cancel Esc check (removed above). Esc is
    // reserved for cancel/dismiss in v1 (not routed to the keymap).
    if k.code == crossterm::event::KeyCode::Esc {
        if !editor.pending_keys.is_empty() {
            editor.pending_keys.clear();
            editor.clear_transient_status();
        } else if editor.filter_in_flight.is_some() {
            editor.filter_in_flight.take().unwrap().cancel();
            editor.set_status(crate::status::StatusKind::Info, "cancelling…");
        } else {
            editor.dismiss_status();
        }
    } else if let Some(chord) = crate::keymap::from_key_event(k) {
        editor.pending_keys.push(chord);
        match keymap.resolve(&editor.pending_keys) {
            crate::keymap::Resolution::Command(id) => {
                editor.pending_keys.clear();
                editor.clear_transient_status();
                let mut ctx = crate::registry::Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
                reg.dispatch(id, &mut ctx);
                crate::app::hydrate_overlays(editor, reg, keymap);
            }
            crate::keymap::Resolution::Pending => {
                editor.set_status(crate::status::StatusKind::Info, format!("{} …", crate::keymap::chords_display(&editor.pending_keys)));
            }
            crate::keymap::Resolution::None => {
                let was_single = editor.pending_keys.len() == 1;
                editor.pending_keys.clear();
                editor.clear_transient_status();
                // Printable fallthrough: single unmodified printable → literal insert.
                if was_single {
                    if let crossterm::event::KeyCode::Char(c) = k.code {
                        if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                            && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                        {
                            crate::commands::run(crate::commands::Command::InsertChar(c), editor, clock);
                        }
                    }
                }
            }
        }
    }
}

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

        // Outline & folding (Effort 5g) — MUST precede bare Up/Down arms to avoid shadowing.
        KeyCode::Char('o') if alt && !shift  => id("outline"),
        KeyCode::Up   if alt && shift        => id("heading_parent"),
        KeyCode::Up   if alt                 => id("heading_prev"),
        KeyCode::Down if alt                 => id("heading_next"),
        KeyCode::Char('z') if alt && !shift  => id("fold_toggle"),
        KeyCode::Char('z') if alt && shift   => id("fold_all"),
        KeyCode::Char('x') if alt && shift   => id("unfold_all"),

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
