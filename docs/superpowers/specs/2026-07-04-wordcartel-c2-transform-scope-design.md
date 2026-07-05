# C2 — transform scope: block-under-caret default + deepest-block snapping + `_buffer` variants

**Status:** draft — pending Codex + Fable spec review
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
  `pos`; `paragraph_range_at` (nav.rs:656-700) already uses it for text-objects, with a
  blank-line-gap fallback when no block contains `pos`.
- **Item-scope marker handling exists:** repar's markdown driver routes list segments to
  `handle_list`/`reflow_item`, which strip the marker/indent, reflow the body at
  `width - content_col`, and re-emit the VERBATIM marker + space-indent continuations.
  Loose items, tab-marked items, and items containing fences/headings/tables pass through
  byte-identical. No new marker handling is needed at item scope.
- **The fragment landmine:** feeding `run_transform` a mid-item FRAGMENT (continuation
  lines without their marker) makes repar classify them as prose and reflow away the
  hanging indent. Every region handed to `run_transform` must therefore be a union of
  WHOLE block spans — the design guarantees this structurally (D2's endpoint snap and
  D1's block spans are always whole `Block.span`s; `ListItem.span` covers marker +
  continuation lines).
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

- Hoist `deepest_block_at` from nav.rs private scope to a shared `pub(crate)` home the
  design leaves to the plan (either `pub(crate)` in nav.rs consumed via
  `crate::nav::deepest_block_at`, or moved beside `snap_to_blocks` — one copy, no
  duplication; `paragraph_range_at` keeps using the same fn).
- `region_for_transform(doc)` with an empty selection: find the deepest block containing
  `caret` (the primary head, clamped to `buf_len.saturating_sub(1)` for the end-of-buffer
  caret — `deepest_block_at` uses a half-open `pos < span.end` test); if found, return
  its span. If NO block contains the caret (a blank-line gap): return the EMPTY range
  `caret..caret` — the transform then no-ops via the existing identical-output path with
  the "already …" status. (Rationale: a caret on a blank line has no block intent; doing
  nothing loudly beats guessing. This diverges from `paragraph_range_at`'s gap-run
  fallback deliberately — that fallback serves text-object selection, where selecting a
  blank run is meaningful; transforming one is not.)

A caret inside a code fence returns the fence's span; repar passes fences through
verbatim, so the result is the honest "already reflowed" no-op — acceptable and pinned.

### D2. Endpoint-deepest snapping (transform.rs)

`snap_to_blocks` (transform.rs:62-80) is REPLACED (same name, same signature, new
semantics — the four `snap_*` tests update): each ENDPOINT snaps to its deepest enclosing
block, the interior stays.

- `start` = the deepest block containing `from` → its `span.start`; if `from` sits in a
  gap, `start = from` (raw, today's fallback behavior).
- `end` = the deepest block containing `to.saturating_sub(1)` (the last selected byte —
  `to` is exclusive) → its `span.end`; gap → `end = to`.
- Return `start..end`. Degenerate guard: if the computed range is empty or inverted
  (unreachable with a non-empty selection, but SATURATING discipline applies), return
  `from..to`.

Consequences, each pinned by a test: a selection inside one item → that item's span; a
selection spanning items 1-3 of one list → exactly items 1-3 (interior item 2 rides
between the snapped endpoints); a selection from a paragraph into a list → the paragraph's
start through the touched item's end; a selection wholly inside a gap → unchanged
(`from..to`, today's fallback). Whole-span endpoints keep every region a union of whole
block spans (the fragment landmine is unreachable).

### D3. The `_buffer` variants (registry.rs + transform.rs)

Three new registry entries beside the existing three (registry.rs:285-295), same
`MenuCategory::Format`, labels `Reflow Buffer` / `Unwrap Buffer` / `Ventilate Buffer`,
ids `reflow_buffer` / `unwrap_buffer` / `ventilate_buffer`. They dispatch through a new
`dispatch_transform_buffer(editor, kind, clock, msg_tx)` (or an explicit-region parameter
on the existing dispatch — the plan picks the smaller diff) whose region is `0..buf_len`
unconditionally. Everything downstream (async threshold, guard, merge, staleness) is the
shared path — the 1 MiB async route now belongs to the `_buffer` variants in practice.

**The ctrl-t chooser is UNCHANGED** (prompt.rs:115-130: `r`/`u`/`v`): its keys now mean
block scope, which IS the new default — the least input produces the proportionate
effect. Buffer scope is a deliberate, named palette/menu action (per decision 1's text).
The chooser's prompt string is also unchanged.

### D4. What does NOT change

- The chooser keys/prompt; `ctrl-t`; the `transform` command id.
- `run_transform`, `guarded_transform`, `merge_transform_into`, `apply_transform_done`,
  the async threshold and staleness discipline, `MAX_TRANSFORM_OUTPUT`.
- `DEFAULT_REFLOW_WIDTH = 72` stays decoupled from `wrap_column` (a pre-existing quirk,
  recorded as out of scope).
- `paragraph_range_at` and every text-object consumer of `deepest_block_at` — behavior
  identical (the hoist is visibility-only).
- No keybindings; no core (`wordcartel-core`) changes — `Block`/`BlockTree` already
  expose everything needed.

## Testing

**Existing tests whose meaning changes (sanctioned, enumerate-and-say-loudly):**
- `reflow_whole_buffer_applies_one_undoable_edit` (app.rs:2699) → becomes the
  `reflow_buffer`-variant test (same assertions, driven by the new command), and a NEW
  sibling pins the ctrl-t default acting on the caret block only.
- `transform_with_identical_output_makes_no_edit` (app.rs:2715) → unchanged assertions;
  its single-block corpus means the caret default reaches the same region (verify, note
  in the report).
- `large_buffer_routes_async_and_delivers_transformdone` (app.rs:2767) → drives
  `reflow_buffer` (the empty-selection default no longer reaches 1 MiB); same async
  assertions.
- The four `snap_*` tests (transform.rs:230-281) → updated to endpoint-deepest semantics
  (the mid-paragraph and fence cases assert the SAME spans as today — paragraphs and
  fences are leaf blocks at top level; only the multi-block case's phrasing and a NEW
  nested case change expectations).

**New pins:**
- transform.rs unit: `snap_inside_one_list_item_touches_only_that_item`;
  `snap_across_three_items_touches_exactly_those`; `snap_paragraph_into_list_unions_endpoints`;
  `snap_selection_wholly_in_gap_returns_input`; `caret_region_is_deepest_block`
  (item + nested-item + paragraph cases); `caret_region_in_gap_is_empty`;
  `caret_region_at_end_of_buffer_clamps` (the `buf_len` caret).
- Behavior: `caret_reflow_inside_item_preserves_siblings` (three-item list, caret in
  item 2, reflow → items 1 and 3 byte-identical, item 2 rewrapped with marker + hanging
  indent intact); `caret_reflow_on_blank_line_noops_with_status` ("already …");
  `caret_reflow_in_fence_noops` (verbatim pass-through); `buffer_variants_act_whole_buffer`
  (one of the three suffices for region proof + the registry test extends to six
  commands); the fragment-safety invariant is structural, pinned indirectly by the
  sibling-preservation test.
- registry.rs: `transforms_are_registered_commands_in_format_category` (registry.rs:573)
  extends to the six ids/labels.
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
- No change to loose-item/fence/table pass-through behavior inside repar.

## Ship-time bookkeeping

Backlog: C2 → SHIPPED (note the gap-caret no-op convention and the chooser-means-block
semantics); working order advances (next = D1+A5). Memory: working-order tick. Ledger:
standard per-task lines.
