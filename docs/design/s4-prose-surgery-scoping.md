# S4 — prose surgery (the operate-on-what-the-lens-shows layer): scoping memo

**Phase:** scoping (pre-brainstorm) · **Date:** 2026-07-14 · **Author:** Fable (design thread)
**Inputs:** `docs/design/s4-grounding.md` (2026-07-12, + the 2026-07-14 correction block),
`docs/design/prose-structure-arc.md`, `docs/design/backlog-integration-relationships.md`,
`docs/design/prose-text-objects-design-space.md` (idea material only), backlog item S4 (L,
needs-design) + S9 (triage, banked), the live tree at `main` (post S5/S6/C1).
**Settled upstream (not re-litigated here):** D1 repar stays at the string seam; D2 three sentence
authorities pinned by fixtures, never unified; D3 objects make SELECTIONS, existing operators act
on the selection (N+M, not N×M — no law-10 amendment); D6 analysis may color/select, never mutate
without a visible abortable selection; D7 nothing on by default; the design-space doc's cuts
(TextObject trait / ObjectRegistry / Affinity, PairedDelimiter, plain-text matrix, section
transpose → S1).

---

## 1. Framing — the writer's north star

**S4 is not "text objects." S4 is what the writer's hand does after the lens shows them the
problem.** S6 shipped the diagnosis and the user loves it: press one key and the paragraph becomes
its sentences — a 41-word monster over three rows, a `7 │ She left.` punch line, four openers in a
column. The S6 pitch promised a second half — *"then move them around like cards"* — and that half
is unshipped. S4 is that half.

The backlog framing ("text objects," "operator layer," `transpose_sentences`, "expand ladder") is
code-editor lineage — implementation-shaped names for writer gestures. This memo re-derives every
deliverable from the writer's chair first (§2), then maps each to the verified code (§3). The test
for every capability: *name the sentence a writer would say while doing it.* "Move this sentence
up." "Cut that one." "This point deserves its own paragraph." "How long is this scene?" "Break this
monster here." Nobody says "apply the transpose operator to the sentence object."

Two shipped/adjacent features anchor the design:

- **S6 (shipped, loved).** SEE==SELECT is a hard inherited constraint: the sentence the lens
  *shows* and the sentence a command *grabs* must be the same object by construction — same
  window (`nav::paragraph_range_at` at a content byte), same detector (`textobj::sentence_spans`).
  The lens is also the natural *theater* for S4: a sentence moved up should visibly change rows,
  like a card.
- **S9 (banked, deliberately undesigned).** In-lens editing *feel* — caret/motion/reflow while
  typing inside the lens. S4 is S9's substrate: S4 decides what the structural commands *do*;
  S9 will refine how they (and ordinary typing) *feel* in the lens. S4 should therefore keep its
  post-command caret/selection behavior simple and explicit (the moved thing stays selected — §5
  F8) and leave row-level motion feel alone.

Position in the arc: S5 → S6 → **S4** → S7 → S8. S4 is the last NLP-free item — it completes the
diagnose-and-operate thesis without placing the +119-crate S7/S8 bet, and it runs during the S6
kill-gate window (relationship map Q2: note which capabilities arrived when, so the two-week
verdict can distinguish "lens alone" from "lens + surgery").

---

## 2. The writer's-eye imagine pass

Each workflow below: what the writer is doing, what they'd reach for, and where the inherited
code-editor idiom fits or doesn't. W3/W4 are workflows the current backlog framing *misses*.

### W1 — "This sentence is weak. Cut it."

The most common surgery. In the lens the weak sentence is a *row*; the writer wants to kill the
row. Outside the lens it's the sentence under the caret. The two-step form — `select_sentence`
then `cut` — already works today (both are shipped commands) and is the D3 shape. What's missing
is *trust*: the selection must be exactly the row the lens shows (SEE==SELECT — holds by
construction, §3), and the command must be honest where there is no sentence (a heading, a code
fence, a list item — today `Scope::Sentence` confidently returns *something*; the lens correctly
declines those same blocks — §5 F3). The gap-eating question (cutting a sentence leaves a
double space) is real but small: `just_one_space`/`delete_horizontal_space` exist; whether cut
should tidy its own gap is a spec detail, not a fork.

**Verdict on the idiom:** keep the two-step select→operate. It is D6's virtue (the writer sees
what will be destroyed before destroying it) and the one-gesture form is exactly the Effort-P
Lua composition law 10 points to.

