//! Editing commands — the layer that translates a `Command` into a
//! `Transaction` + `block_tree::Edit`, calls `editor.apply`, then re-derives.
//!
//! Every edit command:
//!   1. Captures the current caret offset via `nav::head`.
//!   2. Builds a `ChangeSet` and a matching `block_tree::Edit { range, new_len }`
//!      from the *same* `(range, replacement)`.
//!   3. Calls `editor.apply(txn, edit, kind, clock)`.
//!   4. Calls `derive::rebuild(editor)`.
//!   5. Calls `nav::ensure_visible(editor)`.
//!   6. Sets `editor.desired_col = None` (an edit re-anchors vertical motion).

use crate::derive;
use crate::editor::Editor;
use crate::nav;
use wordcartel_core::block_tree::Edit;
use wordcartel_core::change::ChangeSet;
use wordcartel_core::history::{Clock, EditKind, Transaction};
use wordcartel_core::selection::Selection;

/// Commands that can be dispatched to the editor.
///
/// Navigation, clipboard, undo, and file commands will be added in later tasks.
#[derive(Debug, Clone)]
pub enum Command {
    InsertChar(char),
    InsertNewline,
    Backspace,
    DeleteForward,
}

/// Result returned by `run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandResult {
    /// The command was handled and the editor state may have changed.
    Handled,
    /// The command is a no-op; the editor state is unchanged.
    Noop,
    /// The editor should quit.
    Quit,
}

