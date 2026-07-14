//! Caret-shape picker overlay: a fixed 7-row list of (shape · blink) presets with a live
//! sample-cell preview (Fork 5-C). Selection previews immediately via the shared caret
//! setters; Esc restores the caret settings captured at open; Enter commits (the live
//! values already hold). Mirrors the theme picker's intercept/preview/commit shape, but the
//! list is FIXED (no query field), so both paste arms are pure no-ops.

use crate::config::CaretShape;
use crate::app::{Handled, Msg};
use crossterm::event::Event;

/// Live caret-picker overlay state. `selected` indexes the shape row under the cursor;
/// `original_shape`/`original_blink` snapshot the caret settings in effect when the
/// picker opened, so Esc/cancel can restore them after a live preview.
#[derive(Debug)]
pub struct CursorPicker {
    pub selected: usize,
    pub original_shape: CaretShape,
    pub original_blink: bool,
}

/// Total row → (label, glyph, shape, Option<blink>) table. `None` blink = leave
/// `caret_blink` unchanged (row 0 Default only — blink is inert under Default). Glyphs are
/// DESCRIPTIVE (honest on a DECSCUSR-ignoring terminal); the REAL preview is the live
/// sample-cell caret. Row 1 (blinking block) is the first managed stop (decision #4).
pub const ROW_ACTIONS: [(&str, &str, CaretShape, Option<bool>); 7] = [
    ("Default (terminal)",          " ",        CaretShape::Default,   None),
    ("Block \u{00b7} blinking",     "\u{2588}", CaretShape::Block,     Some(true)),
    ("Block \u{00b7} steady",       "\u{2588}", CaretShape::Block,     Some(false)),
    ("Beam \u{00b7} blinking",      "\u{258f}", CaretShape::Beam,      Some(true)),
    ("Beam \u{00b7} steady",        "\u{258f}", CaretShape::Beam,      Some(false)),
    ("Underline \u{00b7} blinking", "\u{2581}", CaretShape::Underline, Some(true)),
    ("Underline \u{00b7} steady",   "\u{2581}", CaretShape::Underline, Some(false)),
];

/// The initial highlighted row for a caret currently at `(shape, blink)`. Under `Default`
/// the caret is the terminal's own and blink is inert, so open on the first *managed* row —
/// blinking block, row 1 (decision #4). Otherwise land on the row whose `(shape, Some(blink))`
/// matches the live caret, defaulting to row 0 if no row matches.
pub(crate) fn initial_row_for(shape: CaretShape, blink: bool) -> usize {
    if shape == CaretShape::Default {
        return 1;
    }
    ROW_ACTIONS.iter()
        .position(|(_, _, s, b)| *s == shape && *b == Some(blink))
        .unwrap_or(0)
}

/// Apply the selected row's action via the shared setters (the ONE code path — total over
/// the table). `None` blink ⇒ leave `caret_blink` untouched.
pub(crate) fn preview_selected(editor: &mut crate::editor::Editor) {
    let sel = editor.cursor_picker.as_ref().map(|p| p.selected).unwrap_or(0);
    let (_, _, shape, blink) = ROW_ACTIONS[sel.min(ROW_ACTIONS.len() - 1)];
    editor.set_caret_shape(shape);
    if let Some(b) = blink { editor.set_caret_blink(b); }
}

/// Enter-commit: the options already hold the previewed values (set live on every
/// selection change); committing is just closing the overlay.
pub(crate) fn commit_cursor_picker(editor: &mut crate::editor::Editor) {
    editor.cursor_picker = None;
}

