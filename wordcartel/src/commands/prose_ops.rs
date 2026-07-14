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

/// `swap` — exchange the primary `Selection` region with the `MarkedBlock` region (F2). ONE undo
/// unit via `build_multi_replace`. Overlap rejects LOUDLY (never reach the builder's silent
/// identity-no-op, spec C-2). Gap fate M2: region bytes move verbatim; outside whitespace untouched.
/// Post-op: selection holds the moved selection-content head-at-start (F8/C-9); marked_block consumed.
pub(crate) fn swap(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let sel = editor.active().document.selection.primary();
    if sel.is_empty() {
        editor.status = "swap needs a selection and a marked block".into();
        return CommandResult::Noop;
    }
    let Some(mb) = editor.active().marked_block else {
        editor.status = "swap needs a selection and a marked block".into();
        return CommandResult::Noop;
    };
    let (s_from, s_to) = (sel.from(), sel.to());
    let (m_from, m_to) = (mb.start, mb.end);
    // Order the two regions. Genuine overlap (r1_to > r2_from) is rejected loudly; ADJACENT
    // (exactly-touching, r1_to == r2_from) regions are allowed and swap correctly — adjacency
    // is not overlap, matching build_multi_replace's own well-formedness guard (w[0].1 <= w[1].0).
    let (r1_from, r1_to, r1_is_sel) = if s_from <= m_from { (s_from, s_to, true) } else { (m_from, m_to, false) };
    let (r2_from, r2_to) = if s_from <= m_from { (m_from, m_to) } else { (s_from, s_to) };
    if r1_to > r2_from {
        editor.status = "can't swap overlapping regions".into();
        return CommandResult::Noop;
    }
    let buf = &editor.active().document.buffer;
    let r1_text = buf.slice(r1_from..r1_to);
    let r2_text = buf.slice(r2_from..r2_to);
    let doc_len = buf.len();
    // ascending, non-overlapping: R1 slot ← R2 text, R2 slot ← R1 text.
    let edits = vec![
        (r1_from, r1_to, r2_text.clone()),
        (r2_from, r2_to, r1_text.clone()),
    ];
    let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
    // Where does the SELECTION's content land? If the selection was R1, its text now sits at R2's
    // slot, shifted by the first replacement's delta (len(R2)-len(R1)); if it was R2, at R1's slot.
    let l1 = r1_to - r1_from;
    let l2 = r2_to - r2_from;
    let (moved_from, moved_len) = if r1_is_sel {
        (r2_from + l2 - l1, l1) // selection was R1 → its text lands at R2 slot (shifted)
    } else {
        (r1_from, l2)           // selection was R2 → its text lands at R1 slot
    };
    let moved_to = moved_from + moved_len;
    let txn = Transaction::new(cs).with_selection(Selection::range(moved_to, moved_from));
    editor.apply(txn, edit, EditKind::Other, clock);
    editor.active_mut().marked_block = None;
    let r = super::edit::settle_after_edit(editor);
    editor.status = "swapped".into();
    r
}

/// `break_paragraph_here` — the caret's sentence (and all after it in the paragraph) becomes a new
/// paragraph. Gap fate M3: consume the single separator before the sentence, insert "\n\n". Decline
/// on non-prose; Noop if already at a paragraph start. F8: the promoted sentence is selected.
pub(crate) fn break_paragraph_here(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let (sf, st) = match super::prose_sentence_at(editor, h) {
        Ok(s) => s, Err(_) => { editor.status = "no sentence here".into(); return CommandResult::Noop; }
    };
    let (ps, _pe) = {
        let b = editor.active();
        nav::paragraph_range_at(b.document.blocks(), &b.document.buffer, h)
    };
    if sf <= ps { editor.status = "already at a paragraph start".into(); return CommandResult::Noop; }
    // Consume the whitespace run immediately before the sentence content.
    let buf = &editor.active().document.buffer;
    let head_text = buf.slice(ps..sf);
    let trimmed = head_text.trim_end_matches(char::is_whitespace).len();
    let gap_start = ps + trimmed;
    let doc_len = buf.len();
    let (cs, edit) = super::build_range_replace(gap_start, sf, "\n\n", doc_len);
    // Sentence shifts by delta = 2 - (sf - gap_start).
    let delta = 2isize - (sf - gap_start) as isize;
    let new_sf = (sf as isize + delta) as usize;
    let new_st = (st as isize + delta) as usize;
    let txn = Transaction::new(cs).with_selection(Selection::range(new_st, new_sf));
    editor.apply(txn, edit, EditKind::Other, clock);
    let r = super::edit::settle_after_edit(editor);
    editor.status = "split paragraph".into();
    r
}

