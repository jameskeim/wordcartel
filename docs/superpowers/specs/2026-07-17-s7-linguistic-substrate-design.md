# S7 — Linguistic substrate (harper-brill in-process) — design note (spec)

**Date:** 2026-07-17
**Effort size:** S–M (small–medium) — ~6 tasks. One NEW pure crate `wordcartel-nlp`
(tokenization bridge + `analyze`), one shell field (`Buffer.nlp: NlpStore`) + a leaf query
helper, `deny.toml` exceptions, workspace plumbing. No `wordcartel-core` change beyond the
existing `textobj::sentence_spans` it reuses.
**Anchor:** S7 (backlog theme S — prose-structure arc; `docs/design/prose-structure-arc.md`,
S7 row of the arc table and the "S7 — Linguistic substrate" section; marker
`<!-- item: S7 -->` in `docs/ux-backlog.md`).
**Severity:** substrate-only. **No data-loss surface** (produces derived data; mutates no
buffer text, selects nothing, paints nothing). **No new hot-path class** (cold-path only —
D7). **Zero idle/background work** (no timers row; pull-based).
**Grounding packet:** `scratchpad/s7-linguistic-substrate/grounding-packet.md`.
**Grounding report:** returned to the coordinator (this warm thread). The harper-brill API,
the tokenization bridge, and the cost measurements in §4–§6 below are lifted from it,
re-anchored on real symbol names and cross-checked against a compiled scratchpad probe
(`scratchpad/s7-linguistic-substrate/probe/`, run against the real `harper-brill = "=2.5.0"`).
**Human rulings folded in (coordinator, 2026-07-17), decided-and-closed:**
D-a crate placement = new pure crate; D-b cache = synchronous pull-based single-slot memo
(the grounding deviation, adopted — no JobKind); D-c the three tokenization hazards each get a
settled decision. See §2.

**Command-surface contract:** **N/A — does not touch the command surface.** S7 is a
substrate: it adds and removes no commands, adds no user-settable option, and does not change
the registry (`registry.rs`), the palette, the menu structure (`menu::grouped_commands` /
`registry::MENU_ORDER`), or keybinding hints. It paints nothing (no `SemanticElement`
variant is added to `wordcartel-core/src/theme.rs`), selects nothing (no `Selection` call),
and mutates nothing (no `editor.apply` / `edit_apply` path). It follows the **A14
anti-regrowth precedent**: a leaf module with **no `Command` enum variant and no
`commands::run` arm** (per the arc doc §6 hazard 8 and hazard note "Follow the A14 precedent").
The contract's invariant tests (palette-completeness, every-option-has-a-command, hint
re-resolution) are therefore unaffected. Both this spec and the plan carry this line
explicitly. **S8** — not S7 — brings the lenses, the `SemanticElement` additions, the
selection operators, and any command surface.

---

## 1. Problem statement

The prose-structure arc's genuinely novel half — S8's "prose lenses" (every adverb dimmed,
every passive underlined, every nominalization flagged; `Phrase`/`Clause` objects) — needs a
part-of-speech + noun-phrase signal over the caret's prose that **no existing wordcartel
capability provides**. The arc doc §5 establishes why harper-ls cannot supply it: harper-ls is
a **subprocess** speaking diagnostics-only LSP (`harper_ls.rs`, `publishDiagnostics` /
`codeAction`) — "there is no LSP method that returns a parse," and the two consumers are
separated by a process boundary (whole-document / debounced / `Review`-gated / out-of-process
vs. caret-local / synchronous / always-available). **The shared substrate can only exist
in-process — which is S7.**

S7 wraps `harper-brill`'s rule-based Brill POS tagger + NP chunker in-process to produce, over
the caret's block window, **POS tags + NP-membership flags byte-aligned to our buffer**,
cold-path and version-cached, for S8 (and S4's objects, and eventually a native
stylistic-diagnostic provider) to consume. S7 ships **only** the substrate + cache. It is
useless on its own by design; that is the point of decomposing it from S8.

---

## 2. Decisions (human-ratified — closed, not open forks)

### D-a — Crate placement: a NEW pure crate `wordcartel-nlp`

