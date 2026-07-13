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

/// The engine that produced a diagnostic — the namespace tag behind the per-engine "separate
/// views, never merged" contract. An exhaustive enum, not a free-form string: match sites are
/// forced to place every engine (the `SemanticElement` exhaustive-literal discipline), and an
/// invalid source is unrepresentable. `Plugin` carries a static name for non-core engines
/// (future plugin-declared providers; the test mock uses `Plugin("mock")`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum DiagSource {
    /// harper-ls — the bundled core provider (Effort A).
    Harper,
    /// ltex-ls-plus — reserved vocabulary; provider ships in the ltex/vale effort.
    LTeX,
    /// vale / vale-ls — reserved vocabulary; provider ships in the ltex/vale effort.
    Vale,
    /// A non-core engine, named statically (plugin-declared engines; test mocks).
    Plugin(&'static str),
}

impl DiagSource {
    /// Human-facing label — the status bar (`REVIEW · Harper`), the lens-cycle status, and the
    /// menu state-in-label all use this. `&'static str` on purpose: feeds `MenuMark::Value`.
    pub fn label(self) -> &'static str {
        match self {
            DiagSource::Harper => "Harper",
            DiagSource::LTeX => "LTeX",
            DiagSource::Vale => "vale",
            DiagSource::Plugin(name) => name,
        }
    }
    /// Config-surface name — the `[diagnostics] linters` entry and the `[diagnostics.<engine>]`
    /// table key for this engine.
    pub fn config_name(self) -> &'static str {
        match self {
            DiagSource::Harper => "harper",
            DiagSource::LTeX => "ltex",
            DiagSource::Vale => "vale",
            DiagSource::Plugin(name) => name,
        }
    }
}

/// A single flagged issue in a checked text, as reported by a
/// `wordcartel::diag_provider::DiagnosticsProvider` in the shell. Diagnostics are pure data —
/// this crate holds no linting logic, no sorting, and no rendering. Each engine's provider (e.g.
/// harper-ls, and future ltex/vale/plugin providers) tags its own diagnostics with its
/// `DiagSource` and sorts its results ascending by `range.start` before handing them to the
/// shell's `DiagStore`, which just holds them, partitioned per source; `wordcartel::render` is
/// what paints them and turns each diagnostic's `suggestions` into a menu for the shell UI.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    /// Byte range into the checked text that the diagnostic covers.
    pub range: std::ops::Range<usize>,
    /// Whether the provider classified this as a spelling or grammar issue.
    pub kind: DiagnosticKind,
    /// The engine that produced this diagnostic.
    pub source: DiagSource,
    /// The provider's own diagnostic code, if it reports one (e.g. an LSP `code` field).
    pub code: Option<String>,
    /// A link to further documentation about this diagnostic, if the provider supplies one.
    pub href: Option<String>,
    /// Human-readable explanation of the issue, as produced by the provider.
    pub message: String,
    /// Zero or more candidate fixes the provider offers; empty when none apply.
    pub suggestions: Vec<Suggestion>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diag_source_labels_and_config_names_are_exhaustive() {
        assert_eq!(DiagSource::Harper.label(), "Harper");
        assert_eq!(DiagSource::LTeX.label(), "LTeX");
        assert_eq!(DiagSource::Vale.label(), "vale");
        assert_eq!(DiagSource::Plugin("mock").label(), "mock");
        assert_eq!(DiagSource::Harper.config_name(), "harper");
        assert_eq!(DiagSource::LTeX.config_name(), "ltex");
        assert_eq!(DiagSource::Vale.config_name(), "vale");
        assert_eq!(DiagSource::Plugin("x").config_name(), "x");
    }

    #[test]
    fn diagnostic_carries_source_code_href() {
        let d = Diagnostic {
            range: 0..3, kind: DiagnosticKind::Spelling, source: DiagSource::Harper,
            code: Some("SpellCheck".into()), href: None, message: "m".into(), suggestions: vec![],
        };
        assert_eq!(d.source, DiagSource::Harper);
        assert_eq!(d.code.as_deref(), Some("SpellCheck"));
        assert_eq!(d.href, None);
    }
}
