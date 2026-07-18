# S8 — Prose lenses — Implementation plan

Status: DRAFT (awaiting Codex plan gate)
Author: Fable
Date: 2026-07-17
Spec (authoritative): `docs/superpowers/specs/2026-07-17-s8-prose-lenses-design.md`
Discipline: `superpowers:writing-plans` — task-by-task TDD (failing test → run(fail) → impl → run(pass) → commit), per-task reviewer gate.

---

## Goal

Ship four single-active prose lenses (Adverbs, Adjectives, Passive, Weak) as a reusable ProseLens
spine: window-paint highlight + doc-wide navigate-and-range-select + a live doc-wide count for the
active lens, built on S7's `wordcartel_nlp::analyze`, riding the `jobs.rs` worker and `render.rs`
paint path. Cold-path only (zero work with no lens active). Select-only act model (arc D6).

## Architecture

A **parallel POS-typed pipeline modeled on `reconcile.rs`**, NOT a `DiagSource`/`DiagnosticsProvider`.
- Pure classifier in `wordcartel-nlp` (`classify` over `&[TaggedSentence]` + the buffer slice) →
  `Vec<(Range<usize>, ProseLensCategory)>`-shaped output.
- Shell leaf `wordcartel/src/lenses.rs`: `PosStore` (per-buffer), `PosMatch`, `ProseLensCategory`, the
  doc-wide sweep dispatcher (near-copy of `reconcile::dispatch_reconcile`), the single-active lens
  state accessor, commands (Rule-8), count segment, and the visible-window match helper.
- One new `SemanticElement::ProseLensMatch` in `wordcartel-core/src/theme.rs`.
- Worker plumbing: `JobKind::PosSweep`, `jobs_apply::apply_panic` arm, `timers::SUBSYSTEMS` row +
  `on_tick` dispatch, `app.rs advance()` arm.
- Paint: one field on `RowCtx`, `use_placed` flip in `gather_row_ctx`, one face-apply run in
  `row_spans_placed` BETWEEN Search and Diag.

## Tech stack

Rust workspace: `wordcartel-core` (pure, `#![forbid(unsafe_code)]`), `wordcartel-nlp` (pure, wraps
`harper-brill` 2.5.0), `wordcartel` (shell). No NEW crates — `harper-brill` is already in-tree from
S7. ratatui 0.30 / crossterm. Tests: `cargo test` per crate; merge gates = workspace clippy (deny),
`too_many_lines` (100), `module_budgets`.

---

## GLOBAL CONSTRAINTS (binding — copied from the spec verbatim; every task honors these)

**Reuse mandate.** Ride `jobs.rs` (dispatch/drain; results ride `JobOutcome::Done`, no new `Msg`) and
COPY `reconcile.rs` (`dispatch_reconcile` shape: snapshot rope, set in-flight, clear due, version-check
INSIDE the merge). POS matches stay OUT of `Diagnostic` / `DiagnosticKind` / `DiagnosticsProvider` —
those are reserved for ltex/vale. Do NOT extend `DiagSource`.

**The 6 grounding corrections (all mandatory):**
1. `PosStore` carries `armed_for_version` (anti-re-arm latch). Armed in `app.rs advance()` (the
   version-latched block at ~lines 391–418, beside reconcile/on_change), NOT in reduce.
2. `JobKind::PosSweep`'s panic arm is COMPILER-FORCED in the PRIVATE `jobs_apply::apply_panic`
   (exhaustive `match kind`, lines ~114–154), reached via `apply_outcome`. Mirror the `Reparse` arm:
   clear `in_flight_version` + leave `computed_for` behind (no retry-loop). `is_stale` gets a
   `PosSweep => false` arm.
3. `commands::set_selection_range` is PRIVATE (`commands.rs:302`, no `pub`). Do NOT try to call it;
   INLINE the 3-line idiom in the lens nav: `selection = Selection::range(to, from); derive::rebuild;
   nav::ensure_visible` (preceded by `unfold_ancestors_of`, like `diag_next`).
4. The sweep analyzes PROSE PARAGRAPHS ONLY: enumerate ranges where `BlockTree::role_at(content_byte)
   == BlockRole::Paragraph` from `document.blocks()` (reuse `ventilate::prose_block_at`); ship
   `Vec<(ps, pe)>` + the O(1) `buffer.snapshot()` rope to the worker; rebase spans `+ps`. Gate sweep
   dispatch on `!reconcile.maybe_stale` (tree converged) — in BOTH the timers deadline fn and the
   `on_tick` due predicate.
5. `computed_for: Option<u64>` — an EMPTY match set is MEANINGFUL ("0 passives"). A match is
   paintable/countable iff `computed_for == Some(document.version)`, regardless of emptiness. This
   DIVERGES from `SourceSlot::valid_for`'s non-empty-sentinel (`diagnostics_run.rs:19-21`). PosStore
   needs NO `maybe_stale` field — `computed_for != Some(version)` IS the staleness signal.
6. Cap `LENS_MAX_SWEEP_BYTES = 8 * 1024 * 1024` (M5-spirit; mirrors `DIAG_MAX_SEND_BYTES`,
   `limits.rs:21-27`). Beyond it: skip the sweep + one-time Sticky notice; count segment suppressed.

**Theme completeness tax.** Adding `SemanticElement::ProseLensMatch`: `ALL_ELEMENTS` 34→35 (+ the
`[SemanticElement; 34]` → `; 35]` type and the "34 = …" tally comment); `Theme::face` + `face_mut`
arms; `ThemeFaces` field `prose_lens_match`; the name-parse map (`"prose_lens_match" => …`); the
8 `ThemeFaces { … }` literal sites (§ Theme-tax note below); a11y/completeness tests. Cue-mode face =
`bold + italic + underline` (the only clean pairwise-distinct unused modifier combo — verified vs
`mono_faces()` + the a11y tests).

**Command-surface Rule-8 conformance (IN SCOPE — stated in spec §8 AND here).** 5 palette-only set
primitives (`prose_lens_adverbs/adjectives/passive/weak/off`) + ONE `register_stateful` cycle rep
(`prose_lens_next`, `MenuMark::Value(active_label)`, `MenuCategory::View`) + ONE shared setter
`lenses::set_prose_lens(editor, Option<ProseLensCategory>)` (Law 6, also arms the sweep) + two nav
commands (`prose_lens_next_match`/`prose_lens_prev_match`, palette-only). Registered via ONE
`lenses::register(r)` line in `Registry::builtins` — NO `Command` enum variant, NO `commands::run` arm
(A14 anti-regrowth). Palette-completeness + every-option-has-a-command invariant tests are merge GATEs.

**Hub budgets (merge GATE, `module_budgets.rs`).** `render.rs` < 900 (currently 840); `app.rs` < 1000
(currently 914); `timers.rs` < 400 (currently 228). The window helper lives in `lenses.rs`, NOT
`render.rs`.

**Arc laws.** D6 (COLOR + range-SELECT, never mutate without a visible abortable selection — the
range-select IS it; the writer mutates by ordinary typing). D7 (cold-path only — sweep + paint run only
when a lens is active). O(visible)+version-cached paint; resource law (zero idle work — `pos_sweep`
deadline `None` with no lens active). Lazy-reparse invariant (sweep the ACTIVE buffer only).

**Subagent-edit staleness.** For any compile/usage/signature question on code you are editing, trust
`cargo build`/`check`/`test` + `grep`, NEVER an editor "unused/undefined" hint — the analyzer lags
your edits by seconds.

**Theme-tax note (8 literal sites — deviation from the spec's "~22 constructors").** The ~22 public
theme fns share three builders, so `prose_lens_match` is added at exactly **8** `ThemeFaces { … }`
literal sites: `default()` (~theme.rs:607), `tokyo_night()` (~669), `terminal_ansi()` (~728),
`from_base16()` (~1013, covers catppuccin/flexoki/gruvbox/rosepine/solarized ×10), `blue_jeans()`
(~900, covers 3 variants), `mono_faces()` (~1139, for `no_color`), `phosphor()` (~1178, covers 5
hues), and the test literal `synthetic-low-contrast` (~2202). The `face_is_total_and_heading_clamps`
GATE still validates every one of the ~22 constructors resolves.

---

## File map

NEW:
- `wordcartel-nlp/src/classify.rs` — pure classifier (`classify`, `ProseLensCategory`, `BE_FORMS`,
  `SKIP`, `IRREGULAR_PARTICIPLES`, the target table). Re-exported from `wordcartel-nlp/src/lib.rs`.
- `wordcartel/src/lenses.rs` — `PosStore`, `PosMatch`, sweep dispatcher, `set_prose_lens`, cycle, nav,
  `active_pos_matches`, `prose_lens_count_segment`, `window_matches`, `register`.

EDITED:
- `wordcartel-core/src/theme.rs` — `+ProseLensMatch` (enum, ALL_ELEMENTS, face/face_mut, ThemeFaces,
  name-parse, 8 literals, tests).
- `wordcartel/src/jobs.rs` — `JobKind::PosSweep` + `is_stale` arm.
- `wordcartel/src/jobs_apply.rs` — `apply_panic` `PosSweep` arm.
- `wordcartel/src/timers.rs` — `pos_sweep` `SUBSYSTEMS` row + `on_tick` dispatch.
- `wordcartel/src/app.rs` — `advance()` arm for the sweep debounce.
- `wordcartel/src/editor.rs` — `Buffer.pos: PosStore`; `View.prose_lens: Option<ProseLensCategory>`.
- `wordcartel/src/registry.rs` — one `lenses::register(r)` line in `builtins`.
- `wordcartel/src/render.rs` — `RowCtx` field, `gather_row_ctx` read, `row_spans_placed` face-apply,
  the DRIVE-BY comment fix at line ~2218.
- `wordcartel/src/render_status.rs` OR `lenses.rs` — the count segment is called from the status
  assembly; the fn lives in `lenses.rs`.
- `wordcartel/src/limits.rs` — `LENS_MAX_SWEEP_BYTES`.
- `wordcartel/src/lib.rs` — `pub mod lenses;`.

---

## Task 1 — Pure classifier in `wordcartel-nlp`

**Objective.** `classify(sentences: &[TaggedSentence], text: &str) -> Vec<ClassifiedMatch>` implementing
the §5 rule. Pure, deterministic, in `wordcartel-nlp` (property-testable; the theme mapping stays in the
shell per the crate doc). `ProseLensCategory` is defined HERE (the shell re-exports it).

**Interfaces.**
- Consumes: `wordcartel_nlp::TaggedSentence { span, tokens }`, `TokenTag { range, upos: Option<UPOS>,
  np }`, `harper_brill::UPOS` (variants `ADV ADJ AUX VERB PART PUNCT NOUN PROPN PRON DET NUM ADP SCONJ
  CCONJ SYM INTJ`).
- Produces:
  ```rust
  #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
  pub enum ProseLensCategory { Adverbs, Adjectives, Passive, Weak }
  #[derive(Clone, PartialEq, Eq, Debug)]
  pub struct ClassifiedMatch { pub range: std::ops::Range<usize>, pub category: ProseLensCategory }
  pub fn classify(sentences: &[TaggedSentence], text: &str) -> Vec<ClassifiedMatch>;
  ```
  Ranges are in `text`'s coordinates (the same coordinates `analyze`'s tokens carry). Output sorted by
  `range.start` within the returned Vec is NOT guaranteed here (the shell sorts per-category after
  splitting); document that.