The wrapper lives in a **new workspace member `wordcartel-nlp`** (pure, deterministic, no IO),
depended on **shell → nlp → core** (one-way). It is **not** in `wordcartel-core` and **not**
in the `wordcartel` shell.

**This supersedes the arc doc's literal wording.** `docs/design/prose-structure-arc.md`'s S7
section says "`harper-brill` behind a **wordcartel-core module**." That sentence is **amended
by this spec** to "a new pure crate `wordcartel-nlp`." The spec's execution includes editing
the arc doc's S7 sentence to record the amendment inline (with a one-line pointer to this
spec's date/path), so the arc doc and the shipped structure do not drift.

**Rationale (fuzz-cycle evidence, grounded):**
- **The fuzz harness path-deps core directly.** `wordcartel-core/fuzz/Cargo.toml` carries
  `[dependencies.wordcartel-core] path = ".."` and is a **detached** workspace built via
  `cargo +nightly fuzz` with `--cfg fuzzing`. A `wordcartel-core` module would drag
  `harper-pos-utils`'s mandatory `burn` compile subtree into **every nightly + sanitizer fuzz
  build** of the F1 (`apply_pipeline`) and F2 (`block_tree`) targets — and CLAUDE.md lists the
  F2 incremental-soundness tail as still-open, so that cycle is live.
- **Core is the gated trust anchor.** `wordcartel-core/src/lib.rs` carries
  `#![forbid(unsafe_code)]`, the H7 cast-soundness denies
  (`cast_possible_truncation`/`cast_sign_loss`/`cast_possible_wrap`), and H17
  `#![warn(missing_docs)]`. Keeping burn out of core keeps its build/test/fuzz cycle lean.
- **Dependency direction is clean.** `wordcartel-nlp` depends on `wordcartel-core` **only** for
  `textobj::sentence_spans` (the S5 sentence authority — arc hazard 1 forbids introducing a
  fourth sentence authority). The reverse never happens: core never learns about POS. The fuzz
  crate keeps path-depping only core, so **burn never enters a fuzz build**.
- **S7's output needs no core types.** Canonical positions in core are plain `usize` byte
  offsets (`lib.rs` header: "Canonical position = byte offset (usize)"); S7's result types use
  `Range<usize>`/`(usize, usize)` and harper's `UPOS`. So `wordcartel-nlp`'s public surface
  introduces no dependency on core *types* — only on the one `sentence_spans` function.
- **Placement is build-neutral for the shell.** The shell links the tagger regardless of
  placement, so the workspace build compiles burn once either way (measured cold build 18.6s;
  target artifacts 319 MB). Placement changes only whether the **fuzzed core** carries it.
  Option (c) — the shell — additionally puts a pure deterministic analyzer in the imperative
  shell, against the functional-core split, for zero savings.

### D-b — Cache: synchronous, pull-based, per-buffer single-slot memo (no JobKind)

**The grounding deviation is adopted.** The `reconcile.rs` `JobKind::Reparse` / `ReconcileStore`
debounce / worker round-trip pattern exists for **O(document)** work; S7's per-window work is
**O(visible)** and measured at **≤~400 µs** for a 40-sentence window (≈10 µs/sentence tag+chunk
— see §6) — cheaper than a job dispatch's machinery. Therefore:

- **No `JobKind`, no `timers.rs` `SUBSYSTEMS` row, no debounce, no worker.** S7 adds nothing to
  `jobs.rs`, `reconcile.rs`, or `timers::SUBSYSTEMS`.
- **A per-buffer single-slot memo `NlpStore { window, computed_version, sentences }`** lives on
  `Buffer` beside its existing `diagnostics: DiagStore` and `reconcile: ReconcileStore` fields
  (`editor.rs`, `struct Buffer`). It is **`valid_for(version, window)`**-shaped, mirroring
  `diagnostics_run::SourceSlot::valid_for` (which checks `computed_version == version`): a query
  recomputes on a miss (window moved or version advanced), returns the memo on a hit. Every
  static repaint frame while a lens is parked over the same window is a memo hit.
- **This satisfies every arc law more strictly than a worker would** (see §3): it runs **only
  when queried** (D7), scans only the window (O(visible)), invalidates on version bump
  (version-cached), and does **zero idle/background work** because there is no timers row to
  fire on an idle wake — the strongest form of the resource-behavior law ("Idle is free").

