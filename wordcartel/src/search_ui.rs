//! Search-and-replace + quick-fix (diagnostics) overlay actions. Extracted verbatim
//! from app.rs (Effort H1).

use crate::{derive, editor::Editor};
use crate::app::Msg;
use crossterm::event::Event;

pub(crate) fn search_sync(editor: &mut Editor) {
    let (rope, version) = { let d = &editor.active().document; (d.buffer.snapshot(), d.version) };
    if let Some(s) = editor.search.as_mut() { s.recompute(&rope, version); }
    search_pin(editor);
}

pub(crate) fn search_step(editor: &mut Editor, forward: bool) {
    if let Some(s) = editor.search.as_mut() { if forward { s.next(); } else { s.prev(); } }
    search_pin(editor);
}

pub(crate) fn search_cancel(editor: &mut Editor) {
    let origin = editor.search.as_ref().map(|s| s.origin).unwrap_or(0);
    editor.search = None;
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(origin);
    derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}

type SearchReplacePlan = Option<(Vec<(usize, usize, String)>, usize, usize)>;

pub(crate) fn search_replace_all(editor: &mut Editor, clock: &dyn wordcartel_core::history::Clock) {
    search_sync(editor); // ensure cache is current
    // §8: invalid regex → distinct status, no mutation.
    if editor.search.as_ref().is_some_and(|s| s.error.is_some()) {
        editor.status = "invalid regex".into();
        return;
    }
    let plan: SearchReplacePlan = editor.search.as_ref().and_then(|s| {
        let m = s.matcher()?;
        if s.matches().is_empty() { return None; }
        let rope = editor.active().document.buffer.snapshot();
        let edits: Vec<(usize, usize, String)> = s.matches().iter().map(|mm| {
            (mm.start, mm.end, wordcartel_core::search::expand_replacement(&rope, m, mm, &s.template, s.mode))
        }).collect();
        Some((edits, rope.len_bytes(), s.origin))
    });
    let Some((edits, doc_len, origin)) = plan else {
        editor.status = "No matches".into();
        return;
    };
    let n = edits.len();
    let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
    // remap origin through this changeset BEFORE moving it into the transaction
    let new_origin = wordcartel_core::change::map_pos(origin, &cs);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(new_origin));
    editor.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
    if let Some(s) = editor.search.as_mut() { s.origin = new_origin; }
    editor.status = format!("Replaced {n} occurrences");
    editor.search = None; // close after replace-all
    derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}

pub(crate) fn search_step_apply(editor: &mut Editor, clock: &dyn wordcartel_core::history::Clock) {
    let plan = editor.search.as_ref().and_then(|s| {
        let m = s.matcher()?; let cur = s.current()?;
        let rope = editor.active().document.buffer.snapshot();
        let text = wordcartel_core::search::expand_replacement(&rope, m, &cur, &s.template, s.mode);
        Some((cur, text, rope.len_bytes(), s.origin))
    });
    let Some((cur, text, doc_len, origin)) = plan else { editor.search = None; return; };
    let (cs, edit) = crate::commands::build_range_replace(cur.start, cur.end, &text, doc_len);
    let new_origin = wordcartel_core::change::map_pos(origin, &cs);
    let caret = cur.start + text.len();
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(caret));
    editor.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
    // Re-find the next match on the MUTATED rope, and remap origin.
    let (rope, version) = { let d = &editor.active().document; (d.buffer.snapshot(), d.version) };
    if let Some(s) = editor.search.as_mut() {
        s.origin = new_origin;
        s.cache_invalidate();                 // force recompute against mutated rope
        s.recompute(&rope, version);
        s.set_current_at_or_after(caret);     // park on next match at/after the just-edited spot
    }
    search_pin(editor);
    if editor.search.as_ref().is_some_and(|s| s.current().is_none()) { editor.search = None; } // done
}

pub(crate) fn search_step_skip(editor: &mut Editor) {
    if let Some(s) = editor.search.as_mut() { s.next(); }
    search_pin(editor);
    if editor.search.as_ref().is_some_and(|s| s.wrapped) { editor.search = None; } // walked off the end
}