### Step 1.1 — Failing tests (the §10 corpus)

Create `wordcartel-nlp/src/classify.rs` with the type stubs + a `todo!()` body, and the test module
below. `ProseLensCategory`/`ClassifiedMatch`/`classify` must exist (compile) so the tests fail on
ASSERTION, not on a missing symbol.

```rust
//! Pure prose-lens classifier (S8): maps a tagged sentence stream to stylistic matches.
//! No theme, no shell types — the shell maps `ProseLensCategory` to a `SemanticElement`.
use crate::{TaggedSentence, UPOS};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ProseLensCategory { Adverbs, Adjectives, Passive, Weak }

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClassifiedMatch { pub range: std::ops::Range<usize>, pub category: ProseLensCategory }

/// Surface forms of "to be" (ASCII-lowercased compare). Triggers passive/weak by SURFACE, not the
/// AUX tag — Brill retags existential be to VERB (probe: "There are/VERB three problems.").
const BE_FORMS: [&str; 8] = ["be", "am", "is", "are", "was", "were", "been", "being"];

/// Skipped while scanning from a be-form to its target: inserted adverbs, negation/infinitival "to"
/// (PART), auxiliary chains (AUX — also consumes intermediate be/been/being so a chain fires once).
fn is_skip(u: Option<UPOS>) -> bool { matches!(u, Some(UPOS::ADV) | Some(UPOS::PART) | Some(UPOS::AUX)) }

/// ~150 irregular past participles whose surface does NOT end in -ed and that the dict maps to VERB.
/// SORTED — membership is binary-searched. Deliberately keeps base-identical irregulars (run/come/
/// put/set/cut/read/let/hit): favors recall on real prose over the rare pseudo-cleft FP.
const IRREGULAR_PARTICIPLES: &[&str] = &[ /* filled in Step 1.2, sorted */ ];

pub fn classify(_sentences: &[TaggedSentence], _text: &str) -> Vec<ClassifiedMatch> { todo!() }

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
    #[test] fn passive_being_prog()  { assert!(has("The results were being analyzed.", "being analyzed", Passive)); }
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
}
```

Run (expect FAIL — `todo!()` panics):
```
cargo test -p wordcartel-nlp classify 2>&1 | tail -20
# expect: multiple tests panicking at 'not yet implemented', test result: FAILED
```

### Step 1.2 — Implement the classifier

Fill `IRREGULAR_PARTICIPLES` (sorted) and `classify`:

```rust
const IRREGULAR_PARTICIPLES: &[&str] = &[
    "begun","bent","bled","born","borne","bought","bound","bred","brought","built","burnt",
    "cast","caught","chosen","clung","come","crept","cut","dealt","done","drawn","driven","drunk",
    "dug","dwelt","eaten","fallen","fed","felt","fled","flung","forbidden","forgiven","forgotten",
    "forsaken","fought","found","frozen","given","gone","ground","grown","heard","held","hidden",
    "hit","hung","hurt","kept","knelt","known","laid","lain","leapt","learnt","led","left","lent",
    "let","lit","lost","made","meant","met","mistaken","overcome","overtaken","paid","proven","put",
    "quit","read","ridden","risen","run","rung","said","sat","seen","sent","set","shaken","shed",
    "shone","shot","shown","shrunk","shut","slain","slept","slid","slit","smelt","sold","sought",
    "sped","spelt","spent","spilt","split","spoken","spread","sprung","spun","spat","stolen","stood",
    "stridden","struck","strung","stuck","stung","stunk","strove","sung","sunk","sworn","swept","swum",
    "swung","taken","taught","thought","thrown","thrust","told","torn","trodden","understood",
    "undertaken","upheld","upset","withdrawn","withheld","withstood","woken","won","worn","woven",
    "wept","wound","written","wrung",
];

fn is_participle_surface(surface: &str) -> bool {
    let lower = surface.to_ascii_lowercase();
    lower.ends_with("ed") || IRREGULAR_PARTICIPLES.binary_search(&lower.as_str()).is_ok()
}

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
```

De-dup + terminal-be reasoning (the two CRITICAL fixes, spec §5.1.2/§5.1.3):
- **Aux chains fire once.** `min_trigger = j.max(i+1)` after each trigger. For "The results were being
  analyzed.": `were` (i=2) triggers, scan skips `being`/AUX to target `analyzed` (j=4) → one Passive
  `were..analyzed`; then `being` (i=3) has `i < min_trigger (4)` → does NOT re-trigger. Exactly ONE
  match. The direct-tag ADV/ADJ pass is untouched (still per-token), so skipped adverbs like `quickly`
  still get the Adverbs lens.
- **Terminal-be vs unknown target are split.** No token remains (`toks.get(j) == None`) → SILENT
  ("Where have you been?"). A REAL `None`-tagged target runs the §5.1.2 `None` rows: lowercase +
  participle morphology → Passive ("The item was defenestrated." → `defenestrated`/None); otherwise
  SILENT (never a false Weak on an unknown word — the conservative floor).

Run (expect PASS):
```
cargo test -p wordcartel-nlp classify 2>&1 | tail -8
# expect: test result: ok. NN passed; 0 failed
```

Wire it in `wordcartel-nlp/src/lib.rs`:
```rust
mod classify;
pub use classify::{classify, ClassifiedMatch, ProseLensCategory};
```
Run the whole nlp crate green:
```
cargo test -p wordcartel-nlp 2>&1 | tail -5   # expect ok
```

**Commit:** `S8 Task 1: pure prose-lens classifier (passive/weak/adverb/adjective) in wordcartel-nlp`

**Reviewer gate (spec + quality):** does the target table match spec §5.1.2 exactly? Is the disjointness
property real (no be double-flags)? Is the irregular list sorted (binary_search precondition)? Any
`--`/em-dash violations, any `.unwrap()` on fallible paths?

---

## Task 2 — Theme element `ProseLensMatch`

**Objective.** Add `SemanticElement::ProseLensMatch` across the theme system; color = bg-tint
(SearchMatch template), cue = `bold + italic + underline`.

**Interfaces.**
- Consumes: `SemanticElement`, `ALL_ELEMENTS`, `Theme::face`/`face_mut`, `ThemeFaces`, `mono_faces`,
  the 8 literal sites, `modface`, `Face`.
- Produces: `SemanticElement::ProseLensMatch`, `ThemeFaces.prose_lens_match`.

### Step 2.1 — Failing GATE tests

Extend the existing tests in `wordcartel-core/src/theme.rs` (NOT new files — the completeness suite
lives there):
- In `ALL_ELEMENTS` add `ProseLensMatch`; change `[SemanticElement; 34]` → `; 35]` and the tally
  comment `34 = …` → `35 = … + 1 prose-lens`.
- In `no_color_is_monochrome_with_modifier_cues`, add `SemanticElement::ProseLensMatch` to the `cued`
  array.
- In the `render.rs` a11y test `a11y_every_cued_element_has_a_modifier_in_cue_mode` and
  `a11y_pairwise_distinct_same_context_pairs`, add `ProseLensMatch` to `cued` and add:
  ```rust
  assert_ne!(t.face(ProseLensMatch), t.face(Selection), "{}: ProseLensMatch vs Selection", t.name);
  assert_ne!(t.face(ProseLensMatch), t.face(SearchMatch), "{}: ProseLensMatch vs SearchMatch", t.name);
  assert_ne!(t.face(ProseLensMatch), t.face(DiagSpelling), "{}: ProseLensMatch vs DiagSpelling", t.name);
  assert_ne!(t.face(ProseLensMatch), t.face(DiagGrammar), "{}: ProseLensMatch vs DiagGrammar", t.name);
  assert_ne!(t.face(ProseLensMatch), t.face(MarkedBlock), "{}: ProseLensMatch vs MarkedBlock", t.name);
  ```

