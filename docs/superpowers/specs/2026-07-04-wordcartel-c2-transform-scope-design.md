# C2 — transform scope: block-under-caret default + deepest-block snapping + `_buffer` variants

**Status:** CLEAN — Codex spec review ×4 (r4 CLEAN) + Fable5 ×4 (r8 READY; its rounds
compiled probes against the locked pulldown/repar rlibs and corrected the span anatomy,
the gap rule, and the unit refinement's wording — three user ratifications along the
way), 2026-07-04.
**Effort:** C2 (backlog Theme C; `settled-design` · Medium — both decisions user-resolved
2026-07-03, design approved 2026-07-04)
**Date:** 2026-07-04 · **Facts as of:** `d1608d7` (post-C4 merge)

## Why

All three transforms (Reflow/Unwrap/Ventilate) share `region_for_transform`
(transform.rs:84-92): WITH a selection the region snaps outward to whole TOP-LEVEL blocks;
WITHOUT one it is the FULL BUFFER (`0..buf_len`). Two failures of proportionality: the
least input (bare ctrl-t r) produces the largest effect (an accidental whole-document
Ventilate is a massive surprise diff), and a selection inside ONE list item transforms the
ENTIRE list (top-level snap reaches the `List` container, not the `ListItem`).

## Decisions (user-resolved 2026-07-03; design approved 2026-07-04)

1. **Empty selection → the deepest block under the caret** (never the whole buffer).
   Whole-document intent becomes explicit: `reflow_buffer` / `unwrap_buffer` /
   `ventilate_buffer` commands (palette + Format menu).
2. **Snapping targets the DEEPEST enclosing block(s), not top-level** — a selection inside
   one list item transforms just that item; spanning three items touches exactly those
   three. Applies equally to the caret default.

## Grounding facts the design leans on (2026-07-04 map)

- **The deepest lookup exists:** `deepest_block_at(block, pos) -> Option<&Block>`
  (nav.rs:641-651, currently private) recursively prefers the deepest child containing
  `pos`; `paragraph_range_at` (nav.rs:656-686) already uses it for text-objects, with a
  blank-line-gap fallback when no block contains `pos`.
- **Span anatomy, probe-verified (Fable r5, compiled against the locked pulldown/repar
  rlibs):** (a) NESTED `ListItem` spans EXCLUDE their leading indent (`"  - inner…"`
  spans from the `-`, not the line start) — a unit slice must be extended to
  `line_start(span.start)` or repar reflows at the wrong content column; (b) loose-list
  inter-item blanks live INSIDE the PRECEDING item's span (unit spans may carry trailing
  blank lines — harmless, repar round-trips them; tests must expect it); (c) container-
  interior bytes include NON-BLANK content: a loose item's own marker bytes sit outside
  its Paragraph child, and a tight item's lead text before a nested list has no child at
  all; (d) `Paragraph` children inside `BlockQuote` start after the first `> ` but
  INCLUDE `> ` on continuation lines — a bare quote-paragraph slice mixes prefixes and
  repar passes it through unchanged (a false "already" status).
- **Item-scope marker handling exists:** repar's markdown driver routes list segments to
  `handle_list`/`reflow_item`, which strip the marker/indent, reflow the body at
  `width - content_col`, and re-emit the VERBATIM marker + space-indent continuations.
  Loose items, tab-marked items, and items containing fences/headings/tables pass through
  byte-identical. No new marker handling is needed at item scope.
