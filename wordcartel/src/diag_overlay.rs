//! Quick-fix overlay state (Task 6 / Effort 5f).
//!
//! `DiagOverlay` holds the anchor diagnostic and the user's current selection
//! within it.  The rows are not fixed: they are COMPUTED from the anchor's kind, its
//! `href`, and the fetch state — see [`DiagOverlay::rows`] (E11 §5.1). Everything that
//! needs to know what row `n` is (paint, mouse, apply) reads that one list; nothing
//! re-derives it with index arithmetic.

use wordcartel_core::diagnostics::{Diagnostic, Suggestion};
use crate::editor::BufferId;
use crate::app::Msg;
use crossterm::event::Event;

#[derive(Debug)]
pub struct DiagOverlay {
    pub anchor: Diagnostic,
    pub selected: usize,
    /// Window offset — the absolute index of the first visible list row.
    /// Maintained by `keep_overlay_visible` in the paint/mouse layers;
    /// `up`/`down` move `selected` only (matching the other list overlays).
    pub scroll_top: usize,
    pub buffer_id: BufferId,
    /// Document version at the time the overlay was opened.  Used to refuse
    /// to apply a quick-fix if the buffer was mutated while the overlay was
    /// open (Fix A4: stale-range panic / wrong-offset apply guard).
    pub opened_version: u64,
    /// E11 §3.2: the correlation token of this overlay's on-demand fix request — `Some` iff a
    /// terminal is still owed (the request was ACCEPTED and has not yet been delivered;
    /// delivery consumes it back to `None`). Token equality is what discriminates a reopen,
    /// which reproduces every other identity field; the delivery guard pairs it with
    /// `buffer_id` so two buffers at the same version cannot be confused (§3.4).
    pub fix_token: Option<u64>,
    /// E11 §5.2: whether a fix fetch is still outstanding. Never a silent wait — the state is a
    /// visible row.
    pub fix_state: FixState,
}

/// E11 §5.2: the overlay's fix-fetch state. `Fetching` iff the open-time `request_fixes`
/// returned `Accepted::Yes`; `Done` at delivery/expiry, or immediately at open on `Accepted::No`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FixState { Fetching, Done }

/// E11 §5.1: one row of the quick-fix list — the single source of truth for paint, mouse,
/// and apply. Conditional rows (kind-aware standing actions, an `href`-only "learn more",
/// the two fetch-state rows) made index arithmetic impossible to state correctly, and the
/// identity-preserving selection policy of §5.2 needs row VALUES to compare.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DiagRow {
    /// A provider-supplied fix, by index into `anchor.suggestions`.
    Suggestion(usize),
    /// Placeholder shown while a fix fetch is still outstanding. Never activatable.
    FetchingFixes,
    /// Placeholder shown when the fetch is over and produced nothing. Never activatable.
    NoFixes,
    /// Copy the diagnostic's documentation link. Present iff `anchor.href.is_some()`.
    LearnMore,
    /// Session-ignore this surface word. Spelling only.
    IgnoreOnce,
    /// Add this surface word to the personal dictionary. Spelling only.
    AddToDictionary,
    /// Dismiss this flag for the session. Non-Spelling only.
    DismissSession,
}

impl DiagOverlay {
    pub fn new(anchor: Diagnostic, buffer_id: BufferId, opened_version: u64) -> Self {
        DiagOverlay { anchor, selected: 0, scroll_top: 0, buffer_id, opened_version,
            fix_token: None, fix_state: FixState::Done }
    }

    /// The overlay's rows, in display order — a pure function of the anchor and the fetch
    /// state (E11 §5.1). The suggestion block comes first; when it is empty a single
    /// fetch-state row stands in for it, so the list is never empty and the writer is never
    /// left staring at a silent wait. `LearnMore` is present iff the provider supplied a link,
    /// and the standing actions are kind-aware (spelling gets word-level actions; everything
    /// else gets the session dismiss).
    ///
    /// # Examples
    /// ```ignore
    /// // A spelling flag with one fix, fetch complete:
    /// // [Suggestion(0), IgnoreOnce, AddToDictionary]
    /// ```
    pub fn rows(&self) -> Vec<DiagRow> {
        let mut out: Vec<DiagRow> = (0..self.anchor.suggestions.len())
            .map(DiagRow::Suggestion).collect();
        if out.is_empty() {
            out.push(match self.fix_state {
                FixState::Fetching => DiagRow::FetchingFixes,
                FixState::Done => DiagRow::NoFixes,
            });
        }
        if self.anchor.href.is_some() { out.push(DiagRow::LearnMore); }
        match self.anchor.kind {
            wordcartel_core::diagnostics::DiagnosticKind::Spelling => {
                out.push(DiagRow::IgnoreOnce);
                out.push(DiagRow::AddToDictionary);
            }
            wordcartel_core::diagnostics::DiagnosticKind::Grammar => {
                out.push(DiagRow::DismissSession);
            }
        }
        out
    }

