//! In-document grammar/spell diagnostics (spec §3.1). Wraps `harper-core`.
//! PURE: no IO, no threads, no global mutable state. Deterministic per
//! (text, opts). The shell injects the personal dictionary via
//! `CheckOpts.ignore_words`; the main dictionary is embedded by Harper (Task-1
//! gate confirms no core filesystem IO).

// Types and public API

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
    pub range: std::ops::Range<usize>, // byte range into `text`
    pub kind: DiagnosticKind,
    pub message: String,
    pub suggestions: Vec<Suggestion>,
}

pub struct CheckOpts<'a> {
    pub grammar: bool,
    pub ignore_words: &'a std::collections::HashSet<String>,
}

/// Run Harper over `text`, returning diagnostics sorted ascending by
/// `range.start`. Spelling lints → DiagnosticKind::Spelling; the curated
/// grammar/style set → DiagnosticKind::Grammar (suppressed when !opts.grammar).
/// Words in `ignore_words` (case-insensitive on the flagged surface form) are
/// dropped. Harper char-spans are converted to BYTE ranges.
pub fn check(text: &str, opts: &CheckOpts) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = harper_lints(text)
        .into_iter()
        .filter_map(|lint| {
            let kind = classify(&lint)?; // None → not in enabled set
            if !opts.grammar && kind == DiagnosticKind::Grammar {
                return None;
            }
            // char_span_to_bytes converts Harper's SOURCE character offsets
            // (Lint.span is Span<char>) to UTF-8 byte offsets into `text`.
            let range = char_span_to_bytes(text, lint.span);
            if kind == DiagnosticKind::Spelling {
                let surface = text.get(range.clone()).unwrap_or("").to_lowercase();
                if opts
                    .ignore_words
                    .iter()
                    .any(|w| w.to_lowercase() == surface)
                {
                    return None;
                }
            }
            // Map Harper's Suggestion enum → ours (ReplaceWith/InsertAfter/Remove),
            // converting Vec<char> → String. NEVER flatten to a bare string.
            let suggestions = map_suggestions(lint.suggestions);
            Some(Diagnostic {
                range,
                kind,
                message: lint.message,
                suggestions,
            })
        })
        .collect();
    out.sort_by_key(|d| d.range.start);
    out
}

// ── Harper adapter helpers ────────────────────────────────────────────────────

use harper_core::{
    Dialect, Document,
    linting::{LintGroup, LintKind, Linter},
    spell::FstDictionary,
};

/// Internal shim: re-export the fields we need from a Harper `Lint`.
struct HarperLint {
    span: harper_core::Span<char>,
    lint_kind: LintKind,
    message: String,
    suggestions: Vec<harper_core::linting::Suggestion>,
}

impl From<harper_core::linting::Lint> for HarperLint {
    fn from(l: harper_core::linting::Lint) -> Self {
        HarperLint {
            span: l.span,
            lint_kind: l.lint_kind,
            message: l.message,
            suggestions: l.suggestions,
        }
    }
}

/// Run Harper's full curated lint set on `text` and return raw lints.
fn harper_lints(text: &str) -> Vec<HarperLint> {
    let doc = Document::new_plain_english_curated(text);
    let dict = FstDictionary::curated();
    let mut group = LintGroup::new_curated(dict, Dialect::American);
    let lints = group.lint(&doc);
    lints.into_iter().map(HarperLint::from).collect()
}

/// Classify a Harper lint into our two-variant kind, or `None` to drop it.
///
/// Curated mapping (discovered at Task-1 gate, harper-core 2.5.0):
/// - `LintKind::Spelling` → `DiagnosticKind::Spelling`
/// - `LintKind::Repetition` → `DiagnosticKind::Grammar`  (repeated words)
/// - `LintKind::Grammar`   → `DiagnosticKind::Grammar`  (pronoun agreement, etc.)
/// - `LintKind::Capitalization` → `DiagnosticKind::Grammar`  (sentence caps)
/// - All others → `None` (dropped; shell config Task-2 can expand)
fn classify(lint: &HarperLint) -> Option<DiagnosticKind> {
    match lint.lint_kind {
        LintKind::Spelling => Some(DiagnosticKind::Spelling),
        LintKind::Repetition | LintKind::Grammar | LintKind::Capitalization => {
            Some(DiagnosticKind::Grammar)
        }
        _ => None,
    }
}

