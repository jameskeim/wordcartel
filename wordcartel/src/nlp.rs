//! Linguistic-substrate cache (shell leaf). A per-buffer single-slot memo of `analyze`'s output
//! over the caret's prose window, queried on demand. Cold-path only: no `JobKind`, no worker, no
//! `timers::SUBSYSTEMS` row (zero idle work). `valid_for` mirrors `diagnostics_run::SourceSlot`.

use wordcartel_nlp::TaggedSentence;

/// Per-buffer memo of the last analyzed prose window. `valid_for` gates a memo hit on the exact
/// window AND the document version, plus a non-empty guard so a fresh (default) store always
/// recomputes on first query. A prose window always has content, so a real query never thrashes.
#[derive(Debug, Default, Clone)]
pub struct NlpStore {
    /// The absolute buffer byte span the memo was computed for.
    pub window: (usize, usize),
    /// The `document.version` the memo reflects.
    pub computed_version: u64,
    /// The analyzed sentences, spans already rebased to ABSOLUTE buffer offsets.
    pub sentences: Vec<TaggedSentence>,
}

impl NlpStore {
    /// A memo hit requires the same document version AND the same window AND a non-empty result.
    pub fn valid_for(&self, version: u64, window: (usize, usize)) -> bool {
        self.computed_version == version && self.window == window && !self.sentences.is_empty()
    }
}

use crate::editor::Editor;

/// The linguistic analysis for the caret's prose window, or `None` when `h` is not in prose.
///
/// Backed by the per-buffer [`NlpStore`] memo: on a hit (same version + window) it returns the
/// cached sentences without re-running `analyze`; on a miss it analyzes the window slice, rebases
/// every span from slice-local to ABSOLUTE buffer offsets by the window origin `ps`, stores, and
/// returns. Cold-path only — call it from a lens/command, never per-keystroke.
pub fn nlp_window_at(editor: &mut Editor, h: usize) -> Option<&[TaggedSentence]> {
    let (ps, pe) = crate::commands::prose_window_at(editor, h)?;
    let version = editor.active().document.version;
    let b = editor.active_mut();
    if !b.nlp.valid_for(version, (ps, pe)) {
        let slice = b.document.buffer.slice(ps..pe);
        let mut sentences = wordcartel_nlp::analyze(&slice);
        for s in &mut sentences {
            s.span = (s.span.0 + ps, s.span.1 + ps);
            for tok in &mut s.tokens {
                tok.range = (tok.range.start + ps)..(tok.range.end + ps);
            }
        }
        b.nlp = NlpStore { window: (ps, pe), computed_version: version, sentences };
    }
    Some(b.nlp.sentences.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_nlp::{TaggedSentence, TokenTag, UPOS};

    fn sample() -> Vec<TaggedSentence> {
        vec![TaggedSentence {
            span: (0, 3),
            tokens: vec![TokenTag { range: 0..3, upos: Some(UPOS::DET), np: false }],
        }]
    }

    #[test]
    fn default_store_is_invalid() {
        // Fresh buffer: empty sentences → never a hit, so the first query always computes.
        let s = NlpStore::default();
        assert!(!s.valid_for(0, (0, 0)));
    }

    #[test]
    fn populated_store_hits_on_same_version_and_window() {
        let s = NlpStore { window: (10, 40), computed_version: 5, sentences: sample() };
        assert!(s.valid_for(5, (10, 40)));
    }

    #[test]
    fn version_bump_invalidates() {
        let s = NlpStore { window: (10, 40), computed_version: 5, sentences: sample() };
        assert!(!s.valid_for(6, (10, 40)));
    }

    #[test]
    fn window_move_invalidates() {
        let s = NlpStore { window: (10, 40), computed_version: 5, sentences: sample() };
        assert!(!s.valid_for(5, (11, 40)));
    }

    #[test]
    fn empty_sentences_never_valid() {
        let s = NlpStore { window: (10, 40), computed_version: 5, sentences: Vec::new() };
        assert!(!s.valid_for(5, (10, 40)));
    }

    use crate::editor::Editor;

    #[test]
    fn nlp_window_at_rebases_to_absolute_buffer_offsets() {
        // Leading heading + blank line so the prose paragraph starts at a non-zero offset,
        // proving the ps-rebase (not just an offset-0 pass-through).
        let t = "# Title\n\nThe cat sat quietly.\n";
        let mut e = Editor::new_from_text(t, None, (80, 24));
        let h = t.find("cat").unwrap(); // a caret byte inside the prose line
        // Extract the OWNED first-token range in an inner scope so the `&mut e`-derived borrow
        // (`sents`) drops before we reborrow `e` immutably for the slice check. `Range<usize>`
        // is `Clone`, so we carry only the owned range out of the scope.
        let first_range = {
            let sents = nlp_window_at(&mut e, h).expect("prose window present");
            assert!(!sents.is_empty());
            sents[0].tokens[0].range.clone()
        };
        // The first token's absolute range slices back to "The" in the real buffer.
        assert_eq!(e.active().document.buffer.slice(first_range.clone()), "The");
        // And that absolute start equals "The"'s byte offset in the source (9), proving rebase.
        assert_eq!(first_range.start, t.find("The").unwrap());
    }

    #[test]
    fn nlp_window_at_off_prose_returns_none() {
        // A caret in a non-prose block (an ATX heading) → `commands::prose_window_at` returns None
        // (role_at != Paragraph) → `nlp_window_at` returns None. Objects say "I don't know."
        let t = "# Heading only\n";
        let mut e = Editor::new_from_text(t, None, (80, 24));
        let h = t.find("Heading").unwrap();
        assert!(nlp_window_at(&mut e, h).is_none(), "off-prose caret yields no analysis");
    }

    #[test]
    fn nlp_window_at_memo_hit_does_not_recompute() {
        // Populate, then plant a sentinel; a second query at the same version+window must be a
        // memo hit that returns the tampered data UNCHANGED (proving analyze was NOT re-run).
        let t = "# Title\n\nThe cat sat quietly.\n";
        let mut e = Editor::new_from_text(t, None, (80, 24));
        let h = t.find("cat").unwrap();
        let _ = nlp_window_at(&mut e, h).expect("first query populates");
        e.active_mut().nlp.sentences.push(TaggedSentence { span: (999, 999), tokens: Vec::new() });
        let sents = nlp_window_at(&mut e, h).expect("second query is a hit");
        assert_eq!(sents.last().unwrap().span, (999, 999), "memo hit must not recompute");
    }

    #[test]
    fn nlp_window_at_recomputes_after_version_bump() {
        let t = "# Title\n\nThe cat sat quietly.\n";
        let mut e = Editor::new_from_text(t, None, (80, 24));
        let h = t.find("cat").unwrap();
        let _ = nlp_window_at(&mut e, h).expect("populate");
        e.active_mut().nlp.sentences.push(TaggedSentence { span: (999, 999), tokens: Vec::new() });
        e.active_mut().document.version += 1; // simulate an edit having advanced the version
        let sents = nlp_window_at(&mut e, h).expect("query after bump");
        assert!(sents.iter().all(|s| s.span != (999, 999)), "version bump must invalidate the memo");
    }

    #[test]
    fn s7_adds_no_timers_subsystem_row() {
        // Resource law (strongest form): S7 is pull-based, so it must add NO timed subsystem —
        // an idle wake dispatches no S7 work.
        assert!(
            !crate::timers::SUBSYSTEMS.iter().any(|s| s.name == "nlp"),
            "S7 must not add a timers::SUBSYSTEMS row"
        );
    }
}
