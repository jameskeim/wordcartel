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
}
