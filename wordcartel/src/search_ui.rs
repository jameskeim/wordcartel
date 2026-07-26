//! Search-and-replace + quick-fix (diagnostics) overlay actions. Extracted verbatim
//! from app.rs (Effort H1).
//!
//! H24: `editor.apply(...)` below drops the returned `EditOutcome` on purpose. All target the
//! ACTIVE buffer, so `BufferGone` cannot occur. `search_replace_all`/`search_step_apply`/
//! `search_step_rest` explicitly entry-guard `read_only` first (A17 T8), so by the time they
//! reach `apply` a `RejectedReadOnly` can only race a concurrent read-only toggle — the funnel's
//! own loud Sticky Warning still covers that. `diag_apply_selected` has no entry guard, but a
//! `RejectedReadOnly` there sets no unconditional success status afterward (only view-state
//! cleanup), so no false ack is possible either way.

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
    if editor.active().read_only { editor.reject_read_only(); return; } // A17 T8: no mutation, no false "Replaced N".
    search_sync(editor); // ensure cache is current
    // §8: invalid regex → distinct status, no mutation.
    if editor.search.as_ref().is_some_and(|s| s.error.is_some()) {
        editor.set_status_full(crate::status::StatusKind::Error, "invalid regex",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
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
        editor.set_status(crate::status::StatusKind::Info, "No matches");
        return;
    };
    let n = edits.len();
    let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
    // remap origin through this changeset BEFORE moving it into the transaction
    let new_origin = wordcartel_core::change::map_pos(origin, &cs);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(new_origin));
    let _ = editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock); // H24: see module doc
    if let Some(s) = editor.search.as_mut() { s.origin = new_origin; }
    editor.set_status(crate::status::StatusKind::Info, format!("Replaced {n} occurrences"));
    editor.search = None; // close after replace-all
}

pub(crate) fn search_step_apply(editor: &mut Editor, clock: &dyn wordcartel_core::history::Clock) {
    if editor.active().read_only { editor.reject_read_only(); return; } // A17 T8: no mutation, no false step ack.
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
    let _ = editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock); // H24: see module doc
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
    if editor.active().read_only { editor.reject_read_only(); return; } // A17 T8: no mutation, no false ack.
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
    let _ = editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock); // H24: see module doc
    editor.search = None;
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

/// E11 §3.4: deliver a fix terminal — (token, buffer)-keyed CONSUMPTION, version-gated
/// DISPLAY; delivery consumes the token, so a re-delivery is silence. Any
/// terminal for a token nobody holds is silence (displaced/expired/closed requests). TWO
/// call sites: `app::reduce_dispatch`'s arm and `prompts::intercept`'s modal-delivery arm
/// (the DiagProviderEvent "second delivery site" precedent) — one body, no drift.
pub(crate) fn apply_diag_fixes_ready(editor: &mut Editor, buffer_id: crate::editor::BufferId,
    token: u64, version: u64,
    suggestions: Vec<wordcartel_core::diagnostics::Suggestion>) {
    // The buffer must match too: two freshly-opened buffers both sit at version 0, so the
    // version check alone cannot discriminate them. Unreachable today (nothing switches
    // buffers with the overlay up), but it goes live the moment anything does.
    let ours = editor.diag.as_ref()
        .is_some_and(|ov| ov.fix_token == Some(token) && ov.buffer_id == buffer_id);
    if !ours { return; }
    let same_version = editor.diag.as_ref()
        .map(|ov| ov.opened_version == version && editor.active().document.version == version)
        .unwrap_or(false);
    if same_version {
        if let Some(ov) = editor.diag.as_mut() {
            // Consume the token: the request this overlay was waiting on is now satisfied, so
            // a second terminal for it is silence. The state machine already promises
            // exactly-once; this makes the delivery body self-protecting rather than
            // dependent on that promise holding forever.
            ov.fix_token = None;
            ov.apply_fix_delivery(suggestions); // §5.2 selection policy — identity, not clamp
        }
    } else {
        editor.diag = None;
        editor.set_status_full(crate::status::StatusKind::Warning,
            "document changed; re-open", crate::status::StatusLifetime::Sticky,
            crate::status::StatusSource::Host, None);
    }
}

