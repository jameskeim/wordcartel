# S5 — Sentence authority: approved design

**Status:** APPROVED DESIGN (brainstorm complete, 2026-07-12). **NOT a spec, NOT a plan.** The next
pipeline steps (spec → Codex spec gate → plan → Codex plan gate → subagent execution) have **not**
been run. Deliberately stopped here at the human's instruction.

**Item:** backlog **S5** — the first item of the prose-structure arc
(`docs/design/prose-structure-arc.md`: S5 → S6 → S4 → S7 → S8). Ships alone; everything downstream
stands on it.

**Grounding:** `docs/design/s4-grounding.md`, plus a Fable grounding pass and direct probes against
the real crate (every claim below marked *(probed)* was executed, not reasoned).

---

## 1. Scope — three bugs, not one

The filed item named one bug. Grounding found three. All *(probed)* against the real
`textobj::sentence_bounds`.

| Bug | Behavior today | Frequency |
|---|---|---|
| **Hard wraps** — UAX-29 (SB4) ends a sentence at **every newline** | In any reflowed document, `select_sentence` selects **a line**; Focus=Sentence focuses **a line** | **DOMINANT.** Self-inflicted: wordcartel's own `reflow` (`transform.rs`) creates the condition |
| **Abbreviations** | `sentence_bounds("Dr. Smith arrived. He was late.", 0)` → `(0,4)` == `"Dr. "` | Common |
| **Lowercase after a closing quote** | `sentence_bounds("“Why?” he asked.", 0)` → `"“Why?” "` | Occasional |

Probe transcript (hard-wrap case — *exactly what our `reflow` emits*):

```
text:  "The committee met on Tuesday and the\nchair insisted on a vote. Then we left."
caret  0 → (0, 37)  == "The committee met on Tuesday and the\n"    ← A LINE
caret 40 → (37, 63) == "chair insisted on a vote. "
```

Plus: the **differential fixture suite**, and **sentence motions**.

An item called "sentence authority" that still selects a line in a reflowed document has not earned
its name. The hard-wrap fix is in scope.

## 2. Blast radius (why this is safe to change now)

Complete caller set:

| Site | Path |
|---|---|
| `commands.rs:212-217` — `scope_range_at(.., Scope::Sentence)` | → `select_sentence` (`registry.rs:330`) |
| `commands.rs:443-447` — the `ExpandSelection` ladder `[Word, Sentence, Paragraph, Document]` | same |
| **`render.rs:494-507` — Focus mode, PER FRAME** | `gather_row_ctx` → `RowCtx.focus_region` → `row_is_active` |
| `textobj.rs:91-103` — unit tests | — |

**No operator mutates text using a sentence span today** (`textops.rs::scope_or_word` uses
`Scope::Word` only; `mouse.rs` uses Word and Paragraph). The blast radius is **selection + one paint
decision. There is no data-loss path.** This is the window to change the contract; after
`transpose_sentences` (S4) ships, it becomes a migration.

## 3. DECISION — the contract: content-only, and total

`sentence_bounds` returns the sentence **without trailing whitespace**.

- Today it returns the raw UAX span, which **includes** trailing whitespace. `textobj.rs:95` pins
  this: `"One two. Three four."` at caret 2 → `(0, 9)`. Under this design it becomes `(0, 8)` — a
  **deliberate contract change made visible in the test**, not a test for an implementer to quietly
  "fix".
- It remains a **total function** with an explicit, doc-commented **attach rule**: a caret in the
  gap *between* sentences attaches to the **PRECEDING** sentence (and to the following one at a block
  start). An empty window returns the zero-width point, preserving `sentence_bounds("", 0) == (0,0)`
  (`textobj.rs:102`).
- Deliberately **not** `Option`. Making it optional forces a signature change through
  `scope_range_at` → `SelectScope` → the expand ladder → **and the renderer**, all structurally total
  today, for zero user-visible benefit: the honest answer for a gap caret is "the sentence you just
  finished," not "nothing." Introduce `Option` when an object arrives that genuinely has no answer
  (a Quotation with no quotes nearby) — with that object, not before.

