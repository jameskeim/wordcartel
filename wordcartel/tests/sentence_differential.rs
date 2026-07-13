//! S5 differential suite — pins our detector's relationship to repar's ventilate.
//! Equality corpus asserts identical word GROUPINGS (both merge OR both break); the
//! ledger asserts KNOWN divergences with a reason so a vanishing divergence also fails.
//! repar is driven through the shell's own ventilate wrapper — the product's ventilate.

use wordcartel::transform::{run_transform, TransformKind};
use wordcartel_core::textobj::sentence_spans;

const W: u32 = 72; // ventilate is width-agnostic (driver.rs:209); any width is inert.

/// Word groups from our detector: each sentence span → its whitespace words, with
/// `markers` tokens (e.g. `">"`, `"-"`) filtered out on both sides.
fn our_groups(para: &str, markers: &[&str]) -> Vec<Vec<String>> {
    sentence_spans(para)
        .map(|(f, t)| para[f..t].split_whitespace()
            .filter(|w| !markers.contains(w))
            .map(str::to_string).collect())
        .collect()
}
/// Word groups from repar ventilate: each non-marker-only output line → its words.
fn repar_groups(para: &str, markers: &[&str]) -> Vec<Vec<String>> {
    let out = run_transform(TransformKind::Ventilate, para, W).expect("ventilate");
    out.lines()
        .filter(|l| !l.split_whitespace().all(|w| markers.contains(&w)))
        .map(|l| l.split_whitespace()
            .filter(|w| !markers.contains(w))
            .map(str::to_string).collect())
        .collect()
}

#[test]
fn equality_corpus_agrees_with_ventilate() {
    // (input, markers) — each verified against live repar ventilate 2026-07-12.
    let cases: &[(&str, &[&str])] = &[
        ("Dr. Smith arrived. He was late.", &[]),
        ("See fig. 2 for details. Then leave.", &[]),
        ("Kramer vs. Wade was long. He read it.", &[]),
        ("See cf. Smith 2001. He agreed.", &[]),
        ("Smith et al. Wrote it.", &[]),
        ("St. Louis is big. He knows.", &[]),
        ("J. R. R. Tolkien wrote it. He was English.", &[]),
        ("Q.E.D. Next problem.", &[]),                    // both BREAK
        ("We met at 10 a.m. and left.", &[]),             // both one group
        ("The U.S.A. is large. It grew.", &[]),
        // The R2 proof: same content, hard-wrapped, must group identically.
        ("The committee met on Tuesday and the chair insisted on a vote. Then we left.", &[]),
        ("The committee met on Tuesday and the\nchair insisted on a vote. Then we left.", &[]),
        // Blockquote + list-item prefix re-emission (markers filtered both sides).
        ("> The committee met and the\n> chair voted. Then we left.", &[">"]),
        ("- Buy milk. Then rest.", &["-"]),
        // Multibyte with ASCII terminators.
        ("été fini. Then done.", &[]),
    ];
    for (input, markers) in cases {
        assert_eq!(our_groups(input, markers), repar_groups(input, markers),
            "equality corpus mismatch for {input:?}");
    }
}

#[test]
fn divergence_ledger() {
    // Each entry: ours ≠ repar, with a reason. assert_ne so a vanishing divergence fails.
    // L1 — the colon: repar terminal_chars default ".?!:" (options.rs:151); UAX has no ':'.
    assert_ne!(our_groups("Note: This is fine.", &[]), repar_groups("Note: This is fine.", &[]),
        "L1 colon: repar breaks at ':' (terminal_chars \".?!:\"); UAX-29 does not");
    // L2 — ideographic full stop: ours breaks at 。, repar's terminal set is ASCII.
    assert_ne!(our_groups("中文。Then done.", &[]), repar_groups("中文。Then done.", &[]),
        "L2 CJK: ours honors 。 as a terminator; repar's ASCII terminal set does not");
    // L3 — name prefixes mt/ft: ours merges (class 1), repar's list lacks them → breaks.
    assert_ne!(our_groups("I saw Mt. Fuji. It was tall.", &[]),
        repar_groups("I saw Mt. Fuji. It was tall.", &[]),
        "L3 mt/ft: ours R1-merges name prefixes repar's stop-list lacks");
    // L4 — class-2 suffix / dropped 'no': ours breaks on the capital, repar always merges.
    assert_ne!(our_groups("Acme Co. Then he quit.", &[]),
        repar_groups("Acme Co. Then he quit.", &[]),
        "L4 class-2: ours breaks after 'Co.'+capital; repar's flat stop-list merges");
    assert_ne!(our_groups("The answer was no. Then we left.", &[]),
        repar_groups("The answer was no. Then we left.", &[]),
        "L4 dropped 'no': ours breaks; repar merges (its most damaging entry)");
    // L5 — Markdown hard break: ours keeps 2 sentences, ventilate collapses to one line.
    assert_ne!(our_groups("Roses are red,  \nViolets are blue.", &[]),
        repar_groups("Roses are red,  \nViolets are blue.", &[]),
        "L5 hard break: ours preserves the authored break (R2 exception); ventilate cannot");
}