Run (expect FAIL — `ProseLensMatch` does not exist yet → compile error, which counts as the failing
step; the enum variant + field must be added to make it COMPILE, then the assertions drive the face
values):
```
cargo test -p wordcartel-core theme 2>&1 | tail -15
# expect: error[E0599]/E0063 — ProseLensMatch missing; that IS the red step
```

### Step 2.2 — Implement

1. `SemanticElement` enum: add `/// A prose-lens flagged token (S8). \n ProseLensMatch,` after
   `ChromeAccent` (end of enum, keeps the chrome block contiguous — but ALL_ELEMENTS order is
   independent; place it after `DiagGrammar` in ALL_ELEMENTS to group with overlays). Choose ONE
   position and be consistent; recommend right after `DiagGrammar` in BOTH the enum and ALL_ELEMENTS.
2. `struct ThemeFaces` (theme.rs:246): add `prose_lens_match: Face,`.
3. `Theme::face` match: `ProseLensMatch => self.faces.prose_lens_match,`.
4. `Theme::face_mut` match: `ProseLensMatch => &mut self.faces.prose_lens_match,`.
5. Name-parse map (~theme.rs:1069): `"prose_lens_match" => ProseLensMatch,`.
6. The 8 `ThemeFaces { … }` literals — add `prose_lens_match: …`:
   - `default()`: `prose_lens_match: Face { bg: Some(Color::Blue), fg: Some(Color::White), ..Face::default() },`
     (bg-tint drawing the eye; distinct hue from SearchMatch's Yellow bg).
   - `mono_faces()`: `prose_lens_match: m(true, true, true, false, false),` (bold+italic+underline —
     `m` args are `(bold, italic, underline, strike, reverse)`).
   - `tokyo_night()`, `terminal_ansi()`, `from_base16()`, `blue_jeans()`, `phosphor()`,
     `synthetic-low-contrast` test literal: a themed bg-tint following each theme's SearchMatch idiom
     (e.g. phosphor `Face { bg: Some(shade(hue, 2)), fg: Some(shade(hue, 5)), ..Face::default() }`;
     from_base16 `Face { bg: Some(b[0x0D]), fg: Some(b[0x00]), ..Face::default() }` — blue slot;
     blue_jeans `Face { bg: Some(r.selection_bg), ..Face::default() }` or a dedicated role; terminal_ansi
     `Face { bg: Some(Color::Blue), fg: Some(Color::Black), ..Face::default() }`). Each MUST clear the
     cue-mode a11y (mono_faces handles cue mode; the colored themes need a distinct bg — the pairwise
     assertions above enforce distinctness).

Run (expect PASS):
```
cargo test -p wordcartel-core theme 2>&1 | tail -8   # expect ok
cargo test -p wordcartel-core 2>&1 | tail -5          # whole core green
```

**Commit:** `S8 Task 2: SemanticElement::ProseLensMatch (bg-tint / bold+italic+underline cue) + theme completeness`

**Reviewer gate:** all 8 literals updated? `face_is_total_and_heading_clamps` green (ALL_ELEMENTS = 35)?
Cue-mode pairwise distinctness holds for tokyo/phosphor/no-color? bg-tint follows the SearchMatch
template per theme?

---

## Task 3 — `lenses.rs` leaf: store, state, commands, count

**Objective.** The shell spine, MINUS the worker sweep (Task 4) and render (Task 5). Everything that
compiles + tests without a running sweep: `PosStore`, `PosMatch`, `Buffer.pos`, `View.prose_lens`,
`set_prose_lens`, the 6 commands + cycle + 2 nav, `active_pos_matches`, `prose_lens_count_segment`,
`window_matches`, `ProseLensCategory::label`, `register`.

**Interfaces.**
- Consumes: `wordcartel_nlp::{ProseLensCategory, ClassifiedMatch}`; `editor::{Editor, Buffer, View,
  BufferId}`; `selection::Selection` (`range(to, from)`, `single`, `primary().to()`); `derive::rebuild`;
  `nav::ensure_visible`; `registry::{Registry, MenuCategory, MenuMark, CommandResult}` (via the
  `register`/`register_stateful` methods called on `&mut Registry` — but those are PRIVATE methods of
  `Registry`; see note); `commands::CommandResult`.
- Produces: `PosStore`, `PosMatch`, `pub use wordcartel_nlp::ProseLensCategory`, `set_prose_lens`,
  `active_pos_matches(&Editor) -> Option<&[PosMatch]>`, `prose_lens_count_segment(&Editor) ->
  Option<String>`, `window_matches(&[PosMatch], lo, hi) -> &[PosMatch]`, `register(&mut Registry)`.

**Registration note (grounding correction).** `Registry::register`/`register_stateful` are PRIVATE
`impl` methods (registry.rs:117/134), called only inside `Registry::builtins`. `lenses::register`
therefore cannot call them from another module. TWO options — pick per the A14 seam:
(a) add the 8 `r.register(...)`/`register_stateful(...)` lines DIRECTLY inside `Registry::builtins`
(one contiguous ProseLens block, like the `analysis_engine_harper`/`analysis_next` block) with the
handler bodies calling `crate::lenses::…` helpers; OR
(b) make `register`/`register_stateful` `pub(crate)` and add `crate::lenses::register(&mut r);` in
`builtins`. **Plan picks (a)** — it matches the existing precedent exactly (every command block is
inline in `builtins`; the handler bodies are one-liners delegating to `lenses::`), keeps
`register`/`register_stateful` private, and the "leaf module" discipline is satisfied because the LOGIC
lives in `lenses.rs` (handlers are thin delegations). The A14 constraint (no `Command` variant, no
`commands::run` arm) is honored: these are `register`/`register_stateful` calls with closures, exactly
like `toggle_ventilate`.

### Step 3.1 — Failing tests

Create `wordcartel/src/lenses.rs`. Define the types + `todo!()` bodies, add `pub mod lenses;` to
`lib.rs`, add `pub pos: PosStore` to `Buffer` and `pub prose_lens: Option<ProseLensCategory>` to `View`
(default `None`). Then the tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use wordcartel_nlp::ProseLensCategory::*;

    // --- helpers ---
    fn matches(cat: ProseLensCategory, spans: &[(usize, usize)]) -> Vec<PosMatch> {
        spans.iter().map(|&(s,e)| PosMatch { start: s, end: e, category: cat }).collect()
    }

    #[test]
    fn set_prose_lens_sets_view_state_per_buffer() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        assert_eq!(e.active().view.prose_lens, None);
        set_prose_lens(&mut e, Some(Passive));
        assert_eq!(e.active().view.prose_lens, Some(Passive));
        set_prose_lens(&mut e, None);
        assert_eq!(e.active().view.prose_lens, None);
    }

    #[test]
    fn active_pos_matches_gated_on_computed_for_version() {
        let mut e = Editor::new_from_text("The report was written here.\n", None, (80, 24));
        let v = e.active().document.version;
        e.active_mut().pos.passive = matches(Passive, &[(4, 21)]);
        e.active_mut().pos.computed_for = Some(v);
        // no lens active → None
        assert!(active_pos_matches(&e).is_none());
        set_prose_lens(&mut e, Some(Passive));
        assert_eq!(active_pos_matches(&e).map(|s| s.len()), Some(1));
        // version bump without re-sweep → stale → None (computed_for != version)
        e.active_mut().document.version += 1;
        assert!(active_pos_matches(&e).is_none(), "stale store must not paint");
    }

    #[test]
    fn active_pos_matches_empty_set_is_meaningful_zero() {
        // computed_for == version but zero matches → Some(&[]) (NOT None) — diverges from SourceSlot.
        let mut e = Editor::new_from_text("Nothing here.\n", None, (80, 24));
        let v = e.active().document.version;
        e.active_mut().pos.computed_for = Some(v);      // swept, found nothing
        set_prose_lens(&mut e, Some(Passive));
        assert_eq!(active_pos_matches(&e).map(|s| s.len()), Some(0), "empty is a real answer");
    }

    #[test]
    fn count_segment_gated_and_labeled() {
        let mut e = Editor::new_from_text("The report was written here.\n", None, (80, 24));
        let v = e.active().document.version;
        e.active_mut().pos.passive = matches(Passive, &[(4, 21)]);
        e.active_mut().pos.computed_for = Some(v);
        assert_eq!(prose_lens_count_segment(&e), None, "no lens → no segment");
        set_prose_lens(&mut e, Some(Passive));
        assert_eq!(prose_lens_count_segment(&e), Some("Passive: 1".into()));
        e.active_mut().document.version += 1;
        assert_eq!(prose_lens_count_segment(&e), None, "stale → suppressed");
    }

    #[test]
    fn cycle_walks_all_then_off() {
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let seq = [Some(Adverbs), Some(Adjectives), Some(Passive), Some(Weak), None, Some(Adverbs)];
        for want in seq {
            cycle_prose_lens(&mut e);
            assert_eq!(e.active().view.prose_lens, want);
        }
    }

    #[test]
    fn nav_next_range_selects_whole_span_head_at_start() {
        let mut e = Editor::new_from_text("The report was written by them.\n", None, (80, 24));
        let v = e.active().document.version;
        // "was written" span (bytes 11..22 of the text). Compute concretely:
        let t = e.active().document.buffer.to_string();
        let start = t.find("was written").unwrap();
        let end = start + "was written".len();
        e.active_mut().pos.passive = matches(Passive, &[(start, end)]);
        e.active_mut().pos.computed_for = Some(v);
        set_prose_lens(&mut e, Some(Passive));
        // caret at 0 → next finds the match, range-selects it, head at START.
        prose_lens_next_match(&mut e);
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (start, end), "whole span selected");
        assert_eq!(sel.head, start, "head-at-start (C-9) — caret lands at span start");
        assert!(!sel.is_empty(), "a visible abortable selection (D6)");
    }

    #[test]
    fn nav_wraps_and_noops_when_empty_or_offlens() {
        let mut e = Editor::new_from_text("no matches at all here.\n", None, (80, 24));
        // off-lens: no-op, no panic
        prose_lens_next_match(&mut e);
        prose_lens_prev_match(&mut e);
        // lens on, empty store: no-op, no panic
        set_prose_lens(&mut e, Some(Passive));
        e.active_mut().pos.computed_for = Some(e.active().document.version);
        prose_lens_next_match(&mut e);
        assert!(e.active().document.selection.primary().is_empty());
    }

    #[test]
    fn window_matches_upper_bounds_by_start() {
        // `window_matches` is ONLY the cheap upper-bound prefilter (partition_point on `start < hi`):
        // it returns the contiguous `[..hi_idx]` slice, the diag idiom. The `end > lo` lower bound is
        // NOT applied here — the paint loop applies it per glyph via `overlaps` (row_spans_placed).
        let ms = matches(Passive, &[(0, 5), (10, 15), (20, 25), (30, 35)]);
        // hi = 28 → keep every match with start < 28 → (0,5),(10,15),(20,25); (30,35) dropped.
        let w = window_matches(&ms, 28);
        assert_eq!(w.iter().map(|m| (m.start, m.end)).collect::<Vec<_>>(), vec![(0,5),(10,15),(20,25)]);
        // hi = 10 → start < 10 → only (0,5).
        assert_eq!(window_matches(&ms, 10).iter().map(|m| m.start).collect::<Vec<_>>(), vec![0]);
        // hi = 0 → empty slice.
        assert!(window_matches(&ms, 0).is_empty());
    }
}
```

Run (expect FAIL — `todo!()`):
```
cargo test -p wordcartel lenses:: 2>&1 | tail -20   # expect todo!() panics
```

### Step 3.2 — Implement the store + accessors + commands

```rust
//! Prose-lens spine (shell leaf, S8): per-buffer POS-match store, single-active lens state,
//! commands (Rule 8), the doc-wide count, and the visible-window helper. The sweep that FILLS the
//! store lives in the same module but is wired to the worker in Task 4. POS matches stay OUT of the
//! Diagnostic contract by construction (this is not a DiagSource).
use crate::editor::Editor;
pub use wordcartel_nlp::ProseLensCategory;

