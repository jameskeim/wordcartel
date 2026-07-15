use crate::app::Msg;
use crossterm::event::Event;

/// Single-line input widget for command prompts (e.g. the `filter` command).
///
/// The cursor is a *byte* offset into `text`, advanced/retreated by whole
/// UTF-8 codepoints.  Multibyte caret arithmetic is safe because `text` is
/// typically very short (a shell invocation).
/// What an open minibuffer's submitted line means — routes the `Enter` handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinibufferKind {
    Filter,
    GotoLine,
    /// File-name input for Save As.
    SaveAs,
    /// File-name input for Write Block (^KW).
    WriteBlock,
    /// Numeric input for Set Wrap Column.
    WrapColumn,
    /// Argument prompt for a parameterized plugin command (Task 5) — opened by
    /// `Registry::dispatch_with_arg` case 3 (declares `meta.arg == Some`, none supplied yet).
    /// Submit enqueues a `PluginCall { id, arg: Some(text) }` directly — case 2 completing.
    PluginArg { id: crate::registry::CommandId },
}

/// Ghost example hint shown after the caret on an empty Filter prompt — the
/// Filter minibuffer runs through `sh -c` (A11.4), and its shell power is
/// undiscoverable without an example.
pub(crate) const FILTER_EXAMPLE_HINT: &str =
    "  e.g. sort | uniq · fmt -w 72 · sed s/a/b/g · tr a-z A-Z · column -t";

#[derive(Debug, Clone)]
pub struct Minibuffer {
    pub prompt: String,
    pub text: String,
    pub cursor: usize, // byte offset into `text`
    pub kind: MinibufferKind,
}

impl Minibuffer {
    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.text[..self.cursor]
            .chars()
            .next_back()
            .map(char::len_utf8)
            .unwrap_or(0);
        self.cursor -= prev;
        self.text.replace_range(self.cursor..self.cursor + prev, "");
    }

    pub fn left(&mut self) {
        if self.cursor > 0 {
            let p = self.text[..self.cursor]
                .chars()
                .next_back()
                .unwrap()
                .len_utf8();
            self.cursor -= p;
        }
    }

    pub fn right(&mut self) {
        if self.cursor < self.text.len() {
            let n = self.text[self.cursor..].chars().next().unwrap().len_utf8();
            self.cursor += n;
        }
    }
}

