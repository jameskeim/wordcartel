//! The oracle property test plus targeted regression cases for each hazard
//! construct named in the spike brief.
//!
//! Port of ~/projects/wordcartel-blocktree-spike/tests/oracle.rs — only the
//! module path changes: `wordcartel_blocktree_spike::*` →
//! `wordcartel_core::block_tree::*`.

use proptest::prelude::*;
use wordcartel_core::block_tree::{
    apply_edit, full_parse, incremental_update, incremental_update_instrumented, UpdateOutcome,
    WidenReason,
};

// ---------------------------------------------------------------------------
// Targeted regression cases (deterministic). Each exercises one named hazard.
// ---------------------------------------------------------------------------

/// Run one edit through both paths and assert equality. Returns the outcome so
/// callers can also assert on the widen reason.
fn check(old_text: &str, range: std::ops::Range<usize>, replacement: &str) -> UpdateOutcome {
    let old_tree = full_parse(old_text);
    let (new_text, edit) = apply_edit(old_text, range, replacement);
    let outcome = incremental_update_instrumented(&old_tree, old_text, &edit, &new_text);
    let full = full_parse(&new_text);
    assert_eq!(
        outcome.tree, full,
        "\nINCREMENTAL != FULL\nold_text={old_text:?}\nnew_text={new_text:?}\nreason={:?}\nincremental={:#?}\nfull={:#?}",
        outcome.reason, outcome.tree, full
    );
    outcome
}

#[test]
fn typing_inside_paragraph_is_local() {
    let doc = "First para.\n\nSecond para here.\n\nThird para.\n";
    // Insert a char inside the middle paragraph.
    let pos = doc.find("here").unwrap();
    let out = check(doc, pos..pos, "X");
    assert_eq!(out.reason, WidenReason::Local);
    // Should reparse far less than the whole doc.
    assert!(out.reparsed_bytes < doc.len(), "reparsed {} of {}", out.reparsed_bytes, doc.len());
}

#[test]
fn fenced_code_spanning_blank_lines() {
    let doc = "intro\n\n```\nline1\n\nline2\n```\n\nafter\n";
    // Edit inside the fence, across the blank line region.
    let pos = doc.find("line1").unwrap();
    check(doc, pos..pos + 5, "CHANGED");
}

#[test]
fn opening_a_fence_swallows_rest_of_doc() {
    let doc = "para one\n\npara two\n\npara three\n";
    // Insert an *unterminated* fence at the top. This should now turn
    // everything below into code -> must widen to end.
    let out = check(doc, 0..0, "```\n");
    assert_eq!(out.reason, WidenReason::WidenToEnd);
}

#[test]
fn closing_a_fence_releases_rest_of_doc() {
    let doc = "```\ncode line\nmore code\nfinal\n";
    // Append a closing fence at the very end.
    let end = doc.len();
    check(doc, end..end, "```\n");
}

#[test]
fn html_block_termination() {
    let doc = "before\n\n<div>\nhtml content\n\nstill?\n</div>\n\nafter\n";
    let pos = doc.find("content").unwrap();
    check(doc, pos..pos, "Z");
}

#[test]
fn lazy_blockquote_continuation() {
    // A blockquote with a lazy continuation line (no '>' prefix).
    let doc = "> quoted line one\ncontinued lazily\n\nplain para\n";
    let pos = doc.find("lazily").unwrap();
    check(doc, pos..pos, "Q");
}

#[test]
fn multi_paragraph_list_item() {
    let doc = "- item one para a\n\n  item one para b\n\n- item two\n\nafter list\n";
    let pos = doc.find("para b").unwrap();
    check(doc, pos..pos, "!");
}

#[test]
fn nested_list() {
    let doc = "- a\n  - b\n    - c\n- d\n\nafter\n";
    let pos = doc.find("b").unwrap();
    check(doc, pos..pos, "X");
}