/// One flagged span + its lens category. `start/end` (not `Range`) so `PosMatch` stays `Copy`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PosMatch { pub start: usize, pub end: usize, pub category: ProseLensCategory }

/// Per-buffer POS-match store. Fuses the diag `SourceSlot` in-flight latch with the reconcile
/// `armed_for_version` anti-re-arm latch. `computed_for` is `Option<u64>` — an EMPTY match set is
/// meaningful ("0 passives"), so validity is `computed_for == Some(version)` regardless of emptiness
/// (diverges from `SourceSlot::valid_for`'s non-empty sentinel). All four category Vecs are sorted by
/// `start` and non-overlapping within a category (the classifier + a post-sort guarantee it).
///
/// `armed_for_version` is a SENTINEL-initialized latch (`u64::MAX`, NOT `0`): the `advance()` arm gate
/// (Task 4) is `armed_for_version != document.version`, which arms a fresh buffer (version 0, sentinel
/// MAX ≠ 0) exactly once and — crucially — does NOT re-arm once armed/dispatched for a version. This is
/// what latches the oversized-doc cap-skip (CRITICAL-3): the arm that led to `dispatch_pos_sweep` set
/// `armed_for_version = version`, so the cap-skip path (which never dispatches a job to clear in-flight)
/// is not re-armed on the same version — no arm→skip→re-arm loop. A real edit bumps the version and
/// re-arms naturally. (`Default` is hand-written for the sentinel; `derive(Default)` would give 0.)
#[derive(Clone, Debug)]
pub struct PosStore {
    pub adverbs: Vec<PosMatch>,
    pub adjectives: Vec<PosMatch>,
    pub passive: Vec<PosMatch>,
    pub weak: Vec<PosMatch>,
    pub computed_for: Option<u64>,
    pub due_at: Option<u64>,
    pub in_flight_version: Option<u64>,
    pub armed_for_version: u64,
}

impl Default for PosStore {
    fn default() -> Self {
        PosStore {
            adverbs: Vec::new(), adjectives: Vec::new(), passive: Vec::new(), weak: Vec::new(),
            computed_for: None, due_at: None, in_flight_version: None,
            armed_for_version: u64::MAX, // sentinel: a fresh buffer (version 0) still arms once
        }
    }
}

impl PosStore {
    /// The matches for `cat`, or `None` unless `computed_for == Some(version)`.
    pub fn matches_for(&self, cat: ProseLensCategory, version: u64) -> Option<&[PosMatch]> {
        if self.computed_for != Some(version) { return None; }
        Some(match cat {
            ProseLensCategory::Adverbs => &self.adverbs,
            ProseLensCategory::Adjectives => &self.adjectives,
            ProseLensCategory::Passive => &self.passive,
            ProseLensCategory::Weak => &self.weak,
        })
    }
}

/// Human label for the status segment and the menu-cycle mark.
pub fn category_label(cat: ProseLensCategory) -> &'static str {
    match cat {
        ProseLensCategory::Adverbs => "Adverbs",
        ProseLensCategory::Adjectives => "Adjectives",
        ProseLensCategory::Passive => "Passive",
        ProseLensCategory::Weak => "Weak",
    }
}

/// The ONE shared setter (contract Law 6). Sets the active buffer's lens; arming the sweep is handled
/// edge-triggered in `app.rs advance()` (it fires whenever a lens is active and the store is stale),
/// so this setter only sets state. (Kept symmetric with `ventilate::set_ventilate`.)
pub fn set_prose_lens(editor: &mut Editor, lens: Option<ProseLensCategory>) {
    editor.active_mut().view.prose_lens = lens;
}

/// Cycle Adverbs -> Adjectives -> Passive -> Weak -> off -> Adverbs.
pub fn cycle_prose_lens(editor: &mut Editor) {
    use ProseLensCategory::*;
    let next = match editor.active().view.prose_lens {
        None => Some(Adverbs),
        Some(Adverbs) => Some(Adjectives),
        Some(Adjectives) => Some(Passive),
        Some(Passive) => Some(Weak),
        Some(Weak) => None,
    };
    set_prose_lens(editor, next);
}

/// The single source of truth for "what the active prose lens shows" — the active category's slice,
/// gated on a lens being active AND the store being current (`computed_for == version`). Mirrors
/// `diagnostics_run::active_lens_diags` but for POS matches.
pub fn active_pos_matches(editor: &Editor) -> Option<&[PosMatch]> {
    let lens = editor.active().view.prose_lens?;
    let v = editor.active().document.version;
    editor.active().pos.matches_for(lens, v)
}

/// Right-side status segment "Passive: 47", shown only when a lens is active AND the count is honest
/// (`computed_for == version`). While a sweep is in flight or the store is stale → `None`.
pub fn prose_lens_count_segment(editor: &Editor) -> Option<String> {
    let lens = editor.active().view.prose_lens?;
    let n = active_pos_matches(editor)?.len();
    Some(format!("{}: {}", category_label(lens), n))
}

/// Upper-bound a sorted-by-`start` match slice to `start < hi` (the diag idiom, `partition_point`).
/// Returns the contiguous `[..hi_idx]` prefix; the `end > lo` LOWER bound is applied per glyph by the
/// paint loop (`overlaps` in `row_spans_placed`), so `lo` is intentionally NOT a parameter here. Lives
/// in `lenses.rs` (NOT render.rs) for the hub budget.
pub fn window_matches(ms: &[PosMatch], hi: usize) -> &[PosMatch] {
    let hi_idx = ms.partition_point(|m| m.start < hi);
    &ms[..hi_idx]
}
```

Nav (inline the private `set_selection_range` idiom; commit `Selection::range(to, from)` — DIVERGES
from `diag_next`'s `Selection::single`):

```rust
use wordcartel_core::selection::Selection;

fn nav_to(editor: &mut Editor, start: usize, end: usize) {
    crate::registry::unfold_ancestors_of(editor, start);
    // Head-at-start (C-9): Selection::range(anchor, head) puts head on the 2nd arg → pass (end, start)
    // so from()==start, to()==end, head==start. The whole span is the visible abortable selection (D6).
    editor.active_mut().document.selection = Selection::range(end, start);
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}

pub fn prose_lens_next_match(editor: &mut Editor) {
    let Some(ms) = active_pos_matches(editor) else { return; };
    if ms.is_empty() { return; }
    let caret = editor.active().document.selection.primary().to();
    let (start, end) = ms.iter().find(|m| m.start > caret)
        .map(|m| (m.start, m.end))
        .unwrap_or((ms[0].start, ms[0].end)); // wrap
    nav_to(editor, start, end);
}