// ── §6.5 second differential corpus: pragmatic_segmenter English "Golden Rules" ──
// Characterizes our intentionally-small detector against a full external segmenter.
// Equality set = rules R1–R4 reproduce; divergence ledger = rules we knowingly miss,
// each with a governing-decision reason. Empirically partitioned 2026-07-12 (34/18).
// GR = golden-rule number. Do NOT grow ABBREV_ALWAYS_MERGE to pass one; record reality.

fn golden(expected: &[&str]) -> Vec<Vec<String>> {
    expected.iter()
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .collect()
}

#[test]
fn golden_rules_equality() {
    // (GR, input, expected split) — our_groups(input, &[]) == golden(expected).
    let cases: &[(&str, &[&str])] = &[
        /* 1  */ ("Hello World. My name is Jonas.", &["Hello World.", "My name is Jonas."]),
        /* 2  */ ("What is your name? My name is Jonas.", &["What is your name?", "My name is Jonas."]),
        /* 3  */ ("There it is! I found it.", &["There it is!", "I found it."]),
        /* 4  */ ("My name is Jonas E. Smith.", &["My name is Jonas E. Smith."]),
        /* 6  */ ("Were Jane and co. at the party?", &["Were Jane and co. at the party?"]),
        /* 7  */ ("They closed the deal with Pitt, Briggs & Co. at noon.", &["They closed the deal with Pitt, Briggs & Co. at noon."]),
        /* 8  */ ("Let's ask Jane and co. They should know.", &["Let's ask Jane and co.", "They should know."]),
        /* 9  */ ("They closed the deal with Pitt, Briggs & Co. It closed yesterday.", &["They closed the deal with Pitt, Briggs & Co.", "It closed yesterday."]),
        /* 10 */ ("I can see Mt. Fuji from here.", &["I can see Mt. Fuji from here."]),
        /* 11 */ ("St. Michael's Church is on 5th st. near the light.", &["St. Michael's Church is on 5th st. near the light."]),
        /* 12 */ ("That is JFK Jr.'s book.", &["That is JFK Jr.'s book."]),
        /* 13 */ ("I visited the U.S.A. last year.", &["I visited the U.S.A. last year."]),
        /* 14 */ ("I live in the E.U. How about you?", &["I live in the E.U.", "How about you?"]),
        /* 15 */ ("I live in the U.S. How about you?", &["I live in the U.S.", "How about you?"]),
        /* 17 */ ("I have lived in the U.S. for 20 years.", &["I have lived in the U.S. for 20 years."]),
        /* 19 */ ("She has $100.00 in her bag.", &["She has $100.00 in her bag."]),
        /* 20 */ ("She has $100.00. It is in her bag.", &["She has $100.00.", "It is in her bag."]),
        /* 21 */ ("He teaches science (He previously worked for 5 years as an engineer.) at the local University.", &["He teaches science (He previously worked for 5 years as an engineer.) at the local University."]),
        /* 22 */ ("Her email is Jane.Doe@example.com. I sent her an email.", &["Her email is Jane.Doe@example.com.", "I sent her an email."]),
        /* 23 */ ("The site is: https://www.example.50.com/new-site/awesome_content.html. Please check it out.", &["The site is: https://www.example.50.com/new-site/awesome_content.html.", "Please check it out."]),
        /* 24 */ ("She turned to him, 'This is great.' she said.", &["She turned to him, 'This is great.' she said."]),
        /* 25 */ ("She turned to him, \"This is great.\" she said.", &["She turned to him, \"This is great.\" she said."]),
        /* 26 */ ("She turned to him, \"This is great.\" She held the book out to show him.", &["She turned to him, \"This is great.\"", "She held the book out to show him."]),
        /* 27 */ ("Hello!! Long time no see.", &["Hello!!", "Long time no see."]),
        /* 28 */ ("Hello?? Who is there?", &["Hello??", "Who is there?"]),
        /* 29 */ ("Hello!? Is that you?", &["Hello!?", "Is that you?"]),
        /* 30 */ ("Hello?! Is that you?", &["Hello?!", "Is that you?"]),
        /* 34 */ ("1) The first item. 2) The second item.", &["1) The first item.", "2) The second item."]),
        /* 40 */ ("This is a sentence\ncut off in the middle because pdf.", &["This is a sentence\ncut off in the middle because pdf."]),
        /* 41 */ ("It was a cold \nnight in the city.", &["It was a cold \nnight in the city."]),
        /* 44 */ ("She works at Yahoo! in the accounting department.", &["She works at Yahoo! in the accounting department."]),
        /* 46 */ ("Thoreau argues that by simplifying one's life, \"the laws of the universe will appear less complex. . . .\"", &["Thoreau argues that by simplifying one's life, \"the laws of the universe will appear less complex. . . .\""]),
        /* 48 */ ("If words are left off at the end of a sentence, and that is all that is omitted, indicate the omission with ellipsis marks (preceded and followed by a space) and then indicate the end of the sentence with a period . . . . Next sentence.", &["If words are left off at the end of a sentence, and that is all that is omitted, indicate the omission with ellipsis marks (preceded and followed by a space) and then indicate the end of the sentence with a period . . . .", "Next sentence."]),
        /* 49 */ ("I never meant that.... She left the store.", &["I never meant that....", "She left the store."]),
    ];
    for (input, expected) in cases {
        assert_eq!(our_groups(input, &[]), golden(expected), "golden equality mismatch: {input:?}");
    }
}