#[test]
fn setext_heading_needs_line_below() {
    // Editing the text of a setext heading, and the underline.
    let doc = "Title here\n=========\n\nbody\n";
    let pos = doc.find("here").unwrap();
    check(doc, pos..pos, " more");
}

#[test]
fn setext_underline_to_thematic_break_ambiguity() {
    // Changing a paragraph followed by `---` (setext h2) by deleting the line
    // above changes `---` from setext underline to thematic break.
    let doc = "para text\n---\n\nbody\n";
    // Delete the paragraph line, leaving `---` at top -> thematic break.
    let end = doc.find("\n---").unwrap() + 1;
    check(doc, 0..end, "");
}

#[test]
fn link_reference_definition_affects_later_links() {
    // A ref def at the top affects a reference link far below.
    let doc = "[foo]: http://example.com\n\nsee [foo] here\n\nmore text\n";
    // Edit the ref def's URL.
    let pos = doc.find("example").unwrap();
    let out = check(doc, pos..pos + 7, "changed");
    assert_eq!(out.reason, WidenReason::WidenToEnd, "ref def edit must widen to end");
}

#[test]
fn adding_a_link_reference_definition() {
    let doc = "see [foo] here\n\nmore text\n";
    // Prepend a ref def.
    let out = check(doc, 0..0, "[foo]: http://example.com\n\n");
    assert_eq!(out.reason, WidenReason::WidenToEnd);
}

#[test]
fn thematic_break_edits() {
    let doc = "above\n\n---\n\nbelow\n";
    let pos = doc.find("above").unwrap();
    check(doc, pos..pos, "X");
}

#[test]
fn atx_heading_edits() {
    let doc = "# Heading\n\npara\n";
    let pos = doc.find("Heading").unwrap();
    check(doc, pos..pos, " Updated");
}

#[test]
fn delete_blank_line_merges_paragraphs() {
    let doc = "para a\n\npara b\n";
    // Delete the blank line between them -> single paragraph.
    let blank = doc.find("\n\n").unwrap();
    check(doc, blank..blank + 1, "");
}

#[test]
fn insert_blank_line_splits_paragraph() {
    let doc = "line one\nline two\n";
    let pos = doc.find("\nline two").unwrap() + 1;
    check(doc, pos..pos, "\n");
}

#[test]
fn edit_at_very_end() {
    let doc = "only para\n";
    let end = doc.len();
    check(doc, end..end, "more\n");
}

#[test]
fn edit_empty_document() {
    let doc = "";
    check(doc, 0..0, "hello world\n");
}

// ---------------------------------------------------------------------------
// Generator: mixes the named constructs.
// ---------------------------------------------------------------------------

fn block_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // plain paragraph (one or two lines, possibly lazy)
        "[a-z]{2,8}( [a-z]{2,8}){0,3}".prop_map(|s| format!("{s}\n")),
        // multi-line paragraph (tests lazy continuation context)
        ("[a-z]{2,6}", "[a-z]{2,6}").prop_map(|(a, b)| format!("{a}\n{b}\n")),
        // ATX heading
        ("#{1,3}", "[a-z]{2,8}").prop_map(|(h, t)| format!("{h} {t}\n")),
        // setext heading
        "[a-z]{2,8}".prop_map(|t| format!("{t}\n===\n")),
        "[a-z]{2,8}".prop_map(|t| format!("{t}\n---\n")),
        // fenced code, possibly with an internal blank line
        "[a-z ]{0,12}".prop_map(|c| format!("```\n{c}\n\ncode2\n```\n")),
        // blockquote with lazy continuation
        ("[a-z]{2,8}", "[a-z]{2,8}").prop_map(|(a, b)| format!("> {a}\n{b}\n")),
        // list, multi-paragraph item + nested
        Just("- a\n\n  cont\n- b\n  - nested\n".to_string()),
        // thematic break
        Just("---\n".to_string()),
        Just("***\n".to_string()),
        // HTML block
        Just("<div>\nhtml\n\nstill html\n</div>\n".to_string()),
        // link reference definition + use
        Just("[ref]: http://x.test\n\nuse [ref] here\n".to_string()),
        // indented code
        Just("    indented code\n    more\n".to_string()),
        // table
        Just("| a | b |\n|---|---|\n| 1 | 2 |\n".to_string()),
    ]
}