pub fn prose_lens_prev_match(editor: &mut Editor) {
    let Some(ms) = active_pos_matches(editor) else { return; };
    if ms.is_empty() { return; }
    let caret = editor.active().document.selection.primary().to();
    let last = ms.len() - 1;
    let (start, end) = ms.iter().rev().find(|m| m.start < caret)
        .map(|m| (m.start, m.end))
        .unwrap_or((ms[last].start, ms[last].end)); // wrap
    nav_to(editor, start, end);
}
```

(`active_pos_matches` borrows `editor` immutably; extract the owned `(start, end)` in a scope that
drops the borrow before `nav_to`'s `&mut`, as the nlp.rs test does with `Range::clone`. The code above
already copies the tuple out before `nav_to`.)

Registration — add this contiguous block INSIDE `Registry::builtins` (registry.rs), right after the
`analysis_next`/`toggle_engine_harper` block, delegating to `lenses::`:

```rust
// Prose lenses (S8) — Rule 8: 5 palette-only set primitives + one stateful cycle rep + two nav
// commands; the shared setter is lenses::set_prose_lens (Law 6). Per-buffer state on View. A14: no
// Command variant, no commands::run arm — thin delegations into the lenses leaf.
r.register("prose_lens_adverbs",    "Prose Lens: Adverbs",    None, |c| { crate::lenses::set_prose_lens(c.editor, Some(crate::lenses::ProseLensCategory::Adverbs)); CommandResult::Handled });
r.register("prose_lens_adjectives", "Prose Lens: Adjectives", None, |c| { crate::lenses::set_prose_lens(c.editor, Some(crate::lenses::ProseLensCategory::Adjectives)); CommandResult::Handled });
r.register("prose_lens_passive",    "Prose Lens: Passive",    None, |c| { crate::lenses::set_prose_lens(c.editor, Some(crate::lenses::ProseLensCategory::Passive)); CommandResult::Handled });
r.register("prose_lens_weak",       "Prose Lens: Weak",       None, |c| { crate::lenses::set_prose_lens(c.editor, Some(crate::lenses::ProseLensCategory::Weak)); CommandResult::Handled });
r.register("prose_lens_off",        "Prose Lens: Off",        None, |c| { crate::lenses::set_prose_lens(c.editor, None); CommandResult::Handled });
r.register_stateful("prose_lens_next", "Prose Lens", Some(MenuCategory::View),
    |e| match e.active().view.prose_lens {
        Some(cat) => MenuMark::Value(crate::lenses::category_label(cat)),
        None => MenuMark::Value("Off"),
    },
    |c| { crate::lenses::cycle_prose_lens(c.editor); CommandResult::Handled });
r.register("prose_lens_next_match", "Next Prose-Lens Match", None, |c| { crate::lenses::prose_lens_next_match(c.editor); CommandResult::Handled });
r.register("prose_lens_prev_match", "Previous Prose-Lens Match", None, |c| { crate::lenses::prose_lens_prev_match(c.editor); CommandResult::Handled });
```

(`category_label` returns `&'static str`, satisfying `MenuMark::Value(&'static str)`.)

Run (expect PASS):
```
cargo test -p wordcartel lenses:: 2>&1 | tail -10   # expect ok
cargo test -p wordcartel registry:: 2>&1 | tail -8  # palette-completeness / every-option-has-a-command still green
```

The palette-completeness and every-option-has-a-command invariant tests (registry.rs / menu.rs)
automatically cover the new commands — confirm they pass (they enumerate the registry; no per-command
edit needed). If `no_registry_command_runs_a_mutating_epilogue_on_a_read_only_buffer` flags a new
command, that is EXPECTED to pass (all ProseLens commands are non-mutating — they set view state / move
the selection, never edit the buffer; registered via `register`/`register_stateful` with `mutates:
false`). Verify.

**Commit:** `S8 Task 3: lenses.rs spine — PosStore, single-active state, Rule-8 commands, count segment`

**Reviewer gate:** Rule-8 conformance (5 primitives + cycle + shared setter + 2 nav, no Command
variant)? `computed_for` Option-gate correct (empty = Some(&[]))? Nav commits `Selection::range`
head-at-start (NOT `single`)? `window_matches` matches the diag idiom? Borrow discipline in nav?

---

## Task 4 — The doc-wide sweep: `JobKind::PosSweep`, panic arm, timers, `advance()`

**Objective.** Fill `PosStore` on the worker, edge-triggered, debounced, version-discarded, prose-only,
gated on tree-converged, idle-free. Near-copy of `reconcile::dispatch_reconcile`.

**Interfaces.**
- Consumes: `jobs::{Job, JobResult, JobKind, JobOutcome, ResultClass, Executor, is_stale}`;
  `jobs_apply::apply_panic`; `timers::{SUBSYSTEMS, on_tick, TimedSubsystem}`; `reconcile::maybe_stale`;
  `Document::blocks()`, `BlockTree::role_at`, `BlockRole::Paragraph`, `ventilate::prose_block_at`;
  `buffer.snapshot()`; `wordcartel_nlp::{analyze, classify}`; `limits::LENS_MAX_SWEEP_BYTES`.
- Produces: `JobKind::PosSweep`, `lenses::dispatch_pos_sweep`, `lenses::pos_sweep_due`,
  `POS_SWEEP_DEBOUNCE_MS`, the `apply_panic` arm, the `pos_sweep` `SUBSYSTEMS` row, the `advance()` arm.

### Step 4.1 — `JobKind::PosSweep` + `is_stale` arm (compiler-forced)

`jobs.rs`: add `PosSweep, // coalescible background POS sweep; version-checked in merge` to `JobKind`.
This breaks the exhaustive matches in `is_stale` and `apply_panic` — a red compile that IS the failing
step. `is_stale` arm: add `JobKind::PosSweep` to the `false` group:
```rust
JobKind::Save | JobKind::SwapWrite | JobKind::Reparse | JobKind::PosSweep => false,
```
Run:
```
cargo build -p wordcartel 2>&1 | grep -A2 "PosSweep\|non-exhaustive" | head
# expect: error in jobs_apply.rs apply_panic — match not exhaustive (Step 4.2 fixes it)
```

### Step 4.2 — `apply_panic` arm (grounding correction #2)

`jobs_apply.rs` `apply_panic` (the exhaustive `match kind`), add BEFORE the `#[cfg(test)] CoalesceProbe`
arm:
```rust
JobKind::PosSweep => {
    // A panicked sweep (upstream harper residual) is deterministic for this text — clear in-flight
    // and leave computed_for behind so we do NOT re-arm and retry every debounce (mirrors Reparse).
    if let Some(b) = editor.by_id_mut(buffer_id) {
        b.pos.in_flight_version = None;
        // computed_for untouched: the next edit bumps document.version, so advance() re-arms exactly
        // once per edit (never a tight loop on the same version).
    }
}
```
Failing-test-first for the panic path (add to `lenses.rs` tests or `jobs_apply.rs` tests):
```rust
#[test]
fn panicked_pos_sweep_clears_in_flight_no_retry_loop() {
    use crate::editor::Editor;
    let mut e = Editor::new_from_text("x\n", None, (80, 24));
    let id = e.active().id;
    e.active_mut().pos.in_flight_version = Some(0);
    crate::jobs_apply::apply_outcome(crate::jobs::JobOutcome::Panicked {
        buffer_id: id, version: 0, kind: crate::jobs::JobKind::PosSweep, msg: "boom".into(),
    }, &mut e);
    assert!(e.active().pos.in_flight_version.is_none(), "panic clears in-flight");
}
```
Run:
```
cargo test -p wordcartel panicked_pos_sweep 2>&1 | tail -6   # after impl: ok
```

### Step 4.3 — `dispatch_pos_sweep` (near-copy of `dispatch_reconcile`) + cap

`limits.rs`: `pub const LENS_MAX_SWEEP_BYTES: u64 = 8 * 1024 * 1024;` (mirrors `DIAG_MAX_SEND_BYTES`).