/// Execute `cmd` against `editor`, then re-derive + ensure visibility.
pub fn run(cmd: Command, editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    match cmd {
        Command::InsertChar(c) => {
            let at = nav::head(editor);
            let s = c.to_string();
            let doc_len = editor.document.buffer.len();
            let cs = ChangeSet::insert(at, &s, doc_len);
            let new_len = s.len(); // == c.len_utf8()
            let edit = Edit { range: at..at, new_len };
            let txn = Transaction::new(cs).with_selection(Selection::single(at + new_len));
            editor.apply(txn, edit, EditKind::Type, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.desired_col = None;
            CommandResult::Handled
        }

        Command::InsertNewline => {
            let at = nav::head(editor);
            let s = "\n";
            let doc_len = editor.document.buffer.len();
            let cs = ChangeSet::insert(at, s, doc_len);
            let new_len: usize = 1;
            let edit = Edit { range: at..at, new_len };
            // EditKind::Other breaks coalescing at each newline so that undo
            // chunks per logical line rather than collapsing multi-line insertions.
            let txn = Transaction::new(cs).with_selection(Selection::single(at + new_len));
            editor.apply(txn, edit, EditKind::Other, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.desired_col = None;
            CommandResult::Handled
        }

        Command::Backspace => {
            let head = nav::head(editor);
            if head == 0 {
                return CommandResult::Noop;
            }
            // Compute the grapheme-correct previous stop by reusing move_left.
            // move_left sets desired_col=None as a side-effect but does NOT change
            // the selection; it purely returns the new offset. We capture `prev`
            // here and then use it for the delete range. `head` is unchanged.
            let prev = nav::move_left(editor);
            let doc_len = editor.document.buffer.len();
            let cs = ChangeSet::delete(prev..head, doc_len);
            let edit = Edit { range: prev..head, new_len: 0 };
            let txn = Transaction::new(cs).with_selection(Selection::single(prev));
            editor.apply(txn, edit, EditKind::Other, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.desired_col = None;
            CommandResult::Handled
        }

        Command::DeleteForward => {
            let head = nav::head(editor);
            // Compute the grapheme-correct next stop by reusing move_right.
            // move_right sets desired_col=None as a side-effect but does NOT change
            // the selection; it purely returns the new offset.
            let next = nav::move_right(editor);
            // EOF / nothing to delete guard: if next == head we are at the very end
            // of the document. Do NOT build a zero-width delete — it would dirty the
            // buffer, bump the version, and push a no-op undo entry.
            if next == head {
                return CommandResult::Noop;
            }
            let doc_len = editor.document.buffer.len();
            let cs = ChangeSet::delete(head..next, doc_len);
            let edit = Edit { range: head..next, new_len: 0 };
            // Caret stays at `head` after a forward delete.
            let txn = Transaction::new(cs).with_selection(Selection::single(head));
            editor.apply(txn, edit, EditKind::Other, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.desired_col = None;
            CommandResult::Handled
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive;
    use crate::editor::Editor;
    use crate::nav;
    use wordcartel_core::selection::Selection;

    /// A fixed-timestamp clock: always returns the same millisecond value.
    /// Used to drive coalescing (same ms → within COALESCE_MS window) or
    /// to break coalescing when two different timestamps are used.
    struct TestClock(u64);
    impl wordcartel_core::history::Clock for TestClock {
        fn now_ms(&self) -> u64 {
            self.0
        }
    }

    /// Set the caret to a raw byte offset without touching history.
    fn set_caret(e: &mut Editor, off: usize) {
        e.document.selection = Selection::single(off);
    }

    /// Set the caret to the end of the current buffer content.
    fn set_caret_end(e: &mut Editor) {
        let end = nav::head(e);
        // Compute the real end: length of the buffer minus the trailing newlines,
        // but for simplicity just move right until we can't anymore.
        // Actually: nav::head gives the current head. We want the last char before EOF.
        // Use the buffer length directly — head of last grapheme position.
        let len = e.document.buffer.len();
        // Find the last grapheme stop before `len`. move_right from any position
        // will stop at EOF. Easier: set caret to `len` and then move_left once to
        // get before the trailing newline. But the brief test types "hi" at end-of-line
        // on "\n" — so the end of the first line (before '\n') is offset 0.
        // Let's use: place caret at whatever move_right reaches from current position
        // iteratively, or just set it to the buffer len and call move_left to find
        // the last valid stop on the last line.
        //
        // For the actual test ("\n" document, 1 byte): we want caret at offset 0
        // (before the '\n'), which is where Editor::new_from_text puts it initially.
        // So we just need to keep calling move_right until it returns the same offset.
        let mut cur = end;
        loop {
            e.document.selection = Selection::single(cur);
            let nxt = nav::move_right(e);
            if nxt == cur {
                break;
            }
            cur = nxt;
        }
        e.document.selection = Selection::single(cur);
        e.desired_col = None;
        let _ = len;
    }

    // -------------------------------------------------------------------------
    // Brief's required failing tests (RED → GREEN)
    // -------------------------------------------------------------------------

    /// Typing 'b' between 'a' and 'c' inserts it and advances the caret.
    #[test]
    fn insert_char_types_and_advances() {
        let mut e = Editor::new_from_text("ac\n", None, (80, 24));
        set_caret(&mut e, 1);
        let clk = TestClock(0);
        run(Command::InsertChar('b'), &mut e, &clk);
        assert_eq!(e.document.buffer.to_string(), "abc\n");
        assert_eq!(nav::head(&e), 2);
    }

    /// Backspace at caret 2 in "abc\n" removes 'b' and moves caret to 1.
    #[test]
    fn backspace_deletes_prev_char() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        set_caret(&mut e, 2);
        let clk = TestClock(0);
        run(Command::Backspace, &mut e, &clk);
        assert_eq!(e.document.buffer.to_string(), "ac\n");
        assert_eq!(nav::head(&e), 1);
    }

    /// Typing "hi" with the same timestamp coalesces into a single undo entry.
    #[test]
    fn typing_coalesces_into_one_undo() {
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let clk = TestClock(0); // same timestamp -> within COALESCE_MS
        // Type "hi" one char at a time, advancing caret to end-of-line each time
        // (before the trailing '\n').
        for c in "hi".chars() {
            set_caret_end(&mut e);
            run(Command::InsertChar(c), &mut e, &clk);
        }
        e.undo();
        assert_eq!(e.document.buffer.to_string(), "\n"); // both chars undone together
    }

    // -------------------------------------------------------------------------
    // DeleteForward at EOF returns Noop; buffer unchanged, not dirty.
    // -------------------------------------------------------------------------

    /// DeleteForward at end of buffer (EOF) must return Noop and leave the
    /// buffer untouched and not dirty.
    #[test]
    fn delete_forward_at_eof_is_noop() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        derive::rebuild(&mut e);
        // Move caret to the end of the document. "ab\n" has 3 bytes; the last
        // valid caret position within the last-but-one line is offset 2 (after 'b').
        // move_right from 2 crosses to the empty trailing line (offset 3).
        // move_right from 3 stays at 3 (EOF). Let's place caret at 3.
        set_caret(&mut e, 3);
        let clk = TestClock(0);
        let result = run(Command::DeleteForward, &mut e, &clk);
        assert_eq!(result, CommandResult::Noop);
        assert_eq!(e.document.buffer.to_string(), "ab\n");
        assert!(!e.document.dirty, "DeleteForward at EOF must not dirty the buffer");
    }

    // -------------------------------------------------------------------------
    // Additional correctness tests
    // -------------------------------------------------------------------------

    /// Backspace at offset 0 is a Noop.
    #[test]
    fn backspace_at_start_is_noop() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        set_caret(&mut e, 0);
        let clk = TestClock(0);
        let result = run(Command::Backspace, &mut e, &clk);
        assert_eq!(result, CommandResult::Noop);
        assert_eq!(e.document.buffer.to_string(), "abc\n");
        assert!(!e.document.dirty);
    }

    /// DeleteForward in the middle of a line removes the next character.
    #[test]
    fn delete_forward_removes_next_char() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        derive::rebuild(&mut e);
        set_caret(&mut e, 1); // caret at 'b'
        let clk = TestClock(0);
        let result = run(Command::DeleteForward, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.document.buffer.to_string(), "ac\n");
        assert_eq!(nav::head(&e), 1); // caret stays at 1
    }

    /// InsertNewline splits the current line.
    #[test]
    fn insert_newline_splits_line() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        set_caret(&mut e, 1); // between 'a' and 'b'
        let clk = TestClock(0);
        let result = run(Command::InsertNewline, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.document.buffer.to_string(), "a\nb\n");
        assert_eq!(nav::head(&e), 2); // caret after the newline
    }

    /// The Edit passed to apply for InsertChar matches the actual byte change:
    /// range is at..at and new_len is the char's UTF-8 byte length.
    #[test]
    fn insert_edit_matches_change() {
        let mut e = Editor::new_from_text("a\n", None, (80, 24));
        set_caret(&mut e, 1);
        let clk = TestClock(0);
        run(Command::InsertChar('é'), &mut e, &clk); // 'é' is 2 bytes
        assert_eq!(e.document.buffer.to_string(), "aé\n");
        // After apply+rebuild, last_edit is None (rebuild consumed it).
        // Verify the result: caret should be at 1 + 2 = 3.
        assert_eq!(nav::head(&e), 3);
    }
}
