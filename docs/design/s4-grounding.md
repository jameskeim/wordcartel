# S4 (prose text objects) — code grounding

**Status:** GROUNDING / pre-brainstorm (2026-07-12). Companion to
`docs/design/prose-text-objects-design-space.md` (the *idea* material, drafted by an external LLM
with **no codebase access** — explicitly "plausible but unverified"). This document is the opposite:
it is what the REAL code says, verified by reading it and by running probes. Where the two conflict,
this one wins.

Follows the `effort-p-grounding.md` convention: map the real surface before any design fork is
resolved.

> ## ⚠ Corrections (2026-07-14) — this document predates the S5 / S6 / C1 merges
>
> Re-verified against the live tree while scoping S4 (`docs/design/s4-prose-surgery-scoping.md`,
> which carries the CURRENT grounding table). Where this doc and the notes below disagree, the
> notes win:
>
> 1. **§1 "Sentence object" row is stale.** `sentence_bounds` is no longer raw UAX-29 — S5
>    rebuilt it atop `textobj::sentence_spans`, a four-rule post-pass (R1 abbreviation/initial
>    merge, R2 hard-wrap merge, R3 lowercase-continuation, R4 closer shift) with a semantic
>    hard-break veto and a **content-only span contract** (no trailing whitespace). New public
>    kernels: `sentence_spans`, `prev_sentence_start`, `next_sentence_end`.
> 2. **§2 "Verified bug" is FIXED.** `sentence_bounds("Dr. Smith arrived. …")` now returns the
>    full sentence (R1). The two-authorities problem is pinned by S5's differential fixture
>    suite (wordcartel detector ↔ repar ventilate), per arc decision D2 — not unified.
> 3. **Sentence motions exist.** `Dir::SentenceLeft/SentenceRight` →
>    `nav::move_sentence_left/right` (cross-paragraph), plus `select_sentence_left/right`
>    extend variants — shipped in S5. The doc's Scope/ladder inventory predates them.
> 4. **§8's absorb-repar question is DECIDED: repar STAYS** (arc doc D1, Fable memos I+II).
>    Treat §§3–4 and §8 as historical record of how the decision was grounded.
> 5. **S6 shipped a constraint that did not exist here:** the ventilate lens
>    (`wordcartel/src/ventilate.rs::prose_block_at`) classifies prose via `role_at` and windows
>    via the IDENTICAL `nav::paragraph_range_at` call `Scope::Sentence` uses — SEE==SELECT is
>    now a shipped invariant S4 must preserve, and `prose_block_at` is the shipped
>    decline-to-answer predicate (the lens already says "not prose" for headings/lists/code).
> 6. **Undo/redo clear `marked_block` too** (editor.rs `undo`/`redo` — "bypass apply's mapping →
>    acting on stale offsets unsafe"), alongside `sel_history`. Any two-region operation design
>    must know a marked block does not survive undo.
> 7. **Line anchors have drifted** (registry.rs is now ~2,122 lines, `dispatch_with_arg` ≈ :811;
>    `scope_range_at` ≈ commands.rs:197, the ladder ≈ :443–459). Locate by symbol NAME.

---

## 1. Headline: S4 already exists in embryo

S4 is not greenfield. The following are SHIPPED today:

| Piece | Where | What it is |
|---|---|---|
| Word object | `wordcartel-core/src/textobj.rs` | `word_bounds`, `next_word_start`, `prev_word_start` — UAX #29 via `unicode-segmentation` |
| Sentence object | `wordcartel-core/src/textobj.rs` | `sentence_bounds` — UAX #29 `split_sentence_bound_indices` |
| Section object | `wordcartel-core/src/outline.rs` | `Section { heading, body }`, `sections(&BlockTree, &Rope)` — "heading + everything until the next heading of level ≤ this one". Exactly the doc's Section. |
| A `Scope` enum | `wordcartel/src/commands.rs:31` | `Scope { Word, Sentence, Paragraph, Document }` |
| Object resolution | `wordcartel/src/commands.rs:195` | `scope_range_at(editor, head, scope) -> (usize, usize)` |
| Expand/shrink ladder | `wordcartel/src/commands.rs:432–458` | `SelectScope` / `ExpandSelection` / `ShrinkSelection` over the fixed array `[Word, Sentence, Paragraph, Document]`, with `Buffer.sel_history: Vec<Selection>` as the shrink stack |
| Operator precedent | `wordcartel/src/commands/textops.rs:22` | `scope_or_word(editor)` — "the selection if non-empty, else the word at the caret". The A14 convention. |
| 10 prose operators (A14) | `wordcartel/src/commands/textops.rs` | `upcase`, `downcase`, `capitalize`, `transpose_chars`, `transpose_words`, `transpose_lines`, `join_line`, `just_one_space`, `delete_blank_lines`, `delete_horizontal_space` |
| **Two** regions | `wordcartel/src/editor.rs:124,168` | the transient `Selection` **and** a persistent `MarkedBlock { start, end, hidden }` that survives edits (remapped in `Buffer::apply`) — with a full Block menu (`block_copy/move/delete/jump/write/clear`, `mark_block_from_selection`, `select_marked_block`) |

So S4 is largely a **promotion and deepening** of an existing thin layer, not a new subsystem.

## 2. Verified bug: `select_sentence` is wrong on title abbreviations

Probe (run against the real crate, 2026-07-12) of `textobj::sentence_bounds(text, 0)`:

```
"Dr. Smith arrived. He was late."   -> (0, 4)   == "Dr. "        <-- WRONG
"Pi is 3.14 exactly."               -> (0, 19)  == whole          ok
"We met at 10 a.m. and left."       -> (0, 27)  == whole          ok
"Magnum, P.I. lived in Hawaii."     -> (0, 29)  == whole          ok
```

UAX #29 handles decimals, `a.m.`, and initialisms, but splits after a title abbreviation followed
by a capital. `Dr.`/`Mr.`/`Mrs.`/`St.` is the most common abbreviation class in real prose.

**Meanwhile repar's `ventilate` gets this right** — repar has an abbreviation-aware sentence
notion (`repar/src/sentence.rs`, `checkcapital`/`checkcurious`, FIDELITY to `reformat.c:89-122`).

So the design-space doc's §8.4 hazard ("`repar` breaks after 'Dr.' while the sentence object treats
'Dr. Smith' as one sentence") is **already real, with the polarity reversed**: it is wordcartel's own
`select_sentence` that is wrong, and repar that is right. Two authorities on "where does a sentence
end" already disagree in the shipped product.

## 3. The repar seam — the doc's #1 open question is ANSWERED

The doc (§8.2) offered three possible factorings of repar and said everything depends on which one
is real. It is the **third and most limiting**: *"a single Markdown-aware entry point — span in,
reflowed span out, structure handled invisibly. Least work now, most limiting later."*

- wordcartel's **entire** contact with repar is `wordcartel/src/transform.rs` ("the ONLY place that
  touches repar's stringly public API"):
  `run_transform(kind: TransformKind, input: &str, width: u32) -> Result<String, TransformError>`
  — builds `repar::Options::from_par_args([--width=N, verb, FIXUPS_STACK])`, calls `opts.format(input)`.
  **`&str` in, `String` out. Nothing else.**
- repar's public surface is only: `Options`, `Charset`, `Compat`, `PResult`/`ParError`, `char_width`,
  `display_width` (`repar/src/lib.rs:65–69`), pinned by `tests/library_api.rs::public_surface_pin_v1`.
- `mod sentence` is **private**; `sentence_breaks(&[Word], &Config) -> Vec<Range<usize>>`
  (`repar/src/transform.rs:39`) is `pub(crate)`. **Unreachable from wordcartel.**
- The dependency is a **local path dep** (`repar = { path = "../../par-command/repar" }`), not
  crates.io. Same author. The "frozen SemVer surface" is self-imposed, not external.

Consequences that follow from this and are NOT optional:
- **Affix transparency (§8.6) is foreclosed** unless repar changes — the decomposition is buried.
- **Single-sourcing sentence boundaries (§8.4) requires an upstream repar change** — or a
  reimplementation in wordcartel that then *duplicates* repar's abbreviation logic and can drift.
- Clause-granularity ventilation (§8.4's "capability the integration creates") is impossible
  without repar accepting an injected boundary function.

## 4. repar's shape (for the absorption question)

Repo: `~/projects/par-command` (git root; the crate is the `repar/` subdirectory — the repo is
named for the *app*, `par-command`, not the crate. Unconventional but deliberate).

Total **11,450 lines** across 21 modules, 36 test files:

```
2738  reformat.rs        <- the par algorithm (FIDELITY-commented port of reformat.c)
1484  options.rs
1111  segment.rs
 816  main.rs            } 
 743  input.rs           }
 698  charset.rs         }  CLI-only plumbing that wordcartel never touches:
 686  markdown_reflow.rs }  main + driver + input + longopts + terminal + atomic + config
 640  markdown.rs        }  ~= 2,000 lines
 379  error.rs
 361  longopts.rs
 356  driver.rs
 262  segments.rs
 224  transform.rs
 212  atomic.rs
 191  keep_tabs.rs
 170  config.rs
 130  width.rs
 117  sentence.rs        <- the abbreviation-aware sentence logic wordcartel cannot reach
  69  lib.rs
  40  compat.rs
  23  terminal.rs
```

The load-bearing parts for wordcartel are `reformat` + `segment(s)` + `markdown(_reflow)` +
`sentence` + `width` + `charset` — roughly 6,000–7,000 lines. The rest is CLI.

Note wordcartel ALREADY ported two pieces of repar by hand rather than depending on them:
`atomic.rs` (atomic+durable save) and `width.rs` (display width). There is precedent for
absorption.

## 5. The command-surface wall

`wordcartel/src/registry.rs`:
- `pub type Handler = fn(&mut Ctx) -> CommandResult;` (:34) — **a bare fn pointer. Nullary. No
  captures, no parameter.**
- `dispatch_with_arg` (:765) exists (added in Effort P3) but the builtin arm is literally
  `HandlerKind::Builtin(h) => { let _ = arg; h(ctx) }` (:768) — **the argument is discarded for
  every builtin.** `meta.arg` only drives the minibuffer prompt for *plugin* commands.
- `docs/design/command-surface-contract.md` **law 10** codifies this: "Commands stay **nullary**
  today; parameterized set-value commands are an Effort-P concern."
- **law 3**: the palette is exhaustive — every command appears in it.
- `builtins()` (:166) is one table carrying `#[allow(clippy::too_many_lines)]`; `registry.rs` is
  1,910 lines, `commands.rs` is 1,394 (its `run` also carries a `too_many_lines` allow).

Therefore an N-objects × M-operators cross-product costs either **N×M nullary registry rows** (and
N×M palette entries), or a change to `Handler`'s signature touching ~150 registration sites plus the
plugin arm, **plus an amendment to contract law 10** (App law — a deliberate act, recorded in that
doc's History).

## 6. Other places the code will resist

1. **`Selection` is effectively single-range.** `Selection { ranges: SmallVec<[Range;1]>, primary }`
   has **private** fields and only two constructors (`single`, `range`). No `ranges()` accessor, no
   multi-range constructor. Disjoint multi-select ("select every sentence matching…") is not
   expressible — though `build_multi_replace` *does* support disjoint EDITS in one undo unit.
2. **No ancestor navigation on `BlockTree`.** `role_at(byte)` classifies a byte; `deepest_block_at`
   finds a leaf. Walking *up* (block → enclosing ListItem → BlockQuote → Section) means re-walking
   from the root. `transform.rs:118` already hand-rolls a `nearest(kind_test)` closure — the missing
   accessor, in evidence. A document→section→block→sentence→clause→word hierarchy needs this chain.
3. **`BlockTree` has no inline structure at all.** pulldown-cmark inline events are explicitly
   discarded ("Inline-level tags are ignored"). No emphasis, links, code spans, or quotation nodes.
   So §7.2's "authoritative inline spans from the parse tree" **does not exist** — every inline
   object (quotation, emphasis, link) must be a text scan today. `List`/`ListItem` carry no payload
   (no ordinal, no marker, no tightness).
4. **`TextBuffer` has no zero-copy read.** `slice(range) -> String` **allocates** and asserts char
   boundaries. `snapshot() -> Rope` (O(1) clone) is the only escape hatch. `TextSource` (the right
   shape, `Cow<'_, str>`) is implemented for `&str` and `&Rope` but **not** for `TextBuffer`.
5. **The expand ladder is a hardcoded array** and `sel_history` is cleared on *every* edit. Adding
   clause/quotation/section levels means the ladder becomes data — and section-level expansion would
   call `outline::sections`, which is O(headings) and allocates.
6. **Perf rule.** Per-keystroke work is `O(visible) + O(edited)`, never `O(document)`. The existing
   objects respect this by scanning only the caret's **leaf-block window** (`scope_range_at`).
   Known O(document) paths that must NOT become per-keystroke: `nav::leaf_spans` (allocates+sorts
   every leaf span), `outline::sections`.
7. **Anti-regrowth GATEs.** `clippy::too_many_lines` = 100 (workspace-deny);
   `wordcartel/tests/module_budgets.rs` caps hub files. The A14 precedent is the template: a **leaf
   module** (`commands/textops.rs`) with no `Command` enum variant and no `commands::run` arm, called
   directly from `registry.rs`.

## 7. Where the brainstorm had got to

**Fork 1 — how objects and operators meet.** Three options:
- **A. Parameterize `Handler`** — true matrix, but ~150 sites + plugin arm + amend law 10.
- **B. Enumerate the cross-product** — N×M nullary rows; palette explosion; grows the very hubs the
  module-structure rule protects.
- **C. The mark is the intermediary (leaning here).** Objects ONLY make selections
  (`select_sentence`, `select_clause`, `select_section`, `expand_selection`); the existing operators
  act on the selection, which is already the A14 `scope_or_word` convention. **N + M commands, not
  N × M.** No contract amendment. Matches the non-modal model the doc itself endorses in §8.8.

**Transpose under C.** It is the one operator needing two spans. Resolution:
- *Neighbour-transpose*: generalize the shipped Emacs idiom (`transpose_words` swaps the word before
  the caret with the word at it, **preserving the gap between them**) to sentence/clause/section —
  N commands, one per object.
- *Swap-two-regions*: **object-agnostic, ONE command.** The `MarkedBlock` + `Selection` pair is
  already the two-region substrate §8.8 asks for (and `block_move` already moves a marked block to
  the caret). One `swap` serves every object forever, because it doesn't care what made the spans.
  `build_multi_replace` already does disjoint spans in one undo unit.

## 8. The question now on the table

**Should wordcartel absorb/reimplement repar's transforms natively, inside the prose-object model,
rather than depending on repar as a frozen, string-in/string-out external crate?**

The pressures toward absorption:
- The seam is the most limiting of the three shapes (§3 above), and it forecloses affix transparency
  and clause-ventilation.
- Sentence boundaries cannot be single-sourced across the seam without changing repar.
- wordcartel already has the richer structural knowledge (a real Markdown block tree with byte
  spans, incremental reparse, folds, marks) — repar re-derives structure from a string it is handed.
- ~2,000 of repar's 11,450 lines are CLI plumbing irrelevant to the editor.
- Precedent: wordcartel already hand-ported repar's `atomic.rs` and `width.rs`.

The pressures against:
- `reformat.rs` is 2,738 lines of FIDELITY-commented port of `par`'s C algorithm, with 36 test files
  and a golden corpus behind it. That correctness is *earned*, and re-earning it is the real cost.
- repar is also a standalone CLI the user ships and uses independently; absorbing its engine into
  wordcartel could orphan or fork it.
- Both are the same author's code — so "upstream change" is cheap in a way it would not be for a
  third-party crate. Widening repar's public surface is an option that a normal dependency would not
  offer.

---

## Paths

- Idea material: `docs/design/prose-text-objects-design-space.md`
- App law: `docs/design/command-surface-contract.md`
- Project rules/process: `CLAUDE.md`
- repar: `~/projects/par-command/repar` (git root is `~/projects/par-command`)
- The seam: `wordcartel/src/transform.rs`
- Existing objects: `wordcartel-core/src/textobj.rs`, `wordcartel-core/src/outline.rs`
- Existing operators: `wordcartel/src/commands/textops.rs`
- Registry: `wordcartel/src/registry.rs`
- Two-region substrate: `wordcartel/src/editor.rs` (`MarkedBlock`), `wordcartel/src/blocks_marked.rs`