/// `merge_paragraph_forward` — join the caret's paragraph with the next. Gap fate M4: replace the
/// paragraph separator with ONE space. Decline on non-prose; Noop if no next paragraph or the next
/// block is non-prose. F8: the absorbed paragraph's first sentence is selected.
pub(crate) fn merge_paragraph_forward(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    if super::prose_sentence_at(editor, h).is_err() {
        editor.status = "no paragraph here".into(); return CommandResult::Noop;
    }
    let (ps, pe) = {
        let b = editor.active();
        nav::paragraph_range_at(b.document.blocks(), &b.document.buffer, h)
    };
    let (nps, next_is_prose) = {
        let b = editor.active();
        let nps = nav::next_paragraph_start(b.document.blocks(), &b.document.buffer, pe);
        let prose = nps < b.document.buffer.len()
            && crate::ventilate::line_content_byte(&b.document.buffer, b.document.buffer.byte_to_line(nps))
                .map(|c| b.document.blocks().role_at(c) == wordcartel_core::style::BlockRole::Paragraph)
                .unwrap_or(false);
        (nps, prose)
    };
    if nps >= editor.active().document.buffer.len() {
        editor.status = "no paragraph to merge".into(); return CommandResult::Noop;
    }
    if !next_is_prose {
        editor.status = "can't merge across a non-paragraph block".into(); return CommandResult::Noop;
    }
    // `pe` (the leaf block's span end) includes the paragraph's OWN trailing line terminator, not
    // just its content — trim trailing whitespace back to the true content end so the FULL separator
    // (own newline + blank line(s)) is consumed in one shot, matching M4 (ONE space, no doubling).
    let content_end = {
        let buf = &editor.active().document.buffer;
        let para_text = buf.slice(ps..pe);
        ps + para_text.trim_end_matches(char::is_whitespace).len()
    };
    // The absorbed paragraph's first sentence begins at `nps` (its content start) → after merge it
    // sits at `content_end + 1` (one space replaces [content_end, nps)). Select it head-at-start.
    let doc_len = editor.active().document.buffer.len();
    let (cs, edit) = super::build_range_replace(content_end, nps, " ", doc_len);
    let new_start = content_end + 1;
    // Length of the absorbed first sentence: recompute from the pre-edit next paragraph window.
    let sent_len = {
        let b = editor.active();
        let (n_ps, n_pe) = nav::paragraph_range_at(b.document.blocks(), &b.document.buffer, nps);
        let nwin = b.document.buffer.slice(n_ps..n_pe);
        let first = wordcartel_core::textobj::sentence_spans(&nwin).next();
        first.map(|(s, e2)| e2 - s).unwrap_or(0)
    };
    let txn = Transaction::new(cs).with_selection(Selection::range(new_start + sent_len, new_start));
    editor.apply(txn, edit, EditKind::Other, clock);
    let r = super::edit::settle_after_edit(editor);
    editor.status = "merged paragraph".into();
    r
}

