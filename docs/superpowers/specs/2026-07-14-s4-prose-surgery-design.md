# S4 — Prose Surgery (operate on what the lens shows): design spec

**Effort:** S4 · **Arc:** S5 → S6 → **S4** → S7 → S8 (`docs/design/prose-structure-arc.md`).
**Absorbs bug fixes:** **B10** (EOF caret clamp) and closes **Hazard 4** (expand-ladder does not
survive undo — dissolved by stateless shrink). **Exports to:** S1 (`select_section` + the sibling
model), S9 (the post-command selection invariant).
**Date:** 2026-07-14 · **Author:** Fable · **Status:** spec (pre-plan), for the Codex spec gate.
**Grounding source of record:** `docs/design/s4-prose-surgery-scoping.md` (this author's prior
grounding; every symbol re-verified against the live tree for this spec). **Decisions of record:**
`s4-brainstorm-decisions.md` (the 8 forks, settled). Anchors are symbol NAMES; `:NNN` are current-tree
conveniences only.

> **Reviewer note.** This spec is gated by cross-checking every claim against the REAL source (not a
> diff; do not run cargo). Where a claim pins a specific arm I give the enclosing symbol so it
> survives line drift. Grounding findings that CHANGE a naive reading of a decision are flagged
> **[CONSEQUENCE C-n]** (C-1 … C-13); the most important are **C-7/C-12/C-13** (fold survival across a
> section move is NOT automatic — its correction is order-sensitive AND must be bracketed by two
> rebuilds) and **C-11** (classify at the content byte, not the line-start, or SEE==SELECT breaks on
> indented prose).

---

## 0. What S4 delivers (one paragraph)

S4 is the surgery layer for the structure the S6 ventilate lens diagnoses. It ships **N+M nullary
commands** in a leaf module on the A14 template: **trustworthy grabbing** (`select_section` + a
strict decline-to-answer that reuses the lens's own `ventilate::prose_block_at` predicate), the
**reorder** (`move_sentence_up`/`move_sentence_down` — gap-preserving neighbor swap, caret and
selection travel with the moved sentence, stop at the paragraph edge) plus one **object-agnostic
`swap`** over the existing `MarkedBlock`+`Selection` pair, the **ladder grown up** (word → sentence
→ paragraph → **section** → document as a DATA TABLE, with **stateless shrink** that deletes
`sel_history` and dissolves Hazard 4), **joint surgery** (`break_paragraph_here`,
`merge_paragraph_forward`, `split_sentence_at_caret`), and **counts** (a sentence-count added to the
already-selection-aware status segment + a `count_region` command, all three consumers refactored
onto ONE SP-7 core helper). Every edit flows through `submit_transaction`/`ChangeSet`
(`editor.apply`) as one undo unit. Two folded bug fixes: **B10** and **Hazard 4**. No
command-surface law-10 amendment (the object×operator cross-product stays Effort-P Lua).

The eight brainstorm decisions are LAW here (§1). Grounding consequences are flagged
**[CONSEQUENCE C-n]** for the Codex gate.

---

## 1. Settled decisions (LAW — not re-opened)

1. **F1 — the reorder is `move_sentence_up` / `move_sentence_down`, stop-at-edge.** Same
   gap-preserving swap mechanics as `transpose_words`; the **caret AND the selection travel with the
   moved sentence** (press-press-press slides the same card). At the first/last sentence of a
   paragraph the move **STOPS** and posts a status message that doubles as discoverability for
   break/merge. No silent cross-paragraph relocation.
2. **F2 — ship the object-agnostic two-region `swap`** over `MarkedBlock` + `Selection`. Overlap
   **rejects LOUDLY**; never reach `build_multi_replace`'s silent identity no-op. **Two-region law:**
   exactly TWO region states (transient `Selection`, persistent `MarkedBlock`); "the current object"
   is NEVER a third state — a select command PRODUCES a `Selection` and dies (Hazard 5 resolved
   structurally). `marked_block` does not survive undo (grounded) → a failed swap-then-undo leaves no
   stale mark.
3. **F3 — strict decline-to-answer.** `select_sentence` and the caret-anchored mutations reuse the
   lens's OWN `ventilate::prose_block_at`; on a non-prose block (the decline set, defined once in
   §3.3; note a **table reads as prose**, consistent with the lens — B14) they select/edit NOTHING and
   post "no sentence here (<block kind>)". `select_paragraph` remains the
   honest any-block fallback. Behavior, not surface — no law change.
4. **F4 — ladder = word → sentence → paragraph → section → document, as a DATA TABLE; shrink is
   STATELESS.** Delete `sel_history` entirely; shrink re-derives "the largest canonical scope strictly
   CONTAINED in the current selection" (mirror of expand). Dissolves Hazard 4. Accepted cost
   (human-decided): shrink lands on canonical rungs, not an exact hand-drawn prior range.
5. **F5 — both, segment is the soul.** (a) extend `render_status::word_count_segment` with a sentence
   count; (b) a `count_region` palette command posting "N words · N sentences · N chars". All three
   consumers (segment, S6 gutter, `count_region`) refactor onto ONE SP-7 core helper extending
   `wordcartel-core/src/count.rs`.
6. **F6 — fold in `break_paragraph_here` + `merge_paragraph_forward` + `split_sentence_at_caret`**
   (terminator + space + capitalize; writer supplies the joint — no NLP). **Join-sentences DEFERRED.**
   Every edit site pins **GAP FATE** explicitly (§5.4 — the whitespace-doubling class).
7. **F7 — `select_section` = heading + body (the whole subtree)**, fold-aware. `select_section`
   selects the whole subtree folded or open; a folded selected section shows the **folded heading row
   highlighted**; a **moved/swapped folded section STAYS FOLDED** at the destination (**C-7**: this is
   NOT automatic — S4 adds explicit fold capture/re-apply). **Byte-verbatim move — S4 does NOT
   re-level** (promote/demote/normalize is S1). Section is the ladder's top structural rung; specced
   with **S1's sibling model** (end = next heading of level ≤ mine; separator normalization stated) so
   S1 extends rather than re-derives.
8. **F8 — after any caret-anchored mutation the operated/created span BECOMES the selection** AND a
   status message describes it. The selection IS F1's "what moves next" contract and satisfies "every
   snapping operator shows its span" (adds NO new members to the `transform_unit_at` silent-snap
   class). **Invariant handed to S9: after ANY S4 command, `selection` = the operated span, caret at
   its head.** **Wiring (C-9):** "caret at head" means the selection is built **head-at-start**
   (`Selection::range(to, from)` — anchor at the END, head at the START), because
   `Selection::range(anchor, head)` puts the caret on the second arg (`selection.rs`); `select_section`
   and every F8 command build it this way, and `set_selection_range` is changed to do the same so the
   ladder evaluates from inside the span (§3.4).

