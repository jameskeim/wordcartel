//! In-document grammar/spell diagnostics: the pure data contract (Effort A §3.1). Values here are
//! byte ranges into a checked text, produced by a `wordcartel::diag_provider::DiagnosticsProvider`
//! in the shell (harper-ls over LSP, Effort A) and consumed by the shell's DiagStore/render/provider
//! seam. This module holds no IO and no linting logic — the embedded `harper-core` backend that
//! used to live here was removed when Effort A swapped it for the external harper-ls language
//! server (the build-weight payoff: `burn`/`harper-core` are no longer in this crate's dependency
//! graph). Diagnostics are sorted ascending by `range.start` by whichever provider produces them.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiagnosticKind {
    Spelling,
    Grammar,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Suggestion {
    ReplaceWith(String), // replace the diagnostic's range with this text
    InsertAfter(String), // insert this text at range.end (no deletion)
    Remove,              // delete the diagnostic's range
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    pub range: std::ops::Range<usize>, // byte range into the checked text
    pub kind: DiagnosticKind,
    pub message: String,
    pub suggestions: Vec<Suggestion>,
}
