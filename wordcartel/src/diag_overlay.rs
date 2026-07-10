//! Quick-fix overlay state (Task 6 / Effort 5f).
//!
//! `DiagOverlay` holds the anchor diagnostic and the user's current selection
//! within it.  Rows are: suggestions… then "ignore once", "add to dictionary".

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
}

impl DiagOverlay {
    pub fn new(anchor: Diagnostic, buffer_id: BufferId, opened_version: u64) -> Self {
        DiagOverlay { anchor, selected: 0, scroll_top: 0, buffer_id, opened_version }
    }

    /// Total row count: one per suggestion, plus "ignore once" + "add to dictionary".
    pub fn row_count(&self) -> usize {
        self.anchor.suggestions.len() + 2
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

    /// True when the selected row is "ignore once".
    pub fn is_ignore(&self) -> bool {
        self.selected == self.anchor.suggestions.len()
    }

    /// True when the selected row is "add to dictionary".
    pub fn is_add_dict(&self) -> bool {
        self.selected == self.anchor.suggestions.len() + 1
    }

    /// The chosen `Suggestion`, or `None` when a non-suggestion row is selected.
    pub fn chosen_suggestion(&self) -> Option<&Suggestion> {
        self.anchor.suggestions.get(self.selected)
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
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled {
    if editor.diag.is_none() { return crate::app::Handled::Pass(msg); }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            match k.code {
                crossterm::event::KeyCode::Up   => { editor.diag.as_mut().unwrap().up(); }
                crossterm::event::KeyCode::Down => { editor.diag.as_mut().unwrap().down(); }
                crossterm::event::KeyCode::Esc  => { editor.diag = None; }
                crossterm::event::KeyCode::Enter => { crate::search_ui::diag_apply_selected(editor, clock); }
                _ => {} // bare Ctrl+key or anything else: no-op, consumed
            }
        }
        for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
        return crate::app::Handled::Done(!editor.quit); // return ONLY for key events (including non-Press)
    }
    // Non-key messages fall through to normal handlers below.
    crate::app::Handled::Pass(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 28 suggestions + ignore + add-dict = 30 rows.
    fn tall_diag() -> DiagOverlay {
        let suggestions = (0..28).map(|i|
            wordcartel_core::diagnostics::Suggestion::ReplaceWith(format!("s{i}"))).collect();
        let d = wordcartel_core::diagnostics::Diagnostic {
            range: 0..1,
            kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
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