**Forward note (out of scope for S7):** S8's doc-wide sweeps ("select every sentence containing
a passive") are O(document) and will want to move off the query thread — S8 will add a
`JobKind` **then**. Because `analyze` is a pure free function and `brill_tagger()` /
`brill_chunker()` return `Arc<…>` handles that are `Send + Sync` (they back
`static LazyLock`s), moving a clone onto the job substrate is a clean drop-in. S7 does not
build that; it only leaves the door open.

### D-c — The three tokenization hazards (each settled, with a test obligation)

Grounded in the shipped model JSON (`harper-brill-2.5.0/trained_tagger_model.json`, a
`FreqDict.mapping` of 26,900 lowercased words + 201 Brill patches) and the probe run:

1. **Joined contractions tag `None` — ACCEPTED as a documented known quality floor.** harper's
   `FreqDictBuilder::inc_from_conllu_file` ingests raw CoNLL-U token forms, but its training
   join-rule (`brill_tagger/mod.rs` epoch: `"n't" | "'ll" | "'ve" | …` are appended to the
   previous token) means the **joined** forms are absent from the shipped dict: `"don't"`,
   `"isn't"`, `"can't"`, `"doesn't"`, `"committee's"` all map to `None` (verified in the model
   JSON). harper-core papers over this with its curated-dictionary `infer_pos_tag()` fallback,
   which S7 deliberately does **not** carry (no harper-core dep). **We accept `None` for these.**
   Rationale: the tags S8 actually relies on are solid — `was`/`is`/`been`/`being`/`were` →
   `AUX`, `very`/`quickly`/`really` → `ADV`, `taken`/`given`/`made` → `VERB` (all verified in
   the dict) — so the S8 passive (`AUX` + participle `VERB`) and adverb (`ADV`) lenses have
   their inputs. **Test obligation:** a fixture asserting `it's → PRON` (present) and that a
   `None`-tagging contraction is tolerated (no panic, parallel-length preserved) — pinning the
   floor so a future dict swap that changes it is a visible diff.
2. **Curly apostrophe `’` — NORMALIZE to `'` in the lookup string only.** The dict has ~112
   straight-apostrophe entries vs. ~17 curly (verified), so `don’t`/`it’s` typed with a
   typographic apostrophe would miss even the entries that exist. UAX-29
   `split_word_bound_indices` keeps `don’t` as one segment, so **byte spans are unaffected** —
   we normalize `’ → '` **only in the `String` handed to `tag_sentence`**, never in the recorded
   span. **Test obligation:** a fixture with a curly-apostrophe token asserting its recorded
   byte span is unchanged and its tag matches the straight-apostrophe form.
3. **Markdown underscore adornment — STRIP-THEN-TEST the `_` runs.** UAX-29 glues underscores:
   `_quiet_` is **one** segment → tags `None`. (Asterisks and backticks split off as their own
   PUNCT segments and are harmless.) The rule is **strip-then-test**, deliberately in that order
   (a leading-`_` token is not alphanumeric-first, so a test-then-strip rule would never fire on
   `_quiet_`): for each kept segment, compute the underscore-stripped inner via
   `trim_matches('_')`; **if** the inner is non-empty AND differs from the segment (there were
   `_` runs) AND the inner's first char is alphanumeric, **narrow the recorded byte span to the
   inner's span** and use the inner as the lookup base; **otherwise** leave the segment and its
   span unchanged (a pure-`_` token strips to empty → left as-is; a non-word token is untouched).
   **Test obligation:** a fixture asserting `_quiet_` yields a token whose span is the inner
   `quiet` and whose tag is `ADJ` (the dict value for `quiet`).

---

## 3. Arc laws & global constraints this spec honors

- **D6 (NLP must never mutate) — trivially honored.** S7 produces derived data (`Vec<TaggedSentence>`);
  it holds no path to `editor.apply` / `edit_apply`, no `Selection` call, no `SemanticElement`.
