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
    pub fn insert(&mut self, c: char) { text_insert(&mut self.text, &mut self.cursor, c); }
    pub fn backspace(&mut self) { text_backspace(&mut self.text, &mut self.cursor); }
    pub fn left(&mut self) { text_left(&self.text, &mut self.cursor); }
    pub fn right(&mut self) { text_right(&self.text, &mut self.cursor); }
}

// ---------------------------------------------------------------------------
// UTF-8-codepoint-safe cursor arithmetic — free functions over (&mut String, &mut usize)
// so the file-browser destination field (`file_browser::BrowseMode::Destination`) reuses
// this rather than growing a second hand-written cursor (C5 Task 18). `Minibuffer`'s own
// insert/backspace/left/right above are one-line delegations, so both callers share ONE
// implementation.
// ---------------------------------------------------------------------------

/// Insert `c` at `cursor`, advancing by its UTF-8 length.
pub(crate) fn text_insert(text: &mut String, cursor: &mut usize, c: char) {
    text.insert(*cursor, c);
    *cursor += c.len_utf8();
}

/// Delete the codepoint before `cursor`.
pub(crate) fn text_backspace(text: &mut String, cursor: &mut usize) {
    if *cursor == 0 { return; }
    let prev = text[..*cursor].chars().next_back().expect("cursor > 0 implies a char");
    *cursor -= prev.len_utf8();
    text.remove(*cursor);
}

/// Move one codepoint left.
pub(crate) fn text_left(text: &str, cursor: &mut usize) {
    if *cursor > 0 {
        let prev = text[..*cursor].chars().next_back().expect("cursor > 0 implies a char");
        *cursor -= prev.len_utf8();
    }
}

/// Move one codepoint right.
pub(crate) fn text_right(text: &str, cursor: &mut usize) {
    if *cursor < text.len() {
        let next = text[*cursor..].chars().next().expect("cursor < len implies a char");
        *cursor += next.len_utf8();
    }
}

/// Minibuffer intercepts KEY INPUT only; non-key messages (FilterDone/JobDone/Tick)
/// fall through to the normal match arm below — a FilterDone must apply even while
/// the minibuffer is open (see test `minibuffer_does_not_starve_filterdone`).
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
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
                }
                crossterm::event::KeyCode::Enter => {
                    let mb = editor.minibuffer.take().unwrap();
                    match mb.kind {
                        MinibufferKind::Filter     => crate::prompts::submit_filter_line(editor, &mb.text, ctx.msg_tx),
                        MinibufferKind::GotoLine   => crate::prompts::goto_line_submit(editor, &mb.text),
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
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
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

    #[test]
    fn destination_field_edits_are_utf8_codepoint_safe() {
        // Byte lengths: 'a'=1, 'é'=2, '中'=3, '🙂'=4. Every assertion is on the BYTE cursor,
        // because that is what a naive `cursor += 1` gets wrong.
        let mut f = String::new();
        let mut c = 0usize;
        for ch in ['a', 'é', '中', '🙂'] { text_insert(&mut f, &mut c, ch); }
        assert_eq!(f, "aé中🙂");
        assert_eq!(c, 1 + 2 + 3 + 4, "cursor advances by UTF-8 length, not by 1 per char");

        text_left(&f, &mut c);                       // back over 🙂 (4 bytes)
        assert_eq!(c, 1 + 2 + 3);
        text_left(&f, &mut c);                       // back over 中 (3)
        assert_eq!(c, 1 + 2);
        text_right(&f, &mut c);                      // forward over 中
        assert_eq!(c, 1 + 2 + 3);

        text_backspace(&mut f, &mut c);              // delete 中
        assert_eq!(f, "aé🙂", "the codepoint BEFORE the cursor is removed whole");
        assert_eq!(c, 1 + 2);

        // Boundary: left at 0 and right at len are no-ops, never a panic or a split codepoint.
        c = 0; text_left(&f, &mut c); assert_eq!(c, 0);
        c = f.len(); text_right(&f, &mut c); assert_eq!(c, f.len());
        c = 0; text_backspace(&mut f, &mut c);
        assert_eq!(f, "aé🙂", "backspace at 0 is a no-op");
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
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clock, msg_tx: &tx, fs: &crate::test_support::test_fs() };
        let msg = crate::app::Msg::Input(Event::Key(enter_key()));
        intercept(msg, &mut editor, &ctx);

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
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clock, msg_tx: &tx, fs: &crate::test_support::test_fs() };
        let msg = crate::app::Msg::Input(Event::Key(enter_key()));
        intercept(msg, &mut editor, &ctx);

        assert!(editor.pending_plugin_calls.is_empty(), "an over-cap arg must not be enqueued");
        assert_eq!(editor.status_text(), "plugin: command arg too long");
        // A17 T5 (F4 Warning table): a Sticky Warning.
        assert_eq!(editor.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(editor.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }
}
