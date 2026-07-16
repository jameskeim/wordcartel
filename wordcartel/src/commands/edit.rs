//! Buffer-edit primitives behind `commands::run`. Each returns `CommandResult`
//! (the arms have early Noop guards) and preserves the exact `EditKind` of the
//! original arm. Non-Noop paths call `editor.apply`, whose core epilogue
//! (`edit_apply::resettle`) settles the active buffer — no manual re-settle
//! call needed here (H22 Task 4; `settle_after_edit` below is retained only
//! for `prose_ops::swap`'s two-rebuild fold-correction path).

use crate::derive;
use crate::nav;
use crate::editor::Editor;
use super::{replace_changeset, CommandResult};
use wordcartel_core::block_tree::Edit;
use wordcartel_core::change::ChangeSet;
use wordcartel_core::history::{Clock, EditKind, Transaction};
use wordcartel_core::register;
use wordcartel_core::selection::Selection;

/// Post-edit epilogue for the buffer-edit primitives — now a thin delegate to the shared core
/// epilogue `edit_apply::resettle` (H22 F2=A). Retained so `swap`'s rebuild #2 (prose_ops.rs)
/// keeps a `CommandResult`-returning re-settle; standard primitives stop calling it once the
/// core owns the epilogue (Task 4).
pub(super) fn settle_after_edit(editor: &mut Editor) -> CommandResult {
    crate::edit_apply::resettle(editor);
    CommandResult::Handled
}

/// `Command::InsertChar(c)` — types `c` at the caret, or replaces a non-empty
/// selection with it (CUA). Collapsed-selection inserts use `EditKind::Type`
/// (coalescing); the sel-replace path uses `EditKind::Other`.
pub(super) fn insert_char(editor: &mut Editor, c: char, clock: &dyn Clock) -> CommandResult {
    let sel = editor.active().document.selection.primary();
    if !sel.is_empty() {
        // Non-empty selection: replace it with the typed character (CUA).
        let (from, to) = (sel.from(), sel.to());
        let text = c.to_string();
        let doc_len = editor.active().document.buffer.len();
        let cs = replace_changeset(from, to, &text, doc_len);
        let edit = Edit { range: from..to, new_len: text.len() };
        let txn = Transaction::new(cs).with_selection(Selection::single(from + text.len()));
        editor.apply(txn, edit, EditKind::Other, clock);
        return CommandResult::Handled;
    }
    // Collapsed selection: normal insert-at-caret path.
    let at = nav::head(editor);
    let s = c.to_string();
    let doc_len = editor.active().document.buffer.len();
    let cs = ChangeSet::insert(at, &s, doc_len);
    let new_len = s.len(); // == c.len_utf8()
    let edit = Edit { range: at..at, new_len };
    let txn = Transaction::new(cs).with_selection(Selection::single(at + new_len));
    editor.apply(txn, edit, EditKind::Type, clock);
    CommandResult::Handled
}

/// `Command::InsertNewline` — splits the line at the caret, or replaces a
/// non-empty selection with a newline (CUA). Both paths use `EditKind::Other`.
pub(super) fn insert_newline(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let sel = editor.active().document.selection.primary();
    if !sel.is_empty() {
        // Non-empty selection: replace it with a newline (CUA).
        let (from, to) = (sel.from(), sel.to());
        let text = "\n";
        let doc_len = editor.active().document.buffer.len();
        let cs = replace_changeset(from, to, text, doc_len);
        let edit = Edit { range: from..to, new_len: text.len() };
        let txn = Transaction::new(cs).with_selection(Selection::single(from + text.len()));
        editor.apply(txn, edit, EditKind::Other, clock);
        return CommandResult::Handled;
    }
    // Collapsed selection: normal insert-newline path.
    let at = nav::head(editor);
    let s = "\n";
    let doc_len = editor.active().document.buffer.len();
    let cs = ChangeSet::insert(at, s, doc_len);
    let new_len: usize = 1;
    let edit = Edit { range: at..at, new_len };
    // EditKind::Other breaks coalescing at each newline so that undo
    // chunks per logical line rather than collapsing multi-line insertions.
    let txn = Transaction::new(cs).with_selection(Selection::single(at + new_len));
    editor.apply(txn, edit, EditKind::Other, clock);
    CommandResult::Handled
}