- **D7 (cold-path only) — honored.** `analyze` runs **only when a consumer queries** the memo
  (S8 lens active or a command). It is never wired into the per-keystroke path, `reduce`, the
  render loop's unconditional work, or a `timers::SUBSYSTEMS` deadline. The arc doc §6 hazard 7
  list of O(document) paths that must not become per-keystroke (`nav::leaf_spans`,
  `outline::sections`) is untouched — S7 adds no new O(document) per-keystroke path.
- **O(visible) + O(edited) — honored.** `analyze` scans the caret's block window slice (via
  `commands::prose_window_at`), never the document. Cost is proportional to window size.
- **Version-cached — honored.** `NlpStore::valid_for(version, window)` invalidates on a
  `document.version` advance or a window move, mirroring `SourceSlot::valid_for`.
- **Resource behavior (proportional to work, free at rest) — honored in its strongest form.**
  Because there is **no timers row**, an idle wake dispatches no S7 work; a settled buffer with
  a parked lens does memo hits only on real repaints. Zero disk, zero background compute at rest.
- **Anti-regrowth GATEs (`clippy::too_many_lines`=100, `tests/module_budgets.rs`) — honored.**
  The query helper is a leaf; no growth in `registry.rs` / `commands.rs` (both already carry
  `too_many_lines` allows — the arc doc §6 hazard 8 names them as the path of least resistance
  to avoid). No `Command` variant, no `commands::run` arm (A14 precedent).

### Global constraints (record verbatim)

- **`harper-brill` pinned `=2.5.0`.** The embedded trained models (`trained_tagger_model.json`,
  `trained_chunker_model.json`) are `serde_json`-deserialized artifacts coupled to
  `harper-pos-utils`'s serde shapes; an exact pin (repar precedent) prevents a silent model /
  schema drift on `cargo update`.
- **Two `deny.toml` exceptions to add** (mirroring the existing quick-xml `{ id, reason }`
  precedent already in `[advisories].ignore`):
  - `[advisories].ignore +=`
    `{ id = "RUSTSEC-2024-0436", reason = "paste unmaintained; build-time proc-macro pulled via burn-ndarray in harper-pos-utils; no vulnerability, no safe upgrade, unavoidable with burn" }`
  - `[licenses].allow += "CC0-1.0"` with the comment
    `# tiny-keccak is CC0-1.0 (public-domain dedication; more permissive than the allowed MIT/Apache); pulled via burn's tree.`
- **`cargo deny check` is a RELEASE-CHECKLIST step, NOT a merge gate.** A red result never
  blocks a merge (CLAUDE.md + `deny.toml` header). The effort's pre-merge report runs it once
  and **records** the result (clean, or the finding), but the merge gates remain `cargo test` +
  workspace clippy (deny) + `too_many_lines` + `module_budgets`.
- **`deny.toml`'s `[licenses]` comment "(287 packages; burn removed)" must be refreshed** when
  burn re-enters the tree with this effort — the package count and the "burn removed" note are
  now stale and would mislead the next supply-chain pass.
- **H2-rationale re-measure obligation (record for the linters effort).** H2 justified the
  harper-ls subprocess split as "drop the ~389-crate tensor stack from the binary." With S7,
  `burn` is **in the linked workspace binary anyway** (compiled; the rule-based paths
  dead-code-eliminate the *neural weights* from the link — verified: the probe binary is 1.5 MB,
  no embedded `.mpk`/`vocab` — but the burn **crates** compile). The dep-weight half of the
  subprocess-split rationale is therefore partly spent and **must be re-measured, not assumed**,
  before it is defended on dep-weight grounds again.

---

## 4. The harper-brill API (grounded — cite symbol names)

Read from `harper-brill-2.5.0/src/lib.rs` and `harper-pos-utils-2.5.0/src/…`:

- **Entry points (rule-based only):**
  `harper_brill::brill_tagger() -> Arc<BrillTagger<FreqDict>>` and
  `harper_brill::brill_chunker() -> Arc<BrillChunker>`. Both are process-wide `LazyLock`
  statics deserialized once from `include_str!`-embedded JSON (tagger 644 KB = FreqDict +
  201 patches; chunker 52 KB = 200 patches). **`burn_chunker()` is NEVER called** — it is the
  neural `CachedChunker<BurnChunkerCpu>` path that `load_from_bytes_cpu`s the 787 KB `model.mpk`
  + 613 KB `vocab.json`; we do not use it, and not calling it lets the linker DCE those weights.