/// Minibuffer intercepts KEY INPUT only; non-key messages (FilterDone/JobDone/Tick)
/// fall through to the normal match arm below — a FilterDone must apply even while
/// the minibuffer is open (see test `minibuffer_does_not_starve_filterdone`).
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled {
    if editor.minibuffer.is_none() { return crate::app::Handled::Pass(msg); }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            match k.code {
                crossterm::event::KeyCode::Char(c)
                    if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    editor.minibuffer.as_mut().unwrap().insert(c);
                }
                crossterm::event::KeyCode::Backspace => {
                    editor.minibuffer.as_mut().unwrap().backspace();
                }
                crossterm::event::KeyCode::Left => {
                    editor.minibuffer.as_mut().unwrap().left();
                }
                crossterm::event::KeyCode::Right => {
                    editor.minibuffer.as_mut().unwrap().right();
                }
                crossterm::event::KeyCode::Esc => {
                    // Dismiss the minibuffer (dismiss > cancel): this Esc is consumed
                    // here and does NOT reach the filter-cancel Esc check below, so
                    // any in-flight filter continues running.
                    editor.minibuffer = None;
                    // Save-As minibuffer dismiss: drop any queued post-save action.
                    editor.pending_save_as = None;
                    // Effort 6 (Codex C2): dismissing a drain's Save-As aborts the quit.
                    editor.quit_drain = None;
                    editor.quit_drain_advance = false;
                }
                crossterm::event::KeyCode::Enter => {
                    let mb = editor.minibuffer.take().unwrap();
                    match mb.kind {
                        MinibufferKind::Filter     => crate::prompts::submit_filter_line(editor, &mb.text, msg_tx),
                        MinibufferKind::GotoLine   => crate::prompts::goto_line_submit(editor, &mb.text),
                        MinibufferKind::SaveAs     => crate::prompts::save_as_submit(editor, &mb.text, ex, clock, msg_tx),
                        MinibufferKind::WriteBlock => crate::prompts::block_write_submit(editor, &mb.text),
                        MinibufferKind::WrapColumn => crate::prompts::wrap_column_submit(editor, &mb.text),
                        MinibufferKind::PluginArg { id } => {
                            if mb.text.len() > crate::limits::PLUGIN_MAX_COMMAND_ARG {
                                editor.set_status_full(crate::status::StatusKind::Warning, "plugin: command arg too long",
                                    crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                            } else {
                                editor.pending_plugin_calls.push_back(
                                    crate::plugin::PluginCall { id, arg: Some(mb.text) });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
    }
    // non-key (FilterDone/JobDone/Tick/Resize/ClipboardPaste/ClipboardAvailability) falls through to the normal match below
    crate::app::Handled::Pass(msg)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minibuffer_edits_text() {
        let mut m = Minibuffer { prompt: "> ".into(), text: String::new(), cursor: 0, kind: MinibufferKind::Filter };
        for c in "abc".chars() { m.insert(c); }
        assert_eq!((m.text.as_str(), m.cursor), ("abc", 3));
        m.left(); m.backspace();
        assert_eq!((m.text.as_str(), m.cursor), ("ac", 1));
    }

    fn enter_key() -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::Enter,
            modifiers: crossterm::event::KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    /// An open `PluginArg` minibuffer with text "25", Enter → `pending_plugin_calls` gains
    /// `PluginCall { id, arg: Some("25") }` and the minibuffer closes (Task 5's case 2
    /// completing after the prompt).
    #[test]
    fn plugin_arg_submit_enqueues_call_with_arg() {
        let id = crate::registry::CommandId(crate::plugin::intern("minibuffer-test.plugin-arg"));
        let mut editor = crate::editor::Editor::new_from_text("hi\n", None, (80, 24));
        editor.minibuffer = Some(Minibuffer {
            prompt: "Minutes:".into(), text: "25".into(), cursor: 2,
            kind: MinibufferKind::PluginArg { id },
        });
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock::new(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let msg = crate::app::Msg::Input(Event::Key(enter_key()));
        intercept(msg, &mut editor, &ex, &clock, &tx);

        assert!(editor.minibuffer.is_none(), "submit must close the minibuffer");
        assert_eq!(editor.pending_plugin_calls.len(), 1);
        assert_eq!(editor.pending_plugin_calls[0], crate::plugin::PluginCall { id, arg: Some("25".into()) });
    }

    /// An arg over `PLUGIN_MAX_COMMAND_ARG` submitted via the `PluginArg` minibuffer is rejected
    /// (typed status), nothing enqueued.
    #[test]
    fn plugin_arg_over_cap_is_rejected_at_submit() {
        let id = crate::registry::CommandId(crate::plugin::intern("minibuffer-test.plugin-arg-cap"));
        let over_cap = "a".repeat(crate::limits::PLUGIN_MAX_COMMAND_ARG + 1);
        let mut editor = crate::editor::Editor::new_from_text("hi\n", None, (80, 24));
        editor.minibuffer = Some(Minibuffer {
            prompt: "Minutes:".into(), text: over_cap, cursor: 0,
            kind: MinibufferKind::PluginArg { id },
        });
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock::new(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let msg = crate::app::Msg::Input(Event::Key(enter_key()));
        intercept(msg, &mut editor, &ex, &clock, &tx);

        assert!(editor.pending_plugin_calls.is_empty(), "an over-cap arg must not be enqueued");
        assert_eq!(editor.status_text(), "plugin: command arg too long");
        // A17 T5 (F4 Warning table): a Sticky Warning.
        assert_eq!(editor.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(editor.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }
}
