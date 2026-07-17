//! Linguistic substrate: rule-based POS tags + NP-chunk flags over a prose text
//! slice, byte-aligned to the caller's buffer. Pure and deterministic — wraps
//! `harper-brill`'s rule-based `BrillTagger`/`BrillChunker` (never the neural
//! `burn_chunker`). Cold-path only; the shell owns caching and the block window.
#![forbid(unsafe_code)]

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
