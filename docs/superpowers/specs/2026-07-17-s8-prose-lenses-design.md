# S8 — Prose lenses (POS-driven stylistic X-rays) — Design spec

Status: DRAFT (awaiting Codex spec gate)
Author: Fable (independent grounding + authoring)
Date: 2026-07-17
Arc: prose-structure arc (S4–S8), `docs/design/prose-structure-arc.md` — S8 = E4 payoff.
Builds on: S7 linguistic substrate (`docs/superpowers/specs/2026-07-17-s7-linguistic-substrate-design.md`).

---

## 1. Summary

S8 adds **prose lenses**: single-active stylistic X-rays that highlight one habit class at a time over
the writer's prose. Four lenses ship — **Adverbs**, **Adjectives**, **Passive voice**, **Weak verbs** —
each with visible-window highlight paint, whole-document navigate-and-select, and a live doc-wide count
for the active lens. The lenses are built on S7's `wordcartel_nlp::analyze` (rule-based UPOS tags +
NP-chunk flags) and ride the existing `jobs.rs` worker substrate and `render.rs` per-glyph paint path — a
**parallel POS-typed pipeline modeled on `reconcile.rs`**, NOT a new diagnostics source. POS matches never
enter `Diagnostic`/`DiagnosticKind`/`DiagnosticsProvider` (those are reserved for the ltex/vale prose
linters). The feature is cold-path only: with no lens active it does ZERO work (no timer wakes, no worker);
with a lens active the sweep runs on the worker, edge-triggered on edit-version, debounced, and
version-discarded, so typing stays instant.

The act model is **select-only** (arc law D6): navigation range-selects the whole flagged span; the writer
edits it away with ordinary keystrokes. No operators.

---

## 2. Scope

### 2.1 In scope
- The reusable **ProseLens spine**: per-buffer store, doc-wide async sweep, single-active lens state,
  window paint, navigate-and-select, doc-wide count.
- Four lenses: **Adverbs** (`ADV`), **Adjectives** (`ADJ`), **Passive voice**, **Weak verbs**.
- Command surface (Rule 8): 5 palette-only set primitives + one stateful cycle representative + one shared
  setter + two nav commands.
- One new `SemanticElement::ProseLensMatch` face across the theme system.

