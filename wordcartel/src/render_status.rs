//! Extracted verbatim from render.rs (Effort H1 round 2).

use crate::editor::Editor;
use wordcartel_core::count;

/// Assemble the left-hand portion of the normal status line (no overlay active).
///
/// Format: `[i/n] <name> [<mode>]` (plus optional status message and BLK indicator).
/// `i` is 1-based active buffer index; `n` is total buffer count.
/// `<name>` comes from `workspace::buffer_display_name` which already handles
/// `*scratch*` / `*untitled*` / filename and the dirty-`*` prefix — so there is
/// no separate dirty marker here.
pub(crate) fn status_left_text(editor: &Editor) -> String {
    let idx = editor.active + 1;
    let count = editor.buffers.len();
    let name = crate::workspace::buffer_display_name(editor, editor.active().id);
    let head = format!("[{idx}/{count}] {name}");
    // Task 6 (SPINE §8.3): the Review label follows the switchable lens, gaining attribution
    // (`REVIEW · <lens>`) only when the LENS engine is *live* (Ready) — Idle/Starting/Unavailable
    // all show plain `REVIEW`, so the label asserts a working checker for whichever engine is
    // actually being shown (spec §10). One mutex read, behind the Review arm only.
    let mode_text: std::borrow::Cow<'static, str> = match editor.active().view.mode {
        crate::editor::RenderMode::LivePreview => "PREVIEW".into(),
        crate::editor::RenderMode::SourceHighlighted => "SRC-HI".into(),
        crate::editor::RenderMode::SourcePlain => "SOURCE".into(),
        crate::editor::RenderMode::Review => {
            let lens = editor.active_analysis_source;
            if editor.diag_providers.availability(lens) == Some(crate::diag_provider::Availability::Ready) {
                format!("REVIEW · {}", lens.label()).into()
            } else { "REVIEW".into() }
        }
    };
    let mut text = if editor.status.is_empty() {
        format!("{head} [{mode_text}]")
    } else {
        format!("{head} [{mode_text}] {}", editor.status)
    };
    // BLK indicator: `· BLK` when a block is marked; `· BLK·hidden` when hidden.
    match editor.active().marked_block {
        Some(b) if b.hidden => text.push_str(" · BLK·hidden"),
        Some(_) => text.push_str(" · BLK"),
        None => {}
    }
    text
}

/// Return a word/char count segment for the status bar, or `None` if the
/// feature is disabled (`view_opts.word_count = false`).
///
/// When the primary selection is non-empty, counts only the selected text;
/// otherwise counts the whole document buffer.
pub(crate) fn word_count_segment(editor: &Editor) -> Option<String> {
    if !editor.view_opts.word_count {
        return None;
    }
    let sel = editor.active().document.selection.primary();
    let text = if !sel.is_empty() {
        editor.active().document.buffer.slice(sel.from()..sel.to())
    } else {
        editor.active().document.buffer.to_string()
    };
    Some(format!(
        "{} words · {} chars",
        count::word_count(&text),
        count::char_count(&text)
    ))
}

// ---------------------------------------------------------------------------
// Search bar formatting
// ---------------------------------------------------------------------------

pub(crate) fn format_search_bar(s: &crate::search_overlay::SearchState) -> String {
    use crate::search_overlay::Phase;
    let mode = if matches!(s.mode, wordcartel_core::search::QueryMode::Regex) { " .*" } else { "" };
    let case = match s.case {
        wordcartel_core::search::CaseMode::Smart => " Aa~",
        wordcartel_core::search::CaseMode::Sensitive => " Aa",
        wordcartel_core::search::CaseMode::Insensitive => " aa",
    };
    let count = if s.error.is_some() {
        " ?".to_string()
    } else if s.count() == 0 {
        " no matches".to_string()
    } else {
        let cap_note = if s.capped() {
            format!(" (first {})", crate::limits::MAX_SEARCH_MATCHES)
        } else {
            String::new()
        };
        format!(" {}/{}{}", s.current_ordinal().unwrap_or(0), s.count(), cap_note)
    };
    let wrapped = if s.wrapped { " (wrapped)" } else { "" };
    match s.phase {
        Phase::Replace | Phase::Stepping =>
            format!("Find: {}  Replace: {}{}{}{}{}", s.needle, s.template, mode, case, count, wrapped),
        Phase::Find =>
            format!("Find: {}{}{}{}{}", s.needle, mode, case, count, wrapped),
    }
}

