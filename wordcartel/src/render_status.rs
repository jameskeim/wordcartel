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
    // (`REVIEW · <lens>`) when the LENS engine is *live* (Ready); E10 §12 adds a steady
    // `REVIEW · warming <lens>…` while the engine is Starting. Idle/Unavailable show plain
    // `REVIEW`, so the label asserts a working (or warming) checker for whichever engine is
    // actually being shown (spec §10). One mutex read, behind the Review arm only.
    let mode_text: std::borrow::Cow<'static, str> = match editor.active().view.mode {
        crate::editor::RenderMode::LivePreview => "PREVIEW".into(),
        crate::editor::RenderMode::SourceHighlighted => "SRC-HI".into(),
        crate::editor::RenderMode::SourcePlain => "SOURCE".into(),
        crate::editor::RenderMode::Review => {
            let lens = editor.active_analysis_source;
            match editor.diag_providers.availability(lens) {
                // The label asserts a WORKING checker for the shown engine (SPINE §8.3)…
                Some(crate::diag_provider::Availability::Ready) =>
                    format!("REVIEW · {}", lens.label()).into(),
                // …and E10 §12 adds the steady warming state — render-derived, self-clearing
                // on the Starting→Ready flip, incapable of animating (no timer, no loop).
                Some(crate::diag_provider::Availability::Starting) =>
                    format!("REVIEW · warming {}…", lens.label()).into(),
                _ => "REVIEW".into(), // Idle / Unavailable / no entry: plain (unchanged)
            }
        }
    };
    let mut text = if editor.status_text().is_empty() {
        format!("{head} [{mode_text}]")
    } else {
        format!("{head} [{mode_text}] {}", editor.status_text())
    };
    // BLK indicator (④ extends): `· BLK` gains a direction when fully off-screen
    // (`↑`/`↓`, line-granular); `· BLK·hidden` keeps its exact legacy form (no arrow
    // for an unpainted landmark); a pending ^KB shows `· BLK…` independently.
    match editor.active().marked_block {
        Some(b) if b.hidden => text.push_str(" · BLK·hidden"),
        Some(b) => {
            text.push_str(" · BLK");
            text.push_str(crate::block_paint::blk_direction(editor, b));
        }
        None => {}
    }
    if editor.active().pending_block_begin.is_some() {
        text.push_str(" · BLK…");
    }
    // Mark identity (④ sub-fork A): the caret line's mark names, BTreeMap order.
    if let Some(mk) = crate::block_paint::marks_on_caret_line(editor) {
        text.push_str(" · ");
        text.push_str(&mk);
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
    let st = count::region_stats(&text);
    Some(format!("{} words · {} sentences · {} chars", st.words, st.sentences, st.chars))
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
        // whole doc: 3 words, 1 sentence (no terminal punctuation), 17 chars (incl. trailing \n)
        assert_eq!(crate::render_status::word_count_segment(&e),
            Some("3 words · 1 sentences · 17 chars".to_string()));
        // select "alpha" → 1 word, 1 sentence, 5 chars
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5);
        assert_eq!(crate::render_status::word_count_segment(&e),
            Some("1 words · 1 sentences · 5 chars".to_string()));
        e.view_opts.word_count = false;
        assert_eq!(crate::render_status::word_count_segment(&e), None);
    }

    /// S8 Task 6: the prose-lens count segment (`lenses::prose_lens_count_segment`) rides the
    /// status line as its own right-side segment, gated on `computed_for == version` (an active
    /// AND current lens) — independent of `word_count_segment`'s own gate, so it composes into
    /// the right-side status string whether or not `view_opts.word_count` is on.
    #[test]
    fn prose_lens_count_segment_shown_when_active_and_current() {
        let mut e = Editor::new_from_text("The report was written here.\n", None, (80, 24));
        let v = e.active().document.version;
        e.active_mut().pos.passive = vec![crate::lenses::PosMatch {
            start: 4, end: 21, category: crate::lenses::ProseLensCategory::Passive,
        }];
        e.active_mut().pos.computed_for = Some(v);
        crate::lenses::set_prose_lens(&mut e, Some(crate::lenses::ProseLensCategory::Passive));
        assert_eq!(crate::lenses::prose_lens_count_segment(&e), Some("Passive: 1".to_string()));
        // stale (version bumped without a re-sweep) → suppressed, not shown.
        e.active_mut().document.version += 1;
        assert_eq!(crate::lenses::prose_lens_count_segment(&e), None, "stale store suppresses the segment");
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

    /// Effort A §10 + E10 §12: the Review attribution matrix. Ready → the engine label;
    /// Starting → the STEADY warming label (changed by E10 — pre-E10 Starting was plain);
    /// Idle / Unavailable → plain REVIEW, no attribution dot.
    #[test]
    fn status_line_review_attribution_matrix() {
        use crate::diag_provider::{RecordingProvider, Availability};
        use wordcartel_core::diagnostics::DiagSource;
        let with_availability = |a: Availability| {
            let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
            e.active_mut().view.mode = crate::editor::RenderMode::Review;
            e.diag_providers.install(Box::new(RecordingProvider::new()
                .with_source(DiagSource::Harper).with_availability(a)), true);
            crate::render_status::status_left_text(&e)
        };
        // The label comes from DiagSource::Harper.label(), not the provider's own identity.
        assert!(with_availability(Availability::Ready).contains("[REVIEW · Harper]"),
            "Ready → attribution");
        assert!(with_availability(Availability::Starting).contains("[REVIEW · warming Harper…]"),
            "Starting → the steady warming label (E10 §12)");
        for quiet in [Availability::Idle, Availability::Unavailable] {
            let s = with_availability(quiet);
            assert!(s.contains("[REVIEW]") && !s.contains("·"),
                "Idle/Unavailable → plain REVIEW, no attribution dot: {s}");
        }
    }

    #[test]
    fn status_shows_pending_blk_ellipsis_and_direction() {
        let text = (0..50).map(|i| format!("line {i}\n")).collect::<String>();
        let mut e = Editor::new_from_text(&text, None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().pending_block_begin = Some(0);
        assert!(crate::render_status::status_left_text(&e).contains("BLK…"),
            "pending ^KB shows the mid-mark segment");
        e.active_mut().pending_block_begin = None;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 6, hidden: false });
        assert!(crate::render_status::status_left_text(&e).ends_with("· BLK"),
            "in view: plain BLK, no arrow");
        e.active_mut().view.scroll = 30;
        crate::derive::rebuild(&mut e);
        assert!(crate::render_status::status_left_text(&e).contains("BLK↑"),
            "scrolled below the block: BLK↑");
    }

    /// PIN, not red (Codex plan-gate round 1, finding 5): this asserts the EXACT
    /// legacy segment shape and is GREEN at introduction — its job is to stay green
    /// through the GREEN phase below, proving the ④ segments never leak onto a hidden
    /// block (no arrow, no pending, no MK — the status line ENDS at the legacy text).
    #[test]
    fn status_hidden_block_keeps_exact_legacy_segment() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 3, hidden: true });
        let s = crate::render_status::status_left_text(&e);
        assert!(s.ends_with(" · BLK·hidden"),
            "hidden keeps the exact legacy tail — no arrow/pending/MK segment after it: {s}");
        assert!(!s.contains('↑') && !s.contains('↓') && !s.contains("BLK…") && !s.contains("MK "),
            "no ④ segment leaks onto a hidden block: {s}");
    }

    #[test]
    fn status_lists_marks_on_caret_line() {
        let mut e = Editor::new_from_text("one\ntwo\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().marks.insert('a', 5);
        e.active_mut().marks.insert('3', 4);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(6);
        assert!(crate::render_status::status_left_text(&e).contains("· MK 3,a"));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert!(!crate::render_status::status_left_text(&e).contains("MK "),
            "no segment when the caret's line has no marks");
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