**Folded-in bug fixes:** **B10** (`nav::caret_line`'s `h.min(len-1)` clamp) and **Hazard 4**
(resolved by F4).

---

## 2. Grounding — the real surfaces S4 touches

All re-verified against the tree at `main` (post S5/S6/C1, 2026-07-14).

### 2.1 The sentence authority (S5) — `wordcartel-core/src/textobj.rs`

- `sentence_spans(text) -> impl Iterator<Item=(usize,usize)>` — UAX-29 + a four-rule post-pass (R1
  abbreviation/single-initial merge, R2 hard-wrap merge, R3 lowercase-continuation, R4 closer shift)
  with a **semantic-hard-break veto** (`  \n` / `\\\n`). Spans are **content-only** (no trailing
  whitespace — `content_end`). Allocation-free, `O(bytes)`.
- `sentence_bounds(text, pos) -> (usize,usize)` — the sentence containing `pos`; **attach rule**: a
  caret in the gap → the PRECEDING sentence; before the first content → the FOLLOWING; a window with
  no content → `(0,0)`.
- `prev_sentence_start` / `next_sentence_end` — the S5 motion kernels (already wired to
  `Dir::SentenceLeft/Right`).
- **[CONSEQUENCE C-1] Content-only spans mean the inter-sentence gap belongs to no span.** Every S4
  mutation site must therefore state GAP FATE explicitly (§5.4). This is the whitespace-doubling
  class and the reason F6 requires per-site answers.

### 2.2 The selection window + the SEE==SELECT path

- `nav::paragraph_range_at(blocks, buf, pos) -> (usize,usize)` — the caret's leaf-block span, else the
  blank-line gap fallback (empty `(s,s)` on a blank line). **This is the window `Scope::Sentence`
  uses** (`commands::scope_range_at`, Sentence arm) **and the window the lens uses**
  (`ventilate::prose_block_at` returns exactly this).
- `ventilate::prose_block_at(blocks, buf, byte) -> Option<(usize,usize)>` — `Some((ps,pe))` iff
  `blocks.role_at(byte) == BlockRole::Paragraph`, and then the value is `paragraph_range_at`'s window.
  `None` for every non-`Paragraph` `BlockRole` (the decline set, enumerated once in §3.3). **A `Table` has NO `BlockRole` mapping** (no
  `Table` variant; `kind_to_role` leaves `BlockKind::Table` unmapped, block_tree.rs:255) so it
  classifies as `Paragraph`/**prose** — the lens ventilates tables and so does S4 (SEE==SELECT stays
  literally true; true table-decline is B14). **This is the shipped decline predicate F3 requires** —
  the lens already says "not prose" for exactly the blocks S4 must decline on. **The lens passes it the first non-whitespace CONTENT byte, not the
  line-start** (`ventilate::line_content_byte` :338, used by `ventilated_window` :354): CommonMark
  strips up-to-3-space block indent, so classifying at the line-start hits the gap fallback and
  diverges from the block on indented prose. **S4 must classify at the SAME content byte** (C-11).
- `ventilate::segment_block` = a re-export of `sentence_spans` over the RAW window (the veto governs
  the view identically to selection).
- **SEE==SELECT holds by construction only if S4 routes through the SAME three calls**
  (`line_content_byte` for the classification byte, `prose_block_at` for classification+window,
  `sentence_bounds`/`sentence_spans` for segmentation). §8 pins the shared helper that guarantees this;
  §3.3 and C-11 pin the content-byte classification.

### 2.3 `Scope`, `scope_range_at`, and the ladder — `wordcartel/src/commands.rs`

- `pub enum Scope { Word, Sentence, Paragraph, Document }` (:31) — S4 adds `Section`.
- `scope_range_at(editor, h, scope) -> (usize,usize)` (:197): `Word`/`Sentence` scan the
  `paragraph_range_at` window; `Paragraph` = the window; `Document` = `(0, buf.len())`. S4 adds a
  `Section` arm.
- `SelectScope(Scope)` arm (:436) clears `sel_history`, sets the range via `set_selection_range`
  (:232, which does `Selection::range` → `derive::rebuild` → `nav::ensure_visible`).
- `ExpandSelection` arm (:443): **inline** `let order = [Scope::Word, Scope::Sentence,
  Scope::Paragraph, Scope::Document];` (:447), returns the first scope strictly containing the current
  selection, PUSHES the prior selection to `sel_history`. S4 replaces the inline array with a DATA
  TABLE and adds `Section`.
- `ShrinkSelection` arm (:462): pops `sel_history`, re-derives, snaps via `place_caret_visible(...,
  CaretPlace::SnapOut)`. S4 replaces this with a **stateless** derivation (§3.4).
- `sel_history: Vec<Selection>` (`editor.rs::Buffer` :161) is the expand/shrink shrink-stack — but it
  is **also seeded by mouse double/triple-click** (`mouse.rs::seed_and_select`), so mouse selection is
  behavior-bearing here, not incidental (see §3.4/C-10 for the consequence of removing that push);
  **full touch-site census (re-verified — the plan MUST edit every one or the build breaks):**
  `editor.rs` field decl (:161) + init (:238) + clears in `Buffer::apply` (:298) / `undo` (:308) /
  `redo` (:327); `commands.rs` `Move` clear (:250), `SelectScope` clear (:437), `ExpandSelection` push
  (:456), `ShrinkSelection` pop (:463), `SelectAll` clear (:481); `marks.rs` clears (:31/:47/:77/:99/
  :107); **`mouse.rs::seed_and_select` PUSH (:58)** + its clear (:622); **`prompts.rs::goto_line`
  clear (:392)**; plus tests `editor.rs::apply_clears_sel_history` (:1356) and the undo-clear assert
  (:1385), `commands.rs` (:1295/:1321), `mouse.rs` (:840). **F4 deletes the field and ALL these
  touch-sites** (§3.4 states the behavioral consequence of the mouse push's removal).

### 2.4 The operator template (A14) — `wordcartel/src/commands/textops.rs`

Leaf module: no `Command` enum variant, no `commands::run` arm; `registry.rs` calls the handlers
directly. Each handler computes one `(from,to)` + replacement, early-`Noop`s on nothing-to-do,
builds ONE `ChangeSet` + matching `block_tree::Edit` via `super::build_range_replace`, `editor.apply`
(one undo unit), then `super::edit::settle_after_edit`.

- `scope_or_word(editor)` (:22): the selection if non-empty, else the word at the caret. **The A14
  convention S4 preserves** (existing operators keep acting on the selection).
- `transpose_words` (:165): swaps the word before the caret with the word at/after it, **preserving
  the exact gap** (`format!("{word2}{gap}{word1}")`), caret lands after the pair. **The mechanics
  `move_sentence_up/down` generalize** — but with the caret+selection landing on the MOVED sentence
  (F1), not after the pair.
- `transpose_lines` (:217): separator-decomposition discipline (content + trailing separator; the
  swap preserves newline STRUCTURE). **The discipline break/merge inherit.**

### 2.5 The two-region substrate — `wordcartel/src/editor.rs` + `blocks_marked.rs`

- `MarkedBlock { start, end, hidden }` (editor.rs:133); remapped through edits in `Buffer::apply`
  (:291); collapse-clears; **cleared by `undo`/`redo`** (editor.rs:313/:332 — "bypass apply's mapping
  → acting on stale offsets unsafe").
- `blocks_marked::block_move` (:35): lift-and-place; overlap guard "can't move a block into itself"
  (caret ∈ `[start,end)`); builds ascending non-overlapping edits → `build_multi_replace` → one undo
  unit; consumes the block. `mark_block_from_selection` (:150), `select_marked_block` (:172),
  `set_block` (:186, empty-reject "empty block").
- `commands::build_multi_replace(edits, doc_len)` (:143): **[CONSEQUENCE C-2]** its H7 well-formedness
  guard **silently degrades a malformed/overlapping list to an identity no-op** (:154-163). S4's
  `swap` MUST reject overlap with a status message BEFORE calling it — never rely on this guard for
  user feedback.

### 2.6 Sections + folds — `wordcartel-core/src/outline.rs` + `wordcartel/src/fold.rs`

- `outline::Section { heading: Heading, body: Range<usize> }`; `sections(blocks, rope) -> Vec<Section>`
  (:92): one pass; `section_end` = next heading of **level ≤ mine**, else doc end; `body.start` = first
  line after the heading's own line(s) (ATX 1 line / setext 2). `Heading` carries `.byte`, `.end`,
  `.level`. **Ranges NEST** (an H2 section contains its H3 subsections) — the reason section
  transpose/re-level is S1's, not S4's.
- `FoldState`: `folded()` set (:36), `epoch()` (:40), `toggle(byte)` (:44), `reconcile_to(starts)`
  (:64), `remap(cs)` (:95 — maps each anchor via `change::map_pos_before`), `clamp(len)` (:86).
  `Buffer::apply` calls `folds.remap(&cs)` (editor.rs:286); `derive` calls
  `folds.reconcile_to(&heading_starts)` after a rebuild (derive.rs:189, gated on non-empty folds +
  stale tree). Folds are a pure VIEW over intact bytes (`FoldView::compute`, fold.rs:128).
- **[CONSEQUENCE C-7] Fold survival across a section MOVE is NOT automatic — grounded, and
  add-only re-fold is NOT enough.** `Buffer::apply` calls `folds.remap(&cs)` (editor.rs:286), which
  maps EVERY folded anchor via `change::map_pos_before` and **keeps them all in the set** (`remap`
  reassigns coordinates; it never drops). For an anchor inside a DELETED region, `map_pos_before`
  returns the delete's collapse point in new coordinates (the `Op::Delete` arm returns `new` when
  `pos < old + n` — verified `change.rs:255`); every anchor of the moved region collapses to that one
  stale point. `block_move` deletes the section at origin and reinserts it elsewhere, so the heading
  anchor (`Section.heading.byte` = `b.start`) lands on the **stale origin collapse point**, NOT the
  destination. Two failure modes follow:
  1. If the stale point is no longer a heading start, `derive`'s `reconcile_to` (fold.rs:64) drops it
     → the moved section lands **UNFOLDED**.
  2. **If the stale point IS still a valid heading after the move** (e.g. the block that shifted up
     into the vacated slot begins with a heading), `reconcile_to` KEEPS that stale fold. An add-only
     re-fold at the destination would then leave **TWO folds** (stale + destination) — the exact bug
     Codex finding 1 names.
  Therefore F7's "stays folded" requires a **REMOVE-then-ADD** correction on S4's region-relocation
  sites: clear the moved anchors' stale-remapped positions, then re-fold at the destination (§7.3).
  This is the single most important grounding finding in this spec.

### 2.7 Counts — `wordcartel-core/src/count.rs` + `render_status.rs`

- `count::word_count(text)` / `char_count(text)` (UAX-29 word segments; alphanumeric-first).
- `render_status::word_count_segment(editor) -> Option<String>` (:52): **already selection-aware** —
  non-empty selection → counts the selection, else the whole buffer; `None` when
  `view_opts.word_count` is off. Format `"{} words · {} chars"`. **F5 extends this and refactors it +
  the S6 gutter (`ventilate::layout_block` calls `count::word_count`) onto the SP-7 helper (§6).**

### 2.8 B10 site + anti-regrowth

- `nav::caret_line` (:45): `buf.byte_to_line(h.min(buf.len().saturating_sub(1)))` — the shared clamp
  that glues an EOF caret onto the last content line's row (B10). **Fires identically on and off the
  lens** (not an S6 regression). §7.4 pins the fix + off-lens re-verify.
- `wordcartel/tests/module_budgets.rs` hub budgets; `clippy::too_many_lines` = 100. S4's new logic
  lives in a leaf module + data rows (§10) — hubs do not grow.

---

## 3. The selection model

### 3.1 `Scope::Section` (F7)

Add `Section` to `commands::Scope`. `scope_range_at`'s new arm:

```
Scope::Section:
  let sections = outline::sections(blocks, &buf.snapshot());   // O(headings) + alloc; cold path
  // the DEEPEST (innermost, smallest) section whose [heading.byte, body.end) contains h.
  choose s minimizing (body.end - heading.byte) among { s : s.heading.byte <= h < s.body.end };
  Some => (s.heading.byte, s.body.end)     // whole subtree: heading + body
  None => decline (see §3.3)               // caret under no heading
```

- **Range = `heading.byte .. body.end`** (F7: heading + body, the whole subtree).
- **Deepest-containing** so that expand Paragraph→Section grabs the innermost enclosing scene; the
  next expand jumps to Document (parent-section climbing is S1's, not a second Section rung).
- **S1's sibling model, stated for the handoff:** section end is `next heading of level ≤ mine`
  (already how `outline::sections` computes `section_end`). Separator normalization on move/re-level
  is **S1's** — S4 selects and moves byte-verbatim (§7.2). S4's `select_section` therefore hands S1 a
  range whose end is already sibling-correct; S1 extends it with level math, does not re-derive it.

**[CONSEQUENCE C-3] `scope_range_at` for `Section` allocates and is O(headings).** This is a COLD
path (command/expand-press triggered, never per-keystroke), within the perf rule as written. Noted for
the record; not a hot-path violation. (Every other `Scope` arm stays window-local.)

### 3.2 `select_section` command

`select_section` = `SelectScope(Scope::Section)` — same registry wiring as the shipped
`select_sentence` (registry row → `Command::SelectScope`). **But the caret must land at the section's
START, not its end (F8, Codex finding 4):** `set_selection_range` (commands.rs:232) currently builds
`Selection::range(from, to)`, and `Selection::range(anchor, head)` puts `head` on the SECOND arg
(`selection.rs:63` — `head = to`), so today the caret lands at the section END. That both violates
F8's "caret at head" AND makes stateless shrink evaluate `scope_range_at` at `body.end`, which is the
EXCLUSIVE end — OUTSIDE the section `[heading.byte, body.end)` — so the derivation would resolve the
wrong (or no) section. **Fix (§3.4): `set_selection_range` is changed to build `Selection::range(to,
from)` (anchor at end, head at start)** so every ladder select — word/sentence/paragraph/section —
lands the caret at the span START, which is always inside the span. Fold-aware selection (§7.1): the
byte range covers the whole subtree whether folded or open; render highlights the visible (folded
heading) rows (§7.1, grounded).

### 3.3 Decline-to-answer (F3) — the shared SEE==SELECT predicate

A single helper is the SOLE sentence-scope resolver for `select_sentence` and every caret-anchored
mutation, so the command can never drift from the lens:

```rust
/// The sentence scope at byte `h`, using the LENS'S OWN classification + window, or `None`
/// with a reason when `h` is not in prose. SEE==SELECT by construction: same `prose_block_at`
/// (role==Paragraph + paragraph_range_at window) and same `sentence_bounds` the lens renders with.
fn prose_sentence_at(editor: &Editor, h: usize) -> Result<(usize, usize), NonProse>;
```

- **Classify at the caret line's first non-whitespace CONTENT byte — NOT the logical line-start**
  (SEE==SELECT correctness, C-11). Compute `c = ventilate::line_content_byte(buf, buf.byte_to_line(h))`
  (the S6 helper, ventilate.rs:338 — `line_start + first-non-whitespace offset`); if the line is blank
  (`None`), decline. **Rationale (grounded):** CommonMark drops up-to-3-space indent from the block
  span, so `role_at(line_start)` on indented prose hits `paragraph_range_at`'s GAP FALLBACK and
  DIVERGES from the lens; S6 already learned this and classifies at the content byte
  (`ventilate::ventilated_window` :354 → `line_content_byte` → `prose_block_at(.., content)`). Using
  the same content byte makes SEE==SELECT hold on indented prose by construction.
- **Visibility fix (Codex-r3 finding 1):** `ventilate::line_content_byte` is a **private `fn` today
  (ventilate.rs:338)** — `commands.rs` cannot call it. **The plan changes it to `pub(crate)`** so the
  ONE content-byte implementation is shared between the lens and the S4 predicate (single-source keeps
  SEE==SELECT honest — the whole point). Do NOT re-implement the logic in `commands.rs`: a second copy
  is exactly the drift SEE==SELECT forbids.
- `ventilate::prose_block_at(blocks, buf, c)`:
  - `None` → `Err(NonProse(role_at(c)))` — decline. The command posts `"no sentence here
    (<block kind>)"` where `<block kind>` names the `BlockRole`. Selection UNCHANGED.
    `CommandResult::Noop`.
- **THE DECLINE SET — defined ONCE here, single-source (Codex-r3 findings 2+3).** `prose_block_at`
  returns `None` **iff `role_at(c) != BlockRole::Paragraph`**, so the S4 decline set is EXACTLY the
  non-`Paragraph` `BlockRole`s — **grounded, verbatim: `Heading` / `BlockQuote` / `ListItem` /
  `CodeBlock` / `ThematicBreak` / `FrontMatter` / `Comment`** (style.rs `BlockRole`). Every other
  decline mention in this spec (§1 F3, §2.2, §5) refers to THIS set — it is not re-enumerated.
  - **A `Table` is NOT in the set — it reads as PROSE (decision, human-ratified = A).** `BlockRole`
    has **no `Table` variant** and `kind_to_role` (block_tree.rs:255) does **not** map
    `BlockKind::Table` to a role, so `role_at` on a table returns `Paragraph` → `prose_block_at`
    returns `Some` → S4 treats a table as prose. This is **consistent with the S6 lens today** (the lens
    ventilates tables too), so SEE==SELECT stays literally true. True table-decline (adding a
    non-paragraph role for `BlockKind::Table`) is a shipped-S6/core fix filed as **B14** — OUT of S4
    scope.
  - `FrontMatter` and `Comment` DO decline today (`kind_to_role`: `FrontMatter→FrontMatter`,
    `HtmlComment→Comment`, block_tree.rs:263-264) — included above.
  - `Some((ps, pe))` → `let rel = h.saturating_sub(ps); let (sf, st) =
    textobj::sentence_bounds(&buf.slice(ps..pe), rel);` → `Ok((ps + sf, ps + st))`. The
    window-relative offset uses the caret byte `h` (not `c`); **`saturating_sub` guards the case where
    the caret sits in the leading indent** (`h < ps`, possible now that `ps` is the indent-stripped
    block start) → `rel = 0` → the block's first sentence, and `sentence_bounds` further clamps `pos`
    into `0..=len`.

`select_sentence` calls `prose_sentence_at(editor, head)`; on `Ok` sets the selection, on `Err` posts
the message and no-ops. **The `Scope::Sentence` arm of `scope_range_at` is refactored to call this
same helper** (dropping its private duplication of the window+`sentence_bounds` call) so there is ONE
sentence-resolution path (SEE==SELECT single-source). `select_paragraph` (`Scope::Paragraph`) is
UNCHANGED — it legitimately grabs any block and is the honest fallback the decline message implies.

**[CONSEQUENCE C-4] The decline changes behavior in a degenerate case** (`select_sentence` on a
heading previously returned the heading text as a "sentence"; now it declines). This is behavior, not
surface — no command added/removed, no contract clause touched. Stated so the gate sees it is
deliberate (F3), not a regression.

### 3.4 The ladder as data + stateless shrink (F4, closes Hazard 4)

**Data table** (replaces the inline `order` array):

```rust
/// Expand/shrink rungs, coarse-independent order (finest → coarsest). A ROW, not a match arm —
/// adding a rung is a table edit, not dispatcher growth.
const LADDER: &[Scope] = &[Scope::Word, Scope::Sentence, Scope::Paragraph, Scope::Section, Scope::Document];
```

Both expand and shrink **evaluate `scope_range_at` at the current selection's `from()`** (the span
start), NOT at `head` — with `set_selection_range` now building head-at-start (§3.2) `from()` and
`head` coincide, but evaluating at `from()` is the robust invariant: it is always strictly inside the
current span, so the derivation never resolves against an exclusive-end byte (finding 4). Both read
the **current selection**, never a history stack (finding 2c confirmation — expand-from-current-
selection needs no `sel_history`).

- **Expand** (`ExpandSelection`): for `sc` in `LADDER`, compute `scope_range_at(editor, from, sc)`;
  return the first whose range **strictly contains** the current selection (`f <= cf && t >= ct &&
  (f < cf || t > ct)`), where a declined scope (`Section` with no enclosing heading; `Sentence` on
  non-prose) is SKIPPED (yields no range). Set the selection (head-at-start). **No `sel_history` push.**
- **Shrink** (`ShrinkSelection`): **STATELESS.** For `sc` in `LADDER` **reversed** (coarsest →
  finest), compute `scope_range_at(editor, from, sc)`; return the first whose range is **strictly
  contained** in the current selection. Set the selection (head-at-start). No stack, nothing to restore.
- **`set_selection_range` builds head-at-start** (`Selection::range(to, from)`), replacing the current
  `Selection::range(from, to)`. This is the single wiring change that makes the caret land at the span
  START for every ladder select and F8 command, and makes expand/shrink evaluate from inside the span.
  **[sub-decision] It changes the shipped caret landing of `select_word`/`select_sentence`/
  `select_paragraph` from the selection END to its START** — a deliberate, minor, arguably-better
  change the ladder robustness + F8 consistency require; the plan updates any test that asserted the
  old end-landing.
- **Delete `sel_history`** (F4): remove the field + init (`editor.rs::Buffer`) and **every** touch-site
  in the §2.3 census — the `.clear()` calls in `Buffer::apply`/`undo`/`redo`, `commands::run`'s
  `Move`/`SelectScope`/`SelectAll` arms, `marks.rs`, **`prompts.rs::goto_line` (:392)**, and both
  **`mouse.rs` sites (`seed_and_select` PUSH :58 + the clear :622)** — plus the tests that assert
  clearing/seeding (`editor.rs::apply_clears_sel_history`, the undo-clear assert, `commands.rs`
  :1295/:1321, **`mouse.rs::…seeds the expand ladder` :840**). Nothing else reads it (verified —
  expand/shrink were its only consumers).
- **[CONSEQUENCE C-10] Removing the mouse PUSH changes one behavior, and it is F4's already-accepted
  cost.** Today `mouse.rs::seed_and_select` pushes the pre-click selection so a following
  `ExpandSelection` grows from the mouse/hand-made selection and a `ShrinkSelection` can restore it.
  With stateless shrink, a mouse or hand-drawn selection is no longer a shrink TARGET — shrink lands on
  the nearest canonical rung strictly inside it. **Expand still works from a mouse selection** (expand
  reads the CURRENT selection, not the stack — finding 2c), so double/triple-click-then-expand still
  grows correctly; only exact-range *restore* is lost. This is precisely F4's accepted cost ("shrink
  lands on canonical rungs, not exact prior ranges"), not a regression — stated so the gate sees it is
  deliberate. **This is what dissolves Hazard 4:** the expand→operate→undo→re-expand workflow no longer
  depends on surviving state; every shrink re-derives from the live selection, correct after undo by
  construction.

**[CONSEQUENCE C-5] Stateless shrink lands on canonical rungs, not the exact pre-expand range**
(human-accepted, F4). Concretely: shrink derives `scope_range_at` at the selection's `from()` (the
span START — head-at-start, C-9); e.g. after expanding to a Paragraph, shrink → Sentence lands on the
paragraph's FIRST sentence (the sentence at the paragraph start), which may differ from the sentence
you expanded from. This is the documented, accepted behavior — a canonical rung is always returned; an
exact hand-drawn range is not restored. State it in the doc-comment. (Evaluating at `from()` rather
than the exclusive end is also what keeps the derivation strictly inside the span — finding 4.)

---

## 4. The reorder + the two-region swap

### 4.1 `move_sentence_up` / `move_sentence_down` (F1) — leaf-module edit handlers

New handlers in the leaf module (§10), on the A14 template (compute → early-Noop → one `ChangeSet` →
`editor.apply` → `settle_after_edit`).

Algorithm (`move_sentence_down` shown; `up` is the mirror):

1. `prose_sentence_at(editor, head)` → decline (`Err`) posts "no sentence here (…)" and `Noop`
   (F3 applies to mutations).
2. Window `(ps, pe)` = the paragraph range (from the same helper's internals — the block window). Gather
   `sentence_spans(&buf.slice(ps..pe))` (window-relative). Let `A` = the span containing `head - ps`
   (the caret's sentence, via the attach rule), `B` = the NEXT span (for `down`) / PREVIOUS span (for
   `up`).
3. **Edge stop (F1):** if `B` does not exist within THIS window (A is the last/first sentence of the
   paragraph), `Noop` + status `"sentence at paragraph edge — break or merge to cross"` (the message
   that advertises F6). **Never cross the paragraph boundary silently.**
4. **Swap preserving the gap (GAP FATE, §5.4 site M1):** with `A` before `B` (order them), the
   replacement over `[A.start, B.end)` (window-relative → +`ps`) is `format!("{B}{gap}{A}")` where
   `gap = &win[A.end .. B.start]` (the exact inter-sentence separator, preserved verbatim — the
   `transpose_words` discipline). One `ChangeSet` over `[ps+A.start, ps+B.end)`.
5. **F8 post-op:** after `apply`, set `selection` to the MOVED sentence's new byte range (A now sits
   where B was; new range `[start, start + A.len())`) built **head-at-start** —
   `Selection::range(start + A.len(), start)` so the caret (head) lands at `start` (C-9) — so a repeat
   press moves the same card; post status `"moved sentence <up|down>"`. (The Transaction's
   `with_selection` uses this head-at-start range.)

`move_sentence_up` uses `B` = previous span, replacement `format!("{A}{gap}{B}")` over `[B.start,
A.end)`, and re-selects A at its new (earlier) position.

**[CONSEQUENCE C-6] The caret+selection landing differs from `transpose_words`** (which lands after
the pair). This is deliberate (F1 repetition contract) and is the reason `move_sentence_*` is a NEW
handler, not a call into `transpose_words`.

### 4.2 `swap` (F2) — object-agnostic two-region exchange

New handler. Exchanges the **primary `Selection`** region with the **`MarkedBlock`** region.

1. Require both: a non-empty `Selection` AND a `Some(marked_block)`. Missing either → `Noop` + status
   (`"swap needs a selection and a marked block"`).
2. Normalize each to `(from, to)`; let `R1` = the earlier, `R2` = the later.
3. **Overlap rejection (LOUD — F2, C-2):** if `R1.to > R2.from` (they overlap or touch-through), `Noop`
   + status `"can't swap overlapping regions"`. **Do NOT call `build_multi_replace` with overlapping
   edits** (its guard would silently identity-no-op — the exact no-silent-UI trap the decision names).
4. Build TWO ascending non-overlapping edits — `(R1.from, R1.to, text(R2))`, `(R2.from, R2.to,
   text(R1))` — via `build_multi_replace` → one undo unit → `editor.apply`.
5. **Fold preservation (§7.3, REMOVE-then-ADD, two-phase, two-rebuild — C-7/C-12/C-13):** capture
   folded anchors inside R1/R2 (relative) + their stale collapse points BEFORE the edit; sequence
   **apply → `derive::rebuild` (settle tree → valid `heading_starts`) → correction → `derive::rebuild`
   (relayout)**. The correction: **remove ALL captured stale collapse points across both regions FIRST,
   THEN add ALL destination folds** (guarded by `heading_starts`). Two-phase order is mandatory (R1's
   stale collapse point can EQUAL R2's destination → an interleaved loop self-clobbers, C-12); the
   trailing rebuild is mandatory (STEP 3 mutates folds after STEP 2's layout, C-13). A swapped folded
   section stays folded exactly once and the view reflects it — no double fold, no bogus anchor, no
   self-clobber, no stale layout.
6. **F8 post-op:** `selection` = whichever region now holds the text that WAS the selection (i.e. the
   selection's content, now at R's other site), built **head-at-start** (`Selection::range(end, start)`,
   caret at start — C-9); **clear `marked_block`** (it was consumed, mirroring `block_move`); status
   `"swapped"`.

**Two-region law (F2), stated for the gate:** S4 introduces NO new region state. `swap` reads the two
existing regions; `select_*` commands each PRODUCE a `Selection` and return — there is no "current
object" struct. `marked_block` does not survive undo (grounded, editor.rs undo/redo), so a
swap-then-undo leaves the selection restored by history and no stale mark. Hazard 5 (three competing
regions) is resolved by never adding the third.

---

## 5. The joint edits (F6) + the gap-fate table

All three are leaf-module edit handlers on the A14 template; all decline on non-prose (F3); all set the
operated/created span as the selection with caret at head (F8).

### 5.1 `break_paragraph_here`

Split the paragraph so the sentence at the caret (and everything after it, within the paragraph)
becomes a new paragraph.

1. `prose_sentence_at(editor, head)` → decline or `(sf, st)` (global) = the caret's sentence; `(ps,
   pe)` its paragraph window.
2. If `sf == ps` (the caret's sentence is already the paragraph's first) → `Noop` + status
   `"already at a paragraph start"` (nothing to promote).
3. Replace the inter-sentence separator immediately before `sf` — the run `[gap_start, sf)` where
   `gap_start` = end of the previous sentence's content within the window — with a paragraph break
   `"\n\n"` (GAP FATE, §5.4 site M3). One `ChangeSet` over `[gap_start, sf)`.
4. **F8:** `selection` = the promoted sentence at its new position `[sf', sf' + (st-sf))` (shifted by
   the break delta), built head-at-start (caret at `sf'` — C-9); status `"split paragraph"`.

### 5.2 `merge_paragraph_forward`

Join the caret's paragraph with the next paragraph into one.

1. `(ps, pe)` = caret's paragraph window (must be prose — decline otherwise).
2. `nps = nav::next_paragraph_start(blocks, buf, pe)`; if `nps >= buf.len()` (no next paragraph) →
   `Noop` + status `"no paragraph to merge"`. If the next block is non-prose (a heading/list/code) →
   `Noop` + status `"can't merge across a <kind>"` (merging into a heading is nonsense).
3. Replace the paragraph separator `[pe, nps)` (the blank-line run) with a single space `" "` (GAP
   FATE, §5.4 site M4), joining the last sentence of paragraph 1 and the first of paragraph 2 within
   one paragraph.
4. **F8:** `selection` = the absorbed paragraph's first sentence at its new position (now mid-merged
   paragraph), built head-at-start (caret at its start — C-9), so a repeat merge continues forward;
   status `"merged paragraph"`.

### 5.3 `split_sentence_at_caret`

Turn one sentence into two at the caret; the writer supplies the joint (no NLP).

1. `prose_sentence_at(editor, head)` → decline or the caret's sentence `(sf, st)` (both GLOBAL,
   content-only).
2. **Interior guard (Codex finding 3):** proceed only when **`sf < head && head < st`** — the caret is
   strictly INSIDE the sentence's content. `head == sf` or `head == st` is an edge; and crucially,
   because spans are content-only and `sentence_bounds` (textobj.rs:212) attaches the trailing gap to
   the PRECEDING sentence, a caret in the inter-sentence whitespace has **`head > st`** (not `head ==
   st`) — the `head < st` clause rejects it, so we never insert a terminator + capitalize INSIDE the
   gap. On failure → `Noop` + status `"place the caret inside a sentence to split"`.
3. Determine the insertion: at the caret, if the char at `head` is whitespace, insert `"."` and
   uppercase the first letter of the following word; else insert `". "` (period + one space) and
   uppercase the following word's first letter (GAP FATE, §5.4 site M5). Terminator is the period
   `.` (stated; not configurable in S4). One `ChangeSet` covering the inserted terminator/space AND
   the single-char case change of the next word's initial (contiguous region from `head` to the end of
   that initial letter; if non-contiguous, a `build_multi_replace` of two ascending edits — insert at
   caret + case-map the initial — one undo unit).
4. **F8:** `selection` = the new SECOND sentence (from the inserted boundary to `st`, shifted), built
   head-at-start (caret at its start — C-9); status `"split sentence"`.

**Join-sentences is DEFERRED (F6):** de-capitalizing the second opener has no honest automatic answer
("The"→"the" right, "France"→"france" wrong). Not shipped; not stubbed.

### 5.4 GAP-FATE table (the whitespace-doubling class — every S4 mutation site, pinned)

| Site | Command | Gap decision |
|---|---|---|
| **M1** | `move_sentence_up/down` | Swap `A`/`B` **preserving** the exact inter-sentence gap between them verbatim (`{B}{gap}{A}`). No normalization, no doubling, no loss — the `transpose_words` discipline. |
| **M2** | `swap` | Move each region's bytes **verbatim**; whitespace OUTSIDE the regions is untouched. Adjacent-but-non-overlapping regions keep their boundary whitespace in place (overlap is rejected, §4.2). |
| **M3** | `break_paragraph_here` | **Consume** the single inter-sentence separator before the promoted sentence and **replace** it with `"\n\n"` — no leading space on the new paragraph, no trailing space on the first. |
| **M4** | `merge_paragraph_forward` | **Replace** the paragraph separator (`\n\n`/blank run) with **one** `" "` — the two sentences become adjacent within one paragraph, single-spaced. |
| **M5** | `split_sentence_at_caret` | Insert `". "` (or `"."` if the next char is already whitespace — no double space); uppercase the next word's initial. **Boundary (finding 3):** only fires for a caret strictly interior to a sentence's content (`sf < head < st`); a caret in the trailing gap has `head > st` (content-only spans attach the gap to the preceding sentence) and is rejected — never split in whitespace. |
| **M6** | plain `cut`/`copy` of a `select_sentence` selection | **OUT OF S4's gap scope — stated deliberately.** `select_sentence` selects the **content-only** span (SEE==SELECT is inviolable — §8), so a generic `cut` leaves the surrounding gap exactly as cutting any hand-made selection does today. A fused "delete sentence that tidies its gap" is Effort-P Lua composition (law 10). S4 does NOT special-case generic cut. Flagged so the gate sees content-only selection is the SEE==SELECT requirement, not a gap oversight. |

**[CONSEQUENCE C-8] M6 is the one place SEE==SELECT and clean-cut pull apart.** The spec resolves it in
favor of SEE==SELECT (content-only selection) and pushes gap-tidying to userspace. This is a
deliberate boundary of S4's mutation ownership, not a missing behavior.

---

## 6. Counts + the SP-7 shared helper (F5)

### 6.1 The one core helper (`wordcartel-core/src/count.rs`)

Add a single window-stats function that all three consumers call — the SP-7 convergence:

```rust
/// Words, sentences, and chars over a text window. Sentences via `textobj::sentence_spans`
/// (content-only), words/chars via the existing counters. The ONE stats-over-a-window helper
/// (SP-7): the status segment, the S6 gutter, and `count_region` all route through it.
pub struct RegionStats { pub words: usize, pub sentences: usize, pub chars: usize }
pub fn region_stats(text: &str) -> RegionStats;
```

`sentences` = `textobj::sentence_spans(text).count()`. (The gutter needs only per-sentence word counts,
which it keeps computing per span via `word_count`; it calls `region_stats`/`word_count` from the
shared module so there is one home — the plan finalizes the exact gutter call shape without changing
gutter behavior.)

### 6.2 The segment (F5a)

Extend `render_status::word_count_segment` to `"{words} words · {sentences} sentences · {chars} chars"`
via `count::region_stats` over the same text it already selects (selection if non-empty, else whole
buffer). Behavior otherwise unchanged (still gated on `view_opts.word_count`, still selection-aware).

### 6.3 `count_region` command (F5b)

New nullary command `count_region` — posts `"{words} words · {sentences} sentences · {chars} chars"`
to the status line for the current region (selection if non-empty, else the whole buffer), via
`count::region_stats`. Works even when the segment is toggled off; palette-discoverable. Pure report,
no mutation, no `ChangeSet`.

---

## 7. Fold behaviors (F7) — grounded, with the C-7 fix

### 7.1 Selecting / rendering a folded section (probe 2 — RESOLVED BY READING)

- `select_section` sets the byte range `[heading.byte, body.end)` regardless of fold state (folds are a
  VIEW over intact bytes; selection is byte-range → fold-independent). Grounded: nothing in
  `SelectScope`/`set_selection_range` consults folds.
- **Render:** the selection paints via `render::row_spans_placed`, which reconstructs each glyph's
  GLOBAL byte offset as `origin_of(view, buf, l) + p.src.start` and applies `SE::Selection` where it
  overlaps `[sel_from, sel_to)`. **Hidden lines are never in `map.placed`** (render.rs comment at the
  `MarkedBlock` paint: "hidden lines are never in `map.placed`, so the block paint is inherently
  fold-safe" — the SAME per-glyph overlap loop paints Selection). Therefore a selected folded section
  highlights exactly the visible **folded heading row** and draws no hidden rows — **by construction,
  no new code.** §11 pins a `TestBackend` assertion.

### 7.2 Byte-verbatim move — S4 does NOT re-level (F7)

Moving an H3 subtree among H2s lands it as H3 (its bytes are moved unchanged). Promote/demote,
level-cascade, clamp-to-H6, skip-code-fence normalization are **S1's** (S1 fork #2), explicitly. S4
and S1 compose; S4's move is the byte transport, S1's is the re-level.

### 7.3 Fold survival across a section move/swap (probe 3 — C-7, the real work)

Grounded (§2.6, C-7): `Buffer::apply`'s `folds.remap` collapses a moved section's anchors onto the
STALE origin point and KEEPS them in the set; `reconcile_to` then either drops them (→ unfolded) or —
if that stale point is still a heading — KEEPS them, so an add-only re-fold would DOUBLE-count. To
honor F7 ("stays folded") with exactly-once folds, S4 adds an explicit **REMOVE-then-ADD** correction
around any handler that RELOCATES a heading — `swap` (§4.2) and, because `select_section` +
`block_move` is a first-class S4 move workflow (memo W6), an extension of `block_move`:

```
STEP 0 — before the edit (pre-apply): for each region R being relocated, capture:
    moved_rel  = { a - R.from : a ∈ folds.folded() ∩ [R.from, R.to) }   // relative offsets
    R.dest     = R's destination base byte (known from the move geometry)
    R.collapse = the stale point R's anchors will remap to
                 (map_pos_before(R.from, cs) — all interior anchors collapse there, §2.6)

STEP 1 — editor.apply(txn, edit, …)   // edit lands; Buffer::apply remaps folds (stale-collapses them)

STEP 2 — derive::rebuild(editor)      // SETTLES document.blocks (apply does NOT reparse — editor.rs);
                                      // heading_starts is valid only now; reconcile_to has pruned
                                      // any stale anchor that is NOT a heading start

STEP 3 — two-phase FoldState correction (heading_starts from the STEP-2 settled tree):
  starts = outline::heading_starts(blocks, buf)
  // PHASE 3a — remove ALL stale collapse points across ALL regions FIRST:
  for each region R: folds.remove(R.collapse)            // idempotent if reconcile_to already dropped it
  // PHASE 3b — THEN add ALL destination folds:
  for each region R:
    for rel in R.moved_rel:
      let dest = R.dest + rel
      if starts.contains(dest) && !folds.folded().contains(dest) { folds.toggle(dest); }

STEP 4 — derive::rebuild(editor)      // RELAYOUT: repopulate fold-aware line_layouts/FoldView from the
                                      // CORRECTED fold set (STEP 3 mutated FoldState AFTER STEP 2's
                                      // layout, so without this the view shows pre-correction folds)
```

**[CONSEQUENCE C-12] The two phases MUST NOT interleave per-region — this is the `swap` ordering
fix.** In a two-region swap, `build_multi_replace` emits deletes/inserts in ascending order
(commands.rs:166) and `map_pos_before` maps an anchor inside a deleted region to that deletion's start
(change.rs:274), so **R1's stale collapse point can EQUAL R2's destination fold**. A per-region
`remove(R.collapse)` then `add(R.dest)` loop would, on R1's turn, `remove` the byte that R2's turn is
about to (or already did) legitimately fold — self-clobbering the swap. Removing ALL captured collapse
points BEFORE adding ANY destination fold makes the correction order-independent: a destination fold
added in phase 2 is never removed by a phase-1 `remove`. (Equivalently, compute the corrected set
functionally and apply it once via `FoldState::replace_folded` (fold.rs:80) — no interleaving is
possible. The plan picks phase-split remove/toggle vs `replace_folded`; both are grounded and sound.)

**[CONSEQUENCE C-13] The correction is bracketed by TWO rebuilds — apply → rebuild → correct →
rebuild (Codex-r3 finding 4).** `editor.apply` does NOT reparse (editor.rs — it updates the buffer,
remaps folds/marks, bumps version); the block tree and therefore `outline::heading_starts` settle only
in `derive::rebuild` (which reparses, then `rebuild_downstream` runs `reconcile_to` + builds
`FoldView`/`line_layouts` from the CURRENT fold state, derive.rs:180-195). So STEP 2's rebuild is
required BEFORE the correction (heading_starts must be valid), and a **SECOND** `derive::rebuild`
(STEP 4) is required AFTER it because STEP 3 mutates `FoldState` post-layout — matching the shipped
fold commands' "mutate folds THEN rebuild" pattern (registry.rs fold toggles → rebuild). STEP 2 is the
handler's EXISTING post-apply rebuild (`block_move`'s `apply_edit` / the leaf handler's
`settle_after_edit` already call `derive::rebuild`); STEP 4 is the NEW added rebuild. Two rebuilds on a
cold, command-triggered path (a move/swap) is within budget — never per-keystroke.

REMOVE uses `FoldState::remove` (fold.rs:73); ADD uses `FoldState::toggle` (fold.rs:44), guarded by
`outline::heading_starts` so a non-heading byte is never fabricated. It runs BETWEEN the settling
rebuild (STEP 2) and the relayout rebuild (STEP 4), touches ONLY the moved regions'
anchors (stationary anchors remap normally — ordinary positions map correctly, §2.6), and is
idempotent (the `!folded().contains(dest)` guard). **Result: a moved/swapped folded section re-lands
folded exactly once, the stale fold is gone, no bogus anchor is created, and a two-region swap cannot
self-clobber a shared collapse/destination byte.**

**[CONSEQUENCE C-7, restated]** The decision's phrase "folds remap through edits" is only half-true:
`remap` keeps anchors byte-safe but (a) does NOT carry a moved section to its destination and (b) can
leave a stale fold that, combined with an add-only re-fold, double-counts. The REMOVE-then-ADD
mechanism above is net-new and is the corrected form after Codex finding 1. Surfaced prominently for
the gate.

### 7.4 B10 — EOF caret clamp (folded-in)

Fix `nav::caret_line`'s `h.min(len-1)` so a caret at `buf.len()` maps to the trailing (empty/phantom)
line's row, not the last content line's. **Re-verify off-lens behavior** (the clamp is shared; the fix
must not regress ordinary EOF caret placement with the lens OFF). §11 pins both on- and off-lens
assertions. (Exact clamp rewrite is a plan detail; the spec requires: EOF caret → phantom row, and no
change to non-EOF placement.)

---

## 8. SEE==SELECT (the inherited hard constraint) + probe results

`select_sentence` and every caret-anchored mutation route through `prose_sentence_at` (§3.3), which
classifies at the caret line's **first non-whitespace content byte** (`ventilate::line_content_byte`)
and calls `ventilate::prose_block_at` (classification + window) then `textobj::sentence_bounds`
(segmentation) — **the identical three calls the S6 lens renders with** (`line_content_byte` →
`prose_block_at` → `sentence_spans`/`sentence_bounds`). Classifying at the content byte (not the
line-start) is load-bearing: CommonMark strips block indent, so a line-start classification would
diverge on indented prose (C-11). The `Scope::Sentence` arm of `scope_range_at` is refactored onto the
same helper so no second sentence-resolution path exists. This makes "the sentence you SEE in the lens"
and "the sentence a command GRABS/OPERATES on" the same object by construction, including the decline
case (both say "not prose" for the same blocks) and indented prose.

**Probe results (the coordinator's verify-at-spec-time items):**

- **Probe 1 — selection paint across ventilated multi-row row-groups: RESOLVED BY READING (no compile
  probe needed).** `render::row_spans_placed` reconstructs each glyph's global offset via
  `ventilate::origin_of` (returns the window `ps` for a ventilated anchor key, else `line_start`) and
  applies `SE::Selection` on global-byte overlap; a non-empty selection forces the placed path
  (`use_placed`). The overlap test is identical for every visual row of a group, so a selected
  multi-row sentence highlights on all its rows. Pinned by a `TestBackend` test (§11), not asserted
  blind.
- **Probe 2 — folded section paint: RESOLVED BY READING** (§7.1): hidden lines never enter
  `map.placed`; the folded heading row's glyphs carry `SE::Selection`. `TestBackend` test in §11.
- **Probe 3 — fold survival across a move/swap: GROUNDED AS NOT AUTOMATIC** (§2.6 C-7, §7.3): the spec
  adds explicit capture/re-apply. Pinned by a unit test asserting a folded section, moved, is still in
  `folds.folded()` at its new anchor (§11).

No render behavior is asserted that was not read from the source; the two paint probes are converted to
`TestBackend` assertions the plan must implement (behavior grounded, test pins it).

---

## 9. Command-surface-contract conformance

**S4 touches commands, the palette, and the menu — full conformance, enumerated. No user-settable
option is added, so the option-specific laws are N/A (stated).**

### 9.1 New commands (ids, labels, menu placement)

Registered in `registry::Registry::builtins`, near the shipped `select_*` rows (:337-341) and the
Block menu (:402-422):

| id | label | menu | handler |
|---|---|---|---|
| `select_section` | "Select Section" | `None` (palette-only) | `Command::SelectScope(Scope::Section)` |
| `move_sentence_up` | "Move Sentence Up" | `Some(Edit)` | leaf-module `move_sentence_up` |
| `move_sentence_down` | "Move Sentence Down" | `Some(Edit)` | leaf-module `move_sentence_down` |
| `swap` | "Swap Selection ⇄ Block" | `Some(Block)` | leaf-module `swap` |
| `break_paragraph_here` | "Break Paragraph Here" | `Some(Edit)` | leaf-module `break_paragraph_here` |
| `merge_paragraph_forward` | "Merge Paragraph Forward" | `Some(Edit)` | leaf-module `merge_paragraph_forward` |
| `split_sentence_at_caret` | "Split Sentence" | `Some(Edit)` | leaf-module `split_sentence_at_caret` |
| `count_region` | "Count Region" | `Some(View)` | leaf-module `count_region` |

(`select_sentence`, `select_paragraph`, `expand_selection`, `shrink_selection` already exist — S4
modifies their internals, not their registration. The **curated menu subset** is: the five Edit-menu
edit commands, `swap` in Block, `count_region` in View; `select_section` stays palette-only like its
`select_*` siblings.)

### 9.2 Law-by-law

- **LAW 1 (registry = SSOT).** Every S4 capability is a registered command; no out-of-registry entry
  point. All edits go through `editor.apply` (`submit_transaction`/`ChangeSet`).
- **LAW 2 (every user-settable option is a command).** **N/A — S4 adds NO user-settable option**
  (no `SettingsSnapshot`/`OView`/config field, no setter). The `every_persisted_setting_has_a_command`
  guard is untouched. Stated so the gate sees the N/A is deliberate.
- **LAW 3 (palette exhaustive).** All eight new commands are non-hidden → they appear in the palette
  automatically; the palette-completeness test enforces. `swap` has both a palette row and a Block-menu
  row; no state is reachable only by a non-palette door.
- **LAW 4 (menu ⊆ palette).** The curated menu subset (§9.1) is a strict subset of the palette
  (every menu row is also a palette row). `select_section` is palette-only (matching `select_sentence`,
  which is also `menu: None`).
- **LAW 5 (every mouse affordance has a keyboard path).** N/A — S4 adds no new mouse affordance; all
  commands are palette/menu reachable by keyboard.
- **LAW 6 (one shared setter per option; profiles call it).** N/A — no option, no setter. (The SP-7
  `region_stats` helper is a shared *reader*, not a setter — the relevant "one source" discipline for
  stats, satisfied.)
- **RULE 8 (multi-state option shape).** N/A — no multi-state option. (`Scope` gains a variant, but
  `Scope` is an internal selection enum, not a user option; expand/shrink are single-action commands,
  not a state cycle.)
- **LAW 7 (hints track the active keymap).** S4 ships **no default chord** — access is via
  palette/menu. Hint re-resolution is inherited for free; `hints_reresolve_on_preset_switch` is
  unaffected. Any user-added binding re-resolves normally. Stated so the N/A is seen as deliberate.
- **RULE 10 (commands are the plugin/automation spine — NO amendment).** All new commands are
  **nullary** (`Handler = fn(&mut Ctx) -> CommandResult`). The object×operator cross-product (e.g.
  "delete sentence" = `select_sentence` then `cut`) is recovered in **Effort-P Lua**, exactly where
  law 10's forward-pointer places it. **No law-10 amendment.** The two-region `swap` needs no argument
  (it reads the two existing regions), so it too stays nullary.

### 9.3 The two-region law (F2) as a contract statement

S4 asserts and preserves: exactly two region states (`Selection`, `MarkedBlock`). No command
introduces a third persistent region; `select_*` produce a `Selection` and return. This is the
structural resolution of Hazard 5 and is a precondition the `swap` design depends on.

---

## 10. Module structure & anti-regrowth

| Change | Landing zone | Seam respected |
|---|---|---|
| `Scope::Section` + `scope_range_at` arm + `prose_sentence_at` helper + `LADDER` table + stateless expand/shrink | `commands.rs` | one enum variant (compiler-forced arm), one data `const`, helper fn; the ladder is a TABLE not a grown match |
| Delete `sel_history` field + its FULL touch-site census | `editor.rs`, `commands.rs`, `marks.rs`, **`mouse.rs` (push+clear+test)**, **`prompts.rs::goto_line`** | net REMOVAL of state (Hazard 4); every site enumerated in §2.3 so nothing fails to compile |
| `move_sentence_up/down`, `swap`, `break_paragraph_here`, `merge_paragraph_forward`, `split_sentence_at_caret`, `count_region` | **new leaf module `wordcartel/src/commands/prose_ops.rs`** (A14 `commands/textops.rs` template: no `Command` variant, no `commands::run` arm; `registry.rs` calls handlers directly) | leaf module — the sanctioned home; hubs (`reduce`/`run`) do NOT grow |
| Fold capture/re-apply helper (§7.3) | `blocks_marked.rs` (shared by `swap` + `block_move`) or a small `fold` fn | one helper, called by two sites; no dispatcher growth |
| `region_stats` (SP-7) | `wordcartel-core/src/count.rs` | one core fn; three callers converge |
| Segment sentence-count | `render_status.rs::word_count_segment` | in-place extension |
| 8 command rows | `registry::builtins` | **data-table rows** — the sanctioned growth spot |
| B10 clamp fix | `nav::caret_line` | one-expression change |

`commands.rs` (already carries a `too_many_lines` allow on `run`) does NOT gain a `run` arm (the leaf
handlers are called from `registry.rs`, not through `Command`). The new edit logic lives in the leaf
module; `registry.rs` grows by data rows only. **Plan must re-check `module_budgets.rs` after edits**
(GATE); the design keeps hub deltas to registration rows + a `Scope` variant.

---

## 11. Testing

Core (`wordcartel-core`):
- `region_stats`: words/sentences/chars over a window; empty window → all zero; multi-sentence,
  abbreviation (one sentence), hard-break veto (authored line ≠ two sentences) — reuses the S5 corpus.

Selection + ladder (`commands.rs` tests):
- `Scope::Section` at a caret under a nested H3 → `(H3.heading.byte, H3.body.end)`; at a caret under no
  heading → declines; deepest-containing when nested.
- Ladder expand: word→sentence→paragraph→section→document over a headed document; Section rung skipped
  when no enclosing heading (jumps paragraph→document).
- **Stateless shrink**: expand to paragraph then shrink returns a Sentence rung — the paragraph's
  FIRST sentence (the `from()`-evaluation landing, C-5); **the Hazard-4 regression**: expand → edit
  (or undo) → shrink still yields a canonical rung with NO panic and NO stale state (the workflow that
  was broken).
- **Head-at-start (finding 4)**: after `select_section` (and `select_sentence`/`select_paragraph`) the
  primary range's `head == from()` (caret at the span START), and `scope_range_at(Section)` evaluated
  at that head resolves the SAME section (inside the span, not at the exclusive `body.end`).
- **Mouse-then-expand (finding 2)**: seed a selection directly (the former double-click target) then
  `ExpandSelection` grows from that current selection (no `sel_history` needed); `ShrinkSelection`
  lands on a canonical rung strictly inside it (the accepted C-10 cost).
- `sel_history` removal compiles (no reader remains — the full §2.3 census is edited).

Decline (F3):
- `select_sentence` on a heading / list item / code block → `Noop`, selection unchanged, status names
  the block kind; on prose → the content-only sentence.
- `move_sentence_up`/`split_sentence_at_caret` on non-prose → `Noop` + status (mutations decline too).

Reorder + swap:
- `move_sentence_down` swaps A/B preserving the gap; caret+selection land on the MOVED sentence;
  press twice slides the same sentence two positions; at the paragraph's last sentence → `Noop` +
  edge status. `move_sentence_up` mirror.
- `swap`: exchanges selection ⇄ marked block (one undo unit); **overlap → `Noop` + loud status, NO
  mutation** (asserts we never hit `build_multi_replace`'s silent guard); missing region → status;
  post-op selection holds the moved selection-content, marked_block cleared; undo restores and leaves
  no stale mark.

Joint edits + gap fate (the whitespace-doubling guardrails):
- `break_paragraph_here`: "A. B." caret in "B" → "A.\n\nB." (single separator consumed, no leading/
  trailing space); already-at-paragraph-start → `Noop`; promoted sentence selected.
- `merge_paragraph_forward`: two paragraphs → one, separator becomes ONE space; no next paragraph →
  `Noop`; next block a heading → `Noop` + status; absorbed first sentence selected.
- `split_sentence_at_caret`: mid-sentence caret → period + space + capitalized next word; caret before
  a space → no double space; at a sentence edge → `Noop`; second sentence selected.
- Each edit is ONE undo unit (undo restores byte-identical).

Counts:
- segment shows "N words · N sentences · N chars", selection-aware (extend
  `word_count_segment_selection_aware`); `count_region` posts the same over selection-or-buffer.

Folds (F7 / probes):
- **Probe 2 (TestBackend):** a folded, selected section highlights the folded heading row and draws no
  hidden rows.
- **Probe 3 (unit):** fold a section, `select_section` → mark → `block_move` (and `swap`); assert the
  section's heading anchor is in `folds.folded()` at its NEW byte and the tree did not panic; assert a
  non-heading stale anchor is NOT fabricated.
- **Double-fold guard (Codex-r1 finding 1):** construct the case where the moved section's stale origin
  point IS a valid heading after the move (a heading shifts up into the vacated slot); assert
  `folds.folded()` contains the destination anchor and does NOT also contain the stale point — exactly
  one fold, not two.
- **Two-phase swap fold (Codex-r2 finding 2 / C-12):** swap two folded sections positioned so R1's
  stale collapse point equals R2's destination byte; assert BOTH destinations end folded (the
  correction did not self-clobber) — the test that would fail under an interleaved per-region loop.
- **Relayout after correction (Codex-r3 finding 4 / C-13):** after a folded-section move, assert the
  rendered fold view reflects the CORRECTED fold state — the destination heading's body lines are
  hidden (`active_fold_view().is_hidden(line)` true; its `line_layouts` collapsed) — proving the STEP-4
  relayout rebuild ran (a single-rebuild implementation would show the destination UNFOLDED in the
  view).
- **Table reads as prose (Codex-r3 finding 2):** `select_sentence` with the caret in a Markdown table
  cell does NOT decline (`role_at` → `Paragraph`); `select_sentence` on a `FrontMatter` / HTML-comment
  block DOES decline (front-matter/comment roles).
- `select_section` byte range covers the whole subtree whether folded or open.

SEE==SELECT (probe 1):
- **TestBackend:** with the lens ON, `select_sentence` a sentence that wraps to multiple ventilated
  rows; assert the `SE::Selection` highlight appears on every row of the group (global-offset paint).
- The window used by `select_sentence` equals the window `ventilate::prose_block_at` returns at the
  same caret (SEE==SELECT single-source — assert equal ranges).
- **Indented prose (C-11):** an indented paragraph (1–3 leading spaces — CommonMark strips them from
  the block span); assert `select_sentence` with the caret in it selects the sentence (does NOT decline
  via the gap fallback), and the window equals the lens's `ventilated_window`/`prose_block_at` result at
  the content byte — the classification-at-line-start failure this fix prevents.

B10:
- caret at `buf.len()` maps to the trailing/phantom line (fix); a non-EOF caret's line is unchanged;
  assert identically with the lens OFF (no regression) and ON.

e2e (`e2e.rs`, in-process `reduce → advance → render` on `TestBackend`):
- lens ON → `select_sentence` → `move_sentence_up` → the gutter row order changes → `undo` → buffer
  byte-identical and the lens re-derives cleanly (no blank paragraph, no panic).

**Limitations stated for the gate:** the fold-move re-apply (§7.3) is asserted at the `FoldState` level
(anchor membership), not via a terminal screenshot; the paint probes use `TestBackend` cell inspection
(the `row_spans_placed` precedent), which is the available mechanism.

---

## 12. Consequences-of-grounding, collected (for the Codex gate)

- **C-1** S5 spans are content-only → every mutation site pins GAP FATE (§5.4).
- **C-2** `build_multi_replace` silently identity-no-ops a malformed/overlapping list → `swap` rejects
  overlap LOUDLY before calling it (§4.2).
- **C-3** `scope_range_at(Section)` is O(headings)+alloc → cold path only (command/expand-press), within
  the perf rule (§3.1).
- **C-4** `select_sentence` on non-prose now DECLINES (was a confident non-answer) → behavior, not
  surface; no contract clause touched (§3.3).
- **C-5** Stateless shrink returns canonical rungs, not exact prior ranges (human-accepted); evaluated
  at the selection's `from()` (head-at-start), so a paragraph-expand shrinks to the paragraph's FIRST
  sentence (§3.4).
- **C-6** `move_sentence_*` lands caret+selection on the MOVED sentence (unlike `transpose_words`,
  which lands after the pair) → a new handler, not a `transpose_words` call (§4.1).
- **C-7** **Fold survival across a section move is NOT automatic, and add-only re-fold is wrong** —
  `remap` collapses a moved section's anchors to the stale origin and KEEPS them; `reconcile_to` may
  drop them (→ unfolded) or keep them (→ double fold if you only add at the destination). S4 uses a
  **REMOVE-then-ADD** correction guarded by `heading_starts` (§7.3). **The headline finding**
  (corrected per Codex finding 1).
- **C-8** M6 (generic cut of a content-only sentence selection) is where SEE==SELECT and clean-cut pull
  apart; resolved in favor of content-only selection, gap-tidying → Effort-P Lua (§5.4).
- **C-9** **F8 "caret at head" = build the selection head-at-start** (`Selection::range(to, from)`,
  because `Selection::range(anchor, head)` puts the caret on the 2nd arg — `selection.rs:63`).
  `set_selection_range` is changed to this form, which also makes expand/shrink evaluate
  `scope_range_at` from INSIDE the span (never at the exclusive `body.end`) — Codex finding 4.
  Sub-decision: this moves the shipped `select_word/sentence/paragraph` caret from the selection END to
  its START (deliberate; ladder-robustness + F8 consistency).
- **C-10** **Deleting the `mouse.rs` `sel_history` PUSH** means a mouse/hand-made selection is no longer
  a shrink TARGET (shrink lands on canonical rungs) — but expand still grows from the current mouse
  selection (reads the selection, not the stack). This is F4's already-accepted cost, not a regression
  — Codex-round-1 finding 2. Full touch-site census (incl. `mouse.rs` + `prompts.rs::goto_line`) in §2.3.
- **C-11** **`prose_sentence_at` classifies at the caret line's first non-whitespace CONTENT byte**
  (`ventilate::line_content_byte`), NOT the logical line-start — CommonMark strips block indent, so a
  line-start classification hits `paragraph_range_at`'s gap fallback and diverges from the lens on
  indented prose. SEE==SELECT correctness fix (Codex-round-2 finding 1); mirrors S6's
  `ventilated_window` (§2.2, §3.3, §8). `saturating_sub` guards the caret-in-indent offset.
- **C-12** **The C-7 fold correction is TWO-PHASE** — remove ALL captured stale collapse points across
  all moved regions FIRST, then add ALL destination folds. In a two-region `swap`, R1's stale collapse
  point can EQUAL R2's destination (ascending `build_multi_replace` edits + `map_pos_before`'s
  delete-to-start mapping), so an interleaved per-region loop would self-clobber the swap. Order-
  independent via the phase split (or `replace_folded`) — Codex-round-2 finding 2 (§7.3, §4.2).
- **C-13** **The C-7 fold correction is bracketed by TWO rebuilds** — `editor.apply` does not reparse,
  so a `derive::rebuild` must precede the correction (settle the tree → valid `heading_starts`) and a
  SECOND `derive::rebuild` must follow it (relayout fold-aware `line_layouts` from the corrected fold
  set, matching shipped fold commands' mutate-then-rebuild). The first is the handler's existing
  post-apply rebuild; the second is added. Cold path, acceptable — Codex-round-3 finding 4
  (§7.3, §4.2).
- **[DECLINE SET, single-source — Codex-round-3 findings 2+3]** S4's non-prose decline set = every
  non-`Paragraph` `BlockRole` (enumerated once in §3.3). A **`Table` reads as PROSE** (no `Table` role;
  ratified = A) — consistent with the lens; true table-decline is **B14**, out of S4 scope.

---

## 13. Out of scope (restated)

Section transpose / re-level / promote-demote (S1) · `TextObject` trait / `ObjectRegistry` / `Affinity`
· `PairedDelimiter` / quotation-emphasis-link objects (BlockTree has no inline nodes; fiction omits
closing quotes; unmatched-quote scan O(document)) · plain-text degradation matrix
(`paragraph_range_at` covers it) · clause anything (post-S7, select-only, D5/D6) · multi-range selection
(S8; core `Selection` is single-range) · **join-sentences** (F6 — no honest case-repair) · in-lens
motion/reflow FEEL (S9) · fused one-gesture ops ("delete sentence" — Effort-P Lua, RULE 10) · gap-tidy
on generic cut (§5.4 M6) · **table-decline** (tables read as prose in S4, consistent with the lens;
adding a non-`Paragraph` role for `BlockKind::Table` is the shipped-S6/core follow-up **B14**).

---

## 14. Self-review (spec checklist)

- **Placeholder scan:** no TODO/TBD/`???` remain. The two "plan finalizes" items (the exact gutter call
  shape onto `region_stats`; the exact `caret_line` clamp rewrite) are mechanical, not open design —
  the eight forks are all settled in §1.
- **Internal consistency:** the selection model (§3) ↔ decline helper (§3.3) ↔ reorder/swap (§4) ↔
  joints + gap table (§5) ↔ counts (§6) ↔ folds (§7) ↔ SEE==SELECT (§8) ↔ commands (§9) all reference
  the SAME `prose_sentence_at` helper, the SAME `LADDER` table, the SAME `region_stats` helper, and the
  SAME two region states; every command in §9.1 has a mechanism section.
- **Scope:** every §0/§1 promise has a mechanism; the OUT list (§13) fences the cuts; the one genuine
  net-new mechanism beyond the decisions' literal text (fold capture/re-apply, C-7) is surfaced, not
  buried.
- **Grounding:** every cited symbol (`sentence_spans`/`sentence_bounds`, `paragraph_range_at`,
  `ventilate::prose_block_at`/`origin_of`, `Scope`/`scope_range_at`/`ExpandSelection`/`ShrinkSelection`,
  `sel_history` + its clear sites, `textops::{scope_or_word, transpose_words, transpose_lines}`,
  `build_multi_replace` + its silent guard, `blocks_marked::{block_move, mark_block_from_selection,
  select_marked_block}`, `outline::{Section, sections, heading_starts}`, `FoldState::{remap,
  reconcile_to, toggle, remove, replace_folded, folded}` + `change::map_pos_before`,
  `render::row_spans_placed`, `render_status::word_count_segment`, `count::{word_count, char_count}`,
  `nav::{caret_line, next_paragraph_start}`, `BlockRole`, `selection::{Range, Selection::range}`
  (head = 2nd arg), `mouse::seed_and_select`, `prompts::goto_line`,
  `ventilate::{line_content_byte (private→pub(crate)), ventilated_window}`, `build_multi_replace`
  ascending-edit order, `block_tree::kind_to_role` (no `BlockKind::Table` mapping; `HtmlComment→Comment`,
  `FrontMatter→FrontMatter`), `style::BlockRole` (no `Table` variant; `FrontMatter`/`Comment` present),
  `editor::Buffer::apply` (no reparse), `derive::rebuild_downstream` (settles `heading_starts` +
  fold-aware `line_layouts`)) was re-verified in the tree for this spec, including the four
  Codex-round-1, three Codex-round-2, and four Codex-round-3 fixes.

---

## History

- **2026-07-14 (spec authored):** the 8 brainstorm decisions (`s4-brainstorm-decisions.md`) rendered as
  LAW. One grounding finding materially extends the decisions' literal text: **C-7** — fold survival
  across a section move is NOT automatic, so §7.3 adds explicit fold handling. All three
  verify-at-spec-time probes resolved by reading (probes 1–2) or grounded as needing new mechanism
  (probe 3); the paint probes are converted to `TestBackend` assertions rather than asserted blind.
- **2026-07-14 (Codex spec-gate round 1 — 4 Important findings folded in):**
  1. **C-7 corrected:** add-only re-fold could DOUBLE-count when the stale origin remains a heading;
     the mechanism is now **REMOVE-then-ADD** (clear the stale-remapped fold via `FoldState::remove`,
     re-fold at the destination via `toggle` guarded by `heading_starts`), grounded that `remap` keeps
     collapsed anchors in the set (§2.6, §7.3, §4.2).
  2. **`sel_history` census completed:** added the previously-missed touch-sites
     `mouse.rs::seed_and_select` (PUSH :58) + its clear (:622) + test (:840) and
     `prompts.rs::goto_line` (:392); stated that removing the mouse push loses exact-range shrink-
     restore (F4's accepted cost, not a regression) while expand-from-current-selection still works
     (§2.3, §3.4, C-10).
  3. **`split_sentence_at_caret` interior guard fixed** from `head==sf||head==st` to
     **`sf < head && head < st`** — content-only spans attach the trailing gap to the preceding
     sentence, so a caret in the gap has `head > st` and must be rejected (§5.3, §5.4 M5).
  4. **F8 head-at-start wiring:** `Selection::range(anchor, head)` puts the caret on the 2nd arg, so
     `set_selection_range` and every F8 command now build `Selection::range(to, from)` (caret at
     START); this also makes expand/shrink evaluate `scope_range_at` from inside the span, not at the
     exclusive `body.end` (§1 F8, §3.2, §3.4, §4, §5, C-9).
- **2026-07-14 (C-9 ratified = A):** the human ratified caret-at-start EVERYWHERE, including the shipped
  `select_word`/`select_sentence`/`select_paragraph` landing change. No spec change beyond confirming §1
  F8 / §3.4 already reflect it.
- **2026-07-14 (Codex spec-gate round 2 — 2 Important + 1 Minor folded in):**
  1. **C-11 (Important):** `prose_sentence_at` now classifies at the caret line's first non-whitespace
     CONTENT byte (`ventilate::line_content_byte`, mirroring S6's `ventilated_window`), NOT
     `derive::line_start` — CommonMark strips block indent, so line-start classification hits the gap
     fallback and diverges from the lens on indented prose. SEE==SELECT correctness (§2.2, §3.3, §8);
     `saturating_sub` guards the caret-in-indent window offset.
  2. **C-12 (Important):** the C-7 fold correction is made **two-phase** — remove ALL stale collapse
     points across all moved regions FIRST, THEN add ALL destination folds — because in a two-region
     `swap` R1's stale collapse point can equal R2's destination (ascending `build_multi_replace` edits
     + `map_pos_before` delete-to-start), so an interleaved loop would self-clobber (§7.3, §4.2).
  3. **Minor:** the stateless-shrink test-plan line was corrected from "last-sentence landing" to the
     paragraph's FIRST sentence, agreeing with C-5's `from()`-evaluation (§11).
- **2026-07-14 (Codex spec-gate round 3 — 2 Important + 1 Minor + 1 Important; table question ratified
  = A):**
  1. **(Important) Content-byte helper visibility:** `ventilate::line_content_byte` is a private `fn`
     (ventilate.rs:338); the plan makes it `pub(crate)` so `commands.rs`'s `prose_sentence_at` shares
     the ONE implementation (no second copy — SEE==SELECT single-source). §3.3.
  2. **(Important) Decline set = A (tables read as prose):** `BlockRole` has no `Table` variant and
     `kind_to_role` leaves `BlockKind::Table` unmapped (block_tree.rs:255), so a table classifies as
     `Paragraph`/prose and the lens does not decline it — REMOVED "table" from the decline set;
     single-sourced the set in §3.3 as "every non-`Paragraph` `BlockRole`." True table-decline filed
     as **B14** (§13). §1 F3, §2.2, §3.3.
  3. **(Minor) Decline examples completed:** added `FrontMatter` and `Comment` (they map to
     non-paragraph roles, block_tree.rs:263-264, and DO decline today; the full set is enumerated
     once in §3.3). §3.3.
  4. **(Important) Final rebuild after the fold correction (C-13):** grounded that `editor.apply` does
     not reparse and `derive::rebuild` is what settles `heading_starts` AND rebuilds fold-aware
     `line_layouts`; sequenced the C-7 correction as apply → rebuild (settle) → two-phase correction →
     rebuild (relayout). §7.3, §4.2.