#[cfg(test)]
mod tests {
    use crate::editor::Editor;

    #[test]
    fn word_count_segment_selection_aware() {
        let mut e = Editor::new_from_text("alpha beta gamma\n", None, (80, 24));
        e.view_opts.word_count = true;
        // whole doc: 3 words, 17 chars (including trailing \n)
        assert_eq!(crate::render_status::word_count_segment(&e), Some("3 words · 17 chars".to_string()));
        // select "alpha" → 1 word, 5 chars
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5);
        assert_eq!(crate::render_status::word_count_segment(&e), Some("1 words · 5 chars".to_string()));
        e.view_opts.word_count = false;
        assert_eq!(crate::render_status::word_count_segment(&e), None);
    }

    #[test]
    fn status_line_shows_buffer_index_and_count() {
        let mut e = crate::editor::Editor::new_from_text("a\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch(); // 2 buffers, active index 0
        let s = crate::render_status::status_left_text(&e);
        assert!(s.contains("[1/2]"), "shows active/count: {s}");
    }

    #[test]
    fn status_line_names_untitled_and_scratch() {
        let mut e = crate::editor::Editor::new_from_text("\n", None, (40, 10));
        e.install_scratch();
        let s_untitled = crate::render_status::status_left_text(&e);
        assert!(s_untitled.contains("*untitled*"), "untitled buffer shows *untitled*: {s_untitled}");
        crate::workspace::goto_scratch(&mut e);
        let s_scratch = crate::render_status::status_left_text(&e);
        assert!(s_scratch.contains("*scratch*"), "scratch buffer shows *scratch*: {s_scratch}");
    }

    #[test]
    fn status_line_shows_review_label() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        // Default empty ProviderSet has no Harper entry (availability() -> None, not Ready) →
        // plain [REVIEW], no attribution.
        assert!(crate::render_status::status_left_text(&e).contains("[REVIEW]"), "review mode labels [REVIEW]");
    }

    /// Effort A §10: a *live* provider attributes the Review label with its source's label; a
    /// non-Ready provider shows plain REVIEW.
    #[test]
    fn status_line_attributes_review_only_when_provider_ready() {
        use crate::diag_provider::{RecordingProvider, Availability};
        use wordcartel_core::diagnostics::DiagSource;
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        e.diag_providers.install(Box::new(RecordingProvider::new()
            .with_source(DiagSource::Harper).with_availability(Availability::Ready)), true);
        // The label comes from DiagSource::Harper.label(), not the provider's own identity.
        assert!(crate::render_status::status_left_text(&e).contains("[REVIEW · Harper]"),
            "Ready → attribution");

        let mut e2 = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        e2.active_mut().view.mode = crate::editor::RenderMode::Review;
        e2.diag_providers.install(Box::new(RecordingProvider::new()
            .with_source(DiagSource::Harper).with_availability(Availability::Starting)), true);
        assert!(crate::render_status::status_left_text(&e2).contains("[REVIEW]"),
            "Starting → plain REVIEW");
        assert!(!crate::render_status::status_left_text(&e2).contains("·"),
            "no attribution dot when not Ready");
    }

    /// Task 6 (SPINE §8.3): the Review label follows the switchable lens, not always Harper.
    #[test]
    fn status_line_review_label_follows_the_lens() {
        use crate::diag_provider::{RecordingProvider, Availability};
        use wordcartel_core::diagnostics::DiagSource;
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        e.diag_providers.install(Box::new(RecordingProvider::new()
            .with_source(DiagSource::Harper).with_availability(Availability::Ready)), true);
        e.diag_providers.install(Box::new(RecordingProvider::new()
            .with_source(DiagSource::Plugin("mock")).with_availability(Availability::Ready)), true);
        assert!(crate::render_status::status_left_text(&e).contains("[REVIEW · Harper]"),
            "default lens = Harper");
        e.set_analysis_source(DiagSource::Plugin("mock"));
        assert!(crate::render_status::status_left_text(&e).contains("[REVIEW · mock]"),
            "lens switched: label follows");
    }
}