/// Convert a Harper `Span<char>` (SOURCE character indices) to a UTF-8 byte
/// range into `text`. Uses `char_indices()` so multi-byte chars map correctly.
///
/// `Span<char>.start` and `.end` are character indices (0-based count of Unicode
/// scalar values). `char_indices()` yields `(byte_offset, char)` pairs; we walk
/// them to map char index → byte offset.
fn char_span_to_bytes(text: &str, span: harper_core::Span<char>) -> std::ops::Range<usize> {
    let mut start_byte = text.len();
    let mut end_byte = text.len();

    for (char_idx, (byte_offset, _ch)) in text.char_indices().enumerate() {
        if char_idx == span.start {
            start_byte = byte_offset;
        }
        if char_idx == span.end {
            end_byte = byte_offset;
            break;
        }
    }

    start_byte..end_byte
}

/// Map Harper's `Suggestion` variants to our own, converting `Vec<char>` → `String`.
fn map_suggestions(
    harper_suggestions: Vec<harper_core::linting::Suggestion>,
) -> Vec<Suggestion> {
    harper_suggestions
        .into_iter()
        .map(|s| match s {
            harper_core::linting::Suggestion::ReplaceWith(chars) => {
                Suggestion::ReplaceWith(chars.into_iter().collect())
            }
            harper_core::linting::Suggestion::InsertAfter(chars) => {
                Suggestion::InsertAfter(chars.into_iter().collect())
            }
            harper_core::linting::Suggestion::Remove => Suggestion::Remove,
        })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn opts<'a>(grammar: bool, ignore: &'a HashSet<String>) -> CheckOpts<'a> {
        CheckOpts { grammar, ignore_words: ignore }
    }

    #[test]
    fn flags_a_misspelling_as_spelling_with_byte_range() {
        let ignore = HashSet::new();
        let ds = check("I love teh cat.", &opts(true, &ignore));
        let spell: Vec<_> = ds.iter().filter(|d| d.kind == DiagnosticKind::Spelling).collect();
        assert!(!spell.is_empty(), "expected a spelling diagnostic for 'teh'");
        let d = spell[0];
        assert_eq!(&"I love teh cat."[d.range.clone()], "teh", "range must cover the misspelled word");
        assert!(d.suggestions.iter().any(|s| matches!(s, Suggestion::ReplaceWith(t) if t == "the")),
                "expected ReplaceWith(\"the\") among suggestions, got {:?}", d.suggestions);
    }

    #[test]
    fn repeated_word_is_grammar_and_suppressed_when_grammar_off() {
        let ignore = HashSet::new();
        let on = check("the the cat", &opts(true, &ignore));
        assert!(on.iter().any(|d| d.kind == DiagnosticKind::Grammar), "repeated 'the' should be a Grammar diagnostic");
        let off = check("the the cat", &opts(false, &ignore));
        assert!(off.iter().all(|d| d.kind != DiagnosticKind::Grammar), "grammar=false must suppress Grammar diagnostics");
    }

    #[test]
    fn ignore_words_drops_the_diagnostic() {
        let mut ignore = HashSet::new();
        ignore.insert("teh".to_string());
        let ds = check("teh cat", &opts(true, &ignore));
        assert!(ds.iter().all(|d| &"teh cat"[d.range.clone()] != "teh"), "ignored word must not be flagged");
    }

    #[test]
    fn multibyte_offsets_are_byte_accurate() {
        // 'é' is 2 bytes; the misspelling after it must have correct BYTE offsets.
        let ignore = HashSet::new();
        let text = "café teh";
        let ds = check(text, &opts(true, &ignore));
        let d = ds.iter().find(|d| text.get(d.range.clone()) == Some("teh")).expect("byte-accurate 'teh'");
        assert_eq!(d.range.start, "café ".len()); // 6 bytes
    }

    #[test]
    fn deterministic_and_sorted() {
        let ignore = HashSet::new();
        let a = check("teh teh", &opts(true, &ignore));
        let b = check("teh teh", &opts(true, &ignore));
        assert_eq!(a, b, "check must be deterministic");
        assert!(a.windows(2).all(|w| w[0].range.start <= w[1].range.start), "sorted by range.start");
    }
}