/// `Command::Backspace` — deletes a non-empty selection, or one grapheme left
/// of the caret. Noop at the start of the buffer.
pub(super) fn backspace(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let sel = editor.active().document.selection.primary();
    if !sel.is_empty() {
        // Non-empty selection: delete the selection range (like Cut, minus clipboard).
        let (from, to) = (sel.from(), sel.to());
        let doc_len = editor.active().document.buffer.len();
        let cs = ChangeSet::delete(from..to, doc_len);
        let edit = Edit { range: from..to, new_len: 0 };
        let txn = Transaction::new(cs).with_selection(Selection::single(from));
        editor.apply(txn, edit, EditKind::Other, clock);
        return CommandResult::Handled;
    }
    // Collapsed selection: delete one grapheme left of the caret.
    let head = nav::head(editor);
    if head == 0 {
        return CommandResult::Noop;
    }
    // Compute the grapheme-correct previous stop by reusing move_left.
    // move_left sets desired_col=None as a side-effect but does NOT change
    // the selection; it purely returns the new offset. We capture `prev`
    // here and then use it for the delete range. `head` is unchanged.
    let prev = nav::move_left(editor);
    let doc_len = editor.active().document.buffer.len();
    let cs = ChangeSet::delete(prev..head, doc_len);
    let edit = Edit { range: prev..head, new_len: 0 };
    let txn = Transaction::new(cs).with_selection(Selection::single(prev));
    editor.apply(txn, edit, EditKind::Other, clock);
    CommandResult::Handled
}

/// `Command::DeleteForward` — deletes a non-empty selection, or one grapheme
/// forward of the caret. Noop at EOF.
pub(super) fn delete_forward(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let sel = editor.active().document.selection.primary();
    if !sel.is_empty() {
        // Non-empty selection: delete the selection range (CUA, mirrors Backspace).
        let (from, to) = (sel.from(), sel.to());
        let doc_len = editor.active().document.buffer.len();
        let cs = ChangeSet::delete(from..to, doc_len);
        let edit = Edit { range: from..to, new_len: 0 };
        let txn = Transaction::new(cs).with_selection(Selection::single(from));
        editor.apply(txn, edit, EditKind::Other, clock);
        return CommandResult::Handled;
    }
    // Collapsed selection: delete one grapheme forward.
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
    let doc_len = editor.active().document.buffer.len();
    let cs = ChangeSet::delete(head..next, doc_len);
    let edit = Edit { range: head..next, new_len: 0 };
    // Caret stays at `head` after a forward delete.
    let txn = Transaction::new(cs).with_selection(Selection::single(head));
    editor.apply(txn, edit, EditKind::Other, clock);
    CommandResult::Handled
}

/// `Command::Cut` — cuts the primary selection into the register and deletes
/// it. Noop on an empty selection. Ordering is significant: `apply` first,
/// THEN the clipboard-sync-request read from `editor.register`, THEN settle.
pub(super) fn cut(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let r = editor.active().document.selection.primary();
    if r.is_empty() {
        return CommandResult::Noop;
    }
    let doc_len = editor.active().document.buffer.len();
    // Borrow the buffer before mutably borrowing editor.register (field-split no longer
    // applies now that both live under editor.active() rather than directly on Editor).
    let buf_snap = editor.active().document.buffer.clone();
    let cs = register::cut(r, doc_len, &mut editor.register, &buf_snap);
    let edit = Edit { range: r.from()..r.to(), new_len: 0 };
    let txn = Transaction::new(cs).with_selection(Selection::single(r.from()));
    editor.apply(txn, edit, EditKind::Other, clock);
    if let Some(text) = editor.register.get().map(str::to_owned) {
        editor.clipboard_sync_request = Some(text);
    }
    CommandResult::Handled
}

