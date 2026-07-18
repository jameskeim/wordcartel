//! Pure prose-lens classifier (S8): maps a tagged sentence stream to stylistic matches.
//! No theme, no shell types — the shell maps `ProseLensCategory` to a `SemanticElement`.
use crate::{TaggedSentence, UPOS};

/// The four stylistic prose lenses S8 supports.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ProseLensCategory {
    Adverbs,
    Adjectives,
    Passive,
    Weak,
}

/// One classified match: a byte range in the analyzed text's coordinates and the lens category
/// it belongs to. Output order is NOT guaranteed sorted by `range.start` — the shell sorts
/// per-category after splitting.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClassifiedMatch {
    pub range: std::ops::Range<usize>,
    pub category: ProseLensCategory,
}

/// Surface forms of "to be" (ASCII-lowercased compare). Triggers passive/weak by SURFACE, not the
/// AUX tag — Brill retags existential be to VERB (probe: "There are/VERB three problems.").
const BE_FORMS: [&str; 8] = ["be", "am", "is", "are", "was", "were", "been", "being"];

/// Skipped while scanning from a be-form to its target: inserted adverbs, negation/infinitival "to"
/// (PART), auxiliary chains (AUX — also consumes intermediate be/been/being so a chain fires once).
fn is_skip(u: Option<UPOS>) -> bool { matches!(u, Some(UPOS::ADV) | Some(UPOS::PART) | Some(UPOS::AUX)) }

/// ~150 irregular past participles whose surface does NOT end in -ed and that the dict maps to VERB.
/// SORTED — membership is binary-searched. Deliberately keeps base-identical irregulars (run/come/
/// put/set/cut/read/let/hit): favors recall on real prose over the rare pseudo-cleft FP.
const IRREGULAR_PARTICIPLES: &[&str] = &[
    "begun", "bent", "bled", "born", "borne", "bought", "bound", "bred", "brought", "built", "burnt",
    "cast", "caught", "chosen", "clung", "come", "crept", "cut", "dealt", "done", "drawn", "driven",
    "drunk", "dug", "dwelt", "eaten", "fallen", "fed", "felt", "fled", "flung", "forbidden", "forgiven",
    "forgotten", "forsaken", "fought", "found", "frozen", "given", "gone", "ground", "grown", "heard",
    "held", "hidden", "hit", "hung", "hurt", "kept", "knelt", "known", "laid", "lain", "leapt", "learnt",
    "led", "left", "lent", "let", "lit", "lost", "made", "meant", "met", "mistaken", "overcome",
    "overtaken", "paid", "proven", "put", "quit", "read", "ridden", "risen", "run", "rung", "said", "sat",
    "seen", "sent", "set", "shaken", "shed", "shone", "shot", "shown", "shrunk", "shut", "slain", "slept",
    "slid", "slit", "smelt", "sold", "sought", "spat", "sped", "spelt", "spent", "spilt", "split",
    "spoken", "spread", "sprung", "spun", "stolen", "stood", "stridden", "strove", "struck", "strung",
    "stuck", "stung", "stunk", "sung", "sunk", "swept", "sworn", "swum", "swung", "taken", "taught",
    "thought", "thrown", "thrust", "told", "torn", "trodden", "understood", "undertaken", "upheld",
    "upset", "wept", "withdrawn", "withheld", "withstood", "woken", "won", "worn", "wound", "woven",
    "written", "wrung",
];

/// Is `surface` a plausible past-participle surface form (regular `-ed` OR in the irregular list)?
/// ASCII-lowercased before both checks (`IRREGULAR_PARTICIPLES` is sorted lowercase — the
/// `binary_search` precondition).
fn is_participle_surface(surface: &str) -> bool {
    let lower = surface.to_ascii_lowercase();
    lower.ends_with("ed") || IRREGULAR_PARTICIPLES.binary_search(&lower.as_str()).is_ok()
}

