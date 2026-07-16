//! A14 — ten Emacs-parity atomic text-edit commands absent from the registry
//! (transpose, case-change, whitespace/line hygiene). Each handler follows the
//! atomic-edit template used throughout `commands/edit.rs`: compute a single
//! contiguous `(from, to)` byte range + replacement text, early-`Noop` when
//! there is nothing to do or the change is a no-op, build ONE `ChangeSet` +
//! matching `block_tree::Edit`, `editor.apply` it as a single undo step — the
//! core epilogue (`edit_apply::resettle`) settles the active buffer, so no
//! manual re-settle call follows (H22 Task 4). A leaf module — no `Command`
//! enum variant, no `commands::run` arm (module-structure GATE); `registry.rs`
//! calls these handlers directly.
//!
//! H24: every `editor.apply(...)` below drops the returned `EditOutcome` on purpose — see the
//! identical rationale in `commands/edit.rs`'s module doc (active-buffer only, so `BufferGone`
//! cannot occur; `RejectedReadOnly` already fired the loud Sticky Warning inside the funnel and
//! Q1 arbitration keeps any later success status from showing over it).

use crate::derive;
use crate::editor::Editor;
use crate::nav;
use super::CommandResult;
use wordcartel_core::block_tree::Edit;
use wordcartel_core::change::ChangeSet;
use wordcartel_core::history::{Clock, EditKind, Transaction};
use wordcartel_core::selection::Selection;

/// Scope for the three case ops (A11 convention): the non-empty selection if
/// one is active, else the word at the caret.
fn scope_or_word(editor: &Editor) -> (usize, usize) {
    let sel = editor.active().document.selection.primary();
    if !sel.is_empty() {
        (sel.from(), sel.to())
    } else {
        super::scope_range_at(editor, nav::head(editor), super::Scope::Word)
    }
}

/// `upcase` — uppercases the selection, or the word at the caret when the
/// selection is empty. Noop when the scope is empty or the uppercase mapping
/// leaves the text unchanged (e.g. a selection of `中`/`🙂`).
pub(crate) fn upcase(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let (from, to) = scope_or_word(editor);
    if from == to {
        return CommandResult::Noop;
    }
    let src = editor.active().document.buffer.slice(from..to);
    let out: String = src.chars().flat_map(char::to_uppercase).collect();
    if out == src {
        return CommandResult::Noop;
    }
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = super::build_range_replace(from, to, &out, doc_len);
    let txn = Transaction::new(cs).with_selection(Selection::range(from, from + out.len()));
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    CommandResult::Handled
}

/// `downcase` — lowercases the selection, or the word at the caret when the
/// selection is empty. Noop when the scope is empty or unchanged.
pub(crate) fn downcase(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let (from, to) = scope_or_word(editor);
    if from == to {
        return CommandResult::Noop;
    }
    let src = editor.active().document.buffer.slice(from..to);
    let out: String = src.chars().flat_map(char::to_lowercase).collect();
    if out == src {
        return CommandResult::Noop;
    }
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = super::build_range_replace(from, to, &out, doc_len);
    let txn = Transaction::new(cs).with_selection(Selection::range(from, from + out.len()));
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    CommandResult::Handled
}

/// Title-cases `src`: each word's first char uppercased, the rest lowercased;
/// non-word runs (whitespace/punctuation) pass through unchanged. Words are
/// found via `textobj::word_bounds`/`next_word_start` — the same UAX-#29
/// segmentation used elsewhere in the shell.
fn capitalize_words(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut pos = 0usize;
    while pos < src.len() {
        let (s, e) = wordcartel_core::textobj::word_bounds(src, pos);
        if s == e {
            // `pos` sits in a non-word run: copy through to the next word (or EOS).
            match wordcartel_core::textobj::next_word_start(src, pos) {
                Some(next) => {
                    out.push_str(&src[pos..next]);
                    pos = next;
                }
                None => {
                    out.push_str(&src[pos..]);
                    break;
                }
            }
        } else {
            let word = &src[s..e];
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                for c in chars {
                    out.extend(c.to_lowercase());
                }
            }
            pos = e;
        }
    }
    out
}