**Why content-only:**
1. **Nothing depends on the current contract.** The renderer is provably insensitive — `row_is_active`
   is a half-open **row** overlap test, and the trailing `\n` is the last byte of the sentence's own
   line, so the region end lands exactly at the next row's start and the next row is never lit.
   `select_sentence` is the only visible consumer, and it currently paints a highlighted blank after
   the period.
2. **Every future operator requires it.** A gap-preserving `transpose_sentences` must swap content and
   preserve the inter-sentence gap verbatim (exactly as `transpose_words` does:
   `{word2}{gap}{word1}`, `textops.rs:195-201`). With UAX spans, every operator would re-implement the
   trim.
3. **It repairs the expand ladder.** In a single-sentence paragraph `"One two.\n"`, Sentence ==
   Paragraph == `(0,9)` today, so the strictly-larger containment test (`commands.rs:447`) makes the
   Sentence rung **silently collapse**. With content-only, `(0,8) ⊂ (0,9)` and the rung survives.
   Worth a regression test.
4. It is what the selection highlight should show.

An "around" span (content + trailing gap), if ever wanted for a delete-sentence, is a **separate
helper** — not a flag on this function.

## 4. DECISION — the algorithm: a four-rule post-pass over UAX-29

**Not a from-scratch scanner** (the design-space doc's §5 `boundary_after`). UAX-29 already gets all
of the following right, for free *(probed)*:

```
Pi is 3.14 exactly. → 1      The U.S.A. is large. → 1        Well… I suppose so. → 1
We met at 10 a.m. and left. → 1   Use v1.2.3 or newer. → 1   Wait... what happened? → 1
Magnum, P.I. lived in Hawaii. → 1  It cost $4.50 total. → 1  He paused (a long one)… → 1
The firm (Acme Corp.) folded. → 1  He lives on Elm Ave. near… → 1
See p. 5 and fig. 2 for details. → 1
She said “Go home.” Then she left. → 2   (boundary correctly AFTER the ”)
```

A from-scratch scanner must **re-earn every one of those**. Its failure modes, by contrast, are a
short closed list. So: fold four rules over the UAX segment list.

| Rule | Action | Cures |
|---|---|---|
| **R1** | **merge** when the previous segment's last token is a known abbreviation, or a single capital + `.` | `"Dr. "`; `"J. R. R. "`; `"Mr. and Mrs. "` |
| **R2** | **merge** when the previous segment, trimmed, does **not** end in a terminator (the break was caused *only* by a line separator) | **the hard-wrap bug** |
| **R3** | **merge** when the next segment begins with a **lowercase** letter | `"“Why?” he asked."`; `"He shouted “Stop!” and ran."` |
| **R4** | **shift** the boundary past a run of closing markup (`*` `_` `` ` `` `)` `]`, quotes) after a terminator | `"This is **bold.** And this is next."` |

R1–R3 are folds over segment indices — **allocation-free, `O(segments in the block)`**. R4 adjusts an
offset. This budget is mandatory: `render.rs:494-507` calls this **per frame** (and already allocates
one `String` via `buf.slice`; do not add a second).

**R2 EXCEPTION — the one place this fix could lose authored content.** R2 must **NOT** merge across a
Markdown **semantic hard break** (two trailing spaces, or `\` before the newline). repar treats those
as content and preserves them through all three transforms; a silent merge there would let
`select_sentence` swallow a verse line or an address-block line. Explicit exception, with a fixture.

## 5. DECISION — the abbreviation list: two classes

**Why not just copy repar's list.** repar's ventilate-only stop-list (`repar/src/transform.rs:72-75`,
24 entries) is unreachable (`mod sentence` is private; the list is deliberately quarantined from the
frozen par oracle), so we duplicate *something*. Copying it verbatim would buy coherence-by-construction
with the differential suite — but it carries `no`, `co`, `eq`, `pp`, `ch`, which **false-merge real
boundaries**.

**The governing insight (human, and it decides the fork):** repar's `ventilate` is a **one-shot
destructive transform** — a wrong break is visible, undoable, and repairable by a round trip. **The
lens (S6) is read continuously** — a wrong boundary there is a persistent lie about the writer's own
prose, and it corrupts the *diagnosis*, which is the entire value of the arc. **So our detector is the
authority, and it is optimized for correctness, not for agreement.**

**Why capitalization is not a sufficient rule.** Capital-after-period is a *weak* signal: English
capitalizes both sentence starts and proper nouns, and abbreviations are overwhelmingly followed by
proper nouns. `"Dr. Smith"` (no boundary), `"St. Louis"` (no boundary), `"Acme Co. Then"` (boundary) —
all identical in shape. `St.` is genuinely undecidable without grammar: *"I visited **St. Louis**
yesterday"* vs *"I live on Main **St. Louis** is nearby."* **Lowercase**-after-period, by contrast, is
a *strong* signal of no boundary (R3).

Therefore:

| Class | Members | Rule |
|---|---|---|
| **Always-merge** — a *prefix* to a proper noun; essentially never sentence-final | titles `Mr. Mrs. Ms. Dr. Prof. Rev. Gen. Sr. Jr.` · name-prefixes `St. Mt. Ft.` · citation forms `fig. vol. ch. pp. eq.` | merge regardless of what follows |
| **Break-on-capital, merge-on-lowercase** — a *suffix* that often ends a sentence | `Co. Inc. Ltd. etc.` | R3 absorbs the continuations (`"Acme Co. filed suit"` → lowercase → merge); a capital really is a boundary (`"Acme Co. Then he quit"`) |
| **Dropped** | `no.` | "the answer was no." is far more common in prose than "No. 5"; repar's most damaging entry |

Exact membership is **curated against the fixture corpus in the spec**, not argued in the abstract.

**NOT user-extensible in S5.** Nobody has hit a missing abbreviation; and the moment the list is
user-extensible it can drift from repar's, reintroducing the very incoherence S5 exists to remove.
When wanted, it is **config-file data, never a command** ("add an abbreviation" is inherently
parameterized; builtins are nullary — command-surface **law 10**). Revisit that together with a
possible additive `repar::Options::abbreviations(&[&str])` upstream, as **one** decision, later.

## 6. DECISION — the differential suite: a divergence LEDGER, not an equality contract

**Full unification with repar is impossible** — `repar/src/sentence.rs:1-8` states that
`checkcapital`/`checkcurious` are shared by ventilate **and** by the reflow `guess_merge` path, and
the reflow path is **frozen by a byte-exact par-1.53.0 oracle**. So the suite *pins* the relationship;
it cannot merge the implementations.

**Assert word GROUPINGS.** Not offsets (repar's ventilate re-emits normalized lines with re-emitted
prefixes — its output has no offsets into the source). Not counts (they can be coincidentally right
with the boundaries in the wrong places).

- ours: sentence spans over the paragraph → per span `split_whitespace()` → `Vec<Vec<&str>>`
- repar's: `run_transform(Ventilate, para, W)` → non-empty lines → strip re-emitted prefix →
  `split_whitespace()` → `Vec<Vec<&str>>`
- `assert_eq!`

**Corpus:** (1) the nasty cases above; (2) **the same content hard-wrapped** — the R2 proof, and the
case that matters most; (3) blockquote (`> `) and list-item variants (prefix re-emission);
(4) multibyte (`é` / `中` / `🙂`).

**Known divergences are an explicit ledger**, each entry carrying a **reason** and asserted with
`assert_ne` — so a divergence that *silently disappears* also fails the build, and the list cannot rot.

> **Ledger entry #1 — the colon.** repar's default `terminal_chars` is `".?!:"`
> (`repar/src/options.rs:151-152`); UAX-29 does not break at `:`. `"Note: This is fine."` → repar
> ventilates to 2 lines; our detector says 1 sentence. **ACCEPTED as defensible**, not accidental: a
> colon genuinely *is* a structural break, and `ventilate` exists to expose structure. (It *could* be
> removed via `--terminal-chars=.?!` through the frozen `from_par_args` surface — no upstream change
> needed — but that would change a **shipped transform's output** for every existing user, to serve a
> detector they are not looking at when they run it.)

Not a fuzz/property test in S5. A fixture table.

## 7. DECISION — sentence motions: `Dir` variants, Emacs semantics

**Where.** `Dir::SentenceLeft` / `Dir::SentenceRight` on the existing enum (`commands.rs:35-52`) +
`nav::move_sentence_left/right`, dispatched from the existing `Move` arm. Consequences, all free:
**extend-selection composes** (the `Move` arm already builds `Selection::range(anchor, new_head)` when
`extend`), as do the ladder reset (`sel_history.clear()`) and fold normalization. **Do NOT** add these
as bespoke leaf commands outside `Dir` — that would duplicate the extend/fold/jump-ring plumbing.

**Semantics — Emacs `M-a`/`M-e` (start/end, NOT next/prev).**
- `Alt+a` → the **start of the current sentence**; if already there, the start of the previous.
- `Alt+e` → the **end of the current sentence** (the *content* end, per §3); if already there, the end
  of the next.

The asymmetry is the point: `Alt+e` lands where you'd *continue writing*, `Alt+a` where you'd
*re-read*. It is idempotent-safe, and `Alt+a` then `Shift+Alt+e` is a natural sentence selection.
(Considered and rejected: symmetric next/prev-start, which would match the app's own word motions —
but it throws away the useful end-of-sentence landing, and A14 already established an Emacs-parity
thread in this product.)

**Block boundaries: motions CROSS them**, matching `move_word_right`/`move_word_left`, which are
documented "crossing block boundaries (skipping gaps)" and fall through to the next paragraph's first
word (`nav.rs:845-905`). No third behavior is invented.

**Keybindings.** `Alt+a` and `Alt+e` are **free** in CUA *(verified: bound Alt chords are*
`b c j k l o r s t u z space f4 up down left right shift`*)*. `Alt+Left/Right` is spoken for by the
jump ring. **Unbound in WordStar** — it has no sentence idiom; law 7 means the command still appears
in the palette without a hint. That is contract-compliant, not a gap.

**Four registry rows**, mirroring the shipped word pair (`registry.rs:190-191`): `sentence_left`,
`sentence_right`, `select_sentence_left`, `select_sentence_right`.

## 8. Command-surface contract conformance

Four new commands → **palette by law 3**; **live hints by law 7**. Motions are **palette-only**
(`menu: None`) per the contract's own classification — **law 4 N/A**. **No new persisted setting** —
**law 2 N/A**; **laws 6/8/9 N/A** (no option). **No amendment to the contract is required.**

## 9. Behavior change to SURFACE, not hide

Focus mode with `focus_granularity = "sentence"` (`config.rs:94`) today focuses **a line** in reflowed
text. After S5 it focuses **a sentence**. Desirable — but it is a **visible change to a shipped view**,
not an invisible bugfix. It must be called out in the effort notes and the release notes.

## 10. Known residue — accepted

`"I live on Main St. It's quiet."` merges into one sentence (`St.` as *Street*, ending a sentence,
followed by a capital). **Benign by design:** the failure mode is a *plausible over-long sentence* the
writer can see and dismiss — never a nonsense one-word fragment, and never a mutation (§2: no operator
consumes a sentence span).

This is exactly the ambiguity **S7's POS tagger dissolves**: `Louis` tags `PROPN` (continuing a noun
phrase); `Then` tags `ADV` (starting a clause). The heuristic is not a permanent compromise — it is
the placeholder for a principled rule that arrives two items later.

## 11. Explicitly OUT of scope for S5

- User-extensible abbreviations (§5) — config data, later, together with the repar upstream question.
- `Option`-returning ("I don't know") objects (§3) — with the object that needs it.
- Any operator that *mutates* using a sentence span — that is **S4**.
- The lens (**S6**), the objects (**S4**), the POS substrate (**S7**), the lenses (**S8**).
- Fuzz/property testing of the detector — a fixture table suffices here.

---

## Pipeline status

**Brainstorm: COMPLETE and approved (2026-07-12).** Stopped here deliberately.

**Not yet done:** spec → Codex spec gate (loop to clean) → plan → Codex plan gate (loop to clean) →
branch → subagent-driven TDD execution → the two final gates (Fable whole-branch + Codex pre-merge) →
`--no-ff` merge.
