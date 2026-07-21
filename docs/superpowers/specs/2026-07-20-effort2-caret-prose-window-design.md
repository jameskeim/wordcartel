# Effort ② — caret & prose-window bundle: design spec

**Date:** 2026-07-20. **Status:** draft for Codex spec gate.
**Items:** B16 (sentence-highlight window drift), B15 (shrink/expand into a fold), B14 (tables
classify as prose), B10 (EOF caret — record correction + screen-assert coverage).
**Branch (at execute time):** `effort2-caret-prose-window` off main. No source edits before then.
**Grounding inputs:** `scratchpad/effort2-grounding/` (shared-machinery + four per-item sweeps +
`fable-forks.md`), re-verified selectively against main @ `16d0d0d`.

All anchors below are SYMBOL names (file + symbol), never line numbers. Every claim about current
behavior is checkable by reading the named symbol; the few claims that could not be verified by
reading are collected in §10 and labeled.

---

## 1. Summary and locked decisions

Four backlog items, one branch, one pipeline pass. Human-locked decisions (2026-07-20):

1. **B10** — the filed bug is already fixed on main (commit `44eacab`, "S4 T9: B10 — EOF caret
   maps to the trailing phantom line", 2026-07-14). Deliverables: correct the stale backlog
   record (an explicit work item, §7.4/§8), and ADD an e2e `cursor_pos()` screen-assert test —
   the suite currently has zero screen-level caret-cell assertions for this machinery.
2. **B16** — record as an **S4-introduced regression** (severity record change, §8), fix by
   content-anchoring the focus paint window via `commands::prose_window_at`, with the current
   raw derivation kept as the fallback on decline. `FocusGranularity::Paragraph` stays raw.
3. **B15** — **UnfoldTo-and-keep-selection, in BOTH arms** (`Command::ShrinkSelection` and
   `Command::ExpandSelection`). This deliberately departs from the filed fix direction
   (`registry::snap_caret_out_of_fold`), which is unusable here because it collapses the
   selection (§4.2); the human adopted the departure knowingly — it is a product-visible
   behavior change (a shrink/expand whose target head is folded now unfolds that section).
4. **Type tags** — none this effort. A future H-item is proposed in §9 (not added to
   `backlog.toml` by this effort).
5. **B14** — add `BlockRole::Table`, map `BlockKind::Table` in `kind_to_role`, paint as
   `SemanticElement::Text` (byte-identical painting off-lens; no theme cascade). Must also heal
   `lenses.rs::prose_paragraph_ranges` (which it does automatically — §4.3).
6. **Verification** — value tests plus targeted e2e screen asserts (§6).
7. **Scope** — bundle stays whole. Command-surface contract: **N/A** (§5).

---

## 2. Current behavior (grounded, symbol-anchored)

### 2.1 The focus paint window (B16)

`wordcartel/src/render.rs::gather_row_ctx` computes `RowCtx.focus_region` when
`editor.view_opts.focus` is on. Its `crate::config::FocusGranularity::Sentence` arm derives the
window as raw `nav::paragraph_range_at(blocks, buf, head)`, slices it, and calls
`wordcartel_core::textobj::sentence_bounds(&win, head - ps)` — note the **bare** `head - ps`
subtraction, safe today only because the raw window's `ps <= head` always holds (see §4.1).

The select side no longer does this. `wordcartel/src/commands.rs::prose_window_at` classifies
and windows at the caret line's first non-whitespace CONTENT byte
(`ventilate::line_content_byte` → `ventilate::prose_block_at`), and
`commands::prose_sentence_at` segments within that window using `h.saturating_sub(ps)`,
returning `Err(NonProse(role))` on a non-`Paragraph` role. `commands::scope_range_at`'s
`Scope::Sentence` arm is literally `prose_sentence_at(editor, h).unwrap_or((h, h))`.

**History (verified in git, drives the severity record):** at the S4 merge base `ef03888`, BOTH
sides used raw `nav::paragraph_range_at` — the merge-base `Scope::Sentence` arm in
`commands.rs` and the merge-base `FocusGranularity::Sentence` arm in `render.rs` are
mechanically identical, so no divergence existed. S4 commits `600cb92` (content-anchored
`prose_sentence_at`) and `2cece7e` (single-sourced `prose_window_at` + mutation rerouting)
moved SELECT to content anchoring and did not touch the paint arm. The drift is the delta S4
created: **an S4-introduced regression**, contradicting the filing's "PRE-EXISTING" claim.
(The filing's anchor "`render.rs` ~:505" has also drifted; the arm now lives inside
`gather_row_ctx`.)

Where it bites: with the caret inside a ≤3-space CommonMark leading indent, the raw call misses
the Paragraph block (`nav::paragraph_range_at`'s `deepest_block_at` requires
`pos >= block.span.start`, and the block span starts AFTER the indent strip) and falls into the
gap fallback — a maximal run of non-blank LOGICAL lines, which can include adjacent non-prose
lines (e.g. a heading directly above the indented paragraph with no blank line between). The
painted "sentence" then differs from what `select_sentence` selects.

`FocusGranularity::Paragraph` paint is raw AND `Scope::Paragraph` select is raw
(`scope_range_at`) — they agree today; changing Paragraph paint alone would CREATE this drift
class, so it stays raw (locked).

### 2.2 Shrink/Expand and folds (B15)

`commands.rs::Command::ShrinkSelection` and `Command::ExpandSelection` walk `LADDER`
(`const LADDER: &[Scope]` — Word, Sentence, Paragraph, Section, Document) via
`scope_range_at(editor, from, sc)` with `from = cur.from()`, then call
`set_selection_range(editor, f, t)`. Per `set_selection_range`'s C-9 doc comment, the head
lands at `from` (`Selection::range(to, from)`). **Neither arm contains any
`normalize_caret` / `FoldView::is_hidden` / `place_caret_visible` call** (verified by reading
both arms; also confirmed by the repo-wide call-site sweep in the B15 grounding report).

So when `cur.from()` sits inside a folded body (a modeled, guarded occurrence elsewhere —
session resume test `resume_snaps_saved_cursor_out_of_restored_fold` in `app.rs`; guards on
`Command::Move`, Undo/Redo, `mouse.rs::visible_doc_end`, `blocks_marked.rs::block_move`,
`prose_ops.rs::swap`), a Word/Sentence/Paragraph rung's `(f, t)` also starts inside the folded
body and the head lands on a `FoldView::is_hidden` line. Section/Document rungs are safe by
construction (`section_range_at` returns the heading byte, and heading LINES stay visible when
folded — `FoldView` hides body lines only; `Scope::Document` returns `(0, len)` and line 0 is
never a hidden body line).

**The filed fix's hole (why the design departs):** `registry.rs::snap_caret_out_of_fold`
assigns `Selection::single(snapped)` when it snaps — it is a caret-placement guard for
caret-moving commands (block_move / swap / resume). Applied after Shrink/Expand it would
destroy the selection those commands just produced, precisely in the trigger case.

The reveal machinery that DOES fit: `registry.rs::place_caret_visible` with
`CaretPlace::UnfoldTo` calls `registry.rs::unfold_ancestors_of(editor, raw)` and returns `raw`
unchanged — no selection write. `unfold_ancestors_of` iterates the folded anchor set and
`FoldState::remove`s every fold whose `outline::body_range` contains the byte (so nested folds
all open); it does NOT rebuild — its existing callers rebuild afterwards. `FoldState::remove`
bumps `epoch` (verified: every `FoldState` mutator bumps `epoch` on change, per the "sole write
path" comment in `fold.rs`), so `Editor::active_fold_view()`'s `(blocks_generation, epoch)`
cache key invalidates correctly, and `set_selection_range`'s `derive::rebuild` +
`nav::ensure_visible` then see the post-unfold state.

### 2.3 Table classification (B14)

`wordcartel_core::style::BlockRole` has 8 variants — Paragraph, Heading(u8), BlockQuote,
ListItem, CodeBlock, ThematicBreak, FrontMatter, Comment — no Table.
`block_tree.rs::kind_to_role` maps eight kinds across seven arms (`FencedCode |
IndentedCode` share one) and ends `_ => None`; `BlockKind::Table` falls through, so `block_tree.rs::role_at` (default `best = Paragraph`, upgraded only via
`collect_role` when `kind_to_role` returns `Some` with lower `role_precedence`) returns
`Paragraph` over table bytes. The tree really does produce Table blocks:
`block_tree.rs::options()` inserts `Options::ENABLE_TABLES`, and the event mapper has
`Tag::Table(_) => BlockKind::Table`.

Consequently `ventilate::prose_block_at` (sole test: `role_at(byte) != BlockRole::Paragraph`)
never declines a table → the lens ventilates it; `commands::prose_window_at` /
`prose_sentence_at` and every S4 mutation windowing through them — FOUR, not the three the
grounding sweep enumerated: `prose_ops.rs::move_sentence`, `break_paragraph_here`,
`merge_paragraph_forward`, and `split_sentence_at_caret` (registered as "Split Sentence";
it too calls `super::prose_sentence_at`) — treat table rows as prose; and `lenses.rs::prose_paragraph_ranges` — whose own doc comment says it must NOT
analyze tables — feeds table rows to the POS/diagnostics sweep.

Blast radius of a new variant (verified in-tree; corrects the B14 sweep's count of 4): exactly
FIVE truly-exhaustive `BlockRole` matches force new arms — four production sites
(`block_tree.rs::role_precedence`, `commands.rs::block_kind_label`,
`render.rs::role_element`, `render.rs::prefix_element`) plus one test-module compile guard,
`style.rs::tests::_exhaustive_block_role`, whose match names every `BlockRole` variant by
design and fails to compile until it gains a `Table` arm.
`md_parse.rs::apply_block_prefix_conceal` ends `_ => None` and silently (correctly: no conceal)
absorbs the variant. `wordcartel_core::theme::SemanticElement` has no Table variant and is NOT
touched (we map to the existing `SemanticElement::Text` — §4.3), so the 17 theme constructors
and `ALL_ELEMENTS` are untouched. `layout.rs` consults the role only via
`matches!(role, BlockRole::Heading(_))` / `matches!(role, BlockRole::CodeBlock)` — Table
matches neither, so wrap/layout behavior is unchanged relative to today's `Paragraph`
classification.

### 2.4 EOF caret (B10) — already fixed; record stale

`nav::caret_line` reads `buf.byte_to_line(h.min(buf.len()))` with an explicit
`// B10: do NOT clamp to len-1` comment; the pre-fix `len-1` clamp was removed by `44eacab`
(S4 T9). Pinning value tests exist and pass (`nav::tests::
caret_line_at_eof_maps_to_phantom_line_not_last_content`,
`nav::tests::trailing_space_before_eof_stacks_flush_row_above_b10_phantom_line`,
`ventilate::tests::t_i1_eof_phantom_is_own_entry_not_a_zero_row_prose_overwrite`).
`docs/backlog-archive.md`'s S4 entry lists "B10 EOF caret fix" among the deliverables. Yet
`backlog.toml`'s B10 block still reads `status = "triage"` with a hook describing the pre-fix
`h.min(len-1)` mechanism, and the `docs/ux-backlog.md` B10 section is an unfleshed stub.

Test-surface fact motivating the new coverage (shared sweep): zero of the ~345 tests co-located
in the coordinate-space modules render a screen; `e2e.rs::Harness::cursor_pos` — the one
primitive that reads the reported terminal caret cell — is used only in cursor-picker tests.

---

## 3. Design principles for the bundle

- **Single-source over parallel derivation.** B16 re-uses `commands::prose_sentence_at` — the
  exact function `Scope::Sentence` select uses — rather than re-deriving "the same" window in
  render. B16 existed because two sites derived "the same" window independently; the fix must
  not create a third derivation.
- **Reveal, don't repair.** B15 prevents the hidden-head state before the selection is set
  (UnfoldTo prior to `set_selection_range`), instead of post-hoc snapping that destroys state.
- **Fix the classifier, not the consumers.** B14 lands at `kind_to_role`/`BlockRole` so every
  `role_at` consumer heals at once; no consumer-local `BlockKind` checks.
- **Preserved invariants:** SEE==SELECT (extended to paint), lens no-op-when-off
  (`ventilate::set_ventilate` still touches only view state), R1 (no-folds paths do no fold
  walks — B15's guard is gated on a hidden head), caret-never-in-a-fold, per-keystroke
  O(visible)+O(edited) (§4 cost notes), lazy reparse (nothing here touches non-active buffers).

---

## 4. Design

### 4.1 B16 — content-anchor the sentence focus window

**Change (one arm):** in `render.rs::gather_row_ctx`, replace the
`crate::config::FocusGranularity::Sentence` arm body with:

```rust
crate::config::FocusGranularity::Sentence => {
    match crate::commands::prose_sentence_at(editor, head) {
        // SEE==SELECT: the painted active-sentence region IS the span select-sentence
        // would select — same window (prose_window_at), same segmentation.
        Ok(r) => r,
        // Decline (heading/list/code/blockquote/front-matter/comment/table, or a
        // content-less line): keep the pre-S4 raw derivation so focus dimming still
        // functions on non-prose blocks, exactly as it does today.
        Err(_) => {
            let (ps, pe) = nav::paragraph_range_at(blocks, buf, head);
            let win = buf.slice(ps..pe);
            let (sf, st) =
                wordcartel_core::textobj::sentence_bounds(&win, head.saturating_sub(ps));
            (ps + sf, ps + st)
        }
    }
}
```

Notes for the implementer/reviewer:

- `commands::prose_sentence_at` and `commands::NonProse` are `pub`; render.rs already calls
  `crate::commands::prose_sentence_at` in one of its own tests, so the dependency direction is
  established.
- **Underflow guard is load-bearing:** the content-anchored window start `ps` can exceed `head`
  (caret inside the indent). `prose_sentence_at` already uses `h.saturating_sub(ps)`
  internally. A hand-rolled "call `prose_window_at` then subtract" would reintroduce a
  `head - ps` underflow — this is why the arm routes through `prose_sentence_at`, not just
  `prose_window_at`. In the fallback arm `ps <= head` holds (a raw
  `nav::paragraph_range_at(pos)` window always starts at a block span or line-run start at or
  before `pos`), so `saturating_sub` there is pure H7-style hardening on a measure path
  (identical result on the invariant, safe clamp off it), not a behavior change.
- **Decline-path behavior is today's behavior:** for a caret on a heading/list/etc., or on a
  blank/phantom line (`ventilate::line_content_byte` → `None` → `prose_window_at` → `None`),
  the fallback computes exactly what the arm computes today. Paint and select still disagree on
  DECLINED blocks (select refuses, paint dims a raw window) — deliberate: focus dimming on a
  heading is a feature, and that asymmetry predates S4.
- **Cost:** runs only while `view_opts.focus` is on, once per rendered frame (frames are
  input-driven; idle stays free). Today's arm already calls `nav::paragraph_range_at` per such
  frame; the new arm calls it once inside `prose_window_at` (Ok path) or twice (decline path),
  plus `TextBuffer::byte_to_line` + `ventilate::line_content_byte` (one line's text) +
  `BlockTree::role_at`. Same asymptotic class as today; no per-keystroke O(document) added.
- `FocusGranularity::Paragraph` arm: untouched (locked; it agrees with `Scope::Paragraph`
  select today).

### 4.2 B15 — reveal the target head before setting the selection (both arms)

**Change:** in `commands.rs`, add one small private helper and call it from BOTH the
`Command::ShrinkSelection` and `Command::ExpandSelection` arms, after a rung `(f, t)` is
chosen and BEFORE `set_selection_range(editor, f, t)`:

```rust
/// B15: a rung derived from a `from()` inside a folded body has its head (`f`, per C-9
/// head-at-start) on a hidden line — typing there would edit invisible text. REVEAL the
/// target (UnfoldTo), do not snap: `registry::snap_caret_out_of_fold` collapses to
/// `Selection::single`, which would destroy the selection expand/shrink just derived.
fn reveal_selection_head(editor: &mut Editor, f: usize) {
    if editor.active().folds.is_empty() { return; } // no folds → no walk (R1 discipline)
    let line = {
        let buf = &editor.active().document.buffer;
        buf.byte_to_line(f.min(buf.len()))
    };
    if editor.active_fold_view().is_hidden(line) {
        crate::registry::place_caret_visible(editor, f, crate::registry::CaretPlace::UnfoldTo);
    }
}
```

and in each arm:

```rust
Some((f, t)) => {
    reveal_selection_head(editor, f);
    set_selection_range(editor, f, t);
    CommandResult::Handled
}
```

Notes for the implementer/reviewer:

- **Why UnfoldTo, not SnapOut (the selection-collapse hole, stated for both arms):**
  `snap_caret_out_of_fold` and the `CaretPlace::SnapOut` path both funnel a single BYTE and,
  in the former, write `Selection::single` — correct for the caret-placement commands that
  ship them (block_move, swap, resume, Undo/Redo), destructive after a selection-producing
  command. Shrink's trigger case would go "selection shrunk → selection gone"; Expand's would
  go "selection grown → selection gone". UnfoldTo (`unfold_ancestors_of`) mutates only the fold
  set and leaves the selection alone; the user deliberately operated on this text, so revealing
  it matches the existing UnfoldTo precedents (`blocks_marked.rs` block_jump, `marks.rs`,
  `prompts.rs`).
- **Ordering:** the reveal runs BEFORE `set_selection_range`, so the single `derive::rebuild`
  + `nav::ensure_visible` inside `set_selection_range` observes the post-unfold fold set
  (epoch bumped by `FoldState::remove`) — no second rebuild, no stale `FoldView` (the
  `active_fold_view` cache keys on `(blocks_generation, folds.epoch())`).
- **Guard shape:** `folds.is_empty()` fast-out keeps the no-folds path allocation-free
  (`unfold_ancestors_of` clones the block tree + buffer up front, acceptable on this
  command-cold path but pointless with no folds); the `is_hidden` check makes the unfold fire
  only when the head would actually be hidden, so ordinary expand/shrink with folds elsewhere
  in the document does not open anything.
- **Both arms, same helper.** The hole is symmetric: both arms place the head at `f`
  (`set_selection_range` → `Selection::range(to, from)`, C-9). Expand from a VISIBLE caret
  cannot land hidden (a fold hides a heading's whole `body_range`; a
  Word/Sentence/Paragraph window containing a visible line starts on a visible line, and
  Section/Document heads are heading-byte/0) — the guard fires only when `from()` was already
  inside a fold, e.g. a resumed or pre-fold selection. Judgment, stated as such: cheap to guard
  both arms uniformly rather than prove Expand unreachable forever.
- **Expand-to-Section note (not a change):** a Section rung's head is the heading byte
  (visible even when folded); the selection interior may span hidden lines with a visible
  head — same as today's select-all-over-folds semantics. The invariant protected here is
  "the HEAD (typing point) is never hidden", not "a selection never spans a fold".
- **Product-visible behavior change (human-approved):** shrink/expand whose chosen rung starts
  inside a folded body now opens that fold (and its folded ancestors) instead of leaving the
  caret on a hidden line. The filed fix direction (snap-out) is superseded — record this in the
  item's prose at ship time (§8).
- **No registry/keybinding change:** both commands keep their registration, labels, bindings.

### 4.3 B14 — `BlockRole::Table`

**Changes (1 variant + 6 match-arm updates). Enforcement is NOT uniform:** the compiler
forces five of the six arm updates (changes 3–7 — the exhaustive `BlockRole` matches). It
does NOT force change 2, the central fix: `kind_to_role` matches over `BlockKind` (where
`Table` already exists as a variant) and ends `_ => None`, so omitting the new mapping arm
still compiles — and tables silently keep classifying as prose. Change 2 is therefore
TEST-guarded, not compiler-guarded: §6's B14 value test (i) (`role_at` returns
`BlockRole::Table` on the GFM fixture) fails whenever the arm is absent, because `role_at`'s
`best` then never upgrades from the `Paragraph` default. Per TDD that test is written first
and observed red against the missing arm.

1. `wordcartel-core/src/style.rs` — add `Table` to `BlockRole`, doc-commented: a GFM table;
   verbatim-class for prose purposes (never a prose window), painted as plain text.
2. `wordcartel-core/src/block_tree.rs::kind_to_role` — add
   `BlockKind::Table => Some(BlockRole::Table)` above the `_ => None` catch-all. (`Document`,
   `Paragraph`, `List`, `HtmlBlock`, `Other` still fall through — unchanged.)
3. `wordcartel-core/src/block_tree.rs::role_precedence` — add `BlockRole::Table => 0`.
   **DERIVED, not arbitrary:** lower rank wins in `collect_role`, and a GFM table can nest
   inside a ListItem (3) and a BlockQuote (4), so Table's rank must be strictly below 3 for a
   nested table byte to classify as Table. The tie with CodeBlock (0) is unreachable: in
   pulldown-cmark's block model neither a code block nor a table nests inside the other
   (background-knowledge claim — §10; the plan pins the reachable half with a
   table-inside-blockquote `role_at` test).
4. `wordcartel/src/commands.rs::block_kind_label` — `BlockRole::Table => "table"`. Side effect
   (desired): `prose_sentence_at`'s decline surfaces as "no sentence here (table)" through the
   existing `NonProse` status path.
5. `wordcartel/src/render.rs::role_element` — `R::Table => SE::Text`. **This is what makes the
   change visually inert off-lens:** table bytes classify `Paragraph` → `SE::Text` today, so
   painting is byte-identical; no `SemanticElement` variant, no theme constructor, no
   `ALL_ELEMENTS` edit.
6. `wordcartel/src/render.rs::prefix_element` — add `Table` to the existing
   `Paragraph | CodeBlock | FrontMatter | Comment => SE::ListMarker` arm (the value that arm
   already yields for the Paragraph classification tables get today).
7. `wordcartel-core/src/style.rs::tests::_exhaustive_block_role` — the test-module compile
   guard that names every `BlockRole` variant: add a `Table` arm (next ordinal). REQUIRED
   migration site — omitting it fails `cargo test --no-run` for `wordcartel-core`. (Its
   sibling `_exhaustive_line_render` guards `LineRender`, not `BlockRole` — untouched.)

**Explicit non-changes, checked:** `md_parse.rs::apply_block_prefix_conceal` — its `_ => None`
absorbs Table with no conceal, which is today's behavior for table lines (they classified
`Paragraph`, also the catch-all); additionally `md_parse.rs`'s own parser options do not enable
tables, and roles are not produced there. `layout.rs` — Table matches neither
`BlockRole::Heading(_)` nor `BlockRole::CodeBlock` in its `matches!` sites, so soft-wrap
behavior of table lines is unchanged. `block_tree.rs`'s incremental/splice grouping that names
`BlockKind::Table` operates on `BlockKind`, not `BlockRole` — untouched.

**Consumers healed with zero code change (each gets a pinning test, §6):**
- `ventilate::prose_block_at` declines (`role_at != Paragraph`) → the lens renders table lines
  through its existing per-line verbatim arm; `set_ventilate` still touches only view state
  (no-op-when-off preserved).
- `commands::prose_window_at` / `prose_sentence_at` → `None` / `Err(NonProse(Table))` → all
  FOUR S4 mutations decline (`move_sentence`, `break_paragraph_here`,
  `merge_paragraph_forward`, `split_sentence_at_caret` — the last is "Split Sentence" in the
  registry and shares the same `prose_sentence_at` chain). Their decline statuses are the
  mutations' own plain messages ("no sentence here" / merge's "no paragraph here"); the
  kind-labeled "(table)" message belongs to the select path only (§7.3.2).
- `lenses.rs::prose_paragraph_ranges` (`role_at(b.span.start) == Paragraph` keep-filter) now
  skips table blocks — the POS/diagnostics sweep stops analyzing table rows (the consumer the
  filing missed; locked decision 5 requires it healed).
- `nlp.rs::nlp_window_at` opens with `commands::prose_window_at(editor, h)?`, so it heals via the
  same `None`-on-table path — no separate pin (its only table-relevant logic IS `prose_window_at`,
  exercised by that consumer's §6 pin; and `nlp_window_at_off_prose_returns_none` already pins the
  `None` propagation).
- Safe-by-construction sites (equality checks against `Paragraph`) need no edit; the five
  exhaustive `BlockRole` matches (§2.3's census — the four production sites in changes 3–6
  plus the `_exhaustive_block_role` test guard in change 7) are compiler-forced; no further
  exhaustive `BlockRole` match was found in-tree, and the compiler is the final census
  (re-checkable by building). The one NON-compiler-forced site is change 2's `kind_to_role`
  arm, guarded by the red-first `role_at` value test (see the enforcement note atop this
  change list).

**Core-crate note:** this is an additive enum variant in the fuzzed crate; the existing oracle
and property suites (`wordcartel-core` lib + `block_tree_oracle`) run in the normal gates.

### 4.4 B10 — record correction + screen-assert coverage

No behavior change. Two deliverables:

**(a) Backlog record correction (explicit work item, reviewer-visible):**
- `backlog.toml` B10 block: `status = "shipped"`, add `shipped_commit = "44eacab"`,
  `shipped_date = "2026-07-14"` (the fix commit's author date; it reached main via the S4
  merge `10b847e`), repoint `doc = "backlog-archive.md#b10"`, and rewrite the `hook` to state
  the shipped fact (fixed by S4 T9; the filed `h.min(len-1)` mechanism is the PRE-fix code).
  Field shape mirrors the existing shipped B7/B17 blocks.
- Move the B10 prose stub (marker `<!-- item: B10 -->`) from `docs/ux-backlog.md` to
  `docs/backlog-archive.md`, replacing the stub text with a short shipped record (what the
  clamp was, the fix commit, the pinning tests, and that the backlog lagged the code until
  this effort corrected it).
- `scripts/backlog bless` regenerates `BACKLOG.md`; the `wordcartel/tests/backlog.rs`
  schema/bijection/freshness gate must stay green (this IS the acceptance test for the record
  move).
- Timing: this record change lands as its own commit on the effort branch (it documents
  shipped history, independent of the three fixes).

**(b) New e2e screen asserts** (in `wordcartel/src/e2e.rs`, using the existing `Harness`):
caret-at-EOF renders on the row BELOW the last content row, off-lens and on-lens — the first
`cursor_pos()`-based caret-cell pins outside the cursor-picker tests. Details in §6/§7.4.

---

## 5. Command-surface contract conformance

**N/A — this effort does not touch the command surface.** No command is added, removed,
renamed, rebound, or re-registered; no user-settable option, palette row, menu row, or
keybinding hint changes. What DOES change is internal behavior of existing registered
commands, which the contract does not govern: B15 alters `ShrinkSelection`/`ExpandSelection`;
B14's classification change makes the select-sentence path and all four registered prose
mutations ("Move Sentence" family, "Split Sentence" = `split_sentence_at_caret`) decline on
tables — every one an already-registered command whose registration, label, binding, and
menu/palette presence are untouched. B16 is render-internal; B10 is record/tests. None of the
contract's laws 1–10 is implicated. The plan must restate this N/A.

---

## 6. Verification design (value tests + targeted e2e screen asserts)

House rule applied throughout: **tests derive every offset/coordinate from the fixture text
(`text.find(..)`, screen scans for landmark substrings) — no bare numeric position constants.**
Rationale: a bare constant in an acceptance criterion broke a prior effort; §7 states each
expected value as DERIVED with its derivation.

New tests (names indicative; final names at plan time):

- **B16 value:** a `render.rs` unit test (module-internal, so it can read
  `gather_row_ctx(..).focus_region`) on the fixture
  `"# Head\n  Para one goes on. It then ends.\n"` with the caret INSIDE the 2-space indent:
  asserts `focus_region == Some(prose_sentence_at(&editor, head).unwrap())`. Pre-fix this
  fails: the raw gap fallback (caret byte < paragraph block start) windows over the non-blank
  line RUN including the heading line, so the painted "sentence" absorbs `# Head`.
- **B16 e2e screen:** same fixture family, `view_opts.focus = true`,
  `focus_granularity = Sentence`, no-color theme + `Depth::None` (the established pattern from
  `e2e_focus_dims_right_rows_under_ventilate_indented`, whose caret sits in CONTENT — the
  in-indent case is exactly the uncovered trigger), caret in the indent: assert the heading
  row's content cells ARE dimmed (outside the focus region) and the first sentence's cells are
  not. Pre-fix the heading row is undimmed (it is inside the mis-derived region).
- **B15 value (per arm):** fixture with a folded section and a selection whose `from()` is
  inside the folded body (set `Selection` directly + `FoldState` fold + rebuild — the
  resume-test pattern). After `Command::ShrinkSelection`: selection equals the expected rung
  (derived via the same `scope_range_at` walk), NOT collapsed (`from() != to()`), head's line
  `!is_hidden`, and the covering fold was removed (`folds.folded()` no longer contains the
  heading byte). Symmetric test for `Command::ExpandSelection` starting from a collapsed caret
  inside the folded body.
- **B15 regression guard:** all existing expand/shrink tests stay green UNMODIFIED (the
  no-folds path is behavior-identical; the `folds.is_empty()` fast-out is the structural
  reason). A small test pins that a shrink with folds elsewhere (head not hidden) does not
  change `folds.epoch()`.
- **B15 e2e screen:** fold a section, place the selection inside it, shrink; assert the
  selected sentence's text IS on screen (`screen_contains`) and `cursor_pos()` sits on a row
  containing it. Pre-fix the text is absent from the screen while the selection claims it.
- **B14 value:** (i) `role_at` on a byte inside the GFM fixture
  `"| A | B |\n|---|---|\n| 1 | 2 |\n"` (the fixture family already used by
  `full_parse_src_gfm_table` and the oracle) returns `BlockRole::Table` — this test is the
  DESIGNATED GUARD for the one non-compiler-forced migration site (§4.3 change 2, the
  `kind_to_role` arm): with that arm omitted the build stays green but this test fails,
  since `role_at` never upgrades table bytes off the `Paragraph` default; written first and
  observed red per TDD; (ii)
  `prose_block_at` at a table line's `line_content_byte` returns `None`; (iii)
  `prose_sentence_at` returns `Err(NonProse(BlockRole::Table))` and the SELECT-path decline
  status names "table" (the mutations' own plain messages do not — §7.3.2); (iv) each of the
  FOUR prose mutations (`move_sentence`, `break_paragraph_here`, `merge_paragraph_forward`,
  `split_sentence_at_caret`) declines on a table with the buffer byte-identical (pins the
  previously UNVERIFIED "non-destructive inside a table" claim over the full mutation set);
  (v) `prose_paragraph_ranges` excludes the table span; (vi) a table nested in a blockquote
  still classifies Table (the `role_precedence` derivation's reachable half).
- **B14 lens/e2e:** ventilate ON over prose + a table: the table rows render verbatim (screen
  contains the literal `| A | B |` row; the prose paragraph is still ventilated), and
  `view.vent_blocks` has no entry anchored on a table line.
- **B10 e2e (off-lens):** fixture `"alpha\nbeta\n"`, caret at `buf.len()` (derived:
  `text.len()`): `cursor_pos().1` equals 1 + the row found by scanning for `"beta"`, and
  `cursor_pos().0` equals the column where `"alpha"` starts on its row (the text left margin —
  derived by locating the landmark, not hardcoded, since chrome rows/margins offset the grid).
- **B10 e2e (on-lens):** same doc + `view.ventilate = true`: cursor row equals 1 + the last
  row containing any paragraph content (derived by screen scan) — the phantom line stays its
  own entry below the ventilated block (the `t_i1_*` value test's screen-level twin).

Merge gates unchanged: `cargo test` all suites, warning-free builds, workspace clippy clean,
module budgets (render.rs gains ~a dozen production lines in `gather_row_ctx` — headroom per
the hub budget test is the binding check at plan time), PTY smoke run-and-quote (advisory).

---

## 7. Acceptance criteria

Constants policy: every expected value below is DERIVED (derivation stated inline); tests
compute them from the fixture rather than embedding literals.

### 7.1 B16
1. With focus on, granularity Sentence, and the caret anywhere in an indented paragraph
   (including INSIDE the ≤3-space indent), `RowCtx.focus_region` equals
   `prose_sentence_at(editor, head)`'s `Ok` span exactly (derivation: both sides call the same
   function post-fix; the test computes the expected span by calling `prose_sentence_at`, never
   by literal offsets).
2. With the caret on a declined line (heading / blank / table after B14), `focus_region`
   equals the raw fallback computation — i.e. today's value (derivation: the test computes
   `nav::paragraph_range_at` + `sentence_bounds` directly and compares).
3. The B16 e2e dim assertions of §6 pass; the pre-fix tree fails the in-indent test (verified
   red-first per TDD).
4. `FocusGranularity::Paragraph` behavior is bit-identical (its arm untouched; existing tests
   green unmodified).

### 7.2 B15
1. Shrink with `from()` inside a folded body: result `Handled`; selection equals the rung the
   LADDER walk yields (derived by calling `scope_range_at` in the test); `from() != to()`
   (anti-collapse — the criterion that distinguishes this design from the filed snap-out);
   `active_fold_view().is_hidden(byte_to_line(head))` is false; the covering fold(s) are
   removed from `folds.folded()`.
2. Expand from a collapsed caret inside a folded body: same five properties for the first
   growing rung.
3. No-folds path: `folds.epoch()` unchanged by expand/shrink; all pre-existing expand/shrink
   tests pass unmodified.
4. Folds elsewhere (head not hidden): no fold is removed (`epoch` unchanged).
5. The B15 e2e screen assert of §6 passes (selected text visible on screen post-shrink).

### 7.3 B14
1. `role_at` returns `BlockRole::Table` for any byte inside a GFM table block, including a
   table nested in a blockquote (derivation: bytes located via `text.find` on the fixture).
2. `prose_block_at` / `prose_window_at` decline on table lines; `prose_sentence_at` yields
   `Err(NonProse(BlockRole::Table))`; the SELECT-path decline status (the
   `block_kind_label`-formatted "no sentence here (…)" message in `commands.rs`) contains the
   substring "table" (derivation: `block_kind_label(BlockRole::Table)` — the label IS the
   constant under test). The mutations' own decline messages are their existing plain strings
   and are NOT expected to name the kind.
3. All FOUR prose mutations routing through `prose_sentence_at`/`prose_window_at` —
   `move_sentence`, `break_paragraph_here`, `merge_paragraph_forward`, and
   `split_sentence_at_caret` ("Split Sentence") — decline on tables with the buffer
   byte-identical before/after (derivation: compare full buffer contents, not lengths; the
   set is the §2.3 census, so the "every mutation through `prose_sentence_at` declines"
   claim is backed by a test over the full set).
4. Ventilate ON: table lines render verbatim per-line (no `vent_blocks` anchor on them; the
   literal delimiter row appears on screen); toggling the lens off remains byte-identical
   (existing `set_ventilate` semantics, re-asserted).
5. `prose_paragraph_ranges` output excludes the table span.
6. Off-lens rendering of a table is unchanged: `role_element(BlockRole::Table)` is
   `SemanticElement::Text` and `prefix_element(BlockRole::Table)` is `SemanticElement::
   ListMarker` — the exact values table bytes received under their previous `Paragraph`
   classification (derivation: equality with `role_element(BlockRole::Paragraph)` /
   `prefix_element(BlockRole::Paragraph)` asserted in the test, so the criterion is
   "unchanged", not a named constant).
7. All `wordcartel-core` suites including the block-tree oracle pass; workspace clippy clean.

### 7.4 B10
1. `scripts/backlog open` no longer lists B10; `scripts/backlog shipped` lists it with
   `shipped_commit = 44eacab`; `cargo test` backlog gate (schema + marker bijection across the
   three docs + dashboard freshness) green after the prose move.
2. The two new e2e caret-cell tests pass, with every expected cell derived by screen scan as
   specified in §6 (off-lens: EOF row = 1 + row-of("beta"); on-lens: EOF row = 1 + last
   content row).
3. The B10-specific pins named in §2.4 remain green unmodified — that named list IS the set
   (three tests: the two `nav.rs` pins and the `ventilate.rs` `t_i1_*` pin; DERIVED by
   locating every B10-marked test in the tree, not a remembered count. The adjacent
   `derive.rs::caret_on_phantom_line_conceals_last_content_line` is a ux-H2 pin — its doc
   comment says so — and is deliberately NOT in this set, though it also stays green via the
   normal gates).

---

## 8. Backlog record changes shipped by this effort

- **B10:** as §4.4(a). Part of the effort's reviewed diff, not an aside.
- **B16:** the item's `hook` in `backlog.toml` and its `docs/ux-backlog.md` prose currently
  state "PRE-EXISTING (present at the S4 merge base `ef03888`)". Correct both to record:
  **S4-introduced regression** — at `ef03888` the select arm (`commands.rs` `Scope::Sentence`)
  and the paint arm (`render.rs`, now `gather_row_ctx`) were mechanically identical (both raw
  `nav::paragraph_range_at`); `600cb92` + `2cece7e` rerouted select only. (Locked decision 2:
  a severity-record change, not just prose.) At ship time the corrected prose moves to
  `docs/backlog-archive.md` per house process, alongside B14's and B15's.
- **B15:** at ship time, the archived prose must record that the shipped behavior is
  UnfoldTo-and-keep-selection (human decision 2026-07-20, superseding the filed snap-out
  direction, which was found to collapse the selection via
  `snap_caret_out_of_fold` → `Selection::single`).

---

## 9. Out of scope (with the one required proposal)

- **Position-space type tags: none this effort** (locked). Proposed future item, NOT added to
  `backlog.toml` here: **H35 — "Position-space newtypes: type-level tags for line-relative vs
  window-relative ColMap/Placed.src offsets (the (map, byte_origin) convention)"** — grounding:
  `scratchpad/effort2-grounding/shared-machinery-facts.md` (the seven-space inventory;
  `ventilate::resolve`'s dual-meaning `byte_origin`). Honest evidence note for whoever files
  it: none of THIS bundle's four bugs was a cross-space confusion tags would have caught.
- A themed `SemanticElement::Table` (17-theme + `ALL_ELEMENTS` cascade) — file separately if
  table styling is ever wanted.
- `FocusGranularity::Paragraph` content-anchoring (would desynchronize it from
  `Scope::Paragraph` select — the B16 drift class in reverse).
- Any change to `nav::paragraph_range_at` itself, to `Command::Move`/Undo/Redo fold guards, or
  to the S9/S10 in-lens editing questions.

---

## 10. Claims not verifiable by reading (labeled; none load-bearing without a test)

1. **"Typing at a hidden head edits invisible text" (B15 motivation):** consistent with the
   project's own documented invariant (`app.rs` "caret-never-in-a-fold";
   `registry.rs::snap_caret_out_of_fold` doc comment) but not runtime-reproduced during
   grounding. The design closes the state regardless; the e2e assert pins visibility, not the
   typing consequence.
2. **"Neither a code block nor a table nests inside the other" (B14 precedence tie):**
   background knowledge of pulldown-cmark's GFM block model, not verified in this repo's
   vendored source. Mitigated: the tie is between two rank-0 verbatim roles (either winning is
   acceptable), and the plan adds the reachable-nesting test (table-in-blockquote).
3. **Coverage-absence claims** ("no existing test places the caret inside the indent bytes";
   "no screen-asserting caret test outside cursor-picker"): confirmed for the named tests
   (`e2e_focus_dims_right_rows_under_ventilate_indented` places the caret at `"committee"`,
   inside content; the `cursor_pos` grep) but exhaustive absence is a sweep result, not a
   proof. Harmless if wrong — the new tests are additive.
4. **B10 test-green claims:** the three B10 pins named in §2.4 (plus the adjacent ux-H2 pin
   `derive.rs::caret_on_phantom_line_conceals_last_content_line`, which is not a B10 pin) were
   run green by the grounding agent's own `cargo test`; this author verified the code and
   commits by reading but did not re-run the suite (no-cargo constraint at spec time). The
   effort's normal gates re-run everything.