- **The fragment landmine:** feeding `run_transform` a mid-item FRAGMENT (continuation
  lines without their marker) makes repar classify them as prose and reflow away the
  hanging indent. Every region handed to `run_transform` must therefore be a union of
  WHOLE block spans OR carry a raw GAP endpoint (blank-line territory outside any leaf
  block — harmless to repar, which sees complete blocks plus surrounding blanks; Codex
  r1 M-1's wording fix). D2's endpoint snap and D1's block spans deliver exactly that;
  `ListItem.span` covers marker + continuation lines (but NOT the leading indent of
  nested items — see the span-anatomy bullet and the line-start extension).
- `BlockTree`/`Block` (block_tree.rs:155-173): containers (`List`, `ListItem`,
  `BlockQuote`, …) carry nested children with spans; `top_level()` = root children.
- The only caller of `region_for_transform` is `dispatch_transform` (transform.rs:110);
  all three transforms share it. The async threshold (1 MiB, transform.rs:6), the M4
  panic guard (`guarded_transform`), `build_range_replace`, and the no-op "already …"
  status are downstream and unchanged.

## Design

### D1. The caret default (transform.rs + a shared deepest lookup)

`region_for_transform`'s empty-selection branch (transform.rs:87-88) stops returning
`0..buf_len`. New behavior: the deepest block containing the caret —

- **The lookup is TRANSFORM-SPECIFIC — `deepest_block_at` alone is insufficient (Codex
  r2; mechanics corrected by Fable r5 probes):** the block tree nests `Paragraph`
  leaves INSIDE `ListItem`s (block_tree.rs:261/:405), so the deepest leaf under a caret
  in item-body text is the marker-LESS paragraph span — feeding it to repar IS the
  fragment landmine. New
  `fn transform_unit_at(text, blocks: &BlockTree, pos: usize) -> Option<Range<usize>>`
  in transform.rs (it needs line-start access — the signature carries whatever text/
  line-index handle the plan grounds; `TextSource::line_start` exists,
  block_tree.rs:19-21): descend recording the path (root → deepest node containing
  `pos`, the half-open `pos < span.end` test), then:
  - **A leaf contains the byte:** the unit is the NEAREST `ListItem` ancestor of that
    leaf (the DEEPEST ListItem on the path — a caret in a sub-item transforms the
    sub-item, per decision 2); if no ListItem is on the path but a `BlockQuote` is, the
    nearest BlockQuote (Fable I2/r5 probe — quote-paragraph spans start after the first
    `> ` but include it on continuations, so a bare paragraph slice mixes prefixes and
    silently no-ops; **ListItem anywhere on the path BEATS BlockQuote**); else the leaf
    itself.
  - **The descent ends at a CONTAINER (no child contains the byte):** discriminate by
    LINE BLANKNESS (Fable r5 C2 — container-interior bytes include real content): if
    the byte's LINE is blank → None (gap; preserves every approved gap outcome); if the
    line is NON-blank (a loose item's own marker bytes; a tight item's lead text before
    its nested list; a top-level quote's own `> ` prefix bytes) → the SAME preference
    set as the leaf branch (Fable r6 N2): nearest `ListItem` on the path, else nearest
    `BlockQuote`, else None — **with ONE refinement (Fable r6 N5, user-ratified A; wording line-keyed per r7
    P1): if the first non-WHITESPACE content of the byte's LINE begins a `ListItem` block
    (Fable r8 Q1 — tab-indented items count) —
    at ANY depth beneath the descent's final node (the first nested item's indent ends
    the descent at the OUTER Item, where the target is a grandchild; List spans exclude
    leading indent too) — the unit is THAT ListItem. Home-then-transform acts on the
    item the eye is on, per decision 2's deepest-item principle, not the outer item the
    indent structurally belongs to.** (The
    degraded-parse derivation in Non-goals stays exact: the childless root has no
    ListItem, no BlockQuote, no child on the line → None.)
  - **Every returned unit span extends to `line_start(span.start)`** (Fable r5 C1:
    nested-item and quote spans EXCLUDE leading indent/prefix bytes; without the
    extension the slice starts mid-line and repar reflows at the wrong content column).
  `deepest_block_at` in nav.rs stays untouched (text-objects keep their semantics).
- `region_for_transform(doc)` with an empty selection: `transform_unit_at(text, blocks, caret)`
  (the primary head, clamped to `buf_len.saturating_sub(1)` for the end-of-buffer
  caret); if found, return its span. If `transform_unit_at` returns None (a top-level
  blank-line gap OR a container-interior gap — Codex r3 wording): return the EMPTY range
  `caret..caret` — `dispatch_transform` ALREADY guards empty ranges and returns the
  status `"nothing to transform"` before `run_transform` runs (transform.rs:111-113 — Codex
  r1 corrected the original claim, which cited the identical-output "already …" path;
  NO new code, the existing guard is the no-op). (Rationale: a caret on a blank line has
  no block intent; doing nothing loudly beats guessing. This diverges from
  `paragraph_range_at`'s gap-run fallback deliberately — that fallback serves
  text-object selection, where selecting a blank run is meaningful; transforming one
  is not.)

A caret inside a code fence returns the fence's span; repar passes fences through
verbatim, so the result is the honest "already reflowed" no-op — acceptable and pinned.

### D2. Endpoint-deepest snapping (transform.rs)

`snap_to_blocks` (transform.rs:62-80) is REPLACED (same name, same signature, new
semantics — the four `snap_*` tests update): each ENDPOINT snaps to its deepest enclosing
block, the interior stays.

- `start` = `transform_unit_at(text, blocks, from)` → its `span.start` (the SAME unit
  lookup as D1, all rules included — ancestor preference, line-blank gaps, line-start
  extension); if `from` sits in a gap, `start = from` (raw, today's fallback behavior).
- `end` = `transform_unit_at(text, blocks, to.saturating_sub(1))` (the last selected
  byte — `to` is exclusive) → its `span.end`; gap → `end = to`.
- **"Gap" = container-interior BLANK-LINE bytes (r1 I-3 → r3 → corrected by Fable r5's
  anatomy):** the probe-true shapes: a loose item's trailing blank lives INSIDE that
  item's span (the descent still ends at the Item container; the BLANK line makes it a
  gap → raw endpoint, which coincides with the item's span end in the single-blank
  shape — one rule, one implementation; Fable r6 N1); the
  genuinely container-interior bytes are the next marker's leading indent and true
  inter-structure blanks. When the descent ends at a container and the byte's line is
  BLANK, `transform_unit_at` returns None → raw endpoint, never a container span
  (otherwise a selection into structural blank territory pulls in whole containers —
  the surprise-diff class decision 1 exists to kill). When the line is NON-blank (Fable
  C2's marker-bytes / lead-text shapes), D1's container branch governs IN FULL — the
  preference set plus the N5 line-keyed refinement; this bullet is a pointer, not a
  divergent endpoint rule (Fable r7 P2). Gap-means-gap applies to BLANKS regardless of
  ancestors; content always finds its unit. Pinned by loose-list, nested-list blank, marker-byte, and lead-text tests.
- Return `start..end`. Degenerate guard: if the computed range is empty or inverted
  (unreachable with a non-empty selection, but SATURATING discipline applies), return
  `from..to`.

Consequences, each pinned by a test: a selection inside one item → that item's span; a
selection spanning items 1-3 of one list → exactly items 1-3 (interior item 2 rides
between the snapped endpoints); a selection from a paragraph into a list → the paragraph's
start through the touched item's end; a selection wholly inside a gap → unchanged
(`from..to`, today's fallback). Every region is a union of whole TRANSFORM-UNIT spans
plus raw gap endpoints (Codex r3 wording — the fragment landmine is unreachable: gap
bytes are blank-line territory, not partial blocks).

### D3. The `_buffer` variants (registry.rs + transform.rs)

Three new registry entries beside the existing three (registry.rs:285-295), same
`MenuCategory::Format`, labels `Reflow Buffer` / `Unwrap Buffer` / `Ventilate Buffer`,
ids `reflow_buffer` / `unwrap_buffer` / `ventilate_buffer`. They dispatch through an
EXPLICIT-REGION parameter on the existing `dispatch_transform` (Fable I3 — the
`transform_in_flight` and empty-range guards live UPSTREAM in that fn, transform.rs:
106-113; a fresh sibling fn would bypass both, double-dispatching async transforms and
mis-reporting empty buffers) whose region is `0..buf_len` unconditionally. Both guards
apply identically to both scopes — pinned by `buffer_variant_rejected_while_in_flight`
and `buffer_variant_on_empty_buffer_says_nothing_to_transform` (Fable r6 N6). Everything downstream (async threshold, guard, merge, staleness) is the
shared path. The 1 MiB async route remains reachable from BOTH scopes — any single
block ≥ 1 MiB routes async under the caret default (Codex r1) — the `_buffer` variants
merely make it the common case.

**The ctrl-t chooser is UNCHANGED** (prompt.rs:115-125: `r`/`u`/`v`): its keys now mean
block scope, which IS the new default — the least input produces the proportionate
effect. Buffer scope is a deliberate, named palette/menu action (per decision 1's text).
The chooser's prompt string is also unchanged.

### D4. What does NOT change

- The chooser keys/prompt; `ctrl-t`; the `transform` command id.
- `run_transform`, `guarded_transform`, `merge_transform_into`, `apply_transform_done`,
  the async threshold and staleness discipline, `MAX_TRANSFORM_OUTPUT`.
- `DEFAULT_REFLOW_WIDTH = 72` stays decoupled from `wrap_column` (a pre-existing quirk,
  recorded as out of scope).
- `paragraph_range_at` and every text-object consumer of `deepest_block_at` — untouched
  (no hoist; `transform_unit_at` is a separate transform-owned walk).
- No keybindings; no core (`wordcartel-core`) changes — `Block`/`BlockTree` already
  expose everything needed.

## Testing

**Existing tests whose meaning changes (sanctioned, enumerate-and-say-loudly):**
- `reflow_whole_buffer_applies_one_undoable_edit` (app.rs:2699) → becomes the
  `reflow_buffer`-variant test (same assertions, driven by the new command; its
  single-paragraph corpus would pass unchanged either way — Fable M6), and a NEW
  sibling with a MULTI-BLOCK corpus pins the ctrl-t default acting on the caret block
  only (a single-block corpus cannot discriminate).
- `transform_with_identical_output_makes_no_edit` (app.rs:2715) → unchanged assertions;
  its single-block corpus means the caret default reaches the same region (verify, note
  in the report).
- `large_buffer_routes_async_and_delivers_transformdone` (app.rs:2767) → UNCHANGED
  (Codex r1 corrected the original claim: its corpus is one giant no-newline paragraph
  — `"word ".repeat(300_000)` — so the caret default's deepest block IS the ≥1 MiB
  region and the async route fires exactly as today; the test becomes a pin that
  single-huge-block caret transforms still route async). A NEW small test additionally
  pins that `reflow_buffer` reaches the async path on the same corpus shape.
- The four `snap_*` tests (transform.rs:230-281) → updated to endpoint-deepest semantics
  (the mid-paragraph and fence cases assert the SAME spans as today — paragraphs and
  fences are leaf blocks at top level; only the multi-block case's phrasing and a NEW
  nested case change expectations).

**New pins:**
- transform.rs unit: `transform_unit_in_item_body_is_the_item_not_the_paragraph` (**the
  Codex r2 pin — the marker rides along**: caret in item-body text → the ListItem span
  including the marker, NOT the nested Paragraph leaf); `transform_unit_in_nested_item_is_the_deepest_item`
  (a sub-item's caret → the sub-ListItem, not the outer item);
  `snap_endpoint_on_loose_list_blank_is_gap_not_container` (the I-3
  rule: selection from mid-item-1 to an inter-item blank → end stays raw, later items
  untouched); `snap_endpoint_on_nested_list_interitem_blank_is_gap` (the r3 shape: the
  blank between a NESTED list's items — inside the outer ListItem — stays raw, the
  outer item is NOT pulled in); `snap_inside_one_list_item_touches_only_that_item`;
  `snap_across_three_items_touches_exactly_those`; `snap_paragraph_into_list_unions_endpoints`;
  `snap_selection_wholly_in_gap_returns_input`; `caret_region_is_the_transform_unit`
  (item + nested-item + paragraph cases — deliberately NOT the deepest block for item
  bodies; Fable M5); `caret_region_in_gap_is_empty`;
  `caret_region_at_end_of_buffer_clamps` (the `buf_len` caret).
- Behavior: `caret_reflow_inside_item_preserves_siblings` (three-item list, caret in
  item 2, reflow → items 1 and 3 byte-identical, item 2 rewrapped with marker + hanging
  indent intact); `caret_reflow_inside_NESTED_item_preserves_indent` (**Fable C1's
  behavior pin**: the unit extends to line start; the nested marker's indent and the
  4-column continuations survive the round trip); `caret_on_loose_item_marker_transforms_the_item`
  and `caret_in_tight_item_lead_text_transforms_the_item` (**Fable C2's content-not-gap
  pins**); `caret_in_nested_item_indent_transforms_the_child_item` (**the r6 N5
  refinement's pin**: Home on a `  - inner` line → the INNER item's unit — corpus must
  include the FIRST-nested-item shape, the r7 P1 case, a space-indented TOP-LEVEL
  item `" - a"`, and a TAB-indented item (r8 Q1) — all resolving to the item); `caret_reflow_on_blank_line_noops_with_status` (status `"nothing to transform"` — the
  existing empty-range guard);
  `caret_reflow_in_fence_noops` (verbatim pass-through); `buffer_variants_act_whole_buffer`
  (one of the three suffices for region proof + the registry test extends to six
  commands); the fragment-safety invariant is structural, pinned indirectly by the
  sibling-preservation test.
- registry.rs: `transforms_are_registered_commands_in_format_category` (registry.rs:573)
  extends to the six ids/labels.
- The D3 guard pins (Fable r7 P3, named here so the plan's enumeration carries them):
  `buffer_variant_rejected_while_in_flight`;
  `buffer_variant_on_empty_buffer_says_nothing_to_transform`.
- e2e journey: a three-item list document, caret into item 2, ctrl-t → `r` → only item 2
  changes on screen; then palette-dispatch `Reflow Buffer` → the whole document rewraps.

**Gates:** the standard set — suite green, workspace clippy deny clean, warning-free;
smoke quoted verbatim pre-merge (advisory) + a live tmux sanity (the e2e journey's script
performed by hand at a narrow width).

## Non-goals (explicit)

- No chooser changes; no keybindings; no core changes.
- No `wrap_column`-aware reflow width (pre-existing decoupling stands; a future effort
  may revisit).
- No multi-cursor/secondary-selection semantics (transforms already use the primary only).
- No change to loose-item/fence/table pass-through behavior inside repar. HtmlBlock/
  HtmlComment units reflow as prose (probe-verified) — the identical pre-existing
  behavior at whole-document scope; not "fixed" here (Fable M3).
- Degraded-parse regression, accepted and stated (Fable M2): under the `empty_tree`
  fallback (block_tree.rs:333-335, childless root) every caret byte is container-
  interior on a non-blank line with NO ListItem → None → "nothing to transform" (today:
  whole-buffer transform). Acceptable — a degraded parse should not gate a
  whole-document rewrite on one keypress; the `_buffer` variants remain available.

## Ship-time bookkeeping

Backlog: C2 → SHIPPED (note the gap-caret no-op convention and the chooser-means-block
semantics); working order advances (next = D1+A5). Memory: working-order tick. Ledger:
standard per-task lines.
