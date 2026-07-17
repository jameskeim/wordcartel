//! Linguistic substrate: rule-based POS tags + NP-chunk flags over a prose text
//! slice, byte-aligned to the caller's buffer. Pure and deterministic — wraps
//! `harper-brill`'s rule-based `BrillTagger`/`BrillChunker` (never the neural
//! `burn_chunker`). Cold-path only; the shell owns caching and the block window.
#![forbid(unsafe_code)]

use unicode_segmentation::UnicodeSegmentation;

/// The Universal POS tagset, re-exported from `harper-brill` (no newtype — it is the
/// standard UD tagset and never enters `wordcartel-core`; S8 maps it to its own theme
/// `SemanticElement` in the shell).
pub use harper_brill::UPOS;

/// One token's analysis: its byte span (in the analyzed slice's coordinates), its POS
/// tag (`None` where the tagger cannot decide), and whether it is a noun-phrase member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenTag {
    /// Byte span of the token in the analyzed slice's coordinates.
    pub range: std::ops::Range<usize>,
    /// The POS tag, or `None` when the tagger cannot decide.
    pub upos: Option<UPOS>,
    /// `true` when the chunker flags this token as part of a noun phrase.
    pub np: bool,
}

/// One sentence's analysis: its content-only span (the S5 authority's span, slice-local)
/// and its tokens in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedSentence {
    /// The S5 content-only sentence span `(from, to)`, in the analyzed slice's coordinates.
    pub span: (usize, usize),
    /// The sentence's tokens, in order; `range`s are parallel to and within `span`.
    pub tokens: Vec<TokenTag>,
}

/// A token's first char is alphanumeric — the lookup gate for the underscore strip-then-test.
/// Inlined here (NOT `wordcartel-core`'s private `textobj::is_word`) so `wordcartel-nlp`'s only
/// dependency on core stays `textobj::sentence_spans`. It is a lookup gate, not a segmentation
/// authority, so it cannot drift from the S5 word/sentence engine.
fn first_is_alphanumeric(s: &str) -> bool {
    s.chars().next().is_some_and(char::is_alphanumeric)
}

/// Tokenize one sentence sub-slice into harper-ready lookup strings + parallel sentence-local
/// byte ranges. UAX-29 word boundaries (`split_word_bound_indices`), whitespace-only segments
/// skipped. For each kept segment: strip-then-test the underscore adornment (narrowing the range
/// to the inner word when the stripped inner is non-empty, differs, and is alphanumeric-first),
/// then normalize a typographic apostrophe (U+2019 → ASCII) in the LOOKUP string only.
fn tokenize_sentence(sent: &str) -> (Vec<String>, Vec<std::ops::Range<usize>>) {
    let mut toks = Vec::new();
    let mut spans = Vec::new();
    for (rel_start, seg) in sent.split_word_bound_indices() {
        if seg.trim().is_empty() {
            continue; // gap material
        }
        // Underscore strip-then-test (order is deliberate — a leading-'_' token is not
        // alphanumeric-first, so a test-then-strip rule would never fire on "_quiet_").
        let inner = seg.trim_matches('_');
        let (lookup_src, span) = if !inner.is_empty() && inner != seg && first_is_alphanumeric(inner)
        {
            let lead = seg.len() - seg.trim_start_matches('_').len(); // leading '_' bytes
            let start = rel_start + lead;
            (inner, start..start + inner.len())
        } else {
            (seg, rel_start..rel_start + seg.len())
        };
        // Apostrophe-normalize the LOOKUP only; the recorded span is untouched.
        let lookup = lookup_src.replace('\u{2019}', "'");
        toks.push(lookup);
        spans.push(span);
    }
    (toks, spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_skips_whitespace_and_keeps_punctuation() {
        // "a, b" → tokens ["a", ",", "b"] at 0..1, 1..2, 3..4 (the space at 2 is skipped).
        let (toks, spans) = tokenize_sentence("a, b");
        assert_eq!(toks, vec!["a".to_string(), ",".to_string(), "b".to_string()]);
        assert_eq!(spans, vec![0..1, 1..2, 3..4]);
    }

    #[test]
    fn tokenize_underscore_strip_then_test_narrows_to_inner() {
        // "_quiet_" is ONE UAX-29 segment; strip-then-test narrows to the inner word.
        // "a _quiet_ b": '_quiet_' is bytes 2..9, inner "quiet" is bytes 3..8.
        let (toks, spans) = tokenize_sentence("a _quiet_ b");
        assert_eq!(toks, vec!["a".to_string(), "quiet".to_string(), "b".to_string()]);
        assert_eq!(spans, vec![0..1, 3..8, 10..11]);
    }

    #[test]
    fn tokenize_pure_underscore_token_left_as_is() {
        // A pure-underscore token trims to empty → left unchanged (span not narrowed).
        let (toks, spans) = tokenize_sentence("a __ b");
        assert_eq!(toks, vec!["a".to_string(), "__".to_string(), "b".to_string()]);
        assert_eq!(spans, vec![0..1, 2..4, 5..6]);
    }

    #[test]
    fn tokenize_curly_apostrophe_normalized_in_lookup_span_unchanged() {
        // "it’s" (U+2019, 3 bytes) is one segment 0..6. Lookup normalizes ’→' ("it's");
        // the recorded span is UNCHANGED (0..6), not narrowed.
        let (toks, spans) = tokenize_sentence("it\u{2019}s");
        assert_eq!(toks, vec!["it's".to_string()]);
        assert_eq!(spans, vec![0..6]);
    }

    #[test]
    fn tokenize_multibyte_spans_do_not_split_codepoints() {
        // "café" — é is 2 bytes → span 0..5, lookup unchanged.
        let (toks, spans) = tokenize_sentence("café");
        assert_eq!(toks, vec!["café".to_string()]);
        assert_eq!(spans, vec![0..5]);
    }

    #[test]
    fn tokenize_curly_quotes_are_their_own_multibyte_tokens() {
        // §5.4 "Why?" fixture: curly DOUBLE quotes (U+201C/U+201D, 3 bytes each) are their own
        // tokens — “ at 0..3, ” at 7..10 — and are NOT apostrophe-normalized (only U+2019 is).
        // POS tags for these are asserted in the Task 3 analyze tests.
        let (toks, spans) = tokenize_sentence("“Why?” he");
        assert_eq!(toks[0], "“");
        assert_eq!(spans[0], 0..3);
        assert_eq!(toks[1], "Why");
        assert_eq!(spans[1], 3..6);
        assert_eq!(toks[3], "”");
        assert_eq!(spans[3], 7..10);
    }

    #[test]
    fn tokenize_parallel_length_holds() {
        let (toks, spans) = tokenize_sentence("The tall man ran quickly.");
        assert_eq!(toks.len(), spans.len());
        assert!(!toks.is_empty());
    }
}