/// `Command::DeleteWord { back }` — deletes one word backwards or forwards
/// from the caret. Noop when the computed range is empty.
pub(super) fn delete_word(editor: &mut Editor, back: bool, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let target = if back { nav::move_word_left(editor) } else { nav::move_word_right(editor) };
    let (from, to) = if back { (target, h) } else { (h, target) };
    if from == to { return CommandResult::Noop; }
    let doc_len = editor.active().document.buffer.len();
    let cs = ChangeSet::delete(from..to, doc_len);
    let edit = Edit { range: from..to, new_len: 0 };
    let txn = Transaction::new(cs).with_selection(Selection::single(from));
    // EditKind::Other — matches existing delete commands, avoids coalescing with typed chars.
    editor.apply(txn, edit, EditKind::Other, clock);
    CommandResult::Handled
}

/// `Command::DeleteLine` — deletes the caret-head's whole logical line
/// (including its trailing newline), disregarding any active selection.
pub(super) fn delete_line(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    // Operates on the caret-head's logical line; any active selection is
    // intentionally disregarded (matches DeleteWord + faithful WordStar ^Y).
    let head = nav::head(editor);
    let len = editor.active().document.buffer.len();
    if len == 0 { return CommandResult::Noop; }
    let (from, to) = {
        let buf = &editor.active().document.buffer;
        let total = derive::total_logical_lines(buf);
        let l = buf.byte_to_line(head);
        let start = buf.line_to_byte(l);
        let end = if l + 1 < total { buf.line_to_byte(l + 1) } else { len };
        if start == end {
            // Empty line — the phantom final logical line that exists only because
            // of a trailing '\n' (start == len). Remove the preceding newline so it
            // disappears.
            if start > 0 { (start - 1, end) } else { (start, end) }
        } else if end == len && buf.slice(len - 1..len) != "\n" {
            // Final line with NO trailing newline → absorb the preceding newline too,
            // so the line fully vanishes (slice returns String).
            if start > 0 { (start - 1, end) } else { (start, end) }
        } else {
            (start, end)
        }
    };
    if from == to { return CommandResult::Noop; }
    let cs = ChangeSet::delete(from..to, len);
    let edit = Edit { range: from..to, new_len: 0 };
    let txn = Transaction::new(cs).with_selection(Selection::single(from));
    editor.apply(txn, edit, EditKind::Other, clock);
    CommandResult::Handled
}

/// `Command::DeleteToLineEnd` — deletes from the caret to the end of the
/// current logical line, keeping the newline. Noop at/after EOL.
pub(super) fn delete_to_line_end(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let head = nav::head(editor);
    let len = editor.active().document.buffer.len();
    let to = {
        let buf = &editor.active().document.buffer;
        let total = derive::total_logical_lines(buf);
        let l = buf.byte_to_line(head);
        let line_end = if l + 1 < total { buf.line_to_byte(l + 1) } else { len };
        // Keep the newline: stop before a trailing '\n' if present.
        if line_end > head && line_end > 0 && buf.slice(line_end - 1..line_end) == "\n" {
            line_end - 1
        } else {
            line_end
        }
    };
    if head >= to { return CommandResult::Noop; } // at/after EOL → no empty changeset
    let cs = ChangeSet::delete(head..to, len);
    let edit = Edit { range: head..to, new_len: 0 };
    let txn = Transaction::new(cs).with_selection(Selection::single(head));
    editor.apply(txn, edit, EditKind::Other, clock);
    CommandResult::Handled
}

#[cfg(test)]
mod tests {
    use super::*;

    // H22 Task 4 (INV-EPILOGUE regression guard): a migrated primitive re-derives correctly
    // with NO manual `settle_after_edit` call — proving the core epilogue (`editor.apply` →
    // `edit_apply::resettle`) fires on its own. Green before and after the epilogue-relocation
    // cleanup (behavior-preserving, not a red→green TDD step — see task-4-brief.md §1).
    #[test]
    fn insert_char_reparses_via_core_epilogue_without_settle_call() {
        let mut e = crate::editor::Editor::new_from_text("# H\n", None, (80, 24));
        struct C; impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { 0 } }
        insert_char(&mut e, 'x', &C);
        // Core's resettle reparsed: blocks_version tracks the new version, caret visible.
        assert_eq!(e.active().reconcile.blocks_version, e.active().document.version);
        assert_eq!(e.active().document.buffer.to_string(), "x# H\n");
    }
}