/// `capitalize` — title-cases the selection, or the word at the caret when the
/// selection is empty (first letter of each word up, rest down). Noop when the
/// scope is empty or unchanged.
pub(crate) fn capitalize(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let (from, to) = scope_or_word(editor);
    if from == to {
        return CommandResult::Noop;
    }
    let src = editor.active().document.buffer.slice(from..to);
    let out = capitalize_words(&src);
    if out == src {
        return CommandResult::Noop;
    }
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = super::build_range_replace(from, to, &out, doc_len);
    let txn = Transaction::new(cs).with_selection(Selection::range(from, from + out.len()));
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    CommandResult::Handled
}

/// `transpose_chars` — swaps the character immediately before the caret with
/// the one immediately after it, leaving the caret after the swapped pair.
/// Bounded to the caret's paragraph window; Noop when either side has no char
/// (start/end of paragraph). Multibyte-safe via `char_indices`/`chars` (never
/// splits a scalar value).
pub(crate) fn transpose_chars(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let (ps, pe) = {
        let b = editor.active();
        nav::paragraph_range_at(b.document.blocks(), &b.document.buffer, h)
    };
    let win = editor.active().document.buffer.slice(ps..pe);
    let rel = h.saturating_sub(ps).min(win.len());
    let Some((prev_rel, prev_ch)) = win[..rel].char_indices().next_back() else {
        return CommandResult::Noop;
    };
    let Some(next_ch) = win[rel..].chars().next() else {
        return CommandResult::Noop;
    };
    if prev_ch == next_ch {
        // Doubled letter ("book", "aa"): the swap is byte-identical — Noop so it
        // never dirties the buffer or pushes an empty undo step.
        return CommandResult::Noop;
    }
    let from = ps + prev_rel;
    let to = ps + rel + next_ch.len_utf8();
    let out = format!("{next_ch}{prev_ch}");
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = super::build_range_replace(from, to, &out, doc_len);
    let txn = Transaction::new(cs).with_selection(Selection::single(from + out.len()));
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    CommandResult::Handled
}

/// `transpose_words` — swaps the word before the caret with the word at/after
/// it, preserving the exact text between them (spaces/punctuation untouched).
/// The caret lands at the end of the swapped region (after the word that is
/// now second). Noop when fewer than two words are available in the caret's
/// paragraph, or the two words are textually identical (no visible change).
pub(crate) fn transpose_words(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let (ps, pe) = {
        let b = editor.active();
        nav::paragraph_range_at(b.document.blocks(), &b.document.buffer, h)
    };
    let win = editor.active().document.buffer.slice(ps..pe);
    let rel = h.saturating_sub(ps).min(win.len());

    // word2: the word at the caret, or the next one if the caret sits in whitespace.
    let (w2s, w2e) = {
        let (s, e) = wordcartel_core::textobj::word_bounds(&win, rel);
        if s == e {
            match wordcartel_core::textobj::next_word_start(&win, rel) {
                Some(start) => wordcartel_core::textobj::word_bounds(&win, start),
                None => return CommandResult::Noop,
            }
        } else {
            (s, e)
        }
    };
    // word1: the nearest word strictly before word2.
    let (w1s, w1e) = match wordcartel_core::textobj::prev_word_start(&win, w2s) {
        Some(start) => wordcartel_core::textobj::word_bounds(&win, start),
        None => return CommandResult::Noop,
    };
    if w1e > w2s {
        return CommandResult::Noop; // overlap guard — defensive, should not occur
    }

    let word1 = &win[w1s..w1e];
    let gap = &win[w1e..w2s];
    let word2 = &win[w2s..w2e];
    if word1 == word2 {
        return CommandResult::Noop;
    }
    let out = format!("{word2}{gap}{word1}");

    let from = ps + w1s;
    let to = ps + w2e;
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = super::build_range_replace(from, to, &out, doc_len);
    let txn = Transaction::new(cs).with_selection(Selection::single(from + out.len()));
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    CommandResult::Handled
}

