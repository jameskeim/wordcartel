# S7 — Linguistic substrate (harper-brill in-process) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a pure `wordcartel-nlp` crate that turns a prose text slice into POS tags + NP-chunk flags byte-aligned to our buffer, plus a per-buffer synchronous memo in the shell, so S8's lenses have a substrate.

**Architecture:** A new pure workspace crate `wordcartel-nlp` (dep direction shell → nlp → core) wraps `harper-brill`'s rule-based `BrillTagger`/`BrillChunker`. Its one public function `analyze(&str)` segments with the S5 sentence authority (`wordcartel_core::textobj::sentence_spans`), tokenizes each sentence with UAX-29 (`split_word_bound_indices`), bridges to harper (`tag_sentence` → `chunk_sentence`), and returns `Vec<TaggedSentence>` with slice-local byte spans. The shell caches results in a per-`Buffer` single-slot memo `NlpStore{window, computed_version, sentences}` (`valid_for(version, window)`, modelled on `diagnostics_run::SourceSlot::valid_for`), queried on demand via a leaf `nlp::nlp_window_at` that rebases spans by the window origin `ps` from `commands::prose_window_at`. Cold-path only: no `JobKind`, no worker, no `timers::SUBSYSTEMS` row, zero idle work.

**Tech Stack:** Rust; `harper-brill = "=2.5.0"` (pulls `harper-pos-utils` + `burn` as a compiled-but-DCE'd passenger); `unicode-segmentation` (already a core dep); the shell's existing `Editor`/`Buffer`/`commands` seams.

## Global Constraints

*(Copied verbatim from the spec §3 "Global constraints" — every task's requirements implicitly include this section. Spec: `docs/superpowers/specs/2026-07-17-s7-linguistic-substrate-design.md`.)*

- **`harper-brill` pinned `=2.5.0`.** The embedded trained models (`trained_tagger_model.json`, `trained_chunker_model.json`) are `serde_json`-deserialized artifacts coupled to `harper-pos-utils`'s serde shapes; an exact pin prevents silent model/schema drift on `cargo update`.
- **Two `deny.toml` exceptions to add** (mirroring the existing quick-xml `{ id, reason }` precedent already in `[advisories].ignore`):
  - `[advisories].ignore +=` `{ id = "RUSTSEC-2024-0436", reason = "paste unmaintained; build-time proc-macro pulled via burn-ndarray in harper-pos-utils; no vulnerability, no safe upgrade, unavoidable with burn" }`
  - `[licenses].allow += "CC0-1.0"` with the comment `# tiny-keccak is CC0-1.0 (public-domain dedication; more permissive than the allowed MIT/Apache); pulled via burn's tree.`
- **`cargo deny check` is a RELEASE-CHECKLIST step, NOT a merge gate.** A red result never blocks a merge. The effort's pre-merge report runs it once and records the result; the merge gates remain `cargo test` + workspace clippy (deny) + `too_many_lines` + `module_budgets`.
- **`deny.toml`'s `[licenses]` comment "(287 packages; burn removed)" must be refreshed** when burn re-enters the tree with this effort — the package count and the "burn removed" note are now stale.
- **H2-rationale re-measure obligation (record for the linters effort).** With S7, `burn` is in the linked workspace binary (compiled; the rule-based paths DCE the neural *weights* — verified 1.5 MB probe binary — but the burn *crates* compile). The dep-weight half of the harper-ls subprocess-split rationale is partly spent and must be **re-measured, not assumed**, before it is defended on dep-weight grounds again.
- **Command-surface contract: N/A — does not touch the command surface.** S7 is a substrate: it paints nothing (adds no `SemanticElement` variant), selects nothing (no `Selection` call), mutates nothing (no `edit_apply` path), and adds no command, palette entry, menu entry, user-settable option, or keybinding hint. It follows the A14 anti-regrowth precedent: a leaf module with **no `Command` enum variant and no `commands::run` arm**. The contract's invariant tests (palette-completeness, every-option-has-a-command, hint re-resolution) are unaffected.

**House rules that bite here:** workspace clippy is `deny` (`cargo clippy --workspace --all-targets` must be clean). Do NOT run `cargo fmt`. Match the dense house style. **For any compile/usage/signature question on code you are editing, trust `cargo build`/`grep`, NOT an editor "unused/undefined" hint** — rust-analyzer lags subagent edits.

---

## File structure (one responsibility each)

**Created:**
- `wordcartel-nlp/Cargo.toml` — new crate manifest (deps: `wordcartel-core` path, `harper-brill = "=2.5.0"`, `unicode-segmentation`).
- `wordcartel-nlp/src/lib.rs` — the entire pure crate: `TokenTag`, `TaggedSentence`, `pub use harper_brill::UPOS`, private `tokenize_sentence`, private `first_is_alphanumeric`, public `analyze`.
- `wordcartel/src/nlp.rs` — the shell leaf: `NlpStore` + `valid_for` + `nlp_window_at` query helper.

**Modified:**
- `Cargo.toml` (workspace root) — add `"wordcartel-nlp"` to `members`.
- `deny.toml` — the two exceptions + the stale-comment refresh.
- `docs/design/prose-structure-arc.md` — amend the S7 sentence (core-module → new crate).
- `wordcartel/Cargo.toml` — add the `wordcartel-nlp` path dependency.
- `wordcartel/src/lib.rs` — register `pub mod nlp;`.
- `wordcartel/src/editor.rs` — add `pub nlp: crate::nlp::NlpStore` to `struct Buffer` + init it in `Buffer::from_text`.

---

## Task 1: Workspace plumbing — new crate, members, deny.toml, arc-doc amendment

**Files:**
- Create: `wordcartel-nlp/Cargo.toml`
- Create: `wordcartel-nlp/src/lib.rs`
- Modify: `Cargo.toml` (root `[workspace].members`)
- Modify: `deny.toml`
- Modify: `docs/design/prose-structure-arc.md` (S7 section)

**Interfaces:**
- Consumes: `harper_brill::UPOS` (re-exported), the workspace `[workspace.package] version` and `[workspace.lints]`.
- Produces (later tasks rely on these exact items):
  - `pub struct TokenTag { pub range: std::ops::Range<usize>, pub upos: Option<UPOS>, pub np: bool }`
  - `pub struct TaggedSentence { pub span: (usize, usize), pub tokens: Vec<TokenTag> }`
  - `pub use harper_brill::UPOS;`

- [ ] **Step 1: Create the crate manifest**

Create `wordcartel-nlp/Cargo.toml`:

```toml
[package]
name = "wordcartel-nlp"
version.workspace = true
edition = "2021"
license = "MIT"

[dependencies]
wordcartel-core = { path = "../wordcartel-core" }
harper-brill = "=2.5.0"
unicode-segmentation = "1"

[lints]
workspace = true
```

- [ ] **Step 2: Create the crate lib with the public types + UPOS re-export (no `analyze` yet)**

Create `wordcartel-nlp/src/lib.rs`:

```rust
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
```

- [ ] **Step 3: Add the crate to the workspace members**

In `Cargo.toml` (workspace root), change:

```toml
members = ["wordcartel-core", "wordcartel"]
```

to:

```toml
members = ["wordcartel-core", "wordcartel", "wordcartel-nlp"]
```

- [ ] **Step 4: Add the two `deny.toml` exceptions + refresh the stale comment**

In `deny.toml`, inside `[advisories].ignore`, add the `paste` entry just before the closing `]` (after the two `RUSTSEC-2026-019x` quick-xml entries):

```toml
    # paste (unmaintained) — build-time proc-macro pulled via burn-ndarray in harper-pos-utils
    # (harper-brill's tree, S7). No vulnerability, no safe upgrade, unavoidable with burn.
    { id = "RUSTSEC-2024-0436", reason = "paste unmaintained; build-time proc-macro pulled via burn-ndarray in harper-pos-utils; no vulnerability, no safe upgrade, unavoidable with burn" },
```

In `deny.toml`, replace the stale licenses comment. Change:

```toml
# The permissive set actually used by the post-T6 tree (287 packages; burn removed), enumerated
# from a live `cargo deny list` / `cargo deny check licenses` run against this tree on
# 2026-07-11 — not guessed.
```

to:

```toml
# The permissive set used by the tree. NOTE: burn RE-ENTERED the tree with S7's harper-brill
# dep (2026-07-17), so the previously-recorded "287 packages; burn removed" count is stale —
# re-run `cargo deny list` on the next supply-chain pass to refresh it.
```

In `deny.toml`, inside `[licenses].allow`, add `CC0-1.0` just before the closing `]` (after `"Zlib",`):

```toml
    # tiny-keccak is CC0-1.0 (public-domain dedication; more permissive than the allowed MIT/Apache); pulled via burn's tree.
    "CC0-1.0",
```

- [ ] **Step 5: Amend the arc doc's S7 sentence**

In `docs/design/prose-structure-arc.md`, change:

```markdown
`harper-brill` behind a `wordcartel-core` module: POS tags + NP chunks over the **caret's block
```

to:

```markdown
`harper-brill` behind a **new pure crate `wordcartel-nlp`** (amended 2026-07-17 — superseding the
original "a `wordcartel-core` module" wording; see the S7 spec/plan dated 2026-07-17. Rationale:
`wordcartel-core/fuzz` path-deps core, so a core module would drag burn's compile tree into every
nightly+sanitizer fuzz build, and S7's output needs no core types): POS tags + NP chunks over the **caret's block
```

- [ ] **Step 6: Verify the workspace builds and record `cargo deny`**

Run: `cargo build --workspace`
Expected: `Finished` (first build compiles the burn passenger — ~15–20 s cold; no warnings).

Run: `cargo test --no-run --workspace`
Expected: `Finished` — all test binaries compile, warning-free.

Run: `cargo clippy --workspace --all-targets`
Expected: `Finished` — clean (no warnings; workspace lints are `deny`).

Run: `cargo deny check 2>&1 | tail -20`
Expected: record the summary verbatim in the task report. `advisories ok`/`licenses ok`/`bans`/`sources ok` (or the specific finding if any). Not a gate — recorded only.

- [ ] **Step 7: Commit**

```bash
git add wordcartel-nlp/Cargo.toml wordcartel-nlp/src/lib.rs Cargo.toml Cargo.lock deny.toml docs/design/prose-structure-arc.md
git commit -m "feat(nlp): scaffold wordcartel-nlp crate + deny.toml exceptions + arc-doc amendment (S7 Task 1)"
```

---

## Task 2: The tokenization bridge — `tokenize_sentence` (UAX-29 walk, underscore strip-then-test, apostrophe-normalize)

**Files:**
- Modify: `wordcartel-nlp/src/lib.rs`
- Test: `wordcartel-nlp/src/lib.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `unicode_segmentation::UnicodeSegmentation::split_word_bound_indices` (returns `(usize, &str)` byte-index + segment pairs).
- Produces (Task 3 relies on these exact private items):
  - `fn first_is_alphanumeric(s: &str) -> bool`
  - `fn tokenize_sentence(sent: &str) -> (Vec<String>, Vec<std::ops::Range<usize>>)` — returns lookup strings (apostrophe-normalized, underscore-stripped) parallel to sentence-local byte ranges (underscore-narrowed). Whitespace-only segments are skipped. `tokens.len() == spans.len()`.

- [ ] **Step 1: Write the failing tests**

Add to `wordcartel-nlp/src/lib.rs` (at end of file):

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p wordcartel-nlp tokenize -- --nocapture`
Expected: FAIL — `cannot find function 'tokenize_sentence' in this scope`.

- [ ] **Step 3: Write the implementation**

Add to `wordcartel-nlp/src/lib.rs`, above the `#[cfg(test)]` module:

```rust
use unicode_segmentation::UnicodeSegmentation;

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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p wordcartel-nlp tokenize`
Expected: PASS — `test result: ok. 7 passed`.

Run: `cargo clippy -p wordcartel-nlp --all-targets`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add wordcartel-nlp/src/lib.rs
git commit -m "feat(nlp): tokenization bridge (UAX-29 walk, underscore strip-then-test, apostrophe-normalize) (S7 Task 2)"
```

---

## Task 3: `analyze` — compose sentence spans + bridge + tagger + chunker

**Files:**
- Modify: `wordcartel-nlp/src/lib.rs`
- Test: `wordcartel-nlp/src/lib.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes:
  - `wordcartel_core::textobj::sentence_spans(text: &str) -> impl Iterator<Item = (usize, usize)>` (S5 content-only spans; `pub`, `wordcartel-core/src/textobj.rs`, symbol `sentence_spans`).
  - `harper_brill::brill_tagger() -> std::sync::Arc<harper_brill::BrillTagger<harper_brill::FreqDict>>`
  - `harper_brill::brill_chunker() -> std::sync::Arc<harper_brill::BrillChunker>`
  - `harper_brill::Tagger::tag_sentence(&self, sentence: &[String]) -> Vec<Option<UPOS>>`
  - `harper_brill::Chunker::chunk_sentence(&self, sentence: &[String], tags: &[Option<UPOS>]) -> Vec<bool>`
  - `tokenize_sentence` / `first_is_alphanumeric` (Task 2).
  - `TokenTag` / `TaggedSentence` (Task 1).
- Produces (Task 5 relies on this exact signature):
  - `pub fn analyze(text: &str) -> Vec<TaggedSentence>` — slice-local byte spans; sentences in order; empty input → empty vec.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `wordcartel-nlp/src/lib.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p wordcartel-nlp analyze`
Expected: FAIL — `cannot find function 'analyze' in this scope`.

- [ ] **Step 3: Write the implementation**

Add to `wordcartel-nlp/src/lib.rs`, above the `#[cfg(test)]` module:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p wordcartel-nlp`
Expected: PASS — all Task 2 + Task 3 tests green (`test result: ok. 17 passed`), plus the doctest on `analyze` (`test result: ok. 1 passed` in the Doc-tests run).

Run: `cargo clippy -p wordcartel-nlp --all-targets`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add wordcartel-nlp/src/lib.rs
git commit -m "feat(nlp): analyze() composes sentence_spans + bridge + tagger + chunker (S7 Task 3)"
```

---

## Task 4: `NlpStore` + `valid_for` — the per-buffer memo (shell leaf `nlp.rs`)

**Files:**
- Create: `wordcartel/src/nlp.rs`
- Modify: `wordcartel/Cargo.toml` (add the `wordcartel-nlp` path dependency)
- Modify: `wordcartel/src/lib.rs` (register `pub mod nlp;`)
- Test: `wordcartel/src/nlp.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `wordcartel_nlp::TaggedSentence` (Task 1/3). Shape template: `diagnostics_run::SourceSlot::valid_for(&self, version: u64) -> bool` (checks `computed_version == version` + non-empty).
- Produces (Task 5 relies on these exact items):
  - `pub struct NlpStore { pub window: (usize, usize), pub computed_version: u64, pub sentences: Vec<wordcartel_nlp::TaggedSentence> }` with `#[derive(Debug, Default, Clone)]`.
  - `pub fn valid_for(&self, version: u64, window: (usize, usize)) -> bool`

- [ ] **Step 1: Add the `wordcartel-nlp` dependency to the shell**

In `wordcartel/Cargo.toml`, under `[dependencies]`, add:

```toml
wordcartel-nlp = { path = "../wordcartel-nlp" }
```

(Place it next to the existing `wordcartel-core = { path = "../wordcartel-core" }` line.)

- [ ] **Step 2: Register the module**

In `wordcartel/src/lib.rs`, add after the `pub mod reconcile;` line:

```rust
pub mod nlp;
```

- [ ] **Step 3: Write the failing tests**

Create `wordcartel/src/nlp.rs` with ONLY the store + tests for now:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they fail, then pass**

Run: `cargo test -p wordcartel nlp::tests`
Expected: FAIL first only if the module were missing; since Step 3 writes both store and tests, run and expect PASS — `test result: ok. 5 passed`. (If it fails to compile with "unresolved import `wordcartel_nlp`", confirm Step 1's dependency line — trust `cargo`, not an editor hint.)

Run: `cargo clippy -p wordcartel --all-targets`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add wordcartel/Cargo.toml wordcartel/src/lib.rs wordcartel/src/nlp.rs Cargo.lock
git commit -m "feat(nlp): NlpStore per-buffer memo + valid_for (S7 Task 4)"
```

---

## Task 5: Shell wiring — `Buffer.nlp` field + `nlp_window_at` query helper + guardrails

**Files:**
- Modify: `wordcartel/src/editor.rs` (`struct Buffer` field + `Buffer::from_text` init)
- Modify: `wordcartel/src/nlp.rs` (add `nlp_window_at` + wiring tests)
- Test: `wordcartel/src/nlp.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes:
  - `commands::prose_window_at(editor: &Editor, h: usize) -> Option<(usize, usize)>` (`wordcartel/src/commands.rs`, symbol `prose_window_at`).
  - `Editor::active()`/`active_mut()` (`wordcartel/src/editor.rs`), `Buffer.document.version` (`u64`), `Buffer.document.buffer.slice(range: std::ops::Range<usize>) -> String` (`wordcartel-core/src/buffer.rs`, `TextBuffer::slice`; `BytePos = usize`).
  - `NlpStore` / `valid_for` (Task 4); `wordcartel_nlp::analyze` (Task 3).
  - `crate::timers::SUBSYSTEMS` (`&[TimedSubsystem]` with a `.name` field) — for the guardrail.
- Produces:
  - `pub fn nlp_window_at(editor: &mut Editor, h: usize) -> Option<&[TaggedSentence]>` — the sole shell entry S8 uses; returns absolute-offset sentences for the caret's prose window, or `None` off-prose.
  - `Buffer.nlp: crate::nlp::NlpStore` (new field).

- [ ] **Step 1: Add the `nlp` field to `struct Buffer`**

In `wordcartel/src/editor.rs`, in `pub struct Buffer`, add after the `pub reconcile: crate::reconcile::ReconcileStore,` field:

```rust
    /// per-buffer linguistic-substrate memo (S7) — filled on demand by `nlp::nlp_window_at`.
    pub nlp: crate::nlp::NlpStore,
```

- [ ] **Step 2: Initialize the field in `Buffer::from_text`**

In `wordcartel/src/editor.rs`, in the `Buffer { … }` literal inside `Buffer::from_text`, add after the `reconcile: crate::reconcile::ReconcileStore::default(),` line:

```rust
            nlp: crate::nlp::NlpStore::default(),
```

(The other `Buffer` constructions in `save.rs` use `Buffer { id, ..new_buf }` spread syntax, so they keep compiling — the field is inherited from `new_buf`.)

- [ ] **Step 3: Verify the struct change compiles**

Run: `cargo build -p wordcartel`
Expected: `Finished` — no "missing field `nlp`" error (the single explicit literal is now complete).

- [ ] **Step 4: Write the failing tests (query helper + guardrails)**

Add to the `#[cfg(test)] mod tests` in `wordcartel/src/nlp.rs`:

```rust
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
```

- [ ] **Step 5: Run the tests to verify they fail**

Run: `cargo test -p wordcartel nlp::tests::nlp_window_at_rebases_to_absolute_buffer_offsets`
Expected: FAIL — `cannot find function 'nlp_window_at' in this scope`.

- [ ] **Step 6: Write the `nlp_window_at` implementation**

Add to `wordcartel/src/nlp.rs`, after the `impl NlpStore` block:

```rust
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
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p wordcartel nlp::`
Expected: PASS — all 10 `nlp::tests` green (`test result: ok. 10 passed`).

Run: `cargo clippy -p wordcartel --all-targets`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add wordcartel/src/editor.rs wordcartel/src/nlp.rs
git commit -m "feat(nlp): Buffer.nlp field + nlp_window_at query helper (ps-rebase) + guardrails (S7 Task 5)"
```

---

## Task 6: Pre-merge gates + PTY smoke + cargo deny recorded

**Files:** none (verification only).

**Interfaces:** none.

- [ ] **Step 1: Full test suite (merge GATE)**

Run: `cargo test --workspace`
Expected: PASS — `wordcartel-core` lib + oracle, `wordcartel` lib, `wordcartel-nlp` lib all green. Record the per-crate `test result: ok.` lines.

- [ ] **Step 2: Warning-free build of touched crates (merge GATE)**

Run: `cargo build --workspace && cargo test --no-run --workspace`
Expected: `Finished`, no warnings.

- [ ] **Step 3: Workspace clippy (merge GATE)**

Run: `cargo clippy --workspace --all-targets`
Expected: `Finished` — clean (workspace lints are `deny`, including `too_many_lines` and the house-rule ratchets).

- [ ] **Step 4: Module budgets (merge GATE)**

Run: `cargo test -p wordcartel --test module_budgets`
Expected: PASS — `nlp.rs` is a new leaf (well under any hub budget); `editor.rs` grew by two lines (a field + an init), no hub budget bumped.

- [ ] **Step 5: PTY smoke suite (mandatory-run, advisory-pass — NOT a gate)**

Run: `scripts/smoke/run.sh`
Expected: quote its one-line summary verbatim in the report (e.g. `smoke: 8/8 PASS`, or `smoke: SKIP — no tmux`, or a red `smoke: FAIL sN — advisory`). A red result does NOT block merge — surface it explicitly to the human.

- [ ] **Step 6: `cargo deny check` (RELEASE-CHECKLIST — recorded, NOT a gate)**

Run: `cargo deny check 2>&1 | tail -30`
Expected: record the summary verbatim. `advisories`/`licenses`/`bans`/`sources` results; the `paste` advisory and `CC0-1.0` license must be accepted by the new `deny.toml` exceptions (no NEW error from them). A red result does not block merge — record it.

- [ ] **Step 7: Final commit (if any report artifact is tracked; otherwise none)**

No code change in this task. If the effort keeps a pre-merge report file, add and commit it here; otherwise this task produces only the recorded gate results for the merge report.

---

## Self-review (writing-plans checklist — completed by the author)

**1. Spec coverage.** Spec §2 D-a (new crate + arc amendment) → Task 1. §2 D-b (`NlpStore`/`valid_for`, no JobKind/timers) → Tasks 4–5 + guardrail `s7_adds_no_timers_subsystem_row`. §2 D-c hazards 1/2/3 (contraction floor / apostrophe / underscore) → Task 2 tokenizer tests + Task 3 `analyze_contraction_none_floor…`/`analyze_curly_apostrophe…`/`analyze_underscore_adornment…`. §3 global constraints → verbatim Global Constraints section + Task 1 deny/pin + Task 6 deny-recorded. §4 harper API → Task 3 Interfaces + impl. §5 bridge (UAX-29 walk / whitespace-skip / strip-then-test / apostrophe / byte-rebase) → Task 2 + Task 3 + Task 5. §5.2 parallel-length invariant (multi / empty / punct-only, concrete length asserts) → `analyze_parallel_length_invariant_three_cases`. §5.4 worked fixtures — café/Zürich/em-dash/it's → `analyze_multibyte_byte_offsets_match_probe`; curly-QUOTE spans → `tokenize_curly_quotes_are_their_own_multibyte_tokens` + tags in `analyze_curly_quotes_punct_and_adverbs_adv`; quietly→ADV (same); bold→ADJ + `the lazy dog` NP run → `analyze_bold_adj_and_noun_phrase_run_flagged`; it's→PRON → `analyze_multibyte…` + `analyze_curly_apostrophe…`. §5.5 result types + UPOS re-export → Task 1. §6 cache + `nlp_window_at` ps-rebase + memo-hit + off-prose→None → Task 4 + Task 5 (`nlp_window_at_off_prose_returns_none`). §7 scope/command-surface N/A → Global Constraints. §9 6-task sketch → Tasks 1–6. All covered.

**2. Placeholder scan.** No "TBD"/"similar to Task N"/"add error handling"/"write tests for the above". Every code step shows complete code; every test has real assertions with concrete expected values; every command has expected output.

**3. Type consistency.** `TokenTag{range: Range<usize>, upos: Option<UPOS>, np: bool}` and `TaggedSentence{span:(usize,usize), tokens:Vec<TokenTag>}` are identical across Tasks 1/3/4/5. `analyze(&str)->Vec<TaggedSentence>`, `tokenize_sentence(&str)->(Vec<String>, Vec<Range<usize>>)`, `NlpStore::valid_for(&self,u64,(usize,usize))->bool`, `nlp_window_at(&mut Editor, usize)->Option<&[TaggedSentence]>` — used consistently everywhere they appear. `prose_window_at`/`slice`/`SUBSYSTEMS.name` match the real source read for this plan.

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-17-s7-linguistic-substrate.md`. Codex gates the plan next (per the effort pipeline); execution begins only after a clean plan gate. When it does: **subagent-driven-development** (fresh implementer per task + two-stage review) is the recommended mode.