pub(crate) fn search_step_rest(editor: &mut Editor, clock: &dyn wordcartel_core::history::Clock) {
    // Replace current + all remaining (from current.start onward) as one unit.
    let plan = editor.search.as_ref().and_then(|s| {
        let m = s.matcher()?; let cur = s.current()?;
        let rope = editor.active().document.buffer.snapshot();
        let edits: Vec<(usize, usize, String)> = s.matches().iter().filter(|mm| mm.start >= cur.start)
            .map(|mm| (mm.start, mm.end, wordcartel_core::search::expand_replacement(&rope, m, mm, &s.template, s.mode)))
            .collect();
        Some((edits, rope.len_bytes()))
    });
    let Some((edits, doc_len)) = plan else { editor.search = None; return; };
    if edits.is_empty() { editor.search = None; return; }
    let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(edits[0].0));
    editor.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
    editor.search = None;
    derive::rebuild(editor); crate::nav::ensure_visible(editor);
}

/// Unfold + select + rebuild + ensure-visible for `editor.search`'s CURRENT match.
/// The shared placement tail (spec §5.2 step 3) — every path that pins the caret
/// on the current match (keyboard step/sync, mouse match-click) goes through this
/// ONE function so painter-visible state (selection, folds, viewport) never drifts
/// between callers. Does NOT recompute the cache — callers that need a fresh cache
/// call `SearchState::recompute` (or `search_sync`, which wraps both) first.
pub(crate) fn search_pin(editor: &mut Editor) {
    if let Some(m) = editor.search.as_ref().and_then(|s| s.current()) {
        crate::registry::unfold_ancestors_of(editor, m.start);
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::range(m.start, m.end);
        derive::rebuild(editor); crate::nav::ensure_visible(editor);
    }
}

/// Accept, ignore, or add-to-dict based on the overlay's current selection.
/// Clears `editor.diag` when done (regardless of outcome).
pub(crate) fn diag_apply_selected(editor: &mut Editor, clock: &dyn wordcartel_core::history::Clock) {
    // Clone what we need out of the overlay before mutating editor.
    let overlay_info = editor.diag.as_ref().map(|ov| {
        let is_ignore = ov.is_ignore();
        let is_add_dict = ov.is_add_dict();
        let suggestion = ov.chosen_suggestion().cloned();
        (ov.anchor.range.start, ov.anchor.range.end, is_ignore, is_add_dict, suggestion, ov.opened_version)
    });
    let Some((raw_a, raw_b, is_ignore, is_add_dict, suggestion, opened_version)) = overlay_info else { return; };

    // Fix A4: if the buffer was mutated while the overlay was open, the anchor
    // ranges are stale.  Refuse to apply — a stale range can cause a panic on
    // multibyte boundaries or silently apply at wrong offsets.
    if editor.active().document.version != opened_version {
        editor.status = "document changed; re-open".into();
        editor.diag = None;
        return;
    }

    // Clamp the stale/oversized anchor range to the current doc length so a
    // multibyte/shrink race can never cause buffer.slice or build_range_replace
    // to panic (defense-in-depth even when the command-handler validity gate fires).
    let doc_len = editor.active().document.buffer.len();
    let a = raw_a.min(doc_len);
    let b = raw_b.min(doc_len);

    if is_ignore {
        // Add the surface word to session_ignores, close, re-arm a recheck.
        let word = editor.active().document.buffer.slice(a..b).to_string();
        editor.session_ignores.insert(word);
        editor.diag = None;
        if crate::diagnostics_run::should_run_diagnostics(editor) {
            let debounce_ms = editor.diag_cfg.debounce_ms;
            editor.active_mut().diagnostics.arm(clock.now_ms(), debounce_ms);
        }
    } else if is_add_dict {
        // Append word to dictionary file + in-memory set, close, re-arm.
        let word = editor.active().document.buffer.slice(a..b).to_string();
        if let Some(ref dict_path) = editor.diag_cfg.dictionary.clone() {
            match crate::diagnostics_run::append_word_to_dict(dict_path, &word) {
                Ok(()) => { editor.dictionary.insert(word); }
                Err(e) => { editor.status = format!("add to dictionary failed: {e}"); }
            }
        } else {
            editor.status = "no dictionary path configured".into();
        }
        editor.diag = None;
        if crate::diagnostics_run::should_run_diagnostics(editor) {
            let debounce_ms = editor.diag_cfg.debounce_ms;
            editor.active_mut().diagnostics.arm(clock.now_ms(), debounce_ms);
        }
    } else if let Some(s) = suggestion {
        // Apply the suggestion as an undoable edit, then close.
        let (cs, edit) = match &s {
            wordcartel_core::diagnostics::Suggestion::ReplaceWith(t) =>
                crate::commands::build_range_replace(a, b, t, doc_len),
            wordcartel_core::diagnostics::Suggestion::InsertAfter(t) =>
                crate::commands::build_range_replace(b, b, t, doc_len),
            wordcartel_core::diagnostics::Suggestion::Remove =>
                crate::commands::build_range_replace(a, b, "", doc_len),
        };
        // Determine cursor position: for ReplaceWith/InsertAfter place after inserted text;
        // for Remove place at a (start of deleted region).
        let new_cursor = match &s {
            wordcartel_core::diagnostics::Suggestion::ReplaceWith(t) => a + t.len(),
            wordcartel_core::diagnostics::Suggestion::InsertAfter(t) => b + t.len(),
            wordcartel_core::diagnostics::Suggestion::Remove => a,
        };
        let txn = wordcartel_core::history::Transaction::new(cs)
            .with_selection(wordcartel_core::selection::Selection::single(new_cursor));
        editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
        derive::rebuild(editor);
        crate::registry::unfold_ancestors_of(editor, new_cursor);
        derive::rebuild(editor);
        crate::nav::ensure_visible(editor);
        editor.diag = None;
    }
    // else: no suggestion and not ignore/add_dict — unreachable (selected is always in range).
}