fn doc_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(block_strategy(), 1..6).prop_map(|blocks| blocks.join("\n"))
}

fn replacement_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("".to_string()),
        Just("x".to_string()),
        Just("\n".to_string()),
        Just("\n\n".to_string()),
        Just("```\n".to_string()),
        Just("> ".to_string()),
        Just("# ".to_string()),
        Just("---\n".to_string()),
        Just("[r]: http://y.test\n".to_string()),
        Just("<div>".to_string()),
    ]
}

/// A doc plus a (range, replacement) edit. The range is expressed as two
/// fractions in [0,1] so it stays valid as the doc shrinks; we snap to char
/// boundaries at use time.
fn doc_and_edit_strategy() -> impl Strategy<Value = (String, (usize, usize), String)> {
    (doc_strategy(), 0u32..1000, 0u32..1000, replacement_strategy())
        .prop_map(|(doc, a, b, rep)| (doc, (a as usize, b as usize), rep))
}

/// Snap a permille position into a valid char boundary of `text`.
fn snap(text: &str, permille: usize) -> usize {
    let mut pos = (text.len() * permille) / 1000;
    if pos > text.len() {
        pos = text.len();
    }
    while pos < text.len() && !text.is_char_boundary(pos) {
        pos += 1;
    }
    pos
}

// ---------------------------------------------------------------------------
// Generator: sequence of edits for the multi-edit chain test.
// Each edit is represented as two permille positions + a replacement string.
// ---------------------------------------------------------------------------

fn edit_seq_strategy() -> impl Strategy<Value = Vec<(usize, usize, String)>> {
    prop::collection::vec(
        (0u32..1000, 0u32..1000, replacement_strategy()),
        1..=8,
    )
    .prop_map(|v| {
        v.into_iter()
            .map(|(a, b, rep)| (a as usize, b as usize, rep))
            .collect()
    })
}

// ---------------------------------------------------------------------------
// Generator: multibyte corpus (ASCII + é/中/🙂 woven in)
// ---------------------------------------------------------------------------

fn mb_word_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z]{2,6}".prop_map(|s| s),
        Just("é".to_string()),
        Just("中".to_string()),
        Just("🙂".to_string()),
        "[a-z]{1,3}".prop_map(|s| format!("{s}é")),
        "[a-z]{1,3}".prop_map(|s| format!("{s}中")),
        "[a-z]{1,3}".prop_map(|s| format!("{s}🙂{s}")),
    ]
}

fn mb_block_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // plain paragraph mixing ASCII and multibyte
        prop::collection::vec(mb_word_strategy(), 1..5)
            .prop_map(|ws| format!("{}\n", ws.join(" "))),
        // multi-line paragraph
        (mb_word_strategy(), mb_word_strategy())
            .prop_map(|(a, b)| format!("{a}\n{b}\n")),
        // ATX heading with multibyte
        ("#{1,3}", mb_word_strategy())
            .prop_map(|(h, t)| format!("{h} {t}\n")),
        // setext heading
        mb_word_strategy().prop_map(|t| format!("{t}\n===\n")),
        mb_word_strategy().prop_map(|t| format!("{t}\n---\n")),
        // fenced code with multibyte content
        mb_word_strategy().prop_map(|c| format!("```\n{c}\n\ncode2\n```\n")),
        // blockquote with multibyte
        (mb_word_strategy(), mb_word_strategy())
            .prop_map(|(a, b)| format!("> {a}\n{b}\n")),
        // ASCII-only structural blocks (reuse from original)
        Just("- a\n\n  cont\n- b\n  - nested\n".to_string()),
        Just("---\n".to_string()),
        Just("| a | b |\n|---|---|\n| 1 | 2 |\n".to_string()),
    ]
}

