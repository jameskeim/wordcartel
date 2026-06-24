//! Name-keyed command registry (spec §4.4 / §10.4). key → CommandId → Handler.
//! Built-in handlers delegate to the proven `commands::run` implementations so
//! the closed `Command` enum is shared built-in *implementation*, not the
//! dispatch boundary. Plugins (Effort P) register CommandId→Handler here without
//! touching the enum.

use std::collections::HashMap;

use crate::commands::{self, Command, CommandResult, Dir};
use crate::editor::Editor;
use crate::jobs::Executor;
use crate::app::Msg;
use wordcartel_core::history::Clock;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CommandId(pub &'static str);

/// Everything a handler may touch. The executor is here so job-dispatching
/// handlers (save, swap) have it; today's built-ins ignore it.
pub struct Ctx<'a> {
    pub editor: &'a mut Editor,
    pub clock: &'a dyn Clock,
    pub executor: &'a dyn Executor,
    /// Owned `Sender` (not a borrow) because `dispatch_filter` moves a clone into a `'static` spawned thread.
    pub msg_tx: std::sync::mpsc::Sender<Msg>,
}

pub type Handler = fn(&mut Ctx) -> CommandResult;

pub struct Registry {
    map: HashMap<CommandId, Handler>,
}

impl Registry {
    pub fn builtins() -> Registry {
        let mut map: HashMap<CommandId, Handler> = HashMap::new();
        // Motions (collapse selection).
        map.insert(CommandId("move_left"),  |c| run(c, Command::Move { dir: Dir::Left,  extend: false }));
        map.insert(CommandId("move_right"), |c| run(c, Command::Move { dir: Dir::Right, extend: false }));
        map.insert(CommandId("move_up"),    |c| run(c, Command::Move { dir: Dir::Up,    extend: false }));
        map.insert(CommandId("move_down"),  |c| run(c, Command::Move { dir: Dir::Down,  extend: false }));
        map.insert(CommandId("move_line_start"), |c| run(c, Command::Move { dir: Dir::LineStart, extend: false }));
        map.insert(CommandId("move_line_end"),   |c| run(c, Command::Move { dir: Dir::LineEnd,   extend: false }));
        // Selecting motions (extend).
        map.insert(CommandId("select_left"),  |c| run(c, Command::Move { dir: Dir::Left,  extend: true }));
        map.insert(CommandId("select_right"), |c| run(c, Command::Move { dir: Dir::Right, extend: true }));
        map.insert(CommandId("select_up"),    |c| run(c, Command::Move { dir: Dir::Up,    extend: true }));
        map.insert(CommandId("select_down"),  |c| run(c, Command::Move { dir: Dir::Down,  extend: true }));
        map.insert(CommandId("select_line_start"), |c| run(c, Command::Move { dir: Dir::LineStart, extend: true }));
        map.insert(CommandId("select_line_end"),   |c| run(c, Command::Move { dir: Dir::LineEnd,   extend: true }));
        // Editing.
        map.insert(CommandId("insert_newline"), |c| run(c, Command::InsertNewline));
        map.insert(CommandId("backspace"),      |c| run(c, Command::Backspace));
        map.insert(CommandId("delete_forward"), |c| run(c, Command::DeleteForward));
        // Clipboard / history / view.
        map.insert(CommandId("copy"),  |c| run(c, Command::Copy));
        map.insert(CommandId("cut"),   |c| run(c, Command::Cut));
        map.insert(CommandId("paste"), |c| run(c, Command::Paste));
        map.insert(CommandId("undo"),  |c| run(c, Command::Undo));
        map.insert(CommandId("redo"),  |c| run(c, Command::Redo));
        map.insert(CommandId("cycle_render_mode"), |c| run(c, Command::CycleRenderMode));
        // Save / quit. (Task 9: save is now a background job dispatcher.)
        map.insert(CommandId("save"), |c| crate::save::dispatch_save(c));
        map.insert(CommandId("quit"), |c| run(c, Command::Quit));
        // Filter / minibuffer.
        map.insert(CommandId("filter"), |c| {
            c.editor.open_minibuffer("> ");
            CommandResult::Handled
        });
        // Export (pandoc presets).
        map.insert(CommandId("export_html"), |c| {
            crate::export::run_export(c.editor, "html", &c.msg_tx);
            CommandResult::Handled
        });
        map.insert(CommandId("export_docx"), |c| {
            crate::export::run_export(c.editor, "docx", &c.msg_tx);
            CommandResult::Handled
        });
        map.insert(CommandId("export_pdf"), |c| {
            crate::export::run_export(c.editor, "pdf", &c.msg_tx);
            CommandResult::Handled
        });
        // Transform chooser.
        map.insert(CommandId("transform"), |c| {
            c.editor.prompt = Some(crate::prompt::Prompt::transform_chooser());
            CommandResult::Handled
        });
        Registry { map }
    }

    /// Dispatch by id. Unknown ids surface a status (never a silent no-op, §12.5).
    pub fn dispatch(&self, id: CommandId, ctx: &mut Ctx) -> CommandResult {
        match self.map.get(&id) {
            Some(handler) => handler(ctx),
            None => {
                ctx.editor.status = format!("unknown command: {}", id.0);
                CommandResult::Noop
            }
        }
    }
}

/// Thin adapter: run a built-in `Command` against the Ctx's editor+clock.
fn run(ctx: &mut Ctx, cmd: Command) -> CommandResult {
    commands::run(cmd, ctx.editor, ctx.clock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::jobs::InlineExecutor;
    use wordcartel_core::history::Clock;

    struct Z;
    impl Clock for Z { fn now_ms(&self) -> u64 { 0 } }

    #[test]
    fn dispatch_save_id_runs_save_handler() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
        let r = reg.dispatch(CommandId("save"), &mut ctx);
        // No path → save handler reports the no-name status (delegates to run()).
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert!(e.status.contains("No file name"));
    }

    #[test]
    fn unknown_command_surfaces_status_not_silent() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
        let r = reg.dispatch(CommandId("nope"), &mut ctx);
        assert_eq!(r, crate::commands::CommandResult::Noop);
        assert!(e.status.contains("unknown command"), "must surface, never silent (§12.5)");
    }

    #[test]
    fn keymap_printable_is_insert_fallthrough_and_arrows_are_ids() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let a = KeyEvent { code: KeyCode::Char('a'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE };
        assert!(matches!(crate::input::key_to_command_id(a), Some(crate::input::KeyAction::Insert('a'))));
        let shift_left = KeyEvent { code: KeyCode::Left, modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press, state: KeyEventState::NONE };
        assert!(matches!(crate::input::key_to_command_id(shift_left),
            Some(crate::input::KeyAction::Id(CommandId("select_left")))));
    }

    #[test]
    fn keymap_ctrl_e_is_filter() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let ctrl_e = KeyEvent {
            code: KeyCode::Char('e'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(
            crate::input::key_to_command_id(ctrl_e),
            Some(crate::input::KeyAction::Id(CommandId("filter")))
        ));
    }

    #[test]
    fn keymap_ctrl_t_is_transform() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let ctrl_t = KeyEvent {
            code: KeyCode::Char('t'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(
            crate::input::key_to_command_id(ctrl_t),
            Some(crate::input::KeyAction::Id(CommandId("transform")))
        ));
    }
}