    /// Total row count — `rows().len()`, so the windowing/mouse layers cannot drift from
    /// what the painter actually draws.
    pub fn row_count(&self) -> usize { self.rows().len() }

    /// The row Enter would activate, or `None` if `selected` is somehow out of range.
    pub fn selected_row(&self) -> Option<DiagRow> { self.rows().get(self.selected).cloned() }

    /// Install fetched suggestions, applying the E11 §5.2 selection POLICY.
    ///
    /// An asynchronous delivery must never silently re-aim Enter. Selection is therefore
    /// preserved by ROW IDENTITY — the `DiagRow` value the writer had selected is re-located
    /// in the new list, so a writer parked on `IgnoreOnce` stays on `IgnoreOnce` when
    /// suggestion rows appear above (or disappear from above) it. When the selected row
    /// VANISHED — only `FetchingFixes` can, replaced by its own results — the selection is a
    /// deliberate reset to the first row of the new list, not a side effect of clamping.
    pub fn apply_fix_delivery(&mut self, suggestions: Vec<Suggestion>) {
        let prev = self.rows().get(self.selected).cloned();
        self.anchor.suggestions = suggestions;
        self.fix_state = FixState::Done;
        let rows = self.rows();
        self.selected = match prev {
            Some(DiagRow::FetchingFixes) | None => 0, // deliberate reset (Suggestion(0)/NoFixes)
            Some(row) => rows.iter().position(|r| *r == row).unwrap_or(0),
        };
    }

    pub fn up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn down(&mut self) {
        if self.selected + 1 < self.row_count() {
            self.selected += 1;
        }
    }

    /// The chosen `Suggestion`, or `None` when a non-suggestion row is selected.
    pub fn chosen_suggestion(&self) -> Option<&Suggestion> {
        match self.selected_row() {
            Some(DiagRow::Suggestion(i)) => self.anchor.suggestions.get(i),
            _ => None,
        }
    }
}

/// Human-readable label for a suggestion row.
pub fn suggestion_label(s: &Suggestion) -> String {
    match s {
        Suggestion::ReplaceWith(t) => t.clone(),
        Suggestion::InsertAfter(t) => format!("+ \"{}\"", t),
        Suggestion::Remove => "(delete)".to_string(),
    }
}