### 2.2 Out of scope — deferred to S9 (arc-doc note)
Per the arc doc (`docs/design/prose-structure-arc.md`, the S8 row and the "S8 — Prose lenses" section),
S8 ships the **spine + four flag-word/pattern lenses + doc-wide count**. The **`Phrase`** object (the
chunker's NP runs) and the **`Clause`** object (POS-informed clause splitting, per **D5**) — both
**select-only** — are DEFERRED to **S9**. This spec records that scoping the same way S7 amended the arc
for crate placement: this document is the authority for the S8 slice; S9 owns Phrase/Clause. No arc law
changes; D5's "clause ships select-only behind a measured precision gate" is untouched and future.

---

## 3. Resolved decisions (human-ratified — do not re-litigate)

- **D-scope.** Ship the spine + four lenses (§2). Phrase/Clause deferred to S9.
- **D-composition.** Single active lens at a time + a cycle (Adverbs → Adjectives → Passive → Weak → off),
  plus direct per-lens set-commands. One shared "flagged token" treatment; the status line names the active
  lens. Lenses need no mutually-distinct cues because only one shows at once.
- **D-visual.** Highlight the matches (draw the eye TO them). One legibility profile; a modifier fallback
  in cue/no-color mode.
- **D-reach.** Paint the visible window; navigate/act across the whole document; show a live doc-wide count
  for the active lens.
- **D-cycle-selector.** A SEPARATE prose-lens selector, distinct from the harper diagnostics/analysis-engine
  cycle. (Errors-you-fix vs habits-you-consider are different mental models.)
- **D-act = select-only (arc D6).** Navigation **RANGE-SELECTS the whole match** (head-at-start, contract
  C-9): jump → the flagged span is selected → typing replaces it → Escape/motion aborts. This diverges from
  the `diag_next` caret-only precedent (which commits `Selection::single(target)`): the ProseLens nav
  commits `Selection::range(to, from)` over the whole match span. The range-selection IS the visible
  abortable selection D6 requires.
- **D-passive-edges (all ratified).** (i) An unknown lowercase token with `-ed`/irregular-participle
  morphology after a be-form → PASSIVE (catches dict gaps). (ii) A be-form at sentence-end / before PUNCT
  → SILENT (no flag). (iii) The passive match range = the WHOLE `be..participle` span (not the participle
  only).
- **D-naming = ProseLens.** Use `ProseLens*` naming throughout (feature, module types, `SemanticElement`,
  commands) to avoid colliding with the diagnostics code's existing "analysis lens" vocabulary
  (`active_lens_diags`, `active_analysis_source`). The module file may be `lenses.rs`, but the public types
  and commands read `ProseLens`.

---

## 4. Lens definitions

Four `ProseLensCategory` values. Each maps a `wordcartel_nlp::TaggedSentence` stream to a sorted list of
byte-range matches. All rules are grounded against the SHIPPED `harper-brill` 2.5.0 model
(`trained_tagger_model.json`), verified by a compiled probe harness (§10 corpus).

| Category | Rule | Match range |
|----------|------|-------------|
| `Adverbs` | every token with `upos == Some(UPOS::ADV)` | the token range |
| `Adjectives` | every token with `upos == Some(UPOS::ADJ)` | the token range |
| `Passive` | a be-form + (skipped ADV/PART/AUX)* + a past-participle target | whole be..participle span |
| `Weak` | a be-form whose non-skipped continuation is copular/existential/locative (not a participle) | the be-form token |

Adverbs/Adjectives are direct-tag flag-all "highlight-to-locate" awareness lenses — simple, predictable,
the writer decides what to cut. Passive and Weak are the two halves of a **disjoint partition of be-usage**
(§5): the same be-occurrence yields exactly one classification, so a be NEVER double-flags.

**Detection philosophy — CONSERVATIVE.** Prefer a miss (false-negative) over a false flag: a lens that cries
wolf loses the writer's trust. Every rule below is calibrated to that: silent on ambiguity, quiet on
constructions the tagger cannot resolve.

---

## 5. §5.1 — The passive/weak classifier (highest-value section)

### 5.1.1 What the tagger gives (and does not)

Ground truth from the real harper source:
- `wordcartel_nlp::analyze(text) -> Vec<TaggedSentence>`; each `TokenTag { range, upos: Option<UPOS>, np }`
  (`wordcartel-nlp/src/lib.rs`).
- `UPOS` (`harper-pos-utils-2.5.0/src/upos.rs`) is the 16-variant Universal POS tagset. It has `VERB` and
  `AUX` but **NO tense/aspect/participle feature** — those are UD morphological "feats", which
  `BrillTagger` does not produce. The ONLY output channel is
  `Tagger::tag_sentence(&[String]) -> Vec<Option<UPOS>>` (`harper-pos-utils-2.5.0/src/tagger/mod.rs`). There
  is no lemma, no feats, no per-token metadata.
- The base tagger is a `FreqDict` — a `HashMap<String, UPOS>` of lowercase-word → single most-common UPOS
  (`harper-pos-utils-2.5.0/src/tagger/freq_dict.rs`) — refined by 201 Brill patches that remap UPOS→UPOS on
  context (`patch_criteria.rs`).

So "was written", "was running", and "was happy" all surface as be + `{VERB | VERB | ADJ}`. UPOS alone
cannot tell participle from finite past from gerund. **The rule below recovers the distinction from token
SURFACE MORPHOLOGY + adjacency, not from any UPOS feature.**

Two shipped-model facts make this achievable (both probe-verified against `trained_tagger_model.json`):
1. **The dict already collapses helpfully.** Every common past participle — regular and irregular, including
   spellings identical to the past tense — maps to `VERB` (`written`, `made`, `found`, `left`, `felt`,
   `thought`, `read`, `put`, `set`, `gotten`, …). Participles whose dominant use is adjectival map to `ADJ`
   (`tired`, `excited`, `interested`, `worried`, `concerned`) — which correctly lands them in the WEAK lens
   ("was tired" IS copular). All eight be-forms map to `AUX`.
2. **Adjacency disambiguates tense for free.** A finite past-tense verb essentially cannot grammatically
   follow a be-form, so `be + VERB` with participle surface morphology IS the passive signal; the gerund
   (progressive) case is excluded by the `-ing` surface.

### 5.1.2 The rule

**Step 1 — trigger on a be-form by SURFACE, not tag.**

```
BE_FORMS = { "be", "am", "is", "are", "was", "were", "been", "being" }   // ASCII lowercase compare
```

A candidate is any token whose lowercased surface text is in `BE_FORMS`. **Do NOT gate on `upos == AUX`:**
Brill patches retag existential be to `VERB` — probe-verified: "There **are**/VERB three problems." and
"There **is**/VERB a problem." A tag-gated trigger silently drops every existential (a Weak-lens case). The
surface set is the authority; the tag is advisory.

**Step 2 — forward scan over the SKIP SET.**

From the be-form, scan forward *within the same sentence* (`TaggedSentence.tokens`), skipping any token
whose tag is in:

```
SKIP = { ADV, PART, AUX }
```

- `ADV` skips inserted adverbs: "was **quickly** closed", "was **never** seen" (probe: quickly/ADV,
  never/ADV).
- `PART` skips negation and infinitival "to": "was **not** solved" (not/PART), "was **to** be expected"
  (to/PART).
- `AUX` skips auxiliary chains and consumes intermediate be-forms: "has **been** written" (been/AUX),
  "was **being** analyzed" (being/AUX). Left-to-right consumption of the skipped be/been/being means they
  never re-trigger — **this is what dedups aux chains** (see §5.1.3).

Land on the first non-skipped token = the **target** `T`.

**Step 3 — classify the target.**

Let `surface = &text[T.range]` (the live buffer slice), `first = surface.chars().next()`.

| Target condition | Classification |
|------------------|----------------|
| `T.upos == VERB` and `surface` ends in `-ing` | **none** (progressive — "was running") |
| `T.upos == VERB` and (`surface` ends in `-ed` OR lowercased `surface` in `IRREGULAR_PARTICIPLES`) | **Passive** |
| `T.upos == VERB`, base form (neither `-ing`/`-ed` nor in the list) | **none** (pseudo-cleft "all I did **was call**") |
| `T.upos == None` and `first` is lowercase and (`-ed` OR in `IRREGULAR_PARTICIPLES`) | **Passive** (dict-gap recovery, D-passive-edge (i)) |
| `T.upos == PUNCT`, or no non-skipped token remains before sentence end | **none** (SILENT terminal-be, D-passive-edge (ii)) |
| anything else (`ADJ`, `NOUN`, `PROPN`, `PRON`, `DET`, `NUM`, `ADP`, `SCONJ`, `CCONJ`, `SYM`, `INTJ`) | **Weak** |

- **Passive** match range = `(be_form.range.start, T.range.end)` — the WHOLE be..participle span
  (D-passive-edge (iii)).
- **Weak** match range = `be_form.range` (the be token only).
- The `None` + lowercase guard excludes capitalized-unknown false positives from the Passive lens: "It was
  **Fred**." tags PROPN → Weak; "It was **Zed**." (a `None`-tagged unknown proper name, uppercase first char)
  does NOT satisfy the None-row's lowercase+morphology condition, so it → **SILENT (no match)**, NOT Passive.
  (Note: a `None`-tagged non-participle target — whether uppercase like "Zed" or lowercase without `-ed`/an
  irregular-participle spelling like "zorgle" — is left SILENT, a deliberate conservative miss; the Weak lens
  only fires on a target the tagger actually classified as ADJ/NOUN/PROPN/etc.) "The item was
  **defenestrated**." (defenestrated/None, lowercase, `-ed`) → Passive.

### 5.1.3 Disjoint by construction

One classifier runs per be-occurrence and returns exactly one of {Passive, Weak, none}. The forward scan
consumes intermediate be-forms as SKIP tokens, so a chained "has been written" fires once (on the leading
`has`? no — `has` is not a be-form; on `been`, whose target after skipping is `written` → Passive; the
subsequent `been`/`being` never re-triggers because the scan already passed them). The Passive and Weak
sets are therefore disjoint partitions of be-usage; the render/nav layers treat them as independent sorted
match lists and never reconcile them.

### 5.1.4 The irregular-participle list

`IRREGULAR_PARTICIPLES`: a static, sorted `&[&str]` of ~150 lowercase irregular past participles whose
surface does not end in `-ed` and that the dict maps to `VERB` (so `-ed` morphology alone would miss them).
Membership is tested by binary search. Seed set (grouped; final list finalized in Task 1 against the probe
corpus):

`written taken given seen done made found held kept left sent put set told thought built brought known
shown grown thrown worn born borne chosen broken driven eaten fallen forgotten frozen hidden ridden risen
shaken spoken stolen sworn woken begun sung won caught bought taught felt lost meant met paid read said
sold heard led laid lent let hit hurt cut come gone run bent bound bled bred burnt cast clung crept dealt
dug drawn drunk dwelt fed fled flung forbidden forgiven forsaken fought ground hung knelt lain leapt learnt
lit meant mistaken overcome overtaken paid proven quit rung sat shed shone shot shrunk shut slain slept slid
slit smelt sought sped spelt spent spilt spun spat split spread sprung stood stuck stung stunk stridden
struck strung strove sung sunk swept swum swung torn thrust trodden understood undertaken upheld upset woven
wept wound withdrawn withheld withstood wrung`

The list is DELIBERATELY conservative: keeping base-identical irregulars (`run`, `come`, `put`, `set`,
`cut`, `read`, `let`, `hit`) means the common "the test **was run**" flags, at the cost of the rare
pseudo-cleft "all I did **was run**" false-positiving. That trade favors recall on real prose. (The list
lives in `wordcartel-nlp` beside the classifier — pure linguistics, property-testable.)

### 5.1.5 Precision profile

Grounded on the probe corpus (§10). **Misses (false negatives), by design:**
- **Contracted be is invisible.** The S7 tokenizer joins "it's"/"they're"/"there's" into ONE token tagged
  `PRON`/`None` (`wordcartel-nlp/src/lib.rs` apostrophe path; probe: "It's written by hand." has no be-form
  token). Contracted passives and copulas are all missed. This is a structural limitation of the shipped
  tokenizer, documented, not fixed here.
- Get-passives ("got promoted") — out of scope by the be-form definition.
- Comma-broken chains ("was, in fact, wrong") — the comma (PUNCT, not in SKIP) ends the scan → SILENT.
- Attributive mis-tags ("the **left** side" → left/VERB) miss the adjective lens but never false-flag.
- Asterisk-adorned be-forms ("`**was** written`") — unlike underscores (S7 strips leading/trailing `_` runs
  from a word token), the `*` splits into its own token that is neither a be-form nor in SKIP, so it lands as
  the scan target and the passive goes SILENT. Conservative; no false flag.
- Locative/adverbial copulas ("It **is here**.", "They **were there**.") — "here"/"there" tag `ADV`, which is
  in SKIP, so the scan runs past them to the terminal PUNCT → SILENT. The Weak lens therefore misses locative
  copulas whose complement is an adverb, while "It was **in** the room." (in/ADP → Weak) still flags.
  Conservative; no false flag.

**False positives (passive), rare constructions:**
- Headless relative "What it **was changed** everything" (finite past adjacent to clause-final be).
- Pseudo-cleft with base-identical irregulars ("all I did **was run**") — accepted per §5.1.4.
- Stative/adjectival passives ("The store **is closed**", "He **was left** alone") flag as passive —
  defensible, arguably desirable, for a style lens.

Net: misses are structural (contractions), FPs are rare constructions. This is the conservative profile the
philosophy (§4) demands and is not materially weaker than "detect passives" for an awareness lens.

---

## 6. Architecture — the validated reuse pipeline

S8 is a **parallel POS-typed pipeline sharing the `jobs.rs` worker substrate and the `render.rs` window-paint
mechanism, modeled on `reconcile.rs`** — NOT a `DiagSource`/`DiagnosticsProvider`. Rationale, grounded:
`Diagnostic` carries message/code/severity/suggestions (all dead for a color+select overlay);
`DiagnosticKind` is a closed `Spelling | Grammar` enum matched in `render.rs row_spans_placed`;
`DiagnosticsProvider` is subprocess/LSP-lifecycle ceremony. `reconcile.rs` already IS the generic
doc-wide-async-in-process framework (generic because it rides `jobs.rs`). Keep POS out of the core
`Diagnostic` contract; ltex/vale are reserved there.

### 6.1 Reuse buckets (validated against real source)

**REUSE AS-IS:** `jobs.rs` worker (`Executor::dispatch`/`drain`; results ride `JobOutcome::Done` — no new
`Msg`); `wordcartel_nlp::analyze` (+ types, Send, pure); the `render.rs row_spans_placed` per-glyph
window-paint (sorted slice + `partition_point` + overlap filter); the `diag_next`/`diag_prev` nav idiom
(`registry.rs`); the `render_status::word_count_segment` seam.

**EXTEND (open-by-design enums):** `jobs::JobKind` (+`PosSweep`); `timers::SUBSYSTEMS` (+row + `on_tick`
dispatch); `theme::SemanticElement` (+`ProseLensMatch`, one variant).

**NET-NEW (small, each a near-copy):** the `lenses.rs` leaf module (store + classifier glue + commands +
count segment + accessor); `ProseLensCategory`; `PosMatch`; the doc-wide sweep dispatcher (near-copy of
`reconcile::dispatch_reconcile`); the classifier (`wordcartel-nlp`, Task 1).

### 6.2 Corrections the real source forced (all folded here)

1. **`PosStore` needs `armed_for_version`** (anti-re-arm latch). `ReconcileStore` (`reconcile.rs`) carries
   `armed_for_version` and is armed in `app.rs advance()` (the version-latched block, ~`b.reconcile.due_at =
   Some(now + RECONCILE_DEBOUNCE_MS); b.reconcile.armed_for_version = b.document.version`), NOT in reduce.
   PosStore mirrors the field and gets a sibling arm in the SAME `advance()` block so idle Ticks cannot push
   the deadline forever.
2. **`JobKind::PosSweep`'s panic arm is compiler-forced.** The panic cleanup lives in
   `jobs_apply::apply_panic`, an **exhaustive `match kind`** (`jobs_apply.rs`). Adding `PosSweep` forces a
   new arm; it mirrors the `Reparse` arm — clear `in_flight_version` AND clear the stale trigger
   (`computed_for` stays behind / the due latch is cleared) so a deterministic panic cannot retry-loop every
   debounce. `is_stale` (`jobs.rs`) gets a `PosSweep => false` arm (version-check happens inside the merge,
   like `Reparse`).
3. **`commands::set_selection_range` is PRIVATE** (`fn set_selection_range`, no `pub` — `commands.rs`). The
   ProseLens nav needs the head-at-start selection idiom. Resolution: make it `pub(crate)`, OR inline its
   three lines in the lens nav (`selection = Selection::range(to, from); derive::rebuild(editor);
   nav::ensure_visible(editor)`). Spec picks **inline in the lens module** (keeps commands.rs closed; the
   nav also does `unfold_ancestors_of` first, like `diag_next`).
4. **The sweep analyzes PROSE PARAGRAPHS ONLY.** A raw whole-document `analyze` would POS-flag "is" inside a
   code fence, a heading, front matter, a table. The S7 authority is `role_at(content_byte) ==
   BlockRole::Paragraph` (`ventilate::prose_block_at` / `BlockTree::role_at`, `block_tree.rs`). At dispatch,
   enumerate prose-paragraph ranges from `document.blocks()` (cheap O(blocks) main-thread walk; `BlockTree`
   is a plain `pub` struct), ship `Vec<(ps, pe)>` + the O(1) `buffer.snapshot()` rope to the worker; the
   worker `analyze`s each window and rebases spans by `+ps` (the exact idiom `nlp::nlp_window_at` uses).
5. **Sweep gates on `!reconcile.maybe_stale` (tree converged).** Prose-range enumeration reads
   `document.blocks()`; if the block tree is mid-divergence the ranges are wrong. Gate sweep dispatch on the
   reconcile tree being settled. The gate goes in BOTH the timers deadline fn and the `on_tick` due
   predicate (the A3 anti-spin pattern `timers.rs` enforces on every subsystem). A reconcile completion
   wakes the loop (its `Executor` drain), so no lost wake-up.
6. **`computed_for` is `Option<u64>`, not the `SourceSlot` non-empty sentinel.** `SourceSlot::valid_for`
   (`diagnostics_run.rs`) requires `!diagnostics.is_empty() && computed_version == version` — an empty
   result is treated as "never computed". For ProseLens an EMPTY match set is MEANINGFUL ("0 passives" is a
   real, displayable answer). So `PosStore` uses `computed_for: Option<u64>`; a match is paintable/countable
   iff `computed_for == Some(document.version)`, regardless of emptiness. **This diverges deliberately from
   the SourceSlot precedent** — call it out in review.
7. **Cap `LENS_MAX_SWEEP_BYTES = 8 * 1024 * 1024`** (M5-spirit; mirrors `DIAG_MAX_SEND_BYTES` in
   `limits.rs`). Beyond the cap the sweep is skipped and a one-time Sticky status notice is shown; the count
   segment reads "…" / is suppressed. Prevents an unbounded worker pass on a pathological document.

### 6.3 Store + state

```rust
// wordcartel/src/lenses.rs
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProseLensCategory { Adverbs, Adjectives, Passive, Weak }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PosMatch { pub range: std::ops::Range<usize>, pub category: ProseLensCategory }
// (range: Copy is fine — Range<usize> is Copy? No; store start/end or keep Clone. Impl detail: use
//  { start: usize, end: usize } to keep PosMatch Copy, or drop Copy. Plan decides.)

#[derive(Default, Clone, Debug)]
pub struct PosStore {
    pub adverbs: Vec<PosMatch>,      // each list sorted by range.start, non-overlapping-per-category
    pub adjectives: Vec<PosMatch>,
    pub passive: Vec<PosMatch>,
    pub weak: Vec<PosMatch>,
    pub computed_for: Option<u64>,   // Some(version) the matches reflect; None = never swept
    pub due_at: Option<u64>,         // debounce deadline
    pub in_flight_version: Option<u64>,
    pub armed_for_version: u64,      // anti-re-arm latch (§6.2 #1)
}
```

- `PosStore` is per-buffer: `Buffer.pos: PosStore` (beside `Buffer.nlp`, `Buffer.reconcile`, `Buffer.diagnostics`).
- **One sweep computes ALL four categories in one pass** — one `analyze`, then classify every token into all
  four lists. This makes lens switching instant (no re-sweep), the doc-wide count of every category free, and
  costs one worker pass. Strictly better than sweeping only the active category.
- Lens state = `Option<ProseLensCategory>` on `View`, beside `View.ventilate` (per-buffer). `None` = off. The
  sweep follows the ACTIVE buffer only (consistent with the lazy-reparse invariant — non-active buffers are
  never eager-swept).
- **`nlp_window_at` / `NlpStore` stay OFF S8's path.** With a doc-wide `PosStore`, painting the visible
  window = windowing the sorted `PosStore` list; S8 never calls `nlp_window_at`. `NlpStore` remains for other
  future callers. State this explicitly to avoid two parallel POS paths.

### 6.4 The sweep dispatcher (near-copy of `reconcile::dispatch_reconcile`)

`lenses::dispatch_pos_sweep(editor, ex)`:
1. Read active buffer id + `document.version`; if `document.buffer.len() as u64 > LENS_MAX_SWEEP_BYTES`,
   skip + notice, return.
2. Enumerate prose-paragraph ranges: walk `document.blocks()` top-level, collect spans whose
   `role_at(content_byte) == Paragraph` (reuse `ventilate::prose_block_at` per candidate line, or a direct
   block walk; plan picks the cheapest). O(blocks).
3. `let rope = b.document.buffer.snapshot();` (O(1)).
4. Set `pos.in_flight_version = Some(version)`, `pos.due_at = None`.
5. Dispatch a `Job { kind: JobKind::PosSweep, class: ResultClass::BufferLocal, version, run: … }`. The `run`
   closure (pure, on the worker): for each `(ps, pe)`, `analyze(&rope.slice(ps..pe))`, rebase spans `+ps`,
   classify tokens into the four category lists, sort each by `range.start`. Returns a `merge` closure that
   **version-checks INSIDE** (`if b.document.version == version`) before adopting into `b.pos`, sets
   `pos.computed_for = Some(version)`, and clears `pos.in_flight_version` unconditionally.

Panic path: `apply_panic`'s new `PosSweep` arm clears `in_flight_version` and leaves `computed_for` behind
(no retry-loop). Staleness: `is_stale => false` for `PosSweep`; the merge's internal version-check does the
discard (identical to `Reparse`).

### 6.5 Timing (near-copy of the reconcile debounce)

- `pub const POS_SWEEP_DEBOUNCE_MS: u64 = 300;` (own const; reconcile's 150 ms inner loop is faster — the
  reconcile must settle the tree before the sweep reads it, so a slightly longer debounce is correct).
- **Arm in `app.rs advance()`**, in the same version-latched block as reconcile/on_change, but ONLY when a
  lens is active. The arm condition mirrors reconcile's latch but keys ONLY on `armed_for_version` (NOT the
  compound `due_at.is_none() || …` that reconcile uses): `arm = view.prose_lens.is_some() &&
  pos.in_flight_version.is_none() && pos.computed_for != Some(document.version) && pos.armed_for_version !=
  document.version`. On arm: `pos.due_at = Some(now + POS_SWEEP_DEBOUNCE_MS); pos.armed_for_version =
  document.version`. **CORRECTED per the plan-gate Critical-3 finding:** the reconcile-style
  `due_at.is_none()` OR-escape must NOT be included here — with it, the oversized-doc cap-skip path (which
  clears `due_at`) leaves `computed_for != Some(version)` and re-arms every tick, an idle re-arm loop.
  Dropping the OR-clause (plus `PosStore::default` seeding `armed_for_version = u64::MAX` so a fresh
  version-0 buffer still arms once, and the cap-skip path pinning `armed_for_version = version`) makes the
  arm fire exactly once per version. Edge-triggered on version; `armed_for_version` prevents idle re-arm (a
  settled buffer whose `armed_for_version == version` never re-arms). PosStore therefore needs NO `maybe_stale` field — the
  `computed_for != Some(version)` comparison IS the staleness signal (this is why `computed_for` is
  `Option<u64>`, §6.2 #6).
- **`timers::SUBSYSTEMS` +row `pos_sweep`.** Its `deadline` fn returns `None` unless: a lens is active AND
  `in_flight_version.is_none()` AND `!reconcile.maybe_stale` (converged) AND `due_at` is armed. With no lens
  active → `None` → idle-free.
- **`on_tick` +dispatch:** `if lenses::pos_sweep_due(editor, now) { lenses::dispatch_pos_sweep(editor, ex) }`,
  beside the reconcile dispatch. `pos_sweep_due` re-checks the same gate (lens-active + not-in-flight +
  converged + past-due) at fire time.
- Guardrail test (mirrors `timers::gated_subsystems_yield_none` / `next_wake_none_when_settled`): with no
  lens active, `pos_sweep` deadline is `None`; with a lens active but in-flight, `None`; un-gate → the
  armed deadline reappears (proves the gate is load-bearing in BOTH directions).

---

## 7. Paint + theme

### 7.1 The face

One new `SemanticElement::ProseLensMatch` (variant #35). Composition, grounded in `render.rs
row_spans_placed`: base ladder → MarkedBlock → Selection → Search → **ProseLensMatch** → Diagnostics (last).
The ProseLens face applies BETWEEN Search and Diag — diagnostics stay topmost (errors-you-fix outrank
habits-you-consider; both can co-occur in Review mode).

**AMENDMENT (2026-07-17, ratified after the final Fable whole-branch review): the lens highlight is
SUPPRESSED on glyphs inside the active selection.** The original order above paints `ProseLensMatch` OVER
`Selection`, so on the 14 colored themes whose Selection face is a plain fg/bg swap (no surviving modifier —
tokyo-night, catppuccin, gruvbox, solarized, rosépine, flexoki, blue-jeans variants), a nav-range-selected
match rendered pixel-identical to any other highlighted match, leaving D6's "visible abortable selection"
(§3, D-act) invisible on those themes — a surprise-span-replace risk. Fix: in `row_spans_placed`, apply the
`ProseLensMatch` patch ONLY to glyphs NOT within the current selection. Effect: the currently-selected
(nav-jumped) match reverts to normal Selection styling — visibly a selection on every theme, and distinct
from the other lens-highlighted matches — while all UNselected matches still show the lens highlight.
Diagnostics remain topmost. This satisfies D6 across all themes and is the visual precedence going forward.

- **Color mode:** a bg-tint highlight drawing the eye TO the token (the `SearchMatch` template — a themed
  `bg`, contrast-safe `fg`), one legibility profile. Applied via `style.patch(face_to_ratatui(&face, depth))`
  exactly like the `SearchMatch`/`Selection` arms.
- **Cue / no-color mode:** `bold + italic + underline`. This is the ONLY clean pairwise-distinct unused
  modifier combo — verified against `mono_faces()` and the a11y tests in `theme.rs` /
  `render.rs`: underline=Link, bold+underline=DiagSpelling, italic+underline=DiagGrammar, reverse=SearchMatch/Code,
  reverse+underline=Selection, reverse+bold=SearchCurrent, reverse+bold+underline=MarkedBlock,
  reverse+italic=FrontMatter, bold+italic=StrongEmphasis. `bold+italic+underline` is unclaimed and distinct
  from every same-context face.

### 7.2 Theme tax (completeness contract)

Adding `ProseLensMatch` touches, all in `wordcartel-core/src/theme.rs` unless noted:
- `SemanticElement` enum (+variant).
- `ALL_ELEMENTS` const array: 34 → **35** (and its `[SemanticElement; 34]` → `; 35]` type + the "34 = …"
  comment tally).
- `Theme::face` match (+arm) and `Theme::face_mut` match (+arm) — both exhaustive.
- `struct ThemeFaces` (+field `prose_lens_match`).
- The name-parse map (`match name { … }`, ~line 299/1069 — `"prose_lens_match" => ProseLensMatch`).
- Every built-in theme constructor's `ThemeFaces { … }` literal: `default`, `tokyo`-family, `phosphor`
  (parametric — one line), `no_color`/`mono_faces` (the `bold+italic+underline` cue), terminal-ansi, and the
  ~others (~22 constructors total; the completeness test `face_is_total_and_heading_clamps` enforces it).
- Completeness/a11y tests: extend `no_color_is_monochrome_with_modifier_cues`'s `cued` list, the
  `a11y_every_cued_element_has_a_modifier_in_cue_mode` `cued` list, and add a pairwise-distinctness assertion
  (`ProseLensMatch` vs Selection/Search*/Diag*/MarkedBlock) in `a11y_pairwise_distinct_same_context_pairs`.

### 7.3 Render touch (surgical — hub budget discipline)

`render.rs` production is at **840/900** (`module_budgets.rs` — `assert_hub_budget("src/render.rs", 900)`),
so the render change is ~25–35 lines, and the windowing helper lives in `lenses.rs`, NOT `render.rs`:
- `RowCtx` (+one field: the active-lens windowed match slice, `Vec<PosMatch>` or `&[PosMatch]`).
- `gather_row_ctx`: read `lenses::active_pos_matches(editor)` (the active category's slice iff
  `computed_for == version`); set `use_placed ||= !prose_lens_window.is_empty()`.
- `row_spans_placed`: window the slice to `[lo, hi)` with the diag idiom (`partition_point` on start +
  linear `end > lo`), apply the face between the Search and Diag arms.
- The per-row windowing arithmetic (partition_point bounds) is factored into a `lenses::window_matches`
  helper so render.rs only calls it.

`active_pos_matches(editor) -> Option<&[PosMatch]>` (a near-copy of `active_lens_diags`): returns the active
category's slice iff a lens is active AND `pos.computed_for == Some(document.version)`.

---

## 8. Command surface (IN SCOPE — Rule 8 conformance)

The **command-surface contract** (`docs/design/command-surface-contract.md`) governs. ProseLens is a
multi-state option (4 categories + off), so per **Rule 8**: set-per-state primitives (palette-only,
deterministic for automation) PLUS one stateful representative (a **cycle** for 3+ states), one shared setter
(**Law 6**), registered from a leaf module with NO `Command` enum variant and NO `commands::run` arm (**A14**
anti-regrowth). The `analysis_engine_harper` + `analysis_next` block in `registry.rs` is the EXACT precedent.

- **Shared setter** (Law 6): `lenses::set_prose_lens(editor, Option<ProseLensCategory>)` — sets
  `active().view.prose_lens`, and (when `Some`) arms the sweep. Every command and any profile route through
  it. Mirrors `ventilate::set_ventilate`.
- **5 set primitives** (palette-only, `menu: None`): `prose_lens_adverbs`, `prose_lens_adjectives`,
  `prose_lens_passive`, `prose_lens_weak`, `prose_lens_off`. Each calls `set_prose_lens(editor, Some(cat))`
  / `None`.
- **1 stateful cycle representative:** `register_stateful("prose_lens_next", "Prose Lens",
  Some(MenuCategory::View), |e| MenuMark::Value(<active label or "Off">), |c| { cycle; Handled })`. Cycle
  order Adverbs → Adjectives → Passive → Weak → off → Adverbs. `MenuMark::Value` shows the active lens in the
  menu label. (`register_stateful` + `MenuMark::Value` precedent: `analysis_next`.)
- **2 nav commands** (palette-only): `prose_lens_next_match`, `prose_lens_prev_match`. Near-copies of
  `diag_next`/`diag_prev` over `active_pos_matches`, EXCEPT they commit `Selection::range(to, from)` over the
  whole match span (head-at-start, C-9 — D-act), not `Selection::single`. Order: `unfold_ancestors_of(target)`
  → set range selection → `derive::rebuild` → `nav::ensure_visible`. `next`: first match with
  `range.start > caret`, wrap to `[0]`; `prev`: last with `range.start < caret`, wrap to `[last]`; caret read
  from `selection.primary().to()`. No-op (Info status, no crash) when no lens active or the slice is empty.
- **Registration:** one `lenses::register(r)` call added to `registry.rs`'s registration flow (one line — the
  A14 seam), NOT inline command bodies in `registry.rs`. No `Command` variant; palette-completeness and
  every-option-has-a-command invariant tests (merge GATEs) are satisfied by the primitives + cycle.

**Keybinding hints** auto-resolve from the active `KeyTrie` (registry mechanism) — no manual hint wiring.

---

## 9. Status line — the count

A right-side segment, sibling to `render_status::word_count_segment`: `lenses::prose_lens_count_segment(editor)
-> Option<String>`. Returns `Some("<Label>: <n>")` (e.g. `"Passive: 47"`) iff a lens is active AND
`pos.computed_for == Some(document.version)` — i.e. an honest count against the current text; while a sweep is
in flight or stale, the segment is suppressed (returns `None`) rather than showing a stale number. Count =
the active category's list length (the doc-wide total; all four are computed, but only the active one is
shown, per the single-active spirit). Label from a `ProseLensCategory::label()` (`"Adverbs"`, `"Adjectives"`,
`"Passive"`, `"Weak"`).

The left-side mode label is untouched (the `REVIEW · <lens>` attribution in `status_left_text` is the
diagnostics lens, a different concept). ProseLens is orthogonal to Review mode — a prose lens can be active in
any render mode.

---

## 10. Test corpus (probe-grounded worked examples)

These are real tagger outputs from the compiled probe (harper-brill 2.5.0). They become the classifier's
unit-test fixtures (Task 1). `w/TAG` = token/UPOS.

| Input | Tagger output (key tokens) | Expected |
|-------|----------------------------|----------|
| "The report was written by the committee." | was/AUX written/VERB | Passive `was..written` |
| "Their proposal was rejected by the board." | was/AUX rejected/VERB | Passive `was..rejected` (regular -ed) |
| "Mistakes were made." | were/AUX made/VERB | Passive `were..made` (irregular list) |
| "The door was quickly closed." | was/AUX quickly/ADV closed/VERB | Passive `was..closed` (ADV skipped) |
| "The problem was not solved." | was/AUX not/PART solved/VERB | Passive (PART skipped) |
| "He was never seen again." | was/AUX never/ADV seen/VERB | Passive (ADV skipped, irregular) |
| "The book has been written." | has/AUX been/AUX written/VERB | Passive `been..written` (AUX skipped) |
| "The results were being analyzed." | were/AUX being/AUX analyzed/VERB | Passive `being..analyzed` |
| "The item was defenestrated." | was/AUX defenestrated/None | **Passive** (None + lowercase + -ed, D-passive-edge (i)) |
| "The song was sung at dawn." | was/AUX sung/None | Passive (None + lowercase + irregular `sung`) |
| "She was running to the store." | was/AUX running/VERB | **none** (-ing progressive) |
| "They were writing letters." | were/AUX writing/VERB | none (-ing) |
| "She was happy." | was/AUX happy/ADJ | **Weak** (be token) |
| "He was tired." | was/AUX tired/ADJ | Weak (adjectival participle → dict ADJ) |
| "He is a doctor." | is/AUX a/DET doctor/NOUN | Weak (be token) |
| "There are three problems." | There/PRON **are/VERB** three/NUM | **Weak** (existential — surface be-match, NOT AUX) |
| "There is a problem." | **is/VERB** a/DET problem/NOUN | Weak (existential surface-match) |
| "It was in the room." | was/AUX in/ADP … | Weak (locative) |
| "It was Fred." | was/AUX Fred/PROPN | Weak (PROPN, not Passive) |
| "Where have you been?" | have/AUX you/PRON been/AUX ?/PUNCT | **none** (terminal be, D-passive-edge (ii)) |
| "All I did was call her." | did/AUX was/AUX call/VERB her/PRON | none (base-form VERB → pseudo-cleft) |
| "It's written by hand." | It's/PRON written/VERB | **none** (contracted be invisible — documented miss §5.1.5) |
| "The extremely tall man moved very quietly." | extremely/ADV tall/ADJ … very/ADV quietly/ADV | Adverbs: {extremely, very, quietly}; Adjectives: {tall} |

Multi-sentence sanity: "It was written. She was happy. He was running." → sentence 1 Passive, sentence 2
Weak, sentence 3 none. (Segmentation via S7's `sentence_spans`; `analyze` yields one `TaggedSentence` per
content sentence.)

---

## 11. Arc laws & global constraints honored

- **D6 (COLOR + SELECT, never mutate without a visible abortable selection):** ProseLens paints + nav
  range-selects the whole match; the range-selection IS the visible abortable selection; the writer mutates
  via ordinary editing (typing replaces the selection — pinned by `commands.rs` tests). No operators.
- **D7 (cold-path only):** nothing on by default; the sweep + paint run ONLY when a lens is active. No
  per-keystroke POS work.
- **O(visible) + O(edited) / version-cached:** paint windows the visible span (`partition_point`); the sweep
  is edge-triggered on edit-version, debounced (300 ms), version-discarded on staleness (reconcile pattern).
- **Resource law (free at rest):** with no lens active, ZERO sweep work — the `pos_sweep` timers deadline is
  `None`, no worker, no wake. Cost is proportional to lens-active + edits, never idle duration. Backed by the
  gated-subsystem-yields-none guardrail (§6.5).
- **Anti-regrowth GATEs (`too_many_lines`=100, `module_budgets`):** the sweep/store/commands/count live in
  the leaf `lenses.rs`; render.rs gains ~25–35 lines (840→<900); registry.rs gains one `lenses::register`
  line; app.rs gains one arm-block clause (914→<1000). No dispatcher grows a body.
- **Command-surface contract:** conformance stated (§8) — Rule 8 primitives + cycle, Law 6 shared setter,
  A14 leaf-module registration, no `Command` variant. The plan MUST restate this conformance and the invariant
  tests (palette-completeness, every-option-has-a-command, hint re-resolution) are merge GATEs.
- **Theme completeness contract:** the new `SemanticElement` extends `ALL_ELEMENTS` + count + all constructors
  + the no-color modifier-cue contract (§7.2); the cue survives cue mode as a pairwise-distinct
  `bold+italic+underline`.
- **Lazy-reparse invariant:** the sweep follows the ACTIVE buffer only; non-active buffers are never eager-swept.

---

## 12. TDD task sketch (~7 tasks, sized MID)

Each task is a near-copy of an existing pattern; failing test → impl → green → commit; per-task reviewer.

1. **`wordcartel-nlp` classifier (pure).** `ProseLensCategory` mapping is done in the shell, but the passive/
   weak/adverb/adjective CLASSIFICATION of a `&[TaggedSentence]` (+ the buffer slice for surface tests) is pure
   linguistics → lives in `wordcartel-nlp` for property-testability. Implement `BE_FORMS`, `SKIP`, the forward
   scan, the target table, `IRREGULAR_PARTICIPLES`, honoring the crate-doc note ("S8 maps UPOS to its own
   theme `SemanticElement` in the shell" — the theme mapping stays in the shell; the *rule* is here). Tests =
   the §10 corpus as failing-first fixtures. Property test: disjointness (no be double-flags), no panic on
   multibyte/degenerate input.
2. **Theme element.** `SemanticElement::ProseLensMatch` + `ALL_ELEMENTS` 35 + `face`/`face_mut` + `ThemeFaces`
   field + name-parse map + all constructors + the `bold+italic+underline` cue. Extend the completeness/a11y
   tests. GATE: `face_is_total`, `no_color_is_monochrome_with_modifier_cues`, the two a11y tests.
3. **`lenses.rs` leaf — store + state + commands + count.** `PosStore`, `PosMatch`, `Buffer.pos`,
   `View.prose_lens`, `set_prose_lens` (Law 6), the 5 set primitives + `prose_lens_next` cycle + 2 nav
   commands, `active_pos_matches`, `prose_lens_count_segment`, `window_matches` helper, `ProseLensCategory::label`.
   One `lenses::register(r)` line into registry.rs. Tests: Rule-8 conformance (palette-completeness,
   every-option-has-a-command), nav range-selects the whole span + wraps + no-ops off-lens, count gated on
   `computed_for == version`.
4. **Sweep + JobKind + panic arm + timers.** `JobKind::PosSweep` (+`is_stale` arm), `dispatch_pos_sweep`
   (prose-range enumeration + snapshot + version-checked merge), `apply_panic` `PosSweep` arm (compiler-forced),
   `timers::SUBSYSTEMS` `pos_sweep` row + `on_tick` dispatch + `app.rs advance()` arm-block clause,
   `POS_SWEEP_DEBOUNCE_MS`, `LENS_MAX_SWEEP_BYTES`. Tests: reconcile's battery shape — sweep converges,
   version-discard, panic clears in-flight without retry-loop, idle-free guardrail (gated-subsystem-yields-none),
   prose-only (code fence "is" NOT flagged), converged-tree gate.
5. **Render.** `RowCtx` field + `gather_row_ctx` read + `use_placed` + `row_spans_placed` face between Search
   and Diag. Fixtures: highlight in color mode + cue-mode modifier + composition with Selection/Search/Diag +
   ventilate origin correctness (the `origin_of` rebase). GATE: `module_budgets` render.rs < 900.
6. **Nav + count wiring end-to-end.** (Folded with Task 3 if small; kept separate if the nav's selection
   semantics + count segment need their own integration fixtures against a real `Editor`.)
7. **e2e journey + polish.** An in-process `e2e.rs` journey: open a doc, activate the Passive lens, assert the
   count segment + that a flagged span paints, `prose_lens_next_match` range-selects it, typing replaces it, the
   count updates after the next sweep, cycling to off clears paint + count and arms no timer. PTY smoke check if
   warranted. Final gates: Codex pre-merge + Fable whole-branch probe.

---

## 13. Open questions for the gate

None blocking. Two implementation-detail choices deferred to the plan (not design forks):
- `PosMatch` `Copy` vs `Clone` (Range<usize> is not Copy; use `{start, end}` fields or keep Clone).
- Whether Task 6 folds into Task 3 (sizing call).