#[test]
fn golden_rules_accepted_divergences() {
    // (input, golden-expected split, reason). assert_ne — a vanishing divergence fails.
    let cases: &[(&str, &[&str], &str)] = &[
        // §11 out-of-scope — numbered / bulleted / alpha lists (no list-marker model)
        ("1.) The first item 2.) The second item", &["1.) The first item", "2.) The second item"], "GR31 §11 list markers"),
        ("1.) The first item. 2.) The second item.", &["1.) The first item.", "2.) The second item."], "GR32 §11 list markers"),
        ("1) The first item 2) The second item", &["1) The first item", "2) The second item"], "GR33 §11 list markers (UAX under-splits)"),
        ("1. The first item 2. The second item", &["1. The first item", "2. The second item"], "GR35 §11 list markers"),
        ("1. The first item. 2. The second item.", &["1. The first item.", "2. The second item."], "GR36 §11 list markers"),
        ("• 9. The first item • 10. The second item", &["• 9. The first item", "• 10. The second item"], "GR37 §11 bullet list"),
        ("⁃9. The first item ⁃10. The second item", &["⁃9. The first item", "⁃10. The second item"], "GR38 §11 hyphen-bullet list"),
        ("a. The first item b. The second item c. The third list item", &["a. The first item", "b. The second item", "c. The third list item"], "GR39 §11 alpha list"),
        // §11 out-of-scope — other edge forms
        ("Please turn to p. 55.", &["Please turn to p. 55."], "GR5 §11 single-lowercase citation abbr not in the frozen §5 list"),
        ("At 5 a.m. Mr. Smith went to the bank. He left the bank at 6 P.M. Mr. Smith then went to the store.", &["At 5 a.m. Mr. Smith went to the bank.", "He left the bank at 6 P.M.", "Mr. Smith then went to the store."], "GR18 §11 a.m./p.m. time abbreviation (interior-dot + capital follow)"),
        ("You can find it at N°. 1026.253.553. That is where the treasure is.", &["You can find it at N°. 1026.253.553.", "That is where the treasure is."], "GR43 §11 geo-coordinates"),
        ("\"Bohr [...] used the analogy of parallel stairways [...]\" (Smith 55).", &["\"Bohr [...] used the analogy of parallel stairways [...]\" (Smith 55)."], "GR47 §11 parenthetical citation after quotation (markup-blind, §10 R3 note)"),
        ("I wasn't really ... well, what I mean...see . . . what I'm saying, the thing is . . . I didn't mean it.", &["I wasn't really ... well, what I mean...see . . . what I'm saying, the thing is . . . I didn't mean it."], "GR50 §11 spaced-ellipsis edge form"),
        ("One further habit which was somewhat weakened . . . was that of combining words into self-interpreting compounds. . . . The practice was not abandoned. . . .", &["One further habit which was somewhat weakened . . . was that of combining words into self-interpreting compounds.", ". . . The practice was not abandoned. . . ."], "GR51 §11 4-dot ellipsis grouping edge form"),
        ("Hello world.Today is Tuesday.Mr. Smith went to the store and bought 1,000.That is a lot.", &["Hello world.", "Today is Tuesday.", "Mr. Smith went to the store and bought 1,000.", "That is a lot."], "GR52 §11 no whitespace between sentences (UAX SB needs whitespace)"),
        // §10 residue — grammar ambiguity, S7 POS resolves
        ("I work for the U.S. Government in Virginia.", &["I work for the U.S. Government in Virginia."], "GR16 §10 abbrev + capitalized proper noun (the St. Louis ambiguity)"),
        ("We make a good team, you and I. Did you see Albert I. Jones yesterday?", &["We make a good team, you and I.", "Did you see Albert I. Jones yesterday?"], "GR45 §10 'I.' pronoun-vs-initial (single-capital rule merges; §4.4 cost)"),
        // R2-by-design — the DOMINANT reflow hard-wrap merge deliberately opposes the golden rule
        ("features\ncontact manager\nevents, activities\n", &["features", "contact manager", "events, activities"], "GR42 R2 by design: newline-separated unterminated fragments merge (§1/§4 reflow fix)"),
    ];
    for (input, expected, reason) in cases {
        assert_ne!(our_groups(input, &[]), golden(expected), "golden divergence vanished — {reason}: {input:?}");
    }
}