`lenses.rs`:
```rust
use crate::jobs::{Executor, Job, JobKind, JobResult, ResultClass};

/// Debounce before a settled buffer is swept. Slightly longer than reconcile's 150 ms so the block
/// tree (which the prose-range enumeration reads) settles first.
pub const POS_SWEEP_DEBOUNCE_MS: u64 = 300;

/// A sweep is due if a lens is active, the tree has converged, nothing is in flight, and the debounce
/// deadline has passed. Gated identically in the timers deadline fn (idle-free / A3 anti-spin).
pub fn pos_sweep_due(editor: &Editor, now: u64) -> bool {
    editor.active().view.prose_lens.is_some()
        && !editor.active().reconcile.maybe_stale        // tree converged (correction #4)
        && editor.active().pos.in_flight_version.is_none()
        && matches!(editor.active().pos.due_at, Some(t) if now >= t)
}

/// Enumerate prose-PARAGRAPH byte ranges (role_at == Paragraph) from the block tree — the sweep must
/// NOT analyze code fences / headings / front matter / tables (correction #4). Walks the block tree
/// (`BlockTree::top_level` + recursion into containers) and keeps each block whose role is Paragraph.
/// A list-item/blockquote paragraph has a higher-precedence role (`role_at` returns ListItem/BlockQuote
/// there, `block_tree.rs role_precedence`), so it is naturally excluded — consistent with
/// `ventilate::prose_block_at`, which the S7/lens paint window uses. GROUNDING NOTE: `TextBuffer` has
/// NO `len_lines()` (verified — buffer.rs), so this walks BLOCKS, not lines.
fn prose_paragraph_ranges(editor: &Editor) -> Vec<(usize, usize)> {
    use wordcartel_core::block_tree::Block;
    use wordcartel_core::style::BlockRole;
    let blocks = editor.active().document.blocks();
    let mut out = Vec::new();
    fn walk(blocks: &wordcartel_core::block_tree::BlockTree, b: &Block, out: &mut Vec<(usize, usize)>) {
        if b.children.is_empty() {
            // Leaf: keep it iff its role resolves to Paragraph (excludes code/heading/rule/etc.).
            if blocks.role_at(b.span.start) == BlockRole::Paragraph {
                out.push((b.span.start, b.span.end));
            }
        } else {
            for c in &b.children { walk(blocks, c, out); }
        }
    }
    for b in blocks.top_level() { walk(blocks, b, &mut out); }
    out
}

/// Snapshot the active buffer + dispatch the doc-wide POS sweep (near-copy of dispatch_reconcile).
pub fn dispatch_pos_sweep(editor: &mut Editor, ex: &dyn Executor) {
    let b = editor.active();
    let buffer_id = b.id;
    let version = b.document.version;
    if b.document.buffer.len() as u64 > crate::limits::LENS_MAX_SWEEP_BYTES {
        // CRITICAL-3 latch: skip WITHOUT dispatching a job, and pin the anti-re-arm latch to THIS
        // version so `advance()` (whose gate is `armed_for_version != version`) will not re-arm on the
        // same version — no arm→skip→re-arm loop. `armed_for_version` is already `version` here (the
        // arm that triggered this dispatch set it), so this is a belt-and-braces reassert; `due_at`
        // stays None. A real edit bumps the version → the gate opens → one fresh attempt + one fresh
        // notice. The sticky notice therefore fires at most once per version (idle-free at rest).
        editor.active_mut().pos.due_at = None;
        editor.active_mut().pos.armed_for_version = version;
        editor.set_status_full(crate::status::StatusKind::Warning,
            "document too large for prose lenses — sweep skipped",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return;
    }
    let ranges = prose_paragraph_ranges(editor);
    let rope = editor.active().document.buffer.snapshot();  // O(1)
    editor.active_mut().pos.in_flight_version = Some(version);
    editor.active_mut().pos.due_at = None;

    let job = Job {
        buffer_id, class: ResultClass::BufferLocal, version, kind: JobKind::PosSweep,
        run: Box::new(move || {
            let mut adverbs = Vec::new(); let mut adjectives = Vec::new();
            let mut passive = Vec::new(); let mut weak = Vec::new();
            for (ps, pe) in ranges {
                // snapshot() returns ropey::Rope (verified — buffer.rs); slice by BYTE via
                // byte_slice (NOT .slice, which is char-indexed) then to_string for analyze's &str.
                let slice = rope.byte_slice(ps..pe).to_string();
                let sentences = wordcartel_nlp::analyze(&slice);
                for m in wordcartel_nlp::classify(&sentences, &slice) {
                    let pm = PosMatch { start: ps + m.range.start, end: ps + m.range.end, category: m.category };
                    match m.category {
                        ProseLensCategory::Adverbs => adverbs.push(pm),
                        ProseLensCategory::Adjectives => adjectives.push(pm),
                        ProseLensCategory::Passive => passive.push(pm),
                        ProseLensCategory::Weak => weak.push(pm),
                    }
                }
            }
            for v in [&mut adverbs, &mut adjectives, &mut passive, &mut weak] {
                v.sort_by_key(|m| m.start);
            }
            JobResult {
                buffer_id, class: ResultClass::BufferLocal, version, kind: JobKind::PosSweep,
                merge: Box::new(move |editor: &mut Editor| {
                    if let Some(b) = editor.by_id_mut(buffer_id) {
                        if b.document.version == version {   // version-discard INSIDE the merge
                            b.pos.adverbs = adverbs; b.pos.adjectives = adjectives;
                            b.pos.passive = passive; b.pos.weak = weak;
                            b.pos.computed_for = Some(version);
                        }
                        b.pos.in_flight_version = None;      // clear regardless
                    }
                }),
            }
        }),
    };
    ex.dispatch(job);
}
```

GROUNDING NOTE (resolved): `snapshot()` returns `ropey::Rope` (buffer.rs:166, `self.rope.clone()`,
O(1)); `analyze`/`classify` take `&str`. Use `rope.byte_slice(ps..pe).to_string()` — ropey's
`byte_slice` is BYTE-indexed (our ranges are byte offsets); the bare `Rope::slice` is CHAR-indexed and
would corrupt multibyte offsets. (Contrast: `TextBuffer::slice(Range) -> String` at buffer.rs:125 is
byte-indexed on the foreground; the worker only holds the `Rope`.)

### Step 4.4 — `advance()` arm (correction #1) + timers row + `on_tick`

`app.rs advance()` — add AFTER the reconcile arm block (keep the same `let now`/`active_mut` scope
pattern):
```rust
// Prose-lens sweep debounce (S8): arm ONLY when a lens is active and the store is stale for the
// current version. The gate is `armed_for_version != version` (NOT reconcile's
// `due_at.is_none() || armed_for_version != version` escape) — this arms EXACTLY ONCE per version and
// never re-arms once armed/dispatched, which is what latches the oversized-doc cap-skip (CRITICAL-3):
// the cap path returns without an in-flight job, so only the `armed_for_version` pin stops a re-arm
// loop. `armed_for_version` is sentinel-initialized to `u64::MAX` (PosStore::default), so a fresh
// buffer (version 0) still arms (MAX != 0). A real edit bumps the version → the gate re-opens. Cost is
// exactly zero when no lens is active (the `is_some()` guard short-circuits before any field read).
{
    let now = clock.now_ms();
    let b = editor.active_mut();
    if b.view.prose_lens.is_some() {
        let stale = b.pos.computed_for != Some(b.document.version);
        let arm = stale
            && b.pos.in_flight_version.is_none()
            && b.pos.armed_for_version != b.document.version;
        if arm {
            b.pos.due_at = Some(now.saturating_add(crate::lenses::POS_SWEEP_DEBOUNCE_MS));
            b.pos.armed_for_version = b.document.version;
        }
    }
}
```

`timers.rs` — add a deadline fn + a `SUBSYSTEMS` row + an `on_tick` dispatch:
```rust
/// Prose-lens sweep deadline — same A3 shape as reconcile, plus a lens-active gate and a
/// tree-converged gate (the enumeration reads document.blocks()). None with no lens active → idle-free.
fn pos_sweep_deadline(e: &Editor, _now: u64) -> Option<u64> {
    if e.active().view.prose_lens.is_some()
        && e.active().pos.in_flight_version.is_none()
        && !e.active().reconcile.maybe_stale {
        e.active().pos.due_at
    } else { None }
}
```
Add `TimedSubsystem { name: "pos_sweep", deadline: pos_sweep_deadline },` to `SUBSYSTEMS` (after
`reconcile`). In `on_tick`, after the reconcile dispatch:
```rust
if crate::lenses::pos_sweep_due(editor, now) {
    crate::lenses::dispatch_pos_sweep(editor, ex);
}
```

### Step 4.5 — Failing → passing tests (the reconcile battery)