/// Search overlay intercepts KEY INPUT only; non-key messages (FilterDone/JobDone/
/// TransformDone/ExportDone/Tick) fall through to the normal match arm below so
/// background work is never starved while the overlay is open (mirror of minibuffer
/// block above — see test `search_does_not_starve_filterdone`).
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled {
    if editor.search.is_none() { return crate::app::Handled::Pass(msg); }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            use crossterm::event::{KeyCode, KeyModifiers};
            let alt = k.modifiers.contains(KeyModifiers::ALT);
            let shift = k.modifiers.contains(KeyModifiers::SHIFT);
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            // Stepping phase: y/n/!/q intercepted BEFORE the text-insert arm.
            if editor.search.as_ref().map(|s| s.phase) == Some(crate::search_overlay::Phase::Stepping) {
                match k.code {
                    KeyCode::Char('y') => { search_step_apply(editor, clock); }
                    KeyCode::Char('n') => { search_step_skip(editor); }
                    KeyCode::Char('!') => { search_step_rest(editor, clock); }
                    KeyCode::Char('q') | KeyCode::Esc => { editor.search = None; }
                    _ => {}
                }
                return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
            }
            match k.code {
                KeyCode::Esc => { search_cancel(editor); return crate::app::Handled::Done(!editor.quit); }
                KeyCode::Char('r') if alt => { editor.search.as_mut().unwrap().toggle_mode(); }
                KeyCode::Char('c') if alt => { editor.search.as_mut().unwrap().cycle_case(); }
                KeyCode::Char('a') if alt => { search_replace_all(editor, clock); return crate::app::Handled::Done(!editor.quit); }
                KeyCode::Enter if alt => {
                    if let Some(s) = editor.search.as_mut() { s.phase = crate::search_overlay::Phase::Stepping; }
                    search_sync(editor); // park on first match
                    return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
                }
                KeyCode::Enter if shift => { search_step(editor, false); }
                KeyCode::F(3) if shift   => { search_step(editor, false); }
                KeyCode::Enter           => { search_step(editor, true); }
                KeyCode::F(3)            => { search_step(editor, true); }
                KeyCode::Tab => {
                    if let Some(s) = editor.search.as_mut() {
                        s.field = match s.field {
                            crate::search_overlay::Field::Needle => crate::search_overlay::Field::Template,
                            crate::search_overlay::Field::Template => crate::search_overlay::Field::Needle,
                        };
                        s.cursor = s.focused_field().len();
                    }
                }
                KeyCode::Backspace       => { editor.search.as_mut().unwrap().backspace(); }
                KeyCode::Left            => { editor.search.as_mut().unwrap().left(); }
                KeyCode::Right           => { editor.search.as_mut().unwrap().right(); }
                KeyCode::Char(c) if !ctrl && !alt => { editor.search.as_mut().unwrap().insert(c); }
                _ => {}
            }
            // Recompute against the live buffer and pin the current match.
            search_sync(editor);
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx)); // return ONLY for key events (including non-Press)
    }
    // Non-key messages (FilterDone/ExportDone/TransformDone/JobDone/Tick/…)
    // fall through to the normal handlers below.
    crate::app::Handled::Pass(msg)
}
