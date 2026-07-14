//! S4 prose-surgery commands — a leaf module on the A14 template (no `Command` variant, no
//! `commands::run` arm; `registry.rs` calls these directly). Edits flow through `editor.apply`
//! (`ChangeSet`) as one undo unit. SEE==SELECT + decline route through `super::prose_sentence_at`.

use crate::editor::Editor;
use crate::nav;
use super::CommandResult;
use wordcartel_core::history::{Clock, EditKind, Transaction};
use wordcartel_core::selection::Selection;

/// `count_region` — post "N words · N sentences · N chars" for the current region (selection if
/// non-empty, else the whole buffer) to the status line. Pure report; no mutation.
pub(crate) fn count_region(editor: &mut Editor) -> CommandResult {
    let sel = editor.active().document.selection.primary();
    let text = if !sel.is_empty() {
        editor.active().document.buffer.slice(sel.from()..sel.to())
    } else {
        editor.active().document.buffer.to_string()
    };
    let st = wordcartel_core::count::region_stats(&text);
    editor.status = format!("{} words · {} sentences · {} chars", st.words, st.sentences, st.chars);
    CommandResult::Handled
}

/// Direction of a sentence reorder.
#[derive(Clone, Copy)]
enum Dir { Up, Down }

/// Move the caret's sentence up (swap with the PRECEDING sentence), preserving the gap. Caret and
/// selection travel with the moved sentence. Stops at the paragraph edge (F1); declines on
/// non-prose (F3).
pub(crate) fn move_sentence_up(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    move_sentence(editor, Dir::Up, clock)
}

/// Move the caret's sentence down (swap with the FOLLOWING sentence), preserving the gap. Caret
/// and selection travel with the moved sentence. Stops at the paragraph edge (F1); declines on
/// non-prose (F3).
pub(crate) fn move_sentence_down(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    move_sentence(editor, Dir::Down, clock)
}