/// Cursor-picker overlay intercepts KEY INPUT and PASTE. Esc restores the captured
/// originals and closes; Enter commits; list-nav keys move the selection and re-preview.
/// Char input is ignored (fixed list — no query). Non-key, non-paste messages fall through
/// while the picker stays open (mirrors the theme-picker block).
pub(crate) fn intercept(msg: Msg, editor: &mut crate::editor::Editor,
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>) -> Handled {
    if editor.cursor_picker.is_none() { return Handled::Pass(msg); }
    // Paste swallow FIRST. The async clipboard-paste-result arm mirrors theme_picker's
    // `Msg::ClipboardPaste` no-op drop. The bracketed-paste arm is a no-op HERE precisely
    // because the cursor picker has NO query field to append to — UNLIKE theme_picker, which
    // appends `Event::Paste` text to its query. Both arms must be consumed so neither leaks
    // into the document behind the overlay (app.rs would otherwise insert the paste text).
    if matches!(&msg, Msg::ClipboardPaste { .. }) {
        return Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
    }
    if matches!(&msg, Msg::Input(Event::Paste(_))) {
        return Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
    }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            use crossterm::event::KeyCode;
            match k.code {
                KeyCode::Esc => {
                    // cancel preview → restore the caret settings active when we opened.
                    if let Some(p) = editor.cursor_picker.take() {
                        editor.set_caret_shape(p.original_shape);
                        editor.set_caret_blink(p.original_blink);
                    }
                }
                KeyCode::Enter => { crate::cursor_picker::commit_cursor_picker(editor); }
                c if crate::list_window::list_nav_key(c).is_some() => {
                    let ah = editor.active().view.area.1;
                    if let Some(p) = editor.cursor_picker.as_mut() {
                        // Fixed 7-row list — it fits any pane, so the scroll window is a
                        // throwaway local (never rendered), but reuse the shared nav API.
                        let mut st = 0usize;
                        crate::list_window::apply_list_nav(
                            crate::list_window::list_nav_key(c).unwrap(),
                            ah, crate::cursor_picker::ROW_ACTIONS.len(), &mut p.selected, &mut st);
                    }
                    crate::cursor_picker::preview_selected(editor);
                }
                _ => {}
            }
        }
        return Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
    }
    // Non-key msg falls through to normal handling while the picker stays open.
    Handled::Pass(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    #[test]
    fn row_actions_total_and_row0_preserves_blink() {
        assert_eq!(ROW_ACTIONS.len(), 7);
        assert!(matches!(ROW_ACTIONS[0].2, crate::config::CaretShape::Default));
        assert_eq!(ROW_ACTIONS[0].3, None, "row 0 leaves caret_blink untouched");
        assert!(ROW_ACTIONS[1..].iter().all(|r| r.3.is_some()), "all concrete rows set blink");
    }

    #[test]
    fn initial_row_for_lands_on_first_managed_or_matching_row() {
        // Default → first managed stop (blinking block, row 1).
        assert_eq!(initial_row_for(CaretShape::Default, true), 1);
        assert_eq!(initial_row_for(CaretShape::Default, false), 1);
        // Concrete shapes land on the matching (shape, blink) row.
        assert_eq!(initial_row_for(CaretShape::Block, true), 1);
        assert_eq!(initial_row_for(CaretShape::Block, false), 2);
        assert_eq!(initial_row_for(CaretShape::Beam, true), 3);
        assert_eq!(initial_row_for(CaretShape::Beam, false), 4);
        assert_eq!(initial_row_for(CaretShape::Underline, true), 5);
        assert_eq!(initial_row_for(CaretShape::Underline, false), 6);
    }

    #[test]
    fn preview_applies_row_action_row0_keeps_blink_off() {
        let mut e = Editor::new_from_text("x\n", None, (40, 12));
        e.set_caret_blink(false);              // user prefers no blink
        e.open_cursor_picker();
        // select row 0 (Default) and preview → shape Default, blink UNCHANGED (still false)
        if let Some(p) = e.cursor_picker.as_mut() { p.selected = 0; }
        preview_selected(&mut e);
        assert_eq!(e.caret_shape, crate::config::CaretShape::Default);
        assert!(!e.caret_blink, "row 0 must not touch blink");
        // select row 2 (Block · steady) → shape Block, blink false
        if let Some(p) = e.cursor_picker.as_mut() { p.selected = 2; }
        preview_selected(&mut e);
        assert_eq!(e.caret_shape, crate::config::CaretShape::Block);
        assert!(!e.caret_blink);
    }

    #[test]
    fn open_cursor_picker_enforces_xor_and_captures_original() {
        let mut e = Editor::new_from_text("x\n", None, (40, 12));
        e.set_caret_shape(crate::config::CaretShape::Beam); e.set_caret_blink(true);
        e.open_palette();
        e.open_cursor_picker();
        assert!(e.cursor_picker.is_some());
        assert!(e.palette.is_none(), "opening cursor picker closes the palette (XOR)");
        let p = e.cursor_picker.as_ref().unwrap();
        assert_eq!(p.original_shape, crate::config::CaretShape::Beam);
        assert!(p.original_blink);
    }

    #[test]
    fn esc_restores_original_options() {
        let mut e = Editor::new_from_text("x\n", None, (40, 12));
        e.set_caret_shape(crate::config::CaretShape::Default); e.set_caret_blink(true);
        e.open_cursor_picker();
        if let Some(p) = e.cursor_picker.as_mut() { p.selected = 3; } // Beam · blinking
        preview_selected(&mut e);
        assert_eq!(e.caret_shape, crate::config::CaretShape::Beam);
        // simulate Esc: restore original then close
        let orig = (e.cursor_picker.as_ref().unwrap().original_shape,
                    e.cursor_picker.as_ref().unwrap().original_blink);
        e.set_caret_shape(orig.0); e.set_caret_blink(orig.1); e.cursor_picker = None;
        assert_eq!(e.caret_shape, crate::config::CaretShape::Default);
    }
}