### W2 — "This point should come first." (the reorder — the marquee capability)

The backlog says `transpose_sentences` — the Emacs neighbor-swap. But watch a writer reorder: they
don't think "swap A and B"; they think **"this sentence belongs earlier — move it up until it's
there."** In the lens that gesture is *literal*: press move-up and the card slides up one row-group,
visibly, repeatably, abortable at every step (undo = one step back). The same mechanics as the
Emacs idiom (swap with the previous sentence, **preserving the gap between them** — the shipped
`transpose_words` discipline), but with two writer-critical differences:

- **The caret travels WITH the moved sentence** (Emacs transpose lands the caret after the pair,
  which kills repeatability — the second press would move the *wrong* sentence).
- **The name and menu row say what the writer means:** "Move Sentence Up/Down," the sibling of
  every code editor's beloved move-line-up/down — not "Transpose."

Long-distance moves ("this belongs two paragraphs up") already have shipped machinery nobody has
connected to sentences: `mark_block_from_selection` + `block_move` IS lift-and-place. Select the
sentence, mark it, walk the caret to the destination, `block_move`. Four steps today; a real
workflow tomorrow if S4 makes step 1 trustworthy. Whether S4 also ships a dedicated two-region
**swap** (mark one region, select another, exchange them) is a genuine fork — §5 F1/F2.

**Boundary semantics need the brainstorm:** what does move-up do to the *first* sentence of a
paragraph — cross into the previous paragraph (powerful, but re-homes the sentence across a
blank line) or stop at the paragraph edge with a status message (predictable; lift-and-place
covers the crossing)? Flagged in F1.

### W3 — "This sentence is really its own point." (paragraph surgery — MISSING from the backlog)

Two of the highest-frequency revision moves in real prose are *paragraph* surgery at *sentence*
joints, and neither is in the current framing:

- **Break the paragraph before this sentence** — the sentence (and everything after it) becomes a
  new paragraph. Mechanically trivial (insert a blank line at a sentence start the detector
  already knows); writerly value high ("this deserves its own paragraph" / "start the scene
  here"). In the lens, the writer is *looking at* the joint when they want this.
- **Merge this paragraph with the next** — the inverse, for choppy fragments. (`join_line` exists
  but is line-based; the writer's unit here is the paragraph.)

These fit D3 perfectly (they're caret-anchored edits, not new region machinery) and they complete
the "sentences as cards" story: cards can move *between* piles, not just within one.

### W4 — "Break this monster here." (sentence split — and the join trap)

The 41-word sentence the gutter exposes. The writer usually rewrites — but the mechanical assist
is real: **split the sentence at the caret** (terminator + space + capitalize the next word). The
writer supplies the judgment (where the joint is — no NLP, no D5 hazard); the command supplies the
mechanics. The capitalization change is visible at the caret, satisfying the show-what-changed
rule cheaply.

The inverse — **join two sentences** — is a trap worth naming: joining requires *de*-capitalizing
the second sentence's opener, and "The" → "the" is right where "France" → "france" is wrong.
Case repair on join needs judgment the editor doesn't have. Options in F6 (ship dumb join that
touches no case; ship split only; defer join entirely).

### W5 — "How long is this scene?" (count — mostly already answered, one gap)

Deeply writerly, and the imagine pass found it *mostly shipped*: the status-line word-count
segment is already **selection-aware** (`render_status::word_count_segment` — select anything and
the segment shows its words · chars, live; test `word_count_segment_selection_aware`). The S6
gutter shows per-sentence counts. What's genuinely missing:

- **Sentence count** in the region readout (the revision-relevant stat the gutter implies —
  "this scene is 34 sentences / 612 words").
- **Count without selecting** — "how long is this section?" wants select_section (W6) + the live
  segment, which composes for free once select_section exists.
- The **SP-7 shared helper**: gutter counts, the segment, and any future PA goals all converge on
  "stats over sentence spans in a window" — S4 should name ONE core helper (extend
  `wordcartel-core/src/count.rs` over `sentence_spans`) rather than a third ad-hoc caller.

Whether a separate `count_region` *command* still earns a palette row (posting to the status line
for terminals where the segment is toggled off) is a small fork — §5 F5.

### W6 — "This scene moves as one." (the section)

`select_section` — the heading and everything under it, as one grabbable unit. The writer's uses:
read its weight (W5 composes), cut it, and above all **move it** — which the block machinery again
already provides (select_section → mark → `block_move`), no S1 required. S4 must spec section
identity with **S1's sibling problem in view** (S1 inherits this selection and owns section
transpose): the section end is "next heading of level ≤ mine" (`outline::sections` computes
exactly this today), and the spec paragraph must say how separators normalize so S1 extends
rather than re-derives. Writer's mental model of "this section": heading + body, one thing —
the Inside/Around split stays dropped (§5 F7).

### W7 — "Select more. No — less." (the ladder)

Grab-the-sentence / no-the-whole-paragraph is a genuinely writerly gesture and the shipped
expand/shrink commands already do it (word → sentence → paragraph → document). Two real gaps:

- **The Section rung.** With select_section shipped, the ladder wants word → sentence → paragraph
  → section → document — which is the promotion of the hardcoded 4-array to a data table the
  backlog already names.
- **Hazard 4 (shipped bug): the ladder does not survive its own workflow.** The structural
  workflow is expand → operate → *undo* → re-expand, but `sel_history` is cleared by apply
  (editor.rs), by undo AND redo, and by every motion. The clears are load-bearing (stale-offset
  safety — undo/redo bypass apply's mapping; the same reason undo clears `marked_block`), so the
  fix is a design choice, not a deletion: map the stack through edits, rebuild it from the
  restored selection — or make shrink *stateless* (re-derive the largest scope strictly contained
  in the current selection), which dissolves the entire hazard by removing the state. §5 F4.

### W8 — Where the code-editor idiom does NOT serve the writer (kept cut / consciously accepted)

- **"Transpose" as a name and as pair-swap semantics** — replaced by move-up/down framing (W2).
- **Inside/Around affinity** — a modal-editor distinction; the non-modal answer is "the selection
  is visible; adjust it before acting." Stays dropped (D3 accepted cost).
- **Counts ("delete 3 sentences")** — not expressible with nullary commands; repeated single
  gestures + Lua composition cover it. Stays accepted.
- **Multi-range selection ("select every sentence that…")** — S8's territory, explicitly out
  (core `Selection` is single-range with private fields; disjoint *edits* exist via
  `build_multi_replace`).
- **Quotation/emphasis/link objects** — the PairedDelimiter cut stands (BlockTree discards inline
  events; fiction omits closing quotes; unmatched-quote scan is O(document)).

---

## 3. Grounded code surface (verified 2026-07-14, by symbol — post S5/S6/C1)

Line numbers are current-tree anchors for convenience; locate by NAME.

### 3.1 Shipped and correct — S4 stands on these

| Surface | Symbol (current location) | Verified state |
|---|---|---|
| Sentence authority (S5) | `wordcartel-core/src/textobj.rs::sentence_spans` (:202), `sentence_bounds` (:212), `prev_sentence_start` (:235), `next_sentence_end` (:253) | UAX-29 + 4-rule post-pass (R1 abbrev/initial, R2 hard-wrap, R3 lowercase, R4 closer shift) + semantic-hard-break veto; **content-only spans** (no trailing whitespace); allocation-free; "Dr. Smith" bug FIXED |
| Word object | `textobj.rs::word_bounds` / `next_word_start` / `prev_word_start` | unchanged (UAX-29) |
| The selection window | `wordcartel/src/nav.rs::paragraph_range_at` (:699) | leaf block span, else blank-line gap fallback (empty `(s,s)` on a blank line); total over the document |
| Sentence motions (S5) | `commands.rs::Dir::{SentenceLeft,SentenceRight}` → `nav::move_sentence_left/right` (nav.rs:948/:975) | cross-paragraph; + `select_sentence_left/right` extend variants (registry.rs:198–199) |
| **Selection commands — ALREADY SHIPPED** | registry.rs:337–341: `select_word`, `select_sentence`, `select_paragraph`, `expand_selection`, `shrink_selection` (palette-only, no menu) | the backlog's "deliverable 1" largely EXISTS; what's missing is trust (decline, F3) and the Section rung |
| Scope resolution | `commands.rs::Scope` (:31), `scope_range_at` (:197) | Word/Sentence/Paragraph/Document; Sentence = `paragraph_range_at` window + `sentence_bounds` — **the IDENTICAL primitive the lens uses** |
| The ladder | `commands.rs` `ExpandSelection` arm (:443), order array `[Word, Sentence, Paragraph, Document]` (:447); `sel_history` stack | hardcoded 4-array; cleared on every motion (:250), in `Buffer::apply` (editor.rs:298), on undo (:308) AND redo (:327) |
| S6 lens — SEE==SELECT | `wordcartel/src/ventilate.rs::prose_block_at` (:33) — `role_at == Paragraph` classify + `paragraph_range_at` window; `segment_block` (:73) = `sentence_spans` re-export; `resolve` (:156) window-aware; `VentBlock.byte_origin` = window `ps` | the lens window and origin are the selector's own; doc-comments + tests pin the identity. **The lens DECLINES non-prose blocks** (`prose_block_at → None` for heading/list/code/blockquote) — the shipped decline predicate F3 wants |
| Operator convention (A14) | `commands/textops.rs::scope_or_word` (:22); ten operators incl. `transpose_words` (:165 — gap-preserving swap), `transpose_lines` (:217 — separator decomposition), `join_line`, `just_one_space` | leaf module, no Command variants, registry calls direct — the module template |
| Two-region substrate | `editor.rs::MarkedBlock` (:133; remapped through edits in `Buffer::apply` :291, collapse-clears); `blocks_marked.rs::block_move` (:35 — **lift-and-place, shipped**, "can't move a block into itself" overlap guard), `block_copy`, `mark_block_from_selection` (:150), `select_marked_block` (:172); full Block menu (registry.rs:402–422) | **undo/redo CLEAR `marked_block`** (editor.rs undo/redo — stale-offset safety); `block_move` uses `build_multi_replace`, one undo unit |
| Section geometry | `wordcartel-core/src/outline.rs::Section {heading, body}` (:79), `sections()` (:92) | one pass, O(headings) + alloc; end = next heading of level ≤ mine; ranges NEST (S1's problem) |
| Counts | `wordcartel-core/src/count.rs::word_count/char_count`; `render_status.rs::word_count_segment` (:52) — **selection-aware TODAY**; S6 gutter per-sentence counts (`ventilate.rs::layout_block` via `count::word_count`) | three separate callers already converge on SP-7; no sentence-count stat anywhere |
| Multi-edit builder | `commands.rs::build_multi_replace` (:143) | H7 guard: malformed/overlapping input **silently degrades to identity no-op** (:154–163) — S4's swap must reject loudly BEFORE calling it |
| Command surface | `registry.rs::Handler` (:34) — nullary fn ptr; `dispatch_with_arg` (:811) discards arg for builtins (:814); law 10 | unchanged; N+M shape confirmed viable |

### 3.2 Drift corrected (vs `s4-grounding.md`, 2026-07-12 — correction block added to that doc)

1. `sentence_bounds` is no longer raw UAX-29 (S5 post-pass; content-only); the "Dr. Smith" bug is
   FIXED; sentence motions exist.
2. The absorb-repar question (its §8) is DECIDED — D1, repar stays.
3. S6 shipped SEE==SELECT and the `prose_block_at` decline predicate — constraints that postdate
   that doc.
4. Undo/redo also clear `marked_block` (not just `sel_history`) — load-bearing for F2.
5. Line anchors drifted throughout (registry.rs ≈ 2,122 lines; `dispatch_with_arg` :811).

### 3.3 Genuinely net-new in S4

`select_section` (no `Scope::Section`, no command); decline-to-answer for prose selections; the
ladder as a data table + the hazard-4 resolution; move-sentence-up/down (no sentence-level
reorder exists); the two-region `swap` (if F2 says yes); sentence-count stat + the SP-7 helper (+
`count_region` command if F5 says yes); paragraph-break/merge at sentence joints and
split-sentence (if F6 says yes); the B10 clamp fix (`nav::caret_line` :45–52,
`h.min(len-1)` — S4 works in exactly this code).

---

## 4. Scope

### 4.1 IN (writer-named, code-grounded)

1. **Trustworthy grabbing** — `select_sentence`/`select_paragraph` gain decline-to-answer per F3
   (shared predicate with the lens); `select_section` net-new over `outline::sections`, specced
   with S1's sibling model (R6).
2. **The reorder** — move-sentence-up/down per F1 (gap-preserving swap mechanics from
   `transpose_words`, separator discipline from `transpose_lines`, caret travels with the
   sentence, post-op selection per F8); the two-region `swap` per F2 (explicit loud overlap
   rejection — never lean on `build_multi_replace`'s silent identity guard).
3. **The ladder, grown up** — data table incl. Section; hazard-4 resolution per F4.
4. **The weight of things** — sentence-count in the selection readout; ONE core SP-7 helper
   (stats over `sentence_spans` in a window) refactoring the gutter + segment onto it;
   `count_region` command per F5.
5. **Paragraph/sentence joints** — per F6 (break-paragraph-here, merge-paragraph, split-sentence;
   join-sentence likely deferred).
6. **Folded bug fixes** — B10 (EOF caret clamp); hazard 4 is item 3.
7. **Tests** — SEE==SELECT identity tests extended to the new commands; decline-behavior table
   tests; move-up/down round-trip + undo-granularity tests; swap overlap-rejection; SP-7 helper
   unit tests; e2e journey (lens on → move sentence up → gutter order changes → undo →
   byte-identical); palette-completeness (automatic).

### 4.2 OUT (with the code reason)

- **Section transpose / reorder** — S1's (nested `outline::sections` ranges; swap-with-next
  corrupts). `select_section` + `block_move` gives writers *a* section-move meanwhile.
- **TextObject trait / registry / Affinity; PairedDelimiter; plain-text matrix** — cut upstream,
  confirmed still right (A14 leaf-fn precedent; BlockTree has no inline nodes).
- **Clause anything** — post-S7, select-only, per D5/D6.
- **Multi-range selection** — S8 territory; core `Selection` is single-range by design.
- **In-lens motion/reflow feel** — S9's, deliberately (S4 keeps post-command behavior explicit
  and simple; S9 refines feel after real use).
- **One-gesture fused ops ("delete sentence")** — userspace (Effort-P Lua), per D3/law 10.

### 4.3 Size: **L — confirmed, with a note**

The imagine pass moves weight around but not the total: deliverable 1 shrank (selection commands
exist; what remains is decline + Section), while W3/W4 add small new edits and the SP-7 refactor
touches three call sites. Every mechanism has a shipped template (A14 edit shape, block machinery,
`sentence_spans`), but the surface is wide: ~8–12 new commands × contract tests × the e2e
journeys, plus two folded bug fixes. If the brainstorm cuts F6 to break-paragraph-only and F4
resolves to stateless-shrink, **M is reachable**; plan for L.

---

## 5. Design forks for the brainstorm (numbered; one at a time; recommendations are NOT decisions)

### F1 — The reorder gesture: what is it, and what happens at the paragraph edge?

- **A. Emacs `transpose_sentences`** — neighbor pair-swap, caret after the pair. Matches shipped
  `transpose_words` naming/mechanics exactly.
- **B. `move_sentence_up` / `move_sentence_down`** — same swap mechanics, but named as motion of
  a *thing*, caret (and selection, F8) travels with the moved sentence, repeatable
  press-press-press. In the lens this is literally the card sliding.
- **C. Both.**

Sub-question either way: at the first/last sentence of a paragraph, does the move **cross** into
the adjacent paragraph (relocating across the blank line) or **stop** with a status message
(lift-and-place covers crossing)?

**Recommendation: B, stop-at-edge.** B is A with the caret placement writers need for
repetition; shipping both names for one operation is palette noise. Stop-at-edge keeps every
press's effect local and predictable (crossing changes TWO paragraphs' shapes at once); W3's
break/merge + block lift-and-place cover deliberate cross-paragraph restructuring. The edge
status message doubles as the discoverability moment for those.

### F2 — Does the object-agnostic two-region `swap` earn its place beside shipped lift-and-place?

- **A. Ship `swap`** over `MarkedBlock` + `Selection`: mark region 1, select region 2, swap.
  One command serves every object forever; explicit overlap rejection with a status message.
- **B. Don't ship it** — `block_move` (lift-and-place) + move-up/down cover the real reorder
  workflows; swap-two-distant-regions is rare in prose revision.
- **C. Ship it later** (after the S6-window trial says whether reordering is even used).

**Recommendation: A.** It is small (the substrate, the builder, and the Block menu all exist),
it is the only *two*-region operation in the product (block_move is region+point), and it
completes the region story cheaply. But two hard requirements: overlap **rejects loudly**
(`build_multi_replace`'s guard silently no-ops — never reach it with overlap), and the spec must
state the region law: **there are exactly TWO region states** (transient `Selection`, persistent
`MarkedBlock`); "the current object" is never state — a select command *produces* a Selection and
dies (hazard 5 resolved structurally). Also note: `marked_block` does not survive undo (verified),
so a failed swap-then-undo leaves no stale mark — good, but the spec should say it.

### F3 — Decline-to-answer: what does `select_sentence` do where the lens shows no sentences?

The lens already declines: `prose_block_at` → `None` for heading/list/code/blockquote/table rows —
they render verbatim, no gutter. `Scope::Sentence` today returns *something* everywhere (a
heading's text scans as one "sentence"). Hazard 6: confident nonsense.

- **A. Strict SEE==SELECT closure** — `select_sentence` uses the SAME `prose_block_at` predicate;
  on non-prose it selects nothing and posts "no sentence here (heading)" to the status line.
- **B. Helpful fallback** — select the block anyway, status names what was actually selected
  ("selected heading — not a sentence").
- **C. Status quo** — silent best-guess.

**Recommendation: A.** The lens has already taught the writer where sentences exist; a command
that agrees with the lens is trust, one that disagrees is incoherence (the D2 lesson one level
up). B is seductive but is exactly the silent-snapping class (hazard 3) with a fig leaf — and
`select_paragraph` (which legitimately selects any block) remains one keystroke away as the
honest fallback. Sub-question for the brainstorm: does `move_sentence_up/down` decline the same
way (it must — it's a mutation, D6's hard case). Note: this makes S4 *remove* capability from a
degenerate case; contract-wise it's behavior, not surface (no law change).

### F4 — The ladder: data table membership, and what shrink remembers (hazard 4)

Membership seems settled (word → sentence → paragraph → **section** → document, as a data table —
confirm Section belongs). The real fork is hazard 4 — the expand→operate→undo→re-expand
workflow destroys the ladder today, and the clears are load-bearing (stale offsets):

- **A. Stateless shrink** — drop `sel_history` entirely; shrink re-derives "the largest scope
  strictly CONTAINED in the current selection" (the mirror of expand's derivation). Nothing to
  clear, nothing to survive; hazard 4 dissolves. Cost: shrink lands on canonical scope
  boundaries, not the exact prior selection (expand from a hand-made selection, then shrink,
  gives the scope ladder's rung — not your hand-made range back).
- **B. Keep the stack, map it** — remap `sel_history` through `Buffer::apply` (as `marked_block`
  is), and rebuild-or-restore across undo/redo. Highest fidelity, real machinery (undo/redo
  bypass apply's mapping — that's WHY they clear today).
- **C. Keep the stack, narrow the clears** — survive undo only (rebuild from the restored
  selection), keep clearing on motions/edits.

**Recommendation: A.** It deletes state and a shipped bug at once, and the fidelity loss is at
the margin of the feature (the ladder's rungs ARE canonical scopes; a writer who expanded
word→sentence→paragraph and shrinks expects the sentence back — stateless gives exactly that,
including after undo, which is the broken workflow). I cannot verify from reading how often the
exact-range-restore margin matters in practice — genuine product-feel judgment; flagged.

### F5 — Where the weight of things surfaces

- **A. Live segment only** — extend the (already selection-aware) status segment with sentence
  count; select-to-weigh is the gesture; no new command.
- **B. Command only** — `count_region` posts "612 words · 34 sentences · 3,401 chars" to the
  status line on demand.
- **C. Both** — segment for the ambient case; a palette command that works when the segment is
  toggled off and gives the fuller readout.

**Recommendation: C** (with A as the soul of it). The segment is where writers will actually
consume this — zero extra gesture; the command is cheap discoverability + completeness (and the
palette row is where "count" is findable). All three consumers (segment, gutter, command) move
onto the ONE SP-7 core helper — that refactor is in scope regardless of the letter chosen.

### F6 — Paragraph/sentence joint surgery: how much of W3/W4 folds in?

- **A. Break-paragraph-here + merge-paragraph-forward** (the paragraph pair only).
- **B. A + split-sentence-at-caret** (terminator + capitalize; visible at the caret).
- **C. B + join-sentences** (dumb form: remove the boundary, touch NO case — the writer fixes
  the capital; honest but half-finished feeling).
- **D. None** — keep S4 to selection+reorder+count; file W3/W4 as a follow-on item.

**Recommendation: B.** A is the highest-value/lowest-risk addition the imagine pass found (the
cards-between-piles half of the story) and split-sentence is the direct answer to the gutter's
41-word diagnosis — both are single-`ChangeSet` edits in the A14 template. Join's case-repair
problem (W4) has no honest automatic answer; C's dumb join is defensible but tests D6's spirit —
human's call. D is wrong: it re-opens the "surgery layer that can't operate on paragraphs" gap
the moment a writer tries the lens for real.

### F7 — `select_section` semantics

- **A. One command: heading + body** (the whole subtree — `Section.heading.byte .. body.end`).
- **B. Body only** (`Section.body`).
- **C. Two commands / a modifier.**

**Recommendation: A.** The writer's "this section" includes its title; move/cut/count all want
the subtree; and the S1 handoff (sibling identity, separator normalization — spec paragraph
required, R6) is defined on the subtree. B's use case ("rewrite the body, keep the heading") is
served by selecting the body by hand or paragraph-ladder from within. Also confirm: does Section
join the ladder (F4) at word→sentence→paragraph→**section**→document? (Lean yes — it's the
ladder's payoff on long documents; `outline::sections` is O(headings)+alloc per press, which is
command-triggered, not per-keystroke — acceptable by the perf rule as read, flagged below for
the record.)

### F8 — Post-operation feedback for caret-anchored structural mutations (hazard 3's positive form)

`move_sentence_up/down` (and split/merge) act at the caret without a prior selection — the one
place the objects-make-selections model doesn't automatically show the affected span.

- **A. The moved/created thing becomes the selection** — after move-up, the moved sentence is
  selected (visible + the next press moves the same sentence); after break-paragraph, the
  promoted sentence is selected.
- **B. Status-line description only** ("moved sentence up").
- **C. A + B.**

**Recommendation: C.** A is the mechanism that makes repetition safe (the selection IS the
"what moves next" contract) and it satisfies "every snapping operator shows the snapped span" by
construction; B costs one string. Note the shipped counter-example this rule exists for:
`transform.rs::transform_unit_at` silently snaps a list-item caret to the whole item — S4 must
not add members to that class.

---

## 6. Web of connections (verified against the relationship map)

- **S5 (shipped) — stands on.** All selection/reorder/count paths consume `sentence_spans` /
  `sentence_bounds` / the motion kernels. Content-only spans mean gap handling is S4's job at
  each edit site (the arc's trailing-whitespace trap, inverted: spans exclude the gap, so
  move/cut must decide gap fate explicitly).
- **S6 (shipped) — earned by; constrained by.** SEE==SELECT: same window
  (`paragraph_range_at` at a content byte), same detector, and (per F3) same *decline* —
  `prose_block_at` becomes the shared predicate rather than a lens-local one. S4's edits are
  ordinary buffer edits; the lens re-derives (`fill_visible`) — no lens-special code paths.
  The S6 kill-gate trial: record which S4 capabilities land when (map Q2).
- **S9 (banked) — substrate for.** S4 defines command *semantics* + the F8 selection contract;
  S9 owns in-lens motion/reflow *feel*. S4 should hand S9 a clean invariant: after any S4
  command, selection = the operated span, caret at its head — S9 can then tune row landing
  without re-opening semantics.
- **S1 — exports to.** `select_section` specced with the sibling model (same-or-higher-level
  boundary, separator normalization stated); S1 owns section *reorder*.
- **S8 — composes later.** "Select every sentence containing a passive" = S8 lens + S4 verbs;
  S4 waits for nothing.
- **SP-7 — names the seam.** One core stats-over-sentence-spans helper; gutter + segment +
  count_region converge on it (redundancy 2 in the relationship map).
- **B10 — folds in.** `nav::caret_line`'s `h.min(len-1)` clamp; S4 lives in this file.
- **Effort P — the cross-product's home.** Every S4 command is nullary and Lua-composable;
  fused gestures ("delete sentence") are plugins, per law 10's forward-pointer.
- **Linter arc — independent** (process boundary; no shared substrate). Nothing inherited.

---

## 7. Command-surface contract note (scoping level)

S4 conforms without amendment:

- **N+M nullary commands** (law 10 untouched — the D3 decision); all handlers are plain fns in a
  leaf module à la `commands/textops.rs` (no new `Command` variants beyond possibly
  `Scope::Section` if the ladder table wants it — anti-regrowth: registry rows are data, hubs
  don't grow).
- **Palette exhaustive** (law 3) — automatic on registration; the shipped `select_*` rows are the
  precedent (palette-only, `menu: None`).
- **Menu ⊆ palette** (law 4) — brainstorm decides the curated subset: move-sentence-up/down are
  Edit-menu candidates (beside nothing similar today); swap belongs in the Block menu beside
  Move Block; count in View or nowhere (the segment carries it).
- **No new user-settable options planned** → LAW-2 snapshot untouched, no setters, no hint
  changes. (If the brainstorm makes decline behavior or edge-crossing an *option*, that decision
  drags in the full option contract — flag: prefer picking ONE behavior.)
- **No hints/keymap obligations** — no default chords proposed; palette/menu access first
  (bindings are a later, separate act; law 7 inherited).

Nothing surfaced that would amend the contract.

---

## 8. Hazards, risks, and claims I could not verify by reading

1. **Selection painting across ventilated row-groups** — a selected multi-row sentence in the
   lens should highlight correctly via the `ColMap` src mapping; the render path reads right but
   I did not compile a probe. **Verify at spec time with a scratch probe** (the whole-branch gate
   would catch it, but SEE==SELECT trust is the product — check early).
2. **F4's fidelity margin** (stateless shrink returns canonical rungs, not exact prior ranges) —
   how much writers notice is unverifiable by reading; product-feel judgment for the human.
3. **F1's edge semantics** (cross vs stop) — same class: judgment, not verification.
4. **`outline::sections` per-press cost in the ladder** — O(headings) + allocation per
   expand-press on a huge document. Command-triggered (not per-keystroke), so within the perf
   rule as written; I cannot measure the felt latency on a pathological document from reading.
   Low risk; noting per the residual-duty rule.
5. **Gap fate on sentence cut/move** — content-only spans mean every mutation site chooses what
   happens to the inter-sentence gap (the `transpose_words` gap-preservation discipline
   generalizes, but cut/break sites each need an explicit answer in the spec — this is where a
   whitespace-doubling class of bugs would live).
6. **Hazard 3's shipped precedent** (`transform_unit_at` silent snap) is *adjacent* scope: S4
   does not fix it, but F8's rule should be stated so S4 adds no new members to the class —
   and the brainstorm may choose to fold a fix in (small) or leave it.
7. **The trial-window question (map Q2)** is the human's: S4 during the S6 two-week window
   muddies the "lens alone" falsification unless capability arrival dates are noted. Cheap
   discipline; someone must actually do it.

---

## 9. Pointer index (for the spec phase)

| Surface | File : symbol |
|---|---|
| Detector | `wordcartel-core/src/textobj.rs::{sentence_spans, sentence_bounds, prev_sentence_start, next_sentence_end}` |
| Window + decline predicate | `wordcartel/src/nav.rs::paragraph_range_at` (:699); `wordcartel/src/ventilate.rs::prose_block_at` (:33) |
| Scope + ladder | `wordcartel/src/commands.rs::{Scope :31, scope_range_at :197}`; ExpandSelection arm :443 (order array :447) |
| Ladder state + clears | `editor.rs::Buffer::{sel_history :161, apply :291–298, undo :308, redo :327}` |
| Operator template | `wordcartel/src/commands/textops.rs::{scope_or_word :22, transpose_words :165, transpose_lines :217}` |
| Two-region substrate | `editor.rs::MarkedBlock :133`; `blocks_marked.rs::{block_move :35, mark_block_from_selection :150, select_marked_block :172}` |
| Multi-edit builder (guard!) | `commands.rs::build_multi_replace :143` (silent identity on malformed input :154–163) |
| Section | `wordcartel-core/src/outline.rs::{Section :79, sections :92}` |
| Counts / SP-7 | `wordcartel-core/src/count.rs`; `render_status.rs::word_count_segment :52`; `ventilate.rs::layout_block` gutter counts |
| Registry precedents | registry.rs `select_*` rows :337–341; Block menu :402–422; `Handler` :34; `dispatch_with_arg` :811 |
| Folded bugs | B10: `nav.rs::caret_line` :45–52; hazard 4: the ladder clears above |
| Lens re-derive path | `ventilate.rs::{fill_visible :372, resolve :156, set_ventilate :462}` |
