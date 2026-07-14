//! S4 prose-surgery commands — a leaf module on the A14 template (no `Command` variant, no
//! `commands::run` arm; `registry.rs` calls these directly). Edits flow through `editor.apply`
//! (`ChangeSet`) as one undo unit. SEE==SELECT + decline route through `super::prose_sentence_at`.

use crate::editor::Editor;
use super::CommandResult;

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

#[cfg(test)]
mod tests {
    use super::*;
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
}