/// `split_sentence_at_caret` — turn one sentence into two at the caret. Gap fate M5: insert ". "
/// (or "." if the next char is whitespace — no double space) and uppercase the next word's initial.
/// Interior guard (finding 3): `sf < head < st` (a gap caret has head > st and is rejected).
pub(crate) fn split_sentence_at_caret(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let (sf, st) = match super::prose_sentence_at(editor, h) {
        Ok(s) => s, Err(_) => { editor.status = "no sentence here".into(); return CommandResult::Noop; }
    };
    if !(sf < h && h < st) {
        editor.status = "place the caret inside a sentence to split".into();
        return CommandResult::Noop;
    }
    let buf = &editor.active().document.buffer;
    let after = buf.slice(h..st);
    let next_is_ws = after.chars().next().is_some_and(char::is_whitespace);
    let ins = if next_is_ws { ".".to_string() } else { ". ".to_string() };
    // The next word's initial (first alphabetic at/after the caret) — the SECOND sentence's content
    // start. Capitalize it only when it is lowercase (never re-case a proper noun already capital).
    let word = after.char_indices().find(|&(_, c)| c.is_alphabetic());
    let doc_len = buf.len();
    let (edits, case_delta): (Vec<(usize, usize, String)>, isize) = match word {
        Some((off, ch)) if ch.is_lowercase() => {
            let ci = h + off;
            let upper: String = ch.to_uppercase().collect();
            let delta = upper.len() as isize - ch.len_utf8() as isize;
            // Ascending, non-overlapping (touching at h when off==0 is allowed): terminator then case-map.
            (vec![(h, h, ins.clone()), (ci, ci + ch.len_utf8(), upper)], delta)
        }
        _ => (vec![(h, h, ins.clone())], 0), // uppercase initial or no following word → terminator only
    };
    let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
    // F8: the second sentence begins at the next word's initial, shifted by the inserted terminator
    // (NOT `h + ins.len()`, which would include the retained leading space — Codex finding 3). No
    // following word → just after the terminator.
    let new_second_from = match word { Some((off, _)) => h + off + ins.len(), None => h + ins.len() };
    let new_st = (st as isize + ins.len() as isize + case_delta) as usize;
    let txn = Transaction::new(cs).with_selection(Selection::range(new_st, new_second_from));
    editor.apply(txn, edit, EditKind::Other, clock);
    let r = super::edit::settle_after_edit(editor);
    editor.status = "split sentence".into();
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

    #[test]
    fn swap_exchanges_selection_and_marked_block() {
        let mut e = Editor::new_from_text("AAAA....BBBB\n", None, (40, 12)); // sel=AAAA(0..4), block=BBBB(8..12)
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 4);
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 8, end: 12, hidden: false });
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "BBBB....AAAA\n");
        assert!(e.active().marked_block.is_none(), "marked block consumed");
        // selection holds the moved selection-content (AAAA, now at 8..12), head-at-start.
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "AAAA");
        assert_eq!(p.head, p.from());
    }

    #[test]
    fn swap_exchanges_selection_and_marked_block_reverse_order() {
        // selection AFTER the marked block: block=AAAA(0..4), sel=BBBB(8..12).
        let mut e = Editor::new_from_text("AAAA....BBBB\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(8, 12);
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false });
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "BBBB....AAAA\n");
        assert!(e.active().marked_block.is_none(), "marked block consumed");
        // selection holds the moved selection-content (BBBB, now at 0..4), head-at-start.
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "BBBB");
        assert_eq!(p.head, p.from());
    }

    #[test]
    fn swap_preserves_unequal_length_regions_and_gap() {
        // sel="AA"(0..2), gap="....."(2..7), block="CCCCC"(7..12) — unequal lengths.
        let mut e = Editor::new_from_text("AA.....CCCCC\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 2);
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 7, end: 12, hidden: false });
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "CCCCC.....AA\n", "gap untouched, unequal lengths swap correctly");
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "AA");
        assert_eq!(p.head, p.from());
    }

    #[test]
    fn swap_rejects_overlap_loudly_without_mutating() {
        let mut e = Editor::new_from_text("abcdefgh\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5);
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 3, end: 7, hidden: false });
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Noop);
        assert!(e.status.contains("overlap"), "status: {}", e.status);
        assert_eq!(e.active().document.buffer.to_string(), "abcdefgh\n", "no mutation on overlap");
    }

    #[test]
    fn swap_allows_adjacent_touching_regions() {
        // sel="AAAA"(0..4), block="BBBB"(4..8) — exactly touching, not overlapping.
        let mut e = Editor::new_from_text("AAAABBBB\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 4);
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 4, end: 8, hidden: false });
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Handled, "adjacency is not overlap");
        assert!(e.status.contains("swap"), "status: {}", e.status);
        assert_eq!(e.active().document.buffer.to_string(), "BBBBAAAA\n");
    }

    #[test]
    fn swap_requires_both_regions() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 2);
        // no marked block
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Noop);
        assert!(e.status.contains("marked block"));
    }

    #[test]
    fn swap_requires_a_selection() {
        let mut e = Editor::new_from_text("abcdef\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0); // empty selection
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 3, end: 6, hidden: false });
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Noop);
        assert!(e.status.contains("marked block"), "status: {}", e.status);
        assert_eq!(e.active().document.buffer.to_string(), "abcdef\n");
        assert!(e.active().marked_block.is_some(), "precondition failure leaves the mark untouched");
    }

    /// Single-undo granularity: one `swap` produces one undo step — an undo restores the
    /// pre-swap buffer exactly. `marked_block` does not survive undo regardless (it bypasses
    /// apply's position mapping — see `undo_clears_marked_block` in editor.rs), so a failed
    /// swap-then-undo never leaves a stale mark either.
    #[test]
    fn swap_is_one_undo_step() {
        let mut e = Editor::new_from_text("AAAA....BBBB\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 4);
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 8, end: 12, hidden: false });
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "BBBB....AAAA\n");
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "AAAA....BBBB\n");
    }

    // ------------------------------------------------------------------
    // T7: break_paragraph_here / merge_paragraph_forward / split_sentence_at_caret
    // ------------------------------------------------------------------

    #[test]
    fn break_paragraph_here_promotes_sentence() {
        let mut e = Editor::new_from_text("Alpha one. Beta two.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        let at = "Alpha one. Beta two.\n".find("Beta").unwrap();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(at);
        assert_eq!(break_paragraph_here(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Alpha one.\n\nBeta two.\n");
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "Beta two.");
        assert_eq!(p.head, p.from());
    }

    /// Multiple spaces before the promoted sentence — the WHOLE trailing-whitespace run must be
    /// trimmed and replaced with exactly "\n\n" (no stray spaces left before the break).
    #[test]
    fn break_paragraph_here_trims_multiple_trailing_spaces() {
        let mut e = Editor::new_from_text("Alpha one.   Beta two.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        let at = "Alpha one.   Beta two.\n".find("Beta").unwrap();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(at);
        assert_eq!(break_paragraph_here(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Alpha one.\n\nBeta two.\n");
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "Beta two.");
        assert_eq!(p.head, p.from());
    }

    #[test]
    fn break_paragraph_here_noop_at_paragraph_start() {
        // caret on the FIRST sentence of the paragraph — nothing precedes it to split off.
        let mut e = Editor::new_from_text("Alpha one. Beta two.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert_eq!(break_paragraph_here(&mut e, &TestClock(0)), CommandResult::Noop);
        assert!(e.status.contains("paragraph start"), "status: {}", e.status);
        assert_eq!(e.active().document.buffer.to_string(), "Alpha one. Beta two.\n");
    }

    #[test]
    fn break_paragraph_here_declines_non_prose() {
        let mut e = Editor::new_from_text("# Heading\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(3);
        assert_eq!(break_paragraph_here(&mut e, &TestClock(0)), CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "# Heading\n");
    }

    /// Single-undo granularity: one `break_paragraph_here` produces one undo step.
    #[test]
    fn break_paragraph_here_is_one_undo_step() {
        let mut e = Editor::new_from_text("Alpha one. Beta two.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        let at = "Alpha one. Beta two.\n".find("Beta").unwrap();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(at);
        assert_eq!(break_paragraph_here(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Alpha one.\n\nBeta two.\n");
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "Alpha one. Beta two.\n");
    }

    #[test]
    fn merge_paragraph_forward_single_spaces() {
        let mut e = Editor::new_from_text("Para one.\n\nPara two.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2); // in Para one
        assert_eq!(merge_paragraph_forward(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Para one. Para two.\n");
        // F8: the absorbed paragraph's first sentence is selected, head-at-start.
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "Para two.");
        assert_eq!(p.head, p.from(), "F8: caret head-at-start on the absorbed sentence");
    }

    /// Multiple blank lines between paragraphs — the whole gap (the paragraph's own trailing
    /// newline plus several blank lines) must collapse to exactly ONE space, not one per blank
    /// line, pinning the `trim_end_matches` content-end logic against a multi-blank-line run.
    #[test]
    fn merge_paragraph_forward_collapses_multiple_blank_lines() {
        let mut e = Editor::new_from_text("Para one.\n\n\n\nPara two.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2); // in Para one
        assert_eq!(merge_paragraph_forward(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Para one. Para two.\n");
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "Para two.");
        assert_eq!(p.head, p.from(), "F8: caret head-at-start on the absorbed sentence");
    }

    #[test]
    fn merge_paragraph_forward_noop_no_next_paragraph() {
        let mut e = Editor::new_from_text("Para one.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        assert_eq!(merge_paragraph_forward(&mut e, &TestClock(0)), CommandResult::Noop);
        assert!(e.status.contains("no paragraph to merge"), "status: {}", e.status);
        assert_eq!(e.active().document.buffer.to_string(), "Para one.\n");
    }

    #[test]
    fn merge_paragraph_forward_declines_across_non_prose_block() {
        let mut e = Editor::new_from_text("Para one.\n\n# Heading\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        assert_eq!(merge_paragraph_forward(&mut e, &TestClock(0)), CommandResult::Noop);
        assert!(e.status.contains("non-paragraph block"), "status: {}", e.status);
        assert_eq!(e.active().document.buffer.to_string(), "Para one.\n\n# Heading\n");
    }

    #[test]
    fn merge_paragraph_forward_declines_non_prose() {
        let mut e = Editor::new_from_text("# Heading\n\nPara one.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(3);
        assert_eq!(merge_paragraph_forward(&mut e, &TestClock(0)), CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "# Heading\n\nPara one.\n");
    }

    /// Single-undo granularity: one `merge_paragraph_forward` produces one undo step.
    #[test]
    fn merge_paragraph_forward_is_one_undo_step() {
        let mut e = Editor::new_from_text("Para one.\n\nPara two.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        assert_eq!(merge_paragraph_forward(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "Para one. Para two.\n");
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "Para one.\n\nPara two.\n");
    }

    #[test]
    fn split_sentence_at_caret_inserts_terminator_and_capitalizes() {
        let mut e = Editor::new_from_text("the cat sat on the mat\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        let at = "the cat sat on the mat\n".find(" on").unwrap(); // caret before " on" (a space)
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(at);
        assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "the cat sat. On the mat\n");
        // F8: the SECOND sentence is selected, caret head-at-start on the capitalized 'O' (NOT the
        // retained leading space — Codex finding 3).
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "On the mat");
        assert_eq!(p.head, p.from());
        assert_eq!(e.active().document.buffer.slice(p.head..p.head + 1), "O", "caret on the capital, not the space");
    }

    #[test]
    fn split_rejects_gap_and_edge() {
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        let gap = "One two. Three four.\n".find(" Three").unwrap(); // in the inter-sentence gap: head > st
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(gap);
        assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "One two. Three four.\n");
    }

    /// A TWO-space separator gives a gap caret that sits STRICTLY inside the whitespace run
    /// (`head > st`, not merely `head == st` as in the single-space edge case above) — pins the
    /// `h < st` guard for the true interior-gap case, not just the boundary.
    #[test]
    fn split_rejects_gap_interior_with_two_space_separator() {
        let mut e = Editor::new_from_text("One two.  Three four.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        let st = "One two.".len(); // 8: preceding sentence's content end
        let gap = st + 1; // strictly inside the two-space gap: st < gap < next sentence start
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(gap);
        assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "One two.  Three four.\n");
    }

    /// The interior guard is `sf < head < st` (STRICT both sides), not `head == sf || head == st`
    /// (Codex finding). A caret sitting exactly on either sentence boundary must decline, not split.
    /// The caret sits MID-WORD (not adjacent to whitespace), so `next_is_ws` is false and the
    /// `". "` branch (terminator + inserted space) runs — every other split test puts the caret
    /// right before an existing space, which only exercises the `"."`-only branch.
    #[test]
    fn split_sentence_at_caret_mid_word_inserts_terminator_and_space() {
        let text = "the cat sat on the mat\n";
        let mut e = Editor::new_from_text(text, None, (40, 12));
        crate::derive::rebuild(&mut e);
        let at = text.find("on").unwrap() + 1; // between 'o' and 'n' of "on" — mid-word, no adjacent ws
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(at);
        assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "the cat sat o. N the mat\n");
        // F8: the new second sentence ("N the mat") is selected, head-at-start on the capitalized 'N'.
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "N the mat");
        assert_eq!(p.head, p.from());
        assert_eq!(e.active().document.buffer.slice(p.head..p.head + 1), "N", "caret on the capital");
    }

    #[test]
    fn split_sentence_at_caret_rejects_at_sentence_edges() {
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        // head == sf (the sentence's own start) — nothing precedes it within the sentence.
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "One two. Three four.\n");
        // head == st (immediately after the terminal period) — attaches to the same sentence
        // (gap-attach-to-preceding), so this is also a boundary, not an interior caret.
        let st = "One two.".len();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(st);
        assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "One two. Three four.\n");
    }

    /// An already-capitalized next word (a proper noun) is never re-cased — only the terminator is
    /// inserted.
    #[test]
    fn split_sentence_at_caret_preserves_already_capitalized_word() {
        let text = "the cat sat and Rex ran\n";
        let mut e = Editor::new_from_text(text, None, (40, 12));
        crate::derive::rebuild(&mut e);
        let at = text.find(" Rex").unwrap(); // caret before the space preceding "Rex"
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(at);
        assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Handled);
        let expected = format!("{}.{}", &text[..at], &text[at..]);
        assert_eq!(e.active().document.buffer.to_string(), expected, "terminator only, no re-casing");
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "Rex ran");
        assert_eq!(p.head, p.from());
        assert_eq!(e.active().document.buffer.slice(p.head..p.head + 1), "R");
    }

    #[test]
    fn split_sentence_at_caret_declines_non_prose() {
        let mut e = Editor::new_from_text("# Heading\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(3);
        assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "# Heading\n");
    }

    /// Single-undo granularity: one `split_sentence_at_caret` produces one undo step.
    #[test]
    fn split_sentence_at_caret_is_one_undo_step() {
        let mut e = Editor::new_from_text("the cat sat on the mat\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        let at = "the cat sat on the mat\n".find(" on").unwrap();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(at);
        assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "the cat sat. On the mat\n");
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "the cat sat on the mat\n");
    }
}
