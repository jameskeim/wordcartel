//! Quick-fix overlay state (Task 6 / Effort 5f).
//!
//! `DiagOverlay` holds the anchor diagnostic and the user's current selection
//! within it.  Rows are: suggestions… then "ignore once", "add to dictionary".

use wordcartel_core::diagnostics::{Diagnostic, Suggestion};
use crate::editor::BufferId;

#[derive(Debug)]
pub struct DiagOverlay {
    pub anchor: Diagnostic,
    pub selected: usize,
    pub buffer_id: BufferId,
}

impl DiagOverlay {
    pub fn new(anchor: Diagnostic, buffer_id: BufferId) -> Self {
        DiagOverlay { anchor, selected: 0, buffer_id }
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