- **`Tagger::tag_sentence(&self, sentence: &[String]) -> Vec<Option<UPOS>>`**
  (`tagger/mod.rs`). Input is caller-supplied token **strings**, one sentence at a time;
  output is one `Option<UPOS>` per input token (`None` when the tagger can't decide). `FreqDict::get`
  lowercases internally; the base `FreqDict` tags each token by most-frequent UPOS, then
  `BrillTagger::apply_patches` rewrites via the 201 transformation rules.
- **`Chunker::chunk_sentence(&self, sentence: &[String], tags: &[Option<UPOS>]) -> Vec<bool>`**
  (`chunker/mod.rs`). Input is the **same** token strings **plus** the tags from the tagger;
  output is one `bool` per token — `true` = the token is a component of a noun phrase.
- **`harper does not tokenize.`** There is no alignment hazard by construction: the caller owns
  tokenization and the byte spans; harper returns vectors **parallel** to the caller's token
  slice. This is the whole feasibility crux, and it lands in our favor.
- **No extra harper dependency.** `harper-brill`'s manifest deps are exactly `harper-pos-utils`
  + `serde_json`. **No `harper-core`.** (`burn` is a non-optional dep of `harper-pos-utils`, so
  it compiles regardless, but burn's direct `use` is confined to `chunker/burn_chunker.rs`;
  `chunker/mod.rs` only declares `mod burn_chunker` and re-exports its types — it does not
  itself `use burn`. The tagger and `BrillChunker` never reference burn; verified.)
- **`UPOS`** (`upos.rs`) is the 16-variant Universal POS tagset (`ADJ ADP ADV AUX CCONJ DET INTJ
  NOUN NUM PART PRON PROPN PUNCT SCONJ SYM VERB`), `#[derive(… Copy, Serialize, Hash, Ord …)]`,
  with `is_nominal()`. `Arc<BrillTagger>` / `Arc<BrillChunker>` are `Send + Sync` (they back
  `static LazyLock`s), which is what makes the S8 job-substrate drop-in clean.
- **Reference caller convention** (harper-core `document.rs::parse`, which we mirror, NOT
  depend on): for each sentence it collects **all non-whitespace tokens** (words AND
  punctuation), case preserved, and passes them to `tag_sentence` then `chunk_sentence`.
  Punctuation tokens are load-bearing: `PatchCriteria` (`patch_criteria.rs`) key on
  relative-index neighbours including `PUNCT`, so dropping punctuation would change the tags of
  adjacent words. **S7 keeps punctuation tokens in the sentence slice** for this reason.

---

## 5. The tokenization bridge (the highest-value section)

The bridge converts a **block-window text slice** (already prose-scoped by the shell) into
harper's per-sentence `&[String]` input and maps harper's parallel output vectors back to byte
offsets **in the slice's coordinates**. The shell then rebases by the window origin `ps`.

### 5.1 Algorithm (`wordcartel-nlp`, pure)

For the input `text: &str` (the window slice), `analyze(text)` does:

1. **Sentence segmentation — reuse the S5 authority.** Iterate
   `wordcartel_core::textobj::sentence_spans(text)` → each `(from, to)` is a content-only
   sentence span (no trailing whitespace; the S5 four-rule post-pass over UAX-29). **No new
   sentence authority is introduced** (arc hazard 1).
2. **Per sentence, tokenize with UAX-29.** Over the sentence sub-slice `&text[from..to]`, walk
   `unicode_segmentation::UnicodeSegmentation::split_word_bound_indices` — the **same** UAX-29
   engine `wordcartel-core::textobj` already uses (`split_word_bound_indices` in `word_bounds` /
   `next_word_start`). For each `(rel_start, seg)`:
   - **Whitespace-skip:** if `seg.trim().is_empty()`, skip it (gap material — the same
     convention `SentenceSpans::pull` uses: "whitespace-only UAX segments are skipped").
   - Otherwise keep it as a token. Its base byte span is **sentence-local**
     `(rel_start, rel_start + seg.len())`, stored in **slice-local** coordinates by adding
     `from`: `range = (from + rel_start .. from + rel_start + seg.len())`.
   - **Underscore strip-then-test (D-c hazard 3):** compute `inner = seg.trim_matches('_')`.
     **If** `inner` is non-empty AND `inner != seg` (there were `_` runs) AND `inner`'s first
     char is alphanumeric, narrow both the recorded `range` and the lookup base to `inner`'s span
     (the leading `_` run length is added to the start; `inner.len()` sets the width).
     **Otherwise** leave the segment and its `range` unchanged (a pure-`_` token trims to empty →
     left as-is; a non-word segment such as `*` or `` ` `` is untouched). This order is
     deliberate: a leading-`_` token is not alphanumeric-first, so a test-*then*-strip rule would
     never fire on `_quiet_` (the Codex-caught self-defeating order). **The predicate is inlined
     in `wordcartel-nlp`** — a one-liner (first char alphanumeric via `char::is_alphanumeric`)
     — **not** `wordcartel-core`'s `is_word`, which is **private** (`textobj::is_word` has no
     `pub`; verified). Inlining keeps D-a's minimal-core-coupling literally true
     (`textobj::sentence_spans` stays the SOLE core dependency, verified `pub` at
     `textobj::sentence_spans`) and adds no H17 doc burden to core. The predicate is only a
     lookup gate, **not** a segmentation authority, so there is no drift risk against the S5
     word/sentence engine.
   - **Apostrophe-normalize (D-c hazard 2):** build the **lookup string** for this token from the
     (possibly narrowed) inner by replacing `’` (U+2019) with `'`. This affects only the `String`
     pushed into the sentence's token vector; the recorded `range` is untouched.
3. **Tag + chunk.** Call `tagger.tag_sentence(&tokens)` then
   `chunker.chunk_sentence(&tokens, &tags)`. The tagger/chunker handles come from
   `brill_tagger()` / `brill_chunker()` (cheap `Arc` clones of the process statics).
4. **Assemble.** Zip the three parallel vectors (token ranges, `tags`, `np_flags`) into
   `Vec<TokenTag>`, and wrap in a `TaggedSentence { span: (from, to), tokens }`. Collect over all
   sentences → `Vec<TaggedSentence>`.

### 5.2 The parallel-length invariant

`tokens.len() == tags.len() == np_flags.len()` for every sentence — guaranteed by harper's
contract (`tag_sentence`/`chunk_sentence` return one element per input token) and by our
building `tokens` and the span vector in the same loop. **This is a test obligation** (assert
the three lengths agree for multi-token, empty, and punctuation-only sentences) and the zip in
step 4 relies on it — no index arithmetic beyond the zip, so no off-by-one class.

### 5.3 Byte-rebase (shell side)

`analyze` returns spans in the **window-slice's** coordinates (offset 0 = the slice start).
The shell's query helper obtains the window `(ps, pe)` from `commands::prose_window_at(editor, h)`
and analyzes `&buf.slice(ps..pe)`; to report absolute buffer offsets it adds `ps` to every
`TokenTag.range` and to each `TaggedSentence.span` — **exactly** the `ps + sf` / `ps + st`
convention `commands::prose_sentence_at` already uses ("SEE==SELECT single-source"). Keeping the
rebase in the shell (not in `analyze`) preserves the pure crate's slice-relative contract and
makes `analyze` trivially unit-testable on bare strings.

### 5.4 Worked multibyte fixtures (from the probe run — become tests)

All byte offsets below are slice-local, verified by the compiled probe:

- **`"The café in Zürich serves espresso — it's excellent."`** — `café` occupies bytes `4..9`
  (`é` is 2 bytes); `Zürich` `13..20` (`ü` 2 bytes); the em-dash `—` (`37..40`, 3 bytes) is its
  own PUNCT token; `it's` (`41..45`) tags `PRON`. No token span splits a codepoint. (`café` /
  `Zürich` / `espresso` tag `None` — accepted floor; they are not lens inputs.)
- **`"\u{201c}Why?\u{201d} he asked the tall man quietly."`** — the curly quotes `"`/`"` are
  their own 3-byte PUNCT tokens (`0..3`, `7..10`); `quietly` tags `ADV` (an S8 adverb-lens
  input). Confirms multibyte openers/closers don't corrupt spans.
- **`"This is **bold** and _quiet_ prose with `code` in it."`** — the `*` and `` ` `` split off as
  individual PUNCT tokens; `bold` tags `ADJ`; **`_quiet_` is one UAX segment** → the
  strip-then-test rule (D-c 3) computes `inner = "quiet"` (`!= "_quiet_"`, non-empty,
  alphanumeric-first), narrows the span to the inner word, and it tags `ADJ` instead of `None`.
  Confirms the strip-then-test decision on a real adornment case.
- **`"The quick brown fox doesn't jump over the lazy dog."`** — `doesn't` (`20..27`) tags `None`
  (accepted floor); the surrounding `DET/ADJ/NOUN/VERB/ADP` tags and the `the lazy dog` NP
  (`np=true` run) are correct — confirms one `None` token does not derail its neighbours.

### 5.5 Result types & re-export

```rust
pub use harper_brill::UPOS;               // re-export, NO newtype (D-a: no core type needed)

pub struct TokenTag {
    pub range: std::ops::Range<usize>,    // byte span in the analyzed slice's coordinates
    pub upos: Option<UPOS>,               // None where the tagger can't decide (accepted)
    pub np: bool,                         // noun-phrase membership (chunker flag)
}

pub struct TaggedSentence {
    pub span: (usize, usize),             // the S5 content-only sentence span, slice-local
    pub tokens: Vec<TokenTag>,
}

pub fn analyze(text: &str) -> Vec<TaggedSentence>;
```

**`UPOS` is re-exported, not newtyped** (D-a): it is the standard UD tagset, `Copy`, stable, and
under the new-crate placement it never enters `wordcartel-core`. S8's paint variants
(`Adverb`, `Passive`, …) are additions to the shell/theme's own `SemanticElement`
(`wordcartel-core/src/theme.rs`, an exhaustive `enum` + `Theme::face` match — the compile-forced
seam), mapped from `UPOS` in the shell **in S8**, not here.

---

## 6. Cache design (shell side)

- **`NlpStore` on `Buffer`** (`wordcartel/src/editor.rs`, `struct Buffer`, beside
  `diagnostics: DiagStore` and `reconcile: ReconcileStore`):

  ```rust
  pub struct NlpStore {
      window: (usize, usize),        // absolute buffer byte span the memo was computed for
      computed_version: u64,         // document.version the memo reflects
      sentences: Vec<TaggedSentence> // spans rebased to ABSOLUTE buffer offsets before storing
  }
  impl NlpStore {
      fn valid_for(&self, version: u64, window: (usize, usize)) -> bool {
          self.computed_version == version && self.window == window   // + non-empty guard
      }
  }
  ```
  Shape mirrors `diagnostics_run::SourceSlot::valid_for` (`computed_version == version`).
- **Query helper `nlp_window_at(editor, h)`** (a leaf module — e.g. `wordcartel/src/nlp.rs`;
  A14 leaf, no `commands::run` arm): resolve `(ps, pe) = commands::prose_window_at(editor, h)?`
  (returns `None` off-prose — the object layer's "I don't know" per arc hazard 6); if the memo
  `valid_for(document.version, (ps, pe))`, return it; else `analyze(&buf.slice(ps..pe))`, rebase
  every span by `ps`, store into `NlpStore`, and return it. **On a memo hit, `analyze` is not
  called** (the whole point). The helper is the only public shell entry S8 uses.
- **No timers, no worker, no invalidation callback.** Invalidation is *lazy*: the next query
  recomputes because `valid_for` fails after a version bump or window move. Nothing runs at
  rest.

NP-chunk noise (e.g. a sentence-final `.` flagged `np=true`, observed in the probe) is
**accepted for the substrate** — the rule-based `BrillChunker` is harper's lesser chunker
(their production `parse` uses `burn_chunker`, which we reject). S8's `Phrase` object gets its
**own** precision look before it ships (the D5 stance: select-only, behind a measured gate);
S7 just surfaces the raw flags.

---

## 7. Scope boundary (confirm)

**S7 is substrate-only.** In scope: the `wordcartel-nlp` crate (bridge + `analyze` + `UPOS`
re-export), the `Buffer.nlp: NlpStore` field + `valid_for`, the `nlp_window_at` query helper,
the two `deny.toml` exceptions + the stale-comment refresh, the arc-doc amendment, workspace
plumbing, and tests. **Out of scope (all S8/S4):** any `SemanticElement` variant, any paint,
any `Selection`/operator, any `Command`/palette/menu/keybinding, any doc-wide sweep / `JobKind`,
the `Phrase`/`Clause` objects, the clause precision gate (D5). **Command surface: N/A** — see
the header; both spec and plan state it. No warm/enable seam is needed: the 2.2 ms lazy init at
first query (measured) is negligible on a cold path.

---

## 8. Cost measurements (probe, release, real crates)

- **Init:** `brill_tagger()` 2.2 ms + `brill_chunker()` 69 µs, one-time (LazyLock deserialization
  of the embedded JSON), amortized across the process.
- **Per sentence:** ~10 µs tag+chunk (4,000 sentence tag+chunk in 39 ms). A 40-sentence visible
  window ≈ 400 µs — a cold-path query, not a per-keystroke cost.
- **Binary weight:** the probe binary is 1.5 MB (≈700 KB models + code); the 1.4 MB neural
  weights are **absent** from the link (DCE, because `burn_chunker()` is never referenced) —
  confirming the rule-based paths keep the runtime lean. The burn **crates** still compile
  (build-time passenger; ~18.6 s cold, 319 MB target artifacts).

---

## 9. TDD task sketch (~6 tasks, sized S–M)

1. **Workspace plumbing.** New `wordcartel-nlp` member (root `Cargo.toml` `members`;
   `[lints] workspace = true`; `version.workspace = true`; `harper-brill = "=2.5.0"`). Add the
   two `deny.toml` exceptions + refresh the "(287 packages; burn removed)" comment. Amend the
   arc-doc S7 sentence. *Test/verify:* workspace `cargo build` + `cargo test --no-run` clean;
   record `cargo deny check` output (release-checklist, not a gate).
2. **Bridge tokenizer** (TDD). The per-sentence UAX-29 walk: whitespace-skip, underscore
   strip-then-test (narrowing span; inlined alphanumeric-first predicate, NOT core's private
   `is_word`), apostrophe-normalize (lookup-only). *Tests:* span fixtures incl.
   `café`/`Zürich`/`—`/curly-quote multibyte; `_quiet_` → inner `quiet` span; a pure-`_` token
   left as-is; curly-apostrophe span unchanged.
3. **`analyze`** (TDD). Sentence composition over `textobj::sentence_spans`, tag+chunk, assembly.
   *Tests:* the parallel-length invariant (multi-token / empty / punctuation-only); the §5.4
   worked fixtures assert tags+np on known words (`quietly→ADV`, `bold→ADJ`, `it's→PRON`, the
   `the lazy dog` NP run); the accepted-`None` contraction floor; a determinism test (same input
   → same output); a model-load test (calling `brill_tagger()`/`brill_chunker()` doesn't panic —
   covers the LazyLock `unwrap`).
4. **`NlpStore` + `valid_for`** (TDD, shell). *Tests:* version bump invalidates; window move
   invalidates; matching version+window is a hit; empty/non-empty guard.
5. **Shell wiring** (TDD, leaf `nlp.rs`). `Buffer.nlp` field; `nlp_window_at` rebasing by `ps`
   via `prose_window_at`; off-prose → `None`. *Tests:* absolute-offset rebase correctness against
   a known buffer; a **guardrail** test that a query is the ONLY trigger (no timers row exists —
   assert `timers::SUBSYSTEMS` gained no entry) and analyze is not called on a memo hit.
6. **Pre-merge.** Merge gates (`cargo test`, workspace clippy deny, `too_many_lines`,
   `module_budgets`) + PTY smoke suite (mandatory-run/advisory-pass) + `cargo deny` recorded.

---

## 10. Open questions for the human

None outstanding for S7. The three arc-doc "open questions" (theme promotion,
ventilate-as-lens naming, the `cargo deny` gate) are either S-theme-wide (not S7-blocking) or
resolved here (the `cargo deny` run is a recorded release-checklist step per the global
constraints, not a merge gate).