/// `transpose_lines` — swaps the caret's logical line with the line above it;
/// the caret lands at the start of the line following the swapped pair. Noop
/// on the first line (no line above to swap with), on identical adjacent lines,
/// or on the trailing phantom line — the latter two would apply a byte-identical
/// edit.
pub(crate) fn transpose_lines(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let (from, to, out) = {
        let buf = &editor.active().document.buffer;
        let total = derive::total_logical_lines(buf);
        let l = buf.byte_to_line(h);
        if l == 0 {
            return CommandResult::Noop;
        }
        let prev_start = buf.line_to_byte(l - 1);
        let cur_start = buf.line_to_byte(l);
        let cur_end = if l + 1 < total { buf.line_to_byte(l + 1) } else { buf.len() };
        let prev_line = buf.slice(prev_start..cur_start);
        let cur_line = buf.slice(cur_start..cur_end);
        // Caret on the trailing phantom logical line (empty — no content, no newline):
        // there is nothing real to swap. Guard first (mirror join_line's discipline).
        if cur_line.is_empty() {
            return CommandResult::Noop;
        }
        // Decompose each line into content + trailing separator so the swap preserves
        // newline STRUCTURE rather than concatenating text. `prev_line` (line l-1 has
        // line l below it) always carries a trailing '\n'; `cur_line` carries one too
        // UNLESS it is the final line of a newline-less buffer ("one\ntwo") — the bug
        // this decomposition fixes (naive `{cur_line}{prev_line}` merged the two lines
        // onto one). The swapped result keeps line l's original trailing separator, so a
        // newline-less last line stays newline-less.
        let prev_content = prev_line.strip_suffix('\n').unwrap_or(&prev_line);
        let (cur_content, cur_sep) = match cur_line.strip_suffix('\n') {
            Some(c) => (c, "\n"),
            None => (cur_line.as_str(), ""),
        };
        // No-op guard: identical adjacent lines ("aa\naa\n") — the swap is byte-identical
        // (spurious dirty + empty undo step). Comparing CONTENT (not the raw slices) is
        // correct because the separators are reattached symmetrically below.
        if prev_content == cur_content {
            return CommandResult::Noop;
        }
        (prev_start, cur_end, format!("{cur_content}\n{prev_content}{cur_sep}"))
    };
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = super::build_range_replace(from, to, &out, doc_len);
    let txn = Transaction::new(cs).with_selection(Selection::single(from + out.len()));
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    CommandResult::Handled
}

/// `join_line` — joins the caret's logical line with the next one, replacing
/// the newline plus the next line's leading run of spaces/tabs with a single
/// space. Noop on the last line (no next line to join).
pub(crate) fn join_line(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let (from, to) = {
        let buf = &editor.active().document.buffer;
        let total = derive::total_logical_lines(buf);
        let l = buf.byte_to_line(h);
        if l + 1 >= total {
            return CommandResult::Noop;
        }
        let next_start = buf.line_to_byte(l + 1);
        // A trailing '\n' contributes a zero-width phantom last line (total_logical_lines'
        // documented convention) — there is nothing real to join onto beyond it.
        if next_start == buf.len() {
            return CommandResult::Noop;
        }
        let next_end = if l + 2 < total { buf.line_to_byte(l + 2) } else { buf.len() };
        let next_text = buf.slice(next_start..next_end);
        // Spaces/tabs are single-byte ASCII, so a byte count is also a char count
        // and every intermediate offset stays a valid char boundary.
        let ws: usize = next_text.bytes().take_while(|b| *b == b' ' || *b == b'\t').count();
        (next_start - 1, next_start + ws) // next_start-1 is the newline ending the caret's line
    };
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = super::build_range_replace(from, to, " ", doc_len);
    let txn = Transaction::new(cs).with_selection(Selection::single(from + 1));
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    CommandResult::Handled
}

/// Byte range of the window the intra-line whitespace ops scan for a run of
/// spaces/tabs. Normally the caret's paragraph (`nav::paragraph_range_at`), but
/// that returns an EMPTY range `(s, s)` on a blank/whitespace-only line — which
/// would hide the very spaces these ops act on (BUG: `just_one_space` then took
/// its insert path and GREW the run; `delete_horizontal_space` silently no-op'd).
/// On an empty paragraph window we therefore fall back to the caret's LOGICAL
/// LINE content (its trailing '\n' excluded). A whitespace run never crosses a
/// '\n' (newline is neither space nor tab), so on a non-blank line the paragraph
/// window and the line window yield the same run — the fallback changes only the
/// blank-line case.
fn ws_scan_window(editor: &Editor, h: usize) -> (usize, usize) {
    let b = editor.active();
    let (ps, pe) = nav::paragraph_range_at(b.document.blocks(), &b.document.buffer, h);
    if ps != pe {
        return (ps, pe);
    }
    let buf = &b.document.buffer;
    let total = derive::total_logical_lines(buf);
    let l = buf.byte_to_line(h);
    let start = buf.line_to_byte(l);
    let end = if l + 1 < total { buf.line_to_byte(l + 1) } else { buf.len() };
    // Drop a trailing '\n' so the window is exactly the line's content.
    let end = if end > start && buf.slice(end - 1..end) == "\n" { end - 1 } else { end };
    (start, end)
}