/// Activate the overlay's currently selected row (E11 §5.1 — keyed on the `DiagRow` VALUE,
/// never on an index comparison). Clears `editor.diag` when an activatable row runs,
/// regardless of outcome; at the same document version the non-activatable rows leave it open.
///
/// Ordering (E11 §5.2 note): the stale-anchor guard runs FIRST and closes the overlay with the
/// sticky "document changed; re-open" warning for EVERY row — inertness governs EXECUTION (an
/// inert row never edits, which the exhaustive match below enforces), not whether a dead overlay
/// is allowed to answer a keypress with silence.
pub(crate) fn diag_apply_selected(editor: &mut Editor, clock: &dyn wordcartel_core::history::Clock) {
    use crate::diag_overlay::DiagRow;
    // Clone what we need out of the overlay before mutating editor.
    let overlay_info = editor.diag.as_ref().map(|ov| {
        let row = ov.selected_row();
        let suggestion = ov.chosen_suggestion().cloned();
        (ov.anchor.range.start, ov.anchor.range.end, row, suggestion, ov.opened_version)
    });
    let Some((raw_a, raw_b, row, suggestion, opened_version)) = overlay_info else { return; };

    // Fix A4: if the buffer was mutated while the overlay was open, the anchor
    // ranges are stale.  Refuse to apply — a stale range can cause a panic on
    // multibyte boundaries or silently apply at wrong offsets.
    if editor.active().document.version != opened_version {
        editor.set_status_full(crate::status::StatusKind::Warning, "document changed; re-open",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        editor.diag = None;
        return;
    }

    // Clamp the stale/oversized anchor range to the current doc length so a
    // multibyte/shrink race can never cause buffer.slice or build_range_replace
    // to panic (defense-in-depth even when the command-handler validity gate fires).
    let doc_len = editor.active().document.buffer.len();
    let a = raw_a.min(doc_len);
    let b = raw_b.min(doc_len);

    // Exhaustive on purpose: a new `DiagRow` variant must be PLACED here by the compiler,
    // not silently absorbed into a catch-all.
    match row {
        Some(DiagRow::IgnoreOnce) => diag_ignore_once(editor, a, b),
        Some(DiagRow::AddToDictionary) => diag_add_to_dictionary(editor, a, b),
        // The row carries the index; `suggestion` was resolved through it above.
        Some(DiagRow::Suggestion(_)) => {
            if let Some(s) = suggestion { diag_apply_suggestion(editor, &s, a, b, doc_len, clock); }
        }
        Some(DiagRow::LearnMore) => {} // T8 fills this in (copy the href + the mandatory ack).
        Some(DiagRow::DismissSession) => diag_dismiss_session(editor, a),
        // The inert rows (§5.2) and an out-of-range selection: no edit, overlay stays open.
        None | Some(DiagRow::FetchingFixes) | Some(DiagRow::NoFixes) => {}
    }
}

/// "Ignore once" (Effort A): ephemeral session-ignore. Add the surface word, close, then
/// refilter the store in place (no server round-trip — a full re-check to remove one underline
/// is pure waste under LSP full-doc sync; the old re-arm is dropped, spec §7.3).
fn diag_ignore_once(editor: &mut Editor, a: usize, b: usize) {
    let word = editor.active().document.buffer.slice(a..b).to_string();
    editor.session_ignores.insert(word);
    editor.diag = None;
    crate::diagnostics_run::retain_unignored(editor);
}

/// "Add to dictionary" (Effort A): single writer, no double-write (spec §7.4).
/// `editor.dictionary` is updated FIRST and unconditionally — instant client-side suppression
/// that holds even with no path — then `append_word_to_dict` (the sole file writer) persists to
/// harper-ls's `userDictPath` and `reload_dictionary()` nudges the server to re-read that same
/// file (a config resend, NOT a second write). The None case still suppresses the word; harper
/// falls back to its own path.
fn diag_add_to_dictionary(editor: &mut Editor, a: usize, b: usize) {
    let word = editor.active().document.buffer.slice(a..b).to_string();
    editor.dictionary.insert(word.clone());
    match editor.diag_cfg.dictionary.clone() {
        // fs-chokepoint-allow: (w) config-class read — the personal dictionary, not document content
        Some(dict_path) => match crate::diagnostics_run::append_word_to_dict(&dict_path, &word) {
            Ok(()) => editor.diag_providers.reload_dictionary_enabled(),
            Err(e) => editor.set_status_full(crate::status::StatusKind::Error, format!("add to dictionary failed: {e}"),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None),
        },
        None => editor.set_status_full(crate::status::StatusKind::Warning, "no dictionary path configured",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None),
    }
    editor.diag = None;
    crate::diagnostics_run::retain_unignored(editor);
}

/// "Dismiss for this session" (E11 §5.3) — the non-spelling standing action, and the reason the
/// spelling rows are no longer offered on grammar/style flags. `session_ignores` could not do this
/// job: it matches a surface WORD and suppresses only `Spelling`, so on a grammar flag it was a
/// visible no-op. The dismissal is keyed instead to one OCCURRENCE — the engine, its code, and the
/// pair of parse-free text units (enclosing sentence + enclosing line) at the anchor.
///
/// A blank line has an EMPTY line unit, which would match every other blank line in the document;
/// that key is refused loudly at store time rather than stored (no silent UI). Otherwise: store,
/// close, and refilter in place — no server round-trip, exactly as the ignore/add-dict rows do.
fn diag_dismiss_session(editor: &mut Editor, a: usize) {
    let Some((source, code)) = editor.diag.as_ref()
        .map(|ov| (ov.anchor.source, ov.anchor.code.clone().unwrap_or_default())) else { return; };
    let key = crate::diagnostics_run::dismissal_units_at(&editor.active().document.buffer, a);
    if key.line.is_empty() {
        editor.set_status_full(crate::status::StatusKind::Warning, "cannot dismiss here",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        editor.diag = None;
        return;
    }
    editor.session_dismissals.insert((source, code, key));
    editor.diag = None;
    crate::diagnostics_run::retain_unignored(editor);
}

/// Apply a chosen suggestion as one undoable edit, then close the overlay. `a`/`b` are the
/// anchor range already clamped to `doc_len` by the caller.
fn diag_apply_suggestion(editor: &mut Editor, s: &wordcartel_core::diagnostics::Suggestion,
    a: usize, b: usize, doc_len: usize, clock: &dyn wordcartel_core::history::Clock) {
    let (cs, edit) = match s {
        wordcartel_core::diagnostics::Suggestion::ReplaceWith(t) =>
            crate::commands::build_range_replace(a, b, t, doc_len),
        wordcartel_core::diagnostics::Suggestion::InsertAfter(t) =>
            crate::commands::build_range_replace(b, b, t, doc_len),
        wordcartel_core::diagnostics::Suggestion::Remove =>
            crate::commands::build_range_replace(a, b, "", doc_len),
    };
    // Determine cursor position: for ReplaceWith/InsertAfter place after inserted text;
    // for Remove place at a (start of deleted region).
    let new_cursor = match s {
        wordcartel_core::diagnostics::Suggestion::ReplaceWith(t) => a + t.len(),
        wordcartel_core::diagnostics::Suggestion::InsertAfter(t) => b + t.len(),
        wordcartel_core::diagnostics::Suggestion::Remove => a,
    };
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(new_cursor));
    let _ = editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock); // H24: see module doc
    crate::registry::unfold_ancestors_of(editor, new_cursor);
    crate::edit_apply::resettle(editor); // reflect the unfold on the already-reparsed tree
    editor.diag = None;
}