fn mb_doc_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(mb_block_strategy(), 1..5).prop_map(|blocks| blocks.join("\n"))
}

fn mb_replacement_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("".to_string()),
        Just("é".to_string()),
        Just("中".to_string()),
        Just("🙂".to_string()),
        Just("x".to_string()),
        Just("\n".to_string()),
        Just("\n\n".to_string()),
        Just("```\n".to_string()),
        Just("> ".to_string()),
        Just("# ".to_string()),
        Just("---\n".to_string()),
        Just("éàü".to_string()),
        Just("中文字".to_string()),
        Just("a🙂b".to_string()),
    ]
}

fn mb_doc_and_edit_strategy() -> impl Strategy<Value = (String, (usize, usize), String)> {
    (mb_doc_strategy(), 0u32..1000, 0u32..1000, mb_replacement_strategy())
        .prop_map(|(doc, a, b, rep)| (doc, (a as usize, b as usize), rep))
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        max_shrink_iters: 4000,
        .. ProptestConfig::default()
    })]

    #[test]
    fn oracle_incremental_equals_full((doc, (pa, pb), rep) in doc_and_edit_strategy()) {
        let lo0 = snap(&doc, pa.min(pb));
        let hi0 = snap(&doc, pa.max(pb));
        let range = lo0..hi0;

        let old_tree = full_parse(&doc);
        let (new_text, edit) = apply_edit(&doc, range, &rep);
        let inc = incremental_update(&old_tree, &doc, &edit, &new_text);
        let full = full_parse(&new_text);
        prop_assert_eq!(inc, full,
            "\noracle mismatch\nold={:?}\nnew={:?}", doc, new_text);
    }

    /// Test A — multi-edit chain.
    ///
    /// Proves that the spliced BlockTree produced by `incremental_update` is a
    /// valid input to a subsequent `incremental_update` call: at each step in
    /// the chain we assert `incremental == full_parse`.
    #[test]
    fn oracle_multi_edit_chain(
        initial in doc_strategy(),
        edits in edit_seq_strategy(),
    ) {
        let mut text = initial;
        let mut tree = full_parse(&text);

        for (pa, pb, rep) in edits {
            let lo = snap(&text, pa.min(pb));
            let hi = snap(&text, pa.max(pb));
            let (new_text, edit) = apply_edit(&text, lo..hi, &rep);
            tree = incremental_update(&tree, &text, &edit, &new_text);
            let full = full_parse(&new_text);
            prop_assert_eq!(
                tree.clone(), full,
                "\nmulti-edit chain mismatch after applying edit ({}..{}, rep={:?})\ntext_before={:?}\ntext_after={:?}",
                lo, hi, rep, text, new_text
            );
            text = new_text;
        }
    }

    /// Test B — multibyte corpus.
    ///
    /// Proves the byte-offset / line_start / line_end arithmetic is UTF-8-safe
    /// when documents and replacements contain multibyte graphemes (é/中/🙂).
    #[test]
    fn oracle_multibyte_corpus((doc, (pa, pb), rep) in mb_doc_and_edit_strategy()) {
        let lo = snap(&doc, pa.min(pb));
        let hi = snap(&doc, pa.max(pb));
        let (new_text, edit) = apply_edit(&doc, lo..hi, &rep);
        let old_tree = full_parse(&doc);
        let inc = incremental_update(&old_tree, &doc, &edit, &new_text);
        let full = full_parse(&new_text);
        prop_assert_eq!(inc, full,
            "\nmultibyte oracle mismatch\nold={:?}\nnew={:?}", doc, new_text);
    }
}
