//! S4 prose-surgery commands — a leaf module on the A14 template (no `Command` variant, no
//! `commands::run` arm; `registry.rs` calls these directly). Edits flow through `editor.apply`
//! (`ChangeSet`) as one undo unit. SEE==SELECT + decline route through `super::prose_sentence_at`.
//!
//! H24: every `editor.apply(...)` below drops the returned `EditOutcome` on purpose — see the
//! identical rationale in `commands/edit.rs`'s module doc (active-buffer only, so `BufferGone`
//! cannot occur; `RejectedReadOnly` already fired the loud Sticky Warning inside the funnel and
//! Q1 arbitration keeps any later success status from showing over it). `swap`'s discard mirrors
//! `blocks_marked::block_move`'s: it still consumes `marked_block` and re-settles regardless.

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
    editor.set_status(crate::status::StatusKind::Info, format!("{} words · {} sentences · {} chars", st.words, st.sentences, st.chars));
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
    // Window via the SAME content-byte anchoring the lens/select path uses (SEE==SELECT, I-1):
    // a raw-`h` `paragraph_range_at` drifts into the gap fallback on ≤3-space indented prose and
    // returns a DIFFERENT window that swallows the indent. `None` declines (non-prose line).
    let Some((ps, pe)) = super::prose_window_at(editor, h) else {
        editor.set_status(crate::status::StatusKind::Info, "no sentence here");
        return CommandResult::Noop;
    };
    let win = editor.active().document.buffer.slice(ps..pe);
    let rel = h.saturating_sub(ps).min(win.len());
    // Window-relative content spans.
    let spans: Vec<(usize, usize)> = wordcartel_core::textobj::sentence_spans(&win).collect();
    if spans.is_empty() { editor.set_status(crate::status::StatusKind::Info, "no sentence here"); return CommandResult::Noop; }
    // Index of the caret's sentence (attach: caret in the gap → the PRECEDING span, i.e. the last
    // span whose start <= rel; before the first content → span 0).
    let cur = spans.iter().rposition(|&(s, _)| s <= rel).unwrap_or(0);
    let (a_idx, b_idx) = match dir {
        Dir::Down if cur + 1 < spans.len() => (cur, cur + 1),
        Dir::Up   if cur >= 1              => (cur - 1, cur),
        _ => {
            editor.set_status(crate::status::StatusKind::Info, "sentence at paragraph edge — break or merge to cross");
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
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    editor.set_status(crate::status::StatusKind::Info, match dir { Dir::Up => "moved sentence up", Dir::Down => "moved sentence down" });
    CommandResult::Handled
}

/// `swap` — exchange the primary `Selection` region with the `MarkedBlock` region (F2). ONE undo
/// unit via `build_multi_replace`. Overlap rejects LOUDLY (never reach the builder's silent
/// identity-no-op, spec C-2). Gap fate M2: region bytes move verbatim; outside whitespace untouched.
/// Post-op: selection holds the moved selection-content head-at-start (F8/C-9); marked_block consumed.
pub(crate) fn swap(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let sel = editor.active().document.selection.primary();
    if sel.is_empty() {
        editor.set_status(crate::status::StatusKind::Info, "swap needs a selection and a marked block");
        return CommandResult::Noop;
    }
    let Some(mb) = editor.active().marked_block else {
        editor.set_status(crate::status::StatusKind::Info, "swap needs a selection and a marked block");
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
        editor.set_status(crate::status::StatusKind::Info, "can't swap overlapping regions");
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
    // T8 (C-7/C-12/C-13) — fold survival. Compute the corrected fold set from the PRE-edit folds +
    // `cs` BEFORE `Transaction::new(cs)` moves `cs`. Both region-contents relocate: R1's content →
    // the R2 slot (shifted by the len delta), R2's content → the R1 slot.
    let regions = [
        (r1_from, r1_to, r2_from + l2 - l1), // R1's content → R2 slot (shifted by len delta)
        (r2_from, r2_to, r1_from),           // R2's content → R1 slot
    ];
    let corrected = if !editor.active().folds.is_empty() {
        Some(crate::fold::corrected_after_move(&editor.active().folds, &regions, &cs))
    } else { None };
    let had_correction = corrected.is_some();
    let txn = Transaction::new(cs).with_selection(Selection::range(moved_to, moved_from)); // moves cs
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // core: mutate + rebuild #1 + ensure_visible (H24: see module doc)
    editor.active_mut().marked_block = None;
    if let Some(c) = corrected {
        editor.active_mut().folds.replace_folded(c); // override the core's plain remap with the corrected set
    }
    let r = super::edit::settle_after_edit(editor);  // REBUILD #2 — relayout + reconcile the corrected folds
    if had_correction {
        // Symmetry with block_move: after the corrected folds settle, snap the head out of any fold
        // so a folded-region swap can never leave the caret on a hidden line.
        crate::registry::snap_caret_out_of_fold(editor);
    }
    editor.set_status(crate::status::StatusKind::Info, "swapped");
    r
}

/// `break_paragraph_here` — the caret's sentence (and all after it in the paragraph) becomes a new
/// paragraph. Gap fate M3: consume the single separator before the sentence, insert "\n\n". Decline
/// on non-prose; Noop if already at a paragraph start. F8: the promoted sentence is selected.
pub(crate) fn break_paragraph_here(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let (sf, st) = match super::prose_sentence_at(editor, h) {
        Ok(s) => s, Err(_) => { editor.set_status(crate::status::StatusKind::Info, "no sentence here"); return CommandResult::Noop; }
    };
    // The SAME content-anchored window `prose_sentence_at` segmented within (I-1) — never a raw-`h`
    // `paragraph_range_at`, whose ≤3-space gap fallback would put `ps` BEFORE the indent so a caret
    // on the paragraph's first sentence reads `sf > ps` and wrongly splits (replacing the indent).
    let Some((ps, _pe)) = super::prose_window_at(editor, h) else {
        editor.set_status(crate::status::StatusKind::Info, "no sentence here"); return CommandResult::Noop;
    };
    if sf <= ps { editor.set_status(crate::status::StatusKind::Info, "already at a paragraph start"); return CommandResult::Noop; }
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
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    editor.set_status(crate::status::StatusKind::Info, "split paragraph");
    CommandResult::Handled
}

/// `merge_paragraph_forward` — join the caret's paragraph with the next. Gap fate M4: replace the
/// paragraph separator with ONE space. Decline on non-prose; Noop if no next paragraph or the next
/// block is non-prose. F8: the absorbed paragraph's first sentence is selected.
pub(crate) fn merge_paragraph_forward(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    // The content-anchored window (I-1): raw-`h` `paragraph_range_at` drifts on ≤3-space indented
    // prose. `None` declines (non-prose line).
    let Some((ps, pe)) = super::prose_window_at(editor, h) else {
        editor.set_status(crate::status::StatusKind::Info, "no paragraph here"); return CommandResult::Noop;
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
        editor.set_status(crate::status::StatusKind::Info, "no paragraph to merge"); return CommandResult::Noop;
    }
    if !next_is_prose {
        editor.set_status(crate::status::StatusKind::Info, "can't merge across a non-paragraph block"); return CommandResult::Noop;
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
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    editor.set_status(crate::status::StatusKind::Info, "merged paragraph");
    CommandResult::Handled
}

/// `split_sentence_at_caret` — turn one sentence into two at the caret. Gap fate M5: insert ". "
/// (or "." if the next char is whitespace — no double space) and uppercase the next word's initial.
/// Interior guard (finding 3): `sf < head < st` (a gap caret has head > st and is rejected).
pub(crate) fn split_sentence_at_caret(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    let h = nav::head(editor);
    let (sf, st) = match super::prose_sentence_at(editor, h) {
        Ok(s) => s, Err(_) => { editor.set_status(crate::status::StatusKind::Info, "no sentence here"); return CommandResult::Noop; }
    };
    if !(sf < h && h < st) {
        editor.set_status(crate::status::StatusKind::Info, "place the caret inside a sentence to split");
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
    let _ = editor.apply(txn, edit, EditKind::Other, clock); // H24: see module doc
    editor.set_status(crate::status::StatusKind::Info, "split sentence");
    CommandResult::Handled
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
        assert!(e.status_text().contains("2 sentences"), "buffer: {}", e.status_text());
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 8);
        count_region(&mut e);
        assert!(e.status_text().contains("1 sentences") && e.status_text().contains("2 words"), "sel: {}", e.status_text());
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
        assert!(e.status_text().contains("edge"), "edge status: {}", e.status_text());
        assert_eq!(e.active().document.buffer.to_string(), "First one. Second two.\n"); // unchanged
    }

    #[test]
    fn move_sentence_declines_non_prose() {
        let mut e = Editor::new_from_text("# Heading\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(3);
        assert_eq!(move_sentence_down(&mut e, &TestClock(0)), CommandResult::Noop);
    }

    /// B14 decline pins. The TERMINATORS INSIDE THE CELLS are load-bearing: pre-B14 the
    /// table classifies as prose and `sentence_spans` segments on the periods, so every
    /// mutation below ACTS (returns Handled and mutates the table). A terminator-free
    /// table collapses to ONE span and `move_sentence` Noops coincidentally at its
    /// paragraph-edge arm — an unchanged-buffer assert would be green for the WRONG
    /// reason (the vacuous-pin hazard). Each pin therefore asserts the decline path's
    /// DISTINGUISHING signals: Noop + the decline status text; byte-identity is support.
    const TABLE_DOC: &str =
        "Intro prose here.\n\n| First. | Second. |\n|---|---|\n| Third. | Fourth. |\n\nOutro prose follows.\n";

    /// Pre-B14 red: Handled — swaps the cell "sentences" inside the table (caret in the
    /// first span, further spans follow). Post-B14: prose_window_at → None → decline.
    #[test]
    fn move_sentence_declines_on_table() {
        let mut e = Editor::new_from_text(TABLE_DOC, None, (80, 12));
        crate::derive::rebuild(&mut e);
        let before = e.active().document.buffer.slice(0..e.active().document.buffer.len());
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(
            TABLE_DOC.find("First").unwrap());
        assert_eq!(move_sentence_down(&mut e, &TestClock(0)), CommandResult::Noop,
            "decline, not a sentence swap inside the table");
        assert_eq!(e.status_text(), "no sentence here", "the decline status (distinguishing signal)");
        let after = e.active().document.buffer.slice(0..e.active().document.buffer.len());
        assert_eq!(before, after, "buffer byte-identical (supporting assert)");
    }

    /// Pre-B14 red: Handled + status "split paragraph" — the caret's cell "sentence" is not
    /// at the window start (`sf > ps`), so a paragraph break is spliced INTO the table.
    /// Post-B14: prose_sentence_at → Err → decline.
    #[test]
    fn break_paragraph_here_declines_on_table() {
        let mut e = Editor::new_from_text(TABLE_DOC, None, (80, 12));
        crate::derive::rebuild(&mut e);
        let before = e.active().document.buffer.slice(0..e.active().document.buffer.len());
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(
            TABLE_DOC.find("Third").unwrap());
        assert_eq!(break_paragraph_here(&mut e, &TestClock(0)), CommandResult::Noop,
            "decline, not a paragraph break spliced into the table");
        assert_eq!(e.status_text(), "no sentence here", "the decline status (distinguishing signal)");
        let after = e.active().document.buffer.slice(0..e.active().document.buffer.len());
        assert_eq!(before, after, "buffer byte-identical (supporting assert)");
    }

    /// Pre-B14 red: Handled — the table (as "prose") merges with the FOLLOWING real prose
    /// paragraph ("Outro prose follows."), which is exactly the wrong edit. Post-B14:
    /// prose_window_at → None → decline with merge's own message.
    #[test]
    fn merge_paragraph_forward_declines_on_table() {
        let mut e = Editor::new_from_text(TABLE_DOC, None, (80, 12));
        crate::derive::rebuild(&mut e);
        let before = e.active().document.buffer.slice(0..e.active().document.buffer.len());
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(
            TABLE_DOC.find("Third").unwrap());
        assert_eq!(merge_paragraph_forward(&mut e, &TestClock(0)), CommandResult::Noop,
            "decline, not a table-with-prose merge");
        assert_eq!(e.status_text(), "no paragraph here", "the decline status (distinguishing signal)");
        let after = e.active().document.buffer.slice(0..e.active().document.buffer.len());
        assert_eq!(before, after, "buffer byte-identical (supporting assert)");
    }

    /// Pre-B14 red: Handled + status "split sentence" — the caret sits strictly inside the
    /// first cell "sentence" (`sf < h < st`), so a ". " terminator is inserted into the
    /// cell and the following char capitalized. Post-B14: prose_sentence_at → Err → decline.
    #[test]
    fn split_sentence_at_caret_declines_on_table() {
        let mut e = Editor::new_from_text(TABLE_DOC, None, (80, 12));
        crate::derive::rebuild(&mut e);
        let before = e.active().document.buffer.slice(0..e.active().document.buffer.len());
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(
            TABLE_DOC.find("irst").unwrap()); // strictly inside "First." — sf < h < st
        assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Noop,
            "decline, not a terminator spliced into the cell");
        assert_eq!(e.status_text(), "no sentence here", "the decline status (distinguishing signal)");
        let after = e.active().document.buffer.slice(0..e.active().document.buffer.len());
        assert_eq!(before, after, "buffer byte-identical (supporting assert)");
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
        assert!(e.status_text().contains("overlap"), "status: {}", e.status_text());
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
        assert!(e.status_text().contains("swap"), "status: {}", e.status_text());
        assert_eq!(e.active().document.buffer.to_string(), "BBBBAAAA\n");
    }

    #[test]
    fn swap_requires_both_regions() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 2);
        // no marked block
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Noop);
        assert!(e.status_text().contains("marked block"));
    }

    #[test]
    fn swap_requires_a_selection() {
        let mut e = Editor::new_from_text("abcdef\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0); // empty selection
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 3, end: 6, hidden: false });
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Noop);
        assert!(e.status_text().contains("marked block"), "status: {}", e.status_text());
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

    /// T8 (C-7/C-12/C-13): a FOLDED section swapped with another stays folded at its NEW byte, and
    /// the fold does NOT stick on the section now sitting at the vacated position. Asserts the
    /// SPECIFIC relocated heading byte — a stale fold at the wrong heading would pass a bare
    /// `len == 1` (plan-gate finding 6).
    #[test]
    fn swap_keeps_a_folded_section_folded_at_its_new_byte() {
        // A=[0,b) B=[b,len). Fold A; select A; mark B; swap → buffer is B_text ++ A_text, so A's heading
        // relocates to `len - (b - 0)` = len - b (l1 = b, A lands at r1_from + l2 = 0 + (len - b)).
        let doc = "## A\n\nbody a.\n\n## B\n\nbody b.\n";
        let mut e = Editor::new_from_text(doc, None, (60, 20));
        crate::derive::rebuild(&mut e);
        let a = doc.find("## A").unwrap(); // 0
        let b = doc.find("## B").unwrap();
        let len = doc.len();
        e.active_mut().folds.toggle(a);
        let (a_from, a_to) = crate::commands::section_range_at(&e, a + 1).unwrap();
        let (b_from, b_to) = crate::commands::section_range_at(&e, b + 1).unwrap();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(a_from, a_to);
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: b_from, end: b_to, hidden: false });
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Handled);
        let a_new = len - b; // A's heading destination (byte-length-preserving swap)
        let folded = e.active().folds.folded();
        assert!(folded.contains(&a_new), "A's heading is folded at its NEW byte {a_new}: {folded:?}");
        assert!(!folded.contains(&0), "the fold did NOT stay on B's heading (now at 0)");
        assert_eq!(folded.len(), 1, "exactly one fold — no double, no drop");
    }

    /// T8 (C-7/C-12): swapping TWO folded sections yields TWO distinct folds at the correct new
    /// bytes — the wholesale `replace_folded` (one set from the pre-edit folds) cannot self-clobber.
    /// Here A's stale-collapse byte (0, its original) EQUALS B's destination byte (0) — a per-region
    /// remove/toggle loop would flip that shared byte and either drop a fold or fold the wrong heading.
    #[test]
    fn swap_two_folded_sections_yields_two_distinct_folds_no_self_clobber() {
        let doc = "## A\n\nbody a.\n\n## B\n\nbody b.\n";
        let mut e = Editor::new_from_text(doc, None, (60, 20));
        crate::derive::rebuild(&mut e);
        let a = doc.find("## A").unwrap(); // 0
        let b = doc.find("## B").unwrap();
        let len = doc.len();
        e.active_mut().folds.toggle(a); // fold BOTH sections
        e.active_mut().folds.toggle(b);
        let (a_from, a_to) = crate::commands::section_range_at(&e, a + 1).unwrap();
        let (b_from, b_to) = crate::commands::section_range_at(&e, b + 1).unwrap();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(a_from, a_to);
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: b_from, end: b_to, hidden: false });
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Handled);
        let a_new = len - b; // A relocates to len - b
        let folded = e.active().folds.folded();
        assert!(folded.contains(&a_new), "A stays folded at its NEW byte {a_new}: {folded:?}");
        assert!(folded.contains(&0), "B stays folded at its NEW byte 0 (B's content moved to the R1 slot): {folded:?}");
        assert_eq!(folded.len(), 2, "exactly two distinct folds — no self-clobber, no double, no drop");
    }

    /// H22 Task 5 regression tripwire: a folded section swapped with the marked block AS the R1 slot
    /// (marked_block = A, selection = B — the `r1_is_sel = false` path, the mirror of
    /// `swap_keeps_a_folded_section_folded_at_its_new_byte`'s `r1_is_sel = true`) keeps its fold at the
    /// destination byte through the core-backed `editor.apply`, and the caret is never left on a
    /// hidden line. Must stay green before AND after the Surface C migration (behavior-preserving,
    /// §3.6) — grounded on the `corrected_after_move` fixtures in `fold.rs:536-564`.
    #[test]
    fn swap_preserves_a_folded_region_through_the_core() {
        let doc = "## A\n\nbody a.\n\n## B\n\nbody b.\n";
        let mut e = Editor::new_from_text(doc, None, (60, 20));
        crate::derive::rebuild(&mut e);
        let a = doc.find("## A").unwrap(); // 0
        let b = doc.find("## B").unwrap();
        let len = doc.len();
        e.active_mut().folds.toggle(a); // fold section A
        let (a_from, a_to) = crate::commands::section_range_at(&e, a + 1).unwrap();
        let (b_from, b_to) = crate::commands::section_range_at(&e, b + 1).unwrap();
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: a_from, end: a_to, hidden: false });
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(b_from, b_to);
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Handled);
        let a_new = len - b; // A relocates to the R2 slot (same geometry as the mirrored r1_is_sel=true pin)
        let folded = e.active().folds.folded();
        assert!(folded.contains(&a_new), "A's heading is folded at its NEW byte {a_new}: {folded:?}");
        assert!(!folded.contains(&0), "the fold did NOT stay on B's heading (now at 0)");
        assert_eq!(folded.len(), 1, "exactly one fold — no double, no drop");
        let fold_view = e.active_fold_view();
        let head = e.active().document.selection.primary().head;
        let caret_line = e.active().document.buffer.byte_to_line(head);
        assert!(!fold_view.is_hidden(caret_line),
            "caret line {caret_line} must be visible after swapping a folded section");
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
        assert!(e.status_text().contains("paragraph start"), "status: {}", e.status_text());
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
        assert!(e.status_text().contains("no paragraph to merge"), "status: {}", e.status_text());
        assert_eq!(e.active().document.buffer.to_string(), "Para one.\n");
    }

    #[test]
    fn merge_paragraph_forward_declines_across_non_prose_block() {
        let mut e = Editor::new_from_text("Para one.\n\n# Heading\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        assert_eq!(merge_paragraph_forward(&mut e, &TestClock(0)), CommandResult::Noop);
        assert!(e.status_text().contains("non-paragraph block"), "status: {}", e.status_text());
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

    // ------------------------------------------------------------------
    // Final-gate FIX 1 (I-1): SEE==SELECT window drift on ≤3-space indented prose.
    // ------------------------------------------------------------------

    /// I-1 probe: `break_paragraph_here` on a 2-space-indented paragraph with the caret in the
    /// indent must Noop (already at the paragraph's first sentence, as the lens/select path sees
    /// it) — NOT replace the indent with a paragraph break. The window MUST be content-anchored;
    /// a raw-`h` `paragraph_range_at` drifts into the gap fallback (ps before the indent) so
    /// `sf > ps` and the handler wrongly splits.
    #[test]
    fn break_paragraph_here_indented_first_sentence_is_noop() {
        let mut e = Editor::new_from_text("  One two. Three four.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1); // in the indent
        assert_eq!(break_paragraph_here(&mut e, &TestClock(0)), CommandResult::Noop);
        assert!(e.status_text().contains("paragraph start"), "status: {}", e.status_text());
        assert_eq!(e.active().document.buffer.to_string(), "  One two. Three four.\n",
            "buffer unchanged — the indent is NOT replaced with a paragraph break");
    }

    /// I-1 probe: `move_sentence_down` on a 2-space-indented paragraph must operate on the
    /// content-window sentence ("One two."), preserving the leading indent at line start — NOT
    /// absorb the indent into the moved sentence (the raw-`h` gap-fallback window produced a triple
    /// space: "Three four.   One two.").
    #[test]
    fn move_sentence_down_indented_preserves_leading_indent() {
        let mut e = Editor::new_from_text("  One two. Three four.\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0); // line start (indent)
        assert_eq!(move_sentence_down(&mut e, &TestClock(0)), CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "  Three four. One two.\n",
            "indent preserved at line start, sentence content moved — no triple space");
    }

    /// FIX 2 symmetry: `swap` of a FOLDED section must leave the caret on a VISIBLE line (snapped
    /// out of any fold the correction re-applied) — typing must never land on hidden text.
    #[test]
    fn swap_of_folded_section_leaves_caret_on_a_visible_line() {
        let doc = "## A\n\nbody a.\n\n## B\n\nbody b.\n";
        let mut e = Editor::new_from_text(doc, None, (60, 20));
        crate::derive::rebuild(&mut e);
        let a = doc.find("## A").unwrap();
        let b = doc.find("## B").unwrap();
        e.active_mut().folds.toggle(a); // fold section A
        let (a_from, a_to) = crate::commands::section_range_at(&e, a + 1).unwrap();
        let (b_from, b_to) = crate::commands::section_range_at(&e, b + 1).unwrap();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(a_from, a_to);
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: b_from, end: b_to, hidden: false });
        assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Handled);
        let fold_view = e.active_fold_view();
        let head = e.active().document.selection.primary().head;
        let caret_line = e.active().document.buffer.byte_to_line(head);
        assert!(!fold_view.is_hidden(caret_line),
            "caret line {caret_line} must be visible after swapping a folded section");
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
