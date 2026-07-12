# The prose-structure arc (S4–S8) — north star, sequence, and the decisions already made

**Status:** DESIGN-SPACE / pre-spec (2026-07-12). NOT law, NOT an approved spec. This is the
umbrella document for a **multi-effort arc**, decided in a brainstorm with an independent Fable
review. Each item below still gets its own brainstorm → spec → Codex gate → plan → execution.

Companion documents:
- `docs/design/prose-text-objects-design-space.md` — the original idea material. **Drafted by an
  external LLM with NO codebase access; it says so itself.** Rich and useful, but unverified, and
  several of its central proposals are refuted below. Read it for the argument, not the architecture.
- `docs/design/s4-grounding.md` — the code-grounded map (real signatures, real seams, verified
  probes). Where it and the design-space doc conflict, **the grounding wins**.
- `docs/design/prose-linters-design-space.md` — the adjacent effort (harper-ls diagnostics). It is
  **independent of this arc** — see §5.

---

## 1. The north star

The original thesis, from the design-space doc:

> *A prose editor that genuinely understands sentences, clauses, quotations, and document structure —
> and lets the writer select, move, and transform them as first-class objects — can offer editing
> operations no code editor ever would.*

**The thesis is right. Its articulation is wrong**, and the correction is the most important
decision in this document. The doc sells *selection, movement, transformation*. That is the weak
half: nobody wakes up wanting to select a clause. The strong half is the one it never states:

> **A prose editor that understands sentences can SHOW the writer the skeleton of their prose —
> non-destructively, on demand — and then let them operate on the bones it revealed.**

**Diagnosis, then surgery.** Selection in service of a defect the editor just showed you is a
product. Selection on its own is a solution hunting for a problem. (Cautionary case: iA Writer's
POS-driven "Syntax Control" — their most-demoed, least-used feature.)

Writing is revision. Drafting is hours; revision is weeks. Every tool a writer owns is built for
drafting (focus mode, typewriter scroll) or for filing (corkboards, graphs). *Revision* — **this
sentence is 41 words and drowns the reader; four of these six open with "The"; that subordinate
clause belongs at the front** — is supported by nothing. That gap is the product.

Wordcartel is uniquely positioned: it already holds the two hard prerequisites — an incremental
Markdown block tree with byte spans, and `repar`, a real structure-preserving reflow/ventilate
engine. **And it already ships `ventilate`** — one-sentence-per-line is *the* canonical revision
technique. We built the engine and then exposed it only as a destructive transform.

---

## 2. Decisions already made (do not re-litigate without new information)

### D1 — `repar` STAYS. We do not absorb it. (Fable memo I + II, both directions.)
The seam is already correct: `transform.rs` puts the **editor's block tree** in charge of *what* to
transform (`snap_to_blocks`) and repar in charge of *how*. `reformat.rs` is 2,738 lines of
DP break-choosers under a **byte-exact par 1.53.0 fidelity contract**, with 36 test files and a
golden corpus. Re-earning that buys two features nobody has asked for (affix transparency, clause
ventilation). Keep repar frozen at the string seam.

*The `atomic.rs`/`width.rs` precedent does NOT generalize* — those are 212/130-line leaf utilities
with no oracle behind them. `reformat.rs` is the crown jewel with a C-fidelity contract.

