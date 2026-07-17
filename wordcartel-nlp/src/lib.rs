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

use harper_brill::{brill_chunker, brill_tagger, Chunker, Tagger};

/// Analyze a prose text slice into POS tags + NP-chunk flags, byte-aligned to the slice.
///
/// Segments with the S5 sentence authority (`wordcartel_core::textobj::sentence_spans`),
/// tokenizes each sentence with UAX-29 (see [`tokenize_sentence`]), tags with the rule-based
/// `BrillTagger`, chunks with the rule-based `BrillChunker`, and returns one [`TaggedSentence`]
/// per content-bearing sentence. Byte ranges are in the SLICE's coordinates (offset 0 = the
/// slice start); the shell rebases by the window origin. Pure and deterministic.
///
/// # Examples
/// ```
/// let out = wordcartel_nlp::analyze("The cat sat.");
/// assert_eq!(out.len(), 1);
/// assert!(!out[0].tokens.is_empty());
/// ```
pub fn analyze(text: &str) -> Vec<TaggedSentence> {
    let tagger = brill_tagger();
    let chunker = brill_chunker();
    let mut out = Vec::new();
    for (from, to) in wordcartel_core::textobj::sentence_spans(text) {
        let (toks, spans) = tokenize_sentence(&text[from..to]);
        if toks.is_empty() {
            continue;
        }
        let tags = tagger.tag_sentence(&toks);
        let nps = chunker.chunk_sentence(&toks, &tags);
        // Harper returns one element per input token; the zip below relies on it.
        debug_assert_eq!(tags.len(), toks.len(), "tagger must return one tag per token");
        debug_assert_eq!(nps.len(), toks.len(), "chunker must return one flag per token");
        let tokens = spans
            .into_iter()
            .zip(tags)
            .zip(nps)
            .map(|((span, upos), np)| TokenTag {
                range: (from + span.start)..(from + span.end),
                upos,
                np,
            })
            .collect();
        out.push(TaggedSentence { span: (from, to), tokens });
    }
    out
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

    #[test]
    fn analyze_empty_and_whitespace_yield_no_sentences() {
        assert!(analyze("").is_empty());
        assert!(analyze("   \n  ").is_empty());
    }

    #[test]
    fn analyze_model_loads_without_panic() {
        // Exercises the LazyLock deserialization (`serde_json::from_str(...).unwrap()`) inside
        // brill_tagger()/brill_chunker() — must not panic.
        let out = analyze("The cat sat.");
        assert_eq!(out.len(), 1);
        assert!(!out[0].tokens.is_empty());
    }

    #[test]
    fn analyze_is_deterministic() {
        let a = analyze("The tall man walked quietly home.");
        let b = analyze("The tall man walked quietly home.");
        assert_eq!(a, b);
    }

    #[test]
    fn analyze_parallel_length_invariant_three_cases() {
        // Spec §5.2: for every sentence, tokens/tags/np_flags are strictly parallel. Pin it with
        // CONCRETE length asserts against the same harper path analyze uses, for three cases:
        // multi-token, punctuation-only, and empty/whitespace-only.
        use harper_brill::{brill_chunker, brill_tagger, Chunker, Tagger};
        let tagger = brill_tagger();
        let chunker = brill_chunker();
        // (a) multi-token and (b) punctuation-only: content sentences with parallel vectors.
        for input in ["She wrote three drafts.", "!!! ??? ..."] {
            let mut sentences_seen = 0;
            for (from, to) in wordcartel_core::textobj::sentence_spans(input) {
                let (toks, spans) = tokenize_sentence(&input[from..to]);
                if toks.is_empty() {
                    continue;
                }
                let tags = tagger.tag_sentence(&toks);
                let nps = chunker.chunk_sentence(&toks, &tags);
                assert_eq!(spans.len(), toks.len(), "spans len == tokens len: {input:?}");
                assert_eq!(tags.len(), toks.len(), "tags len == tokens len: {input:?}");
                assert_eq!(nps.len(), toks.len(), "np_flags len == tokens len: {input:?}");
                sentences_seen += 1;
            }
            assert!(sentences_seen >= 1, "content input yields >= 1 sentence: {input:?}");
        }
        // (c) empty / whitespace-only shape. `analyze("   ")` yields ZERO sentences (correct —
        // sentence_spans skips whitespace-only), so the four vectors never materialize inside
        // analyze. Pin the invariant concretely for the DEGENERATE shape by driving the identical
        // tokenize→tag→chunk path on an empty slice: an empty token list must give empty tag/np
        // vectors — 0 == 0 == 0 == 0, the same length-agreement made concrete.
        let (etoks, espans) = tokenize_sentence("   ");
        let etags = tagger.tag_sentence(&etoks);
        let enps = chunker.chunk_sentence(&etoks, &etags);
        assert_eq!(etoks.len(), 0, "whitespace-only slice yields zero tokens");
        assert_eq!(espans.len(), etoks.len(), "spans len == tokens len (empty)");
        assert_eq!(etags.len(), etoks.len(), "tags len == tokens len (empty)");
        assert_eq!(enps.len(), etoks.len(), "np_flags len == tokens len (empty)");
        // And the zero-sentence behavior itself.
        assert!(analyze("   \n  ").is_empty());
        // analyze itself carries the aligned tokens for the multi-token case.
        let m = analyze("She wrote three drafts.");
        assert_eq!(m.len(), 1);
        assert!(m[0].tokens.len() >= 4);
    }

    #[test]
    fn analyze_multibyte_byte_offsets_match_probe() {
        // §5.4 worked fixture — exact slice-local byte offsets verified by the grounding probe.
        let t = "The café in Zürich serves espresso — it's excellent.";
        let out = analyze(t);
        assert_eq!(out.len(), 1);
        let toks = &out[0].tokens;
        let by_range = |lo: usize, hi: usize| toks.iter().find(|k| k.range == (lo..hi)).unwrap();
        // café is bytes 4..9 (é 2 bytes) and tags None (accepted floor — not a lens input).
        assert_eq!(by_range(4, 9).upos, None);
        assert_eq!(&t[4..9], "café");
        // Zürich is 13..20 (ü 2 bytes).
        assert_eq!(&t[13..20], "Zürich");
        assert!(toks.iter().any(|k| k.range == (13..20)));
        // the em-dash — is 37..40 (3 bytes) and tags PUNCT.
        assert_eq!(by_range(37, 40).upos, Some(UPOS::PUNCT));
        assert_eq!(&t[37..40], "—");
        // it's is 41..45 and tags PRON.
        assert_eq!(by_range(41, 45).upos, Some(UPOS::PRON));
    }

    #[test]
    fn analyze_underscore_adornment_yields_inner_adj() {
        // "_quiet_" → inner span "quiet", tagged ADJ (the dict value for "quiet").
        let t = "This is _quiet_ prose.";
        let out = analyze(t);
        let inner = out[0]
            .tokens
            .iter()
            .find(|k| &t[k.range.clone()] == "quiet")
            .expect("inner 'quiet' token present");
        assert_eq!(inner.upos, Some(UPOS::ADJ));
    }

    #[test]
    fn analyze_curly_quotes_punct_and_adverbs_adv() {
        // §5.4 "Why?" fixture: curly DOUBLE quotes tag PUNCT at 0..3 / 7..10; Why & quietly ADV.
        let t = "“Why?” he asked the tall man quietly.";
        let out = analyze(t);
        assert_eq!(out.len(), 1);
        let toks = &out[0].tokens;
        let by = |lo: usize, hi: usize| toks.iter().find(|k| k.range == (lo..hi)).unwrap();
        assert_eq!(by(0, 3).upos, Some(UPOS::PUNCT)); // “
        assert_eq!(by(7, 10).upos, Some(UPOS::PUNCT)); // ”
        assert_eq!(by(3, 6).upos, Some(UPOS::ADV)); // Why
        assert_eq!(by(33, 40).upos, Some(UPOS::ADV)); // quietly
    }

    #[test]
    fn analyze_bold_adj_and_noun_phrase_run_flagged() {
        // §5.4: markdown ** split into their own PUNCT tokens (harmless); bold → ADJ.
        let b = "This is **bold** here.";
        let bo = analyze(b);
        let bold = bo[0].tokens.iter().find(|k| &b[k.range.clone()] == "bold").unwrap();
        assert_eq!(bold.upos, Some(UPOS::ADJ));
        // §5.4: "the lazy dog" is a noun-phrase RUN — the chunker flags all three np=true.
        let d = "The quick brown fox doesn't jump over the lazy dog.";
        let out = analyze(d);
        let toks = &out[0].tokens;
        for w in ["the", "lazy", "dog"] {
            // The NP run is at the tail; take the LAST occurrence (avoids the leading "The").
            let k = toks
                .iter()
                .rev()
                .find(|k| d[k.range.clone()].eq_ignore_ascii_case(w))
                .unwrap();
            assert!(k.np, "{w:?} must be flagged as part of the noun phrase");
        }
    }

    #[test]
    fn analyze_curly_apostrophe_tags_same_as_straight() {
        // The curly form's recorded span is the full token (unchanged), and its tag matches the
        // straight-apostrophe form (both PRON).
        let straight = analyze("it's here.");
        let curly = analyze("it\u{2019}s here.");
        assert_eq!(straight[0].tokens[0].upos, Some(UPOS::PRON));
        assert_eq!(curly[0].tokens[0].upos, Some(UPOS::PRON));
        // Span unchanged (not narrowed): "it’s" is 6 bytes (’ is 3).
        assert_eq!(curly[0].tokens[0].range, 0..6);
    }

    #[test]
    fn analyze_contraction_none_floor_does_not_derail_neighbours() {
        // Joined contraction "doesn't" tags None (accepted floor); its neighbours stay correct.
        let t = "The fox doesn't jump.";
        let out = analyze(t);
        let toks = &out[0].tokens;
        let doesnt = toks.iter().find(|k| &t[k.range.clone()] == "doesn't").unwrap();
        assert_eq!(doesnt.upos, None);
        // The AUX/ADV/participle coverage S8 relies on is solid: "The" is DET.
        assert_eq!(toks[0].upos, Some(UPOS::DET));
    }
}
