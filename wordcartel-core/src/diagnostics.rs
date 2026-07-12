//! In-document grammar/spell diagnostics: the pure data contract (Effort A §3.1). Values here are
//! byte ranges into a checked text, produced by a `wordcartel::diag_provider::DiagnosticsProvider`
//! in the shell (harper-ls over LSP, Effort A) and consumed by the shell's DiagStore/render/provider
//! seam. This module holds no IO and no linting logic — the embedded `harper-core` backend that
//! used to live here was removed when Effort A swapped it for the external harper-ls language
//! server (the build-weight payoff: `burn`/`harper-core` are no longer in this crate's dependency
//! graph). Diagnostics are sorted ascending by `range.start` by whichever provider produces them.

/// The linter-agnostic category of a flagged issue — whether a provider judged it a
/// spelling problem or a grammar/style problem. Providers (e.g. harper-ls over LSP) map
/// their own diagnostic taxonomy down to one of these two buckets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiagnosticKind {
    /// A misspelled word or unrecognized token.
    Spelling,
    /// A grammar, punctuation, or style issue.
    Grammar,
}

/// One candidate fix a provider offers for a [`Diagnostic`], expressed as an edit against
/// the diagnostic's own `range`. The shell applies a chosen suggestion as a normal edit;
/// this type carries no IO or provider-specific state.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Suggestion {
    /// Replace the diagnostic's `range` with this text.
    ReplaceWith(String),
    /// Insert this text at `range.end`, leaving the flagged range itself untouched.
    InsertAfter(String),
    /// Delete the diagnostic's `range` entirely, inserting nothing.
    Remove,
}

/// A single flagged issue in a checked text, as reported by a
/// `wordcartel::diag_provider::DiagnosticsProvider` in the shell. Diagnostics are pure data —
/// this crate holds no linting logic, no sorting, and no rendering. The producing provider
/// (e.g. harper-ls) sorts its results ascending by `range.start` before handing them to the
/// shell's `DiagStore`, which just holds them; `wordcartel::render` is what paints them and
/// turns each diagnostic's `suggestions` into a menu for the shell UI.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    /// Byte range into the checked text that the diagnostic covers.
    pub range: std::ops::Range<usize>,
    /// Whether the provider classified this as a spelling or grammar issue.
    pub kind: DiagnosticKind,
    /// Human-readable explanation of the issue, as produced by the provider.
    pub message: String,
    /// Zero or more candidate fixes the provider offers; empty when none apply.
    pub suggestions: Vec<Suggestion>,
}
