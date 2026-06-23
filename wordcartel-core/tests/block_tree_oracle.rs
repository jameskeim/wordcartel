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

// ---------------------------------------------------------------------------
// Gap 1: multibyte × multi-edit chain
// Separate proptest block so we can use a distinct case count (~256).
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 4000,
        .. ProptestConfig::default()
    })]

    /// Test C — multibyte multi-edit chain (Gap 1).
    ///
    /// Combines the multibyte corpus with a chain of 1..8 edits. Proves that
    /// spliced BlockTrees produced by incremental_update remain correct across
    /// multiple edits when both the initial document AND each replacement
    /// contain multibyte graphemes (é/中/🙂). Positions are snapped to char
    /// boundaries. The result tree of edit N is fed as old_tree of edit N+1,
    /// and we assert incremental == full_parse at each step.
    #[test]
    fn oracle_mb_multi_edit_chain(
        initial in mb_doc_strategy(),
        edits in prop::collection::vec(
            (0u32..1000, 0u32..1000, mb_replacement_strategy()),
            1..=8,
        ),
    ) {
        let mut text = initial;
        let mut tree = full_parse(&text);

        for (pa, pb, rep) in edits {
            let pa = pa as usize;
            let pb = pb as usize;
            let lo = snap(&text, pa.min(pb));
            let hi = snap(&text, pa.max(pb));
            let (new_text, edit) = apply_edit(&text, lo..hi, &rep);
            tree = incremental_update(&tree, &text, &edit, &new_text);
            let full = full_parse(&new_text);
            prop_assert_eq!(
                tree.clone(), full,
                "\nmb multi-edit chain mismatch after applying edit ({}..{}, rep={:?})\ntext_before={:?}\ntext_after={:?}",
                lo, hi, rep, text, new_text
            );
            text = new_text;
        }
    }
}

// ---------------------------------------------------------------------------
// Gap 2: delete-to-empty hazard regression
// ---------------------------------------------------------------------------

/// Helper: builds a non-trivial multi-block doc and returns it.
fn make_multiblock_doc() -> String {
    // Heading + paragraph + fenced code + another paragraph.
    "# Title\n\nFirst paragraph with some text.\n\n```\ncode block\n```\n\nSecond paragraph.\n".to_string()
}

#[test]
fn hazard_delete_entire_document_to_empty() {
    let doc = make_multiblock_doc();
    let len = doc.len();
    let old_tree = full_parse(&doc);
    let (new_text, edit) = apply_edit(&doc, 0..len, "");
    assert_eq!(
        new_text, "",
        "replacement should produce empty string"
    );
    let inc = incremental_update(&old_tree, &doc, &edit, &new_text);
    let full = full_parse("");
    assert_eq!(
        inc, full,
        "\ndelete-to-empty: incremental != full\ndoc={:?}\nincremental={:#?}\nfull={:#?}",
        doc, inc, full
    );
}

#[test]
fn hazard_delete_to_single_char() {
    let doc = make_multiblock_doc();
    let len = doc.len();
    // Keep only the very first byte (ASCII 'h' from "# Title").
    let old_tree = full_parse(&doc);
    let (new_text, edit) = apply_edit(&doc, 1..len, "");
    assert_eq!(new_text.len(), 1, "should be left with one character");
    let inc = incremental_update(&old_tree, &doc, &edit, &new_text);
    let full = full_parse(&new_text);
    assert_eq!(
        inc, full,
        "\ndelete-to-single-char: incremental != full\ndoc={:?}\nnew_text={:?}\nincremental={:#?}\nfull={:#?}",
        doc, new_text, inc, full
    );
}

/// Regression test: proptest oracle_multi_edit_chain found a real bug where
/// inserting a link-reference-definition AFTER a list caused full_parse to
/// return a different tree structure than incremental_update.
///
/// Root cause: parse_region's Event::End handler was popping the block stack
/// unconditionally for ALL End events, including inline tags (Link, Image,
/// Emphasis, etc.) that never pushed anything onto the stack.  When a list
/// item's paragraph contained an inline link (e.g. "[r]" resolved via a
/// ref-def added by this edit), End(Link) would spuriously pop the enclosing
/// Paragraph, shifting all subsequent End events one slot too early and
/// causing the second ListItem to end up as a sibling of the List instead of
/// a child.
///
/// The fix: check `tag_end_is_block()` before popping; ignore End events for
/// inline/table-internal tags.
///
/// The bug was first reproducible as a single-edit oracle failure:
///   incremental_update(&full_parse(text1), text1, &edit2, text2)
///     != full_parse(text2)
/// even without the chained edit1.  We test both forms.
#[test]
fn regression_inline_link_end_corrupts_list_nesting() {
    // text1: a nested-list doc where "[r]" appears in a list item but "[r]:"
    // is not yet a ref-def, so "[r]" is plain text.
    let text1 = "    indented code\n    more\n\n- a\n\n  c[r]: http://y.test\nt\n- b\n  - nested\n\n---\n\n[ref]: http://x.test\n\nuse [ref] here\n\naaaa\naaaaaa\n";
    // text2: insert "[r]: http://y.test\n" at byte 78 of text1, making "[r]"
    // inside the list item resolve as a link.  full_parse must yield the same
    // tree whether we call it directly or via incremental_update.
    let (text2, edit2) = apply_edit(text1, 78..78, "[r]: http://y.test\n");

    // Single-edit form (simpler): feed full_parse(text1) as old_tree.
    let t1_full = full_parse(text1);
    let inc_single = incremental_update(&t1_full, text1, &edit2, &text2);
    let full2 = full_parse(&text2);
    assert_eq!(
        inc_single, full2,
        "\nregression: single-edit incremental != full_parse\ntext1={text1:?}\ntext2={text2:?}\nincremental={inc_single:#?}\nfull={full2:#?}"
    );

    // Chained form: replay edit1 first, then edit2.
    // edit1: replace initial[36..38] ("on") with "[r]: http://y.test\n"
    let initial = "    indented code\n    more\n\n- a\n\n  cont\n- b\n  - nested\n\n---\n\n[ref]: http://x.test\n\nuse [ref] here\n\naaaa\naaaaaa\n";
    let (text1_check, edit1) = apply_edit(initial, 36..38, "[r]: http://y.test\n");
    assert_eq!(text1_check, text1, "edit1 must reconstruct text1");
    let t0 = full_parse(initial);
    let t1_inc = incremental_update(&t0, initial, &edit1, text1);
    let inc_chain = incremental_update(&t1_inc, text1, &edit2, &text2);
    assert_eq!(
        inc_chain, full2,
        "\nregression: chained incremental != full_parse\ntext2={text2:?}\nincremental={inc_chain:#?}\nfull={full2:#?}"
    );
}
