//! The oracle property test plus targeted regression cases for each hazard
//! construct named in the spike brief.
//!
//! Port of ~/projects/wordcartel-blocktree-spike/tests/oracle.rs — only the
//! module path changes: `wordcartel_blocktree_spike::*` →
//! `wordcartel_core::block_tree::*`.

use proptest::prelude::*;
use ropey::Rope;
use wordcartel_core::block_tree::{
    apply_edit, full_parse, full_parse_rope, incremental_update, incremental_update_instrumented,
    incremental_update_rope, UpdateOutcome, WidenReason,
};

// ---------------------------------------------------------------------------
// Task-5 helpers: assert str path == rope path == full_parse
// ---------------------------------------------------------------------------

/// Assert that str-incremental, rope-incremental, and full_parse all agree for
/// a single edit. For use inside proptests (uses prop_assert_eq!).
///
/// NOTE: this function contains `prop_assert_eq!` — it must only be called
/// from within a `proptest!` closure.
macro_rules! assert_all_paths_agree {
    ($old:expr, $edit:expr, $new:expr) => {{
        let old: &str = $old;
        let edit = $edit;
        let new: &str = $new;
        let ot = full_parse(old);
        let full = full_parse(new);
        let str_inc = incremental_update(&ot, old, edit, new);
        let rope_inc = incremental_update_rope(
            &ot,
            &Rope::from_str(old),
            edit,
            &Rope::from_str(new),
        );
        prop_assert_eq!(&str_inc, &full,
            "\nstr path != full_parse\nold={:?}\nnew={:?}", old, new);
        prop_assert_eq!(&rope_inc, &full,
            "\nrope path != full_parse\nold={:?}\nnew={:?}", old, new);
        prop_assert_eq!(&rope_inc, &str_inc,
            "\nrope path != str path\nold={:?}\nnew={:?}", old, new);
    }};
}