/// Swap the caret's sentence A with its neighbour B within the paragraph window, PRESERVING the
/// exact inter-sentence gap (`{B}{gap}{A}` — the `transpose_words` discipline). Caret+selection land
/// on the MOVED sentence (head-at-start, F8/C-9). Stop at the paragraph edge (F1). Decline on
/// non-prose (F3). Gap fate M1: the gap between the pair is preserved verbatim.
fn move_sentence(editor: &mut Editor, dir: Dir, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    // Decline / classify via the shared predicate (SEE==SELECT).
    if super::prose_sentence_at(editor, h).is_err() {
        editor.status = "no sentence here".into();
        return CommandResult::Noop;
    }
    let (ps, pe) = {
        let b = editor.active();
        nav::paragraph_range_at(b.document.blocks(), &b.document.buffer, h)
    };
    let win = editor.active().document.buffer.slice(ps..pe);
    let rel = h.saturating_sub(ps).min(win.len());
    // Window-relative content spans.
    let spans: Vec<(usize, usize)> = wordcartel_core::textobj::sentence_spans(&win).collect();
    if spans.is_empty() { return CommandResult::Noop; }
    // Index of the caret's sentence (attach: caret in the gap → the PRECEDING span, i.e. the last
    // span whose start <= rel; before the first content → span 0).
    let cur = spans.iter().rposition(|&(s, _)| s <= rel).unwrap_or(0);
    let (a_idx, b_idx) = match dir {
        Dir::Down if cur + 1 < spans.len() => (cur, cur + 1),
        Dir::Up   if cur >= 1              => (cur - 1, cur),
        _ => {
            editor.status = "sentence at paragraph edge — break or merge to cross".into();
            return CommandResult::Noop;
        }
    };
    let (a_s, a_e) = spans[a_idx];
    let (b_s, b_e) = spans[b_idx]; // a_idx < b_idx always (ordered)
    let gap = &win[a_e..b_s];
    let out = format!("{}{}{}", &win[b_s..b_e], gap, &win[a_s..a_e]); // {B}{gap}{A}
    let from = ps + a_s;
    let to = ps + b_e;
    // The MOVED sentence is always the caret's (`cur`). In `{B}{gap}{A}` (A=spans[a_idx],
    // B=spans[b_idx]): Down → caret==a_idx (A) lands LAST; Up → caret==b_idx (B) lands FIRST.
    let (moved_from, moved_len) = if a_idx == cur {
        let a_len = a_e - a_s;
        (from + (out.len() - a_len), a_len) // Down: caret sentence lands last
    } else {
        (from, b_e - b_s)                    // Up: caret sentence lands first
    };
    let moved_to = moved_from + moved_len;
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = super::build_range_replace(from, to, &out, doc_len);
    // Head-at-start on the moved sentence (C-9): Selection::range(anchor=end, head=start).
    let txn = Transaction::new(cs).with_selection(Selection::range(moved_to, moved_from));
    editor.apply(txn, edit, EditKind::Other, clock);
    let r = super::edit::settle_after_edit(editor);
    editor.status = match dir { Dir::Up => "moved sentence up".into(), Dir::Down => "moved sentence down".into() };
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestClock(u64);
    impl wordcartel_core::history::Clock for TestClock {
        fn now_ms(&self) -> u64 { self.0 }
    }

    #[test]
    fn count_region_reports_selection_then_buffer() {
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        count_region(&mut e);
        assert!(e.status.contains("2 sentences"), "buffer: {}", e.status);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 8);
        count_region(&mut e);
        assert!(e.status.contains("1 sentences") && e.status.contains("2 words"), "sel: {}", e.status);
    }

    #[test]
    fn move_sentence_down_swaps_preserving_gap_caret_travels() {
        let mut e = Editor::new_from_text("Alpha one. Beta two. Gamma three.\n", None, (60, 12));
        crate::derive::rebuild(&mut e);
        // caret in "Alpha one."
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        assert_eq!(move_sentence_down(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Beta two. Alpha one. Gamma three.\n");
        // caret+selection now on the MOVED sentence "Alpha one." at its new position, head at start.
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "Alpha one.");
        assert_eq!(p.head, p.from());
        // repeat moves the SAME sentence again.
        assert_eq!(move_sentence_down(&mut e, &TestClock(1)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Beta two. Gamma three. Alpha one.\n");
    }

    #[test]
    fn move_sentence_up_swaps_and_caret_travels() {
        let mut e = Editor::new_from_text("Alpha one. Beta two. Gamma three.\n", None, (60, 12));
        crate::derive::rebuild(&mut e);
        // caret in the LAST sentence "Gamma three."
        let at = "Alpha one. Beta two. Gamma three.\n".find("Gamma").unwrap() + 1;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(at);
        assert_eq!(move_sentence_up(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Alpha one. Gamma three. Beta two.\n");
        // caret+selection on the MOVED sentence "Gamma three." (now at Beta's old start), head-at-start.
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "Gamma three.");
        assert_eq!(p.head, p.from());
        // repeat moves the SAME sentence up again.
        assert_eq!(move_sentence_up(&mut e, &TestClock(1)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Gamma three. Alpha one. Beta two.\n");
    }

    #[test]
    fn move_sentence_up_stops_at_paragraph_edge() {
        let mut e = Editor::new_from_text("First one. Second two.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2); // in "First one."
        assert_eq!(move_sentence_up(&mut e, &TestClock(0)), CommandResult::Noop);
        assert!(e.status.contains("edge"), "edge status: {}", e.status);
        assert_eq!(e.active().document.buffer.to_string(), "First one. Second two.\n"); // unchanged
    }

    #[test]
    fn move_sentence_declines_non_prose() {
        let mut e = Editor::new_from_text("# Heading\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(3);
        assert_eq!(move_sentence_down(&mut e, &TestClock(0)), CommandResult::Noop);
    }

    /// Single-undo granularity (spec §data-integrity): one `move_sentence_down` produces one
    /// undo step — an undo restores the pre-move buffer exactly.
    #[test]
    fn move_sentence_down_is_one_undo_step() {
        let mut e = Editor::new_from_text("Alpha one. Beta two. Gamma three.\n", None, (60, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        assert_eq!(move_sentence_down(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Beta two. Alpha one. Gamma three.\n");
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "Alpha one. Beta two. Gamma three.\n");
    }
}