Add to `lenses.rs` tests (drive with `InlineExecutor` like reconcile's tests):
```rust
#[test]
fn sweep_fills_store_and_marks_computed_for() {
    use crate::jobs::{Executor, InlineExecutor};
    let mut e = crate::editor::Editor::new_from_text("The report was written by them.\n", None, (80, 24));
    let v = e.active().document.version;
    set_prose_lens(&mut e, Some(ProseLensCategory::Passive));
    let ex = InlineExecutor::default();
    dispatch_pos_sweep(&mut e, &ex);
    for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
    assert_eq!(e.active().pos.computed_for, Some(v));
    assert_eq!(e.active().pos.passive.len(), 1, "one passive found doc-wide");
    assert!(e.active().pos.in_flight_version.is_none());
}

#[test]
fn sweep_discards_when_version_advanced() {
    use crate::jobs::{Executor, InlineExecutor};
    let mut e = crate::editor::Editor::new_from_text("The report was written.\n", None, (80, 24));
    set_prose_lens(&mut e, Some(ProseLensCategory::Passive));
    let ex = InlineExecutor::default();
    dispatch_pos_sweep(&mut e, &ex);
    e.active_mut().document.version += 1;   // edit lands before merge
    for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
    assert_ne!(e.active().pos.computed_for, Some(e.active().document.version - 1));
    assert!(e.active().pos.computed_for.is_none() || e.active().pos.computed_for != Some(e.active().document.version));
    assert!(e.active().pos.in_flight_version.is_none(), "in-flight cleared even on discard");
}

#[test]
fn sweep_is_prose_only_code_fence_is_not_flagged() {
    use crate::jobs::{Executor, InlineExecutor};
    // "is" inside a fenced code block must NOT be classified.
    let t = "Para one is here.\n\n```\nthis is code\n```\n";
    let mut e = crate::editor::Editor::new_from_text(t, None, (80, 24));
    crate::derive::rebuild(&mut e);   // build the block tree
    set_prose_lens(&mut e, Some(ProseLensCategory::Weak));
    let ex = InlineExecutor::default();
    dispatch_pos_sweep(&mut e, &ex);
    for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
    // The paragraph's "is" is weak; the code fence's "is" is not in any match.
    let code_is = t.rfind("is code").unwrap();
    assert!(!e.active().pos.weak.iter().any(|m| m.start <= code_is && code_is < m.end),
        "code-fence 'is' must not be flagged");
}

#[test]
fn no_lens_active_arms_no_pos_sweep_deadline() {
    // idle-free: with no lens active, the pos_sweep subsystem yields None even with a past-due arm.
    let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
    let dl = || crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "pos_sweep").unwrap().deadline;
    e.active_mut().pos.due_at = Some(0);              // armed, past due
    assert_eq!((dl())(&e, 10_000), None, "no lens → None (no wake)");
    // lens on + converged tree + not in flight → the armed deadline reappears (gate is load-bearing).
    set_prose_lens(&mut e, Some(ProseLensCategory::Passive));
    e.active_mut().reconcile.maybe_stale = false;
    assert_eq!((dl())(&e, 10_000), Some(0), "lens on + converged → armed deadline live");
    // in-flight gate:
    e.active_mut().pos.in_flight_version = Some(1);
    assert_eq!((dl())(&e, 10_000), None, "in-flight → None");
    e.active_mut().pos.in_flight_version = None;
    // tree-stale gate:
    e.active_mut().reconcile.maybe_stale = true;
    assert_eq!((dl())(&e, 10_000), None, "tree not converged → None");
}

#[test]
fn oversized_doc_cap_latches_no_rearm_loop() {
    // CRITICAL-3: an oversized doc skips WITHOUT dispatching a job, latches armed_for_version to the
    // current version so advance() will NOT re-arm on the same version, and yields a None deadline.
    use crate::jobs::{Executor, InlineExecutor};
    let big = "word ".repeat((crate::limits::LENS_MAX_SWEEP_BYTES as usize / 5) + 2); // just over 8 MiB
    let mut e = crate::editor::Editor::new_from_text(&big, None, (80, 24));
    let v = e.active().document.version;
    set_prose_lens(&mut e, Some(ProseLensCategory::Weak));
    // Mimic the advance() arm that fired this dispatch (it sets armed_for_version = version, due_at Some).
    e.active_mut().pos.armed_for_version = v;
    e.active_mut().pos.due_at = Some(0);
    let ex = InlineExecutor::default();
    dispatch_pos_sweep(&mut e, &ex);
    assert!(ex.drain().is_empty(), "cap path dispatches NO job");
    assert_eq!(e.active().pos.due_at, None, "due_at cleared");
    assert_eq!(e.active().pos.armed_for_version, v, "latched to this version");
    assert_eq!(e.active().pos.computed_for, None, "never computed");
    assert!(e.active().pos.in_flight_version.is_none());
    assert!(e.status_text().contains("too large"), "one-time sticky notice");
    // The advance() arm gate would NOT re-arm on the same version (no loop).
    let stale = e.active().pos.computed_for != Some(v);
    let would_rearm = stale && e.active().pos.in_flight_version.is_none()
        && e.active().pos.armed_for_version != v;
    assert!(!would_rearm, "cap-skip must not re-arm on the same version");
    // Deadline None after the skip (due_at None) even with a converged tree + lens active → idle-free.
    let dl = crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "pos_sweep").unwrap().deadline;
    e.active_mut().reconcile.maybe_stale = false;
    assert_eq!((dl)(&e, 10_000), None, "deadline None after the one skip");
}
```

Run:
```
cargo test -p wordcartel lenses:: 2>&1 | tail -14       # expect ok
cargo test -p wordcartel timers:: 2>&1 | tail -6        # existing idle-free guardrails still green
cargo test -p wordcartel reconcile:: 2>&1 | tail -6     # unaffected
```

**Commit:** `S8 Task 4: doc-wide PosSweep job — prose-only, converged-gated, debounced, version-discarded`

**Reviewer gate:** version-check INSIDE the merge? panic arm clears in-flight without retry-loop?
`advance()` arm version-latched (no idle re-arm)? deadline None with no lens (idle-free, both
directions)? prose-only enumeration correct (code fence excluded)? cap enforced?

---

## Task 5 — Render: paint the active lens

**Objective.** Paint `active_pos_matches` in the visible window, BETWEEN Search and Diag, forcing
`use_placed`. Surgical (~25–35 lines). Plus the DRIVE-BY comment fix.

**Interfaces.**
- Consumes: `RowCtx`, `gather_row_ctx`, `row_spans_placed`, `lenses::{active_pos_matches,
  window_matches, PosMatch}`, `SemanticElement::ProseLensMatch`, `compose::compose`/`face_to_ratatui`,
  `overlaps`.
- Produces: the painted highlight.

### Step 5.1 — Failing render test

Add to `render.rs` tests (Truecolor depth, real Editor, TestBackend or the existing style-probe
harness the file already uses):
```rust
#[test]
fn prose_lens_paints_flagged_span_between_search_and_diag() {
    use wordcartel_core::theme::SemanticElement::ProseLensMatch;
    let t = "The report was written by them.\n";
    let mut ed = crate::editor::Editor::new_from_text(t, None, (80, 24));
    ed.depth = wordcartel_core::theme::Depth::Truecolor;
    crate::derive::rebuild(&mut ed);
    let v = ed.active().document.version;
    let start = t.find("was written").unwrap();
    let end = start + "was written".len();
    ed.active_mut().pos.passive = vec![crate::lenses::PosMatch { start, end, category: crate::lenses::ProseLensCategory::Passive }];
    ed.active_mut().pos.computed_for = Some(v);
    crate::lenses::set_prose_lens(&mut ed, Some(crate::lenses::ProseLensCategory::Passive));
    // use_placed must be forced on by an active lens match.
    let ctx = super::gather_row_ctx(&ed);
    assert!(ctx.use_placed, "an active prose lens forces the placed path");
    // The painted style over a glyph inside the span carries the ProseLensMatch bg.
    let want_bg = crate::compose::face_to_ratatui(&ed.theme.face(ProseLensMatch), ed.depth).bg;
    // (assert via the row-span builder over the flagged span — mirror the existing diag paint test's
    //  structure: build spans for line 0, find the run covering `start`, assert its bg == want_bg.)
}
```
(Flesh the assertion out against the file's existing paint-test helper — the S7/diag tests in render.rs
show the exact pattern for extracting a placed run's style. The load-bearing asserts: `use_placed` is
forced, and a glyph in the span carries the ProseLensMatch bg while a glyph outside does not.)

Run (expect FAIL — the `RowCtx` field / paint arm don't exist):
```
cargo test -p wordcartel prose_lens_paints 2>&1 | tail -12
```

### Step 5.2 — Implement

`RowCtx`: add `prose_lens: &'a [crate::lenses::PosMatch],` (the active-lens slice, or `&[]`). Update
the struct doc comment's field count.

`gather_row_ctx`: after the diag block:
```rust
let prose_lens: &[crate::lenses::PosMatch] =
    crate::lenses::active_pos_matches(editor).unwrap_or(&[]);
let prose_lens_active = !prose_lens.is_empty();
```
and fold into `use_placed`:
```rust
let use_placed = !hl_window.is_empty() || diag_active || has_sel || has_block || prose_lens_active;
```
Add `prose_lens` to the `RowCtx { … }` construction.

`row_spans_placed`: after computing `lo`/`hi`, upper-bound the lens matches (the `end > lo` lower bound
is applied per glyph by `overlaps` in the loop below, so `window_matches` takes only `hi`):
```rust
let lens_window = crate::lenses::window_matches(ctx.prose_lens, hi);
```
In the per-glyph loop, BETWEEN the Search arm and the Diag arm:
```rust
// Prose-lens highlight (S8) — composes above Search, below Diagnostics (errors stay topmost).
if lens_window.iter().any(|m| overlaps(g_from, g_to, m.start, m.end)) {
    let lf = editor.theme.face(SE::ProseLensMatch);
    style = style.patch(crate::compose::face_to_ratatui(&lf, editor.depth));
}
```

**DRIVE-BY (Codex-flagged).** Fix the stale comment at `render.rs:~2218` in
`a11y_every_cued_element_has_a_modifier_in_cue_mode`: `DiagGrammar=bold+underline` →
`DiagGrammar=italic+underline` (the real `mono_faces` value is `italic+underline`). One-line comment
correction only.

Run (expect PASS):
```
cargo test -p wordcartel prose_lens_paints 2>&1 | tail -8   # ok
cargo test -p wordcartel render 2>&1 | tail -6              # composition/cue/ventilate green
cargo test -p wordcartel --test module_budgets 2>&1 | tail -3   # render.rs < 900 GATE
```

**Commit:** `S8 Task 5: render prose-lens highlight (between Search and Diag) + DiagGrammar comment fix`

**Reviewer gate:** paint order (above Search, below Diag)? `use_placed` forced? windowing = the diag
idiom (partition_point + end>lo filter)? ventilate `origin_of` rebase correct (span coords are absolute,
matching `line_off + p.src`)? render.rs still < 900? cue-mode a11y green? comment fixed?

---

## Task 6 — Nav + count end-to-end wiring

**Objective.** Wire the count segment into the status assembly and pin the full nav→select→type-replaces
loop against a real `Editor`. (Task 3 built the fns; this integrates + the status render.)

**Interfaces.**
- Consumes: `render_status::status` assembly (where `word_count_segment` is placed);
  `lenses::prose_lens_count_segment`; `commands::insert_char` (typing replaces the selection).
- Produces: the count segment shown on the right of the status line when a lens is active + current.

### Step 6.1 — Failing test

```rust
#[test]
fn typing_replaces_the_nav_selected_match() {
    use crate::test_support::TestClock;
    let t = "The report was written by them.\n";
    let mut e = crate::editor::Editor::new_from_text(t, None, (80, 24));
    let v = e.active().document.version;
    let start = t.find("was written").unwrap();
    let end = start + "was written".len();
    e.active_mut().pos.passive = vec![crate::lenses::PosMatch { start, end, category: crate::lenses::ProseLensCategory::Passive }];
    e.active_mut().pos.computed_for = Some(v);
    crate::lenses::set_prose_lens(&mut e, Some(crate::lenses::ProseLensCategory::Passive));
    crate::lenses::prose_lens_next_match(&mut e);
    // typing over the selection replaces the whole flagged span (D6 — abortable, then mutate by typing).
    crate::commands::insert_char(&mut e, 'X', &TestClock(0));
    assert_eq!(e.active().document.buffer.to_string(), "The report X by them.\n");
}
```
And a status-render assertion that `prose_lens_count_segment` appears when active (mirror
`word_count_segment_selection_aware` in render_status.rs tests).

Run (expect FAIL if the status wiring isn't in place; the typing test may already pass from Task 3 —
that's fine, it's the integration pin):
```
cargo test -p wordcartel typing_replaces_the_nav 2>&1 | tail -8
```

### Step 6.2 — Implement

In the status-line assembly (where `render_status::word_count_segment` is consumed — find its call site
in `render.rs`/`render_status.rs`), add the prose-lens count as a sibling right-side segment:
```rust
if let Some(seg) = crate::lenses::prose_lens_count_segment(editor) { /* push seg into the right group */ }
```
(Place it beside the word-count segment; the exact push mechanism follows the existing right-segment
assembly — the implementer matches the neighbor.)

Run (expect PASS):
```
cargo test -p wordcartel typing_replaces_the_nav 2>&1 | tail -6   # ok
cargo test -p wordcartel render_status 2>&1 | tail -6             # count segment shown when active
```

**Commit:** `S8 Task 6: prose-lens count segment in the status line + nav-select-then-type integration pin`

**Reviewer gate:** count shown only when `computed_for == version`? typing replaces the whole selected
span? no double-count / no overlap with word-count segment?

---

## Task 7 — e2e journey + final merge gates

**Objective.** An in-process e2e journey exercising the real `reduce → advance → render` loop, plus all
merge gates.

### Step 7.1 — e2e journey (`wordcartel/src/e2e.rs`)

```rust
#[test]
fn journey_prose_lens_passive_paints_navigates_counts() {
    // Open a doc, activate the Passive lens via the command, advance until the sweep lands, assert the
    // count segment renders "Passive: N", nav range-selects the first match, typing replaces it, cycling
    // to Off clears paint + count and arms no timer (idle-free).
    // Drives the REAL loop: dispatch the command, drive Tick(s) with an InlineExecutor + TestClock past
    // POS_SWEEP_DEBOUNCE_MS, drain, render to a TestBackend, scrape the status line for "Passive:".
    // ... (follow the existing e2e journey harness pattern in e2e.rs) ...
}
```
Run:
```
cargo test -p wordcartel journey_prose_lens 2>&1 | tail -8   # ok
```

### Step 7.2 — Merge gates

```
cargo test --workspace 2>&1 | tail -15
# expect: all suites ok (wordcartel-core lib+oracle, wordcartel-nlp, wordcartel lib, integration)

cargo build --workspace 2>&1 | tail -3            # warning-free
cargo test --workspace --no-run 2>&1 | tail -3    # warning-free

cargo clippy --workspace --all-targets 2>&1 | tail -5
# expect: no warnings (deny gate). Any needed #[allow] carries a one-line rationale.

cargo test -p wordcartel --test module_budgets 2>&1 | tail -3
# expect: ok — render.rs < 900, app.rs < 1000, timers.rs < 400

# too_many_lines is part of clippy --all-targets above; confirm no new function trips 100.

bash scripts/smoke/run.sh 2>&1 | tail -3
# PTY smoke — mandatory-run/advisory-pass. Quote the one-line summary verbatim in the merge report
# (e.g. "smoke: 8/8 PASS", or "smoke: SKIP — no tmux", or a red result surfaced as advisory).

cargo deny check 2>&1 | tail -5
# RELEASE-CHECKLIST step (not a merge gate). S8 adds NO new crate (harper-brill already in-tree from
# S7), so the dependency graph is UNCHANGED — record "cargo deny: unchanged (no new deps)" in notes.
```

### Step 7.3 — Final review gates (both must pass)
- Fable whole-branch review (cross-task invariants: paint order, idle-free across the whole feature,
  no data-loss/panic classes, borrow discipline; compile scratch probes against the branch).
- Codex pre-merge GO/NO-GO.
Re-run after fixes until clean/GO.

**Commit:** `S8 Task 7: e2e prose-lens journey; merge gates green`

**Merge:** `--no-ff` to trunk; verify `cargo test --workspace` on the merged result; delete the branch.
Push only when asked.

---

## Command-surface contract conformance (stated in spec §8 AND here)

ProseLens is a multi-state option (4 categories + off). Rule 8 is honored: **5 palette-only set
primitives** (`prose_lens_adverbs/adjectives/passive/weak/off`, `menu: None`, deterministic for
automation) + **ONE stateful cycle representative** (`prose_lens_next`, `register_stateful`,
`MenuMark::Value(active_label)`, `MenuCategory::View`) + **ONE shared setter**
(`lenses::set_prose_lens`, Law 6, the only writer of `view.prose_lens`) + two palette-only nav commands.
Registered via `register`/`register_stateful` closures inside `Registry::builtins` (thin delegations to
`lenses::`), with the LOGIC in the `lenses.rs` leaf — **no `Command` enum variant, no `commands::run`
arm** (A14 anti-regrowth). The palette-completeness and every-option-has-a-command invariant tests
(merge GATEs) cover the new commands automatically. Hint re-resolution is automatic (registry reads the
active `KeyTrie`).

---

## Self-review (writing-plans)

- **Spec coverage — every §-requirement has a task:** §2 scope (all tasks); §3 decisions (T1 rule, T3
  act-model nav, naming throughout); §4 lens defs (T1); §5 classifier (T1, the corpus); §6 architecture
  + all 6 corrections (T3 `computed_for`/set_selection_range-inline, T4 armed_for_version/panic-arm/
  prose-only/converged-gate/cap, jobs `is_stale`); §7 paint+theme (T2 element+tax, T5 paint+cue); §8
  command surface (T3 + the conformance section); §9 count (T3 fn, T6 wiring); §10 corpus (T1 fixtures);
  §11 arc laws (T4 idle-free, T5 O(visible), T3 D6 nav); §12 task sketch (T1–T7). COVERED.
- **Placeholder scan:** no `TODO`/`TBD`/`FIXME`/`XXX`. Two explicitly-scoped "implementer fleshes the
  assertion against the existing harness" notes (T5 paint-probe, T7 e2e) point at real in-file patterns,
  not missing design.
- **Type-name consistency across tasks:** `PosMatch` (fields `start`/`end`/`category`, `Copy`),
  `PosStore`, `ProseLensCategory` (from `wordcartel_nlp`, re-exported by `lenses`), `set_prose_lens`,
  `active_pos_matches`, `prose_lens_count_segment`, `window_matches`, `cycle_prose_lens`,
  `prose_lens_next_match`/`prose_lens_prev_match`, `dispatch_pos_sweep`, `pos_sweep_due`,
  `POS_SWEEP_DEBOUNCE_MS`, `LENS_MAX_SWEEP_BYTES`, `JobKind::PosSweep`, `SemanticElement::ProseLensMatch`,
  `PosStore.computed_for/due_at/in_flight_version/armed_for_version` — used identically in every task.
- **No `--`/em-dash-as-`--` violations:** prose uses em-dashes; code comments use em-dash in prose per
  house style; no bare `--` operator misuse.
- **Codex round-1 fixes re-reviewed (all in plan CODE; spec + signatures were confirmed):**
  - **CRITICAL-1 (aux-chain double-emit):** the classifier now carries a `min_trigger` high-water mark
    (`min_trigger = j.max(i+1)` after each trigger); a be-form at `i < min_trigger` does not re-trigger,
    so "were being analyzed" emits exactly ONE Passive. The direct-tag ADV/ADJ pass stays per-token
    (skipped adverbs still flag). New test `aux_chain_fires_once` (asserts len==1 for the chain and for
    "has been written"). Re-checked: the outer loop still visits every token; only the trigger is gated.
  - **CRITICAL-2 (terminal-be vs None target):** split into `toks.get(j) == None` → SILENT
    (terminal be), vs a real `None`-tagged target → the §5.1.2 `None` rows (lowercase+morphology →
    Passive, else SILENT — never a false Weak on an unknown word). The old `else`-pushes-Weak path is
    gone. Corpus tests `none_terminal_be` and `passive_none_ed`/`passive_none_irreg` pin both.
  - **CRITICAL-3 (oversized-doc re-arm loop):** the `advance()` arm gate is now
    `armed_for_version != version` (dropped reconcile's `due_at.is_none()` OR-escape); `PosStore::default`
    sentinel-inits `armed_for_version = u64::MAX` so a fresh buffer still arms once. The cap-skip pins
    `armed_for_version = version` (+ `due_at = None`) so no arm→skip→re-arm loop; sticky notice fires at
    most once per version. New guardrail `oversized_doc_cap_latches_no_rearm_loop` (no job dispatched,
    latched state, deadline None, `would_rearm == false`). Re-verified the sentinel handles the
    fresh-buffer-version-0 case the new gate would otherwise strand.
  - **MINOR (window_matches test contradiction):** `window_matches` signature is now `(&[PosMatch],
    hi)` (upper-bound-only prefilter; the `end > lo` lower bound is a per-glyph `overlaps` in the paint
    loop). Test `window_matches_upper_bounds_by_start` is self-consistent (three concrete `hi` values,
    no contradiction, no "rewrite" note); render call site updated to `window_matches(ctx.prose_lens, hi)`.
- **Codex-confirmed items unchanged (per instruction):** registry-private → inline-in-`builtins`
  delegation; `rope.byte_slice` + block-tree walk (no `len_lines`); `set_selection_range` private →
  `Selection::range(to, from)` inlined; `blocks()`/`role_at`/`BlockRole::Paragraph`.
```