/// `just_one_space` — collapses the run of spaces/tabs touching the caret to
/// exactly one space (inserting one if the caret sits between two non-space
/// characters). Caret lands just after the space. Noop when the run is already
/// exactly one plain space (no dirty on an already-collapsed run).
pub(crate) fn just_one_space(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let (ps, pe) = ws_scan_window(editor, h);
    let win = editor.active().document.buffer.slice(ps..pe);
    let rel = h.saturating_sub(ps).min(win.len());
    let bytes = win.as_bytes();
    let mut start = rel;
    while start > 0 && (bytes[start - 1] == b' ' || bytes[start - 1] == b'\t') {
        start -= 1;
    }
    let mut end = rel;
    while end < bytes.len() && (bytes[end] == b' ' || bytes[end] == b'\t') {
        end += 1;
    }
    if &win[start..end] == " " {
        return CommandResult::Noop; // already exactly one plain space
    }
    let from = ps + start;
    let to = ps + end;
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = super::build_range_replace(from, to, " ", doc_len);
    let txn = Transaction::new(cs).with_selection(Selection::single(from + 1));
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    CommandResult::Handled
}

/// `delete_blank_lines` — Emacs `C-x C-o` semantics: on a blank line with
/// adjacent blank lines, collapses the whole run down to a single blank line;
/// on an isolated blank line, deletes it entirely; on a non-blank line,
/// deletes the following run of blank lines (if any). Noop when there is
/// nothing to collapse or delete. A "blank" line is empty or whitespace-only.
pub(crate) fn delete_blank_lines(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let (from, to) = {
        let buf = &editor.active().document.buffer;
        let total = derive::total_logical_lines(buf);
        let l = buf.byte_to_line(h);
        let is_blank = |i: usize| derive::line_text(buf, i).trim().is_empty();

        if is_blank(l) {
            let mut run_start = l;
            while run_start > 0 && is_blank(run_start - 1) {
                run_start -= 1;
            }
            let mut run_end = l;
            while run_end + 1 < total && is_blank(run_end + 1) {
                run_end += 1;
            }
            if run_start == run_end {
                // Isolated blank line: delete it entirely.
                let start = derive::line_start(buf, run_start);
                let end = if run_end + 1 < total { derive::line_start(buf, run_end + 1) } else { buf.len() };
                (start, end)
            } else {
                // Blank run: collapse to one blank line, keeping the first.
                let start = derive::line_start(buf, run_start + 1);
                let end = if run_end + 1 < total { derive::line_start(buf, run_end + 1) } else { buf.len() };
                (start, end)
            }
        } else {
            // Non-blank line: delete the following blank run, if any.
            if l + 1 >= total || !is_blank(l + 1) {
                return CommandResult::Noop;
            }
            let mut run_end = l + 1;
            while run_end + 1 < total && is_blank(run_end + 1) {
                run_end += 1;
            }
            let start = derive::line_start(buf, l + 1);
            let end = if run_end + 1 < total { derive::line_start(buf, run_end + 1) } else { buf.len() };
            (start, end)
        }
    };
    if from >= to {
        return CommandResult::Noop;
    }
    let doc_len = editor.active().document.buffer.len();
    let cs = ChangeSet::delete(from..to, doc_len);
    let edit = Edit { range: from..to, new_len: 0 };
    let txn = Transaction::new(cs).with_selection(Selection::single(from));
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    CommandResult::Handled
}