/// Assert str-incremental == rope-incremental == full_parse at every step of a
/// multi-edit chain. Carries BOTH spliced trees forward — str_tree feeds the
/// next str update, rope_tree feeds the next rope update — so we prove that a
/// spliced rope tree is valid input to the next incremental_update_rope call.
///
/// NOTE: contains `prop_assert_eq!` — must be called from a `proptest!` closure.
macro_rules! assert_chain_paths_agree {
    ($initial:expr, $edits:expr) => {{
        let initial: &str = $initial;
        let edits: &[(wordcartel_core::block_tree::Edit, String)] = $edits;
        let mut text = initial.to_string();
        let mut str_tree = full_parse(initial);
        let mut rope_tree = full_parse_rope(&Rope::from_str(initial));
        for (edit, new_text) in edits {
            let new_str_tree = incremental_update(&str_tree, &text, edit, new_text);
            let new_rope_tree = incremental_update_rope(
                &rope_tree,
                &Rope::from_str(&text),
                edit,
                &Rope::from_str(new_text),
            );
            let full = full_parse(new_text);
            prop_assert_eq!(&new_str_tree, &full,
                "\nchain: str path != full_parse\nbefore={:?}\nafter={:?}", text, new_text);
            prop_assert_eq!(&new_rope_tree, &full,
                "\nchain: rope path != full_parse\nbefore={:?}\nafter={:?}", text, new_text);
            prop_assert_eq!(&new_rope_tree, &new_str_tree,
                "\nchain: rope path != str path\nbefore={:?}\nafter={:?}", text, new_text);
            str_tree = new_str_tree;
            rope_tree = new_rope_tree;
            text = new_text.clone();
        }
    }};
}

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

        let (new_text, edit) = apply_edit(&doc, range, &rep);
        // Assert str path == rope path == full_parse (the Task-5 gate).
        assert_all_paths_agree!(&doc, &edit, &new_text);
    }

    /// Test A — multi-edit chain.
    ///
    /// Proves that the spliced BlockTree produced by `incremental_update` is a
    /// valid input to a subsequent `incremental_update` call: at each step in
    /// the chain we assert `incremental == full_parse`. Both the str tree and
    /// the rope tree are carried forward so the rope path is also gated.
    #[test]
    fn oracle_multi_edit_chain(
        initial in doc_strategy(),
        edits in edit_seq_strategy(),
    ) {
        let edits: Vec<(wordcartel_core::block_tree::Edit, String)> = {
            let mut text = initial.clone();
            let mut out = Vec::new();
            for (pa, pb, rep) in edits {
                let lo = snap(&text, pa.min(pb));
                let hi = snap(&text, pa.max(pb));
                let (new_text, edit) = apply_edit(&text, lo..hi, &rep);
                out.push((edit, new_text.clone()));
                text = new_text;
            }
            out
        };
        assert_chain_paths_agree!(&initial, &edits);
    }

    /// Test B — multibyte corpus.
    ///
    /// Proves the byte-offset / line_start / line_end arithmetic is UTF-8-safe
    /// when documents and replacements contain multibyte graphemes (é/中/🙂).
    /// Also asserts the rope path agrees with the str path and full_parse.
    #[test]
    fn oracle_multibyte_corpus((doc, (pa, pb), rep) in mb_doc_and_edit_strategy()) {
        let lo = snap(&doc, pa.min(pb));
        let hi = snap(&doc, pa.max(pb));
        let (new_text, edit) = apply_edit(&doc, lo..hi, &rep);
        assert_all_paths_agree!(&doc, &edit, &new_text);
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
    /// boundaries. Both the str tree and the rope tree are carried forward so
    /// the rope path is also gated at every step.
    #[test]
    fn oracle_mb_multi_edit_chain(
        initial in mb_doc_strategy(),
        edits in prop::collection::vec(
            (0u32..1000, 0u32..1000, mb_replacement_strategy()),
            1..=8,
        ),
    ) {
        let edits: Vec<(wordcartel_core::block_tree::Edit, String)> = {
            let mut text = initial.clone();
            let mut out = Vec::new();
            for (pa, pb, rep) in edits {
                let pa = pa as usize;
                let pb = pb as usize;
                let lo = snap(&text, pa.min(pb));
                let hi = snap(&text, pa.max(pb));
                let (new_text, edit) = apply_edit(&text, lo..hi, &rep);
                out.push((edit, new_text.clone()));
                text = new_text;
            }
            out
        };
        assert_chain_paths_agree!(&initial, &edits);
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
    // Also asserts the rope path agrees (Task-5 gate).
    assert_all_paths_agree_det(text1, &edit2, &text2);

    // Chained form: replay edit1 first, then edit2.
    // edit1: replace initial[36..38] ("on") with "[r]: http://y.test\n"
    let initial = "    indented code\n    more\n\n- a\n\n  cont\n- b\n  - nested\n\n---\n\n[ref]: http://x.test\n\nuse [ref] here\n\naaaa\naaaaaa\n";
    let (text1_check, edit1) = apply_edit(initial, 36..38, "[r]: http://y.test\n");
    assert_eq!(text1_check, text1, "edit1 must reconstruct text1");
    let full2 = full_parse(&text2);
    let t0 = full_parse(initial);
    let t1_inc = incremental_update(&t0, initial, &edit1, text1);
    let inc_chain = incremental_update(&t1_inc, text1, &edit2, &text2);
    assert_eq!(
        inc_chain, full2,
        "\nregression: chained incremental != full_parse\ntext2={text2:?}\nincremental={inc_chain:#?}\nfull={full2:#?}"
    );
    // Also assert the rope chain agrees.
    let rope_t0 = full_parse_rope(&Rope::from_str(initial));
    let rope_t1 = incremental_update_rope(&rope_t0, &Rope::from_str(initial), &edit1, &Rope::from_str(text1));
    let rope_chain = incremental_update_rope(&rope_t1, &Rope::from_str(text1), &edit2, &Rope::from_str(&text2));
    assert_eq!(
        rope_chain, full2,
        "\nregression: rope chained incremental != full_parse\ntext2={text2:?}\nrope={rope_chain:#?}\nfull={full2:#?}"
    );
}

// ---------------------------------------------------------------------------
// CE1 / CE2 — forward/downstream container merge (pinned regressions)
// ---------------------------------------------------------------------------

/// Deterministic (non-proptest) version of the three-way assertion:
/// str-incremental == rope-incremental == full_parse.
fn assert_all_paths_agree_det(old_text: &str, edit: &wordcartel_core::block_tree::Edit, new_text: &str) {
    let ot = full_parse(old_text);
    let full = full_parse(new_text);
    let str_inc = incremental_update(&ot, old_text, edit, new_text);
    let rope_inc = incremental_update_rope(
        &ot,
        &Rope::from_str(old_text),
        edit,
        &Rope::from_str(new_text),
    );
    assert_eq!(
        str_inc, full,
        "\nstr path != full_parse\nold={old_text:?}\nnew={new_text:?}\nstr={str_inc:#?}\nfull={full:#?}"
    );
    assert_eq!(
        rope_inc, full,
        "\nrope path != full_parse\nold={old_text:?}\nnew={new_text:?}\nrope={rope_inc:#?}\nfull={full:#?}"
    );
    assert_eq!(
        rope_inc, str_inc,
        "\nrope path != str path\nold={old_text:?}\nnew={new_text:?}"
    );
}

/// CE1: editing the first table (bytes 21..30 -> "> ") produces a new region
/// that ends exactly at the start of the second table. The second table would
/// merge with the reparsed content under full_parse (GFM table greedily absorbs
/// following pipe-rows with no blank line separator), but the incremental path
/// shifts it verbatim → stale split. Fix: widen-to-full when the block
/// immediately following the region (or the slack block) is a container.
#[test]
fn regression_ce1_downstream_table_merge() {
    let old_text = "| a | b |\n|---|---|\n| 1 | 2 |\n\n| a | b |\n|[r]: http://y.test\n---|\n| 1 | 2 |\n\naa\n---\n";
    let (new_text, edit) = apply_edit(old_text, 21..30, "> ");
    assert_all_paths_agree_det(old_text, &edit, &new_text);
}

/// CE2: deleting "# 中- " (bytes 0..5) turns the heading line into "- a",
/// a list bullet that should merge with the following List block. The
/// incremental path places region_old_end exactly at the following list's
/// span.start and shifts it verbatim → two separate lists instead of one.
#[test]
fn regression_ce2_downstream_list_merge() {
    let old_text = "# 中- a\n\n  cont\n- b\n  - nested\n";
    let (new_text, edit) = apply_edit(old_text, 0..5, "");
    assert_all_paths_agree_det(old_text, &edit, &new_text);
}

// ---------------------------------------------------------------------------
// Separator-byte deterministic regressions (Task-5 must-fix)
//
// The random corpus never emits CR, FF, LS, or PS, so without these explicit
// tests the str==rope assertion is untested for the exact bytes where ropey's
// Unicode-line APIs would diverge from \n-only semantics. Each test applies
// an edit near a non-LF separator and asserts:
//   incremental_update_rope == full_parse_rope == incremental_update == full_parse
// This PINS that the rope TextSource impl treats ONLY \n as a line break.
// ---------------------------------------------------------------------------

/// Pin: \r (carriage return) — NOT a line separator in our semantics.
#[test]
fn separator_cr_is_not_a_line_break() {
    let old = "a\rb\nc";
    // Insert a char adjacent to the \r (byte offset 1).
    let (new, edit) = apply_edit(old, 1..1, "X");
    assert_all_paths_agree_det(old, &edit, &new);
    // Also verify the rope full_parse agrees.
    assert_eq!(
        full_parse_rope(&Rope::from_str(&new)),
        full_parse(&new),
        "full_parse_rope != full_parse for {:?}", new
    );
}

/// Pin: \r\n (CRLF) — the \r must NOT be treated as a line break.
#[test]
fn separator_crlf_only_lf_is_line_break() {
    let old = "a\r\nb\nc";
    // Insert adjacent to the \r (byte offset 1).
    let (new, edit) = apply_edit(old, 1..1, "X");
    assert_all_paths_agree_det(old, &edit, &new);
    assert_eq!(
        full_parse_rope(&Rope::from_str(&new)),
        full_parse(&new),
        "full_parse_rope != full_parse for {:?}", new
    );
}

/// Pin: \x0c (form feed / FF) — ropey unicode_lines treats this as a break;
/// our impl must NOT.
#[test]
fn separator_ff_is_not_a_line_break() {
    let old = "x\x0cy\nz";
    // Insert adjacent to the FF (byte offset 1).
    let (new, edit) = apply_edit(old, 1..1, "X");
    assert_all_paths_agree_det(old, &edit, &new);
    assert_eq!(
        full_parse_rope(&Rope::from_str(&new)),
        full_parse(&new),
        "full_parse_rope != full_parse for {:?}", new
    );
}

/// Pin: \x0b (vertical tab / VT) — ropey unicode_lines treats this as a break;
/// our impl must NOT.
#[test]
fn separator_vt_is_not_a_line_break() {
    let old = "x\x0by\nz";
    // Insert adjacent to the VT (byte offset 1).
    let (new, edit) = apply_edit(old, 1..1, "X");
    assert_all_paths_agree_det(old, &edit, &new);
    assert_eq!(
        full_parse_rope(&Rope::from_str(&new)),
        full_parse(&new),
        "full_parse_rope != full_parse for {:?}", new
    );
}

/// Pin: U+0085 (NEL / next line) — ropey unicode_lines treats this as a break;
/// our impl must NOT.
#[test]
fn separator_nel_is_not_a_line_break() {
    let old = "p\u{0085}q\nr";
    // Insert adjacent to the NEL (byte offset 1, before the 2-byte U+0085).
    let (new, edit) = apply_edit(old, 1..1, "X");
    assert_all_paths_agree_det(old, &edit, &new);
    assert_eq!(
        full_parse_rope(&Rope::from_str(&new)),
        full_parse(&new),
        "full_parse_rope != full_parse for {:?}", new
    );
}

/// Pin: U+2028 (LINE SEPARATOR) — ropey unicode_lines treats this as a break;
/// our impl must NOT.
#[test]
fn separator_ls_is_not_a_line_break() {
    let old = "p\u{2028}q\nr";
    // Insert adjacent to the LS (byte offset 1, before the 3-byte U+2028).
    let (new, edit) = apply_edit(old, 1..1, "X");
    assert_all_paths_agree_det(old, &edit, &new);
    assert_eq!(
        full_parse_rope(&Rope::from_str(&new)),
        full_parse(&new),
        "full_parse_rope != full_parse for {:?}", new
    );
}

/// Pin: U+2029 (PARAGRAPH SEPARATOR) — ropey unicode_lines treats this as a
/// break; our impl must NOT.
#[test]
fn separator_ps_is_not_a_line_break() {
    let old = "p\u{2029}q\nr";
    // Insert adjacent to the PS (byte offset 1, before the 3-byte U+2029).
    let (new, edit) = apply_edit(old, 1..1, "X");
    assert_all_paths_agree_det(old, &edit, &new);
    assert_eq!(
        full_parse_rope(&Rope::from_str(&new)),
        full_parse(&new),
        "full_parse_rope != full_parse for {:?}", new
    );
}