/// Classify a tagged sentence stream into stylistic prose-lens matches.
///
/// Two independent passes fold into one output vec, per token:
/// - **Adverbs/Adjectives**: every token tagged `ADV`/`ADJ` flags directly (flag-all, always
///   fires — never suppressed by the be-chain de-dup below).
/// - **Passive/Weak**: a forward scan from each be-form (matched by SURFACE, not the `AUX` tag —
///   existential "there is/are" retags to `VERB`) over the skip set `{ADV, PART, AUX}` to a
///   target token, classified per the target-classification table (see module docs / spec
///   §5.1.2). A `min_trigger` high-water mark de-dups aux chains ("was being analyzed" fires
///   once) while still letting a genuinely separate later be-form fire.
///
/// Ranges are in `text`'s byte coordinates. Output order is NOT guaranteed sorted by
/// `range.start`.
///
/// # Examples
/// ```
/// use wordcartel_nlp::{analyze, classify, ProseLensCategory};
/// let t = "The report was written by the committee.";
/// let sentences = analyze(t);
/// let matches = classify(&sentences, t);
/// assert!(matches.iter().any(|m| m.category == ProseLensCategory::Passive
///     && &t[m.range.clone()] == "was written"));
/// ```
pub fn classify(sentences: &[TaggedSentence], text: &str) -> Vec<ClassifiedMatch> {
    let mut out = Vec::new();
    for s in sentences {
        let toks = &s.tokens;
        // `min_trigger` is the aux-chain de-dup high-water mark (spec §5.1.3): a be-form at index
        // `i < min_trigger` was already CONSUMED as a skip token by an earlier trigger's forward scan,
        // so it must not re-trigger. The outer loop still VISITS every token (direct-tag ADV/ADJ
        // emission is per-token and must fire even for consumed skip tokens, e.g. "quickly" in
        // "was quickly closed"), but the be-trigger is gated on `i >= min_trigger`.
        let mut min_trigger = 0usize;
        for (i, tok) in toks.iter().enumerate() {
            let surface = &text[tok.range.clone()];
            // Direct-tag flag-all lenses — ALWAYS per token (never suppressed by chain de-dup).
            match tok.upos {
                Some(UPOS::ADV) => out.push(ClassifiedMatch { range: tok.range.clone(), category: ProseLensCategory::Adverbs }),
                Some(UPOS::ADJ) => out.push(ClassifiedMatch { range: tok.range.clone(), category: ProseLensCategory::Adjectives }),
                _ => {}
            }
            // Be-form trigger by SURFACE (not AUX tag) — and only if not already consumed by a chain.
            if i < min_trigger { continue; }
            if !BE_FORMS.contains(&surface.to_ascii_lowercase().as_str()) { continue; }
            // Forward scan over the skip set {ADV, PART, AUX} to the target.
            let mut j = i + 1;
            while j < toks.len() && is_skip(toks[j].upos) { j += 1; }
            // CRITICAL-1 de-dup: everything the scan passed (up to and including `j`) is consumed —
            // any intermediate been/being at index < j must not re-trigger. Advance the high-water mark.
            // (`j` may equal toks.len() when no target remains — that is the no-target case below.)
            min_trigger = j.max(i + 1);
            // CRITICAL-2: distinguish "no target remains before sentence end" (→ NONE, terminal-be,
            // D-passive-edge (ii)) from "a real target token exists" (→ the §5.1.2 target table).
            let Some(target) = toks.get(j) else {
                continue; // no non-skipped token remains → SILENT (terminal be, e.g. "…been?")
            };
            let tsurface = &text[target.range.clone()];
            let tlower = tsurface.to_ascii_lowercase();
            let first_lower = tsurface.chars().next().is_some_and(|c| c.is_lowercase());
            // The §5.1.2 target table → an Option<category> (None = SILENT).
            let cat: Option<ProseLensCategory> = match target.upos {
                Some(UPOS::VERB) if tlower.ends_with("ing") => None,               // progressive → none
                Some(UPOS::VERB) if is_participle_surface(tsurface) => Some(ProseLensCategory::Passive),
                Some(UPOS::VERB) => None,                                          // base-form pseudo-cleft → none
                // Dict-gap recovery (D-passive-edge (i)): unknown lowercase + participle morphology → passive.
                None if first_lower && is_participle_surface(tsurface) => Some(ProseLensCategory::Passive),
                None => None,                                                     // unknown, non-participle → SILENT (conservative)
                Some(UPOS::PUNCT) => None,                                        // terminal be before punctuation → none
                Some(_) => Some(ProseLensCategory::Weak),                         // ADJ/NOUN/PROPN/PRON/DET/NUM/ADP/… copular → weak
            };
            match cat {
                Some(ProseLensCategory::Passive) => out.push(ClassifiedMatch {
                    range: tok.range.start..target.range.end,                     // whole be..participle span (edge (iii))
                    category: ProseLensCategory::Passive,
                }),
                Some(ProseLensCategory::Weak) => out.push(ClassifiedMatch {
                    range: tok.range.clone(),                                     // the be token only
                    category: ProseLensCategory::Weak,
                }),
                _ => {} // SILENT (none) or the unreachable ADV/ADJ direct-tag categories
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze;
    use ProseLensCategory::*;

    // Helper: analyze real text, classify, and collect (surface, category) pairs for readable asserts.
    fn run(t: &str) -> Vec<(String, ProseLensCategory)> {
        let s = analyze(t);
        classify(&s, t).into_iter()
            .map(|m| (t[m.range].to_string(), m.category))
            .collect()
    }
    fn has(t: &str, surface: &str, cat: ProseLensCategory) -> bool {
        run(t).iter().any(|(s, c)| s == surface && *c == cat)
    }
    fn none_passive(t: &str) -> bool { !run(t).iter().any(|(_, c)| *c == Passive) }
    fn none_weak(t: &str) -> bool { !run(t).iter().any(|(_, c)| *c == Weak) }

    // ---- PASSIVE (whole be..participle span) ----
    #[test] fn passive_canonical()   { assert!(has("The report was written by the committee.", "was written", Passive)); }
    #[test] fn passive_regular_ed()  { assert!(has("Their proposal was rejected by the board.", "was rejected", Passive)); }
    #[test] fn passive_irregular()   { assert!(has("Mistakes were made.", "were made", Passive)); }
    #[test] fn passive_adv_skip()    { assert!(has("The door was quickly closed.", "was quickly closed", Passive)); }
    #[test] fn passive_part_skip()   { assert!(has("The problem was not solved.", "was not solved", Passive)); }
    #[test] fn passive_never_skip()  { assert!(has("He was never seen again.", "was never seen", Passive)); }
    #[test] fn passive_aux_been()    { assert!(has("The book has been written.", "been written", Passive)); }
    // Both "were" and "being" tag AUX (harper probe); the chain fires ONCE, whole span
    // "were..analyzed" (matches aux_chain_fires_once below and the min_trigger reasoning).
    #[test] fn passive_being_prog()  { assert!(has("The results were being analyzed.", "were being analyzed", Passive)); }
    // D-passive-edge (i): None-tagged + lowercase + morphology → passive (dict-gap recovery).
    #[test] fn passive_none_ed()     { assert!(has("The item was defenestrated.", "was defenestrated", Passive)); }
    #[test] fn passive_none_irreg()  { assert!(has("The song was sung at dawn.", "was sung", Passive)); }

    // ---- NONE (progressive / pseudo-cleft / terminal / contracted) ----
    #[test] fn none_progressive()    { assert!(none_passive("She was running to the store.")); }
    #[test] fn none_progressive2()   { assert!(none_passive("They were writing letters.")); }
    #[test] fn none_pseudocleft()    { assert!(none_passive("All I did was call her.")); }
    // D-passive-edge (ii): terminal be before PUNCT/sentence-end → SILENT.
    #[test] fn none_terminal_be()    { assert!(none_passive("Where have you been?")); }
    // §5.1.5 documented miss: contracted be is one PRON token → invisible.
    #[test] fn none_contracted_be()  { assert!(none_passive("It's written by hand.") && none_weak("It's written by hand.")); }

    // ---- WEAK (the be token only) ----
    #[test] fn weak_adj()            { assert!(has("She was happy.", "was", Weak)); }
    #[test] fn weak_adjectival_ptc() { assert!(has("He was tired.", "was", Weak)); }   // tired/ADJ in dict
    #[test] fn weak_noun()           { assert!(has("He is a doctor.", "is", Weak)); }
    // existential surface-match: "are"/"is" tag VERB here (Brill patch), still trigger by surface.
    #[test] fn weak_existential_are(){ assert!(has("There are three problems.", "are", Weak)); }
    #[test] fn weak_existential_is() { assert!(has("There is a problem.", "is", Weak)); }
    #[test] fn weak_locative()       { assert!(has("It was in the room.", "was", Weak)); }
    #[test] fn weak_propn_not_pass() { assert!(has("It was Fred.", "was", Weak) && none_passive("It was Fred.")); }

    // ---- ADVERBS / ADJECTIVES (flag-all) ----
    #[test] fn adverbs_flag_all() {
        let r = run("The extremely tall man moved very quietly.");
        for w in ["extremely", "very", "quietly"] { assert!(r.iter().any(|(s,c)| s==w && *c==Adverbs), "{w}"); }
        assert!(r.iter().any(|(s,c)| s=="tall" && *c==Adjectives));
    }

    // ---- CRITICAL-1: aux chains emit EXACTLY ONE passive (no been/being re-trigger) ----
    #[test] fn aux_chain_fires_once() {
        // "were being analyzed": `were` triggers (target `analyzed`); `being` must NOT re-trigger.
        let t = "The results were being analyzed.";
        let ms: Vec<_> = classify(&analyze(t), t).into_iter().filter(|m| m.category==Passive).collect();
        assert_eq!(ms.len(), 1, "exactly one passive for the aux chain, got {ms:?}");
        // range is the whole span from the FIRST be to the participle (were..analyzed).
        assert_eq!(&t[ms[0].range.clone()], "were being analyzed");
        // "has been written": `been` triggers `written`; a second scan must not double-emit.
        let t2 = "The book has been written.";
        let ms2: Vec<_> = classify(&analyze(t2), t2).into_iter().filter(|m| m.category==Passive).collect();
        assert_eq!(ms2.len(), 1, "one passive for has-been-written, got {ms2:?}");
    }

    // ---- disjointness property: no be-occurrence yields both Passive and Weak ----
    #[test] fn passive_weak_disjoint() {
        for t in ["The report was written.", "She was happy.", "There are problems.",
                  "The results were being analyzed.", "He was never seen again.", "It was Fred."] {
            let ms = classify(&analyze(t), t);
            for a in &ms { for b in &ms {
                if a.range == b.range { assert_eq!(a.category, b.category, "double-flag at {:?} in {t:?}", a.range); }
                // passive spans and weak spans never share a be-start
            }}
            let pass: Vec<_> = ms.iter().filter(|m| m.category==Passive).collect();
            let weak: Vec<_> = ms.iter().filter(|m| m.category==Weak).collect();
            for p in &pass { for w in &weak { assert_ne!(p.range.start, w.range.start, "be double-classified in {t:?}"); } }
        }
    }

    // ---- multi-sentence sanity ----
    #[test] fn multi_sentence() {
        let t = "It was written. She was happy. He was running.";
        assert!(has(t, "was written", Passive));
        assert!(has(t, "was", Weak));
        assert!(none_passive("He was running."));
    }

    // ---- never panics on degenerate/multibyte input ----
    #[test] fn no_panic_degenerate() {
        for t in ["", "   ", "café", "🙂 was", "was", "is is is", "The café was frozen."] {
            let _ = classify(&analyze(t), t);
        }
    }

    // ---- CRITICAL: IRREGULAR_PARTICIPLES must stay sorted — binary_search's precondition ----
    #[test]
    fn irregular_participles_sorted() {
        assert!(IRREGULAR_PARTICIPLES.windows(2).all(|w| w[0] <= w[1]),
            "IRREGULAR_PARTICIPLES must be sorted for binary_search");
    }

    // Regression: these irregular participles were previously unreachable via binary_search
    // because the array was unsorted (silently wrong — no passive match at all).
    #[test] fn passive_irregular_wept()  { assert!(has("The tears were wept.", "were wept", Passive)); }
    #[test] fn passive_irregular_swept() { assert!(has("He was swept away by the crowd.", "was swept", Passive)); }

    // ---- MINOR: a genuinely separate later be-form in the same sentence is NOT wrongly
    // suppressed by min_trigger (already-correct behavior — previously untested) ----
    #[test] fn separate_be_forms_both_fire() {
        let t = "The door was closed and the window was opened.";
        assert!(has(t, "was closed", Passive));
        assert!(has(t, "was opened", Passive));
        let ms: Vec<_> = classify(&analyze(t), t).into_iter().filter(|m| m.category == Passive).collect();
        assert_eq!(ms.len(), 2, "two independent passive matches, got {ms:?}");
    }
}