/// `delete_horizontal_space` — deletes the run of spaces/tabs immediately
/// touching the caret. Noop when the caret is not adjacent to any horizontal
/// whitespace.
pub(crate) fn delete_horizontal_space(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let (ps, pe) = ws_scan_window(editor, h);
    let win = editor.active().document.buffer.slice(ps..pe);
    let rel = h.saturating_sub(ps).min(win.len());
    let bytes = win.as_bytes();
    let mut start = rel;
    while start > 0 && (bytes[start - 1] == b' ' || bytes[start - 1] == b'\t') {
        start -= 1;
    }
    let mut end = rel;
    while end < bytes.len() && (bytes[end] == b' ' || bytes[end] == b'\t') {
        end += 1;
    }
    if start == end {
        return CommandResult::Noop;
    }
    let from = ps + start;
    let to = ps + end;
    let doc_len = editor.active().document.buffer.len();
    let cs = ChangeSet::delete(from..to, doc_len);
    let edit = Edit { range: from..to, new_len: 0 };
    let txn = Transaction::new(cs).with_selection(Selection::single(from));
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    CommandResult::Handled
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

    struct TestClock(u64);
    impl wordcartel_core::history::Clock for TestClock {
        fn now_ms(&self) -> u64 {
            self.0
        }
    }

    fn set_caret(e: &mut Editor, off: usize) {
        e.active_mut().document.selection = Selection::single(off);
        derive::rebuild(e);
    }

    fn set_selection(e: &mut Editor, from: usize, to: usize) {
        e.active_mut().document.selection = Selection::range(from, to);
        derive::rebuild(e);
    }

    // -- transpose_chars --------------------------------------------------

    #[test]
    fn transpose_chars_swaps_around_caret() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        set_caret(&mut e, 2); // between 'b' and 'c'
        let r = transpose_chars(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "acbd\n");
        assert_eq!(nav::head(&e), 3);
    }

    #[test]
    fn transpose_chars_multibyte() {
        let mut e = Editor::new_from_text("é中\n", None, (80, 24));
        set_caret(&mut e, 2); // between 'é' (2 bytes) and '中' (3 bytes)
        let r = transpose_chars(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "中é\n");
        assert_eq!(nav::head(&e), 5);
    }

    #[test]
    fn transpose_chars_noop_at_edges() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        set_caret(&mut e, 0); // start of buffer — no char before the caret
        let r = transpose_chars(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "abc\n");
        assert!(!e.active().document.dirty());
    }

    #[test]
    fn transpose_chars_noop_on_doubled_char() {
        // A doubled letter ("book" at the doubled position, or "aa"): the swap is
        // byte-identical, so the command must Noop — no dirty, no undo step.
        let mut e = Editor::new_from_text("book\n", None, (80, 24));
        set_caret(&mut e, 2); // between the two 'o's
        let before_version = e.active().document.version;
        let r = transpose_chars(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "book\n");
        assert!(!e.active().document.dirty(), "identical-char swap must not dirty the buffer");
        assert_eq!(e.active().document.version, before_version, "no undo step pushed");
    }

    // -- transpose_words ----------------------------------------------------

    #[test]
    fn transpose_words_swaps_words_keeping_gap() {
        let mut e = Editor::new_from_text("alpha   beta gamma\n", None, (80, 24));
        set_caret(&mut e, 9); // inside "beta"
        let r = transpose_words(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "beta   alpha gamma\n");
    }

    #[test]
    fn transpose_words_noop_without_two() {
        let mut e = Editor::new_from_text("alpha\n", None, (80, 24));
        set_caret(&mut e, 2); // only one word in the paragraph
        let r = transpose_words(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "alpha\n");
        assert!(!e.active().document.dirty());
    }

    // -- transpose_lines ------------------------------------------------------

    #[test]
    fn transpose_lines_swaps_with_line_above() {
        let mut e = Editor::new_from_text("one\ntwo\nthree\n", None, (80, 24));
        set_caret(&mut e, 5); // inside "two"
        let r = transpose_lines(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "two\none\nthree\n");
        assert_eq!(nav::head(&e), 8); // start of "three"
    }

    #[test]
    fn transpose_lines_swaps_when_last_line_has_no_trailing_newline() {
        // BUG-1 regression: buffer with NO trailing newline. The last line ("two")
        // carries no '\n', so a naive `{cur_line}{prev_line}` merged both lines onto
        // one ("twoone\n"). A correct swap preserves newline STRUCTURE → "two\none",
        // and the result's last line stays newline-less.
        let mut e = Editor::new_from_text("one\ntwo", None, (80, 24));
        set_caret(&mut e, 5); // inside "two" (the newline-less last line)
        let r = transpose_lines(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "two\none",
            "must swap order preserving newline structure, NOT concatenate onto one line");
    }

    #[test]
    fn transpose_lines_noop_on_first_line() {
        let mut e = Editor::new_from_text("one\ntwo\n", None, (80, 24));
        set_caret(&mut e, 1); // on the first line
        let r = transpose_lines(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "one\ntwo\n");
        assert!(!e.active().document.dirty());
    }

    #[test]
    fn transpose_lines_noop_on_identical_lines() {
        // Two identical adjacent lines: the swap is byte-identical → Noop.
        let mut e = Editor::new_from_text("aa\naa\n", None, (80, 24));
        set_caret(&mut e, 3); // on the second "aa"
        let before_version = e.active().document.version;
        let r = transpose_lines(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "aa\naa\n");
        assert!(!e.active().document.dirty(), "identical-line swap must not dirty the buffer");
        assert_eq!(e.active().document.version, before_version, "no undo step pushed");
    }

    #[test]
    fn transpose_lines_noop_on_phantom_trailing_line() {
        // Caret on the trailing phantom logical line (after the final '\n'): cur_line
        // is empty, so out == prev_line == the replaced slice — byte-identical → Noop.
        let mut e = Editor::new_from_text("one\ntwo\n", None, (80, 24));
        set_caret(&mut e, 8); // == buf.len(), the phantom line start
        let before_version = e.active().document.version;
        let r = transpose_lines(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "one\ntwo\n");
        assert!(!e.active().document.dirty(), "phantom-line swap must not dirty the buffer");
        assert_eq!(e.active().document.version, before_version, "no undo step pushed");
    }

    // -- case ops -------------------------------------------------------------

    #[test]
    fn upcase_word_at_caret_or_selection() {
        // Empty selection: word-at-caret.
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        set_caret(&mut e, 2);
        let r = upcase(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "HELLO world\n");

        // Non-empty selection wins over the caret-word scope.
        let mut e2 = Editor::new_from_text("hello world\n", None, (80, 24));
        set_selection(&mut e2, 0, 11);
        let r2 = upcase(&mut e2, &TestClock(0));
        assert_eq!(r2, CommandResult::Handled);
        assert_eq!(e2.active().document.buffer.to_string(), "HELLO WORLD\n");
    }

    #[test]
    fn downcase_word_at_caret_or_selection() {
        let mut e = Editor::new_from_text("HELLO WORLD\n", None, (80, 24));
        set_selection(&mut e, 0, 11);
        let r = downcase(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "hello world\n");
    }

    #[test]
    fn capitalize_word_at_caret_or_selection() {
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        set_selection(&mut e, 0, 11);
        let r = capitalize(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Hello World\n");
    }

    #[test]
    fn case_op_noop_when_unchanged() {
        // A selection of "中" (already both-case-invariant) is a Noop for upcase.
        let mut e = Editor::new_from_text("中🙂\n", None, (80, 24));
        set_selection(&mut e, 0, 3); // "中" is 3 bytes
        let before_version = e.active().document.version;
        let r = upcase(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "中🙂\n");
        assert!(!e.active().document.dirty(), "unchanged case-op must not dirty the buffer");
        assert_eq!(e.active().document.version, before_version, "unchanged case-op must not push an undo step");

        // "🙂" (4 bytes) is also case-invariant.
        let mut e2 = Editor::new_from_text("中🙂\n", None, (80, 24));
        set_selection(&mut e2, 3, 7);
        let r2 = downcase(&mut e2, &TestClock(0));
        assert_eq!(r2, CommandResult::Noop);
        assert!(!e2.active().document.dirty());
    }

    #[test]
    fn upcase_length_changing_maps() {
        // German eszett 'ß' (2 bytes) uppercases to "SS" (2 bytes but 2 chars) —
        // a length-preserving-in-bytes but 1-char-to-2-char mapping; the general
        // case (e.g. Turkish dotted/dotless variants) can also change byte length.
        let mut e = Editor::new_from_text("straße\n", None, (80, 24));
        set_selection(&mut e, 0, 7); // "straße" — 6 chars, 7 bytes ('ß' is 2 bytes)
        let r = upcase(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "STRASSE\n");
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (0, 7), "selection must track the widened mapping");
    }

    // -- join_line --------------------------------------------------------

    #[test]
    fn join_line_joins_next_with_single_space() {
        let mut e = Editor::new_from_text("one\n   two\nthree\n", None, (80, 24));
        set_caret(&mut e, 1); // on "one"
        let r = join_line(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "one two\nthree\n");
        assert_eq!(nav::head(&e), 4); // right after the join space
    }

    #[test]
    fn join_line_noop_on_last_line() {
        let mut e = Editor::new_from_text("one\ntwo\n", None, (80, 24));
        set_caret(&mut e, 5); // on "two" — the last real line (only the trailing phantom
                              // empty line follows, nothing real to join onto)
        let r = join_line(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "one\ntwo\n");
        assert!(!e.active().document.dirty());
    }

    // -- just_one_space -----------------------------------------------------

    #[test]
    fn just_one_space_collapses_run() {
        let mut e = Editor::new_from_text("a   b\n", None, (80, 24));
        set_caret(&mut e, 2); // inside the 3-space run
        let r = just_one_space(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "a b\n");
        assert_eq!(nav::head(&e), 2);
    }

    #[test]
    fn just_one_space_inserts_when_none() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        set_caret(&mut e, 1); // between 'a' and 'b', no whitespace
        let r = just_one_space(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "a b\n");
        assert_eq!(nav::head(&e), 2);
    }

    #[test]
    fn just_one_space_on_whitespace_only_line_collapses_and_is_idempotent() {
        // BUG-2 regression: on a whitespace-only line, nav::paragraph_range_at returns
        // an EMPTY window, so the old code missed the real spaces and took the INSERT
        // path — GROWING "    " to "     " (anti-idempotent). The empty-window fallback
        // to the logical line makes it collapse to exactly one space instead.
        let mut e = Editor::new_from_text("    ", None, (80, 24));
        set_caret(&mut e, 2); // inside the 4-space run
        let r = just_one_space(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), " ",
            "must collapse the whitespace-only line to one space, not grow it");
        assert_eq!(nav::head(&e), 1);
        // Idempotent: a second invocation on the now single-space line is a Noop.
        let r2 = just_one_space(&mut e, &TestClock(0));
        assert_eq!(r2, CommandResult::Noop, "second call must be a Noop (idempotent)");
        assert_eq!(e.active().document.buffer.to_string(), " ");
    }

    // -- delete_blank_lines ---------------------------------------------------

    #[test]
    fn delete_blank_lines_collapses_run() {
        let mut e = Editor::new_from_text("one\n\n\n\ntwo\n", None, (80, 24));
        set_caret(&mut e, 5); // on the middle of the 3-line blank run
        let r = delete_blank_lines(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "one\n\ntwo\n");
    }

    #[test]
    fn delete_blank_lines_isolated_line_is_deleted() {
        let mut e = Editor::new_from_text("one\n\ntwo\n", None, (80, 24));
        set_caret(&mut e, 4); // the isolated blank line
        let r = delete_blank_lines(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "one\ntwo\n");
    }

    #[test]
    fn delete_blank_lines_on_nonblank_deletes_following_run() {
        let mut e = Editor::new_from_text("one\n\n\ntwo\n", None, (80, 24));
        set_caret(&mut e, 1); // on "one" (non-blank)
        let r = delete_blank_lines(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "one\ntwo\n");
    }

    #[test]
    fn delete_blank_lines_noop_when_nothing_to_do() {
        let mut e = Editor::new_from_text("one\ntwo\n", None, (80, 24));
        set_caret(&mut e, 1); // non-blank line, no following blank run
        let r = delete_blank_lines(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "one\ntwo\n");
        assert!(!e.active().document.dirty());
    }

    // -- delete_horizontal_space ----------------------------------------------

    #[test]
    fn delete_horizontal_space_removes_run() {
        let mut e = Editor::new_from_text("a   b\n", None, (80, 24));
        set_caret(&mut e, 2); // inside the 3-space run
        let r = delete_horizontal_space(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");
        assert_eq!(nav::head(&e), 1);
    }

    #[test]
    fn delete_horizontal_space_noop_when_none() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        set_caret(&mut e, 1); // between 'a' and 'b', no whitespace
        let r = delete_horizontal_space(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");
        assert!(!e.active().document.dirty());
    }

    #[test]
    fn delete_horizontal_space_on_whitespace_only_line_removes_all() {
        // BUG-2 sibling: the same empty-paragraph-window root made this silently no-op
        // on a whitespace-only line. The logical-line fallback lets it delete the run.
        let mut e = Editor::new_from_text("    ", None, (80, 24));
        set_caret(&mut e, 2); // inside the 4-space run
        let r = delete_horizontal_space(&mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "",
            "must delete all horizontal whitespace on the line");
        assert_eq!(nav::head(&e), 0);
    }

    // -- undo-step invariant --------------------------------------------------

    /// A representative textop (`transpose_chars`) applies as a single history
    /// entry: one undo restores the pre-op buffer exactly.
    #[test]
    fn each_textop_is_one_undo_step() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        set_caret(&mut e, 2);
        transpose_chars(&mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "acbd\n");
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "abcd\n");
    }
}