/// Search overlay intercepts KEY INPUT only; non-key messages (FilterDone/JobDone/
/// TransformDone/ExportDone/Tick) fall through to the normal match arm below so
/// background work is never starved while the overlay is open (mirror of minibuffer
/// block above — see test `search_does_not_starve_filterdone`).
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
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
                    KeyCode::Char('y') => { search_step_apply(editor, ctx.clock); }
                    KeyCode::Char('n') => { search_step_skip(editor); }
                    KeyCode::Char('!') => { search_step_rest(editor, ctx.clock); }
                    KeyCode::Char('q') | KeyCode::Esc => { editor.search = None; }
                    _ => {}
                }
                return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
            }
            match k.code {
                KeyCode::Esc => { search_cancel(editor); return crate::app::Handled::Done(!editor.quit); }
                KeyCode::Char('r') if alt => { editor.search.as_mut().unwrap().toggle_mode(); }
                KeyCode::Char('c') if alt => { editor.search.as_mut().unwrap().cycle_case(); }
                KeyCode::Char('a') if alt => { search_replace_all(editor, ctx.clock); return crate::app::Handled::Done(!editor.quit); }
                KeyCode::Enter if alt => {
                    if let Some(s) = editor.search.as_mut() { s.phase = crate::search_overlay::Phase::Stepping; }
                    search_sync(editor); // park on first match
                    return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
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
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs)); // return ONLY for key events (including non-Press)
    }
    // Non-key messages (FilterDone/ExportDone/TransformDone/JobDone/Tick/…)
    // fall through to the normal handlers below.
    crate::app::Handled::Pass(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Editor, RenderMode};
    use crate::test_support::TestClock;
    use wordcartel_core::diagnostics::{Diagnostic, DiagnosticKind, DiagSource, Suggestion};

    // A17 T8 — the Codex-named FALSE-SUCCESS path: a replace-all on a read-only buffer is rejected
    // with no mutation AND no "Replaced N".
    #[test]
    fn search_replace_all_on_read_only_is_rejected_not_falsely_reported() {
        let mut e = Editor::new_from_text("aaa\n", None, (40, 6));
        e.active_mut().read_only = true; // the entry guard fires before any search work
        let clk = TestClock(0);
        let before = e.active().document.buffer.to_string();
        crate::search_ui::search_replace_all(&mut e, &clk);
        assert_eq!(e.active().document.buffer.to_string(), before, "no mutation on read-only");
        assert_eq!(e.status_text(), "buffer is read-only");
        assert_ne!(e.status_text(), "Replaced 3 occurrences", "must NOT report false success");
    }

    /// Opens a fresh single-suggestion `DiagOverlay` on `e`, selected on the "ignore once"
    /// row (`ignore == true`) or the "add to dictionary" row (`ignore == false`) — neither
    /// row edits the document, so `document.version` never moves.
    fn open_diag_selected(e: &mut Editor, ignore: bool) {
        let v = e.active().document.version;
        let id = e.active().id;
        let d = Diagnostic { range: 0..3, kind: DiagnosticKind::Spelling,
            source: DiagSource::Harper, code: None, href: None, message: "x".into(),
            suggestions: vec![Suggestion::ReplaceWith("the".into())] };
        let mut ov = crate::diag_overlay::DiagOverlay::new(d, id, v);
        ov.selected = if ignore { ov.anchor.suggestions.len() } else { ov.anchor.suggestions.len() + 1 };
        e.diag = Some(ov);
    }

    /// Seed the active store with a spelling diagnostic on "teh" (0..3) so an ignore/add-dict row
    /// has something to refilter in place.
    fn seed_teh_diag(e: &mut Editor) {
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).diagnostics = vec![Diagnostic {
            range: 0..3, kind: DiagnosticKind::Spelling, source: DiagSource::Harper, code: None,
            href: None, message: "x".into(),
            suggestions: vec![Suggestion::ReplaceWith("the".into())] }];
    }

    /// Effort A: "ignore once" adds the surface word to `session_ignores`, closes the overlay, and
    /// refilters the store in place — no re-arm (a full re-check to remove one underline is waste
    /// under full-doc sync, spec §7.3).
    #[test]
    fn diag_apply_selected_ignore_suppresses_in_place_without_rearm() {
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::Review;
        seed_teh_diag(&mut e);
        open_diag_selected(&mut e, true);
        let v_before = e.active().document.version;
        e.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).recheck_due_at = None;
        diag_apply_selected(&mut e, &TestClock(1_000));
        assert_eq!(e.active().document.version, v_before, "ignore does not edit the document");
        assert!(e.diag.is_none(), "overlay closes regardless of outcome");
        assert!(e.session_ignores.contains("teh"), "surface word added to session ignores");
        assert!(e.active().diagnostics.slot(DiagSource::Harper).is_none_or(|s| s.diagnostics.is_empty()), "the ignored underline is refiltered out");
        assert_eq!(e.active().diagnostics.slot(wordcartel_core::diagnostics::DiagSource::Harper).and_then(|s| s.recheck_due_at), None, "no re-arm — the refilter is immediate");
    }

    /// A17 T5 (F4 Warning table): a stale overlay anchor (buffer mutated while the overlay was
    /// open — Fix A4) refuses to apply with a Sticky Warning, not an ordinary Info echo.
    #[test]
    fn diag_apply_selected_stale_version_is_a_sticky_warning() {
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::Review;
        seed_teh_diag(&mut e);
        open_diag_selected(&mut e, true);
        e.active_mut().document.version += 1; // buffer mutated while the overlay was open
        diag_apply_selected(&mut e, &TestClock(1_000));
        assert_eq!(e.status_text(), "document changed; re-open");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        assert!(e.diag.is_none(), "overlay closes on the stale-version refusal");
    }

    /// Effort A: "add to dictionary" with no configured path still suppresses the word client-side
    /// (into `editor.dictionary`), sets the no-path status, closes, and refilters in place — the
    /// None branch is no longer a status-only no-op (round-1 IMPORTANT 5). No re-arm.
    #[test]
    fn diag_apply_selected_add_dict_no_path_still_suppresses() {
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        e.diag_cfg.enabled = true;
        e.diag_cfg.dictionary = None;
        e.active_mut().view.mode = RenderMode::Review;
        seed_teh_diag(&mut e);
        open_diag_selected(&mut e, false);
        e.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).recheck_due_at = None;
        diag_apply_selected(&mut e, &TestClock(3_000));
        assert!(e.diag.is_none(), "overlay closes regardless of outcome");
        assert!(e.dictionary.contains("teh"), "word suppressed client-side even with no path");
        assert_eq!(e.status_text(), "no dictionary path configured");
        // A17 T5 (F4 Warning table): a Sticky Warning.
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        assert!(e.active().diagnostics.slot(DiagSource::Harper).is_none_or(|s| s.diagnostics.is_empty()), "the added word is refiltered out");
        assert_eq!(e.active().diagnostics.slot(wordcartel_core::diagnostics::DiagSource::Harper).and_then(|s| s.recheck_due_at), None, "no re-arm");
    }

    /// Effort A single-writer (spec §7.4): add-to-dict writes the word to the file EXACTLY once
    /// (our `append_word_to_dict` is the sole writer) and nudges the provider to reload — never a
    /// second write. Asserts the on-disk file has one line and the provider saw a ReloadDictionary.
    #[test]
    fn diag_apply_selected_add_dict_writes_file_once_and_nudges_reload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dict_path = dir.path().join("dictionary.txt");
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        e.diag_cfg.enabled = true;
        e.diag_cfg.dictionary = Some(dict_path.clone());
        e.active_mut().view.mode = RenderMode::Review;
        let rec = crate::diag_provider::RecordingProvider::new()
            .with_source(wordcartel_core::diagnostics::DiagSource::Harper);
        let calls = rec.calls_handle();
        e.diag_providers.install(Box::new(rec), true);
        seed_teh_diag(&mut e);
        open_diag_selected(&mut e, false);
        diag_apply_selected(&mut e, &TestClock(5_000));
        let contents = std::fs::read_to_string(&dict_path).expect("dict file written");
        assert_eq!(contents.lines().filter(|l| l.trim() == "teh").count(), 1,
            "the word is written to the file exactly once — single writer, no double write");
        let log = calls.lock().unwrap();
        assert_eq!(log.iter().filter(|c|
            matches!(c, crate::diag_provider::ProviderCall::ReloadDictionary)).count(), 1,
            "the provider is nudged to re-read exactly once (a config resend, not a write)");
        assert!(!log.iter().any(|c|
            matches!(c, crate::diag_provider::ProviderCall::NotifyChange { .. })),
            "no full re-check is dispatched — the client filter hides the word immediately");
        assert!(e.dictionary.contains("teh"), "word also suppressed client-side");
    }

    /// Build a suggestion-free Grammar diagnostic — the shape whose row 0 IS a fetch-state
    /// row (`FetchingFixes` while fetching, `NoFixes` once done).
    fn grammar_no_sugg() -> Diagnostic {
        Diagnostic { range: 0..1, kind: DiagnosticKind::Grammar, source: DiagSource::LTeX,
            code: Some("R".into()), href: None, message: "m".into(), suggestions: vec![] }
    }

    /// The boundary this test and its companion pin (E11 §5.2 ordering note): inertness governs EXECUTION,
    /// the stale-anchor guard governs ENTRY. THIS half is the stale side — the overlay is dead
    /// (the document moved under it), so Enter on `fetching fixes…` / `(no fixes available)` gets
    /// the same shipped Fix-A4 sticky warning every activatable row gets, rather than answering a
    /// keypress with total silence. It must still perform no EDIT: inert is inert.
    ///
    /// FAIL-VERIFY (mutation): reinstate the hoisted
    /// `if matches!(row, None | Some(FetchingFixes) | Some(NoFixes)) { return; }` above the
    /// version check — this test fails (overlay stays open, status empty) while its same-version
    /// companion below stays green. Confirmed, then reverted.
    #[test]
    fn enter_on_an_inert_row_in_a_stale_overlay_closes_with_the_shipped_warning() {
        for fetch_state in [crate::diag_overlay::FixState::Fetching,
                            crate::diag_overlay::FixState::Done] {
            let mut e = Editor::new_from_text("ab\n", None, (40, 10));
            e.open_diag(grammar_no_sugg());
            e.diag.as_mut().unwrap().fix_state = fetch_state;
            e.diag.as_mut().unwrap().selected = 0; // FetchingFixes or NoFixes, per state
            // Move the document under the open overlay through the real edit funnel, so the
            // staleness is the one a writer actually produces (a version bump by hand would
            // pin the guard against a state the funnel can never reach).
            let id = e.active().id;
            let opened_at = e.diag.as_ref().unwrap().opened_version;
            let (cs, edit) = crate::commands::build_multi_replace(
                &[(0, 0, "X".into())], e.active().document.buffer.len());
            assert_eq!(crate::edit_apply::apply_edit(&mut e, id,
                wordcartel_core::history::Transaction::new(cs), edit,
                wordcartel_core::history::EditKind::Other, &TestClock(0)),
                crate::edit_apply::EditOutcome::Applied, "precondition: the funnel edit committed");
            let v = e.active().document.version;
            assert_ne!(v, opened_at, "precondition: the overlay's anchor is now stale");
            crate::search_ui::diag_apply_selected(&mut e, &TestClock(0));
            assert_eq!(e.active().document.version, v, "the Enter itself performed no edit");
            assert!(e.diag.is_none(), "a dead overlay closes on Enter, whichever row is selected");
            assert_eq!(e.status_text(), "document changed; re-open");
            assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
            assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        }
    }

    /// The other half of that boundary: at the SAME version the overlay is alive, and there the
    /// inert rows really are inert — no edit, no dismissal, no status. Kept beside the stale case
    /// so the pair pins the ordering itself rather than one side of it (a guard that closed on
    /// every Enter, or one that never closed, would fail exactly one of the two).
    #[test]
    fn enter_on_an_inert_row_same_version_does_nothing_and_stays_open() {
        for fetch_state in [crate::diag_overlay::FixState::Fetching,
                            crate::diag_overlay::FixState::Done] {
            let mut e = Editor::new_from_text("ab\n", None, (40, 10));
            e.open_diag(grammar_no_sugg());
            e.diag.as_mut().unwrap().fix_state = fetch_state;
            e.diag.as_mut().unwrap().selected = 0;
            let v = e.active().document.version;
            crate::search_ui::diag_apply_selected(&mut e, &TestClock(0));
            assert_eq!(e.active().document.version, v, "no edit from a no-op row");
            assert!(e.diag.is_some(), "the overlay stays open — the row is inert, not a dismissal");
            assert_ne!(e.status_text(), "document changed; re-open",
                "and nothing was refused — the anchor is live");
        }
    }

    /// E11 §3.4 + T5-review carry-forward 1: the delivery guard is (token, buffer, version).
    /// Two freshly-opened buffers both sit at version 0, so the version check alone cannot
    /// discriminate them — only `buffer_id` can. Unreachable through `reduce` today (no
    /// overlay-surviving buffer switch exists), so the guard is exercised directly.
    #[test]
    fn a_terminal_for_another_buffer_is_dropped_even_at_the_same_version() {
        let mut e = Editor::new_from_text("ab\n", None, (40, 10));
        e.open_diag(grammar_no_sugg());
        let ov = e.diag.as_mut().unwrap();
        ov.fix_token = Some(7);
        ov.fix_state = crate::diag_overlay::FixState::Fetching;
        let other = crate::editor::BufferId(e.active().id.0 + 1);
        let version = e.active().document.version;
        apply_diag_fixes_ready(&mut e, other, 7, version,
            vec![Suggestion::ReplaceWith("x".into())]);
        let ov = e.diag.as_ref().expect("overlay untouched");
        assert!(ov.anchor.suggestions.is_empty(), "another buffer's terminal delivered nothing");
        assert_eq!(ov.fix_state, crate::diag_overlay::FixState::Fetching, "still waiting");
    }

    /// T5-review carry-forward 2: delivery CONSUMES the token. The state machine promises one
    /// terminal per token, but the delivery body must be self-protecting — a second terminal
    /// for a consumed token would otherwise wipe the delivered suggestions (and re-aim Enter).
    #[test]
    fn delivery_consumes_the_token_so_a_second_terminal_cannot_wipe_it() {
        let mut e = Editor::new_from_text("ab\n", None, (40, 10));
        e.open_diag(grammar_no_sugg());
        let ov = e.diag.as_mut().unwrap();
        ov.fix_token = Some(7);
        ov.fix_state = crate::diag_overlay::FixState::Fetching;
        let (bid, version) = (e.active().id, e.active().document.version);
        apply_diag_fixes_ready(&mut e, bid, 7, version, vec![Suggestion::ReplaceWith("x".into())]);
        assert_eq!(e.diag.as_ref().unwrap().fix_token, None, "the token is consumed on delivery");
        apply_diag_fixes_ready(&mut e, bid, 7, version, vec![]); // a second terminal, same token
        assert_eq!(e.diag.as_ref().unwrap().anchor.suggestions.len(), 1,
            "the re-delivery was dropped; the delivered suggestions survive");
    }

    /// The `prompts::intercept` DiagFixesReady arm is defense-in-depth: no production route
    /// raises a modal prompt while the quick-fix overlay is open. That XOR is a real invariant
    /// of `open_prompt`, not an assumption — pin it, so a future prompt-raising path that
    /// leaves the overlay up shows as a red test rather than as a mis-aimed delivery.
    #[test]
    fn open_prompt_closes_the_diag_overlay() {
        let mut e = Editor::new_from_text("ab\n", None, (40, 10));
        e.open_diag(grammar_no_sugg());
        assert!(e.diag.is_some(), "precondition: the overlay is up");
        e.open_prompt(crate::prompt::Prompt::quit_confirm());
        assert!(e.diag.is_none(), "raising a modal prompt closes the quick-fix overlay");
    }

    /// A17 T4: an invalid-regex replace-all refusal must land Sticky/Error — surviving a
    /// later Info ack (Q1), not clearing on the next keystroke.
    #[test]
    fn search_replace_all_invalid_regex_is_a_sticky_error_that_survives_a_later_info() {
        use wordcartel_core::search::QueryMode;
        let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
        let id = e.active().id;
        let mut s = crate::search_overlay::SearchState::open(crate::search_overlay::Phase::Replace, 0, id);
        s.mode = QueryMode::Regex;
        s.needle = "(".into(); // unbalanced open paren — invalid regex
        e.search = Some(s);
        let clk = crate::test_support::TestClock(0);
        search_replace_all(&mut e, &clk);
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        e.set_status(crate::status::StatusKind::Info, "later ack");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error, "Q1: Info must not displace a held Error");
    }

    /// A17 T4: an add-to-dictionary write failure (dict path's parent is a regular FILE, so
    /// `append_word_to_dict`'s `create_dir_all` fails) must land Sticky/Error (Q1).
    #[test]
    fn diag_apply_selected_add_dict_failure_is_a_sticky_error() {
        let parent = std::env::temp_dir().join(format!("wc-adddict-fail-{}.md", std::process::id()));
        std::fs::write(&parent, "i am a file, not a dir\n").unwrap();
        let dict_path = parent.join("dictionary.txt"); // parent "inside" a regular file
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        e.diag_cfg.enabled = true;
        e.diag_cfg.dictionary = Some(dict_path);
        e.active_mut().view.mode = RenderMode::Review;
        seed_teh_diag(&mut e);
        open_diag_selected(&mut e, false);
        diag_apply_selected(&mut e, &TestClock(1_000));
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        e.set_status(crate::status::StatusKind::Info, "later ack");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error, "Q1: Info must not displace a held Error");
        let _ = std::fs::remove_file(&parent);
    }

    /// E11 §5.3 (T7): the empty-key belt. A blank line has an EMPTY line-unit, which would key a
    /// dismissal that matches every other blank line in the document — refuse at store time and
    /// say so, rather than storing a key that silences unrelated flags.
    #[test]
    fn dismiss_on_an_empty_line_is_refused_at_store_time() {
        let mut e = Editor::new_from_text("a\n\nb\n", None, (40, 10));
        let d = wordcartel_core::diagnostics::Diagnostic { range: 2..2,
            kind: wordcartel_core::diagnostics::DiagnosticKind::Grammar,
            source: wordcartel_core::diagnostics::DiagSource::LTeX,
            code: Some("R".into()), href: None, message: "m".into(), suggestions: vec![] };
        e.open_diag(d);
        // Select the DismissSession row (Grammar, no href, Done ⇒ rows = [NoFixes, Dismiss]).
        e.diag.as_mut().unwrap().fix_state = crate::diag_overlay::FixState::Done;
        e.diag.as_mut().unwrap().selected = 1;
        crate::search_ui::diag_apply_selected(&mut e, &crate::test_support::TestClock::new(0));
        assert!(e.session_dismissals.is_empty(), "empty line-unit ⇒ refused (belt)");
        assert_eq!(e.status_text(), "cannot dismiss here");
        // Review M2: the refusal deliberately CLOSES the overlay (settled) — pin it, so the
        // behaviour is asserted rather than assumed by the reader of the arm.
        assert!(e.diag.is_none(), "the refusal closes the overlay, it does not leave it open");
    }

    /// E11 §5.3, review FIX 1 — the STORE side of the `None → ""` keying `DismissSet` documents.
    /// A code-less engine still gets per-occurrence dismissal: the anchor's absent `code` must key
    /// as the empty string, so the stored triple re-matches other code-LESS flags on that pair and
    /// leaves a coded flag on the very same sentence+line alone.
    #[test]
    fn an_anchor_without_a_code_stores_the_empty_string_key() {
        let mut e = Editor::new_from_text(
            "Alpha beta gamma. Delta epsilon zeta.\n", None, (80, 24));
        let mk = |range: std::ops::Range<usize>, code: Option<&str>| Diagnostic { range,
            kind: DiagnosticKind::Grammar, source: DiagSource::LTeX,
            code: code.map(Into::into), href: None, message: "m".into(), suggestions: vec![] };
        // Both flags sit inside "Delta epsilon zeta." ⇒ ONE pair key; only `code` separates them.
        e.active_mut().diagnostics.slot_mut(DiagSource::LTeX).diagnostics =
            vec![mk(18..23, None), mk(24..31, Some("R"))];
        e.open_diag(mk(18..23, None));
        e.diag.as_mut().unwrap().fix_state = crate::diag_overlay::FixState::Done;
        e.diag.as_mut().unwrap().selected = 1; // rows = [NoFixes, DismissSession]
        diag_apply_selected(&mut e, &TestClock(0));
        let key = crate::diagnostics_run::dismissal_units_at(&e.active().document.buffer, 18);
        assert!(e.session_dismissals.contains(&(DiagSource::LTeX, String::new(), key)),
            "an absent anchor code keys as the EMPTY STRING, not as a wildcard");
        let kept: Vec<std::ops::Range<usize>> =
            e.active().diagnostics.slot(DiagSource::LTeX).unwrap()
                .diagnostics.iter().map(|d| d.range.clone()).collect();
        assert_eq!(kept, vec![24..31],
            "the code-less flag went; the coded flag on the same pair key stayed");
    }

    /// The companion the refusal test cannot supply: the `DismissSession` arm's SUCCESS path.
    /// Without it an arm that only ever refused would still pass the belt test above. Pins the
    /// stored triple (source, code, pair key), the close, and the in-place refilter.
    #[test]
    fn dismiss_stores_the_pair_key_closes_and_refilters_in_place() {
        let mut e = Editor::new_from_text(
            "Alpha beta gamma. Delta epsilon zeta.\n", None, (80, 24));
        let d = Diagnostic { range: 18..23, kind: DiagnosticKind::Grammar, source: DiagSource::LTeX,
            code: Some("R".into()), href: None, message: "m".into(), suggestions: vec![] };
        e.active_mut().diagnostics.slot_mut(DiagSource::LTeX).diagnostics = vec![d.clone()];
        assert_eq!(e.active().diagnostics.slot(DiagSource::LTeX).unwrap().diagnostics.len(), 1,
            "precondition: the flag is in the store before the dismissal");
        e.open_diag(d);
        e.diag.as_mut().unwrap().fix_state = crate::diag_overlay::FixState::Done;
        e.diag.as_mut().unwrap().selected = 1; // rows = [NoFixes, DismissSession]
        diag_apply_selected(&mut e, &TestClock(0));
        let key = crate::diagnostics_run::dismissal_units_at(&e.active().document.buffer, 18);
        assert_eq!(key.sentence, "Delta epsilon zeta.");
        assert!(e.session_dismissals.contains(&(DiagSource::LTeX, "R".to_string(), key)),
            "the (source, code, pair-key) triple is stored");
        assert!(e.diag.is_none(), "the overlay closes on a dismissal");
        assert!(e.active().diagnostics.slot(DiagSource::LTeX).unwrap().diagnostics.is_empty(),
            "the underline disappears immediately — retain_unignored ran");
    }
}