### D2 — There are THREE sentence authorities, and full unification is IMPOSSIBLE.
Verified in `repar/src/sentence.rs:1-8` (repar's own words): `checkcapital`/`checkcurious` are shared
by ventilate **and** by the reflow `guess_merge` path, and the reflow path is **frozen by the
byte-exact par oracle**. The abbreviation stop-list (`repar/src/transform.rs:61-78`) is deliberately
ventilate-only *to protect that oracle*. So inside repar, reflow and ventilate already disagree —
and wordcartel's UAX-29 detector is a third.

**The only coherent architecture:** wordcartel's own detector owns everything the user **sees** and
**selects**. repar's ventilate remains the **destructive transform** — unchanged, still a command,
still the export path. The two are pinned together by a **differential fixture suite** (S5), not by
shared code. Any spec promising "one sentence authority" is unimplementable as written.

### D3 — Objects make SELECTIONS; existing operators act on the selection. (N+M, not N×M.)
`Handler = fn(&mut Ctx) -> CommandResult` is nullary; `dispatch_with_arg` discards the arg for
builtins (`registry.rs:34,768`); command-surface contract **law 10** blesses this; **law 3** makes
the palette exhaustive. A true object×operator matrix costs N×M palette rows *or* a ~150-site
signature change *plus* an App-law amendment.

The code already voted three times: `scope_or_word` (`commands/textops.rs:22`) is the shipped
convention for ten operators; `select_marked_block` (`blocks_marked.rs:169`) exists *specifically*
as the block→selection bridge; and the registry is nullary by law.

**The cross-product is recoverable in userspace:** an Effort-P Lua plugin can register a one-gesture
"Delete Sentence" that dispatches `select_sentence` then `cut`. That is exactly where law 10's own
forward-pointer says it belongs. **No contract amendment is needed for this arc.**

*Accepted costs:* every object-op is two invocations for palette users; the Inside/Around affinity
axis is dropped in v1 (YAGNI); counts ("delete 3 sentences") are not expressible.

### D4 — ADOPT `harper-brill` in-process. (Decided 2026-07-12; MEASURED, not assumed.)
The POS backend is **not** `harper-core` and **does not** come via the linters effort. `harper-brill`
2.5.0 has exactly two dependencies (`harper-pos-utils`, `serde_json`) and exposes a **rule-based
Brill POS tagger** (`tag_sentence -> Vec<Option<UPOS>>`) and a **noun-phrase chunker**
(`chunk_sentence`) — the design-space doc's `Phrase` object, which it deferred as "needs real
grammatical parsing," is a shipped crate function.

**Measured on this machine, 2026-07-12** (probe crate; not assumed):

| | harper-core (H2, ejected 2026-07-11) | **harper-brill (this decision)** |
|---|---|---|
| crates added | +389 | **+119 activated** |
| binary delta | +16 MB (3×) | **+0.95 MB** |
| GPU/tensor backends compiled | cubecl + CUDA + ROCm + wgpu | **none** — `burn` core + `ndarray` only |
| native/FFI (`-sys`) crates | — | **zero** |
| clean release build | +39 s | +184 s one-time (incremental unaffected) |

⚠ The **lockfile lists 491 crates** including `burn-cuda`/`cubecl-hip`/`burn-rocm`/`cudarc`. **That
number is noise** — they are optional deps that never compile. `default-features = false` does its
job. Do not re-panic on the lockfile; the activated count is 119.

**Live proof the tagger does what the clause splitter needs:**
```
"The committee met on Tuesday because the chair insisted."
  DET  NOUN  VERB  ADP  PROPN  SCONJ  DET  NOUN
```
`on` → **ADP** (preposition), `because` → **SCONJ** (subordinating conjunction). That is precisely
the distinction that disambiguates "for"/"so"/"yet" and turns clause-splitting from a trap into a
principled rule (see D5).

**This PARTIALLY REVERSES H2** (which pushed harper out-of-process to shed `burn`). The reversal is
deliberate and justified by the numbers above: a third of the crates, a sixteenth of the binary, no
GPU stack, no FFI. **Recorded in the H2 archive entry.** Open follow-up: `cargo deny`/`cargo audit`
has NOT yet been run against these 119 crates — that is a **release-checklist gate** and must run
before S7 merges. Supply-chain surface (+~40% of the lockfile) is the real cost, and H2 rightly
noted it matters more now that Effort P has opened a plugin attack surface.

*Fable's claim that harper-brill carries "1.4 MB of dead neural model" is REFUTED by measurement:*
the blobs are `trained_tagger_model.json` (660 KB), `vocab.json` (628 KB), `trained_chunker_model.json`
(53 KB) — the Brill rule tables and frequency dictionary, i.e. exactly what we use. Total measured
binary cost is +0.95 MB.

### D5 — Rule-based clause splitting is a TRAP. POS-informed clause splitting is not.
The design-space doc's rule ("split at `, ; : —` and at coordinating/subordinating conjunctions")
collapses on real prose: the Oxford comma fires mid-list; "for" is usually a preposition; "so" an
intensifier; "yet" an adverb. A low-precision boundary feeding a **mutating** operator silently
mangles sentences. The doc also never mentions that clause transpose requires **capitalization
repair** ("I went home, but she stayed" → swap → lowercase sentence start).

With UPOS the rule becomes principled and testable: a clause-comma is followed by a conjunction +
subject NP + finite verb; a list-comma is not. **Clause therefore ships only after S7, and ships
SELECT-ONLY**, behind a measured precision gate on a hand-labeled corpus. Clause *mutation* is a
separate, later decision.

### D6 — THE LAW OF THIS ARC.
> **A linguistic analysis may COLOR, and it may SELECT. It may never MUTATE text without a visible
> selection the writer can see and abort.**

Brill is trained on newswire; the chunker on treebanks. On fiction, fragments, dialect, verse, and
dialogue they *will* mistag. A wrong highlight is noise. A wrong transposition is a corrupted
manuscript, and the writer never trusts the tool again.

### D7 — Nothing in this arc is ON BY DEFAULT.
Every feature here is *revision* machinery. If it intrudes on drafting — a lens on by default, a
highlight that flickers while typing, a reflow on save — it becomes the thing writers hate about
Word, and it violates the project's top priority (instant typing, no silent UI). The E7 precedent
governs: **the cost lands in the summoned view.**

---

## 3. What is CUT (and why) — corrections to the design-space doc

| Design-space proposal | Verdict |
|---|---|
| `TextObject` trait, `ObjectRegistry`, `BufferView`, `Operator` enum, `Affinity` | **CUT.** Vim-shaped scaffolding this codebase has proven it doesn't need — A14 shipped ten operators as plain fns in a leaf module with zero trait machinery. The trait buys extensibility for a *Rust* plugin ecosystem we don't have; our plugins are Lua and compose **commands**. If a heuristic/POS backend swap is later wanted for one object, use a `SentenceBackend`/`ClauseBackend` **enum**, not `Box<dyn TextObject>` in a registry. |
| `PairedDelimiter` framework (§6), 9 registered instances, dual fast/authoritative paths | **CUT — and it is WRONG, not merely unverified.** (a) The "authoritative" tree-backed path **cannot exist**: `BlockTree` discards ALL inline events ("Inline-level tags are ignored") — there are no emphasis/link/code/quotation nodes. (b) Fiction conventionally **omits the closing quote** on a paragraph of continuing dialogue, so the symmetric-delimiter model is wrong for the most common quotation in the target use case. (c) An unmatched `"` makes the scan **O(document)** — a per-keystroke perf violation the doc never bounds. |
| §7.3 plain-text degradation matrix | **CUT.** Wordcartel is a Markdown editor; `nav::paragraph_range_at`'s blank-line gap fallback already covers plain text. |
| §5.3 `STARTER_ABBREVIATIONS` | **NOT SHIPPABLE AS WRITTEN.** `St` and `Dr` are duplicated; `No`, `Co`, `Mon` will eat real sentence boundaries. Curate against the fixture corpus; treat as provenance-tainted like the rest of that doc. |
| Section transpose | **CUT from this arc → belongs to S1.** `outline::sections` yields **nested, overlapping** ranges — the "next section" after an H2 is usually its own H3 child, so swap-with-next **corrupts the document**. S1 must solve sibling identification and separator normalization anyway. |
| Syllable / morpheme | **CUT.** Never planned. |
| Affix transparency (§8.6) | **CUT.** Requires opening repar's decomposition (see D1). No user has asked. |
| Multi-range selection ("select every sentence matching…") | **OUT OF SCOPE.** `Selection` is single-range with private fields. Disjoint *edits* already work via `build_multi_replace`. Stated here so it does not creep back. |

---

## 4. The arc

> ⚠ **Naming:** Fable's memo called these E0–E4. Those labels **collide with the existing E theme**
> (chrome/theming: E1 = density presets … E7 = grammar view). The arc uses **S-theme IDs**.

| Item | Was | What | Size | Needs NLP? |
|---|---|---|---|---|
| **S5** | E0 | **Sentence authority** — fix `select_sentence`; differential fixture suite; sentence motions | S/SM | no |
| **S6** | E1 | **Ventilate-as-a-lens** — non-destructive sentence view + rhythm gutter ⭐ | M | no |
| **S4** | E2 | **Prose text objects + operator layer** (re-scoped: was XL) | M/L | no |
| **S7** | E3 | **Linguistic substrate** — harper-brill POS + NP chunker, in-process | M | **yes** |
| **S8** | E4 | **Prose lenses** — POS stylistic X-rays; Phrase/Clause select-only | M | **yes** |
| S1 | — | Rearrangeable outline — *inherits* `select_section` from S4; **owns** section transpose | M | no |

### S5 — Sentence authority *(everything downstream stands on this)*
The live bug, verified 2026-07-12: `textobj::sentence_bounds("Dr. Smith arrived. He was late.", 0)`
returns **`(0,4)` == `"Dr. "`**. UAX-29 handles `3.14`, `10 a.m.`, and `P.I.` correctly but splits
after a title abbreviation followed by a capital — the most common abbreviation class in prose.
Meanwhile repar's ventilate gets it right. **`select_sentence` is wrong today.**

Contents: an abbreviation-aware post-pass over the UAX-29 boundaries (merge a boundary when the
trailing word is in a curated stop set); a **differential fixture suite** asserting wordcartel's
detector and `run_transform(Ventilate)` agree across a corpus (this is the testable form of "coherence"
— see D2); and **sentence motions**, which BOTH source documents forgot: `Dir` (`commands.rs:35-52`)
has Word and Paragraph motions but **no Sentence** — no Emacs `M-a`/`M-e` parity. Cheaper than any
operator and likely more used.

⚠ Trap: UAX sentence spans **include trailing whitespace** (unlike word bounds — `textobj.rs` test:
`"One two. "` → `(0,9)`). Any consumer must trim to content or gaps double.

### S6 — Ventilate-as-a-lens ⭐ *THE THESIS IS PROVEN OR KILLED HERE*
Today `ventilate` (`registry.rs:435` → `transform.rs:207`) **destructively rewrites** the buffer.
S6 adds a **non-destructive view**: the buffer is untouched; the *display* breaks one sentence per
line. Toggle off → prose returns, byte-identical. `RenderMode` already cycles four states
(`commands.rs:355-361`).

With it, a **rhythm gutter** — per sentence: word count and opening word:
```
 8  The  The committee met on Tuesday.
12  The  The chair, who had prepared, spoke first.
41  The  The proposal, which had been circulating since…
 7  She  She left.
```
The writer sees the 41-word monster and the three `The`s **instantly**. Plus repeated-opener
highlighting within a paragraph (note the codebase already carries the anaphora corpus —
`transform.rs:497-516` tests "We will fight on the beaches / …landing grounds / …fields"; repar's
`prose-prefix` fixup exists *because* anaphora is real).

**Zero NLP. No new objects. No command matrix. No contract amendment.** It is cheap, it is a
screenshot, and it is the one-line pitch:
> *Press one key and see your paragraph as its sentences — length, opener, rhythm — then move them
> around like cards. Press it again and your prose is back, untouched.*

**Architectural constraint (why S6 must precede S4):** the lens MUST render using **wordcartel's own
detector**, not repar's. Then the sentence you *see* and the sentence you *select* are the same
object **by construction**. (It also cannot call repar: repar is `&str`→`String`, so extracting
boundaries means running `--ventilate` and diffing lines — a full-document round-trip per render,
an outright violation of the `O(visible)+O(edited)` rule.)

**⛔ FAILURE SIGNAL — the cheapest falsification in this arc:** *the author uses the lens on real
prose for two weeks and turns it off.* If that happens, **STOP THE ARC.** Everything after S6 is a
more expensive bet on the same premise, and the premise will have been falsified for free.

### S4 — Prose text objects + operator layer *(re-scoped from XL)*
Now **earned** — surgery for what S6 diagnosed, rather than a Vim tribute act.
`select_sentence` / `select_section` (`outline::sections` already computes it — `outline.rs:92`);
the expand ladder extended from its hardcoded 4-array (`commands.rs:443`) to data;
`transpose_sentences`; an **object-agnostic `swap`** over the existing `MarkedBlock`+`Selection`
pair; `count_region` (today only a *view toggle* exists — `registry.rs:545` — there is no
count-of-region command at all). N+M nullary commands, leaf module à la `commands/textops.rs`.

Transpose specifics: `transpose_sentences` generalizes the shipped neighbour idiom
(`transpose_words` swaps the word before the caret with the word at it, **preserving the gap
between them** — `textops.rs:195-201`) and must inherit `transpose_lines`' separator-decomposition
discipline (`textops.rs:236-254`). The `swap` must **reject overlap explicitly with a status
message** — do NOT lean on `build_multi_replace`'s guard, which *silently* degrades to an identity
no-op (`commands.rs:152-161`), violating the no-silent-UI rule.

### S7 — Linguistic substrate *(gated on the `cargo deny` check — see D4)*
`harper-brill` behind a `wordcartel-core` module: POS tags + NP chunks over the **caret's block
window**, **cold-path only** (command/lens-triggered, never per-keystroke), cached by
`(block_span, document.version)`. Serves three consumers: the objects, the lenses, and eventually a
*native* stylistic-diagnostic provider alongside harper-ls.

### S8 — Prose lenses *(the genuinely novel half)*
Every adverb dimmed. Every passive construction (`AUX` + participle `VERB`) underlined. Every
nominalization flagged. Composable with objects: *select every sentence containing a passive.*
**This is what harper-ls CANNOT give you** — harper-ls flags *errors*; these are **stylistic X-rays
of correct prose**, which is what revision actually needs. Then `Phrase` (the chunker's NPs) and
`Clause` (POS-informed, per D5) — **select-only**, per D6.

---

## 5. Relationship to the prose-linters effort — INDEPENDENT

`harper-ls` is a **subprocess** speaking diagnostics-only LSP (`publishDiagnostics`, `codeAction` —
`harper_ls.rs:311,537`). There is **no LSP method that returns a parse.** Diagnostics and text
objects are genuinely different consumers separated by a **process boundary**: one is whole-document,
debounced, `Review`-gated, out-of-process; the other is caret-local, synchronous, always-available.
**No shared substrate is possible across that seam.** The shared substrate exists only in-process —
which is S7.

Therefore this arc should **neither wait for nor merge with** the linters effort. Different engines,
different process models, different latency budgets, no shared code.

**⚠ One thing the linters effort must be told:** H2's rationale for the subprocess split was
"drop the ~389-crate tensor stack from the binary." If S7 brings `harper-pos-utils` in-process,
**`burn` is in the binary anyway** — and part of that rationale is retroactively spent. It may still
hold (harper-core's dictionary + rule corpus is the bulk of its weight, not burn alone), but it must
be **re-measured, not assumed**, before the subprocess split is defended on dep-weight grounds again.

---

## 6. Known hazards to design against

1. **Three sentence authorities** (D2). Unification is impossible; pin with a differential suite.
2. **NLP must never mutate** (D6).
3. **Silent structural snapping already ships.** `transform_unit_at` (`transform.rs:96-142`) snaps a
   caret in a list item to the *entire item*. An object layer that also snaps invisibly means
   operations touch more text than the user selected. **Every snapping operator must show the
   snapped span first.**
4. **The expand ladder does not survive the workflow it enables.** `sel_history` is cleared on every
   motion (`commands.rs:248`), every edit (`editor.rs:287`), and undo (`editor.rs:1287` asserts it).
   But the structural workflow *is* expand → expand → operate → **undo** → re-expand. Today the
   ladder is destroyed every single time. Invisible in the conservative scope; a **shipped bug** for
   this arc.
5. **Three competing regions.** Transient `Selection`; persistent `MarkedBlock` (survives edits —
   `editor.rs:280`; but cleared by undo — `editor.rs:1411`); and now "the current object." The mature
   design must **collapse these to two, not add a third.**
6. **Prose is not a tree, and objects must be able to say "I don't know."** A sentence can cross a
   list-item boundary. Today `Scope::Sentence` always returns *some* range (`commands.rs:212-217`) —
   it cannot decline. Confident nonsense is worse than no answer.
7. **Perf.** Objects scan the caret's **leaf-block window** (`scope_range_at` discipline), never the
   document. Known O(document) paths that must NOT become per-keystroke: `nav::leaf_spans`,
   `outline::sections`. S7's tagger is **cold-path only**.
8. **Anti-regrowth GATEs.** `clippy::too_many_lines`=100; `tests/module_budgets.rs`. `registry.rs`
   (1,910 lines) and `commands.rs` (1,394) already carry `too_many_lines` allows — growth there is
   the path of least resistance and the one the module-structure rule forbids. **Follow the A14
   precedent: a leaf module, no `Command` variant, no `commands::run` arm.**

---

## 7. Open questions for the human

1. **Theme promotion.** Fable argues the full arc is a *theme*, not a feature cluster — a coherent
   product identity. Adding a theme letter is a **schema change** (`backlog.toml` validates themes
   against `A B C D E H M P R S`; the gate rejects unknown letters). Filed under **S** for now.
2. **Ventilate-as-lens vs. ventilate-as-command.** Both exist after S6. Do they share a menu entry,
   a keybinding, a name? (Command-surface contract question — S6's spec must answer it.)
3. **`cargo deny` on the 119 new crates** — not yet run (not installed on the laptop). **Gate for S7.**