/// Diag overlay intercepts KEY INPUT only; non-key messages fall through to
/// normal handling so background work is never starved while the overlay is open
/// (mirror of minibuffer/search blocks above — 5e starvation lesson).
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
    if editor.diag.is_none() { return crate::app::Handled::Pass(msg); }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            match k.code {
                crossterm::event::KeyCode::Up   => { editor.diag.as_mut().unwrap().up(); }
                crossterm::event::KeyCode::Down => { editor.diag.as_mut().unwrap().down(); }
                crossterm::event::KeyCode::Esc  => { editor.diag = None; }
                crossterm::event::KeyCode::Enter => { crate::search_ui::diag_apply_selected(editor, ctx.clock); }
                _ => {} // bare Ctrl+key or anything else: no-op, consumed
            }
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs)); // return ONLY for key events (including non-Press)
    }
    // Non-key messages fall through to normal handlers below.
    crate::app::Handled::Pass(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_core::diagnostics::DiagnosticKind;

    /// A minimal overlay over one diagnostic of `kind`, with `n_sugg` synthetic suggestions
    /// and an optional `href` — the two conditionals `rows()` keys on besides `fix_state`.
    fn diag(kind: DiagnosticKind, href: Option<&str>, n_sugg: usize) -> DiagOverlay {
        let suggestions = (0..n_sugg).map(|i| Suggestion::ReplaceWith(format!("s{i}"))).collect();
        let d = Diagnostic { range: 0..1, kind,
            source: wordcartel_core::diagnostics::DiagSource::LTeX,
            code: Some("C".into()), href: href.map(str::to_string),
            message: "m".into(), suggestions };
        DiagOverlay::new(d, crate::editor::BufferId(1), 0)
    }

    #[test]
    fn rows_are_kind_aware_and_href_conditional() {
        let mut sp = diag(DiagnosticKind::Spelling, None, 1);
        sp.fix_state = FixState::Done;
        assert_eq!(sp.rows(), vec![DiagRow::Suggestion(0), DiagRow::IgnoreOnce,
            DiagRow::AddToDictionary], "Spelling keeps the two standing rows; no dismiss");
        let mut gr = diag(DiagnosticKind::Grammar, Some("https://x"), 0);
        gr.fix_state = FixState::Done;
        assert_eq!(gr.rows(), vec![DiagRow::NoFixes, DiagRow::LearnMore, DiagRow::DismissSession],
            "Grammar: dismiss instead of the spelling rows; LearnMore iff href");
    }

    #[test]
    fn fetching_row_shows_while_fetching_and_empty_done_shows_nofixes() {
        let mut g = diag(DiagnosticKind::Grammar, None, 0);
        g.fix_state = FixState::Fetching;
        assert_eq!(g.rows()[0], DiagRow::FetchingFixes);
        g.fix_state = FixState::Done;
        assert_eq!(g.rows()[0], DiagRow::NoFixes);
    }

    #[test]
    fn delivery_preserves_user_selection_by_row_identity() {
        // Round-3 Important-4: a writer parked on IgnoreOnce stays there when rows appear.
        let mut sp = diag(DiagnosticKind::Spelling, None, 0);
        sp.fix_state = FixState::Fetching; // rows: [FetchingFixes, IgnoreOnce, AddToDictionary]
        sp.selected = 1; // IgnoreOnce
        let before = sp.rows()[sp.selected].clone();
        sp.apply_fix_delivery(vec![Suggestion::ReplaceWith("a".into()),
            Suggestion::ReplaceWith("b".into())]);
        assert_eq!(sp.rows()[sp.selected], before, "selection followed the ROW, not the index");
    }

    #[test]
    fn delivery_on_vanished_fetching_row_resets_deterministically() {
        let mut sp = diag(DiagnosticKind::Spelling, None, 0);
        sp.fix_state = FixState::Fetching;
        sp.selected = 0; // FetchingFixes
        sp.apply_fix_delivery(vec![Suggestion::ReplaceWith("a".into())]);
        assert_eq!(sp.rows()[sp.selected], DiagRow::Suggestion(0), "deliberate reset, not clamp");
        let mut g = diag(DiagnosticKind::Grammar, None, 0);
        g.fix_state = FixState::Fetching;
        g.selected = 0;
        g.apply_fix_delivery(vec![]);
        assert_eq!(g.rows()[g.selected], DiagRow::NoFixes, "empty delivery lands on NoFixes");
    }

    /// Identity re-location works DOWNWARD too — the rule is row-VALUE relocation, not "pin
    /// the tail rows to the end of the list". A shrinking delivery (three suggestions
    /// collapsing to one) moves `IgnoreOnce` up from index 3 to index 1; a `min(len-1)` clamp
    /// would leave the writer on index 2 — `AddToDictionary`, a DIFFERENT standing action than
    /// the one they parked on.
    #[test]
    fn delivery_preserves_identity_when_the_row_list_shrinks() {
        let mut sp = diag(DiagnosticKind::Spelling, None, 3);
        sp.fix_state = FixState::Done; // rows: [S0, S1, S2, IgnoreOnce, AddToDictionary]
        sp.selected = 3; // IgnoreOnce
        sp.apply_fix_delivery(vec![Suggestion::ReplaceWith("only".into())]);
        assert_eq!(sp.rows(), vec![DiagRow::Suggestion(0), DiagRow::IgnoreOnce,
            DiagRow::AddToDictionary], "precondition: the list shrank from 5 rows to 3");
        assert_eq!(sp.selected, 1, "IgnoreOnce moved up; the selection moved with it");
    }

    /// 28 suggestions + ignore + add-dict = 30 rows.
    fn tall_diag() -> DiagOverlay {
        let suggestions = (0..28).map(|i|
            wordcartel_core::diagnostics::Suggestion::ReplaceWith(format!("s{i}"))).collect();
        let d = wordcartel_core::diagnostics::Diagnostic {
            range: 0..1,
            kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "m".into(),
            suggestions,
        };
        DiagOverlay::new(d, crate::editor::BufferId(1), 0)
    }

    #[test]
    fn diag_window_follows_selection() {
        // `down()` takes NO arg (diag_overlay.rs:33); windowing is applied by the
        // mouse/paint layer via keep_overlay_visible (the two-layer list_window
        // invariant) — drive both.
        let mut d = tall_diag();
        assert_eq!(d.row_count(), 30);
        for _ in 0..20 {
            d.down();
            crate::app::keep_overlay_visible(24, d.selected, d.row_count(), &mut d.scroll_top);
        }
        let lh = crate::list_window::list_h_for(d.row_count(), 24);
        assert!(d.selected.saturating_sub(d.scroll_top) < lh,
            "selection stays inside the window (selected={}, scroll_top={}, lh={lh})",
            d.selected, d.scroll_top);
    }
}
